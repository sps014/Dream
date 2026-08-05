//! End-to-end coverage: `dreamer init` -> `dreamer add` -> `dreamer install`, exercised against a
//! local `file://` registry fixture (no server process required) and a local `--path` dependency,
//! then a `dreamer build` through the real `dream` compiler to prove the installed
//! `dream_packages/` layout is actually importable.

use dreamer::commands;
use dreamer::manifest::Manifest;
use dreamer::registry::{checksum, open_registry, IndexEntry};
use std::path::Path;

fn publish_fixture_package(registry_dir: &Path, name: &str, version: &str, body_fun_src: &str) {
    let pkg_dir = registry_dir
        .join("staging")
        .join(format!("{}-{}", name, version));
    std::fs::create_dir_all(pkg_dir.join("src")).unwrap();
    Manifest::new(
        name.to_string(),
        version.to_string(),
        format!("src/{}.dream", name),
    )
    .save(&pkg_dir.join("dream.toml"))
    .unwrap();
    std::fs::write(
        pkg_dir.join("src").join(format!("{}.dream", name)),
        body_fun_src,
    )
    .unwrap();

    let tarball_path = registry_dir
        .join("staging")
        .join(format!("{}-{}.tar.gz", name, version));
    let bytes = dreamer::fetch::package_project(&pkg_dir, &tarball_path).unwrap();

    let registry = open_registry(&format!("file://{}", registry_dir.display()));
    let entry = IndexEntry {
        name: name.to_string(),
        vers: version.to_string(),
        deps: Vec::new(),
        cksum: checksum::sha256_of(&bytes),
        tarball: format!("dl/{}/{}-{}.tar.gz", name, name, version),
        description: None,
    };
    registry.publish(&entry, &tarball_path).unwrap();
}

#[test]
fn init_add_install_materializes_registry_and_path_dependencies() {
    let tmp = tempfile::tempdir().unwrap();
    let registry_dir = tmp.path().join("registry");
    std::fs::create_dir_all(&registry_dir).unwrap();
    publish_fixture_package(
        &registry_dir,
        "greeter",
        "1.0.0",
        "public fun hello(): string {\n    return \"hello from the registry\";\n}\n",
    );

    let local_lib_dir = tmp.path().join("local-lib");
    std::fs::create_dir_all(local_lib_dir.join("src")).unwrap();
    Manifest::new(
        "local-lib".to_string(),
        "0.1.0".to_string(),
        "src/local-lib.dream".to_string(),
    )
    .save(&local_lib_dir.join("dream.toml"))
    .unwrap();
    std::fs::write(
        local_lib_dir.join("src").join("local-lib.dream"),
        "public fun answer(): int {\n    return 42;\n}\n",
    )
    .unwrap();

    let project_dir = tmp.path().join("myapp");
    commands::init::run(&project_dir, Some("myapp".to_string())).unwrap();

    {
        let mut workspace = dreamer::workspace::Workspace::discover(&project_dir).unwrap();
        workspace.manifest.registries.insert(
            "default".to_string(),
            format!("file://{}", registry_dir.display()),
        );
        workspace.save_manifest().unwrap();
    }

    commands::add::run(
        &project_dir,
        "greeter".to_string(),
        Some("^1.0".to_string()),
        None,
        None,
        None,
        None,
        None,
        false,
    )
    .unwrap();

    commands::add::run(
        &project_dir,
        "local-lib".to_string(),
        None,
        Some(local_lib_dir.to_str().unwrap().to_string()),
        None,
        None,
        None,
        None,
        false,
    )
    .unwrap();

    let lock = dreamer::lockfile::Lockfile::load(&project_dir.join("dream.lock")).unwrap();
    assert!(lock.find("greeter").is_some());
    assert!(lock.find("local-lib").is_some());
    assert_eq!(lock.find("greeter").unwrap().version, "1.0.0");

    let greeter_file = project_dir
        .join("dream_packages")
        .join("greeter")
        .join("src")
        .join("greeter.dream");
    assert!(
        greeter_file.is_file(),
        "{} should exist",
        greeter_file.display()
    );

    let local_lib_file = project_dir
        .join("dream_packages")
        .join("local_lib")
        .join("src")
        .join("local-lib.dream");
    assert!(
        local_lib_file.is_file(),
        "{} should exist",
        local_lib_file.display()
    );

    // Re-running install without changes must not change the locked versions (respects the lock).
    commands::install::run(&project_dir).unwrap();
    let lock_again = dreamer::lockfile::Lockfile::load(&project_dir.join("dream.lock")).unwrap();
    assert_eq!(lock_again.find("greeter").unwrap().version, "1.0.0");
}

/// Only runs the real compiler when a `dream` binary is discoverable (it is, for anyone running
/// `cargo test --workspace` from a checkout where `dream` has already been built at least once).
#[test]
fn build_compiles_a_project_using_an_installed_dependency() {
    if dreamer::dream_bin::locate().is_err() {
        eprintln!("skipping: no `dream` compiler binary found on PATH or in target/");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let registry_dir = tmp.path().join("registry");
    std::fs::create_dir_all(&registry_dir).unwrap();
    // A distinct package name from the other test in this file: the download cache under
    // `~/.dream/registry/src/<name>-<version>` is keyed only by name+version (mirroring the
    // crates.io assumption that a name+version pair is globally unique content), so two tests
    // publishing the *same* name+version to *different* registries in parallel would otherwise
    // race on that shared cache entry.
    publish_fixture_package(
        &registry_dir,
        "greeter2",
        "1.0.0",
        "public fun hello(): string {\n    return \"hello from the registry\";\n}\n",
    );

    let project_dir = tmp.path().join("myapp");
    commands::init::run(&project_dir, Some("myapp".to_string())).unwrap();
    {
        let mut workspace = dreamer::workspace::Workspace::discover(&project_dir).unwrap();
        workspace.manifest.registries.insert(
            "default".to_string(),
            format!("file://{}", registry_dir.display()),
        );
        workspace.save_manifest().unwrap();
    }
    commands::add::run(
        &project_dir,
        "greeter2".to_string(),
        Some("^1.0".to_string()),
        None,
        None,
        None,
        None,
        None,
        false,
    )
    .unwrap();

    std::fs::write(
        project_dir.join("src").join("main.dream"),
        "import greeter2;\nimport system;\n\nfun main(): void {\n    System.println(hello());\n}\n",
    )
    .unwrap();

    commands::build::run(&project_dir, false).unwrap();
    assert!(project_dir.join("src").join("main.wat").is_file());
}
