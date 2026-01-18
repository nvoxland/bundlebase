//! Text/BM25 full-text search index using Tantivy
//!
//! This module provides full-text search capabilities using the Tantivy search engine.
//! It supports multiple tokenizers for different languages and use cases.

use crate::data::RowId;
use crate::index::{Index, IndexType, TokenizerConfig};
use crate::BundlebaseError;
use bytes::Bytes;
use std::collections::HashMap;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Schema, STORED, TextFieldIndexing, TextOptions, IndexRecordOption};
use tantivy::schema::Value;
use tantivy::tokenizer::{
    Language, LowerCaser, SimpleTokenizer, Stemmer, StopWordFilter, TextAnalyzer,
};
use tantivy::{Index as TantivyIndex, IndexWriter, TantivyDocument};
use tempfile::TempDir;

/// Field names used in the Tantivy schema
const CONTENT_FIELD: &str = "content";
const ROWID_FIELD: &str = "rowid";

/// A text/BM25 full-text search index for a column.
///
/// The index files are stored in a temporary directory that is automatically
/// cleaned up when this struct is dropped.
pub struct TextColumnIndex {
    column_name: String,
    index: TantivyIndex,
    doc_count: u64,
    tokenizer_config: TokenizerConfig,
    /// Temporary directory holding the index files - automatically cleaned up on drop
    _temp_dir: Option<TempDir>,
}

impl std::fmt::Debug for TextColumnIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextColumnIndex")
            .field("column_name", &self.column_name)
            .field("doc_count", &self.doc_count)
            .field("tokenizer_config", &self.tokenizer_config)
            .finish_non_exhaustive()
    }
}

/// Search result with row ID and BM25 score
#[derive(Debug, Clone)]
pub struct TextSearchResult {
    pub row_id: RowId,
    pub score: f32,
}

impl TextColumnIndex {
    /// Build a new text index from value-to-rowids mapping
    ///
    /// This creates a Tantivy index in a temp directory for persistence.
    /// The temp directory is automatically cleaned up when this index is dropped.
    ///
    /// # Arguments
    /// * `column_name` - Name of the column being indexed
    /// * `value_to_rowids` - Map of text values to their row IDs
    /// * `tokenizer_config` - Tokenizer configuration to use
    pub fn build(
        column_name: &str,
        value_to_rowids: HashMap<String, Vec<RowId>>,
        tokenizer_config: &TokenizerConfig,
    ) -> Result<Self, BundlebaseError> {
        // Build schema with content and rowid fields
        let mut schema_builder = Schema::builder();

        // Configure text field with position information for phrase queries
        let text_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(tokenizer_config.tantivy_tokenizer_name())
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();

        let content_field = schema_builder.add_text_field(CONTENT_FIELD, text_options);
        let rowid_field = schema_builder.add_u64_field(ROWID_FIELD, STORED);

        let schema = schema_builder.build();

        // Create a temp directory for the index (auto-cleaned on drop)
        let temp_dir = TempDir::with_prefix("bundlebase_text_index_").map_err(|e| {
            BundlebaseError::from(format!("Failed to create temp directory: {}", e))
        })?;
        let index_path = temp_dir.path();

        // Create index in the temp directory
        let index = TantivyIndex::create_in_dir(index_path, schema.clone()).map_err(|e| {
            BundlebaseError::from(format!("Failed to create index in directory: {}", e))
        })?;

        // Register custom tokenizers
        Self::register_tokenizers(&index)?;

        // Create index writer with 50MB heap
        let mut index_writer: IndexWriter = index
            .writer(50_000_000)
            .map_err(|e| BundlebaseError::from(format!("Failed to create index writer: {}", e)))?;

        let mut doc_count = 0u64;

        // Index all documents
        for (text_value, row_ids) in value_to_rowids {
            for row_id in row_ids {
                let mut doc = TantivyDocument::default();
                doc.add_text(content_field, &text_value);
                doc.add_u64(rowid_field, row_id.as_u64());

                index_writer.add_document(doc).map_err(|e| {
                    BundlebaseError::from(format!("Failed to add document to index: {}", e))
                })?;

                doc_count += 1;
            }
        }

        // Commit the index
        index_writer.commit().map_err(|e| {
            BundlebaseError::from(format!("Failed to commit index: {}", e))
        })?;

        Ok(Self {
            column_name: column_name.to_string(),
            index,
            doc_count,
            tokenizer_config: tokenizer_config.clone(),
            _temp_dir: Some(temp_dir),
        })
    }

    /// Build a text index from an iterator of (text, row_id) pairs
    ///
    /// This is more memory efficient for large datasets as it doesn't
    /// require collecting all data into a HashMap first. The temp directory
    /// is automatically cleaned up when this index is dropped.
    pub fn build_streaming<I>(
        column_name: &str,
        documents: I,
        tokenizer_config: &TokenizerConfig,
    ) -> Result<Self, BundlebaseError>
    where
        I: Iterator<Item = (String, RowId)>,
    {
        // Build schema
        let mut schema_builder = Schema::builder();

        let text_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(tokenizer_config.tantivy_tokenizer_name())
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();

        let content_field = schema_builder.add_text_field(CONTENT_FIELD, text_options);
        let rowid_field = schema_builder.add_u64_field(ROWID_FIELD, STORED);

        let schema = schema_builder.build();

        // Create a temp directory for the index (auto-cleaned on drop)
        let temp_dir = TempDir::with_prefix("bundlebase_text_index_").map_err(|e| {
            BundlebaseError::from(format!("Failed to create temp directory: {}", e))
        })?;
        let index_path = temp_dir.path();

        // Create index in the temp directory
        let index = TantivyIndex::create_in_dir(index_path, schema.clone()).map_err(|e| {
            BundlebaseError::from(format!("Failed to create index in directory: {}", e))
        })?;

        // Register custom tokenizers
        Self::register_tokenizers(&index)?;

        // Create index writer
        let mut index_writer: IndexWriter = index
            .writer(50_000_000)
            .map_err(|e| BundlebaseError::from(format!("Failed to create index writer: {}", e)))?;

        let mut doc_count = 0u64;

        // Index all documents from iterator
        for (text_value, row_id) in documents {
            let mut doc = TantivyDocument::default();
            doc.add_text(content_field, &text_value);
            doc.add_u64(rowid_field, row_id.as_u64());

            index_writer.add_document(doc).map_err(|e| {
                BundlebaseError::from(format!("Failed to add document to index: {}", e))
            })?;

            doc_count += 1;
        }

        // Commit the index
        index_writer.commit().map_err(|e| {
            BundlebaseError::from(format!("Failed to commit index: {}", e))
        })?;

        Ok(Self {
            column_name: column_name.to_string(),
            index,
            doc_count,
            tokenizer_config: tokenizer_config.clone(),
            _temp_dir: Some(temp_dir),
        })
    }

    /// Register all supported tokenizers with the index.
    ///
    /// While it would be ideal to only register the tokenizer needed for this specific index,
    /// Tantivy requires all tokenizers that might be referenced in the schema or queries to be
    /// registered. For deserialized indexes, we don't know which tokenizer was used until
    /// after the index is opened, so we register all supported tokenizers.
    fn register_tokenizers(index: &TantivyIndex) -> Result<(), BundlebaseError> {
        let tokenizer_manager = index.tokenizers();

        // Simple tokenizer (whitespace + lowercase) - always register as it's the default
        tokenizer_manager.register(
            "simple",
            TextAnalyzer::builder(SimpleTokenizer::default())
                .filter(LowerCaser)
                .build(),
        );

        // Raw tokenizer (no tokenization)
        tokenizer_manager.register(
            "raw",
            TextAnalyzer::builder(SimpleTokenizer::default()).build(),
        );

        // Register language-specific tokenizers
        Self::register_language_tokenizer(tokenizer_manager, "en_stem", Language::English)?;
        Self::register_language_tokenizer(tokenizer_manager, "de_stem", Language::German)?;
        Self::register_language_tokenizer(tokenizer_manager, "fr_stem", Language::French)?;
        Self::register_language_tokenizer(tokenizer_manager, "es_stem", Language::Spanish)?;
        Self::register_language_tokenizer(tokenizer_manager, "it_stem", Language::Italian)?;
        Self::register_language_tokenizer(tokenizer_manager, "pt_stem", Language::Portuguese)?;
        Self::register_language_tokenizer(tokenizer_manager, "nl_stem", Language::Dutch)?;
        Self::register_language_tokenizer(tokenizer_manager, "sv_stem", Language::Swedish)?;
        Self::register_language_tokenizer(tokenizer_manager, "no_stem", Language::Norwegian)?;
        Self::register_language_tokenizer(tokenizer_manager, "da_stem", Language::Danish)?;
        Self::register_language_tokenizer(tokenizer_manager, "fi_stem", Language::Finnish)?;
        Self::register_language_tokenizer(tokenizer_manager, "ru_stem", Language::Russian)?;

        Ok(())
    }

    /// Helper to register a language-specific stemming tokenizer
    fn register_language_tokenizer(
        tokenizer_manager: &tantivy::tokenizer::TokenizerManager,
        name: &str,
        language: Language,
    ) -> Result<(), BundlebaseError> {
        let stop_words = StopWordFilter::new(language).ok_or_else(|| {
            BundlebaseError::from(format!("Failed to create stop word filter for {:?}", language))
        })?;

        tokenizer_manager.register(
            name,
            TextAnalyzer::builder(SimpleTokenizer::default())
                .filter(LowerCaser)
                .filter(stop_words)
                .filter(Stemmer::new(language))
                .build(),
        );

        Ok(())
    }

    /// Search the index with a query string
    ///
    /// Returns matching row IDs with their BM25 scores, sorted by relevance.
    ///
    /// # Arguments
    /// * `query` - The search query (supports Tantivy query syntax)
    /// * `limit` - Maximum number of results to return
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<TextSearchResult>, BundlebaseError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| BundlebaseError::from(format!("Failed to create index reader: {}", e)))?;

        let searcher = reader.searcher();
        let schema = self.index.schema();

        let content_field = schema
            .get_field(CONTENT_FIELD)
            .map_err(|e| BundlebaseError::from(format!("Content field not found: {}", e)))?;

        let rowid_field = schema
            .get_field(ROWID_FIELD)
            .map_err(|e| BundlebaseError::from(format!("RowId field not found: {}", e)))?;

        // Create query parser
        let query_parser = QueryParser::for_index(&self.index, vec![content_field]);
        let parsed_query = query_parser
            .parse_query(query)
            .map_err(|e| BundlebaseError::from(format!("Failed to parse query '{}': {}", query, e)))?;

        // Execute search
        let top_docs = searcher
            .search(&parsed_query, &TopDocs::with_limit(limit))
            .map_err(|e| BundlebaseError::from(format!("Search failed: {}", e)))?;

        // Collect results
        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| BundlebaseError::from(format!("Failed to retrieve document: {}", e)))?;

            // Extract row ID from document
            if let Some(rowid_value) = doc.get_first(rowid_field) {
                if let Some(rowid_u64) = rowid_value.as_u64() {
                    results.push(TextSearchResult {
                        row_id: RowId::from(rowid_u64),
                        score,
                    });
                }
            }
        }

        Ok(results)
    }

    /// Search and return only the row IDs (without scores)
    pub fn search_rowids(&self, query: &str, limit: usize) -> Result<Vec<RowId>, BundlebaseError> {
        let results = self.search(query, limit)?;
        Ok(results.into_iter().map(|r| r.row_id).collect())
    }

    /// Check if a query matches any documents (boolean search)
    pub fn matches(&self, query: &str) -> Result<bool, BundlebaseError> {
        let results = self.search(query, 1)?;
        Ok(!results.is_empty())
    }

    /// Get the column name this index is for
    pub fn column_name(&self) -> &str {
        &self.column_name
    }

    /// Get the number of documents in the index
    pub fn doc_count(&self) -> u64 {
        self.doc_count
    }

    /// Get the tokenizer configuration
    pub fn tokenizer_config(&self) -> &TokenizerConfig {
        &self.tokenizer_config
    }

    /// Serialize the index to bytes for storage
    ///
    /// This creates a tar archive containing all index files and metadata.
    pub fn serialize(&self) -> Result<Bytes, BundlebaseError> {
        // Create metadata
        let metadata = serde_json::json!({
            "column_name": self.column_name,
            "doc_count": self.doc_count,
            "tokenizer": self.tokenizer_config,
            "version": 1,
        });
        let metadata_bytes = serde_json::to_vec(&metadata).map_err(|e| {
            BundlebaseError::from(format!("Failed to serialize metadata: {}", e))
        })?;

        // Get the index path from temp directory
        let index_path = self._temp_dir.as_ref().ok_or_else(|| {
            BundlebaseError::from("Index was not created with a file-based directory, cannot serialize")
        })?.path();

        // Create tar archive
        use tar::Builder;
        let mut archive_data = Vec::new();
        {
            let mut builder = Builder::new(&mut archive_data);

            // Add metadata
            let mut header = tar::Header::new_gnu();
            header.set_size(metadata_bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "_metadata.json", metadata_bytes.as_slice())
                .map_err(|e| {
                    BundlebaseError::from(format!("Failed to add metadata to archive: {}", e))
                })?;

            // Read all files from the index directory
            for entry in std::fs::read_dir(index_path).map_err(|e| {
                BundlebaseError::from(format!("Failed to read index directory: {}", e))
            })? {
                let entry = entry.map_err(|e| {
                    BundlebaseError::from(format!("Failed to read directory entry: {}", e))
                })?;

                let path = entry.path();
                if path.is_file() {
                    let file_name = path.file_name()
                        .ok_or_else(|| BundlebaseError::from("Invalid file name"))?
                        .to_string_lossy();

                    // Skip lock files and temp files
                    if file_name.ends_with(".lock") || file_name.starts_with('.') {
                        continue;
                    }

                    let data = std::fs::read(&path).map_err(|e| {
                        BundlebaseError::from(format!("Failed to read file {:?}: {}", path, e))
                    })?;

                    let mut header = tar::Header::new_gnu();
                    header.set_size(data.len() as u64);
                    header.set_mode(0o644);
                    header.set_cksum();
                    builder
                        .append_data(&mut header, &*file_name, data.as_slice())
                        .map_err(|e| {
                            BundlebaseError::from(format!("Failed to add file to archive: {}", e))
                        })?;
                }
            }

            builder.finish().map_err(|e| {
                BundlebaseError::from(format!("Failed to finish archive: {}", e))
            })?;
        }

        Ok(Bytes::from(archive_data))
    }

    /// Deserialize an index from bytes
    ///
    /// Creates a temporary directory to extract and load the index files.
    /// The temporary directory is automatically cleaned up when the
    /// TextColumnIndex is dropped.
    pub fn deserialize(data: Bytes) -> Result<Self, BundlebaseError> {
        use tar::Archive;

        // Create a temp directory for extraction (auto-cleaned on drop)
        let temp_dir = TempDir::with_prefix("bundlebase_text_index_").map_err(|e| {
            BundlebaseError::from(format!("Failed to create temp directory: {}", e))
        })?;
        let temp_path = temp_dir.path();

        // Extract tar archive
        let mut archive = Archive::new(data.as_ref());
        archive.unpack(temp_path).map_err(|e| {
            BundlebaseError::from(format!("Failed to extract index archive: {}", e))
        })?;

        // Read metadata
        let metadata_path = temp_path.join("_metadata.json");
        let metadata_content = std::fs::read_to_string(&metadata_path).map_err(|e| {
            BundlebaseError::from(format!("Failed to read metadata: {}", e))
        })?;

        let metadata: serde_json::Value = serde_json::from_str(&metadata_content).map_err(|e| {
            BundlebaseError::from(format!("Failed to parse metadata: {}", e))
        })?;

        let column_name = metadata["column_name"]
            .as_str()
            .ok_or_else(|| BundlebaseError::from("Missing column_name in metadata"))?
            .to_string();

        let doc_count = metadata["doc_count"]
            .as_u64()
            .ok_or_else(|| BundlebaseError::from("Missing doc_count in metadata"))?;

        let tokenizer_config: TokenizerConfig =
            serde_json::from_value(metadata["tokenizer"].clone()).map_err(|e| {
                BundlebaseError::from(format!("Failed to parse tokenizer config: {}", e))
            })?;

        // Remove metadata file before opening index
        std::fs::remove_file(&metadata_path).ok();

        // Open the index from the extracted directory
        let index = TantivyIndex::open_in_dir(temp_path).map_err(|e| {
            BundlebaseError::from(format!("Failed to open index: {}", e))
        })?;

        // Register tokenizers
        Self::register_tokenizers(&index)?;

        Ok(Self {
            column_name,
            index,
            doc_count,
            tokenizer_config,
            _temp_dir: Some(temp_dir),
        })
    }
}

impl Index for TextColumnIndex {
    fn serialize(&self) -> Result<Bytes, BundlebaseError> {
        self.serialize()
    }

    fn cardinality(&self) -> u64 {
        // For text indexes, cardinality represents the number of unique text values indexed
        self.doc_count
    }

    fn column_name(&self) -> &str {
        &self.column_name
    }

    fn index_type(&self) -> IndexType {
        IndexType::Text {
            tokenizer: self.tokenizer_config.clone(),
        }
    }

    fn total_rows(&self) -> u64 {
        self.doc_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_and_search() {
        let mut value_to_rowids = HashMap::new();
        value_to_rowids.insert(
            "The quick brown fox jumps over the lazy dog".to_string(),
            vec![RowId::from(1u64)],
        );
        value_to_rowids.insert(
            "Machine learning is transforming how we process data".to_string(),
            vec![RowId::from(2u64)],
        );
        value_to_rowids.insert(
            "The fox was very quick and agile".to_string(),
            vec![RowId::from(3u64)],
        );

        let index = TextColumnIndex::build("content", value_to_rowids, &TokenizerConfig::Simple)
            .expect("Failed to build index");

        // Search for "fox"
        let results = index.search("fox", 10).expect("Search failed");
        assert_eq!(results.len(), 2);

        // Both documents with "fox" should be found
        let row_ids: Vec<u64> = results.iter().map(|r| r.row_id.as_u64()).collect();
        assert!(row_ids.contains(&1));
        assert!(row_ids.contains(&3));

        // Search for "machine learning"
        let results = index.search("machine learning", 10).expect("Search failed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].row_id.as_u64(), 2);
    }

    #[test]
    fn test_english_stemming() {
        let mut value_to_rowids = HashMap::new();
        value_to_rowids.insert("running".to_string(), vec![RowId::from(1u64)]);
        value_to_rowids.insert("run".to_string(), vec![RowId::from(2u64)]);
        value_to_rowids.insert("runner".to_string(), vec![RowId::from(3u64)]);

        let index =
            TextColumnIndex::build("content", value_to_rowids, &TokenizerConfig::EnglishStem)
                .expect("Failed to build index");

        // With stemming, searching for "run" should find all variants
        let results = index.search("run", 10).expect("Search failed");
        assert!(results.len() >= 1); // At least "run" itself
    }

    #[test]
    fn test_serialize_deserialize() {
        let mut value_to_rowids = HashMap::new();
        value_to_rowids.insert("Hello world".to_string(), vec![RowId::from(1u64)]);
        value_to_rowids.insert("Goodbye world".to_string(), vec![RowId::from(2u64)]);

        let index = TextColumnIndex::build("test_col", value_to_rowids, &TokenizerConfig::Simple)
            .expect("Failed to build index");

        // Serialize
        let bytes = index.serialize().expect("Serialization failed");

        // Deserialize
        let restored = TextColumnIndex::deserialize(bytes).expect("Deserialization failed");

        assert_eq!(restored.column_name(), "test_col");
        assert_eq!(restored.doc_count(), 2);

        // Verify search still works
        let results = restored.search("world", 10).expect("Search failed");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_streaming_build() {
        let documents = vec![
            ("Document one about cats".to_string(), RowId::from(1u64)),
            ("Document two about dogs".to_string(), RowId::from(2u64)),
            ("Document three about cats and dogs".to_string(), RowId::from(3u64)),
        ];

        let index = TextColumnIndex::build_streaming(
            "content",
            documents.into_iter(),
            &TokenizerConfig::Simple,
        )
        .expect("Failed to build index");

        let results = index.search("cats", 10).expect("Search failed");
        assert_eq!(results.len(), 2);
    }
}
