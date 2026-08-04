use crate::lockfile::Lockfile;
use crate::workspace::Workspace;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub fn run(start_dir: &Path) -> Result<()> {
    let workspace = Workspace::discover(start_dir)?;
    let lockfile = Lockfile::load_if_exists(&workspace.lockfile_path())?.context(
        "no dream.lock found; run `dreamer install` first",
    )?;

    let by_name: HashMap<&str, &crate::lockfile::LockedPackage> = lockfile
        .packages
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();

    println!("{} {}", workspace.manifest.package.name, workspace.manifest.package.version);

    let all_deps = workspace.manifest.all_dependencies(true);
    let mut top_level: Vec<&str> = all_deps.keys().map(String::as_str).collect();
    top_level.sort_unstable();

    let mut seen = HashSet::new();
    for (i, name) in top_level.iter().enumerate() {
        let is_last = i == top_level.len() - 1;
        print_node(name, &by_name, "", is_last, &mut seen);
    }
    Ok(())
}

fn print_node<'a>(
    name: &str,
    by_name: &HashMap<&'a str, &'a crate::lockfile::LockedPackage>,
    prefix: &str,
    is_last: bool,
    seen: &mut HashSet<String>,
) {
    let branch = if is_last { "└── " } else { "├── " };
    let Some(pkg) = by_name.get(name) else {
        println!("{}{}{} (unresolved)", prefix, branch, name);
        return;
    };
    let already_shown = !seen.insert(format!("{} {}", pkg.name, pkg.version));
    println!(
        "{}{}{} {}{}",
        prefix,
        branch,
        pkg.name,
        pkg.version,
        if already_shown { " (*)" } else { "" }
    );
    if already_shown {
        return;
    }

    let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
    let mut dep_names: Vec<&str> = pkg
        .dependencies
        .iter()
        .map(|d| d.split(' ').next().unwrap_or(d.as_str()))
        .collect();
    dep_names.sort_unstable();
    for (i, dep_name) in dep_names.iter().enumerate() {
        let is_last_child = i == dep_names.len() - 1;
        print_node(dep_name, by_name, &child_prefix, is_last_child, seen);
    }
}
