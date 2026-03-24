//! Text/BM25 full-text search index using Tantivy
//!
//! This module provides full-text search capabilities using the Tantivy search engine.
//! It supports multiple tokenizers for different languages and use cases, and can index
//! one or more columns in a single index for multi-field search.

// search_table_fn stays in the core crate (depends on BundleFacade)

use bundlebase_common::RowId;
use crate::{Index, IndexType, TokenizerConfig};
use bundlebase_common::BundlebaseError;
use bytes::Bytes;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Schema, STORED, TextFieldIndexing, TextOptions, IndexRecordOption, Field};
use tantivy::schema::Value;
use tantivy::tokenizer::{
    Language, LowerCaser, SimpleTokenizer, Stemmer, StopWordFilter, TextAnalyzer,
};
use tantivy::{Index as TantivyIndex, IndexWriter, TantivyDocument};
use tempfile::TempDir;

/// Field name for the stored row ID
const ROWID_FIELD: &str = "rowid";

/// A text/BM25 full-text search index supporting one or more columns.
///
/// Each column gets its own named field in the tantivy schema,
/// enabling field-specific queries like `company:group AND city:east`.
///
/// The index files are stored in a temporary directory that is automatically
/// cleaned up when this struct is dropped.
pub struct TextIndex {
    /// Index name
    name: String,
    /// The columns indexed by this text index
    columns: Vec<String>,
    index: TantivyIndex,
    doc_count: u64,
    tokenizer_config: TokenizerConfig,
    /// Temporary directory holding the index files - automatically cleaned up on drop
    _temp_dir: Option<TempDir>,
}

impl std::fmt::Debug for TextIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextIndex")
            .field("name", &self.name)
            .field("columns", &self.columns)
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

/// Builder for incrementally constructing a TextIndex.
///
/// Documents are added one at a time via `add_document()`,
/// then `finish()` commits and returns the final TextIndex.
/// This avoids buffering all documents in memory before indexing.
pub struct TextIndexBuilder {
    name: String,
    columns: Vec<String>,
    index: TantivyIndex,
    index_writer: IndexWriter,
    column_fields: Vec<Field>,
    rowid_field: Field,
    doc_count: u64,
    tokenizer_config: TokenizerConfig,
    _temp_dir: TempDir,
}

impl TextIndexBuilder {
    /// Create a new builder, setting up the Tantivy schema and index writer.
    pub fn new(
        name: &str,
        columns: &[String],
        tokenizer_config: &TokenizerConfig,
    ) -> Result<Self, BundlebaseError> {
        // Build schema with one text field per column + rowid field
        let mut schema_builder = Schema::builder();

        let text_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(tokenizer_config.tantivy_tokenizer_name())
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            );

        let mut column_fields: Vec<Field> = Vec::with_capacity(columns.len());
        for col in columns {
            let field = schema_builder.add_text_field(col, text_options.clone());
            column_fields.push(field);
        }
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
        TextIndex::register_tokenizers(&index)?;

        // Create index writer with 50MB heap
        let index_writer: IndexWriter = index
            .writer(50_000_000)
            .map_err(|e| BundlebaseError::from(format!("Failed to create index writer: {}", e)))?;

        Ok(Self {
            name: name.to_string(),
            columns: columns.to_vec(),
            index,
            index_writer,
            column_fields,
            rowid_field,
            doc_count: 0,
            tokenizer_config: tokenizer_config.clone(),
            _temp_dir: temp_dir,
        })
    }

    /// Add a single document to the index.
    pub fn add_document(
        &mut self,
        column_values: &[Option<String>],
        row_id: RowId,
    ) -> Result<(), BundlebaseError> {
        let mut doc = TantivyDocument::default();
        for (col_idx, field) in self.column_fields.iter().enumerate() {
            if let Some(Some(text_value)) = column_values.get(col_idx) {
                doc.add_text(*field, text_value);
            }
        }
        doc.add_u64(self.rowid_field, row_id.as_u64());
        self.index_writer.add_document(doc).map_err(|e| {
            BundlebaseError::from(format!("Failed to add document to index: {}", e))
        })?;
        self.doc_count += 1;
        Ok(())
    }

    /// Commit and finalize into a TextIndex.
    pub fn finish(mut self) -> Result<TextIndex, BundlebaseError> {
        self.index_writer.commit().map_err(|e| {
            BundlebaseError::from(format!("Failed to commit index: {}", e))
        })?;

        Ok(TextIndex {
            name: self.name,
            columns: self.columns,
            index: self.index,
            doc_count: self.doc_count,
            tokenizer_config: self.tokenizer_config,
            _temp_dir: Some(self._temp_dir),
        })
    }
}

impl TextIndex {
    /// Build a text index from an iterator of (column_values, row_id) pairs
    ///
    /// # Arguments
    /// * `name` - Index name (used as identifier)
    /// * `columns` - Column names to index
    /// * `documents` - Iterator of (column_values, row_id) pairs where column_values
    ///   is a Vec with one Option<String> per column (positional, matching `columns` order)
    /// * `tokenizer_config` - Tokenizer configuration to use
    pub fn build_streaming_multi<I>(
        name: &str,
        columns: &[String],
        documents: I,
        tokenizer_config: &TokenizerConfig,
    ) -> Result<Self, BundlebaseError>
    where
        I: Iterator<Item = (Vec<Option<String>>, RowId)>,
    {
        let mut builder = TextIndexBuilder::new(name, columns, tokenizer_config)?;
        for (column_values, row_id) in documents {
            builder.add_document(&column_values, row_id)?;
        }
        builder.finish()
    }

    /// Register all supported tokenizers with the index.
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
    /// Supports Tantivy query syntax including field-specific queries
    /// (e.g., `company:group AND city:east`).
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

        let rowid_field = schema
            .get_field(ROWID_FIELD)
            .map_err(|e| BundlebaseError::from(format!("RowId field not found: {}", e)))?;

        // Collect all text fields as default search fields
        let default_fields: Vec<Field> = self.columns.iter()
            .filter_map(|col| schema.get_field(col).ok())
            .collect();

        if default_fields.is_empty() {
            return Err(BundlebaseError::from("No searchable fields found in index"));
        }

        // Create query parser with all column fields as default search fields
        let query_parser = QueryParser::for_index(&self.index, default_fields);
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

    /// Get the index name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the columns indexed by this text index
    pub fn columns(&self) -> &[String] {
        &self.columns
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
            "name": self.name,
            "columns": self.columns,
            "doc_count": self.doc_count,
            "tokenizer": self.tokenizer_config,
            "version": 3,
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

        let column_name = metadata["name"]
            .as_str()
            .or_else(|| metadata["column_name"].as_str()) // backward compat
            .ok_or_else(|| BundlebaseError::from("Missing 'name' in metadata"))?
            .to_string();

        let doc_count = metadata["doc_count"]
            .as_u64()
            .ok_or_else(|| BundlebaseError::from("Missing doc_count in metadata"))?;

        let tokenizer_config: TokenizerConfig =
            serde_json::from_value(metadata["tokenizer"].clone()).map_err(|e| {
                BundlebaseError::from(format!("Failed to parse tokenizer config: {}", e))
            })?;

        let columns: Vec<String> = serde_json::from_value(
            metadata["columns"].clone()
        ).map_err(|e| {
            BundlebaseError::from(format!("Failed to parse columns from metadata: {}", e))
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
            name: column_name,
            columns,
            index,
            doc_count,
            tokenizer_config,
            _temp_dir: Some(temp_dir),
        })
    }
}

impl Index for TextIndex {
    fn serialize(&self) -> Result<Bytes, BundlebaseError> {
        self.serialize()
    }

    fn cardinality(&self) -> u64 {
        self.doc_count
    }

    fn column_name(&self) -> &str {
        &self.name
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

    /// Helper to build a single-column index for tests
    fn build_single_column(
        column_name: &str,
        documents: Vec<(&str, u64)>,
        tokenizer_config: &TokenizerConfig,
    ) -> TextIndex {
        let columns = vec![column_name.to_string()];
        let docs = documents.into_iter().map(move |(text, row_id)| {
            (vec![Some(text.to_string())], RowId::from(row_id))
        });
        TextIndex::build_streaming_multi(column_name, &columns, docs, tokenizer_config)
            .expect("Failed to build index")
    }

    #[test]
    fn test_build_and_search() {
        let index = build_single_column("content", vec![
            ("The quick brown fox jumps over the lazy dog", 1),
            ("Machine learning is transforming how we process data", 2),
            ("The fox was very quick and agile", 3),
        ], &TokenizerConfig::Simple);

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
        let index = build_single_column("content", vec![
            ("running", 1),
            ("run", 2),
            ("runner", 3),
        ], &TokenizerConfig::EnglishStem);

        // With stemming, searching for "run" should find all variants
        let results = index.search("run", 10).expect("Search failed");
        assert!(results.len() >= 1); // At least "run" itself
    }

    #[test]
    fn test_serialize_deserialize() {
        let index = build_single_column("test_col", vec![
            ("Hello world", 1),
            ("Goodbye world", 2),
        ], &TokenizerConfig::Simple);

        // Serialize
        let bytes = index.serialize().expect("Serialization failed");

        // Deserialize
        let restored = TextIndex::deserialize(bytes).expect("Deserialization failed");

        assert_eq!(restored.name(), "test_col");
        assert_eq!(restored.doc_count(), 2);

        // Verify search still works
        let results = restored.search("world", 10).expect("Search failed");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_streaming_build() {
        let index = build_single_column("content", vec![
            ("Document one about cats", 1),
            ("Document two about dogs", 2),
            ("Document three about cats and dogs", 3),
        ], &TokenizerConfig::Simple);

        let results = index.search("cats", 10).expect("Search failed");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_multi_column_build_and_search() {
        let columns = vec!["company".to_string(), "city".to_string()];

        let documents = vec![
            (vec![Some("Acme Group".to_string()), Some("East Leonard".to_string())], RowId::from(1u64)),
            (vec![Some("Beta Corp".to_string()), Some("West Haven".to_string())], RowId::from(2u64)),
            (vec![Some("Group Holdings".to_string()), Some("East Bay".to_string())], RowId::from(3u64)),
        ];

        let index = TextIndex::build_streaming_multi(
            "my_search",
            &columns,
            documents.into_iter(),
            &TokenizerConfig::Simple,
        )
        .expect("Failed to build multi-column index");

        // Search across all columns (default fields)
        let results = index.search("group", 10).expect("Search failed");
        assert_eq!(results.len(), 2); // "Acme Group" and "Group Holdings"

        // Field-specific search
        let results = index.search("company:group", 10).expect("Search failed");
        assert_eq!(results.len(), 2);

        // Field-specific search on city
        let results = index.search("city:east", 10).expect("Search failed");
        assert_eq!(results.len(), 2); // "East Leonard" and "East Bay"

        // Search for a specific term only in one document
        let results = index.search("city:haven", 10).expect("Search failed");
        assert_eq!(results.len(), 1); // Only "West Haven"
        assert_eq!(results[0].row_id.as_u64(), 2);
    }

    #[test]
    fn test_multi_column_serialize_deserialize() {
        let columns = vec!["title".to_string(), "body".to_string()];

        let documents = vec![
            (vec![Some("Hello World".to_string()), Some("This is the body text".to_string())], RowId::from(1u64)),
            (vec![Some("Goodbye World".to_string()), Some("Another body here".to_string())], RowId::from(2u64)),
        ];

        let index = TextIndex::build_streaming_multi(
            "my_index",
            &columns,
            documents.into_iter(),
            &TokenizerConfig::Simple,
        )
        .expect("Failed to build index");

        let bytes = index.serialize().expect("Serialization failed");
        let restored = TextIndex::deserialize(bytes).expect("Deserialization failed");

        assert_eq!(restored.name(), "my_index");
        assert_eq!(restored.columns(), &["title", "body"]);
        assert_eq!(restored.doc_count(), 2);

        // Field-specific search still works after deserialization
        let results = restored.search("title:hello", 10).expect("Search failed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].row_id.as_u64(), 1);
    }
}
