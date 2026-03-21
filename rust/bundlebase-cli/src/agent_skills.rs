use bundlebase::BundlebaseError;
use std::fs;
use std::path::{Path, PathBuf};

const SKILL_MD: &str = include_str!("../../../skills/bundlebase/SKILL.md");
const REFERENCE_MD: &str = include_str!("../../../skills/bundlebase/reference.md");

pub fn install(global: bool) -> Result<(), BundlebaseError> {
    let skill_dir: PathBuf = if global {
        let home = dirs::home_dir().ok_or_else(|| {
            BundlebaseError::from("Could not determine home directory".to_string())
        })?;
        home.join(".agents/skills/bundlebase")
    } else {
        Path::new(".agents/skills/bundlebase").to_path_buf()
    };

    if skill_dir.join("SKILL.md").exists() {
        println!(
            "Bundlebase agent skills already installed at {}/",
            skill_dir.display()
        );
        return Ok(());
    }

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
    println!("Your coding agent can now use bundlebase automatically.");
    Ok(())
}
