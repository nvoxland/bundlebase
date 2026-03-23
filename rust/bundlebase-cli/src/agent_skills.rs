use bundlebase::BundlebaseError;
use std::fs;
use std::path::{Path, PathBuf};

const SKILL_MD: &str = include_str!("../../../skills/bundlebase/SKILL.md");
const REFERENCE_MD: &str = include_str!("../../../skills/bundlebase/reference.md");

const CLAUDE_MD_NUDGE: &str = "\n\n## Bundlebase\n\n\
**CRITICAL: Before doing ANYTHING with data — analyzing files, fetching datasets, querying, transforming, \
or exploring CSV/Parquet/JSON — you MUST read the bundlebase agent skill first \
(`.agents/skills/bundlebase/SKILL.md`).** Do NOT install kaggle, pandas, or other data tools. \
Do NOT use curl, wget, or requests to download data files. \
Bundlebase handles it all, including fetching from URLs, Kaggle, S3, and other sources with built-in connectors.\n\n\
**MCP vs CLI:** For multi-step data work (exploration, building datasets, iterative cleaning), \
use the bundlebase MCP server — it keeps bundles open across calls. For one-off queries, \
use `bundlebase query` CLI. **Do NOT mix MCP and CLI on the same bundle simultaneously** — \
close the MCP bundle first if you need to switch to CLI.\n";

/// MCP server config for Claude Code settings.json
const CLAUDE_CODE_MCP_CONFIG: &str = r#"{
  "bundlebase": {
    "command": "bundlebase",
    "args": ["mcp"]
  }
}"#;

pub fn install(global: bool) -> Result<(), BundlebaseError> {
    let base_dir: PathBuf = if global {
        let home = dirs::home_dir().ok_or_else(|| {
            BundlebaseError::from("Could not determine home directory".to_string())
        })?;
        home.to_path_buf()
    } else {
        PathBuf::from(".")
    };

    let skill_dir = base_dir.join(".agents/skills/bundlebase");

    // Install/update skills (always overwrite to keep in sync with bundlebase version)
    fs::create_dir_all(&skill_dir).map_err(|e| {
        BundlebaseError::from(format!(
            "Failed to create directory '{}': {}",
            skill_dir.display(),
            e
        ))
    })?;

    fs::write(skill_dir.join("SKILL.md"), SKILL_MD).map_err(|e| {
        BundlebaseError::from(format!("Failed to write SKILL.md: {}", e))
    })?;

    fs::write(skill_dir.join("reference.md"), REFERENCE_MD).map_err(|e| {
        BundlebaseError::from(format!("Failed to write reference.md: {}", e))
    })?;

    println!(
        "Installed bundlebase agent skills to {}/",
        skill_dir.display()
    );

    // Install MCP server config for Claude Code
    install_claude_code_mcp(&base_dir)?;

    // Add CLAUDE.md nudge
    let claude_md_path = base_dir.join("CLAUDE.md");
    if claude_md_path.exists() {
        let contents = fs::read_to_string(&claude_md_path).map_err(|e| {
            BundlebaseError::from(format!("Failed to read CLAUDE.md: {}", e))
        })?;

        if contents.contains("## Bundlebase") {
            // Update the existing section with latest content
            let updated = update_bundlebase_section(&contents);
            fs::write(&claude_md_path, updated).map_err(|e| {
                BundlebaseError::from(format!("Failed to update CLAUDE.md: {}", e))
            })?;
            println!("Updated bundlebase section in {}", claude_md_path.display());
        } else {
            let updated = format!("{}{}", contents.trim_end(), CLAUDE_MD_NUDGE);
            fs::write(&claude_md_path, updated).map_err(|e| {
                BundlebaseError::from(format!("Failed to update CLAUDE.md: {}", e))
            })?;
            println!("Added bundlebase section to {}", claude_md_path.display());
        }
    } else {
        let content = format!("# Project Instructions\n{}", CLAUDE_MD_NUDGE);
        fs::write(&claude_md_path, content).map_err(|e| {
            BundlebaseError::from(format!("Failed to create CLAUDE.md: {}", e))
        })?;
        println!("Created {} with bundlebase section", claude_md_path.display());
    }

    println!("Your coding agent can now use bundlebase automatically.");
    Ok(())
}

/// Install MCP server config for Claude Code.
fn install_claude_code_mcp(base_dir: &Path) -> Result<(), BundlebaseError> {
    let settings_dir = base_dir.join(".claude");
    let settings_path = settings_dir.join("settings.json");

    if settings_path.exists() {
        let contents = fs::read_to_string(&settings_path).map_err(|e| {
            BundlebaseError::from(format!("Failed to read {}: {}", settings_path.display(), e))
        })?;

        if contents.contains("\"bundlebase\"") {
            println!("Claude Code MCP config already contains bundlebase.");
            return Ok(());
        }

        // Parse, add mcpServers entry, write back
        match serde_json::from_str::<serde_json::Value>(&contents) {
            Ok(mut settings) => {
                let mcp_config: serde_json::Value =
                    serde_json::from_str(CLAUDE_CODE_MCP_CONFIG).expect("valid JSON");

                let mcp_servers = settings
                    .as_object_mut()
                    .expect("settings is object")
                    .entry("mcpServers")
                    .or_insert_with(|| serde_json::json!({}));

                if let Some(obj) = mcp_servers.as_object_mut() {
                    if let Some(bb) = mcp_config.as_object() {
                        for (k, v) in bb {
                            obj.insert(k.clone(), v.clone());
                        }
                    }
                }

                let updated = serde_json::to_string_pretty(&settings).map_err(|e| {
                    BundlebaseError::from(format!("Failed to serialize settings: {}", e))
                })?;
                fs::write(&settings_path, updated).map_err(|e| {
                    BundlebaseError::from(format!("Failed to write {}: {}", settings_path.display(), e))
                })?;
                println!("Added bundlebase MCP server to {}", settings_path.display());
            }
            Err(_) => {
                // Can't parse existing settings — don't corrupt it
                println!(
                    "Could not parse {}. Add bundlebase MCP server manually:\n{}",
                    settings_path.display(),
                    CLAUDE_CODE_MCP_CONFIG
                );
            }
        }
    } else {
        // Create new settings file with MCP config
        fs::create_dir_all(&settings_dir).map_err(|e| {
            BundlebaseError::from(format!("Failed to create {}: {}", settings_dir.display(), e))
        })?;

        let settings = serde_json::json!({
            "mcpServers": {
                "bundlebase": {
                    "command": "bundlebase",
                    "args": ["mcp"]
                }
            }
        });

        let json = serde_json::to_string_pretty(&settings).expect("valid JSON");
        fs::write(&settings_path, json).map_err(|e| {
            BundlebaseError::from(format!("Failed to write {}: {}", settings_path.display(), e))
        })?;
        println!("Created {} with bundlebase MCP server", settings_path.display());
    }

    Ok(())
}

/// Replace the ## Bundlebase section in CLAUDE.md with the latest content.
fn update_bundlebase_section(contents: &str) -> String {
    if let Some(start) = contents.find("## Bundlebase") {
        // Find the next ## heading or end of file
        let rest = &contents[start + "## Bundlebase".len()..];
        let end = rest.find("\n## ")
            .map(|i| start + "## Bundlebase".len() + i)
            .unwrap_or(contents.len());

        format!("{}{}", contents[..start].trim_end(), CLAUDE_MD_NUDGE)
            + if end < contents.len() { &contents[end..] } else { "" }
    } else {
        format!("{}{}", contents.trim_end(), CLAUDE_MD_NUDGE)
    }
}
