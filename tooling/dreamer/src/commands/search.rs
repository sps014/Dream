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
        match entry.description {
            Some(desc) => println!("{} = \"{}\"  # {}", entry.name, entry.vers, desc),
            None => println!("{} = \"{}\"", entry.name, entry.vers),
        }
    }
    Ok(())
}
