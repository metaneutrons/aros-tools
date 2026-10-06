use super::*;

#[test]
fn vendor_metadata_selects_one_exact_recommended_host_archive() {
    let checksum = sha256_bytes(b"vendor archive");
    let metadata = serde_json::json!({"tools":[{"name":"compiler","versions":[{
        "name":"any-version","status":"recommended","macos-arm64":{
            "url":"https://vendor.invalid/releases/tool.tar.xz","sha256":checksum,"size":14
        }
    }]}]});
    require_vendor_archive(
        &metadata,
        "compiler",
        "macos-arm64",
        "tool.tar.xz",
        &checksum,
    )
    .unwrap();
    for (tool, host, archive, digest) in [
        ("other", "macos-arm64", "tool.tar.xz", checksum.clone()),
        ("compiler", "linux-amd64", "tool.tar.xz", checksum.clone()),
        ("compiler", "macos-arm64", "other.tar.xz", checksum.clone()),
        (
            "compiler",
            "macos-arm64",
            "tool.tar.xz",
            sha256_bytes(b"other"),
        ),
    ] {
        assert!(require_vendor_archive(&metadata, tool, host, archive, &digest).is_err());
    }
    let mut changed = metadata.clone();
    changed["tools"]
        .as_array_mut()
        .unwrap()
        .push(metadata["tools"][0].clone());
    assert!(require_vendor_archive(
        &changed,
        "compiler",
        "macos-arm64",
        "tool.tar.xz",
        &checksum
    )
    .is_err());
    let mut changed = metadata.clone();
    changed["tools"][0]["versions"]
        .as_array_mut()
        .unwrap()
        .push(metadata["tools"][0]["versions"][0].clone());
    assert!(require_vendor_archive(
        &changed,
        "compiler",
        "macos-arm64",
        "tool.tar.xz",
        &checksum
    )
    .is_err());
    for (key, value) in [
        ("size", serde_json::json!(0)),
        ("size", serde_json::json!(-1)),
        ("sha256", serde_json::json!("unknown")),
        ("url", serde_json::json!(null)),
    ] {
        let mut changed = metadata.clone();
        changed["tools"][0]["versions"][0]["macos-arm64"][key] = value;
        assert!(require_vendor_archive(
            &changed,
            "compiler",
            "macos-arm64",
            "tool.tar.xz",
            &checksum
        )
        .is_err());
    }
}

#[test]
fn execution_snapshot_must_match_the_lock_not_only_its_copy() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fs::write(root.join("source.c"), b"locked source").unwrap();
    let limits = TreeTraversalLimits::new(100, 1024).unwrap();
    let initial = measure_tree_content_cas_bounded(&root, limits).unwrap();
    let locked = initial.payload_digest_excluding(None);
    require_locked_idf_snapshot(&initial, &locked).unwrap();
    fs::write(root.join("source.c"), b"coherent changed source").unwrap();
    let changed = measure_tree_content_cas_bounded(&root, limits).unwrap();
    assert!(require_locked_idf_snapshot(&changed, &locked).is_err());
    fs::write(root.join("source.c"), b"locked source").unwrap();
    assert!(require_locked_idf_snapshot(&changed, &locked).is_err());
}

#[test]
fn configured_cache_values_must_be_exact_and_unambiguous() {
    let cache = "// ignored comment\nOTHER:STRING=value\nPYTHON:UNINITIALIZED=/private/python\n";
    assert_eq!(cache_value(cache, "PYTHON").unwrap(), "/private/python");
    assert!(cache_value(cache, "MISSING").is_err());
    assert!(cache_value("PYTHON:STRING=/one\nPYTHON:STRING=/two\n", "PYTHON").is_err());
    assert!(cache_value("PYTHON_WRAPPER:STRING=/other\n", "PYTHON").is_err());
}

#[test]
fn toolchain_prefix_requires_one_exact_source_selection() {
    let expected = "set(_CMAKE_TOOLCHAIN_PREFIX riscv32-esp-elf-)";
    require_toolchain_prefix(expected, "riscv32-esp-elf").unwrap();
    require_toolchain_prefix(&format!("  {expected}\n"), "riscv32-esp-elf").unwrap();
    for raw in [
        "",
        "set(_CMAKE_TOOLCHAIN_PREFIX other-)",
        &format!("{expected}\n{expected}"),
    ] {
        assert!(require_toolchain_prefix(raw, "riscv32-esp-elf").is_err());
    }
}

#[test]
fn configured_program_must_be_absolute_and_match_selected_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let selected = root.join("selected");
    let other = root.join("other");
    fs::write(&selected, b"selected").unwrap();
    fs::write(&other, b"other").unwrap();
    fs::set_permissions(&selected, fs::Permissions::from_mode(0o700)).unwrap();
    alias_program(&root, "alias", &selected).unwrap();
    require_tool_selection(&root.join("alias"), &selected).unwrap();
    assert!(require_tool_selection(Path::new("selected"), &selected).is_err());
    assert!(require_tool_selection(&other, &selected).is_err());
}

#[test]
fn non_executable_selected_tool_is_refused_before_alias_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let selected = root.join("selected");
    fs::write(&selected, b"#!/bin/sh\nexit 0\n").unwrap();
    for mode in [0o600, 0o644] {
        fs::set_permissions(&selected, fs::Permissions::from_mode(mode)).unwrap();
        assert!(alias_program(&root, "git", &selected).is_err());
        assert!(!root.join("git").exists());
    }
}

#[test]
fn selected_path_excludes_shadowing_tool_and_wheel_directories() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let bin = root.join("host-bin");
    let vendor = root.join("vendor-bin");
    let wheels = root.join("wheel-bin");
    fs::create_dir(&bin).unwrap();
    fs::create_dir(&vendor).unwrap();
    fs::create_dir(&wheels).unwrap();
    let marker = root.join("shadow-executed");
    for directory in [&vendor, &wheels] {
        for name in ["git", "riscv32-esp-elf-gcc", "python3"] {
            let path = directory.join(name);
            fs::write(
                &path,
                format!("#!/bin/sh\ntouch '{}'\nexit 99\n", marker.display()),
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    for name in ["git", "riscv32-esp-elf-gcc"] {
        alias_program(&bin, name, Path::new("/usr/bin/true")).unwrap();
    }
    // Include a quote in the selected path to exercise literal argument handling.
    let python = root.join("selected'python");
    fs::write(&python, b"#!/bin/sh\n[ \"$1\" = 'literal argument' ]\n").unwrap();
    fs::set_permissions(&python, fs::Permissions::from_mode(0o700)).unwrap();
    alias_interpreter(&bin, "python3", &python).unwrap();
    let path = controlled_path(&root).unwrap();
    assert_eq!(
        std::env::split_paths(&path).collect::<Vec<_>>(),
        [bin, PathBuf::from("/usr/bin"), PathBuf::from("/bin")]
    );
    let status = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "git && riscv32-esp-elf-gcc && python3 'literal argument'",
        ])
        .env_clear()
        .env("PATH", path)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!marker.exists());
}
