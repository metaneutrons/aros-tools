//! Process-boundary GNU layout probes with synthetic executable fixtures.
//! These are installation/identity tests, not compiler or runtime qualification.

#![cfg(unix)]

use aros_common::{
    sha256_bytes, sha256_file, target::native_host_key, toolchain_tree_inventory,
    ArosCompilerIdentity, ArosToolchainArtifact, ArosToolchainLock, ArosToolchainManifest,
    AROS_TOOLCHAIN_MANIFEST_FILE,
};
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    temporary: tempfile::TempDir,
    project: PathBuf,
    payload: PathBuf,
    cache: PathBuf,
    store: PathBuf,
    home: PathBuf,
    marker: PathBuf,
    triple: String,
    layout: Value,
}

impl Fixture {
    fn new(width: u8) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        let payload = temporary.path().join("compiler");
        for directory in ["arch", "compiler", "rom"] {
            fs::create_dir_all(project.join(directory)).unwrap();
        }
        for file in ["configure", "Makefile.in"] {
            fs::write(project.join(file), b"").unwrap();
        }
        let (cpu, arch, abi, isa, attributes) = if width == 32 {
            (
                "riscv",
                "riscv32",
                "ilp32f",
                "rv32imafc",
                "rv32i2p1_m2p0_a2p1_f2p2_c2p0",
            )
        } else {
            (
                "riscv64",
                "riscv64",
                "lp64d",
                "rva22u64",
                "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
            )
        };
        let triple = format!("{cpu}-aros");
        fs::write(project.join("aros-targets.toml"), format!(
            "[[targets]]\nname='fixture-target'\narch='{arch}'\nplatform='fixture'\nbsp='fixture'\nfloat_abi='{abi}'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='gnu'\ncpu32=''\nuse_mmu=false\n"
        )).unwrap();
        let compiler = json!({
            "family": "gnu", "gcc_version": "16.2.0", "binutils_version": "2.47",
            "target": {"schema":"aros-riscv-target-v1", "isa":isa, "abi":abi,
                "code_model":"medany", "architecture":attributes, "unaligned_access":false,
                "atomic_abi":0, "x3_reg_usage":0}
        });
        let tools = json!({
            "c":format!("{triple}-gcc"), "cxx":format!("{triple}-g++"),
            "assembler":format!("{triple}-as"), "linker":format!("{triple}-ld"),
            "archive":format!("{triple}-ar"), "ranlib":format!("{triple}-ranlib"),
            "strip":format!("{triple}-strip"), "collector":format!("{triple}/bin/collect-aros")
        });
        let layout = json!({"schema":"aros-toolchain-tools-v1", "compiler":compiler,
            "target_triple":triple, "tools":tools});
        let marker = temporary.path().join("executed");
        let script = format!(
            "#!/bin/sh\nprintf x >> '{}'\ncase \"$1\" in\n--version) printf '%s\\n' 'synthetic GNU fixture';;\n-dumpmachine) printf '%s\\n' '{triple}';;\n-dumpfullversion) printf '%s\\n' '16.2.0';;\n*) exit 99;;\nesac\n",
            marker.display()
        );
        for path in tools.as_object().unwrap().values() {
            let path = payload.join(path.as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &script).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let fixture = Self {
            project,
            payload,
            cache: temporary.path().join("cache"),
            store: temporary.path().join("store"),
            home: temporary.path().join("home"),
            marker,
            triple,
            layout,
            temporary,
        };
        fixture.write_layout();
        fixture.write_manifest();
        fixture
    }

    fn write_layout(&self) {
        fs::write(
            self.payload.join("toolchain-tools.json"),
            serde_json::to_vec(&self.layout).unwrap(),
        )
        .unwrap();
    }

    fn add_native_utilities(&mut self) {
        let script = fs::read(self.payload.join(format!("{}-gcc", self.triple))).unwrap();
        self.layout["schema"] = json!("aros-toolchain-tools-v2");
        for role in ["nm", "objcopy"] {
            let relative = format!("{}-{role}", self.triple);
            self.layout["tools"][role] = json!(relative);
            let executable = self.payload.join(&relative);
            fs::write(&executable, &script).unwrap();
            fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        self.write_layout();
        self.write_manifest();
    }

    fn write_manifest(&self) -> ArosToolchainManifest {
        let (tree_sha256, files) = toolchain_tree_inventory(&self.payload).unwrap();
        let compiler: ArosCompilerIdentity =
            serde_json::from_value(self.layout["compiler"].clone()).unwrap();
        let manifest = ArosToolchainManifest {
            schema: 2,
            release_id: "synthetic-v1".into(),
            host: native_host_key().unwrap().into(),
            target_profile: "fixture-target".into(),
            target_triple: self.triple.clone(),
            tree_sha256,
            llvm_version: None,
            compiler: Some(compiler),
            recipe_sha256: "a".repeat(64),
            source_lock_sha256: "b".repeat(64),
            profiles_sha256: "c".repeat(64),
            source_commit: "1".repeat(40),
            producer_commit: "2".repeat(40),
            tools_commit: "3".repeat(40),
            source_date_epoch: 0,
            capabilities: vec!["c".into(), "cxx".into(), "standalone-collector".into()],
            build_environment: serde_json::Map::new(),
            files,
        };
        manifest.validate().unwrap();
        fs::write(
            self.payload.join(AROS_TOOLCHAIN_MANIFEST_FILE),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        manifest
    }

    fn base_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
        command
            .current_dir(&self.project)
            .env("AROS_HOME", &self.home)
            .env("AROS_CACHE_DIR", &self.cache)
            .env("AROS_CROSS_TOOLCHAINS_DIR", &self.store)
            .env_remove("AROS_LOG_FILE")
            .env_remove("AROS_LOG_LEVEL")
            .env_remove("AROS_LOG_FORMAT")
            .args(["--diagnostic-format", "json", "toolchain"]);
        command
    }

    fn command(&self, verb: &str) -> Command {
        let mut command = self.base_command();
        command.args([verb, "--preset", "fixture-target"]);
        command
    }

    fn local(&self, verb: &str) -> Output {
        self.command(verb)
            .arg("--local")
            .arg(&self.payload)
            .output()
            .unwrap()
    }

    fn seed_locked_archive(&self) -> ArosToolchainLock {
        let manifest = self.write_manifest();
        let archive = self.temporary.path().join("archive.tar.xz");
        let output = fs::File::create(&archive).unwrap();
        let mut builder = tar::Builder::new(xz2::write::XzEncoder::new(output, 1));
        builder.follow_symlinks(false);
        builder.append_dir_all("toolchain", &self.payload).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        let measured = sha256_file(&archive).unwrap();
        let digest = measured.digest.to_string();
        let cache = self.cache.join("downloads/sha256");
        fs::create_dir_all(&cache).unwrap();
        fs::copy(&archive, cache.join(format!("{digest}.tar.xz"))).unwrap();
        let mut required = vec!["toolchain-tools.json".to_owned()];
        required.extend(
            self.layout["tools"]
                .as_object()
                .unwrap()
                .values()
                .map(|value| value.as_str().unwrap().to_owned()),
        );
        let artifact = ArosToolchainArtifact {
            host: manifest.host.clone(),
            target_profile: manifest.target_profile.clone(),
            target_triple: manifest.target_triple.clone(),
            asset: "synthetic.tar.xz".into(),
            sha256: digest,
            tree_sha256: manifest.tree_sha256,
            llvm_version: None,
            compiler: manifest.compiler,
            size: Some(measured.size),
            enabled: true,
            disabled_reason: None,
            strip_components: 1,
            required_paths: required,
        };
        let lock = ArosToolchainLock {
            schema: 2,
            release_id: manifest.release_id,
            base_url: Some("https://example.invalid/synthetic-v1".into()),
            artifacts: vec![artifact],
        };
        lock.validate().unwrap();
        fs::write(
            self.project.join("aros-toolchains.lock.toml"),
            toml::to_string(&lock).unwrap(),
        )
        .unwrap();
        lock
    }
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn failure(output: &Output, text: &str) {
    assert!(!output.status.success());
    let diagnostics: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostics["schema"], "aros-tool-diagnostics-v1");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(text),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn local_rv32_and_rv64_resolve_declared_flat_tools_without_llvm_filenames() {
    for width in [32, 64] {
        let fixture = Fixture::new(width);
        success(&fixture.local("verify"));
        assert_eq!(fs::read(&fixture.marker).unwrap().len(), 12);
        assert!(!fixture.payload.join("bin/clang").exists());
        assert!(!fixture.store.exists());
        let output = fixture.local("path");
        success(&output);
        assert!(String::from_utf8_lossy(&output.stdout).contains(fixture.payload.to_str().unwrap()));
    }
}

#[test]
fn native_utility_layout_installs_and_verifies_all_ten_declared_tools() {
    for width in [32, 64] {
        let mut fixture = Fixture::new(width);
        fixture.add_native_utilities();
        success(&fixture.local("verify"));
        assert_eq!(fs::read(&fixture.marker).unwrap().len(), 14);
        fs::remove_file(&fixture.marker).unwrap();
        fixture.seed_locked_archive();
        success(
            &fixture
                .command("install")
                .arg("--offline")
                .output()
                .unwrap(),
        );
        // Installation probes both the staged payload before publication and
        // the completed content-addressed payload after publication.
        assert_eq!(fs::read(&fixture.marker).unwrap().len(), 28);
        fs::remove_file(&fixture.marker).unwrap();
        success(
            &fixture
                .base_command()
                .args(["list", "--format", "json"])
                .output()
                .unwrap(),
        );
        assert!(!fixture.marker.exists());
        success(&fixture.command("verify").output().unwrap());
        assert_eq!(fs::read(&fixture.marker).unwrap().len(), 14);
    }
}

#[test]
fn locked_native_utility_layout_cannot_omit_a_declared_utility() {
    for role in ["nm", "objcopy"] {
        let mut fixture = Fixture::new(64);
        fixture.add_native_utilities();
        let mut lock = fixture.seed_locked_archive();
        let declared = fixture.layout["tools"][role].as_str().unwrap();
        lock.artifacts[0]
            .required_paths
            .retain(|path| path != declared);
        fs::write(
            fixture.project.join("aros-toolchains.lock.toml"),
            toml::to_string(&lock).unwrap(),
        )
        .unwrap();
        failure(
            &fixture
                .command("install")
                .arg("--offline")
                .output()
                .unwrap(),
            "GNU lock omits",
        );
        assert!(!fixture.marker.exists());
        let artifact = &lock.artifacts[0];
        let envelope = fixture
            .store
            .join(&lock.release_id)
            .join(&artifact.host)
            .join(&artifact.target_profile)
            .join(&artifact.sha256);
        assert!(!envelope.exists());
    }
}

#[test]
fn internal_driver_alias_preserves_its_declared_invocation_name() {
    let fixture = Fixture::new(32);
    let declared = fixture.payload.join(format!("{}-gcc", fixture.triple));
    let original = fs::read_to_string(&declared).unwrap();
    let guarded = original.replacen(
        "#!/bin/sh\n",
        &format!(
            "#!/bin/sh\ncase \"$0\" in */{}-gcc) ;; *) exit 98;; esac\n",
            fixture.triple
        ),
        1,
    );
    fs::create_dir(fixture.payload.join("libexec")).unwrap();
    let driver = fixture.payload.join("libexec/driver");
    fs::write(&driver, guarded).unwrap();
    fs::set_permissions(&driver, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        Command::new(&driver)
            .arg("-dumpmachine")
            .output()
            .unwrap()
            .status
            .code(),
        Some(98)
    );
    fs::remove_file(&declared).unwrap();
    symlink("libexec/driver", &declared).unwrap();
    fixture.write_manifest();
    success(&fixture.local("verify"));
    fixture.seed_locked_archive();
    success(
        &fixture
            .command("install")
            .arg("--offline")
            .output()
            .unwrap(),
    );
}

#[test]
fn offline_gnu_install_reuses_a_complete_bound_payload() {
    let fixture = Fixture::new(64);
    let lock = fixture.seed_locked_archive();
    success(
        &fixture
            .command("install")
            .arg("--offline")
            .output()
            .unwrap(),
    );
    let artifact = &lock.artifacts[0];
    let envelope = fixture
        .store
        .join(&lock.release_id)
        .join(&artifact.host)
        .join(&artifact.target_profile)
        .join(&artifact.sha256);
    assert_eq!(fs::read(envelope.join(".complete")).unwrap(), b"complete\n");
    assert!(envelope.join("toolchain/toolchain-tools.json").is_file());
    success(&fixture.command("verify").output().unwrap());
    let original = fs::read(envelope.join("toolchain/toolchain-manifest.json")).unwrap();
    success(
        &fixture
            .command("install")
            .arg("--offline")
            .output()
            .unwrap(),
    );
    assert_eq!(
        fs::read(envelope.join("toolchain/toolchain-manifest.json")).unwrap(),
        original
    );
}

#[test]
fn list_checks_payload_integrity_without_executing_tools() {
    let fixture = Fixture::new(64);
    fixture.seed_locked_archive();
    success(
        &fixture
            .command("install")
            .arg("--offline")
            .output()
            .unwrap(),
    );
    assert!(fixture.marker.exists());
    fs::remove_file(&fixture.marker).unwrap();
    let output = fixture
        .base_command()
        .args(["list", "--format", "json"])
        .output()
        .unwrap();
    success(&output);
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["artifacts"][0]["status"], "installed");
    assert_eq!(document["artifacts"][0]["verification"], "verified");
    assert!(!fixture.marker.exists());
    success(&fixture.command("verify").output().unwrap());
    assert_eq!(fs::read(&fixture.marker).unwrap().len(), 12);
}

#[test]
fn producer_cli_packages_and_verifies_both_gnu_widths_without_executing_tools() {
    const LOCK: &[u8] =
        include_bytes!("../../aros-toolchain/tests/fixtures/gnu-source-lock-v3.json");
    for width in [32, 64] {
        let fixture = Fixture::new(width);
        let inputs = fixture.temporary.path().join("inputs");
        fs::create_dir(&inputs).unwrap();
        let target = &fixture.layout["compiler"]["target"];
        let profiles = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v2", "family": "gnu",
            "upstream_commit": "d".repeat(40),
            "profiles": [{
                "name": "fixture-target", "configure_target": "fixture-target",
                "upstream_output_target": "fixture-target", "target_triple": fixture.triple,
                "cpu": if width == 32 { "riscv" } else { "riscv64" },
                "platform": "fixture", "float_abi": target["abi"],
                "capabilities": ["c", "libgcc", "standalone-collector"], "target": target
            }]
        }))
        .unwrap();
        let mut recipe = json!({
            "schema": "aros-toolchain-recipe-v2",
            "source_commit": "1".repeat(40), "source_tree": "2".repeat(40),
            "producer_commit": "3".repeat(40), "producer_tree": "4".repeat(40),
            "tools_commit": "5".repeat(40), "tools_tree": "6".repeat(40),
            "source_date_epoch": 0, "source_lock_sha256": sha256_bytes(LOCK),
            "profiles_sha256": sha256_bytes(&profiles), "patches": []
        });
        recipe["recipe_sha256"] = json!(sha256_bytes(
            &aros_toolchain::canonical::bytes(&recipe).unwrap()
        ));
        fs::write(
            inputs.join("recipe.json"),
            serde_json::to_vec(&recipe).unwrap(),
        )
        .unwrap();
        fs::write(inputs.join("lock.json"), LOCK).unwrap();
        fs::write(inputs.join("profiles.json"), &profiles).unwrap();
        fs::write(
            inputs.join("environment.json"),
            serde_json::to_vec(&json!({
                "schema": "aros-toolchain-build-environment-v1", "host": native_host_key().unwrap()
            }))
            .unwrap(),
        )
        .unwrap();
        let producer = |verb: &str| {
            let mut command = fixture.base_command();
            command
                .args([
                    "producer",
                    verb,
                    "--preset",
                    "fixture-target",
                    "--release-id",
                    "synthetic-v1",
                    "--host",
                    native_host_key().unwrap(),
                    "--format",
                    "json",
                ])
                .arg("--recipe")
                .arg(inputs.join("recipe.json"))
                .arg("--source-lock")
                .arg(inputs.join("lock.json"))
                .arg("--profiles")
                .arg(inputs.join("profiles.json"))
                .arg("--build-environment")
                .arg(inputs.join("environment.json"));
            command
        };
        let package = fixture.temporary.path().join("package");
        let output = producer("package")
            .arg("--input-dir")
            .arg(&fixture.payload)
            .arg("--output-dir")
            .arg(&package)
            .output()
            .unwrap();
        success(&output);
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(document["archive"]
            .as_str()
            .unwrap()
            .contains("aros-toolchain-v2-gcc16.2.0-binutils2.47-"));
        let output = producer("verify-package")
            .arg("--input-dir")
            .arg(&package)
            .output()
            .unwrap();
        success(&output);
        let verified: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(verified["manifest"]["compiler"]["target"], *target);
        assert!(!fixture.marker.exists());

        // Raw-byte substitution must stop before any output directory exists.
        let mut changed = profiles;
        changed.push(b'\n');
        fs::write(inputs.join("profiles.json"), changed).unwrap();
        let absent = fixture.temporary.path().join("absent-parent/package");
        let output = producer("package")
            .arg("--input-dir")
            .arg(&fixture.payload)
            .arg("--output-dir")
            .arg(&absent)
            .output()
            .unwrap();
        failure(&output, "profiles differ from the selected recipe digest");
        assert!(!absent.parent().unwrap().exists());
        assert!(!fixture.marker.exists());
    }
}

#[test]
fn gnu_identity_and_layout_fail_before_any_tool_execution() {
    let mut fixture = Fixture::new(32);
    fixture.layout["compiler"]["gcc_version"] = json!("16.3.0");
    fixture.write_layout();
    // Keep the manifest's original compiler identity but bind the modified
    // layout bytes into its tree, so this is not merely a tree-hash failure.
    let mut manifest = ArosToolchainManifest::load(&fixture.payload).unwrap();
    let (tree, files) = toolchain_tree_inventory(&fixture.payload).unwrap();
    manifest.tree_sha256 = tree;
    manifest.files = files;
    fs::write(
        fixture.payload.join(AROS_TOOLCHAIN_MANIFEST_FILE),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    failure(&fixture.local("verify"), "compiler");
    assert!(!fixture.marker.exists());
    assert!(!fixture.store.exists());
}

#[test]
fn wrong_profile_abi_and_family_reject_selection_without_execution() {
    for (from, to) in [
        ("float_abi='ilp32f'", "float_abi='ilp32d'"),
        ("toolchain='gnu'", "toolchain='llvm'"),
    ] {
        let fixture = Fixture::new(32);
        let config = fixture.project.join("aros-targets.toml");
        fs::write(
            &config,
            fs::read_to_string(&config).unwrap().replace(from, to),
        )
        .unwrap();
        failure(
            &fixture.local("verify"),
            if from.starts_with("float") {
                "ABI"
            } else {
                "expected"
            },
        );
        assert!(!fixture.marker.exists());
    }
}

#[test]
fn missing_roles_and_external_symlinks_do_not_publish_or_execute() {
    for external in [false, true] {
        let fixture = Fixture::new(64);
        let role = fixture
            .payload
            .join(fixture.layout["tools"]["linker"].as_str().unwrap());
        fs::remove_file(&role).unwrap();
        if external {
            let outside = fixture.temporary.path().join("outside-linker");
            fs::write(&outside, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
            symlink(&outside, &role).unwrap();
        }
        if !external {
            fixture.write_manifest();
        }
        failure(
            &fixture.local("verify"),
            if external {
                "tree SHA256 mismatch"
            } else {
                "omits"
            },
        );
        assert!(!fixture.marker.exists());
        assert!(!fixture.store.exists());
    }
}

#[test]
fn locked_gnu_install_requires_every_declared_role_before_publication() {
    let fixture = Fixture::new(64);
    let mut lock = fixture.seed_locked_archive();
    let original_manifest = fs::read(fixture.payload.join(AROS_TOOLCHAIN_MANIFEST_FILE)).unwrap();
    let collector = fixture.layout["tools"]["collector"].as_str().unwrap();
    lock.artifacts[0]
        .required_paths
        .retain(|path| path != collector);
    fs::write(
        fixture.project.join("aros-toolchains.lock.toml"),
        toml::to_string(&lock).unwrap(),
    )
    .unwrap();
    failure(
        &fixture
            .command("install")
            .arg("--offline")
            .output()
            .unwrap(),
        "GNU lock omits",
    );
    assert!(!fixture.marker.exists());
    let artifact = &lock.artifacts[0];
    let envelope = fixture
        .store
        .join(&lock.release_id)
        .join(&artifact.host)
        .join(&artifact.target_profile)
        .join(&artifact.sha256);
    assert!(!envelope.exists());
    assert_eq!(
        fs::read(fixture.payload.join(AROS_TOOLCHAIN_MANIFEST_FILE)).unwrap(),
        original_manifest
    );
}

#[test]
fn gnu_driver_machine_and_version_are_observed_not_inferred_from_names() {
    for (from, to, diagnostic) in [
        ("riscv64-aros", "riscv-aros", "-dumpmachine"),
        ("16.2.0", "16.3.0", "-dumpfullversion"),
    ] {
        let fixture = Fixture::new(64);
        let driver = fixture
            .payload
            .join(fixture.layout["tools"]["c"].as_str().unwrap());
        fs::write(
            &driver,
            fs::read_to_string(&driver).unwrap().replace(from, to),
        )
        .unwrap();
        fixture.write_manifest();
        failure(&fixture.local("verify"), diagnostic);
        assert!(!fixture.store.exists());
    }
}
