use bundlebase_common::BundlebaseError;
use clap::ValueEnum;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

const SKILL_MD: &str = include_str!("../../../skills/bundlebase/SKILL.md");
const REFERENCE_MD: &str = include_str!("../../../skills/bundlebase/reference.md");

const AGENTS_MD: &str = "\n\n## Bundlebase\n\n\
**CRITICAL: Before doing ANYTHING with data — analyzing files, fetching datasets, querying, transforming, \
or exploring CSV/Parquet/JSON — you MUST read the bundlebase agent skill first \
(`.agents/skills/bundlebase/SKILL.md`).** Do NOT install kaggle, pandas, or other data tools. \
Do NOT use curl, wget, or requests to download data files. \
Bundlebase handles it all, including fetching from URLs, Kaggle, S3, and other sources with built-in connectors.\n\n\
**MCP vs CLI:** For multi-step data work (exploration, building datasets, iterative cleaning), \
use the bundlebase MCP server — it keeps bundles open across calls. For one-off queries, \
use `bundlebase query` CLI. **Do NOT mix MCP and CLI on the same bundle simultaneously** — \
close the MCP bundle first if you need to switch to CLI.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AgentTarget {
    Claude,
    Copilot,
}

impl AgentTarget {
    fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Copilot => "copilot",
        }
    }
}

#[derive(Debug, Default)]
struct InstallReport {
    installed: Vec<AgentTarget>,
    skipped: Vec<String>,
}

pub fn install(global: bool, target: Option<AgentTarget>) -> Result<(), BundlebaseError> {
    if global && matches!(target, Some(AgentTarget::Copilot)) {
        return Err(BundlebaseError::from(
            "Copilot setup is workspace-only. Use `bundlebase setup-agent --agent copilot --scope local`."
                .to_string(),
        ));
    }

    let base_dir = install_base_dir(global)?;
    let targets = resolve_targets(target)?;
    let report = install_selected_targets(&base_dir, global, &targets)?;

    println!(
        "Installed bundlebase setup for {}.",
        format_target_list(&report.installed)
    );

    for skipped in &report.skipped {
        println!("Skipped {}.", skipped);
    }

    Ok(())
}

fn install_base_dir(global: bool) -> Result<PathBuf, BundlebaseError> {
    if global {
        let home = dirs::home_dir().ok_or_else(|| {
            BundlebaseError::from("Could not determine home directory".to_string())
        })?;
        Ok(home)
    } else {
        Ok(PathBuf::from("."))
    }
}

fn resolve_targets(target: Option<AgentTarget>) -> Result<Vec<AgentTarget>, BundlebaseError> {
    if let Some(target) = target {
        return Ok(vec![target]);
    }

    let mut detected = Vec::new();

    if command_exists_on_path(&["claude"]) {
        detected.push(AgentTarget::Claude);
    }

    if command_exists_on_path(&["copilot"]) {
        detected.push(AgentTarget::Copilot);
    }

    if detected.is_empty() {
        return Err(BundlebaseError::from(
            "No supported agent executables were found on PATH. Install Claude Code (`claude`) or Copilot (`copilot`), or rerun with `--agent claude` or `--agent copilot`."
                .to_string(),
        ));
    }

    Ok(detected)
}

fn command_exists_on_path(commands: &[&str]) -> bool {
    let path = std::env::var_os("PATH");
    let pathext = std::env::var_os("PATHEXT");
    command_exists_in_env(commands, path.as_deref(), pathext.as_deref())
}

fn command_exists_in_env(commands: &[&str], path: Option<&OsStr>, pathext: Option<&OsStr>) -> bool {
    let paths: Vec<PathBuf> = match path {
        Some(path) => std::env::split_paths(path).collect(),
        None => Vec::new(),
    };
    let executable_extensions = executable_extensions(pathext);

    commands
        .iter()
        .any(|command| command_exists_in_paths(command, &paths, &executable_extensions))
}

fn executable_extensions(pathext: Option<&OsStr>) -> Vec<String> {
    let mut extensions = vec![String::new()];

    if cfg!(windows) {
        if let Some(pathext) = pathext.and_then(|value| value.to_str()) {
            extensions.extend(
                pathext
                    .split(';')
                    .filter(|ext| !ext.is_empty())
                    .map(ToOwned::to_owned),
            );
        } else {
            extensions.extend([".exe", ".cmd", ".bat"].into_iter().map(String::from));
        }
    }

    extensions
}

fn command_exists_in_paths(
    command: &str,
    paths: &[PathBuf],
    executable_extensions: &[String],
) -> bool {
    paths.iter().any(|dir| {
        executable_extensions.iter().any(|ext| {
            let candidate = if ext.is_empty() {
                dir.join(command)
            } else {
                dir.join(format!("{}{}", command, ext))
            };
            candidate.is_file()
        })
    })
}

fn install_selected_targets(
    base_dir: &Path,
    global: bool,
    targets: &[AgentTarget],
) -> Result<InstallReport, BundlebaseError> {
    let mut report = InstallReport::default();

    for target in targets {
        match target {
            AgentTarget::Claude => {
                install_claude(base_dir)?;
                report.installed.push(*target);
            }
            AgentTarget::Copilot => {
                if global {
                    report.skipped.push(
                        "copilot because workspace MCP config lives in `.vscode/mcp.json`; use `--scope local`".to_string(),
                    );
                } else {
                    install_copilot_workspace(base_dir)?;
                    report.installed.push(*target);
                }
            }
        }
    }

    if report.installed.is_empty() {
        let mut message = "setup-agent did not install any agent configuration.".to_string();
        if !report.skipped.is_empty() {
            message.push(' ');
            message.push_str(&report.skipped.join(" "));
        }
        return Err(BundlebaseError::from(message));
    }

    Ok(report)
}

fn install_claude(base_dir: &Path) -> Result<(), BundlebaseError> {
    let skill_dir = base_dir.join(".agents/skills/bundlebase");

    fs::create_dir_all(&skill_dir).map_err(|e| {
        BundlebaseError::from(format!(
            "Failed to create directory '{}': {}",
            skill_dir.display(),
            e
        ))
    })?;

    fs::write(skill_dir.join("SKILL.md"), SKILL_MD)
        .map_err(|e| BundlebaseError::from(format!("Failed to write SKILL.md: {}", e)))?;

    fs::write(skill_dir.join("reference.md"), REFERENCE_MD)
        .map_err(|e| BundlebaseError::from(format!("Failed to write reference.md: {}", e)))?;

    println!(
        "Installed bundlebase agent skills to {}/",
        skill_dir.display()
    );

    install_claude_code_mcp(base_dir)?;
    install_claude_md_nudge(base_dir)?;

    Ok(())
}

fn install_copilot_workspace(base_dir: &Path) -> Result<(), BundlebaseError> {
    let mcp_path = base_dir.join(".vscode/mcp.json");
    upsert_json_server(
        &mcp_path,
        "servers",
        bundlebase_mcp_server(),
        "GitHub Copilot",
    )?;
    install_agents_md_nudge(base_dir)
}

fn install_claude_md_nudge(base_dir: &Path) -> Result<(), BundlebaseError> {
    let claude_md_path = base_dir.join("CLAUDE.md");
    upsert_bundlebase_markdown_section(
        &claude_md_path,
        AGENTS_MD,
        "# Project Instructions\n",
        "CLAUDE.md",
    )
}

fn install_agents_md_nudge(base_dir: &Path) -> Result<(), BundlebaseError> {
    let agents_md_path = base_dir.join("AGENTS.md");
    upsert_bundlebase_markdown_section(
        &agents_md_path,
        AGENTS_MD,
        "# Agent Instructions\n",
        "AGENTS.md",
    )
}

fn install_claude_code_mcp(base_dir: &Path) -> Result<(), BundlebaseError> {
    let mcp_path = base_dir.join(".mcp.json");
    upsert_json_server(
        &mcp_path,
        "mcpServers",
        bundlebase_mcp_server(),
        "Claude Code",
    )
}

fn upsert_json_server(
    path: &Path,
    root_key: &str,
    server_config: serde_json::Value,
    product_name: &str,
) -> Result<(), BundlebaseError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to create directory '{}': {}",
                parent.display(),
                e
            ))
        })?;
    }

    let file_exists = path.exists();
    let mut config = if file_exists {
        let contents = fs::read_to_string(path).map_err(|e| {
            BundlebaseError::from(format!("Failed to read {}: {}", path.display(), e))
        })?;

        serde_json::from_str::<serde_json::Value>(&contents).map_err(|e| {
            BundlebaseError::from(format!("Failed to parse {} as JSON: {}", path.display(), e))
        })?
    } else {
        serde_json::json!({})
    };

    let config_object = config.as_object_mut().ok_or_else(|| {
        BundlebaseError::from(format!(
            "Expected {} to contain a top-level JSON object.",
            path.display()
        ))
    })?;

    let servers = config_object
        .entry(root_key.to_string())
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            BundlebaseError::from(format!(
                "Expected `{}` in {} to be a JSON object.",
                root_key,
                path.display()
            ))
        })?;

    let had_bundlebase = servers.contains_key("bundlebase");
    servers.insert("bundlebase".to_string(), server_config);

    let updated = serde_json::to_string_pretty(&config)
        .map_err(|e| BundlebaseError::from(format!("Failed to serialize config: {}", e)))?;

    fs::write(path, updated)
        .map_err(|e| BundlebaseError::from(format!("Failed to write {}: {}", path.display(), e)))?;

    match (file_exists, had_bundlebase) {
        (false, _) => println!(
            "Created {} with bundlebase {} config",
            path.display(),
            product_name
        ),
        (true, false) => println!(
            "Added bundlebase {} config to {}",
            product_name,
            path.display()
        ),
        (true, true) => println!(
            "Updated bundlebase {} config in {}",
            product_name,
            path.display()
        ),
    }

    Ok(())
}

fn bundlebase_mcp_server() -> serde_json::Value {
    serde_json::json!({
        "command": "bundlebase",
        "args": ["mcp"]
    })
}

fn format_target_list(targets: &[AgentTarget]) -> String {
    targets
        .iter()
        .map(|target| target.display_name())
        .collect::<Vec<_>>()
        .join(", ")
}

fn upsert_bundlebase_markdown_section(
    path: &Path,
    section: &str,
    initial_preamble: &str,
    label: &str,
) -> Result<(), BundlebaseError> {
    if path.exists() {
        let contents = fs::read_to_string(path)
            .map_err(|e| BundlebaseError::from(format!("Failed to read {}: {}", label, e)))?;

        let updated = if contents.contains("## Bundlebase") {
            replace_bundlebase_section(&contents, section)
        } else {
            format!("{}{}", contents.trim_end(), section)
        };

        fs::write(path, updated)
            .map_err(|e| BundlebaseError::from(format!("Failed to update {}: {}", label, e)))?;

        if contents.contains("## Bundlebase") {
            println!("Updated bundlebase section in {}", path.display());
        } else {
            println!("Added bundlebase section to {}", path.display());
        }
    } else {
        let content = format!("{}{}", initial_preamble, section);
        fs::write(path, content)
            .map_err(|e| BundlebaseError::from(format!("Failed to create {}: {}", label, e)))?;
        println!("Created {} with bundlebase section", path.display());
    }

    Ok(())
}

/// Replace the ## Bundlebase section in an instructions file with the latest content.
fn replace_bundlebase_section(contents: &str, section: &str) -> String {
    if let Some(start) = contents.find("## Bundlebase") {
        let rest = &contents[start + "## Bundlebase".len()..];
        let end = rest
            .find("\n## ")
            .map(|i| start + "## Bundlebase".len() + i)
            .unwrap_or(contents.len());

        format!("{}{}", contents[..start].trim_end(), section)
            + if end < contents.len() {
                &contents[end..]
            } else {
                ""
            }
    } else {
        format!("{}{}", contents.trim_end(), section)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolve_targets_prefers_explicit_selection() {
        let targets = resolve_targets(Some(AgentTarget::Copilot)).expect("should resolve");
        assert_eq!(targets, vec![AgentTarget::Copilot]);
    }

    #[test]
    fn command_detection_finds_supported_executables() {
        let temp = tempdir().expect("tempdir");
        fs::write(temp.path().join("claude"), "").expect("write claude");
        fs::write(temp.path().join("copilot"), "").expect("write copilot");

        let path = std::env::join_paths([temp.path()]).expect("join paths");
        assert!(command_exists_in_env(
            &["claude"],
            Some(path.as_os_str()),
            None
        ));
        assert!(command_exists_in_env(
            &["copilot"],
            Some(path.as_os_str()),
            None
        ));
    }

    #[test]
    fn install_claude_writes_expected_files() {
        let temp = tempdir().expect("tempdir");
        let report = install_selected_targets(temp.path(), false, &[AgentTarget::Claude])
            .expect("install claude");

        assert_eq!(report.installed, vec![AgentTarget::Claude]);
        assert!(temp
            .path()
            .join(".agents/skills/bundlebase/SKILL.md")
            .exists());
        assert!(temp
            .path()
            .join(".agents/skills/bundlebase/reference.md")
            .exists());
        assert!(temp.path().join(".mcp.json").exists());
        assert!(temp.path().join("CLAUDE.md").exists());
    }

    #[test]
    fn install_copilot_writes_workspace_mcp_config() {
        let temp = tempdir().expect("tempdir");
        let report = install_selected_targets(temp.path(), false, &[AgentTarget::Copilot])
            .expect("install copilot");

        assert_eq!(report.installed, vec![AgentTarget::Copilot]);

        let config = fs::read_to_string(temp.path().join(".vscode/mcp.json")).expect("read config");
        let parsed: serde_json::Value = serde_json::from_str(&config).expect("parse config");
        assert_eq!(
            parsed["servers"]["bundlebase"]["command"],
            serde_json::Value::String("bundlebase".to_string())
        );
        assert_eq!(
            parsed["servers"]["bundlebase"]["args"],
            serde_json::json!(["mcp"])
        );
        assert!(temp.path().join("AGENTS.md").exists());
    }

    #[test]
    fn install_copilot_merges_existing_servers() {
        let temp = tempdir().expect("tempdir");
        let config_dir = temp.path().join(".vscode");
        fs::create_dir_all(&config_dir).expect("create config dir");
        fs::write(
            config_dir.join("mcp.json"),
            r#"{
  "servers": {
    "github": {
      "url": "https://api.githubcopilot.com/mcp"
    }
  }
}"#,
        )
        .expect("write initial config");

        install_selected_targets(temp.path(), false, &[AgentTarget::Copilot])
            .expect("install copilot");

        let config = fs::read_to_string(config_dir.join("mcp.json")).expect("read config");
        let parsed: serde_json::Value = serde_json::from_str(&config).expect("parse config");
        assert!(parsed["servers"]["github"].is_object());
        assert_eq!(
            parsed["servers"]["bundlebase"]["command"],
            serde_json::Value::String("bundlebase".to_string())
        );
        let agents = fs::read_to_string(temp.path().join("AGENTS.md")).expect("read AGENTS");
        assert!(agents.contains("## Bundlebase"));
        assert!(agents.contains("Before doing ANYTHING with data"));
    }

    #[test]
    fn install_copilot_updates_existing_agents_md_section() {
        let temp = tempdir().expect("tempdir");
        fs::write(
            temp.path().join("AGENTS.md"),
            "# Agent Instructions\n\n## Bundlebase\n\nOld text.\n\n## Other\n\nKeep me.\n",
        )
        .expect("write AGENTS");

        install_selected_targets(temp.path(), false, &[AgentTarget::Copilot])
            .expect("install copilot");

        let agents = fs::read_to_string(temp.path().join("AGENTS.md")).expect("read AGENTS");
        assert!(agents.contains("Before doing ANYTHING with data"));
        assert!(agents.contains("## Other"));
        assert!(!agents.contains("Old text."));
    }

    #[test]
    fn install_auto_global_skips_copilot() {
        let temp = tempdir().expect("tempdir");
        let report = install_selected_targets(
            temp.path(),
            true,
            &[AgentTarget::Claude, AgentTarget::Copilot],
        )
        .expect("install selected targets");

        assert_eq!(report.installed, vec![AgentTarget::Claude]);
        assert_eq!(report.skipped.len(), 1);
        assert!(temp
            .path()
            .join(".agents/skills/bundlebase/SKILL.md")
            .exists());
        assert!(!temp.path().join(".vscode/mcp.json").exists());
    }
}
