use crate::connector::SourceFormat;
use crate::BundlebaseError;
use std::fmt;

/// The general category of a content-addressed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentCategory {
    /// Block data and block metadata (layout, etc.)
    Block,
    /// Column indexes, text search indexes
    Index,
    /// Mutation overlays (tombstones, update overlays)
    Overlay,
    /// Generated reports
    Report,
    /// UDF runtime binaries
    Udf,
}

impl fmt::Display for ContentCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContentCategory::Block => write!(f, "block"),
            ContentCategory::Index => write!(f, "index"),
            ContentCategory::Overlay => write!(f, "overlay"),
            ContentCategory::Report => write!(f, "report"),
            ContentCategory::Udf => write!(f, "udf"),
        }
    }
}

/// The file format of a content-addressed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentFormat {
    Parquet,
    Csv,
    Tsv,
    Json,
    JsonL,
    Xlsx,
    Xls,
    Ods,
    Tar,
    Md,
    Bin,
    Dat,
    /// Physical row group page map format
    Pagemap,
    /// Sorted value→RowId mapping format
    Rowmap,
    /// Serialized RowId set format
    Rowids,
}

impl fmt::Display for ContentFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContentFormat::Parquet => write!(f, "parquet"),
            ContentFormat::Csv => write!(f, "csv"),
            ContentFormat::Tsv => write!(f, "tsv"),
            ContentFormat::Json => write!(f, "json"),
            ContentFormat::JsonL => write!(f, "jsonl"),
            ContentFormat::Xlsx => write!(f, "xlsx"),
            ContentFormat::Xls => write!(f, "xls"),
            ContentFormat::Ods => write!(f, "ods"),
            ContentFormat::Tar => write!(f, "tar"),
            ContentFormat::Md => write!(f, "md"),
            ContentFormat::Bin => write!(f, "bin"),
            ContentFormat::Dat => write!(f, "dat"),
            ContentFormat::Pagemap => write!(f, "pagemap"),
            ContentFormat::Rowmap => write!(f, "rowmap"),
            ContentFormat::Rowids => write!(f, "rowids"),
        }
    }
}

impl ContentFormat {
    /// Parse a file extension string into a ContentFormat.
    pub fn from_extension(ext: &str) -> Result<Self, BundlebaseError> {
        match ext.to_lowercase().as_str() {
            "parquet" => Ok(ContentFormat::Parquet),
            "csv" => Ok(ContentFormat::Csv),
            "tsv" => Ok(ContentFormat::Tsv),
            "json" => Ok(ContentFormat::Json),
            "jsonl" => Ok(ContentFormat::JsonL),
            "xlsx" => Ok(ContentFormat::Xlsx),
            "xls" => Ok(ContentFormat::Xls),
            "ods" => Ok(ContentFormat::Ods),
            "tar" => Ok(ContentFormat::Tar),
            "md" => Ok(ContentFormat::Md),
            "bin" => Ok(ContentFormat::Bin),
            "dat" => Ok(ContentFormat::Dat),
            "pagemap" => Ok(ContentFormat::Pagemap),
            "rowmap" => Ok(ContentFormat::Rowmap),
            "rowids" => Ok(ContentFormat::Rowids),
            _ => Err(format!("Unknown content format: {}", ext).into()),
        }
    }

    /// Convert from a SourceFormat to a ContentFormat.
    pub fn from_source_format(sf: &SourceFormat) -> Self {
        match sf {
            SourceFormat::Csv => ContentFormat::Csv,
            SourceFormat::Tsv => ContentFormat::Tsv,
            SourceFormat::Json => ContentFormat::Json,
            SourceFormat::JsonL => ContentFormat::JsonL,
            SourceFormat::Parquet => ContentFormat::Parquet,
            SourceFormat::Xlsx => ContentFormat::Xlsx,
            SourceFormat::Xls => ContentFormat::Xls,
            SourceFormat::Ods => ContentFormat::Ods,
            SourceFormat::Auto => ContentFormat::Dat,
        }
    }
}

/// Structured extension for content-addressed files.
///
/// Produces filenames like `{hash}.block.data.parquet` or `{hash}.index.inverted.tar`.
#[derive(Debug, Clone)]
pub struct ContentAddress {
    pub category: ContentCategory,
    pub sub_type: Option<String>,
    pub format: ContentFormat,
}

impl ContentAddress {
    /// Create a ContentAddress with no sub-type.
    ///
    /// Produces extensions like `report.md` or `udf.bin`.
    pub fn new(category: ContentCategory, format: ContentFormat) -> Self {
        Self {
            category,
            sub_type: None,
            format,
        }
    }

    /// Create a ContentAddress with a sub-type.
    ///
    /// Produces extensions like `block.data.parquet` or `index.inverted.tar`.
    pub fn with_sub_type(
        category: ContentCategory,
        sub_type: &str,
        format: ContentFormat,
    ) -> Result<Self, BundlebaseError> {
        if sub_type.contains('.') {
            return Err(format!("Content address sub_type must not contain dots: {}", sub_type).into());
        }
        Ok(Self {
            category,
            sub_type: Some(sub_type.to_string()),
            format,
        })
    }

    /// Returns the extension string for the filename.
    ///
    /// Examples: `block.data.parquet`, `report.md`, `index.inverted.tar`
    pub fn extension(&self) -> String {
        match &self.sub_type {
            Some(st) => format!("{}.{}.{}", self.category, st, self.format),
            None => format!("{}.{}", self.category, self.format),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_extension_without_sub_type() {
        let addr = ContentAddress::new(ContentCategory::Report, ContentFormat::Md);
        assert_eq!(addr.extension(), "report.md");
    }

    #[test]
    fn test_extension_with_sub_type() {
        let addr =
            ContentAddress::with_sub_type(ContentCategory::Block, "data", ContentFormat::Parquet)
                .unwrap();
        assert_eq!(addr.extension(), "block.data.parquet");
    }

    #[test]
    fn test_block_layout_pagemap() {
        let addr =
            ContentAddress::with_sub_type(ContentCategory::Block, "layout", ContentFormat::Pagemap)
                .unwrap();
        assert_eq!(addr.extension(), "block.layout.pagemap");
    }

    #[test]
    fn test_index_btree_rowmap() {
        let addr =
            ContentAddress::with_sub_type(ContentCategory::Index, "btree", ContentFormat::Rowmap)
                .unwrap();
        assert_eq!(addr.extension(), "index.btree.rowmap");
    }

    #[test]
    fn test_index_inverted_tar() {
        let addr =
            ContentAddress::with_sub_type(ContentCategory::Index, "inverted", ContentFormat::Tar)
                .unwrap();
        assert_eq!(addr.extension(), "index.inverted.tar");
    }

    #[test]
    fn test_overlay_tomb_rowids() {
        let addr = ContentAddress::with_sub_type(
            ContentCategory::Overlay,
            "tomb",
            ContentFormat::Rowids,
        )
        .unwrap();
        assert_eq!(addr.extension(), "overlay.tomb.rowids");
    }

    #[test]
    fn test_overlay_update_parquet() {
        let addr = ContentAddress::with_sub_type(
            ContentCategory::Overlay,
            "update",
            ContentFormat::Parquet,
        )
        .unwrap();
        assert_eq!(addr.extension(), "overlay.update.parquet");
    }

    #[test]
    fn test_sub_type_with_dots_rejected() {
        let result =
            ContentAddress::with_sub_type(ContentCategory::Block, "data.extra", ContentFormat::Csv);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_extension() {
        assert_eq!(
            ContentFormat::from_extension("parquet").unwrap(),
            ContentFormat::Parquet
        );
        assert_eq!(
            ContentFormat::from_extension("CSV").unwrap(),
            ContentFormat::Csv
        );
        assert!(ContentFormat::from_extension("unknown").is_err());
    }

    #[test]
    fn test_from_source_format() {
        assert_eq!(
            ContentFormat::from_source_format(&SourceFormat::Csv),
            ContentFormat::Csv
        );
        assert_eq!(
            ContentFormat::from_source_format(&SourceFormat::Auto),
            ContentFormat::Dat
        );
    }
}
