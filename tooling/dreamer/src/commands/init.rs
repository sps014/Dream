use crate::manifest::{validate_package_name, Manifest, MANIFEST_FILE_NAME};
use anyhow::{bail, Context, Result};
use std::path::Path;

pub fn run(dir: &Path, name: Option<String>) -> Result<()> {
    let manifest_path = dir.join(MANIFEST_FILE_NAME);
    if manifest_path.exists() {
        bail!("{} already exists", manifest_path.display());
    }

    let name = name.unwrap_or_else(|| {
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "myapp".to_string())
    });
    validate_package_name(&name)?;

    std::fs::create_dir_all(dir.join("src"))?;

    let entry_rel = "src/main.dream";
    let entry_path = dir.join(entry_rel);
    if !entry_path.exists() {
        std::fs::write(
            &entry_path,
            "fun main() {\n    print(\"Hello from Dream!\");\n}\n",
        )
        .with_context(|| format!("writing {}", entry_path.display()))?;
    }

    let manifest = Manifest::new(name.clone(), "0.1.0".to_string(), entry_rel.to_string());
    manifest.save(&manifest_path)?;

    let gitignore_path = dir.join(".gitignore");
    let gitignore_entry = "dream_packages/\n";
    match std::fs::read_to_string(&gitignore_path) {
        Ok(existing) if existing.contains("dream_packages/") => {}
        Ok(existing) => {
            let mut updated = existing;
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str(gitignore_entry);
            std::fs::write(&gitignore_path, updated)?;
        }
        Err(_) => std::fs::write(&gitignore_path, gitignore_entry)?,
    }

    println!("Created Dream project '{}' at {}", name, dir.display());
    println!("  {}", manifest_path.display());
    println!("  {}", entry_path.display());
    println!();
    println!("Next steps:");
    println!("  dreamer add <package>   # add a dependency");
    println!("  dreamer run             # install dependencies and run src/main.dream");
    Ok(())
}
