use bundlebase::BundlebaseError;
use std::fs;
use std::path::Path;

const SKILL_MD: &str = include_str!("../../../skills/bundlebase/SKILL.md");
const REFERENCE_MD: &str = include_str!("../../../skills/bundlebase/reference.md");

pub fn install() -> Result<(), BundlebaseError> {
    let skill_dir = Path::new(".agents/skills/bundlebase");

    if skill_dir.join("SKILL.md").exists() {
        println!(
            "Bundlebase agent skills already installed at {}/",
            skill_dir.display()
        );
        return Ok(());
    }

    fs::create_dir_all(skill_dir).map_err(|e| {
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
