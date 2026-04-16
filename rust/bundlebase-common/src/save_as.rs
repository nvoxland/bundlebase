//! Save strategy for fetched source data.
//!
//! Configured on the source via the `SAVE AS` clause of `CREATE SOURCE`.
//! Controls how data from connectors gets stored in the bundle.

use crate::connector::SourceFormat;
use crate::BundlebaseError;

/// How fetched data should be stored.
///
/// Configured on the source via `SAVE AS` clause in `CREATE SOURCE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveAs {
    /// Copy original bytes into the data directory. Only valid for attachable formats.
    Copy,
    /// Convert to Parquet before saving.
    Parquet,
    /// Reference the remote URL directly — no download. Only valid for attachable
    /// formats and connectors that don't require copying (must_copy=false).
    Ref,
    /// Always convert to Parquet and store in the bundle.
    Auto,
}

impl Default for SaveAs {
    fn default() -> Self {
        SaveAs::Auto
    }
}

impl SaveAs {
    /// Parse a save_as string from user input.
    pub fn parse(s: &str) -> Result<Self, BundlebaseError> {
        match s.to_lowercase().as_str() {
            "copy" => Ok(SaveAs::Copy),
            "parquet" => Ok(SaveAs::Parquet),
            "ref" => Ok(SaveAs::Ref),
            "auto" => Ok(SaveAs::Auto),
            other => Err(format!(
                "Invalid save_as value '{}'. Valid values: auto, copy, parquet, ref",
                other
            )
            .into()),
        }
    }

    /// Resolve to a concrete strategy based on format and must_copy.
    pub fn resolve(
        &self,
        format: &SourceFormat,
        must_copy: bool,
    ) -> Result<ResolvedSaveAs, BundlebaseError> {
        match self {
            SaveAs::Copy => {
                let is_attachable = matches!(
                    format,
                    SourceFormat::Csv
                        | SourceFormat::Tsv
                        | SourceFormat::JsonL
                        | SourceFormat::Parquet
                );
                if is_attachable {
                    Ok(ResolvedSaveAs::Copy)
                } else {
                    Err(format!(
                        "save_as='copy' is not valid for format '{}'. \
                         Use save_as='parquet' to convert.",
                        format
                    )
                    .into())
                }
            }
            SaveAs::Parquet => Ok(ResolvedSaveAs::Parquet),
            SaveAs::Ref => {
                if must_copy {
                    return Err("save_as='ref' is not supported for this source — \
                        the connector requires data to be copied into the bundle."
                        .into());
                }
                let is_attachable = matches!(
                    format,
                    SourceFormat::Csv
                        | SourceFormat::Tsv
                        | SourceFormat::JsonL
                        | SourceFormat::Parquet
                );
                if !is_attachable {
                    return Err(format!(
                        "save_as='ref' is not valid for format '{}'. \
                         Non-attachable formats must be converted. Use save_as='parquet'.",
                        format
                    )
                    .into());
                }
                Ok(ResolvedSaveAs::Ref)
            }
            SaveAs::Auto => Ok(ResolvedSaveAs::Parquet),
        }
    }
}

/// A resolved (non-Auto) save strategy ready for execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedSaveAs {
    /// Copy original bytes into the data directory.
    Copy,
    /// Convert to Parquet and store in the data directory.
    Parquet,
    /// Reference the remote URL directly — no download.
    Ref,
}
