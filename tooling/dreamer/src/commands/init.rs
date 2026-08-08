use crate::manifest::{
    import_segment, parse_target_list, validate_package_name, Manifest, PackageType, RunTarget,
    MANIFEST_FILE_NAME,
};
use anyhow::{bail, Context, Result};
use std::path::Path;

const GITIGNORE_ENTRIES: &[&str] = &["dream_packages/", "target/"];

pub fn run(
    dir: &Path,
    name: Option<String>,
    runtime_spec: Option<String>,
    as_lib: bool,
) -> Result<()> {
    let manifest_path = dir.join(MANIFEST_FILE_NAME);
    if manifest_path.exists() {
        bail!("{} already exists", manifest_path.display());
    }

    let name = name.unwrap_or_else(|| {
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                if as_lib {
                    "mylib".to_string()
                } else {
                    "myapp".to_string()
                }
            })
    });
    validate_package_name(&name)?;

    if as_lib && runtime_spec.is_some() {
        bail!("--lib cannot be combined with --runtime (libraries are not runnable hosts)");
    }

    let targets = match runtime_spec {
        Some(spec) => parse_target_list(&spec)?,
        None => Vec::new(),
    };

    std::fs::create_dir_all(dir.join("src"))?;

    let source_rel = if as_lib {
        let seg = import_segment(&name);
        format!("src/{}.dream", seg)
    } else {
        "src/main.dream".to_string()
    };
    let source_path = dir.join(&source_rel);
    if !source_path.exists() {
        let body = if as_lib {
            format!(
                "import system;\n\n// Library root for `import {};`\n\npublic fun hello(): string {{\n    return \"hello\";\n}}\n",
                import_segment(&name)
            )
        } else {
            "import system;\n\nfun main() {\n    System.println(\"Hello from Dream!\");\n}\n"
                .to_string()
        };
        std::fs::write(&source_path, body)
            .with_context(|| format!("writing {}", source_path.display()))?;
    }

    let mut manifest = if as_lib {
        Manifest::new_lib(name.clone(), "0.1.0".to_string())
    } else {
        Manifest::new(name.clone(), "0.1.0".to_string(), source_rel.clone())
    };
    manifest.package.targets = targets.iter().map(|t| t.as_str().to_string()).collect();
    debug_assert_eq!(
        manifest.package.package_type,
        if as_lib {
            PackageType::Lib
        } else {
            PackageType::Bin
        }
    );
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
    println!("  {}", source_path.display());
    if targets.contains(&RunTarget::Web) {
        println!("  {}", dir.join("index.html").display());
    }
    if targets.contains(&RunTarget::Node) {
        println!("  {}", dir.join("run.mjs").display());
    }
    println!();
    println!("Next steps:");
    if as_lib {
        println!("  dreamer build           # typecheck the library root");
        println!("  # depend on this package from another project via path/registry");
    } else {
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
      import { run } from "./target/web/main.web.runtime.js";
      await run("./target/web/main.wasm");
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
    let body = r#"import { run } from "./target/node/main.node.runtime.js";
await run("./target/node/main.wasm");
"#;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
