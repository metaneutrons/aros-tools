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

    fn add_native_source_contract(&mut self) -> Value {
        self.layout["schema"] = json!("aros-toolchain-tools-v3");
        let relative = format!("{}-objdump", self.triple);
        self.layout["tools"]["objdump"] = json!(relative);
        let executable = self.payload.join(&relative);
        fs::write(
            &executable,
            fs::read(self.payload.join(format!("{}-gcc", self.triple))).unwrap(),
        )
        .unwrap();
        fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
        self.write_layout();
        self.write_manifest();
        let declared = [
            "native/core.src",
            "native/linker.lds",
            "native/check.sh",
            "native/package.src",
            "native/board.mk",
            "native/partitions.csv",
            "native/sdkconfig",
            "native/bootloader.diff",
        ];
        let inputs = declared
            .iter()
            .map(|relative| {
                let path = self.project.join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, format!("synthetic source: {relative}\n")).unwrap();
                json!({"path": relative, "sha256": sha256_file(&path).unwrap().digest})
            })
            .collect::<Vec<_>>();
        let document = json!({
            "schema_version": 1, "profile": "fixture-target", "board": "fixture",
            "source_baseline": "1".repeat(40), "qualification": "experimental-unqualified",
            "inputs": inputs,
            "abi": {"source_cpu": "riscv", "target_triple": "riscv-aros",
                "isa": "rv32imafc", "abi": "ilp32f", "code_model": "medany",
                "flavour": "standalone", "platform_smp": false, "use_mmu": false},
            "core": {"recipe": "native/core.src", "linker_script": "native/linker.lds",
                "residency_check": "native/check.sh", "resources": ["kernel"],
                "residency_policy": {
                    "algorithm": "riscv32-xip-v1", "section": ".sramtext",
                    "flash_start": 1_073_741_824, "flash_end": 1_140_850_688,
                    "sram_start": 1_341_128_704, "sram_end": 1_341_652_992
                },
                "libraries": ["exec"], "devices": ["timer"], "link_libraries": ["exec"],
                "compiler_runtime_role": "libgcc"},
            "package": {"recipe": "native/package.src", "format": "aros-pkg-v1",
                "target": "synthetic-package", "limit_from_board": "BOARD_LIMIT"},
            "media": {"chip": "synthetic-chip", "board_rules": "native/board.mk",
                "partition_table": "native/partitions.csv", "core_partition": "core",
                "package_partition": "bsp", "development_volume_offset_from_board": "BOARD_VOLUME",
                "bootloader_configuration": "native/sdkconfig", "bootloader_patch": "native/bootloader.diff",
                "idf_version": "6.0.1"}
        });
        fs::write(
            self.project.join("native-build-v1.json"),
            serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
        let profile = fs::read_to_string(self.project.join("aros-targets.toml"))
            .unwrap()
            .replace(
                "[targets.transpiler]",
                "native_build_contract='native-build-v1.json'\n[targets.transpiler]",
            );
        fs::write(
            self.project.join("aros-targets.toml"),
            format!("{profile}[targets.bootstrap_abi]\nflavour='standalone'\nplatform_smp=false\n"),
        )
        .unwrap();
        document
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

    fn cli_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
        command
            .current_dir(&self.project)
            .env("AROS_HOME", &self.home)
            .env("AROS_CACHE_DIR", &self.cache)
            .env("AROS_CROSS_TOOLCHAINS_DIR", &self.store)
            .env_remove("AROS_LOG_FILE")
            .env_remove("AROS_LOG_LEVEL")
            .env_remove("AROS_LOG_FORMAT")
            .args(["--diagnostic-format", "json"]);
        command
    }

    fn base_command(&self) -> Command {
        let mut command = self.cli_command();
        command.arg("toolchain");
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

/// The child CMake is a recorder, not a compiler: this proves that the public
/// build command forwards the verified GNU contract across its process boundary.
#[test]
fn native_build_forwards_declared_gnu_roles_without_llvm_or_host_fallback() {
    let mut fixture = Fixture::new(32);
    fixture.add_native_utilities();
    let mut contract = fixture.add_native_source_contract();
    contract["profile"] = json!("fixture-board");
    fs::write(
        fixture.project.join("native-build-v1.json"),
        serde_json::to_vec(&contract).unwrap(),
    )
    .unwrap();
    let profiles = fixture.project.join("aros-targets.toml");
    let profile_text = fs::read_to_string(&profiles).unwrap();
    fs::write(
        profiles,
        profile_text.replace(
            "name='fixture-target'",
            "name='fixture-board'\ntoolchain_profile='fixture-target'",
        ),
    )
    .unwrap();
    let tools = fixture.temporary.path().join("suite");
    fs::create_dir(&tools).unwrap();
    for name in [
        "aros-transpiler",
        "aros-genmodule",
        "aros-romtool",
        "aros-collect",
        "aros-ahi-runner",
        "aros-fetch",
        "aros-verify",
    ] {
        let path = tools.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\n[ \"${{1:-}}\" = --version ] || exit 99\nprintf '%s\\n' '{name} {}'\n",
                env!("CARGO_PKG_VERSION")
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let recorder = fixture.temporary.path().join("cmake-arguments");
    let cmake = tools.join("cmake");
    fs::write(
        &cmake,
        "#!/bin/sh\nprintf '%s\\n' invocation \"$@\" >> \"$AROS_TEST_CMAKE_ARGUMENTS\"\n",
    )
    .unwrap();
    fs::set_permissions(cmake, fs::Permissions::from_mode(0o755)).unwrap();
    let mut path = vec![tools.clone()];
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let run = |fixture: &Fixture| {
        fixture
            .cli_command()
            .env("AROS_BUILD_TOOLS_DIR", &tools)
            .env("AROS_TEST_CMAKE_ARGUMENTS", &recorder)
            .env("PATH", std::env::join_paths(&path).unwrap())
            .args([
                "build",
                "--preset",
                "fixture-board",
                "--offline",
                "--compiler-cache",
                "off",
                "--toolchain-dir",
            ])
            .arg(&fixture.payload)
            .output()
            .unwrap()
    };
    success(&run(&fixture));
    let observed = fs::read_to_string(&recorder).unwrap();
    for value in [
        "-DAROS_TOOLCHAIN=gnu",
        "-DAROS_TARGET_CPU=riscv",
        "-DAROS_TARGET_PROFILE=fixture-board",
        "-DAROS_CROSS_TOOLCHAIN_PROFILE=fixture-target",
        "-DAROS_TARGET_TRIPLE=riscv-aros",
        "-DAROS_ENABLE_MMU=OFF",
        "-DAROS_TARGET_VARIANT=",
        "-DAROS_TARGET_CPU32=",
        "-DAROS_ABI_FLAVOUR=standalone",
        "-DAROS_ABI_PLATFORM_SMP=OFF",
    ] {
        assert!(
            observed.lines().any(|line| line == value),
            "{value}: {observed}"
        );
    }
    for (variable, role) in [
        ("CMAKE_C_COMPILER", "c"),
        ("CMAKE_CXX_COMPILER", "cxx"),
        ("CMAKE_ASM_COMPILER", "c"),
        ("AROS_AS_BIN", "assembler"),
        ("AROS_LINKER_BIN", "linker"),
        ("AROS_COLLECT_BIN", "collector"),
        ("CMAKE_NM", "nm"),
        ("CMAKE_OBJCOPY", "objcopy"),
        ("CMAKE_OBJDUMP", "objdump"),
    ] {
        let expected = format!(
            "-D{variable}={}",
            fixture
                .payload
                .canonicalize()
                .unwrap()
                .join(fixture.layout["tools"][role].as_str().unwrap())
                .display()
        );
        assert!(
            observed.lines().any(|line| line == expected),
            "{expected}: {observed}"
        );
    }
    assert!(!observed.contains("clang"));
    assert!(!observed.contains("AROS_LLD_BIN"));
    for expected in [
        format!(
            "-DAROS_NATIVE_BUILD_CONTRACT={}",
            fixture
                .project
                .join("native-build-v1.json")
                .canonicalize()
                .unwrap()
                .display()
        ),
        format!(
            "-DAROS_NATIVE_BUILD_CONTRACT_SHA256={}",
            sha256_file(&fixture.project.join("native-build-v1.json"))
                .unwrap()
                .digest
        ),
    ] {
        assert!(
            observed.lines().any(|line| line == expected),
            "{expected}: {observed}"
        );
    }
    assert_eq!(
        observed
            .lines()
            .filter(|line| *line == "invocation")
            .count(),
        2
    );

    // A same-width but different source ISA is not a compatible compiler.
    let mut changed = contract.clone();
    changed["abi"]["isa"] = json!("rv32imafdc");
    fs::write(
        fixture.project.join("native-build-v1.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    failure(&run(&fixture), "differs from the verified compiler target");
    assert_eq!(fs::read_to_string(&recorder).unwrap(), observed);
    fs::write(
        fixture.project.join("native-build-v1.json"),
        serde_json::to_vec(&contract).unwrap(),
    )
    .unwrap();

    // Altered source input fails before even a toolchain probe executes.
    let probes = fs::read(&fixture.marker).unwrap();
    let source = fixture.project.join("native/linker.lds");
    let original = fs::read(&source).unwrap();
    fs::write(&source, b"corrupted source input\n").unwrap();
    failure(&run(&fixture), "SHA-256 differs");
    assert_eq!(fs::read(&fixture.marker).unwrap(), probes);
    assert_eq!(fs::read_to_string(&recorder).unwrap(), observed);
    fs::write(source, original).unwrap();

    // An installation-compatible v1 layout is not a complete native GNU build
    // contract. Missing utilities fail before either CMake child can run.
    fixture.layout["schema"] = json!("aros-toolchain-tools-v1");
    fixture.layout["tools"]
        .as_object_mut()
        .unwrap()
        .remove("nm");
    fixture.layout["tools"]
        .as_object_mut()
        .unwrap()
        .remove("objcopy");
    fixture.layout["tools"]
        .as_object_mut()
        .unwrap()
        .remove("objdump");
    fixture.write_layout();
    fixture.write_manifest();
    let rejected = run(&fixture);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("one verified objdump role"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(fs::read_to_string(recorder).unwrap(), observed);
}

fn local_native_fixture() -> (Fixture, PathBuf, PathBuf) {
    let mut fixture = Fixture::new(32);
    fixture.add_native_utilities();
    fixture.add_native_source_contract();
    fs::remove_file(fixture.payload.join(AROS_TOOLCHAIN_MANIFEST_FILE)).unwrap();
    let compiler = serde_json::from_value(fixture.layout["compiler"].clone()).unwrap();
    let descriptor = aros_common::local_toolchain::LocalToolchainDescriptor::capture(
        &fixture.payload,
        native_host_key().unwrap(),
        "fixture-target",
        &fixture.triple,
        compiler,
    )
    .unwrap();
    fs::write(
        fixture.payload.join("toolchain-local.json"),
        serde_json::to_vec(&descriptor).unwrap(),
    )
    .unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Native input fixture"],
        vec!["config", "user.email", "native-input@example.invalid"],
        vec!["add", "."],
        vec!["commit", "-qm", "Initial native input fixture"],
    ] {
        let output = Command::new("git")
            .arg("-C")
            .arg(&fixture.project)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let tools = fixture.temporary.path().join("suite");
    fs::create_dir(&tools).unwrap();
    for name in [
        "aros-transpiler",
        "aros-genmodule",
        "aros-romtool",
        "aros-collect",
        "aros-ahi-runner",
        "aros-fetch",
        "aros-verify",
    ] {
        let path = tools.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\n[ \"${{1:-}}\" = --version ] || exit 99\nprintf '%s\\n' '{name} {}'\n",
                env!("CARGO_PKG_VERSION")
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let recorder = fixture.temporary.path().join("cmake-invocations");
    let cmake = tools.join("cmake");
    fs::write(&cmake, "#!/bin/sh\nprintf '%s\\n' invocation \"$@\" >> \"$AROS_TEST_CMAKE_ARGUMENTS\"\nif [ \"$1\" != --build ]; then mkdir -p \"$AROS_TEST_CMAKE_BUILD_ROOT/CMakeFiles\"; for file in CMakeCache.txt build.ninja CMakeFiles/rules.ninja; do printf '%s' 'synthetic configured rule' > \"$AROS_TEST_CMAKE_BUILD_ROOT/$file\"; done; fi\nif [ -n \"$AROS_TEST_LOCAL_INPUT\" ]; then printf '%s' 'modified during configure' > \"$AROS_TEST_LOCAL_INPUT\"; fi\n").unwrap();
    fs::set_permissions(cmake, fs::Permissions::from_mode(0o755)).unwrap();
    (fixture, tools, recorder)
}

fn local_native_command(
    fixture: &Fixture,
    tools: &std::path::Path,
    recorder: &std::path::Path,
) -> Command {
    let mut path = vec![tools.to_path_buf()];
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let mut command = fixture.cli_command();
    command
        .env("AROS_BUILD_TOOLS_DIR", tools)
        .env("AROS_TEST_CMAKE_ARGUMENTS", recorder)
        .env(
            "AROS_TEST_CMAKE_BUILD_ROOT",
            fixture.project.join("build/fixture-target"),
        )
        .env("PATH", std::env::join_paths(path).unwrap())
        .args([
            "build",
            "--preset",
            "fixture-target",
            "--offline",
            "--compiler-cache",
            "off",
            "--toolchain-dir",
        ])
        .arg(&fixture.payload);
    command
}

#[test]
fn local_native_cli_binds_source_and_refuses_drift_without_adopting_outputs() {
    let (fixture, tools, recorder) = local_native_fixture();
    success(
        &local_native_command(&fixture, &tools, &recorder)
            .output()
            .unwrap(),
    );
    let build = fixture.project.join("build/fixture-target");
    let stamp_path = build.join(".aros-local-native-inputs.json");
    let stamp = fs::read(&stamp_path).unwrap();
    let document: Value = serde_json::from_slice(&stamp).unwrap();
    assert_eq!(document["schema"], "aros-local-native-inputs-v1");
    assert_eq!(document["source"]["schema"], "aros-local-source-v1");
    assert!(document.get("release_id").is_none());
    assert!(document.get("build_succeeded").is_none());
    assert_eq!(document["executable_sha256"].as_object().unwrap().len(), 10);
    let invocations = fs::read(&recorder).unwrap();
    let dirty = fixture.project.join("untracked-source.c");
    fs::write(&dirty, b"new local source").unwrap();
    failure(
        &local_native_command(&fixture, &tools, &recorder)
            .output()
            .unwrap(),
        "local native inputs changed",
    );
    assert_eq!(fs::read(&stamp_path).unwrap(), stamp);
    assert_eq!(fs::read(&recorder).unwrap(), invocations);
    fs::remove_file(dirty).unwrap();

    // Same source and stamp do not authorize a changed persistent build graph.
    let cache_path = build.join("CMakeCache.txt");
    let cache = fs::read(&cache_path).unwrap();
    fs::write(&cache_path, b"CMAKE_C_FLAGS:STRING=-DUNREVIEWED=1").unwrap();
    failure(
        &local_native_command(&fixture, &tools, &recorder)
            .output()
            .unwrap(),
        "cache/Ninja rules differ",
    );
    assert_eq!(fs::read(&recorder).unwrap(), invocations);
    fs::write(&cache_path, cache).unwrap();

    // A contract beneath --clean's namespace must be rejected before deletion.
    fs::copy(
        fixture.project.join("native-build-v1.json"),
        build.join("native-contract.json"),
    )
    .unwrap();
    let target_path = fixture.project.join("aros-targets.toml");
    let targets = fs::read_to_string(&target_path).unwrap().replace(
        "native-build-v1.json",
        "build/fixture-target/native-contract.json",
    );
    fs::write(&target_path, targets).unwrap();
    let preserved = fs::read(build.join("native-contract.json")).unwrap();
    failure(
        &local_native_command(&fixture, &tools, &recorder)
            .arg("--clean")
            .output()
            .unwrap(),
        "generated build namespace",
    );
    assert_eq!(
        fs::read(build.join("native-contract.json")).unwrap(),
        preserved
    );
    assert_eq!(fs::read(&stamp_path).unwrap(), stamp);
    assert_eq!(fs::read(&recorder).unwrap(), invocations);
}

#[test]
fn local_native_cli_refuses_input_changes_during_configure_before_build() {
    let (fixture, tools, recorder) = local_native_fixture();
    let output = local_native_command(&fixture, &tools, &recorder)
        .env(
            "AROS_TEST_LOCAL_INPUT",
            fixture.project.join("untracked-source.c"),
        )
        .output()
        .unwrap();
    failure(&output, "inputs changed during execution");
    let arguments = fs::read_to_string(recorder).unwrap();
    assert_eq!(
        arguments
            .lines()
            .filter(|line| *line == "invocation")
            .count(),
        1
    );
    assert!(!arguments.lines().any(|line| line == "--build"));
    assert!(fixture
        .project
        .join("build/fixture-target/.aros-local-native-inputs.json")
        .is_file());
}

#[test]
fn local_native_cli_refuses_engine_links_before_configure() {
    let (fixture, tools, recorder) = local_native_fixture();
    let engine = fixture.temporary.path().join("engine");
    fs::create_dir_all(engine.join("toolchains")).unwrap();
    fs::write(engine.join("AROS.cmake"), b"synthetic engine").unwrap();
    fs::write(engine.join("toolchains/AROS.cmake"), b"synthetic toolchain").unwrap();
    let outside = fixture.temporary.path().join("outside.cmake");
    fs::write(&outside, b"unbound engine input").unwrap();
    symlink(&outside, engine.join("linked.cmake")).unwrap();
    failure(
        &local_native_command(&fixture, &tools, &recorder)
            .arg("--engine-dir")
            .arg(engine)
            .output()
            .unwrap(),
        "only regular files and directories",
    );
    assert!(!recorder.exists());
    assert_eq!(fs::read(outside).unwrap(), b"unbound engine input");
}

#[test]
fn local_native_cli_clean_refuses_symlinked_build_parent() {
    let (fixture, tools, recorder) = local_native_fixture();
    let outside = fixture.temporary.path().join("outside-build");
    fs::create_dir_all(outside.join("fixture-target")).unwrap();
    let sentinel = outside.join("fixture-target/preserved");
    fs::write(&sentinel, b"must survive cleanup refusal").unwrap();
    symlink(&outside, fixture.project.join("build")).unwrap();
    let output = local_native_command(&fixture, &tools, &recorder)
        .arg("--clean")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "cleanup must reject a linked parent"
    );
    assert!(!recorder.exists());
    assert_eq!(fs::read(sentinel).unwrap(), b"must survive cleanup refusal");
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
fn board_alias_selects_an_explicit_shared_compiler_for_local_and_locked_installs() {
    let fixture = Fixture::new(32);
    fixture.seed_locked_archive();
    let config = fs::read_to_string(fixture.project.join("aros-targets.toml")).unwrap();
    let config = config.replace(
        "name='fixture-target'",
        "name='board-a'\ntoolchain_profile='fixture-target'",
    );
    fs::write(fixture.project.join("aros-targets.toml"), &config).unwrap();
    let command = || {
        let mut command = fixture.cli_command();
        command.args(["toolchain", "install", "--preset", "board-a"]);
        command
    };
    success(
        &command()
            .arg("--local")
            .arg(&fixture.payload)
            .output()
            .unwrap(),
    );
    success(&command().arg("--offline").output().unwrap());
    // A similar name is not a compatible compiler identity and must never
    // silently select the first lock entry or mutate the measured manifest.
    fs::write(
        fixture.project.join("aros-targets.toml"),
        config.replace(
            "toolchain_profile='fixture-target'",
            "toolchain_profile='other-compiler'",
        ),
    )
    .unwrap();
    failure(
        &command()
            .arg("--local")
            .arg(&fixture.payload)
            .output()
            .unwrap(),
        "local manifest is for",
    );
    failure(
        &command().arg("--offline").output().unwrap(),
        "no locked AROS toolchain",
    );
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
