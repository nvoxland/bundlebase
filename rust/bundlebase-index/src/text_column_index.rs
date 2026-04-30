//! Text/BM25 full-text search index using Tantivy
//!
//! This module provides full-text search capabilities using the Tantivy search engine.
//! It supports multiple tokenizers for different languages and use cases, and can index
//! one or more columns in a single index for multi-field search.

// search_table_fn stays in the core crate (depends on BundleFacade)

use crate::{Index, IndexType, TokenizerConfig};
use bundlebase_common::BundlebaseError;
use bundlebase_common::RowId;
use bytes::Bytes;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::Value;
use tantivy::schema::{Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, STORED};
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

        let text_options = TextOptions::default().set_indexing_options(
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
        self.index_writer
            .commit()
            .map_err(|e| BundlebaseError::from(format!("Failed to commit index: {}", e)))?;

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
            BundlebaseError::from(format!(
                "Failed to create stop word filter for {:?}",
                language
            ))
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
    pub fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<TextSearchResult>, BundlebaseError> {
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
        let default_fields: Vec<Field> = self
            .columns
            .iter()
            .filter_map(|col| schema.get_field(col).ok())
            .collect();

        if default_fields.is_empty() {
            return Err(BundlebaseError::from("No searchable fields found in index"));
        }

        // Create query parser with all column fields as default search fields
        let query_parser = QueryParser::for_index(&self.index, default_fields);
        let parsed_query = query_parser.parse_query(query).map_err(|e| {
            BundlebaseError::from(format!("Failed to parse query '{}': {}", query, e))
        })?;

        // Execute search
        let top_docs = searcher
            .search(&parsed_query, &TopDocs::with_limit(limit))
            .map_err(|e| BundlebaseError::from(format!("Search failed: {}", e)))?;

        // Collect results
        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|e| {
                BundlebaseError::from(format!("Failed to retrieve document: {}", e))
            })?;

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
        let metadata_bytes = serde_json::to_vec(&metadata)
            .map_err(|e| BundlebaseError::from(format!("Failed to serialize metadata: {}", e)))?;

        // Get the index path from temp directory
        let index_path = self
            ._temp_dir
            .as_ref()
            .ok_or_else(|| {
                BundlebaseError::from(
                    "Index was not created with a file-based directory, cannot serialize",
                )
            })?
            .path();

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
                    let file_name = path
                        .file_name()
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

            builder
                .finish()
                .map_err(|e| BundlebaseError::from(format!("Failed to finish archive: {}", e)))?;
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
        let metadata_content = std::fs::read_to_string(&metadata_path)
            .map_err(|e| BundlebaseError::from(format!("Failed to read metadata: {}", e)))?;

        let metadata: serde_json::Value = serde_json::from_str(&metadata_content)
            .map_err(|e| BundlebaseError::from(format!("Failed to parse metadata: {}", e)))?;

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

        let columns: Vec<String> =
            serde_json::from_value(metadata["columns"].clone()).map_err(|e| {
                BundlebaseError::from(format!("Failed to parse columns from metadata: {}", e))
            })?;

        // Remove metadata file before opening index
        std::fs::remove_file(&metadata_path).ok();

        // Open the index from the extracted directory
        let index = TantivyIndex::open_in_dir(temp_path)
            .map_err(|e| BundlebaseError::from(format!("Failed to open index: {}", e)))?;

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

/// A hit produced by `search_unified`, tagged with which input pass it came
/// from so the caller can resolve the originating block_id without depending
/// on tantivy segment IDs.
#[derive(Debug, Clone)]
pub struct UnifiedSearchHit {
    /// Index into the `passes` slice passed to `search_unified`.
    pub pass_idx: usize,
    pub row_id: RowId,
    pub score: f32,
}

/// Run a single BM25 query across multiple per-pass tar blobs as if they
/// were one tantivy index, so corpus stats (term DF, total docs, avg doc
/// length) are computed across the full active set rather than per-pass.
///
/// The on-disk format is unchanged — each pass is still its own tar with
/// its own `meta.json`. We materialize a unified tantivy Index in a temp
/// directory by:
///   1. extracting every pass's tar files into one shared dir,
///   2. reading each pass's `meta.json` and concatenating their `segments`
///      lists into a single `IndexMeta`,
///   3. opening that dir with `Index::open_in_dir`.
///
/// Tantivy assigns globally-unique UUIDs to its segment files, so file
/// names don't collide across passes. The schema and `IndexSettings` are
/// taken from the first pass — they're identical by construction (every
/// pass for a given index uses the same columns + tokenizer).
///
/// Each returned `UnifiedSearchHit` carries the index of the pass that
/// produced it, recovered via the unified `Searcher`'s segment readers.
///
/// This call extracts each pass's tar, copies all segment files, writes
/// a synthesized `meta.json`, and opens a fresh tantivy `Index` — work
/// that is identical for repeated calls against the same input. Most
/// callers should go through [`search_unified_cached`] instead, which
/// reuses an `Arc<UnifiedIndex>` keyed by pass identity. This bare
/// entrypoint is kept for callers that want one-off behavior or have
/// already resolved their own caching.
pub fn search_unified(
    passes: &[Bytes],
    query: &str,
    limit: usize,
) -> Result<Vec<UnifiedSearchHit>, BundlebaseError> {
    if passes.is_empty() {
        return Ok(Vec::new());
    }
    let unified = build_unified_index(passes)?;
    search_in_unified(&unified, query, limit)
}

/// A pre-assembled unified tantivy index over multiple `IndexedBlocks`
/// passes. Owns its own tempdir + tantivy `Index` and is reusable
/// across many search calls — the assembly cost (extracting each tar,
/// shuffling segment files, opening tantivy) only happens once.
///
/// Cheap to clone via `Arc`; expensive to construct.
pub struct UnifiedIndex {
    /// Kept alive so the on-disk segment files survive every `search`
    /// call. Dropped when the last `Arc<UnifiedIndex>` goes away.
    _temp_dir: TempDir,
    index: TantivyIndex,
    /// Stable u64 segment_id (from `SegmentId::uuid_string`) → which
    /// input pass produced that segment. Same role as the closure-local
    /// map in the original `search_unified`.
    seg_to_pass: std::collections::HashMap<String, usize>,
}

/// Build a `UnifiedIndex` from raw pass tar bytes. Idempotent for a
/// given set of `passes` — equivalent calls produce semantically
/// identical indexes (segment uuids carry through, so a hit's
/// `pass_idx` is stable across rebuilds).
pub fn build_unified_index(passes: &[Bytes]) -> Result<UnifiedIndex, BundlebaseError> {
    use std::collections::HashMap;
    use tar::Archive;

    if passes.is_empty() {
        return Err(BundlebaseError::from(
            "build_unified_index: passes must be non-empty",
        ));
    }

    let unified_dir = TempDir::with_prefix("bundlebase_text_unified_").map_err(|e| {
        BundlebaseError::from(format!("Failed to create unified temp dir: {}", e))
    })?;
    let unified_path = unified_dir.path();

    // tantivy's IndexMeta is `Serialize` but not `Deserialize` (it uses a
    // private `UntrackedIndexMeta` shadow type during open). We treat
    // meta.json as opaque JSON: peel off each pass's `segments` array and
    // concatenate, take `schema` / `index_settings` from the first pass
    // (identical by construction for a given index), and use the max
    // `opstamp`. tantivy will reparse on open.
    let mut combined_segments: Vec<serde_json::Value> = Vec::new();
    let mut max_opstamp: u64 = 0;
    let mut template_meta: Option<serde_json::Value> = None;
    // segment uuid → pass_idx; lets us map a hit's segment back to its
    // source pass without depending on tantivy-internal state.
    let mut seg_to_pass: HashMap<String, usize> = HashMap::new();

    for (pass_idx, tar_bytes) in passes.iter().enumerate() {
        let extract_dir = TempDir::new_in(unified_path).map_err(|e| {
            BundlebaseError::from(format!("Failed to create per-pass temp dir: {}", e))
        })?;
        let extract_path = extract_dir.path();

        Archive::new(tar_bytes.as_ref())
            .unpack(extract_path)
            .map_err(|e| {
                BundlebaseError::from(format!("Failed to extract pass {} tar: {}", pass_idx, e))
            })?;

        let meta_path = extract_path.join("meta.json");
        let meta_str = std::fs::read_to_string(&meta_path).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to read meta.json for pass {}: {}",
                pass_idx, e
            ))
        })?;
        let meta_val: serde_json::Value = serde_json::from_str(&meta_str).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to parse meta.json for pass {}: {}",
                pass_idx, e
            ))
        })?;

        if let Some(segs) = meta_val.get("segments").and_then(|v| v.as_array()) {
            for seg in segs {
                if let Some(seg_id) = seg.get("segment_id").and_then(|v| v.as_str()) {
                    let simple = seg_id.replace('-', "");
                    seg_to_pass.insert(simple, pass_idx);
                }
                combined_segments.push(seg.clone());
            }
        }
        if let Some(op) = meta_val.get("opstamp").and_then(|v| v.as_u64()) {
            max_opstamp = max_opstamp.max(op);
        }
        if template_meta.is_none() {
            template_meta = Some(meta_val);
        }

        // Move every segment file into the shared unified directory; skip
        // per-pass metadata and any tantivy lock/managed bookkeeping
        // (tantivy regenerates managed.json on open).
        for entry in std::fs::read_dir(extract_path).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to read pass {} extract dir: {}",
                pass_idx, e
            ))
        })? {
            let entry = entry.map_err(|e| {
                BundlebaseError::from(format!("Failed to read entry: {}", e))
            })?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let fname = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| BundlebaseError::from("Invalid file name in extracted pass"))?
                .to_string();
            if matches!(
                fname.as_str(),
                "meta.json" | "_metadata.json" | ".managed.json"
            ) || fname.starts_with('.')
            {
                continue;
            }
            let dest = unified_path.join(&fname);
            std::fs::rename(&path, &dest).map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to move segment file {} into unified dir: {}",
                    fname, e
                ))
            })?;
        }
    }

    // Synthesize one unified meta.json reusing the first pass's schema /
    // index_settings (identical by construction across passes).
    let mut unified_meta = template_meta.expect("template_meta set when passes is non-empty");
    if let Some(obj) = unified_meta.as_object_mut() {
        obj.insert(
            "segments".to_string(),
            serde_json::Value::Array(combined_segments),
        );
        obj.insert(
            "opstamp".to_string(),
            serde_json::Value::Number(serde_json::Number::from(max_opstamp)),
        );
        obj.remove("payload");
    }
    let unified_meta_str = serde_json::to_string(&unified_meta).map_err(|e| {
        BundlebaseError::from(format!("Failed to serialize unified meta.json: {}", e))
    })?;
    std::fs::write(unified_path.join("meta.json"), &unified_meta_str).map_err(|e| {
        BundlebaseError::from(format!("Failed to write unified meta.json: {}", e))
    })?;

    let index = TantivyIndex::open_in_dir(unified_path).map_err(|e| {
        BundlebaseError::from(format!("Failed to open unified index: {}", e))
    })?;
    TextIndex::register_tokenizers(&index)?;

    Ok(UnifiedIndex {
        _temp_dir: unified_dir,
        index,
        seg_to_pass,
    })
}

/// Run a single search against a pre-assembled `UnifiedIndex`. Cheap
/// (single-digit ms on warm tantivy mmaps); the expensive part lives in
/// [`build_unified_index`].
pub fn search_in_unified(
    unified: &UnifiedIndex,
    query: &str,
    limit: usize,
) -> Result<Vec<UnifiedSearchHit>, BundlebaseError> {
    let reader = unified
        .index
        .reader()
        .map_err(|e| BundlebaseError::from(format!("Failed to create reader: {}", e)))?;
    let searcher = reader.searcher();
    let schema = unified.index.schema();
    let rowid_field = schema
        .get_field(ROWID_FIELD)
        .map_err(|e| BundlebaseError::from(format!("rowid field missing: {}", e)))?;

    let default_fields: Vec<Field> = schema
        .fields()
        .filter_map(|(f, e)| {
            if matches!(e.field_type(), tantivy::schema::FieldType::Str(_))
                && schema.get_field_name(f) != ROWID_FIELD
            {
                Some(f)
            } else {
                None
            }
        })
        .collect();
    if default_fields.is_empty() {
        return Err(BundlebaseError::from(
            "Unified index has no searchable text fields",
        ));
    }

    let parser = QueryParser::for_index(&unified.index, default_fields);
    let parsed = parser.parse_query(query).map_err(|e| {
        BundlebaseError::from(format!("Failed to parse query '{}': {}", query, e))
    })?;
    let top_docs = searcher
        .search(&parsed, &TopDocs::with_limit(limit))
        .map_err(|e| BundlebaseError::from(format!("Search failed: {}", e)))?;

    let mut hits = Vec::with_capacity(top_docs.len());
    for (score, doc_address) in top_docs {
        let segment_id = searcher
            .segment_reader(doc_address.segment_ord)
            .segment_id()
            .uuid_string();
        let pass_idx = *unified.seg_to_pass.get(&segment_id).ok_or_else(|| {
            BundlebaseError::from(format!(
                "Unified search returned a hit from segment {} which doesn't belong to any pass",
                segment_id
            ))
        })?;
        let doc: TantivyDocument = searcher.doc(doc_address).map_err(|e| {
            BundlebaseError::from(format!("Failed to retrieve document: {}", e))
        })?;
        if let Some(rowid_value) = doc.get_first(rowid_field) {
            if let Some(rowid_u64) = rowid_value.as_u64() {
                hits.push(UnifiedSearchHit {
                    pass_idx,
                    row_id: RowId::from(rowid_u64),
                    score,
                });
            }
        }
    }

    Ok(hits)
}

/// Bounded LRU keyed by the *paths* of the input passes — not their
/// bytes. Pass paths are content-addressed
/// (`xx/<sha>.index.inverted.tar`), so two calls with the same paths
/// must mean the same bytes. This means we can avoid the expensive
/// "hash MB of bytes per call" approach and still get a safe key.
///
/// A single entry holds a `UnifiedIndex` (with its tempdir + tantivy
/// `Index`) keeping the on-disk index alive across many searches.
/// Eviction drops it; the next miss triggers a fresh assemble.
type UnifiedIndexKey = Vec<String>;
const UNIFIED_INDEX_CACHE_CAPACITY: usize = 16;

static UNIFIED_INDEX_CACHE: std::sync::LazyLock<
    parking_lot::Mutex<lru::LruCache<UnifiedIndexKey, std::sync::Arc<UnifiedIndex>>>,
> = std::sync::LazyLock::new(|| {
    parking_lot::Mutex::new(lru::LruCache::new(
        std::num::NonZeroUsize::new(UNIFIED_INDEX_CACHE_CAPACITY).expect("non-zero capacity"),
    ))
});

/// Cached version of [`search_unified`]. The unified tantivy `Index` is
/// assembled once per (set-of-pass-paths) and reused across search
/// calls. Skipping reassembly is the difference between ~700 ms and
/// ~5 ms per query on a multi-block bundle.
///
/// `paths` is the cache key; pass IDs in the same order as `passes`.
/// `passes` is the raw tar bytes for each pass — only consulted on a
/// cache miss. If callers want to skip the I/O entirely when the
/// cache is warm, check [`unified_index_cached`] first and skip
/// reading bytes in that case (passing `passes = []` is fine on a
/// cache hit and an error on a cache miss).
pub fn search_unified_cached(
    paths: &[String],
    passes: &[Bytes],
    query: &str,
    limit: usize,
) -> Result<Vec<UnifiedSearchHit>, BundlebaseError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    // Pass paths must be content-addressed
    // (`<hash-prefix>/<sha>.index.inverted.tar`) for the cache key to
    // be safe — two calls with the same paths must mean the same
    // bytes. Catch accidental non-content-addressed callers in debug.
    debug_assert!(
        paths
            .iter()
            .all(|p| p.contains(".index.inverted.tar") || p.contains(".index.")),
        "search_unified_cached: pass paths must be content-addressed; got {:?}",
        paths
    );
    let key: UnifiedIndexKey = paths.to_vec();

    // Fast path: cache hit.
    {
        let mut cache = UNIFIED_INDEX_CACHE.lock();
        if let Some(unified) = cache.get(&key) {
            let unified = std::sync::Arc::clone(unified);
            drop(cache);
            return search_in_unified(&unified, query, limit);
        }
    }

    // Cache miss — we need the actual bytes to build.
    if passes.len() != paths.len() {
        return Err(BundlebaseError::from(format!(
            "search_unified_cached: cache miss for {} passes but only {} pass bytes provided. \
             Caller should check unified_index_cached() before deciding whether to skip the I/O.",
            paths.len(),
            passes.len()
        )));
    }
    let unified = std::sync::Arc::new(build_unified_index(passes)?);
    {
        let mut cache = UNIFIED_INDEX_CACHE.lock();
        cache.put(key, std::sync::Arc::clone(&unified));
    }
    search_in_unified(&unified, query, limit)
}

/// Returns `true` when the unified-index cache has an entry for the
/// given path set. Lets callers skip reading pass tars on a cache hit.
pub fn unified_index_cached(paths: &[String]) -> bool {
    let mut cache = UNIFIED_INDEX_CACHE.lock();
    cache.contains(&paths.to_vec())
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
        IndexType::Inverted {
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
        let docs = documents
            .into_iter()
            .map(move |(text, row_id)| (vec![Some(text.to_string())], RowId::from(row_id)));
        TextIndex::build_streaming_multi(column_name, &columns, docs, tokenizer_config)
            .expect("Failed to build index")
    }

    #[test]
    fn test_build_and_search() {
        let index = build_single_column(
            "content",
            vec![
                ("The quick brown fox jumps over the lazy dog", 1),
                ("Machine learning is transforming how we process data", 2),
                ("The fox was very quick and agile", 3),
            ],
            &TokenizerConfig::Simple,
        );

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
        let index = build_single_column(
            "content",
            vec![("running", 1), ("run", 2), ("runner", 3)],
            &TokenizerConfig::EnglishStem,
        );

        // With stemming, searching for "run" should find all variants
        let results = index.search("run", 10).expect("Search failed");
        assert!(results.len() >= 1); // At least "run" itself
    }

    #[test]
    fn test_serialize_deserialize() {
        let index = build_single_column(
            "test_col",
            vec![("Hello world", 1), ("Goodbye world", 2)],
            &TokenizerConfig::Simple,
        );

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
        let index = build_single_column(
            "content",
            vec![
                ("Document one about cats", 1),
                ("Document two about dogs", 2),
                ("Document three about cats and dogs", 3),
            ],
            &TokenizerConfig::Simple,
        );

        let results = index.search("cats", 10).expect("Search failed");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_multi_column_build_and_search() {
        let columns = vec!["company".to_string(), "city".to_string()];

        let documents = vec![
            (
                vec![
                    Some("Acme Group".to_string()),
                    Some("East Leonard".to_string()),
                ],
                RowId::from(1u64),
            ),
            (
                vec![
                    Some("Beta Corp".to_string()),
                    Some("West Haven".to_string()),
                ],
                RowId::from(2u64),
            ),
            (
                vec![
                    Some("Group Holdings".to_string()),
                    Some("East Bay".to_string()),
                ],
                RowId::from(3u64),
            ),
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
            (
                vec![
                    Some("Hello World".to_string()),
                    Some("This is the body text".to_string()),
                ],
                RowId::from(1u64),
            ),
            (
                vec![
                    Some("Goodbye World".to_string()),
                    Some("Another body here".to_string()),
                ],
                RowId::from(2u64),
            ),
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

    /// Regression for the "search returns 0 hits on real multi-pass bundles"
    /// failure mode hit on the public claude-history bundle. Build two
    /// independent `TextIndex` instances containing matching documents,
    /// serialize each into its own pass tar, and call `search_unified` —
    /// must find hits from BOTH passes (not just one, not zero).
    ///
    /// Previously hidden by the e2e tests, which all use a single bundle's
    /// auto-reindex path: that path runs each reindex against an in-memory
    /// `TextIndexBuilder` whose tantivy temp dir is fresh. Here we exercise
    /// the actual deserialize-serialize-merge surface that runs at query
    /// time.
    #[test]
    fn test_search_unified_finds_hits_across_two_passes() {
        let columns = vec!["text".to_string()];

        let pass0 = TextIndex::build_streaming_multi(
            "pass0",
            &columns,
            vec![
                (vec![Some("apple cargo banana".to_string())], RowId::from(1u64)),
                (vec![Some("the quick brown fox".to_string())], RowId::from(2u64)),
                (vec![Some("cargo build success".to_string())], RowId::from(3u64)),
            ]
            .into_iter(),
            &TokenizerConfig::Simple,
        )
        .expect("build pass0");

        let pass1 = TextIndex::build_streaming_multi(
            "pass1",
            &columns,
            vec![
                (
                    vec![Some("rust cargo install crate".to_string())],
                    RowId::from(101u64),
                ),
                (vec![Some("nothing to see here".to_string())], RowId::from(102u64)),
                (vec![Some("cargo run --release".to_string())], RowId::from(103u64)),
            ]
            .into_iter(),
            &TokenizerConfig::Simple,
        )
        .expect("build pass1");

        let p0_bytes = pass0.serialize().expect("serialize pass0");
        let p1_bytes = pass1.serialize().expect("serialize pass1");

        // Sanity: each pass alone must find its own "cargo" hits via the
        // per-index `search` path. If this regresses, the unified bug isn't
        // what's being measured.
        let p0_solo = pass0.search("cargo", 10).expect("solo search pass0");
        assert_eq!(
            p0_solo.len(),
            2,
            "pass0 alone should find 2 'cargo' hits, got {}",
            p0_solo.len()
        );

        let hits = search_unified(&[p0_bytes, p1_bytes], "cargo", 100)
            .expect("unified search must succeed");
        let pass0_hits: Vec<_> = hits.iter().filter(|h| h.pass_idx == 0).collect();
        let pass1_hits: Vec<_> = hits.iter().filter(|h| h.pass_idx == 1).collect();
        assert_eq!(
            pass0_hits.len(),
            2,
            "expected 2 pass0 'cargo' hits across unified search, got {} (total hits {}). \
             A count of 0 reproduces the public claude-history failure mode where \
             search_unified silently fails to assemble the unified index.",
            pass0_hits.len(),
            hits.len()
        );
        assert_eq!(
            pass1_hits.len(),
            2,
            "expected 2 pass1 'cargo' hits across unified search, got {} (total hits {})",
            pass1_hits.len(),
            hits.len()
        );
    }

    /// Cache contract: a cache hit must skip the build entirely
    /// (callers can pass empty `passes` and still get hits) and produce
    /// the same results as the bare `search_unified`.
    #[test]
    fn test_search_unified_cached_reuses_assembly() {
        let columns = vec!["text".to_string()];
        let pass0 = TextIndex::build_streaming_multi(
            "p0",
            &columns,
            vec![
                (vec![Some("alpha bravo".to_string())], RowId::from(1u64)),
                (vec![Some("charlie alpha".to_string())], RowId::from(2u64)),
            ]
            .into_iter(),
            &TokenizerConfig::Simple,
        )
        .expect("build pass0");
        let p0_bytes = pass0.serialize().expect("serialize");

        let paths = vec!["test://cache_reuse/0".to_string()];
        let passes = vec![p0_bytes];

        // Miss → cached
        assert!(!unified_index_cached(&paths));
        let first =
            search_unified_cached(&paths, &passes, "alpha", 10).expect("first call");
        assert_eq!(first.len(), 2);
        assert!(unified_index_cached(&paths));

        // Hit → skips build entirely. Pass an empty `passes` slice to
        // prove the cache fed the result without re-extracting.
        let second =
            search_unified_cached(&paths, &[], "alpha", 10).expect("second call (cache hit)");
        assert_eq!(second.len(), 2);
        let scores_first: Vec<_> = first.iter().map(|h| h.score).collect();
        let scores_second: Vec<_> = second.iter().map(|h| h.score).collect();
        assert_eq!(scores_first, scores_second);

        // A different query against the same cached index also works.
        let other =
            search_unified_cached(&paths, &[], "charlie", 10).expect("second query (cache hit)");
        assert_eq!(other.len(), 1);

        // Cache miss with no bytes → clear error, not a panic.
        let unknown_paths = vec!["test://cache_reuse/never_seen".to_string()];
        let err = search_unified_cached(&unknown_paths, &[], "alpha", 10).unwrap_err();
        assert!(
            err.to_string().contains("cache miss"),
            "expected 'cache miss' in error: {}",
            err
        );
    }
}
