//! ImportConnector command implementation.

use crate::parser::extract_identifier;
use crate::parser::{escape_string, extract_string_content};
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::ImportConnectorOp;
use bundlebase::BundleBuilder;
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use bundlebase_common::Platform;
use bundlebase_connector::plugin::ffi::{
    verify_shared_lib_connector, verify_shared_lib_header,
};
use bundlebase_udf::runtime::UdfRuntime;
use std::collections::HashMap;

/// Command to define a named connector.
///
/// Combines connector loading and entrypoint setting into a single command.
/// If the connector already exists, adds/replaces the entrypoint for the given platform.
///
/// Supports three source forms:
/// - **Single**: one (from, platform) pair — the original syntax.
/// - **Multi**: explicit `{ 'platform': 'from', ... }` map for fat connectors.
/// - **Glob**: a path with `{os}`, `{arch}`, `{ext}` placeholders, expanded by
///   scanning the filesystem at execute time.
#[derive(Debug, Clone)]
pub struct ImportConnectorCommand {
    /// Full dotted connector name (e.g., "acme.weather")
    pub name: String,
    /// Source binaries: one or more (from, platform) entries.
    pub source: ImportConnectorSource,
    /// Optional path to a source archive (zip) for the connector. Copied into
    /// the bundle's data directory at execute time and shared by every
    /// platform entry generated from this command.
    pub src: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportConnectorSource {
    /// Single binary, optionally tagged with a platform (defaults to `*/*`).
    Single { from: String, platform: Platform },
    /// Multiple binaries, one per explicitly-listed platform. Used by the
    /// `FROM { 'linux/amd64': '...', 'darwin/arm64': '...' }` syntax.
    Multi { entries: Vec<(Platform, String)> },
    /// Glob pattern with `{os}`, `{arch}`, `{ext}` placeholders. Expanded by
    /// scanning the filesystem at execute time.
    Glob { pattern: String },
}

impl ImportConnectorCommand {
    /// Construct a single-platform IMPORT CONNECTOR (back-compat constructor).
    pub fn new(name: impl Into<String>, from: impl Into<String>, platform: Platform) -> Self {
        Self {
            name: name.into(),
            source: ImportConnectorSource::Single {
                from: from.into(),
                platform,
            },
            src: None,
        }
    }

    /// Construct a multi-platform IMPORT CONNECTOR.
    pub fn new_multi(
        name: impl Into<String>,
        entries: Vec<(Platform, String)>,
    ) -> Self {
        Self {
            name: name.into(),
            source: ImportConnectorSource::Multi { entries },
            src: None,
        }
    }

    /// Construct a glob-form IMPORT CONNECTOR.
    pub fn new_glob(name: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: ImportConnectorSource::Glob {
                pattern: pattern.into(),
            },
            src: None,
        }
    }

    /// Attach a source-archive path. Returns `self` for chaining.
    pub fn with_src(mut self, src: Option<String>) -> Self {
        self.src = src;
        self
    }
}

impl CommandParsing for ImportConnectorCommand {
    fn rule() -> Rule {
        Rule::import_connector_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut from = None;
        let mut platform_map: Option<Vec<(Platform, String)>> = None;
        let mut args: HashMap<String, String> = HashMap::new();

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::dotted_identifier => {
                    name = Some(inner_pair.as_str().to_string());
                }
                Rule::quoted_string => {
                    from = Some(extract_string_content(inner_pair.as_str())?);
                }
                Rule::platform_map => {
                    let mut entries = Vec::new();
                    for pair in inner_pair.into_inner() {
                        if pair.as_rule() == Rule::platform_map_pair {
                            let mut strings = pair
                                .into_inner()
                                .filter(|p| p.as_rule() == Rule::quoted_string);
                            let plat_str = strings.next().ok_or_else(|| -> BundlebaseError {
                                "platform map entry missing platform".into()
                            })?;
                            let from_str = strings.next().ok_or_else(|| -> BundlebaseError {
                                "platform map entry missing FROM string".into()
                            })?;
                            let plat: Platform =
                                extract_string_content(plat_str.as_str())?.parse()?;
                            let from = extract_string_content(from_str.as_str())?;
                            entries.push((plat, from));
                        }
                    }
                    if entries.is_empty() {
                        return Err("IMPORT CONNECTOR platform map cannot be empty".into());
                    }
                    platform_map = Some(entries);
                }
                Rule::source_args => {
                    for arg_pair in inner_pair.into_inner() {
                        if arg_pair.as_rule() == Rule::source_arg_pair {
                            let mut key = None;
                            let mut value = None;
                            for part in arg_pair.into_inner() {
                                match part.as_rule() {
                                    Rule::identifier => {
                                        key = Some(extract_identifier(&part));
                                    }
                                    Rule::quoted_string => {
                                        value = Some(extract_string_content(part.as_str())?);
                                    }
                                    _ => {}
                                }
                            }
                            if let (Some(k), Some(v)) = (key, value) {
                                args.insert(k, v);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "IMPORT CONNECTOR missing connector name".into()
        })?;

        // `src` is shared by every entry generated from this command, so we
        // pull it out before we branch on map / glob / single.
        let src = args.remove("src");

        let cmd = if let Some(entries) = platform_map {
            // Map form is mutually exclusive with WITH (platform = ...).
            if args.contains_key("platform") {
                return Err(
                    "IMPORT CONNECTOR cannot combine platform map with WITH (platform = ...)"
                        .into(),
                );
            }
            ImportConnectorCommand::new_multi(name, entries)
        } else {
            let from = from.ok_or_else(|| -> BundlebaseError {
                "IMPORT CONNECTOR missing FROM clause".into()
            })?;

            // A glob pattern in the single-string form expands at execute time.
            if has_glob_placeholder(&from) {
                if args.contains_key("platform") {
                    return Err(
                        "IMPORT CONNECTOR with glob pattern cannot combine WITH (platform = ...)"
                            .into(),
                    );
                }
                ImportConnectorCommand::new_glob(name, from)
            } else {
                let platform: Platform = match args.remove("platform") {
                    Some(s) => s.parse()?,
                    None => Platform::any(),
                };
                ImportConnectorCommand::new(name, from, platform)
            }
        };

        Ok(cmd.with_src(src))
    }

    fn to_statement(&self) -> String {
        // Build the WITH (...) tail. `platform` only applies to the Single
        // form; `src` applies to any form.
        let mut with_parts: Vec<String> = Vec::new();
        if let ImportConnectorSource::Single { platform, .. } = &self.source {
            if *platform != Platform::any() {
                with_parts.push(format!(
                    "platform = {}",
                    escape_string(&platform.to_string())
                ));
            }
        }
        if let Some(src) = &self.src {
            with_parts.push(format!("src = {}", escape_string(src)));
        }
        let with_tail = if with_parts.is_empty() {
            String::new()
        } else {
            format!(" WITH ({})", with_parts.join(", "))
        };

        match &self.source {
            ImportConnectorSource::Single { from, .. } => {
                format!(
                    "IMPORT CONNECTOR {} FROM {}{}",
                    self.name,
                    escape_string(from),
                    with_tail
                )
            }
            ImportConnectorSource::Multi { entries } => {
                let body = entries
                    .iter()
                    .map(|(p, f)| {
                        format!(
                            "{}: {}",
                            escape_string(&p.to_string()),
                            escape_string(f)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "IMPORT CONNECTOR {} FROM {{ {} }}{}",
                    self.name, body, with_tail
                )
            }
            ImportConnectorSource::Glob { pattern } => {
                format!(
                    "IMPORT CONNECTOR {} FROM {}{}",
                    self.name,
                    escape_string(pattern),
                    with_tail
                )
            }
        }
    }
}

/// Quick check used by the parser to disambiguate single-quoted-string FROM
/// values that should be treated as glob patterns.
fn has_glob_placeholder(s: &str) -> bool {
    s.contains("{os}") || s.contains("{arch}") || s.contains("{ext}")
}

impl BundleBuilderCommand for ImportConnectorCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        // Resolve the requested source into a flat list of (platform, from) pairs.
        let entries: Vec<(Platform, String)> = match &self.source {
            ImportConnectorSource::Single { from, platform } => {
                vec![(platform.clone(), from.clone())]
            }
            ImportConnectorSource::Multi { entries } => {
                // Reject duplicate platform keys — the registry would silently
                // keep both, but the user almost certainly meant one.
                let mut seen = std::collections::HashSet::new();
                for (p, _) in entries {
                    if !seen.insert(p.clone()) {
                        return Err(format!(
                            "IMPORT CONNECTOR platform map has duplicate platform '{}'",
                            p
                        )
                        .into());
                    }
                }
                entries.clone()
            }
            ImportConnectorSource::Glob { pattern } => expand_glob_pattern(pattern)?,
        };

        if entries.is_empty() {
            return Err("IMPORT CONNECTOR resolved to zero entries".into());
        }
        let n_entries = entries.len();

        // FFI shared libraries can't be `dlopen`-ed from inside an archive —
        // the OS dynamic linker only loads from real filesystem paths. Reject
        // FFI imports into a tar (or other archive-backed) bundle up front
        // with an actionable message rather than letting verification, or
        // worse, the recipient's `FETCH`, fail with a cryptic dlopen error.
        let data_dir_url = builder.data_dir().url().clone();
        let data_dir_scheme = data_dir_url.scheme().to_string();
        let data_dir_is_archive = data_dir_scheme.starts_with("tar+");
        if data_dir_is_archive {
            for (_, from_str) in &entries {
                if from_str.starts_with("ffi::") || from_str.starts_with("lib::") {
                    return Err(format!(
                        "Cannot IMPORT CONNECTOR with an `ffi::` runtime into a {} bundle: \
                        shared libraries can't be loaded from inside an archive. \
                        Build the bundle as a directory first, then package it with \
                        EXPORT TAR (or `tar` on the filesystem). Recipients of the tar \
                        must extract it before running FETCH. \
                        (data_dir = {})",
                        data_dir_scheme, data_dir_url
                    )
                    .into());
                }
            }
        }

        // Per-entry: parse runtime, validate, copy into bundle, verify.
        let mut prepared: Vec<(Platform, UdfRuntime)> = Vec::with_capacity(n_entries);
        for (platform, from_str) in entries {
            let runtime = UdfRuntime::parse_from(&from_str)?;
            if !runtime.can_bundle() {
                return Err(format!(
                    "'{}' runtime cannot be bundled — use import_temp_connector instead",
                    runtime.runtime_name()
                )
                .into());
            }

            let bundled_from = runtime.copy_into_bundle(&builder.data_dir()).await?;
            let resolved = bundled_from.resolve_path(&builder.data_dir());

            if resolved.runtime_name() == "ffi" {
                let path = resolved.file_path().ok_or_else(|| -> BundlebaseError {
                    "FFI connector verification requires a shared library path".into()
                })?;
                if platform.matches_current() {
                    // Host platform — full dlopen verification.
                    verify_shared_lib_connector(path)?;
                } else {
                    // Foreign platform — header-only check.
                    verify_shared_lib_header(path, &platform.os, &platform.arch)?;
                }
            } else {
                // Non-FFI runtimes (ipc, java, etc.) don't have foreign-platform
                // binaries to validate; the existing per-runtime check works
                // for any platform.
                resolved
                    .verify_bundled_connector(&builder.data_dir())
                    .await?;
            }

            prepared.push((platform, bundled_from));
        }

        // Warn if no entry covers the host — bundles often get built on a dev
        // box that isn't a deployment target, so this is just an FYI.
        if !prepared.iter().any(|(p, _)| p.matches_current()) {
            tracing::warn!(
                "IMPORT CONNECTOR {}: no entry covers host platform {} — connector won't be loadable here",
                self.name,
                Platform::current()
            );
        }

        // Copy the optional source archive into the bundle (content-addressed)
        // and record the bundle-relative path. All entries share one src.
        let bundled_src: Option<String> = match &self.src {
            Some(src_path) => Some(copy_src_into_bundle(src_path, &builder.data_dir()).await?),
            None => None,
        };

        let name = self.name.clone();
        let description = format!("IMPORT CONNECTOR {} ({} entries)", name, n_entries);
        builder
            .do_change(&description, |b| {
                let prepared = prepared.clone();
                let name = name.clone();
                let bundled_src = bundled_src.clone();
                Box::pin(async move {
                    for (platform, bundled_from) in prepared {
                        let op = ImportConnectorOp::new(name.clone(), bundled_from, platform)
                            .with_src(bundled_src.clone());
                        b.apply_operation(op.into()).await?;
                    }
                    Ok(())
                })
            })
            .await?;

        Ok(format!(
            "Loaded connector: {} ({} platform{}{})",
            self.name,
            n_entries,
            if n_entries == 1 { "" } else { "s" },
            if bundled_src.is_some() { ", src bundled" } else { "" }
        ))
    }
}

/// Copy a source archive into the bundle's data directory (content-addressed)
/// and return the bundle-relative path. Used by `IMPORT CONNECTOR ... WITH (src = '...')`.
async fn copy_src_into_bundle(
    src_path: &str,
    data_dir: &std::sync::Arc<dyn bundlebase_io::IOReadWriteDir>,
) -> Result<String, BundlebaseError> {
    use bundlebase_common::{ContentAddress, ContentCategory, ContentFormat};

    let abs_path = if std::path::Path::new(src_path).is_absolute() {
        std::path::PathBuf::from(src_path)
    } else {
        std::env::current_dir()
            .map_err(|e| BundlebaseError::from(format!("Failed to get cwd: {}", e)))?
            .join(src_path)
    };
    let bytes = tokio::fs::read(&abs_path).await.map_err(|e| {
        BundlebaseError::from(format!(
            "Failed to read connector src '{}': {}",
            abs_path.display(),
            e
        ))
    })?;

    // Pick the format from the file extension (zip is the documented case but
    // tar/etc. are accepted because content-addressed storage doesn't care).
    let ext = abs_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("zip");
    let format = ContentFormat::from_extension(ext).unwrap_or(ContentFormat::Zip);
    let address = ContentAddress::new(ContentCategory::Udf, format);
    let stream = futures::stream::once(async move {
        Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(bytes))
    });
    let written = data_dir.write_stream(Box::pin(stream), &address).await?;
    let hash = &written.hash;
    Ok(format!(
        "{}/{}.{}",
        &hash[..2],
        &hash[2..16],
        address.extension()
    ))
}

/// Expand a `{os}`/`{arch}`/`{ext}` glob pattern by scanning the filesystem
/// directory it points into. Returns one (Platform, "ffi::<path>") entry per
/// match. Errors if the pattern resolves to zero files.
fn expand_glob_pattern(pattern: &str) -> Result<Vec<(Platform, String)>, BundlebaseError> {
    // Pull the runtime prefix off (e.g. "ffi::./weather-{os}-{arch}.{ext}") so
    // we can re-prepend it onto each discovered file.
    let (runtime_prefix, body) = match pattern.find("::") {
        Some(pos) => (&pattern[..pos + 2], &pattern[pos + 2..]),
        None => ("", pattern),
    };

    if !has_glob_placeholder(body) {
        return Err(format!(
            "Glob pattern '{}' has no {{os}}, {{arch}}, or {{ext}} placeholders",
            pattern
        )
        .into());
    }

    // Split off the directory we need to scan. We assume placeholders only
    // appear in the filename, not in directory components — the common case.
    let path = std::path::Path::new(body);
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
    let file_pattern = path
        .file_name()
        .ok_or_else(|| -> BundlebaseError {
            format!("Glob pattern '{}' has no filename component", pattern).into()
        })?
        .to_string_lossy()
        .to_string();

    if file_pattern.contains("/") || file_pattern.contains('\\') {
        return Err(format!(
            "Glob pattern '{}' must keep placeholders within the filename, not the directory.",
            pattern
        )
        .into());
    }

    // Build a regex from the file pattern: literal characters except for
    // `{os}`/`{arch}`/`{ext}`, which become named capture groups.
    let regex_str = build_glob_regex(&file_pattern);
    let re = regex::Regex::new(&regex_str).map_err(|e| -> BundlebaseError {
        format!("Failed to build regex for glob '{}': {}", pattern, e).into()
    })?;

    let mut entries = Vec::new();
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let read_dir = std::fs::read_dir(dir).map_err(|e| -> BundlebaseError {
        format!("Failed to scan directory '{}' for glob: {}", dir.display(), e).into()
    })?;

    for dent in read_dir.flatten() {
        let fname_os = dent.file_name();
        let fname = fname_os.to_string_lossy();
        let Some(caps) = re.captures(&fname) else {
            continue;
        };
        let os = caps.name("os").map(|m| m.as_str().to_string());
        let arch = caps.name("arch").map(|m| m.as_str().to_string());
        let ext = caps.name("ext").map(|m| m.as_str().to_string());

        // {os} and {arch} are required for a meaningful platform — fall back
        // to wildcards only if the user didn't include the placeholder.
        let os_val = os.unwrap_or_else(|| "*".to_string());
        let arch_val = arch.unwrap_or_else(|| "*".to_string());

        // {ext} must be valid for the captured os if both are present.
        if let (Some(e), Some(o)) = (ext.as_ref(), Some(&os_val)) {
            if !ext_matches_os(e, o) {
                continue;
            }
        }

        let plat: Platform = format!("{}/{}", os_val, arch_val).parse()?;
        if !seen.insert((plat.os.clone(), plat.arch.clone())) {
            continue;
        }

        let full_path = dir.join(fname.as_ref());
        let from_str = format!("{}{}", runtime_prefix, full_path.display());
        entries.push((plat, from_str));
    }

    if entries.is_empty() {
        return Err(format!(
            "Glob pattern '{}' matched zero files in {}",
            pattern,
            dir.display()
        )
        .into());
    }
    if entries.len() == 1 {
        tracing::warn!(
            "IMPORT CONNECTOR glob '{}' matched only 1 file — consider the explicit single-platform form.",
            pattern
        );
    }
    Ok(entries)
}

fn build_glob_regex(file_pattern: &str) -> String {
    // Walk char-by-char so we can replace exactly `{os}`, `{arch}`, `{ext}`
    // and escape everything else.
    let mut out = String::with_capacity(file_pattern.len() + 32);
    out.push('^');
    let mut rest = file_pattern;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix("{os}") {
            out.push_str(r"(?P<os>[A-Za-z0-9_]+)");
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("{arch}") {
            out.push_str(r"(?P<arch>[A-Za-z0-9_]+)");
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("{ext}") {
            out.push_str(r"(?P<ext>[A-Za-z0-9_]+)");
            rest = stripped;
        } else {
            let c = rest.chars().next().unwrap();
            out.push_str(&regex::escape(&c.to_string()));
            rest = &rest[c.len_utf8()..];
        }
    }
    out.push('$');
    out
}

fn ext_matches_os(ext: &str, os: &str) -> bool {
    match os {
        "linux" => ext == "so",
        "darwin" => ext == "dylib" || ext == "so",
        "windows" => ext == "dll",
        _ => true,
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;
    use bundlebase_common::Platform;

    fn unwrap_single(c: &ImportConnectorCommand) -> (&str, &Platform) {
        match &c.source {
            ImportConnectorSource::Single { from, platform } => (from.as_str(), platform),
            other => panic!("expected Single, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_import_connector() {
        let input = "IMPORT CONNECTOR acme.weather FROM 'ipc::./my_source'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                let (from, platform) = unwrap_single(&c);
                assert_eq!(from, "ipc::./my_source");
                assert_eq!(*platform, Platform::any());
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_connector_with_platform() {
        let input =
            "IMPORT CONNECTOR acme.weather FROM 'ffi::./lib.so' WITH (platform = 'linux/amd64')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                let (from, platform) = unwrap_single(&c);
                assert_eq!(from, "ffi::./lib.so");
                assert_eq!(*platform, "linux/amd64".parse::<Platform>().unwrap());
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_connector_deep_name_parses_but_check_rejects() {
        let input = "IMPORT CONNECTOR acme.weather FROM 'ipc::./weather'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.name, "acme.weather");
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_connector_roundtrip() {
        let cmd = ImportConnectorCommand::new(
            "acme.weather",
            "ffi::/usr/lib/weather.so",
            Platform::any(),
        );
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "IMPORT CONNECTOR acme.weather FROM 'ffi::/usr/lib/weather.so'"
        );
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                let (from, platform) = unwrap_single(&c);
                assert_eq!(from, "ffi::/usr/lib/weather.so");
                assert_eq!(*platform, Platform::any());
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_connector_roundtrip_with_platform() {
        let cmd = ImportConnectorCommand::new(
            "acme.weather",
            "ipc::./my_source",
            "linux/amd64".parse().unwrap(),
        );
        let statement = cmd.to_statement();
        assert!(statement.contains("WITH (platform = 'linux/amd64')"));
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportConnector(c) => {
                let (_from, platform) = unwrap_single(&c);
                assert_eq!(platform.os, "linux");
                assert_eq!(platform.arch, "amd64");
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_connector_case_insensitive() {
        let input = "load connector acme.weather from 'ipc::./test'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                let (from, _) = unwrap_single(&c);
                assert_eq!(from, "ipc::./test");
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    // ----- multi-platform map form -----

    #[test]
    fn test_parse_platform_map_two_entries() {
        let input = "IMPORT CONNECTOR acme.weather FROM { \
            'linux/amd64'  : 'ffi::./lib-linux.so', \
            'darwin/arm64' : 'ffi::./lib-mac.dylib' \
        }";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                match &c.source {
                    ImportConnectorSource::Multi { entries } => {
                        assert_eq!(entries.len(), 2);
                        assert_eq!(entries[0].0, "linux/amd64".parse::<Platform>().unwrap());
                        assert_eq!(entries[0].1, "ffi::./lib-linux.so");
                        assert_eq!(entries[1].0, "darwin/arm64".parse::<Platform>().unwrap());
                        assert_eq!(entries[1].1, "ffi::./lib-mac.dylib");
                    }
                    other => panic!("expected Multi, got {:?}", other),
                }
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_platform_map_trailing_comma() {
        let input = "IMPORT CONNECTOR acme.weather FROM { 'linux/amd64' : 'ffi::./a.so', }";
        let cmd = parse_command(input).expect("trailing comma should parse");
        match cmd {
            BundleCommand::ImportConnector(c) => match &c.source {
                ImportConnectorSource::Multi { entries } => assert_eq!(entries.len(), 1),
                other => panic!("expected Multi, got {:?}", other),
            },
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_platform_map_with_clause_rejected() {
        let input = "IMPORT CONNECTOR acme.weather FROM { 'linux/amd64' : 'ffi::./a.so' } \
            WITH (platform = 'darwin/arm64')";
        let err = parse_command(input).unwrap_err();
        assert!(
            err.to_string().contains("cannot combine platform map"),
            "got: {}",
            err
        );
    }

    #[test]
    fn test_roundtrip_multi() {
        let cmd = ImportConnectorCommand::new_multi(
            "acme.weather",
            vec![
                ("linux/amd64".parse().unwrap(), "ffi::./a.so".to_string()),
                ("darwin/arm64".parse().unwrap(), "ffi::./b.dylib".to_string()),
            ],
        );
        let stmt = cmd.to_statement();
        assert!(stmt.contains("FROM {"), "got: {}", stmt);
        assert!(stmt.contains("'linux/amd64': 'ffi::./a.so'"), "got: {}", stmt);
        let parsed = parse_command(&stmt).unwrap();
        match parsed {
            BundleCommand::ImportConnector(c) => match c.source {
                ImportConnectorSource::Multi { entries } => assert_eq!(entries.len(), 2),
                other => panic!("expected Multi, got {:?}", other),
            },
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    // ----- glob form -----

    #[test]
    fn test_parse_glob_form() {
        let input = "IMPORT CONNECTOR acme.weather FROM 'ffi::./weather-{os}-{arch}.{ext}'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportConnector(c) => match c.source {
                ImportConnectorSource::Glob { pattern } => {
                    assert_eq!(pattern, "ffi::./weather-{os}-{arch}.{ext}");
                }
                other => panic!("expected Glob, got {:?}", other),
            },
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_glob_with_platform_rejected() {
        let input = "IMPORT CONNECTOR acme.weather FROM 'ffi::./weather-{os}-{arch}.{ext}' \
            WITH (platform = 'linux/amd64')";
        let err = parse_command(input).unwrap_err();
        assert!(err.to_string().contains("cannot combine"), "got: {}", err);
    }

    #[test]
    fn test_glob_regex_matches_expected_files() {
        let re = regex::Regex::new(&build_glob_regex("weather-{os}-{arch}.{ext}")).unwrap();
        let caps = re.captures("weather-linux-amd64.so").unwrap();
        assert_eq!(&caps["os"], "linux");
        assert_eq!(&caps["arch"], "amd64");
        assert_eq!(&caps["ext"], "so");
        assert!(re.captures("weather-linux-amd64.zip").is_some()); // ext check happens in caller
        assert!(re.captures("unrelated-file.txt").is_none());
    }

    #[test]
    fn test_glob_expand_real_dir() {
        let dir = tempfile::tempdir().unwrap();
        for name in &[
            "weather-linux-amd64.so",
            "weather-linux-arm64.so",
            "weather-darwin-arm64.dylib",
            "weather-windows-amd64.dll",
            "unrelated.txt",
        ] {
            std::fs::write(dir.path().join(name), b"").unwrap();
        }
        let pattern = format!(
            "ffi::{}/weather-{{os}}-{{arch}}.{{ext}}",
            dir.path().display()
        );
        let entries = expand_glob_pattern(&pattern).unwrap();
        let mut platforms: Vec<String> = entries.iter().map(|(p, _)| p.to_string()).collect();
        platforms.sort();
        assert_eq!(
            platforms,
            vec![
                "darwin/arm64".to_string(),
                "linux/amd64".to_string(),
                "linux/arm64".to_string(),
                "windows/amd64".to_string(),
            ]
        );
        // Each entry's `from` string should keep the runtime prefix.
        assert!(entries.iter().all(|(_, f)| f.starts_with("ffi::")));
    }

    #[test]
    fn test_glob_expand_zero_matches_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("nothing-related.txt"), b"").unwrap();
        let pattern = format!(
            "ffi::{}/weather-{{os}}-{{arch}}.{{ext}}",
            dir.path().display()
        );
        let err = expand_glob_pattern(&pattern).unwrap_err();
        assert!(err.to_string().contains("matched zero files"), "got: {}", err);
    }

    #[test]
    fn test_glob_pattern_without_placeholders_errors() {
        let err = expand_glob_pattern("ffi::./weather-linux-amd64.so").unwrap_err();
        assert!(err.to_string().contains("no {os}"), "got: {}", err);
    }

    // ----- src attribute -----

    #[test]
    fn test_parse_with_src_single() {
        let cmd = parse_command(
            "IMPORT CONNECTOR acme.weather FROM 'ffi::./lib.so' \
             WITH (platform = 'linux/amd64', src = '/tmp/source.zip')",
        )
        .unwrap();
        match cmd {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.src.as_deref(), Some("/tmp/source.zip"));
                let (_from, plat) = unwrap_single(&c);
                assert_eq!(*plat, "linux/amd64".parse::<Platform>().unwrap());
            }
            _ => panic!("expected ImportConnector"),
        }
    }

    #[test]
    fn test_parse_with_src_multi() {
        let cmd = parse_command(
            "IMPORT CONNECTOR acme.weather FROM { \
                'linux/amd64' : 'ffi::./a.so', \
                'darwin/arm64' : 'ffi::./b.dylib' \
             } WITH (src = '/tmp/source.zip')",
        )
        .unwrap();
        match cmd {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.src.as_deref(), Some("/tmp/source.zip"));
                match c.source {
                    ImportConnectorSource::Multi { entries } => assert_eq!(entries.len(), 2),
                    other => panic!("expected Multi, got {:?}", other),
                }
            }
            _ => panic!("expected ImportConnector"),
        }
    }

    #[test]
    fn test_roundtrip_with_src() {
        let cmd = ImportConnectorCommand::new(
            "acme.weather",
            "ffi::./lib.so",
            "linux/amd64".parse().unwrap(),
        )
        .with_src(Some("/tmp/source.zip".to_string()));
        let stmt = cmd.to_statement();
        assert!(stmt.contains("src = '/tmp/source.zip'"), "got: {}", stmt);
        assert!(stmt.contains("platform = 'linux/amd64'"), "got: {}", stmt);
        let parsed = parse_command(&stmt).unwrap();
        match parsed {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.src.as_deref(), Some("/tmp/source.zip"));
            }
            _ => panic!("expected ImportConnector"),
        }
    }
}
