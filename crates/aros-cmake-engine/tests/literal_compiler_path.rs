#![cfg(unix)]

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const COMPILER_BYTES: &[u8] = b"test-only compiler payload\n";

struct Fixture {
    _temporary: TempDir,
    root: PathBuf,
    toolchain: PathBuf,
    bin: PathBuf,
    driver: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let toolchain = root.join("toolchain");
        let bin = toolchain.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let compiler = bin.join("clang-11");
        fs::write(&compiler, COMPILER_BYTES).unwrap();
        fs::set_permissions(&compiler, fs::Permissions::from_mode(0o755)).unwrap();
        symlink("clang-11", bin.join("clang")).unwrap();
        symlink("clang", bin.join("clang++")).unwrap();

        let fixture = Self {
            _temporary: temporary,
            root,
            toolchain,
            bin: bin.clone(),
            driver: bin.join("clang++"),
        };
        fixture.write_entries(&Self::default_entries());
        fixture
    }

    fn default_entries() -> Vec<Value> {
        vec![
            json!({
                "path": "bin/clang",
                "mode": "0777",
                "type": "symlink",
                "target": "clang-11"
            }),
            json!({
                "path": "bin/clang++",
                "mode": "0777",
                "type": "symlink",
                "target": "clang"
            }),
            json!({
                "path": "bin/clang-11",
                "mode": "0755",
                "type": "file",
                "sha256": aros_common::sha256_bytes(COMPILER_BYTES).as_str(),
                "size": COMPILER_BYTES.len()
            }),
        ]
    }

    fn write_entries(&self, entries: &[Value]) {
        fs::write(
            self.toolchain.join("toolchain-manifest.json"),
            serde_json::to_vec(&json!({ "files": entries })).unwrap(),
        )
        .unwrap();
    }

    fn run(&self, driver: &Path, bind_root: bool) -> Output {
        self.run_with_action(driver, bind_root, "")
    }

    fn run_with_action(&self, driver: &Path, bind_root: bool, after_first_resolve: &str) -> Output {
        let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("engine");
        let script = format!(
            "cmake_minimum_required(VERSION 3.22)\n\
             include({})\n\
             set(AROS_CROSS_TOOLCHAIN_ROOT {})\n\
             set(_driver {})\n\
             _aros_literal_resolve_compiler(_resolved _sha \"${{_driver}}\")\n\
             message(STATUS \"RESULT_PATH=${{_resolved}}\")\n\
             message(STATUS \"RESULT_SHA=${{_sha}}\")\n\
             {after_first_resolve}\n",
            cmake_bracket(
                &engine
                    .join("LiteralCompilerPath.cmake")
                    .display()
                    .to_string()
            ),
            if bind_root {
                cmake_bracket(&self.toolchain.display().to_string())
            } else {
                cmake_bracket("")
            },
            cmake_bracket(&driver.display().to_string()),
            after_first_resolve = after_first_resolve,
        );
        let script_path = self.root.join("resolve-compiler.cmake");
        fs::write(&script_path, script).unwrap();
        Command::new("cmake")
            .arg("-P")
            .arg(script_path)
            .output()
            .unwrap()
    }
}

fn cmake_bracket(value: &str) -> String {
    let mut equals = String::new();
    while value.contains(&format!("]{equals}]")) {
        equals.push('=');
    }
    format!("[{equals}[{value}]{equals}]")
}

fn assert_failed(output: &Output, expected: &str) {
    assert!(!output.status.success());
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized.contains(expected),
        "expected {expected:?}: {text}"
    );
}

#[test]
fn resolves_manifest_declared_clang_alias_chain_and_returns_final_digest() {
    let fixture = Fixture::new();
    let output = fixture.run(&fixture.driver, true);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains(&format!(
        "RESULT_PATH={}",
        fixture.bin.join("clang-11").display()
    )));
    assert!(text.contains(&format!(
        "RESULT_SHA={}",
        aros_common::sha256_bytes(COMPILER_BYTES).as_str()
    )));
}

#[test]
fn ordinary_regular_driver_keeps_strict_path_policy_and_returns_digest() {
    let fixture = Fixture::new();
    let regular = fixture.bin.join("plain-compiler");
    fs::write(&regular, COMPILER_BYTES).unwrap();
    fs::set_permissions(&regular, fs::Permissions::from_mode(0o755)).unwrap();

    let output = fixture.run(&regular, false);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains(&format!("RESULT_PATH={}", regular.display())));
    assert!(text.contains(&format!(
        "RESULT_SHA={}",
        aros_common::sha256_bytes(COMPILER_BYTES).as_str()
    )));
}

#[test]
fn unbound_compiler_alias_is_refused() {
    let fixture = Fixture::new();
    assert_failed(
        &fixture.run(&fixture.driver, false),
        "symlink compiler driver path refused",
    );
}

#[test]
fn alias_target_hash_and_size_must_match_manifest() {
    let fixture = Fixture::new();
    let mut entries = Fixture::default_entries();
    entries[1]["target"] = json!("clang-11");
    fixture.write_entries(&entries);
    assert_failed(
        &fixture.run(&fixture.driver, true),
        "differs from its toolchain manifest inventory",
    );

    let fixture = Fixture::new();
    let mut entries = Fixture::default_entries();
    entries[2]["sha256"] = json!("0".repeat(64));
    fixture.write_entries(&entries);
    assert_failed(
        &fixture.run(&fixture.driver, true),
        "final compiler bytes differ from toolchain manifest inventory",
    );

    let fixture = Fixture::new();
    let mut entries = Fixture::default_entries();
    entries[2]["size"] = json!(COMPILER_BYTES.len() + 1);
    fixture.write_entries(&entries);
    assert_failed(
        &fixture.run(&fixture.driver, true),
        "final compiler bytes differ from toolchain manifest inventory",
    );
}

#[test]
fn alias_target_may_not_escape_toolchain_bin() {
    let fixture = Fixture::new();
    fs::remove_file(&fixture.driver).unwrap();
    symlink("../bin/clang-11", &fixture.driver).unwrap();
    let mut entries = Fixture::default_entries();
    entries[1]["target"] = json!("../bin/clang-11");
    fixture.write_entries(&entries);
    assert_failed(
        &fixture.run(&fixture.driver, true),
        "alias target must be a same-bin filename",
    );
}

#[test]
fn alias_parent_symlink_is_refused() {
    let fixture = Fixture::new();
    let moved_bin = fixture.root.join("moved-bin");
    fs::rename(&fixture.bin, &moved_bin).unwrap();
    symlink(&moved_bin, &fixture.bin).unwrap();
    assert_failed(
        &fixture.run(&fixture.driver, true),
        "symlink compiler driver path refused",
    );
}

#[test]
fn duplicate_alias_or_final_file_inventory_entries_are_refused() {
    for path in ["bin/clang", "bin/clang-11"] {
        let fixture = Fixture::new();
        let mut entries = Fixture::default_entries();
        let duplicate = entries
            .iter()
            .find(|entry| entry["path"] == path)
            .unwrap()
            .clone();
        entries.push(duplicate);
        fixture.write_entries(&entries);
        assert_failed(
            &fixture.run(&fixture.driver, true),
            &format!("inventories '{path}' more than once"),
        );
    }
}

#[test]
fn manifest_digest_cache_does_not_hide_manifest_mutation_in_one_cmake_process() {
    let fixture = Fixture::new();
    let mut entries = Fixture::default_entries();
    entries[1]["target"] = json!("clang-11");
    let changed_manifest = serde_json::to_string(&json!({ "files": entries })).unwrap();
    let action = format!(
        "file(WRITE {} {})\n_aros_literal_resolve_compiler(_resolved_after _sha_after \"${{_driver}}\")",
        cmake_bracket(&fixture.toolchain.join("toolchain-manifest.json").display().to_string()),
        cmake_bracket(&changed_manifest),
    );
    assert_failed(
        &fixture.run_with_action(&fixture.driver, true, &action),
        "differs from its toolchain manifest inventory",
    );
}

#[test]
fn inventory_cache_does_not_hide_alias_mutation_in_one_cmake_process() {
    let fixture = Fixture::new();
    let action = format!(
        "file(REMOVE {})\nfile(CREATE_LINK {} {} SYMBOLIC)\n_aros_literal_resolve_compiler(_resolved_after _sha_after \"${{_driver}}\")",
        cmake_bracket(&fixture.driver.display().to_string()),
        cmake_bracket("clang-11"),
        cmake_bracket(&fixture.driver.display().to_string()),
    );
    assert_failed(
        &fixture.run_with_action(&fixture.driver, true, &action),
        "differs from its toolchain manifest inventory",
    );
}

#[test]
fn inventory_cache_does_not_hide_final_compiler_mutation_in_one_cmake_process() {
    let fixture = Fixture::new();
    let compiler = fixture.bin.join("clang-11");
    let action = format!(
        "file(WRITE {} [==[changed bytes]==])\nfile(CHMOD {} PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE)\n_aros_literal_resolve_compiler(_resolved_after _sha_after \"${{_driver}}\")",
        cmake_bracket(&compiler.display().to_string()),
        cmake_bracket(&compiler.display().to_string()),
    );
    assert_failed(
        &fixture.run_with_action(&fixture.driver, true, &action),
        "final compiler bytes differ from toolchain manifest inventory",
    );
}
