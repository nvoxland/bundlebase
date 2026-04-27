//! End-to-end tests for multi-platform IMPORT CONNECTOR.
//!
//! Covers the explicit map form, the glob form, and the empty-export
//! round-trip. Uses synthetic ELF/Mach-O/PE headers so the structural verifier
//! accepts the binaries without a real cross-compile toolchain.

use bundlebase::bundle::{BundleBuilder, BundleFacade};
use bundlebase_command::parser::parse_command;
use bundlebase_command::{BundleBuilderCommand, BundleCommand, BundleFacadeCommand};
use bundlebase_common::{BundlebaseError, Platform};
use tempfile::TempDir;

mod common;

fn init() {
    common::init_catalog();
}

fn fake_elf(e_machine: u16) -> Vec<u8> {
    let mut v = vec![0u8; 64];
    v[0..4].copy_from_slice(b"\x7FELF");
    v[18..20].copy_from_slice(&e_machine.to_le_bytes());
    v
}

fn fake_macho(cputype: u32) -> Vec<u8> {
    let mut v = vec![0u8; 32];
    v[0..4].copy_from_slice(&0xFEEDFACFu32.to_le_bytes());
    v[4..8].copy_from_slice(&cputype.to_le_bytes());
    v
}

fn fake_pe(machine: u16) -> Vec<u8> {
    let mut v = vec![0u8; 0x100];
    v[0..2].copy_from_slice(b"MZ");
    let pe_off: u32 = 0x80;
    v[0x3C..0x40].copy_from_slice(&pe_off.to_le_bytes());
    v[0x80..0x84].copy_from_slice(b"PE\0\0");
    v[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
    v
}

/// Stage four cross-platform "binaries" in a temp dir. Avoid the host's
/// platform so we don't trip the full `dlopen` verifier on the matching entry.
struct FakeBinaries {
    dir: TempDir,
    /// (platform_string, file_name)
    files: Vec<(&'static str, &'static str)>,
}

fn stage_fake_binaries() -> FakeBinaries {
    let dir = tempfile::tempdir().unwrap();

    // Pick three foreign platforms — host is always darwin/arm64 or linux/amd64
    // in CI, so this set excludes both common dev hosts.
    std::fs::write(dir.path().join("weather-windows-amd64.dll"), fake_pe(0x8664)).unwrap();
    std::fs::write(dir.path().join("weather-windows-arm64.dll"), fake_pe(0xAA64)).unwrap();
    let foreign_elf = if Platform::current().os == "linux" {
        // host is linux — pick a non-host arch and a non-linux os
        std::fs::write(dir.path().join("weather-linux-arm64.so"), fake_elf(0xB7)).unwrap();
        std::fs::write(dir.path().join("weather-darwin-arm64.dylib"), fake_macho(0x0100000C)).unwrap();
        ("linux/arm64", "darwin/arm64")
    } else {
        std::fs::write(dir.path().join("weather-linux-amd64.so"), fake_elf(0x3E)).unwrap();
        std::fs::write(dir.path().join("weather-darwin-amd64.dylib"), fake_macho(0x01000007)).unwrap();
        ("linux/amd64", "darwin/amd64")
    };

    FakeBinaries {
        dir,
        files: vec![
            ("windows/amd64", "weather-windows-amd64.dll"),
            ("windows/arm64", "weather-windows-arm64.dll"),
            (foreign_elf.0, if foreign_elf.0 == "linux/arm64" { "weather-linux-arm64.so" } else { "weather-linux-amd64.so" }),
            (foreign_elf.1, if foreign_elf.1 == "darwin/arm64" { "weather-darwin-arm64.dylib" } else { "weather-darwin-amd64.dylib" }),
        ],
    }
}

#[tokio::test]
async fn test_import_connector_platform_map_registers_all_entries() -> Result<(), BundlebaseError>
{
    init();
    let stage = stage_fake_binaries();

    let map_body = stage
        .files
        .iter()
        .map(|(plat, file)| {
            let path = stage.dir.path().join(file);
            format!("'{}': 'ffi::{}'", plat, path.display())
        })
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!(
        "IMPORT CONNECTOR acme.weather FROM {{ {} }}",
        map_body
    );

    let bundle_dir = tempfile::tempdir().unwrap();
    let builder = BundleBuilder::create(
        &format!("file://{}/multi", bundle_dir.path().display()),
        None,
    )
    .await?;

    let cmd = match parse_command(&sql).expect("parse") {
        BundleCommand::ImportConnector(c) => c,
        other => panic!("expected ImportConnector, got {:?}", other),
    };
    Box::new(cmd).execute(&builder).await?;

    let registry = builder.bundle().connector_registry();
    let entries: Vec<_> = registry
        .read()
        .entries()
        .iter()
        .filter(|e| e.name.to_string() == "acme.weather")
        .cloned()
        .collect();
    assert_eq!(
        entries.len(),
        4,
        "expected one entry per requested platform, got {:?}",
        entries
    );

    let mut platforms: Vec<String> = entries.iter().map(|e| e.platform.to_string()).collect();
    platforms.sort();
    let mut expected: Vec<String> = stage.files.iter().map(|(p, _)| p.to_string()).collect();
    expected.sort();
    assert_eq!(platforms, expected);

    // Each entry's bundled `from` path is content-addressed inside data_dir,
    // and resolves to a real file on disk.
    for entry in &entries {
        let resolved = entry.from.resolve_path(&builder.bundle().data_dir());
        let path = resolved.file_path().expect("ffi path");
        assert!(
            std::path::Path::new(path).exists(),
            "bundled binary {} should exist",
            path
        );
    }

    Ok(())
}

#[tokio::test]
async fn test_import_connector_glob_expands_filesystem() -> Result<(), BundlebaseError> {
    init();
    let stage = stage_fake_binaries();

    let pattern = format!(
        "ffi::{}/weather-{{os}}-{{arch}}.{{ext}}",
        stage.dir.path().display()
    );
    let sql = format!("IMPORT CONNECTOR acme.weather FROM '{}'", pattern);

    let bundle_dir = tempfile::tempdir().unwrap();
    let builder = BundleBuilder::create(
        &format!("file://{}/glob", bundle_dir.path().display()),
        None,
    )
    .await?;

    let cmd = match parse_command(&sql).expect("parse") {
        BundleCommand::ImportConnector(c) => c,
        other => panic!("expected ImportConnector, got {:?}", other),
    };
    Box::new(cmd).execute(&builder).await?;

    let registry = builder.bundle().connector_registry();
    let entries: Vec<_> = registry
        .read()
        .entries()
        .iter()
        .filter(|e| e.name.to_string() == "acme.weather")
        .cloned()
        .collect();
    assert_eq!(entries.len(), 4, "glob should match all 4 staged files");

    Ok(())
}

#[tokio::test]
async fn test_import_connector_platform_map_duplicate_rejected(
) -> Result<(), BundlebaseError> {
    init();
    let stage = stage_fake_binaries();
    let p1 = stage.dir.path().join("weather-windows-amd64.dll");

    // Same platform twice -> command must reject.
    let sql = format!(
        "IMPORT CONNECTOR acme.weather FROM {{ \
            'windows/amd64' : 'ffi::{}', \
            'windows/amd64' : 'ffi::{}' \
        }}",
        p1.display(),
        p1.display()
    );

    let bundle_dir = tempfile::tempdir().unwrap();
    let builder = BundleBuilder::create(
        &format!("file://{}/dup", bundle_dir.path().display()),
        None,
    )
    .await?;

    let cmd = match parse_command(&sql).expect("parse") {
        BundleCommand::ImportConnector(c) => c,
        other => panic!("expected ImportConnector, got {:?}", other),
    };
    let err = Box::new(cmd).execute(&builder).await.unwrap_err();
    assert!(
        err.to_string().contains("duplicate platform"),
        "got: {}",
        err
    );

    Ok(())
}

#[tokio::test]
async fn test_export_empty_includes_all_platform_binaries() -> Result<(), BundlebaseError> {
    init();

    let stage = stage_fake_binaries();
    let map_body = stage
        .files
        .iter()
        .map(|(plat, file)| {
            let path = stage.dir.path().join(file);
            format!("'{}': 'ffi::{}'", plat, path.display())
        })
        .collect::<Vec<_>>()
        .join(", ");

    let bundle_dir = tempfile::tempdir().unwrap();
    let src_url = format!("file://{}/src", bundle_dir.path().display());
    let builder = BundleBuilder::create(&src_url, None).await?;

    // Run the import, then commit so the change is persisted.
    let sql = format!(
        "IMPORT CONNECTOR acme.weather FROM {{ {} }}",
        map_body
    );
    let cmd = match parse_command(&sql).expect("parse") {
        BundleCommand::ImportConnector(c) => c,
        other => panic!("expected ImportConnector, got {:?}", other),
    };
    Box::new(cmd).execute(&builder).await?;
    builder.commit("import").await?;

    // Snapshot the data_dir's files so we know what to look for in the empty.
    let src_files: std::collections::HashSet<String> = std::fs::read_dir(bundle_dir.path().join("src"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir() && e.file_name() != "_bundlebase")
        .flat_map(|sub| {
            let parent = sub.file_name().to_string_lossy().into_owned();
            std::fs::read_dir(sub.path()).unwrap().flatten().map(move |e| {
                let name = e.file_name().to_string_lossy().into_owned();
                format!("{}/{}", parent, name)
            })
        })
        .collect();
    assert_eq!(
        src_files.len(),
        4,
        "source bundle should contain 4 connector binaries, got {:?}",
        src_files
    );

    // Export empty.
    let empty_path = bundle_dir.path().join("empty");
    let export_sql = format!("EXPORT EMPTY TO 'file://{}'", empty_path.display());
    let cmd = match parse_command(&export_sql).expect("parse") {
        BundleCommand::ExportEmpty(c) => c,
        other => panic!("expected ExportEmpty, got {:?}", other),
    };
    Box::new(cmd).execute(&builder).await?;

    // The empty bundle's data_dir should contain the same 4 binaries.
    let empty_files: std::collections::HashSet<String> = std::fs::read_dir(&empty_path)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir() && e.file_name() != "_bundlebase")
        .flat_map(|sub| {
            let parent = sub.file_name().to_string_lossy().into_owned();
            std::fs::read_dir(sub.path()).unwrap().flatten().map(move |e| {
                let name = e.file_name().to_string_lossy().into_owned();
                format!("{}/{}", parent, name)
            })
        })
        .collect();
    assert_eq!(
        empty_files, src_files,
        "empty bundle should contain identical connector binaries"
    );

    Ok(())
}

#[tokio::test]
async fn test_import_connector_with_src_bundles_archive() -> Result<(), BundlebaseError> {
    init();
    let stage = stage_fake_binaries();

    // Stage a fake source archive next to the binaries.
    let src_path = stage.dir.path().join("weather-source.zip");
    let archive_bytes = b"PK\x03\x04 fake zip contents for testing";
    std::fs::write(&src_path, archive_bytes).unwrap();

    let bundle_dir = tempfile::tempdir().unwrap();
    let builder = BundleBuilder::create(
        &format!("file://{}/withsrc", bundle_dir.path().display()),
        None,
    )
    .await?;

    let map_body = stage
        .files
        .iter()
        .map(|(plat, file)| {
            let path = stage.dir.path().join(file);
            format!("'{}': 'ffi::{}'", plat, path.display())
        })
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "IMPORT CONNECTOR acme.weather FROM {{ {} }} WITH (src = '{}')",
        map_body,
        src_path.display()
    );
    let cmd = match parse_command(&sql).expect("parse") {
        BundleCommand::ImportConnector(c) => c,
        other => panic!("expected ImportConnector, got {:?}", other),
    };
    Box::new(cmd).execute(&builder).await?;

    // All entries should share the same `src`.
    let registry = builder.bundle().connector_registry();
    let entries: Vec<_> = registry
        .read()
        .entries()
        .iter()
        .filter(|e| e.name.to_string() == "acme.weather")
        .cloned()
        .collect();
    assert_eq!(entries.len(), 4);
    let srcs: std::collections::HashSet<_> = entries.iter().map(|e| e.src.clone()).collect();
    assert_eq!(srcs.len(), 1, "all entries should share one src");
    let bundled_src = entries[0].src.clone().expect("src must be bundled");
    assert!(bundled_src.ends_with(".udf.zip"), "got: {}", bundled_src);

    // The bundled file should exist in data_dir with the original bytes.
    let f = builder.bundle().data_dir().file(&bundled_src)?;
    let bytes = f.read_bytes().await?.expect("file present");
    assert_eq!(&bytes[..], &archive_bytes[..]);

    Ok(())
}

#[tokio::test]
async fn test_export_source_writes_archive() -> Result<(), BundlebaseError> {
    use bundlebase_command::ExportSourceCommand;

    init();
    let stage = stage_fake_binaries();
    let src_path = stage.dir.path().join("weather-source.zip");
    let archive_bytes = b"PK\x03\x04 fake zip for export test";
    std::fs::write(&src_path, archive_bytes).unwrap();

    let bundle_dir = tempfile::tempdir().unwrap();
    let builder = BundleBuilder::create(
        &format!("file://{}/exp", bundle_dir.path().display()),
        None,
    )
    .await?;

    let (plat, file) = &stage.files[0];
    let bin_path = stage.dir.path().join(file);
    let sql = format!(
        "IMPORT CONNECTOR acme.weather FROM 'ffi::{}' WITH (platform = '{}', src = '{}')",
        bin_path.display(),
        plat,
        src_path.display()
    );
    let cmd = match parse_command(&sql).expect("parse") {
        BundleCommand::ImportConnector(c) => c,
        other => panic!("expected ImportConnector, got {:?}", other),
    };
    Box::new(cmd).execute(&builder).await?;

    // Export the source to a new location.
    let out_path = bundle_dir.path().join("extracted-source.zip");
    let cmd = ExportSourceCommand {
        connector_name: "acme.weather".to_string(),
        path: out_path.to_string_lossy().into_owned(),
    };
    BundleFacadeCommand::execute(Box::new(cmd), builder.bundle()).await?;

    let extracted = std::fs::read(&out_path).unwrap();
    assert_eq!(&extracted[..], &archive_bytes[..]);

    Ok(())
}

#[tokio::test]
async fn test_export_source_errors_when_no_src() -> Result<(), BundlebaseError> {
    use bundlebase_command::ExportSourceCommand;

    init();
    let stage = stage_fake_binaries();
    let (plat, file) = &stage.files[0];
    let bin_path = stage.dir.path().join(file);

    let bundle_dir = tempfile::tempdir().unwrap();
    let builder = BundleBuilder::create(
        &format!("file://{}/nosrc", bundle_dir.path().display()),
        None,
    )
    .await?;

    let sql = format!(
        "IMPORT CONNECTOR acme.weather FROM 'ffi::{}' WITH (platform = '{}')",
        bin_path.display(),
        plat
    );
    let cmd = match parse_command(&sql).expect("parse") {
        BundleCommand::ImportConnector(c) => c,
        other => panic!("expected ImportConnector, got {:?}", other),
    };
    Box::new(cmd).execute(&builder).await?;

    let out_path = bundle_dir.path().join("should-not-exist.zip");
    let cmd = ExportSourceCommand {
        connector_name: "acme.weather".to_string(),
        path: out_path.to_string_lossy().into_owned(),
    };
    let err = BundleFacadeCommand::execute(Box::new(cmd), builder.bundle())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("no bundled source archive"),
        "got: {}",
        err
    );
    assert!(!out_path.exists());

    Ok(())
}

#[tokio::test]
async fn test_import_connector_ffi_into_tar_bundle_rejected() -> Result<(), BundlebaseError> {
    init();
    let stage = stage_fake_binaries();
    let (plat, file) = &stage.files[0];
    let bin_path = stage.dir.path().join(file);

    // Create a bundle backed by a tar archive (tar+file:// scheme).
    let bundle_dir = tempfile::tempdir().unwrap();
    let tar_url = format!(
        "tar+file://{}/empty-tar-bundle.tar/",
        bundle_dir.path().display()
    );
    let builder = BundleBuilder::create(&tar_url, None).await?;

    // Installing an FFI binary into a tar bundle must produce a helpful
    // up-front error rather than failing later at fetch time with the
    // dynamic linker's "no such file" message.
    let sql = format!(
        "IMPORT CONNECTOR acme.weather FROM 'ffi::{}' WITH (platform = '{}')",
        bin_path.display(),
        plat
    );
    let cmd = match parse_command(&sql).expect("parse") {
        BundleCommand::ImportConnector(c) => c,
        other => panic!("expected ImportConnector, got {:?}", other),
    };
    let err = Box::new(cmd).execute(&builder).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("ffi") && msg.contains("tar+") && msg.contains("EXPORT TAR"),
        "expected actionable tar/ffi message, got: {}",
        msg
    );

    Ok(())
}
