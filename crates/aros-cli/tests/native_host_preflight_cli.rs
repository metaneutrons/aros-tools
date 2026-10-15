//! Native host prerequisite checks through the public CLI; no compiler build.

#![cfg(unix)]

use aros_common::{sha256_file, target::native_host_key};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const YAML_ARCHIVE: &str = "pyyaml-6.0.2.tar.gz";

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    tools: PathBuf,
    cache: PathBuf,
    source_lock: PathBuf,
    profiles: PathBuf,
    python_env: PathBuf,
}

impl Fixture {
    fn new(family: &str) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let tools = root.join("host-tools");
        let cache = root.join("cache");
        fs::create_dir(&tools).unwrap();
        fs::create_dir(&cache).unwrap();
        write_host_tools(&tools, family, true);
        write_python_packages(&cache);

        let source_lock = root.join("source-lock.json");
        fs::write(&source_lock, source_lock_bytes(family, &cache)).unwrap();
        let profiles = root.join("profiles.json");
        fs::write(&profiles, profiles_bytes(family)).unwrap();
        let python_env = root.join("python-env");

        Self {
            _temporary: temporary,
            root,
            tools,
            cache,
            source_lock,
            profiles,
            python_env,
        }
    }

    fn run(
        &self,
        source_lock: &Path,
        profiles: &Path,
        host: &str,
        cache: &Path,
        python_env: &Path,
    ) -> Output {
        Command::new(env!("CARGO_BIN_EXE_aros"))
            .current_dir(&self.root)
            .env_clear()
            // The CLI and its probes see only the explicit fixture tool closure.
            // python3 is a symlink to the actual interpreter, not an ambient PATH fallback.
            .env("PATH", &self.tools)
            .args(["toolchain", "producer", "host-preflight", "--source-lock"])
            .arg(source_lock)
            .arg("--profiles")
            .arg(profiles)
            .arg("--preset")
            .arg(profile_name(profiles))
            .arg("--host")
            .arg(host)
            .arg("--cache-dir")
            .arg(cache)
            .arg("--python-env-dir")
            .arg(python_env)
            .output()
            .unwrap()
    }

    fn run_default(&self) -> Output {
        self.run(
            &self.source_lock,
            &self.profiles,
            native_host_key().expect("CLI integration tests need a supported native host"),
            &self.cache,
            &self.python_env,
        )
    }
}

const fn profile_name(path: &Path) -> &'static str {
    // Both fixture families use the same preset selector.
    let _ = path;
    "host-preflight-fixture"
}

fn write_host_tools(directory: &Path, family: &str, include_llvm_ar: bool) {
    let python = which::which("python3").expect("system Python 3 is required for this fixture");
    symlink(fs::canonicalize(python).unwrap(), directory.join("python3")).unwrap();

    let mut names = vec!["git", "cmake", "cc", "c++", "rustc", "cargo", "gmake"];
    if family == "gnu" {
        names.extend(["bison", "flex", "patch", "pkg-config", "ninja"]);
        if include_llvm_ar {
            names.extend(["llvm-ar", "llvm-ranlib"]);
        }
    }

    for name in names {
        let version = match name {
            "cc" | "c++" => "Apple clang version 17.0.0",
            "gmake" => "GNU Make 4.4.1",
            _ => "fixture host tool version 1.0",
        };
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' '{version}'\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}

fn write_python_packages(cache: &Path) {
    write_package_archive(
        cache,
        "mako-1.3.10.tar.gz",
        "mako-1.3.10",
        &[
            ("mako/__init__.py", b"__version__ = '1.3.10'\n"),
            (
                "mako/template.py",
                b"class Template:\n def __init__(self, value): self.value = value\n def render(self): return self.value\n",
            ),
        ],
    );
    write_package_archive(
        cache,
        "markupsafe-3.0.2.tar.gz",
        "markupsafe-3.0.2",
        &[("src/markupsafe/__init__.py", b"__version__ = '3.0.2'\n")],
    );
    write_package_archive(
        cache,
        YAML_ARCHIVE,
        "pyyaml-6.0.2",
        &[("yaml/__init__.py", b"__version__ = '6.0.2'\n")],
    );
}

fn write_package_archive(cache: &Path, filename: &str, source_root: &str, files: &[(&str, &[u8])]) {
    let mut builder = tar::Builder::new(Vec::new());
    for (relative, contents) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(u64::try_from(contents.len()).unwrap());
        header.set_mode(0o644);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("{source_root}/{relative}"), *contents)
            .unwrap();
    }
    let tar_bytes = builder.into_inner().unwrap();

    let mut gzip = Command::new(which::which("gzip").expect("gzip is required for this fixture"))
        .args(["-n", "-c"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    gzip.stdin.take().unwrap().write_all(&tar_bytes).unwrap();
    let output = gzip.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "could not create Python fixture archive: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(cache.join(filename), output.stdout).unwrap();
}

fn package_record(
    cache: &Path,
    name: &str,
    version: &str,
    filename: &str,
    source_root: &str,
    python_path: &str,
) -> Value {
    let measured = sha256_file(&cache.join(filename)).unwrap();
    json!({
        "name": name,
        "version": version,
        "filename": filename,
        "url": format!("https://example.invalid/{filename}"),
        "sha256": measured.digest,
        "size": measured.size,
        "source_root": source_root,
        "python_path": python_path,
    })
}

fn source_lock_bytes(family: &str, cache: &Path) -> Vec<u8> {
    let packages = vec![
        package_record(
            cache,
            "mako",
            "1.3.10",
            "mako-1.3.10.tar.gz",
            "mako-1.3.10",
            ".",
        ),
        package_record(
            cache,
            "markupsafe",
            "3.0.2",
            "markupsafe-3.0.2.tar.gz",
            "markupsafe-3.0.2",
            "src",
        ),
        package_record(cache, "yaml", "6.0.2", YAML_ARCHIVE, "pyyaml-6.0.2", "."),
    ];

    let mut document = if family == "gnu" {
        serde_json::from_slice::<Value>(include_bytes!(
            "../../aros-toolchain/tests/fixtures/gnu-source-lock-v3.json"
        ))
        .unwrap()
    } else {
        json!({
            "schema": "aros-toolchain-source-lock-v2",
            "family": "llvm",
            "version": "11.0.0",
            "sources": [{
                "component": "llvm",
                "version": "11.0.0",
                "purpose": "toolchain-component",
                "filename": "llvm.tar.xz",
                "url": "https://example.invalid/llvm.tar.xz",
                "sha256": "a".repeat(64),
                "size": 1,
            }],
        })
    };
    document["host_python_packages"] = json!(packages);
    serde_json::to_vec(&document).unwrap()
}

fn profiles_bytes(family: &str) -> Vec<u8> {
    let profile = if family == "gnu" {
        json!({
            "name": "host-preflight-fixture",
            "configure_target": "rv32-fixture",
            "upstream_output_target": "rv32-fixture",
            "target_triple": "riscv-aros",
            "cpu": "riscv",
            "platform": "fixture",
            "float_abi": "ilp32f",
            "capabilities": ["c", "libgcc", "standalone-collector"],
            "target": {
                "schema": "aros-riscv-target-v1",
                "isa": "rv32imafc",
                "abi": "ilp32f",
                "code_model": "medany",
                "architecture": "rv32i2p1_m2p0_a2p1_f2p2_c2p0",
                "unaligned_access": false,
                "atomic_abi": 0,
                "x3_reg_usage": 0,
            },
        })
    } else {
        json!({
            "name": "host-preflight-fixture",
            "configure_target": "pc-x86_64",
            "upstream_output_target": "pc-x86_64",
            "target_triple": "x86_64-unknown-aros",
            "cpu": "x86_64",
            "platform": "pc",
            "float_abi": "",
            "capabilities": ["c", "cxx", "standalone-collector"],
        })
    };

    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v2",
        "family": family,
        "upstream_commit": "d".repeat(40),
        "profiles": [profile],
    }))
    .unwrap()
}

fn assert_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "host preflight failed: {}",
        diagnostic(output)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema"], "aros-native-host-preflight-v1");
    assert_eq!(
        report["assurance"],
        "host-prerequisites-only-not-configure-build-or-release-qualification"
    );
    assert!(report["host_preflight"]["host"]
        .as_str()
        .is_some_and(|host| !host.is_empty()));
    report
}

fn assert_python_packages(report: &Value) {
    let packages = report["python"]["packages"]
        .as_array()
        .expect("preflight should report its lock-owned Python packages");
    let names = packages
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["mako", "markupsafe", "yaml"]);
}

fn assert_failure(output: &Output) {
    assert!(!output.status.success(), "unexpected success: {output:?}");
}

fn assert_failure_contains(output: &Output, expected: &str) {
    assert_failure(output);
    let text = diagnostic(output);
    assert!(
        text.contains(expected),
        "expected diagnostic containing {expected:?}, got: {text}"
    );
}

fn diagnostic(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

#[test]
fn imports_locked_packages_for_llvm_and_gnu_profiles_without_compiling() {
    for family in ["llvm", "gnu"] {
        let fixture = Fixture::new(family);
        let output = fixture.run_default();
        let report = assert_success(&output);
        assert_python_packages(&report);
        if family == "gnu" {
            let tools = report["host_preflight"]["tools"]
                .as_array()
                .expect("preflight should report inspected host tools");
            for name in ["llvm-ar", "llvm-ranlib"] {
                assert!(
                    tools.iter().any(|tool| tool["name"] == name),
                    "GNU profile omitted required host tool {name}"
                );
            }
        }
        assert!(fixture.python_env.is_dir());
    }
}

#[test]
fn accepts_relative_input_documents_without_weakening_output_path_validation() {
    let fixture = Fixture::new("llvm");
    let report = assert_success(&fixture.run(
        Path::new("./source-lock.json"),
        Path::new("./profiles.json"),
        native_host_key().unwrap(),
        &fixture.cache,
        &fixture.python_env,
    ));
    assert_python_packages(&report);
    assert!(fixture.python_env.is_dir());
}

#[test]
fn missing_llvm_ar_fails_gnu_preflight_before_creating_python_environment() {
    let fixture = Fixture::new("gnu");
    fs::remove_file(fixture.tools.join("llvm-ar")).unwrap();

    assert_failure_contains(&fixture.run_default(), "llvm-ar");
    assert!(!fixture.python_env.exists());
}

#[test]
fn missing_or_corrupt_locked_yaml_archive_is_rejected() {
    for corrupt in [false, true] {
        let fixture = Fixture::new("llvm");
        let yaml = fixture.cache.join(YAML_ARCHIVE);
        if corrupt {
            fs::write(yaml, b"corrupt locked YAML archive").unwrap();
        } else {
            fs::remove_file(yaml).unwrap();
        }
        assert_failure_contains(&fixture.run_default(), "AX0301");
        assert!(!fixture.python_env.exists());
    }
}

#[test]
fn wrong_host_or_profile_family_is_rejected_before_python_environment_creation() {
    let fixture = Fixture::new("gnu");
    let wrong_host_env = fixture.root.join("wrong-host-python-env");
    assert_failure_contains(
        &fixture.run(
            &fixture.source_lock,
            &fixture.profiles,
            "wrong-host",
            &fixture.cache,
            &wrong_host_env,
        ),
        "--host does not match",
    );
    assert!(!wrong_host_env.exists());

    let llvm_lock = fixture.root.join("llvm-lock.json");
    fs::write(&llvm_lock, source_lock_bytes("llvm", &fixture.cache)).unwrap();
    let wrong_family_env = fixture.root.join("wrong-family-python-env");
    assert_failure_contains(
        &fixture.run(
            &llvm_lock,
            &fixture.profiles,
            native_host_key().unwrap(),
            &fixture.cache,
            &wrong_family_env,
        ),
        "select different compiler families",
    );
    assert!(!wrong_family_env.exists());
}

#[test]
fn rejects_symlinked_cache_and_python_environment_paths() {
    let fixture = Fixture::new("llvm");
    let cache_link = fixture.root.join("cache-link");
    symlink(&fixture.cache, &cache_link).unwrap();
    let cache_env = fixture.root.join("cache-link-python-env");
    assert_failure_contains(
        &fixture.run(
            &fixture.source_lock,
            &fixture.profiles,
            native_host_key().unwrap(),
            &cache_link,
            &cache_env,
        ),
        "--cache-dir",
    );
    assert!(!cache_env.exists());

    let env_target = fixture.root.join("env-target");
    fs::create_dir(&env_target).unwrap();
    let env_link = fixture.root.join("env-link");
    symlink(&env_target, &env_link).unwrap();
    assert_failure_contains(
        &fixture.run(
            &fixture.source_lock,
            &fixture.profiles,
            native_host_key().unwrap(),
            &fixture.cache,
            &env_link,
        ),
        "--python-env-dir",
    );
    assert_eq!(fs::read_dir(env_target).unwrap().count(), 0);
}

#[test]
fn exposes_required_arguments_and_help_without_running_preflight() {
    let fixture = Fixture::new("llvm");
    let help = Command::new(env!("CARGO_BIN_EXE_aros"))
        .current_dir(&fixture.root)
        .env_clear()
        .args(["toolchain", "producer", "host-preflight", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    for argument in [
        "--source-lock",
        "--profiles",
        "--preset",
        "--host",
        "--cache-dir",
        "--python-env-dir",
    ] {
        assert!(help.contains(argument), "help omitted {argument}: {help}");
    }

    let missing = Command::new(env!("CARGO_BIN_EXE_aros"))
        .current_dir(&fixture.root)
        .env_clear()
        .args(["toolchain", "producer", "host-preflight"])
        .output()
        .unwrap();
    assert_failure(&missing);
}
