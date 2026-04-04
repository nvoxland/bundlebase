//! Formats the reader system can directly attach without conversion.

use bundlebase_common::connector::SourceFormat;

/// Formats the reader system can directly attach without conversion.
///
/// A strict subset of `SourceFormat`. Used on `AttachBlockOp.format` and
/// as the parameter to `DataReaderFactory::reader()` / `ReaderPlugin::reader()`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachFormat {
    Csv,
    Tsv,
    JsonL,
    Parquet,
}

impl AttachFormat {
    /// File extension for this format (without leading dot).
    pub fn extension(&self) -> &'static str {
        match self {
            AttachFormat::Csv => "csv",
            AttachFormat::Tsv => "tsv",
            AttachFormat::JsonL => "jsonl",
            AttachFormat::Parquet => "parquet",
        }
    }

    /// Convert from a SourceFormat, returning None for non-attachable formats.
    /// JSON arrays are converted to Parquet upstream and never stored as blocks.
    pub fn from_source_format(format: &SourceFormat) -> Option<Self> {
        match format {
            SourceFormat::Csv => Some(AttachFormat::Csv),
            SourceFormat::Tsv => Some(AttachFormat::Tsv),
            SourceFormat::JsonL => Some(AttachFormat::JsonL),
            SourceFormat::Parquet => Some(AttachFormat::Parquet),
            _ => None,
        }
    }
}

impl std::fmt::Display for AttachFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.extension())
    }
}
