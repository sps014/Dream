use crate::registry::open_registry;
use crate::workspace::Workspace;
use anyhow::Result;
use std::path::Path;

pub fn run(start_dir: &Path, query: &str) -> Result<()> {
    let url = match Workspace::discover(start_dir) {
        Ok(workspace) => workspace
            .manifest
            .registry_url(None)
            .unwrap_or_else(|| crate::registry::DEFAULT_REGISTRY.to_string()),
        Err(_) => crate::registry::DEFAULT_REGISTRY.to_string(),
    };

    let client = open_registry(&url);
    let results = client.search(query)?;
    if results.is_empty() {
        println!("No packages found matching '{}' in {}", query, url);
        return Ok(());
    }
    for entry in results {
        match entry.description.as_deref() {
            Some(desc) if !desc.is_empty() => {
                println!("{} = \"{}\"    # {}", entry.name, entry.vers, desc);
            }
            _ => println!("{} = \"{}\"", entry.name, entry.vers),
        }
        let mut meta = Vec::new();
        if let Some(ty) = entry.package_type.as_deref() {
            meta.push(format!("type={}", ty));
        }
        if let Some(license) = entry.license.as_deref() {
            meta.push(format!("license={}", license));
        }
        if !entry.authors.is_empty() {
            meta.push(format!("authors={}", entry.authors.join(", ")));
        }
        if !entry.keywords.is_empty() {
            meta.push(format!("keywords={}", entry.keywords.join(", ")));
        }
        if !meta.is_empty() {
            println!("    {}", meta.join(" · "));
        }
    }
    Ok(())
}
