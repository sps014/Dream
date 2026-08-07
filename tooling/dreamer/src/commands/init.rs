use crate::manifest::{parse_target_list, validate_package_name, Manifest, RunTarget, MANIFEST_FILE_NAME};
use anyhow::{bail, Context, Result};
use std::path::Path;

const GITIGNORE_ENTRIES: &[&str] = &[
    "dream_packages/",
    "*.wat",
    "*.wasm",
    "*.abi.json",
    "*.runtime.js",
    "*.wgsl",
    "*.dbg.json",
];

pub fn run(dir: &Path, name: Option<String>, runtime_spec: Option<String>) -> Result<()> {
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

    let targets = match runtime_spec {
        Some(spec) => parse_target_list(&spec)?,
        None => Vec::new(),
    };

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

    let mut manifest = Manifest::new(name.clone(), "0.1.0".to_string(), entry_rel.to_string());
    manifest.package.targets = targets.iter().map(|t| t.as_str().to_string()).collect();
    manifest.save(&manifest_path)?;

    write_gitignore(dir)?;

    if targets.contains(&RunTarget::Web) {
        write_index_html(dir)?;
    }
    if targets.contains(&RunTarget::Node) {
        write_run_mjs(dir)?;
    }

    println!("Created Dream project '{}' at {}", name, dir.display());
    println!("  {}", manifest_path.display());
    println!("  {}", entry_path.display());
    if targets.contains(&RunTarget::Web) {
        println!("  {}", dir.join("index.html").display());
    }
    if targets.contains(&RunTarget::Node) {
        println!("  {}", dir.join("run.mjs").display());
    }
    println!();
    println!("Next steps:");
    println!("  dreamer add <package>   # add a dependency");
    println!("  dreamer build           # compile the entry point");
    match targets.len() {
        0 => println!("  dreamer run             # install deps and run (native)"),
        1 => println!(
            "  dreamer run             # run via {}",
            targets[0].as_str()
        ),
        _ => {
            println!("  dreamer run --target <{}>", {
                targets
                    .iter()
                    .map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join("|")
            });
        }
    }
    Ok(())
}

fn write_gitignore(dir: &Path) -> Result<()> {
    let gitignore_path = dir.join(".gitignore");
    let desired = GITIGNORE_ENTRIES.join("\n") + "\n";
    match std::fs::read_to_string(&gitignore_path) {
        Ok(existing) => {
            let mut updated = existing;
            if !updated.ends_with('\n') && !updated.is_empty() {
                updated.push('\n');
            }
            for entry in GITIGNORE_ENTRIES {
                if !updated.lines().any(|l| l.trim() == *entry) {
                    updated.push_str(entry);
                    updated.push('\n');
                }
            }
            std::fs::write(&gitignore_path, updated)?;
        }
        Err(_) => std::fs::write(&gitignore_path, desired)?,
    }
    Ok(())
}

fn write_index_html(dir: &Path) -> Result<()> {
    let path = dir.join("index.html");
    if path.exists() {
        return Ok(());
    }
    let html = r#"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Dream app</title>
  </head>
  <body>
    <h1>Dream</h1>
    <p>Build with <code>dreamer build</code>, then open this page (or <code>dreamer run</code>).</p>
    <script type="module">
      import { run } from "./src/main.web.runtime.js";
      await run("./src/main.wasm");
    </script>
  </body>
</html>
"#;
    std::fs::write(&path, html).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn write_run_mjs(dir: &Path) -> Result<()> {
    let path = dir.join("run.mjs");
    if path.exists() {
        return Ok(());
    }
    let body = r#"import { run } from "./src/main.node.runtime.js";
await run("./src/main.wasm");
"#;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
