use crate::data::{ObjectId, VersionedBlockId};
use crate::bundle::IndexedBlocks;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

/// Type of index for a column
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum IndexType {
    /// B-tree style column index for equality/range queries
    #[default]
    Column,
    /// BM25 full-text search index
    Text {
        tokenizer: TokenizerConfig,
    },
}

impl IndexType {
    /// Create a new Text index type with the specified tokenizer
    pub fn text(tokenizer: TokenizerConfig) -> Self {
        IndexType::Text { tokenizer }
    }

    /// Check if this is a text/full-text index
    pub fn is_text(&self) -> bool {
        matches!(self, IndexType::Text { .. })
    }

    /// Check if this is a column/btree index
    pub fn is_column(&self) -> bool {
        matches!(self, IndexType::Column)
    }

    /// Get the tokenizer config if this is a text index
    pub fn tokenizer(&self) -> Option<&TokenizerConfig> {
        match self {
            IndexType::Text { tokenizer } => Some(tokenizer),
            IndexType::Column => None,
        }
    }

    /// Parse an index type from a string, optionally with tokenizer.
    ///
    /// For text indexes, a tokenizer can be specified separately via `with_tokenizer()`.
    pub fn with_tokenizer(self, tokenizer: Option<TokenizerConfig>) -> Self {
        match self {
            IndexType::Text { .. } => IndexType::Text {
                tokenizer: tokenizer.unwrap_or_default(),
            },
            IndexType::Column => self,
        }
    }

    /// Validate args and return configured IndexType.
    ///
    /// Each index type validates its own arguments and returns an error for
    /// unknown arguments.
    ///
    /// # Arguments
    /// * `args` - HashMap of argument name to value
    ///
    /// # Supported Arguments
    /// - **Column index**: No arguments accepted
    /// - **Text index**: `tokenizer` (optional) - tokenizer name (e.g., "simple", "en_stem")
    pub fn with_args(self, args: &HashMap<String, String>) -> Result<Self, IndexTypeConfigError> {
        match self {
            IndexType::Column => {
                // Column index accepts no args
                if let Some(unknown) = args.keys().next() {
                    return Err(IndexTypeConfigError(format!(
                        "Unknown argument '{}' for column index",
                        unknown
                    )));
                }
                Ok(self)
            }
            IndexType::Text { .. } => {
                let mut tokenizer = TokenizerConfig::default();
                for (key, value) in args {
                    match key.as_str() {
                        "tokenizer" => {
                            tokenizer = TokenizerConfig::from_str(value).map_err(|e| {
                                IndexTypeConfigError(format!("Invalid tokenizer: {}", e))
                            })?;
                        }
                        unknown => {
                            return Err(IndexTypeConfigError(format!(
                                "Unknown argument '{}' for text index. Valid arguments: tokenizer",
                                unknown
                            )));
                        }
                    }
                }
                Ok(IndexType::Text { tokenizer })
            }
        }
    }
}

/// Error type for parsing index type from string
#[derive(Debug, Clone)]
pub struct ParseIndexTypeError(String);

impl std::fmt::Display for ParseIndexTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ParseIndexTypeError {}

/// Error type for configuring index type with arguments
#[derive(Debug, Clone)]
pub struct IndexTypeConfigError(pub String);

impl std::fmt::Display for IndexTypeConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for IndexTypeConfigError {}

impl FromStr for IndexType {
    type Err = ParseIndexTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "column" | "btree" | "b-tree" => Ok(IndexType::Column),
            "text" | "fulltext" | "fts" | "full-text" => {
                Ok(IndexType::Text {
                    tokenizer: TokenizerConfig::default(),
                })
            }
            other => Err(ParseIndexTypeError(format!(
                "Unknown index type '{}'. Valid options: column, btree, text, fulltext, fts",
                other
            ))),
        }
    }
}

/// Tokenizer configuration for text indexes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub enum TokenizerConfig {
    /// Simple tokenization: whitespace + lowercase
    #[default]
    Simple,
    /// No tokenization - treat entire text as single token
    Raw,
    /// English stemming + stop words
    EnglishStem,
    /// German stemming
    GermanStem,
    /// French stemming
    FrenchStem,
    /// Spanish stemming
    SpanishStem,
    /// Italian stemming
    ItalianStem,
    /// Portuguese stemming
    PortugueseStem,
    /// Dutch stemming
    DutchStem,
    /// Swedish stemming
    SwedishStem,
    /// Norwegian stemming
    NorwegianStem,
    /// Danish stemming
    DanishStem,
    /// Finnish stemming
    FinnishStem,
    /// Russian stemming
    RussianStem,
}

/// Error type for parsing tokenizer config from string
#[derive(Debug, Clone)]
pub struct ParseTokenizerError(String);

impl std::fmt::Display for ParseTokenizerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ParseTokenizerError {}

impl FromStr for TokenizerConfig {
    type Err = ParseTokenizerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "simple" => Ok(TokenizerConfig::Simple),
            "raw" => Ok(TokenizerConfig::Raw),
            "en_stem" | "english_stem" | "english" => Ok(TokenizerConfig::EnglishStem),
            "de_stem" | "german_stem" | "german" => Ok(TokenizerConfig::GermanStem),
            "fr_stem" | "french_stem" | "french" => Ok(TokenizerConfig::FrenchStem),
            "es_stem" | "spanish_stem" | "spanish" => Ok(TokenizerConfig::SpanishStem),
            "it_stem" | "italian_stem" | "italian" => Ok(TokenizerConfig::ItalianStem),
            "pt_stem" | "portuguese_stem" | "portuguese" => Ok(TokenizerConfig::PortugueseStem),
            "nl_stem" | "dutch_stem" | "dutch" => Ok(TokenizerConfig::DutchStem),
            "sv_stem" | "swedish_stem" | "swedish" => Ok(TokenizerConfig::SwedishStem),
            "no_stem" | "norwegian_stem" | "norwegian" => Ok(TokenizerConfig::NorwegianStem),
            "da_stem" | "danish_stem" | "danish" => Ok(TokenizerConfig::DanishStem),
            "fi_stem" | "finnish_stem" | "finnish" => Ok(TokenizerConfig::FinnishStem),
            "ru_stem" | "russian_stem" | "russian" => Ok(TokenizerConfig::RussianStem),
            other => Err(ParseTokenizerError(format!(
                "Unknown tokenizer '{}'. Valid options: simple, raw, en_stem, de_stem, fr_stem, es_stem, it_stem, pt_stem, nl_stem, sv_stem, no_stem, da_stem, fi_stem, ru_stem",
                other
            ))),
        }
    }
}

impl TokenizerConfig {

    /// Get the tantivy tokenizer name
    pub fn tantivy_tokenizer_name(&self) -> &'static str {
        match self {
            TokenizerConfig::Simple => "simple",
            TokenizerConfig::Raw => "raw",
            TokenizerConfig::EnglishStem => "en_stem",
            TokenizerConfig::GermanStem => "de_stem",
            TokenizerConfig::FrenchStem => "fr_stem",
            TokenizerConfig::SpanishStem => "es_stem",
            TokenizerConfig::ItalianStem => "it_stem",
            TokenizerConfig::PortugueseStem => "pt_stem",
            TokenizerConfig::DutchStem => "nl_stem",
            TokenizerConfig::SwedishStem => "sv_stem",
            TokenizerConfig::NorwegianStem => "no_stem",
            TokenizerConfig::DanishStem => "da_stem",
            TokenizerConfig::FinnishStem => "fi_stem",
            TokenizerConfig::RussianStem => "ru_stem",
        }
    }
}

/// Key for the block lookup HashMap: (block_id, version)
type BlockLookupKey = (ObjectId, String);

#[derive(Debug)]
pub struct IndexDefinition {
    id: ObjectId,
    column: String,
    index_type: IndexType,
    blocks: RwLock<Vec<Arc<IndexedBlocks>>>,
    /// O(1) lookup from versioned block ID to its IndexedBlocks
    block_lookup: RwLock<HashMap<BlockLookupKey, Arc<IndexedBlocks>>>,
}

impl IndexDefinition {
    /// Create a new column index definition (default type)
    pub(crate) fn new(id: &ObjectId, column: &String) -> IndexDefinition {
        Self::with_type(id, column, IndexType::Column)
    }

    /// Create a new index definition with a specific type
    pub(crate) fn with_type(id: &ObjectId, column: &String, index_type: IndexType) -> IndexDefinition {
        Self {
            id: *id,
            column: column.clone(),
            index_type,
            blocks: RwLock::new(Vec::new()),
            block_lookup: RwLock::new(HashMap::new()),
        }
    }

    pub fn id(&self) -> &ObjectId {
        &self.id
    }

    pub fn column(&self) -> &String {
        &self.column
    }

    pub fn index_type(&self) -> &IndexType {
        &self.index_type
    }

    /// Check if this is a text/full-text index
    pub fn is_text(&self) -> bool {
        self.index_type.is_text()
    }

    /// Check if this is a column/btree index
    pub fn is_column(&self) -> bool {
        self.index_type.is_column()
    }

    /// Get the IndexedBlocks containing the specified versioned block.
    /// Uses O(1) HashMap lookup instead of O(n) linear search.
    pub fn indexed_blocks(&self, versioned_block: &VersionedBlockId) -> Option<Arc<IndexedBlocks>> {
        let key = (versioned_block.block, versioned_block.version.clone());
        self.block_lookup.read().get(&key).cloned()
    }

    /// Adds a new set of indexed blocks to this index definition.
    /// Updates both the blocks Vec and the lookup HashMap.
    pub(crate) fn add_indexed_blocks(&self, indexed_blocks: Arc<IndexedBlocks>) {
        // Add to lookup map for each versioned block
        {
            let mut lookup = self.block_lookup.write();
            for vb in indexed_blocks.blocks() {
                let key = (vb.block, vb.version);
                lookup.insert(key, Arc::clone(&indexed_blocks));
            }
        }

        // Add to blocks vec
        self.blocks.write().push(indexed_blocks);
    }

    /// Returns all indexed blocks for this index definition
    pub(crate) fn all_indexed_blocks(&self) -> Vec<Arc<IndexedBlocks>> {
        self.blocks.read().clone()
    }

    /// Prunes stale indexed blocks that don't match current block versions.
    /// This prevents memory leaks from accumulating old index references.
    /// Also updates the lookup HashMap to maintain consistency.
    ///
    /// # Arguments
    /// * `current_versions` - Map of block IDs to their current versions
    ///
    /// # Returns
    /// Number of stale IndexedBlocks removed
    pub(crate) fn prune_stale_blocks(&self, current_versions: &HashMap<ObjectId, String>) -> usize {
        let mut blocks = self.blocks.write();
        let mut lookup = self.block_lookup.write();
        let initial_count = blocks.len();

        // Collect keys to remove from lookup before modifying blocks vec
        let mut keys_to_remove: Vec<BlockLookupKey> = Vec::new();

        blocks.retain(|indexed_blocks| {
            // Keep only blocks where ALL versioned blocks match current versions
            let should_keep = indexed_blocks.blocks().iter().all(|vb| {
                current_versions
                    .get(&vb.block)
                    .map(|current_ver| current_ver == &vb.version)
                    .unwrap_or(false) // Remove if block doesn't exist anymore
            });

            if !should_keep {
                // Collect keys for removal from lookup
                for vb in indexed_blocks.blocks() {
                    keys_to_remove.push((vb.block, vb.version));
                }
            }

            should_keep
        });

        // Remove stale entries from lookup
        for key in keys_to_remove {
            lookup.remove(&key);
        }

        let removed_count = initial_count - blocks.len();

        if removed_count > 0 {
            log::debug!(
                "Pruned {} stale IndexedBlocks from index {} (column '{}')",
                removed_count,
                self.id,
                self.column
            );
        }

        removed_count
    }
}
