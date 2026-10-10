//! A complete synthetic native lifecycle proves orchestration, not a compiler release.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use aros_common::toolchain_layout::ToolchainToolLayout;
use aros_common::{
    measure_tree_content_cas, sha256_bytes, ArosCompilerIdentity, CancellationToken,
};
use aros_toolchain::canonical;
use aros_toolchain::cargo_vendor::{
    fetch_vendor_generation, select_vendor_generation, CargoVendorRequest,
};
use aros_toolchain::executor::{self, BuildRequest, BuildResult, ResumePhase};
use aros_toolchain::native_candidate::{
    readback_finished_build_result, readback_finished_candidate, FinishedBuildResultRequest,
    FinishedCandidateRequest,
};
use aros_toolchain::profiles::Profiles;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::json;
use tar::Builder;

const LLVM_FIXTURE_CONFIGURE: &str = r#"#!/bin/sh
set -eu
prefix=
cache=
for arg in "$@"; do
  case "$arg" in
    --with-aros-toolchain-install=*) prefix=${arg#*=} ;;
    --with-portssources=*) cache=${arg#*=} ;;
  esac
done
test -n "$prefix"
test -n "$cache"
test "${CFLAGS+set}" = set
test "${CXXFLAGS+set}" = set
printf '%s\n' "$@" > configure.args
printf '%s\n' "$CMAKE_BUILD_PARALLEL_LEVEL" > configure-cmake-jobs
printf 'crosstools-release:\n\t@test "$${CFLAGS+set}" = set\n\t@test "$${CXXFLAGS+set}" = set\n\t@$(FETCH) -a llvm-11.0.0.src -s tar.xz -l %s\n\t@printf "%%s\\n" "$${CMAKE_BUILD_PARALLEL_LEVEL}" > native-cmake-jobs\n\t@mkdir -p %s/bin %s/lib/cmake/llvm\n\t@printf compiler > %s/bin/clang\n\t@chmod 755 %s/bin/clang\n\t@printf producer-only > %s/bin/llvm-config\n' "$cache" "$prefix" "$prefix" "$prefix" "$prefix" "$prefix" > Makefile
"#;

const GNU_FIXTURE_CONFIGURE: &str = r#"#!/bin/sh
set -eu
prefix=
cache=
toolchain=
gcc_version=
binutils_version=
for arg in "$@"; do
  case "$arg" in
    --with-aros-toolchain-install=*) prefix=${arg#*=} ;;
    --with-portssources=*) cache=${arg#*=} ;;
    --with-toolchain=*) toolchain=${arg#*=} ;;
    --with-gcc-version=*) gcc_version=${arg#*=} ;;
    --with-binutils-version=*) binutils_version=${arg#*=} ;;
  esac
done
test -n "$prefix"
test -n "$cache"
test "$toolchain" = gnu
test "$gcc_version" = 16.2.0
test "$binutils_version" = 2.47
test -x "$CC"
test -x "$CXX"
case "$CFLAGS" in *-ffile-prefix-map=*) ;; *) exit 1 ;; esac
case "$CXXFLAGS" in *-ffile-prefix-map=*) ;; *) exit 1 ;; esac
recursive_make="$(command -v make)"
test -L "$recursive_make"
readlink "$recursive_make" > configure-recursive-make
printf 'probe:\n\t@printf "%%s\\n" "$(MAKE_VERSION)" > recursive-make-version\n' > recursive-make.mk
make -f recursive-make.mk
printf '%s\n' "$@" > configure.args
printf '%s\n' "$CMAKE_BUILD_PARALLEL_LEVEL" > configure-cmake-jobs
printf '%s\n' "$CC" > configure-cc
printf '%s\n' "$CXX" > configure-cxx
printf 'PREFIX := %s\nCACHE := %s\nCFLAGS := -march=rva22u64\nCXXFLAGS := -mabi=lp64d\ncrosstools-release:\n\t@test "$${CFLAGS+set}" != set\n\t@test "$${CXXFLAGS+set}" != set\n\t@$(FETCH) -a gcc-16.2.0 -s tar.xz -l $(CACHE)\n\t@$(FETCH) -a binutils-2.47 -s tar.bz2 -l $(CACHE)\n\t@mkdir -p $(PREFIX)/riscv64-aros/bin $(PREFIX)/bin $(PREFIX)/lib/cmake/llvm\n\t@for tool in gcc g++ as ld ar ranlib strip nm objcopy objdump; do printf root-tool > $(PREFIX)/riscv64-aros-$$tool; chmod 755 $(PREFIX)/riscv64-aros-$$tool; done\n\t@for tool in ld strip; do printf tuple-tool > $(PREFIX)/riscv64-aros/bin/$$tool; chmod 755 $(PREFIX)/riscv64-aros/bin/$$tool; done\n\t@printf legacy-collector > $(PREFIX)/riscv64-aros/bin/collect-aros\n\t@chmod 755 $(PREFIX)/riscv64-aros/bin/collect-aros\n\t@printf legacy-collector > $(PREFIX)/riscv64-aros-collect-aros\n\t@chmod 755 $(PREFIX)/riscv64-aros-collect-aros\n\t@printf producer-only > $(PREFIX)/bin/llvm-config\n\t@printf retained-by-gnu > $(PREFIX)/lib/cmake/llvm/producer.marker\n' "$prefix" "$cache" > Makefile
"#;

const GNU_FIXTURE_BRIDGE: &str = r#"#!/bin/bash
set -eu
test "$1" = toolchain
test "$2" = __metamake-fetch
arguments=("$@")
shift 2
archive=
suffix=
while (($#)); do
  case "$1" in
    -a) archive=$2; shift 2 ;;
    -s) suffix=$2; shift 2 ;;
    -l) shift 2 ;;
    *) shift ;;
  esac
done
test -n "$archive"
test -n "$suffix"
printf '%s.%s\n' "$archive" "$suffix" >> "$AROS_TOOLCHAIN_FETCH_LEDGER"
exec /bin/bash "$AROS_TOOLCHAIN_FETCH_UPSTREAM" "${arguments[@]}"
"#;

struct GroupFixture {
    source_lock_path: String,
    profiles_path: String,
    source_lock: Vec<u8>,
    profiles: Vec<u8>,
}

fn v2_llvm_source_lock() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
        "sources": [{
            "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
            "patch": "tools/crosstools/llvm/llvm-11.0.0.src-aros.diff",
            "filename": "llvm-11.0.0.src.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
            "sha256": "a".repeat(64), "size": 1
        }],
        "host_python_packages": [
            {"name": "mako", "version": "1.3.10", "filename": "mako.tar.gz", "url": "https://example.invalid/mako.tar.gz", "sha256": "b".repeat(64), "size": 1, "source_root": "mako", "python_path": "."},
            {"name": "markupsafe", "version": "3.0.2", "filename": "markupsafe.tar.gz", "url": "https://example.invalid/markupsafe.tar.gz", "sha256": "c".repeat(64), "size": 1, "source_root": "markupsafe", "python_path": "."}
        ]
    }))
    .unwrap()
}

fn v2_llvm_profiles() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v1", "upstream_commit": "4".repeat(40),
        "profiles": [{
            "name": "pc-x86_64", "configure_target": "pc-x86_64", "upstream_output_target": "pc-x86_64",
            "target_triple": "x86_64-unknown-aros", "cpu": "x86_64", "platform": "pc", "float_abi": "",
            "capabilities": ["c", "cxx", "standalone-collector"]
        }]
    }))
    .unwrap()
}

fn v2_gnu_profiles() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v2", "family": "gnu", "upstream_commit": "4".repeat(40),
        "profiles": [{
            "name": "rv64-reference", "configure_target": "opensbi-riscv64", "upstream_output_target": "opensbi-riscv64",
            "target_triple": "riscv64-aros", "cpu": "riscv64", "platform": "opensbi", "float_abi": "lp64d",
            "capabilities": ["c", "libgcc", "standalone-collector"],
            "target": {"schema": "aros-riscv-target-v1", "isa": "rva22u64", "abi": "lp64d",
                "code_model": "medany", "architecture": "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
                "unaligned_access": false, "atomic_abi": 0, "x3_reg_usage": 0}
        }]
    }))
    .unwrap()
}

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    recipe: PathBuf,
    bridge: PathBuf,
    preset: String,
    source_lock_relative: String,
    profiles_relative: String,
}

impl Fixture {
    fn new() -> Self {
        Self::new_inner(false)
    }

    fn new_with_cold_vendor_generation() -> Self {
        Self::new_inner(true)
    }

    fn new_inner(cold_vendor_generation: bool) -> Self {
        Self::new_inner_for_family(cold_vendor_generation, false)
    }

    fn new_gnu() -> Self {
        Self::new_inner_for_family(false, true)
    }

    fn new_v2(gnu: bool) -> (Self, PathBuf) {
        let fixture = if gnu { Self::new_gnu() } else { Self::new() };
        let unselected_profiles = fixture.install_v2_groups(gnu);
        (fixture, unselected_profiles)
    }

    fn new_inner_for_family(cold_vendor_generation: bool, gnu: bool) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        Self::new_with_temporary_root(cold_vendor_generation, gnu, temporary)
    }

    fn new_short() -> Self {
        Self::new_with_temporary_root(false, false, tempfile::tempdir_in("/tmp").unwrap())
    }

    fn new_with_temporary_root(
        cold_vendor_generation: bool,
        gnu: bool,
        temporary: tempfile::TempDir,
    ) -> Self {
        let root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("native-lifecycle");
        fs::create_dir(&root).unwrap();
        for name in ["source", "producer", "tools"] {
            let path = root.join(name);
            fs::create_dir(&path).unwrap();
            git(&path, &["init", "-q", "--template="]);
        }
        let cache = root.join("cache");
        fs::create_dir(&cache).unwrap();

        let patch_path = "tools/crosstools/llvm/llvm-11.0.0.src-aros.diff";
        if !gnu {
            fs::create_dir_all(root.join("source/tools/crosstools/llvm")).unwrap();
            fs::write(root.join("source").join(patch_path), b"fixture patch\n").unwrap();
        }
        fs::create_dir_all(root.join("source/scripts")).unwrap();
        write_executable(
            &root.join("source/configure"),
            if gnu {
                GNU_FIXTURE_CONFIGURE
            } else {
                LLVM_FIXTURE_CONFIGURE
            },
        );
        write_executable(
            &root.join("source/scripts/fetch.sh"),
            "#!/bin/sh\nset -eu\nexit 0\n",
        );

        let source_payload = b"x";
        if !gnu {
            fs::write(cache.join("llvm-11.0.0.src.tar.xz"), source_payload).unwrap();
        }
        let gcc_payload = b"fixture gcc 16.2.0 source";
        let binutils_payload = b"fixture binutils 2.47 source";
        if gnu {
            fs::write(cache.join("gcc-16.2.0.tar.xz"), gcc_payload).unwrap();
            fs::write(cache.join("binutils-2.47.tar.bz2"), binutils_payload).unwrap();
        }
        let mako = python_archive(
            "mako",
            &[
                ("mako/__init__.py", b"__version__ = '1.3.10'\n"),
                (
                    "mako/template.py",
                    b"class Template:\n    def __init__(self, text): self.text = text\n    def render(self): return self.text\n",
                ),
            ],
        );
        let markupsafe = python_archive(
            "markupsafe",
            &[("markupsafe/__init__.py", b"__version__ = '3.0.2'\n")],
        );
        fs::write(cache.join("mako.tar.gz"), &mako).unwrap();
        fs::write(cache.join("markupsafe.tar.gz"), &markupsafe).unwrap();
        let package_checksum = "d".repeat(64);
        if !cold_vendor_generation {
            let vendor = cache.join("cargo-vendor/fixture-dependency-1.0.0");
            fs::create_dir_all(vendor.join("src")).unwrap();
            let vendor_manifest = b"[package]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\nedition = \"2021\"\n";
            let vendor_source = b"pub fn answer() -> u8 { 42 }\n";
            fs::write(vendor.join("Cargo.toml"), vendor_manifest).unwrap();
            fs::write(vendor.join("src/lib.rs"), vendor_source).unwrap();
            fs::write(
                vendor.join(".cargo-checksum.json"),
                serde_json::to_vec(&json!({
                    "package": package_checksum,
                    "files": {
                        "Cargo.toml": sha256_bytes(vendor_manifest),
                        "src/lib.rs": sha256_bytes(vendor_source)
                    }
                }))
                .unwrap(),
            )
            .unwrap();
            fs::write(
                cache.join("cargo-vendor-config.toml"),
                "[source.crates-io]\nreplace-with = \"vendored-sources\"\n[source.vendored-sources]\ndirectory = \"__CARGO_VENDOR_DIRECTORY__\"\n",
            )
            .unwrap();
        }

        fs::create_dir_all(root.join("tools/contracts")).unwrap();
        fs::create_dir_all(root.join("tools/src")).unwrap();
        let contract = b"native lifecycle fixture contract\n";
        fs::write(
            root.join("tools/contracts/toolchain-producer-v1.toml"),
            contract,
        )
        .unwrap();
        fs::write(
            root.join("tools/Cargo.toml"),
            "[package]\nname = \"aros-collect\"\nversion = \"0.0.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n[dependencies]\nfixture-dependency = \"1.0.0\"\n",
        )
        .unwrap();
        fs::write(root.join("tools/Cargo.lock"), format!("version = 4\n\n[[package]]\nname = \"aros-collect\"\nversion = \"0.0.0\"\ndependencies = [\"fixture-dependency\"]\n\n[[package]]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{package_checksum}\"\n")).unwrap();
        fs::write(
            root.join("tools/src/main.rs"),
            "fn main() { assert_eq!(fixture_dependency::answer(), 42); print!(\"{}\", env!(\"FIXTURE_CARGO_JOBS\")); }\n",
        )
        .unwrap();
        fs::write(
            root.join("tools/build.rs"),
            "fn main() { println!(\"cargo:rustc-env=FIXTURE_CARGO_JOBS={}\", std::env::var(\"CARGO_BUILD_JOBS\").unwrap()); }\n",
        )
        .unwrap();
        git(&root.join("tools"), &["add", "."]);
        git(
            &root.join("tools"),
            &["commit", "-qm", "test: native lifecycle tools"],
        );
        let tools_commit = git(&root.join("tools"), &["rev-parse", "HEAD"]);

        let lock = if gnu {
            serde_json::to_vec(&json!({
                "schema": "aros-toolchain-source-lock-v3", "family": "gnu", "version": "16.2.0",
                "sources": [
                    {"component": "gcc", "version": "16.2.0", "purpose": "toolchain-component",
                     "filename": "gcc-16.2.0.tar.xz", "url": "https://example.invalid/gcc-16.2.0.tar.xz",
                     "sha256": sha256_bytes(gcc_payload), "size": gcc_payload.len()},
                    {"component": "binutils", "version": "2.47", "purpose": "toolchain-component",
                     "filename": "binutils-2.47.tar.bz2", "url": "https://example.invalid/binutils-2.47.tar.bz2",
                     "sha256": sha256_bytes(binutils_payload), "size": binutils_payload.len()}
                ],
                "host_python_packages": [
                    {"name": "mako", "version": "1.3.10", "filename": "mako.tar.gz", "url": "https://example.invalid/mako.tar.gz", "sha256": sha256_bytes(&mako), "size": mako.len(), "source_root": "mako", "python_path": "."},
                    {"name": "markupsafe", "version": "3.0.2", "filename": "markupsafe.tar.gz", "url": "https://example.invalid/markupsafe.tar.gz", "sha256": sha256_bytes(&markupsafe), "size": markupsafe.len(), "source_root": "markupsafe", "python_path": "."}
                ]
            }))
            .unwrap()
        } else {
            serde_json::to_vec(&json!({
                "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
                "sources": [{
                    "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
                    "patch": patch_path, "filename": "llvm-11.0.0.src.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
                    "sha256": sha256_bytes(source_payload), "size": source_payload.len()
                }],
                "host_python_packages": [
                    {"name": "mako", "version": "1.3.10", "filename": "mako.tar.gz", "url": "https://example.invalid/mako.tar.gz", "sha256": sha256_bytes(&mako), "size": mako.len(), "source_root": "mako", "python_path": "."},
                    {"name": "markupsafe", "version": "3.0.2", "filename": "markupsafe.tar.gz", "url": "https://example.invalid/markupsafe.tar.gz", "sha256": sha256_bytes(&markupsafe), "size": markupsafe.len(), "source_root": "markupsafe", "python_path": "."}
                ]
            }))
            .unwrap()
        };
        let profiles = if gnu {
            serde_json::to_vec(&json!({
                "schema": "aros-toolchain-profiles-v2", "family": "gnu", "upstream_commit": "4".repeat(40),
                "profiles": [{
                    "name": "rv64-reference", "configure_target": "opensbi-riscv64", "upstream_output_target": "opensbi-riscv64",
                    "target_triple": "riscv64-aros", "cpu": "riscv64", "platform": "opensbi", "float_abi": "lp64d",
                    "capabilities": ["c", "libgcc", "standalone-collector"],
                    "target": {"schema": "aros-riscv-target-v1", "isa": "rva22u64", "abi": "lp64d",
                        "code_model": "medany", "architecture": "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
                        "unaligned_access": false, "atomic_abi": 0, "x3_reg_usage": 0}
                }]
            }))
            .unwrap()
        } else {
            serde_json::to_vec(&json!({
                "schema": "aros-toolchain-profiles-v1", "upstream_commit": "4".repeat(40),
                "profiles": [{
                    "name": "pc-x86_64", "configure_target": "pc-x86_64", "upstream_output_target": "pc-x86_64",
                    "target_triple": "x86_64-unknown-aros", "cpu": "x86_64", "platform": "pc", "float_abi": "",
                    "capabilities": ["c", "cxx", "standalone-collector"]
                }]
            }))
            .unwrap()
        };
        let source_lock_relative = if gnu {
            "toolchains/gnu.sources.json"
        } else {
            "toolchains/fixture.sources.json"
        };
        let profiles_relative = if gnu {
            "toolchains/profiles-v2.json"
        } else {
            "toolchains/profiles-v1.json"
        };
        fs::create_dir_all(root.join("producer/toolchains")).unwrap();
        let cargo_channel = current_cargo_channel();
        fs::write(
            root.join("producer/toolchains/rust-toolchain.toml"),
            format!(
                "[toolchain]\nchannel = \"{cargo_channel}\"\nprofile = \"minimal\"\ncomponents = [\"clippy\", \"rustfmt\"]\n"
            ),
        )
        .unwrap();
        fs::write(root.join("producer").join(source_lock_relative), &lock).unwrap();
        fs::write(root.join("producer").join(profiles_relative), &profiles).unwrap();
        fs::write(
            root.join("producer/toolchains/producer-executor-v1.toml"),
            format!(
                "schema_version = 1\ncontract_id = \"aros-toolchain-producer-v1\"\ncontract_path = \"contracts/toolchain-producer-v1.toml\"\ncontract_sha256 = \"{}\"\ntools_commit = \"{}\"\nsource_lock = \"{}\"\nprofiles = \"{}\"\n",
                sha256_bytes(contract), tools_commit, source_lock_relative, profiles_relative
            ),
        )
        .unwrap();

        for name in ["source", "producer"] {
            git(&root.join(name), &["add", "."]);
            git(
                &root.join(name),
                &["commit", "-qm", "test: native lifecycle inputs"],
            );
        }
        let vendor_request = CargoVendorRequest {
            producer_dir: root.join("producer"),
            tools_dir: root.join("tools"),
            tools_tree: Some(git(&root.join("tools"), &["rev-parse", "HEAD^{tree}"])),
            cargo: which::which("cargo").unwrap(),
            cache_dir: cache.clone(),
        };
        if cold_vendor_generation {
            fetch_vendor_generation(&vendor_request, false, &CancellationToken::default()).unwrap();
        } else {
            let selection = select_vendor_generation(&vendor_request).unwrap();
            let generation = cache.join("cargo").join("v1").join(&selection.generation);
            fs::create_dir_all(&generation).unwrap();
            fs::rename(cache.join("cargo-vendor"), generation.join("cargo-vendor")).unwrap();
            fs::rename(
                cache.join("cargo-vendor-config.toml"),
                generation.join("cargo-vendor-config.toml"),
            )
            .unwrap();
            let vendor_tree_sha256 = measure_tree_content_cas(&generation.join("cargo-vendor"))
                .unwrap()
                .payload_digest_excluding(None);
            let template = fs::read(generation.join("cargo-vendor-config.toml")).unwrap();
            fs::write(
                generation.join("receipt.json"),
                serde_json::to_vec_pretty(&json!({
                    "schema": "aros-cargo-vendor-generation-v1",
                    "identity": selection.identity(),
                    "package_count": 1,
                    "vendor_tree_sha256": vendor_tree_sha256,
                    "configuration_template_sha256": sha256_bytes(&template),
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let mut recipe = json!({
            "schema": "aros-toolchain-recipe-v2",
            "source_commit": git(&root.join("source"), &["rev-parse", "HEAD"]),
            "source_tree": git(&root.join("source"), &["rev-parse", "HEAD^{tree}"]),
            "producer_commit": git(&root.join("producer"), &["rev-parse", "HEAD"]),
            "producer_tree": git(&root.join("producer"), &["rev-parse", "HEAD^{tree}"]),
            "tools_commit": git(&root.join("tools"), &["rev-parse", "HEAD"]),
            "tools_tree": git(&root.join("tools"), &["rev-parse", "HEAD^{tree}"]),
            "source_date_epoch": 0,
            "source_lock_sha256": sha256_bytes(&lock),
            "profiles_sha256": sha256_bytes(&profiles),
            "patches": if gnu {
                Vec::<serde_json::Value>::new()
            } else {
                vec![json!({"path": patch_path, "sha256": sha256_bytes(b"fixture patch\n")})]
            }
        });
        recipe["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&recipe).unwrap()));
        let recipe_path = root.join("recipe.json");
        fs::write(&recipe_path, serde_json::to_vec(&recipe).unwrap()).unwrap();

        let bridge = root.join("bridge");
        let llvm_bridge = r#"#!/bin/sh
set -eu
test "$1" = toolchain
test "$2" = __metamake-fetch
printf '%s\n' llvm-11.0.0.src.tar.xz >> "$AROS_TOOLCHAIN_FETCH_LEDGER"
exec /bin/bash "$AROS_TOOLCHAIN_FETCH_UPSTREAM" "$@"
"#;
        write_executable(&bridge, if gnu { GNU_FIXTURE_BRIDGE } else { llvm_bridge });
        Self {
            _temporary: temporary,
            root,
            recipe: recipe_path,
            bridge,
            preset: if gnu {
                "rv64-reference".to_owned()
            } else {
                "pc-x86_64".to_owned()
            },
            source_lock_relative: source_lock_relative.to_owned(),
            profiles_relative: profiles_relative.to_owned(),
        }
    }

    fn request(&self) -> BuildRequest {
        BuildRequest {
            preset: self.preset.clone(),
            recipe: self.recipe.clone(),
            source_dir: self.root.join("source"),
            producer_dir: self.root.join("producer"),
            tools_dir: self.root.join("tools"),
            work_dir: self.root.join("work"),
            output_dir: self.root.join("output"),
            cache_dir: self.root.join("cache"),
            compiler_cache: aros_cache::CompilerBackendChoice::Off,
            compiler_cache_dir: None,
            jobs: 1,
            timeout_seconds: 120,
            release_id: "native-fixture".into(),
            fetch_bridge: Some(self.bridge.clone()),
            resume_from: None,
        }
    }

    fn plan_request(&self) -> aros_toolchain::plan::PlanRequest {
        aros_toolchain::plan::PlanRequest {
            preset: self.preset.clone(),
            recipe: self.recipe.clone(),
            source_dir: self.root.join("source"),
            producer_dir: self.root.join("producer"),
            tools_dir: self.root.join("tools"),
            work_dir: Some(self.root.join("work")),
            output_dir: Some(self.root.join("output")),
            cache_dir: Some(self.root.join("cache")),
            jobs: Some(1),
            timeout_seconds: Some(120),
        }
    }

    fn install_v2_groups(&self, selected_gnu: bool) -> PathBuf {
        let producer = self.root.join("producer");
        let selected = GroupFixture {
            source_lock_path: self.source_lock_relative.clone(),
            profiles_path: self.profiles_relative.clone(),
            source_lock: fs::read(producer.join(&self.source_lock_relative)).unwrap(),
            profiles: fs::read(producer.join(&self.profiles_relative)).unwrap(),
        };
        let (gnu, llvm) = if selected_gnu {
            (
                selected,
                GroupFixture {
                    source_lock_path: "toolchains/v2-llvm.sources.json".into(),
                    profiles_path: "toolchains/v2-llvm-profiles.json".into(),
                    source_lock: v2_llvm_source_lock(),
                    profiles: v2_llvm_profiles(),
                },
            )
        } else {
            (
                GroupFixture {
                    source_lock_path: "toolchains/v2-gnu.sources.json".into(),
                    profiles_path: "toolchains/v2-gnu-profiles.json".into(),
                    source_lock: include_bytes!("fixtures/gnu-source-lock-v3.json").to_vec(),
                    profiles: v2_gnu_profiles(),
                },
                selected,
            )
        };

        for group in [&gnu, &llvm] {
            fs::write(producer.join(&group.source_lock_path), &group.source_lock).unwrap();
            fs::write(producer.join(&group.profiles_path), &group.profiles).unwrap();
        }

        let contract =
            fs::read(self.root.join("tools/contracts/toolchain-producer-v1.toml")).unwrap();
        let recipe: serde_json::Value =
            serde_json::from_slice(&fs::read(&self.recipe).unwrap()).unwrap();
        let declaration = format!(
            "schema_version = 2\ncontract_id = \"aros-toolchain-producer-v1\"\ncontract_path = \"contracts/toolchain-producer-v1.toml\"\ncontract_sha256 = \"{}\"\ntools_commit = \"{}\"\n\n[[groups]]\nid = \"gnu\"\nsource_lock = \"{}\"\nsource_lock_sha256 = \"{}\"\nprofiles = \"{}\"\nprofiles_sha256 = \"{}\"\n\n[[groups]]\nid = \"llvm\"\nsource_lock = \"{}\"\nsource_lock_sha256 = \"{}\"\nprofiles = \"{}\"\nprofiles_sha256 = \"{}\"\n",
            sha256_bytes(&contract),
            recipe["tools_commit"].as_str().unwrap(),
            gnu.source_lock_path,
            sha256_bytes(&gnu.source_lock),
            gnu.profiles_path,
            sha256_bytes(&gnu.profiles),
            llvm.source_lock_path,
            sha256_bytes(&llvm.source_lock),
            llvm.profiles_path,
            sha256_bytes(&llvm.profiles),
        );
        fs::write(
            producer.join("toolchains/producer-executor-v1.toml"),
            declaration,
        )
        .unwrap();

        let selected_lock_sha256 = if selected_gnu {
            sha256_bytes(&gnu.source_lock)
        } else {
            sha256_bytes(&llvm.source_lock)
        };
        let selected_profiles_sha256 = if selected_gnu {
            sha256_bytes(&gnu.profiles)
        } else {
            sha256_bytes(&llvm.profiles)
        };
        let alternate_lock_sha256 = if selected_gnu {
            sha256_bytes(&llvm.source_lock)
        } else {
            sha256_bytes(&gnu.source_lock)
        };
        let alternate_profiles_sha256 = if selected_gnu {
            sha256_bytes(&llvm.profiles)
        } else {
            sha256_bytes(&gnu.profiles)
        };
        assert_ne!(selected_lock_sha256, alternate_lock_sha256);
        assert_ne!(selected_profiles_sha256, alternate_profiles_sha256);

        self.commit_producer_update("test: add native executor v2 groups");
        let mut recipe: serde_json::Value =
            serde_json::from_slice(&fs::read(&self.recipe).unwrap()).unwrap();
        assert_eq!(recipe["source_lock_sha256"], selected_lock_sha256.as_str());
        assert_eq!(recipe["profiles_sha256"], selected_profiles_sha256.as_str());
        let selected_recipe_sha256 = recipe["recipe_sha256"].as_str().unwrap().to_owned();
        recipe["source_lock_sha256"] = serde_json::json!(alternate_lock_sha256);
        recipe["profiles_sha256"] = serde_json::json!(alternate_profiles_sha256);
        recipe.as_object_mut().unwrap().remove("recipe_sha256");
        let alternate_recipe_sha256 = sha256_bytes(&canonical::bytes(&recipe).unwrap());
        assert_ne!(selected_recipe_sha256, alternate_recipe_sha256.as_str());

        if selected_gnu {
            self.root.join("producer/toolchains/v2-llvm-profiles.json")
        } else {
            self.root.join("producer/toolchains/v2-gnu-profiles.json")
        }
    }

    fn commit_producer_update(&self, message: &str) {
        let producer = self.root.join("producer");
        git(&producer, &["add", "."]);
        git(&producer, &["commit", "-qm", message]);
        let mut recipe: serde_json::Value =
            serde_json::from_slice(&fs::read(&self.recipe).unwrap()).unwrap();
        recipe["producer_commit"] = serde_json::json!(git(&producer, &["rev-parse", "HEAD"]));
        recipe["producer_tree"] = serde_json::json!(git(&producer, &["rev-parse", "HEAD^{tree}"]));
        recipe.as_object_mut().unwrap().remove("recipe_sha256");
        recipe["recipe_sha256"] =
            serde_json::json!(sha256_bytes(&canonical::bytes(&recipe).unwrap()));
        fs::write(&self.recipe, serde_json::to_vec(&recipe).unwrap()).unwrap();
    }

    fn replace_source_configure(&self, contents: &str) {
        let source = self.root.join("source");
        write_executable(&source.join("configure"), contents);
        git(&source, &["add", "configure"]);
        git(
            &source,
            &["commit", "-qm", "test: alter native configure phase"],
        );
        self.refresh_recipe_after_input_change();
    }

    fn replace_gnu_profiles_with_llvm(&self) {
        let producer = self.root.join("producer");
        let profiles = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v1", "upstream_commit": "4".repeat(40),
            "profiles": [{
                "name": "pc-x86_64", "configure_target": "pc-x86_64", "upstream_output_target": "pc-x86_64",
                "target_triple": "x86_64-unknown-aros", "cpu": "x86_64", "platform": "pc", "float_abi": "",
                "capabilities": ["c", "cxx", "standalone-collector"]
            }]
        }))
        .unwrap();
        fs::write(producer.join(&self.profiles_relative), profiles).unwrap();
        git(&producer, &["add", &self.profiles_relative]);
        git(
            &producer,
            &[
                "commit",
                "-qm",
                "test: mismatch GNU source and profile families",
            ],
        );
        self.refresh_recipe_after_input_change();
    }

    fn refresh_recipe_after_input_change(&self) {
        let source = self.root.join("source");
        let producer = self.root.join("producer");
        let mut recipe: serde_json::Value =
            serde_json::from_slice(&fs::read(&self.recipe).unwrap()).unwrap();
        recipe["source_commit"] = json!(git(&source, &["rev-parse", "HEAD"]));
        recipe["source_tree"] = json!(git(&source, &["rev-parse", "HEAD^{tree}"]));
        recipe["producer_commit"] = json!(git(&producer, &["rev-parse", "HEAD"]));
        recipe["producer_tree"] = json!(git(&producer, &["rev-parse", "HEAD^{tree}"]));
        recipe["source_lock_sha256"] = json!(sha256_bytes(
            &fs::read(producer.join(&self.source_lock_relative)).unwrap()
        ));
        recipe["profiles_sha256"] = json!(sha256_bytes(
            &fs::read(producer.join(&self.profiles_relative)).unwrap()
        ));
        recipe.as_object_mut().unwrap().remove("recipe_sha256");
        recipe["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&recipe).unwrap()));
        fs::write(&self.recipe, serde_json::to_vec(&recipe).unwrap()).unwrap();
    }
}

#[test]
fn native_compiler_cache_refuses_unprepared_root_before_reservation() {
    let fixture = Fixture::new();
    let mut request = fixture.request();
    request.compiler_cache = aros_cache::CompilerBackendChoice::Ccache;
    request.compiler_cache_dir = Some(fixture.root.join("unprepared-compiler-cache"));
    let error = executor::run(&request, &CancellationToken::default()).unwrap_err();
    assert!(error.to_string().contains("compiler-cache selection"));
    assert!(!request.work_dir.exists());
    assert!(!request.output_dir.exists());
}

#[test]
fn native_compiler_cache_off_refuses_a_namespace_before_reservation() {
    let fixture = Fixture::new();
    let mut request = fixture.request();
    request.compiler_cache_dir = Some(fixture.root.join("compiler-cache"));
    assert!(executor::run(&request, &CancellationToken::default()).is_err());
    assert!(!request.work_dir.exists());
    assert!(!request.output_dir.exists());
}

#[test]
fn native_compiler_cache_is_bound_to_configure_and_compiler_receipts() {
    real_native_compiler_cache(aros_cache::CompilerBackend::Ccache);
}

#[test]
fn native_compiler_cache_sccache_has_real_hits_and_safe_resume() {
    real_native_compiler_cache(aros_cache::CompilerBackend::Sccache);
}

fn real_native_compiler_cache(backend: aros_cache::CompilerBackend) {
    if which::which(backend.program()).is_err() {
        eprintln!(
            "real {} integration requires its executable on PATH",
            backend.program()
        );
        return;
    }
    let fixture = Fixture::new_short();
    // Exercise a real host compilation through the source-owned configure
    // process, not only an argv recorder. The remaining producer is synthetic.
    let configure = LLVM_FIXTURE_CONFIGURE.replace(
        "printf '%s\\n' \"$@\" > configure.args",
        "test -x \"$CC\"\ntest -x \"$CXX\"\nprintf 'int cache_probe(void) { return 42; }\\n' > probe.c\n\"$CC\" -c probe.c -o probe.o\n\"$CC\" -c probe.c -o probe.o\nprintf '%s\\n' \"$CC\" > configure-cc\nprintf '%s\\n' \"$@\" > configure.args",
    );
    // Source-owned build rules retain the selected C++ wrapper and invoke it
    // in the compiler phase, after the configure process has exited.
    fixture.replace_source_configure(&format!("{configure}\ncommand cp probe.c probe.cpp\nprintf '\\t@%s -c probe.cpp -o probe-cxx.o\\n\\t@%s -c probe.cpp -o probe-cxx.o\\n' \"$CXX\" \"$CXX\" >> Makefile\n"));
    // macOS's default temporary path is too long for the managed sccache
    // Unix socket. This explicit short, private root also models --dir.
    let cache_owner = tempfile::tempdir_in("/tmp").unwrap();
    let cache = cache_owner.path().join("cache");
    aros_cache::prepare_managed_compiler_cache(backend, cache.clone()).unwrap();
    let managed = aros_cache::load_managed_compiler_cache(backend, cache.clone()).unwrap();
    let _stop_server = StopTestSccache {
        executable: (backend == aros_cache::CompilerBackend::Sccache)
            .then(|| managed.executable().unwrap()),
        environment: aros_cache::compiler_cache_environment(&managed).unwrap(),
    };
    let mut request = fixture.request();
    request.compiler_cache = match backend {
        aros_cache::CompilerBackend::Ccache => aros_cache::CompilerBackendChoice::Ccache,
        aros_cache::CompilerBackend::Sccache => aros_cache::CompilerBackendChoice::Sccache,
    };
    request.compiler_cache_dir = Some(cache.clone());
    executor::run(&request, &CancellationToken::default()).unwrap_or_else(|error| {
        let logs = ["configure", "compiler"]
            .into_iter()
            .map(|phase| {
                let path = fixture
                    .root
                    .join(format!("work/native-lifecycle/logs/{phase}.stderr.log"));
                format!("{phase}: {}", fs::read_to_string(path).unwrap_or_default())
            })
            .collect::<Vec<_>>()
            .join("\n");
        panic!("{error}\n{logs}");
    });
    let build = fixture.root.join("work/native-lifecycle/build");
    let cc = fs::read_to_string(build.join("configure-cc")).unwrap();
    assert!(cc.contains("compiler-launchers/cc"));
    assert!(build.join("probe.o").is_file());
    assert!(build.join("probe-cxx.o").is_file());
    let root = aros_cache::load_managed_compiler_cache(backend, cache).unwrap();
    let mut stats = Command::new(backend.program());
    aros_cache::compiler_cache_environment(&root)
        .unwrap()
        .apply_to(&mut stats);
    let stats = match backend {
        aros_cache::CompilerBackend::Ccache => stats.arg("--print-stats"),
        aros_cache::CompilerBackend::Sccache => stats.args(["--show-stats", "--stats-format=json"]),
    }
    .output()
    .unwrap();
    assert!(stats.status.success());
    let stats = String::from_utf8(stats.stdout).unwrap();
    if backend == aros_cache::CompilerBackend::Ccache {
        assert!(
            stats.lines().any(|line| line
                .split_whitespace()
                .next()
                .is_some_and(|name| matches!(name, "direct_cache_hit" | "preprocessed_cache_hit"))
                && line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|value| value.parse::<u64>().ok())
                    .is_some_and(|value| value > 0)),
            "{stats}"
        );
    } else {
        let stats: serde_json::Value = serde_json::from_str(&stats).unwrap();
        assert!(
            stats["stats"]["cache_hits"]["counts"]
                .as_object()
                .unwrap()
                .values()
                .any(|value| value.as_u64().is_some_and(|hits| hits > 0)),
            "{stats}"
        );
        let mut stop = Command::new(backend.program());
        aros_cache::compiler_cache_environment(&root)
            .unwrap()
            .apply_to(&mut stop);
        assert!(stop.arg("--stop-server").output().unwrap().status.success());
    }
    let lifecycle = fixture.root.join("work/native-lifecycle");
    let staging = fixture.root.join("output/.aros-native-toolchain-stage");
    fs::rename(fixture.root.join("output/toolchain"), &staging).unwrap();
    for phase in ["collector", "publish", "finished-candidate"] {
        fs::remove_file(lifecycle.join(format!("receipts/{phase}.json"))).unwrap();
    }
    for name in ["aros-collect", "collect-aros", "collect-aros32"] {
        fs::remove_file(staging.join("bin").join(name)).unwrap();
    }
    fs::write(staging.join("bin/llvm-config"), b"producer-only").unwrap();
    request.resume_from = Some(ResumePhase::Compiler);
    let mut changed = request.clone();
    changed.compiler_cache = aros_cache::CompilerBackendChoice::Off;
    changed.compiler_cache_dir = None;
    assert!(executor::run(&changed, &CancellationToken::default())
        .unwrap_err()
        .to_string()
        .contains("preflight receipt"));
    let launcher = lifecycle.join("compiler-launchers/cc");
    let original = fs::read(&launcher).unwrap();
    fs::write(&launcher, "#!/bin/sh\nexit 1\n").unwrap();
    assert!(executor::run(&request, &CancellationToken::default())
        .unwrap_err()
        .to_string()
        .contains("launcher changed"));
    fs::write(&launcher, original).unwrap();
    executor::run(&request, &CancellationToken::default()).unwrap();
    assert!(fixture
        .root
        .join("output/toolchain/bin/aros-collect")
        .is_file());
}

struct StopTestSccache {
    executable: Option<PathBuf>,
    environment: aros_cache::CompilerCacheEnvironment,
}

impl Drop for StopTestSccache {
    fn drop(&mut self) {
        if let Some(executable) = &self.executable {
            let mut command = Command::new(executable);
            self.environment.apply_to(&mut command);
            let _ = command.arg("--stop-server").output();
        }
    }
}

fn assert_finished_payload(fixture: &Fixture, result: &BuildResult) {
    let request = fixture.request();
    let recipe = aros_toolchain::Recipe::parse(&fs::read(&request.recipe).unwrap()).unwrap();
    let lock = aros_toolchain::source_lock::SourceLock::parse(
        &fs::read(request.producer_dir.join(&fixture.source_lock_relative)).unwrap(),
    )
    .unwrap();
    let profiles =
        Profiles::parse(&fs::read(request.producer_dir.join(&fixture.profiles_relative)).unwrap())
            .unwrap();
    // Select the identity from a fresh read-only inspection, never the new
    // finished receipt. These fixture drivers prove orchestration only.
    let plan = aros_toolchain::plan::inspect(&fixture.plan_request()).unwrap();
    assert_eq!(
        serde_json::to_value(&plan.identity).unwrap(),
        serde_json::to_value(&result.identity).unwrap()
    );
    let digest = |name: &str| {
        let entries = result
            .evidence
            .iter()
            .filter(|entry| entry.check == name)
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "passed");
        entries[0].report_sha256.as_ref().unwrap().clone()
    };
    let phases = [
        "preflight",
        "environment",
        "configure",
        "compiler",
        "collector",
        "publish",
    ]
    .map(digest);
    let finished = digest("finished-candidate");
    let proof = readback_finished_candidate(&FinishedCandidateRequest {
        work_dir: &request.work_dir,
        output_dir: &request.output_dir,
        recipe: &recipe,
        source_lock: &lock,
        profile: profiles.select(&fixture.preset).unwrap(),
        identity: &plan.identity,
        phase_receipt_digests: &phases,
        candidate_receipt_digest: &finished,
    })
    .unwrap();
    assert_eq!(proof.receipt_sha256(), &finished);
    assert!(proof.entry_count() > result.outputs.len() as u64);
    assert_eq!(
        proof.payload_sha256(),
        &measure_tree_content_cas(&request.output_dir.join("toolchain"))
            .unwrap()
            .payload_digest_excluding(None)
    );
    // Select the exact serialized CLI result independently of the retained
    // receipts. This fixture proves the result adapter, not authenticated execution.
    let selected = tempfile::tempdir().unwrap();
    let result_path = selected.path().join("build.json");
    let bytes = serde_json::to_vec_pretty(result).unwrap();
    fs::write(&result_path, &bytes).unwrap();
    let result_digest = sha256_bytes(&bytes);
    let result_request = FinishedBuildResultRequest {
        work_dir: &request.work_dir,
        output_dir: &request.output_dir,
        recipe: &recipe,
        source_lock: &lock,
        profile: profiles.select(&fixture.preset).unwrap(),
        host: result.identity.host,
        build_result: &result_path,
        build_result_sha256: &result_digest,
    };
    let result_proof = readback_finished_build_result(&result_request).unwrap();
    assert_eq!(result_proof.receipt_sha256(), proof.receipt_sha256());
    assert_eq!(result_proof.payload_sha256(), proof.payload_sha256());
    let mut changed = bytes;
    changed.push(b'\n');
    fs::write(&result_path, &changed).unwrap();
    assert!(readback_finished_build_result(&result_request).is_err());
}

#[test]
fn native_lifecycle_runs_configure_compiler_and_collector_with_receipt_chain() {
    let fixture = Fixture::new();
    let result =
        executor::run(&fixture.request(), &CancellationToken::default()).unwrap_or_else(|error| {
            let log_root = &fixture.root;
            let logs = ["configure", "compiler", "collector"]
                .into_iter()
                .flat_map(|phase| {
                    ["stdout", "stderr"].into_iter().map(move |stream| {
                        let path = log_root
                            .join(format!("work/native-lifecycle/logs/{phase}.{stream}.log"));
                        let content = fs::read_to_string(path)
                            .unwrap_or_else(|_| format!("<no retained {stream} log>"));
                        format!("{phase} {stream}:\n{content}")
                    })
                })
                .collect::<Vec<_>>()
                .join("\n");
            panic!("{error}\n{logs}");
        });
    assert_eq!(result.qualification, "local-only");
    assert_eq!(result.commit_state, "committed");
    assert_finished_payload(&fixture, &result);
    assert_eq!(result.outputs.len(), 1);
    assert_eq!(result.outputs[0].path, "toolchain/bin/aros-collect");
    let public_result = serde_json::to_value(&result).unwrap();
    assert!(public_result.get("environment").is_none());
    let prefix = fixture.root.join("output/toolchain");
    assert!(prefix.join("bin/clang").is_file());
    assert!(prefix.join("bin/aros-collect").is_file());
    assert!(prefix.join("bin/collect-aros").is_symlink());
    assert!(prefix.join("bin/collect-aros32").is_symlink());
    assert!(!prefix.join("bin/llvm-config").exists());
    assert!(!prefix.join("lib/cmake/llvm").exists());
    assert!(!fixture
        .root
        .join("output/.aros-native-toolchain-stage")
        .exists());
    let build = fixture.root.join("work/native-lifecycle/build");
    let configure_args = fs::read_to_string(build.join("configure.args")).unwrap();
    for expected in [
        "--target=pc-x86_64",
        "--with-toolchain=llvm",
        "--with-llvm-version=11.0.0",
        "--enable-toolchain-release",
        &format!(
            "--with-portssources={}",
            fixture.root.join("cache").display()
        ),
        &format!(
            "--with-aros-toolchain-install={}",
            fixture
                .root
                .join("output/.aros-native-toolchain-stage")
                .display()
        ),
    ] {
        assert!(configure_args.contains(expected), "missing {expected}");
    }
    assert_eq!(
        fs::read_to_string(build.join("configure-cmake-jobs"))
            .unwrap()
            .trim(),
        "1"
    );

    assert_eq!(
        fs::read_to_string(build.join("native-cmake-jobs"))
            .unwrap()
            .trim(),
        "1"
    );
    let collector_jobs = Command::new(prefix.join("bin/aros-collect"))
        .output()
        .unwrap();
    assert!(collector_jobs.status.success());
    assert_eq!(collector_jobs.stdout, b"1");
    let mut previous: Option<String> = None;
    for phase in [
        "preflight",
        "environment",
        "configure",
        "compiler",
        "collector",
        "publish",
    ] {
        let receipt: serde_json::Value = serde_json::from_slice(
            &fs::read(
                fixture
                    .root
                    .join(format!("work/native-lifecycle/receipts/{phase}.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            receipt["previous_receipt_sha256"].as_str(),
            previous.as_deref(),
            "{phase} receipt must bind its exact predecessor"
        );
        let expected = receipt["receipt_sha256"].as_str().unwrap().to_owned();
        let mut unsigned = receipt;
        unsigned.as_object_mut().unwrap().remove("receipt_sha256");
        assert_eq!(
            sha256_bytes(&canonical::bytes(&unsigned).unwrap()).as_str(),
            expected
        );
        previous = Some(expected);
    }
    let usage = fs::read_to_string(
        fixture
            .root
            .join("work/native-lifecycle/verified-source-usage.log"),
    )
    .unwrap();
    assert_eq!(usage, "llvm-11.0.0.src.tar.xz\n");
}

#[test]
fn native_gnu_lifecycle_builds_locked_rv64_tools_and_binds_collector_layout() {
    let fixture = Fixture::new_gnu();
    let result =
        executor::run(&fixture.request(), &CancellationToken::default()).unwrap_or_else(|error| {
            let log_root = &fixture.root;
            let logs = ["configure", "compiler", "collector"]
                .into_iter()
                .flat_map(|phase| {
                    ["stdout", "stderr"].into_iter().map(move |stream| {
                        let path = log_root
                            .join(format!("work/native-lifecycle/logs/{phase}.{stream}.log"));
                        let content = fs::read_to_string(path)
                            .unwrap_or_else(|_| format!("<no retained {stream} log>"));
                        format!("{phase} {stream}:\n{content}")
                    })
                })
                .collect::<Vec<_>>()
                .join("\n");
            panic!("{error}\n{logs}");
        });

    assert_eq!(result.qualification, "local-only");
    assert_eq!(result.commit_state, "committed");
    assert_finished_payload(&fixture, &result);
    assert_eq!(
        result
            .outputs
            .iter()
            .map(|output| output.path.as_str())
            .collect::<Vec<_>>(),
        [
            "toolchain/riscv64-aros/bin/collect-aros",
            "toolchain/riscv64-aros-collect-aros",
            "toolchain/riscv64-aros/bin/aros-collector-tools.json",
            "toolchain/aros-collector-tools.json",
            "toolchain/toolchain-tools.json",
        ]
    );

    let prefix = fixture.root.join("output/toolchain");
    let build = fixture.root.join("work/native-lifecycle/build");
    let configure_args = fs::read_to_string(build.join("configure.args")).unwrap();
    for expected in [
        "--target=opensbi-riscv64",
        "--with-toolchain=gnu",
        "--with-gcc-version=16.2.0",
        "--with-binutils-version=2.47",
        "--enable-toolchain-release",
        &format!(
            "--with-portssources={}",
            fixture.root.join("cache").display()
        ),
    ] {
        assert!(configure_args.contains(expected), "missing {expected}");
    }
    assert!(!configure_args.contains("--with-toolchain=llvm"));
    assert_eq!(
        fs::read_to_string(build.join("configure-cc"))
            .unwrap()
            .trim(),
        which::which("cc").unwrap().to_str().unwrap()
    );
    assert_eq!(
        fs::read_to_string(build.join("configure-cxx"))
            .unwrap()
            .trim(),
        which::which("c++").unwrap().to_str().unwrap()
    );
    assert_eq!(
        fs::read_to_string(build.join("configure-cmake-jobs"))
            .unwrap()
            .trim(),
        "1"
    );

    assert_eq!(
        fs::read_to_string(build.join("configure-recursive-make"))
            .unwrap()
            .trim(),
        which::which("gmake")
            .or_else(|_| which::which("make"))
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert!(fs::read_to_string(build.join("recursive-make-version"))
        .unwrap()
        .trim()
        .starts_with('4'));

    // These LLVM-shaped producer markers prove the GNU collector branch does
    // not apply LLVM's producer-only cleanup.
    assert_eq!(
        fs::read(prefix.join("bin/llvm-config")).unwrap(),
        b"producer-only"
    );
    assert_eq!(
        fs::read(prefix.join("lib/cmake/llvm/producer.marker")).unwrap(),
        b"retained-by-gnu"
    );
    assert!(!fixture
        .root
        .join("output/.aros-native-toolchain-stage")
        .exists());

    let tuple_manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(prefix.join("riscv64-aros/bin/aros-collector-tools.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        tuple_manifest,
        json!({
            "schema": "aros-collector-tools-v1", "family": "gnu",
            "invocation": "collect-aros", "linker": "ld", "strip": "strip",
            "emulation": "riscv64elf_aros", "driver_emulation": "elf64lriscv"
        })
    );
    let root_manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(prefix.join("aros-collector-tools.json")).unwrap())
            .unwrap();
    assert_eq!(
        root_manifest,
        json!({
            "schema": "aros-collector-tools-v1", "family": "gnu",
            "invocation": "riscv64-aros-collect-aros", "linker": "riscv64-aros-ld",
            "strip": "riscv64-aros-strip", "emulation": "riscv64elf_aros",
            "driver_emulation": "elf64lriscv"
        })
    );
    let layout = ToolchainToolLayout::load(&prefix).unwrap();
    let profiles = Profiles::parse(
        &fs::read(
            fixture
                .root
                .join("producer")
                .join(&fixture.profiles_relative),
        )
        .unwrap(),
    )
    .unwrap();
    let profile = profiles.select("rv64-reference").unwrap();
    let identity = ArosCompilerIdentity::Gnu {
        gcc_version: "16.2.0".to_owned(),
        binutils_version: "2.47".to_owned(),
        target: profile.target().unwrap().clone(),
    };
    layout.validate_binding(&identity, "riscv64-aros").unwrap();
    assert_eq!(
        layout.tools().entries().collect::<Vec<_>>(),
        [
            ("c", "riscv64-aros-gcc"),
            ("cxx", "riscv64-aros-g++"),
            ("assembler", "riscv64-aros-as"),
            ("linker", "riscv64-aros-ld"),
            ("archive", "riscv64-aros-ar"),
            ("ranlib", "riscv64-aros-ranlib"),
            ("strip", "riscv64-aros-strip"),
            ("collector", "riscv64-aros/bin/collect-aros"),
            ("nm", "riscv64-aros-nm"),
            ("objcopy", "riscv64-aros-objcopy"),
            ("objdump", "riscv64-aros-objdump"),
        ]
    );
    assert!(layout.has_objdump_role());
    assert_eq!(layout.resolve_tools(&prefix).unwrap().len(), 11);

    let usage = fs::read_to_string(
        fixture
            .root
            .join("work/native-lifecycle/verified-source-usage.log"),
    )
    .unwrap();
    assert_eq!(usage, "gcc-16.2.0.tar.xz\nbinutils-2.47.tar.bz2\n");
    let mut previous: Option<String> = None;
    for phase in [
        "preflight",
        "environment",
        "configure",
        "compiler",
        "collector",
        "publish",
    ] {
        let receipt: serde_json::Value = serde_json::from_slice(
            &fs::read(
                fixture
                    .root
                    .join(format!("work/native-lifecycle/receipts/{phase}.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            receipt["previous_receipt_sha256"].as_str(),
            previous.as_deref(),
            "{phase} receipt must bind its exact predecessor"
        );
        let expected = receipt["receipt_sha256"].as_str().unwrap().to_owned();
        let mut unsigned = receipt;
        unsigned.as_object_mut().unwrap().remove("receipt_sha256");
        assert_eq!(
            sha256_bytes(&canonical::bytes(&unsigned).unwrap()).as_str(),
            expected
        );
        previous = Some(expected);
    }
}

#[test]
fn native_gnu_lifecycle_rejects_wrong_family_inputs_before_publication() {
    let fixture = Fixture::new_gnu();
    fixture.replace_gnu_profiles_with_llvm();

    let error = executor::run(&fixture.request(), &CancellationToken::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("native source lock and profiles select different compiler families"),
        "{error}"
    );
    assert!(!fixture.root.join("output/toolchain").exists());
    assert!(!fixture
        .root
        .join("output/.aros-native-toolchain-stage")
        .exists());
    assert!(!fixture
        .root
        .join("work/native-lifecycle/build/configure.args")
        .exists());
}

#[test]
fn native_gnu_configure_failure_never_publishes_a_final_prefix() {
    let fixture = Fixture::new_gnu();
    fixture.replace_source_configure(
        "#!/bin/sh\nprintf 'fixture GNU configure failure\\n' >&2\nexit 23\n",
    );

    let error = executor::run(&fixture.request(), &CancellationToken::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("configure phase exited unsuccessfully"),
        "{error}"
    );
    assert!(!fixture.root.join("output/toolchain").exists());
    assert!(!fixture
        .root
        .join("work/native-lifecycle/receipts/configure.json")
        .exists());
    assert!(!fixture
        .root
        .join("work/native-lifecycle/receipts/publish.json")
        .exists());
}

#[test]
fn native_gnu_collector_resume_reinstalls_contract_without_rebuilding_compiler() {
    let fixture = Fixture::new_gnu();
    executor::run(&fixture.request(), &CancellationToken::default()).unwrap();

    let lifecycle = fixture.root.join("work/native-lifecycle");
    let published = fixture.root.join("output/toolchain");
    let staging = fixture.root.join("output/.aros-native-toolchain-stage");
    fs::rename(&published, &staging).unwrap();
    fs::remove_file(lifecycle.join("receipts/publish.json")).unwrap();
    fs::remove_file(lifecycle.join("receipts/collector.json")).unwrap();
    fs::remove_file(lifecycle.join("receipts/finished-candidate.json")).unwrap();

    // Reconstruct the exact pre-collector compiler outputs. The two legacy
    // collector paths are compiler-owned destinations which the GNU collector
    // installer replaces; the three new contract files did not exist yet.
    let tuple_directory = staging.join("riscv64-aros/bin");
    write_executable(&tuple_directory.join("collect-aros"), "legacy-collector");
    write_executable(
        &staging.join("riscv64-aros-collect-aros"),
        "legacy-collector",
    );
    for path in [
        tuple_directory.join("aros-collector-tools.json"),
        staging.join("aros-collector-tools.json"),
        staging.join("toolchain-tools.json"),
    ] {
        fs::remove_file(path).unwrap();
    }

    let mut request = fixture.request();
    request.resume_from = Some(ResumePhase::Compiler);
    let result = executor::run(&request, &CancellationToken::default()).unwrap();
    assert_eq!(result.commit_state, "committed");
    assert_eq!(result.outputs.len(), 5);
    assert!(fixture
        .root
        .join("output/toolchain/toolchain-tools.json")
        .is_file());
    assert!(!staging.exists());
    assert!(lifecycle.join("receipts/collector.json").is_file());
    assert!(lifecycle
        .join("logs/collector-resume-1.stdout.log")
        .is_file());
    assert!(lifecycle.join("rust-target-resume-1").is_dir());
}

fn exercise_native_v2_group_lifecycle(gnu: bool) {
    // These are synthetic orchestration fixtures. The GNU lane exercises the
    // selected profile and collector path, not RV64 compiler qualification.
    let (fixture, _) = Fixture::new_v2(gnu);
    let recipe: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture.recipe).unwrap()).unwrap();
    let expected_recipe_sha256 = recipe["recipe_sha256"].as_str().unwrap();
    let plan = aros_toolchain::plan::inspect(&fixture.plan_request()).unwrap();
    assert_eq!(plan.identity.recipe_sha256.as_str(), expected_recipe_sha256);
    assert_eq!(plan.identity.target_profile, fixture.preset);
    assert_eq!(
        plan.identity.executor.contract_id,
        Some("aros-toolchain-producer-v1")
    );

    let result = executor::run(&fixture.request(), &CancellationToken::default()).unwrap();
    assert_eq!(result.qualification, "local-only");
    assert_eq!(result.commit_state, "committed");
    assert_eq!(
        result.identity.recipe_sha256.as_str(),
        expected_recipe_sha256
    );
    assert_eq!(result.identity.target_profile, fixture.preset);
    let configure_args = fs::read_to_string(
        fixture
            .root
            .join("work/native-lifecycle/build/configure.args"),
    )
    .unwrap();
    for expected in if gnu {
        ["--with-toolchain=gnu", "--target=opensbi-riscv64"]
    } else {
        ["--with-toolchain=llvm", "--target=pc-x86_64"]
    } {
        assert!(configure_args.contains(expected), "missing {expected}");
    }

    let lifecycle = fixture.root.join("work/native-lifecycle");
    let published = fixture.root.join("output/toolchain");
    let staging = fixture.root.join("output/.aros-native-toolchain-stage");
    let compiler_receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(lifecycle.join("receipts/compiler.json")).unwrap())
            .unwrap();
    assert_eq!(
        compiler_receipt["identity"]["recipe_sha256"],
        expected_recipe_sha256
    );
    assert_eq!(
        compiler_receipt["identity"]["target_profile"],
        fixture.preset
    );

    fs::rename(&published, &staging).unwrap();
    fs::remove_file(lifecycle.join("receipts/publish.json")).unwrap();
    fs::remove_file(lifecycle.join("receipts/collector.json")).unwrap();
    fs::remove_file(lifecycle.join("receipts/finished-candidate.json")).unwrap();
    if gnu {
        let tuple_directory = staging.join("riscv64-aros/bin");
        write_executable(&tuple_directory.join("collect-aros"), "legacy-collector");
        write_executable(
            &staging.join("riscv64-aros-collect-aros"),
            "legacy-collector",
        );
        for path in [
            tuple_directory.join("aros-collector-tools.json"),
            staging.join("aros-collector-tools.json"),
            staging.join("toolchain-tools.json"),
        ] {
            fs::remove_file(path).unwrap();
        }
    } else {
        let prefix = staging.join("bin");
        for name in ["aros-collect", "collect-aros", "collect-aros32"] {
            fs::remove_file(prefix.join(name)).unwrap();
        }
        fs::write(prefix.join("llvm-config"), b"producer-only").unwrap();
    }

    let mut request = fixture.request();
    request.resume_from = Some(ResumePhase::Compiler);
    let resumed = executor::run(&request, &CancellationToken::default()).unwrap();
    assert_eq!(resumed.commit_state, "committed");
    assert_eq!(
        resumed.identity.recipe_sha256.as_str(),
        expected_recipe_sha256
    );
    assert_eq!(resumed.identity.target_profile, fixture.preset);
    let collector_receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(lifecycle.join("receipts/collector.json")).unwrap())
            .unwrap();
    assert_eq!(
        collector_receipt["identity"]["recipe_sha256"],
        expected_recipe_sha256
    );
    assert_eq!(
        collector_receipt["identity"]["target_profile"],
        fixture.preset
    );
    assert!(!staging.exists());
    assert!(lifecycle
        .join("logs/collector-resume-1.stdout.log")
        .is_file());
}

#[test]
fn native_lifecycle_v2_llvm_group_plans_builds_and_resumes() {
    exercise_native_v2_group_lifecycle(false);
}

#[test]
fn native_lifecycle_v2_gnu_group_plans_builds_and_resumes() {
    exercise_native_v2_group_lifecycle(true);
}

#[test]
fn native_lifecycle_v2_plan_rejects_changed_unselected_group_after_commit() {
    let (fixture, unselected_profiles) = Fixture::new_v2(false);
    let mut changed = fs::read(&unselected_profiles).unwrap();
    changed.push(b'\n');
    fs::write(&unselected_profiles, changed).unwrap();
    fixture.commit_producer_update("test: change unselected native input group");

    let request = fixture.plan_request();
    let error = aros_toolchain::plan::inspect(&request).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("native executor input-group bytes differ from their declared digests"),
        "{error}"
    );
    assert!(!request.work_dir.unwrap().exists());
    assert!(!request.output_dir.unwrap().exists());
    assert!(!fixture
        .root
        .join("work/native-lifecycle/build/configure.args")
        .exists());
}

#[test]
fn native_lifecycle_consumes_a_cold_vendor_generation_offline() {
    if std::env::var_os("AROS_CARGO_VENDOR_CREDENTIAL_TEST_CHILD").is_none() {
        let temporary = tempfile::tempdir().unwrap();
        let bin = temporary.path().join("bin");
        let poison = temporary.path().join("poisoned-cargo-home");
        let trace = temporary.path().join("cargo-trace");
        fs::create_dir(&bin).unwrap();
        fs::create_dir(&poison).unwrap();
        fs::write(
            poison.join("config.toml"),
            "[source.crates-io]\nreplace-with = \"poisoned\"\n[source.poisoned]\ndirectory = \"/definitely-not-a-cargo-registry\"\n",
        )
        .unwrap();
        let real_cargo = which::which("cargo").unwrap();
        write_executable(
            &bin.join("cargo"),
            &cold_vendor_cargo_wrapper(&real_cargo, &poison, &trace),
        );
        let inherited_path = std::env::var_os("PATH").unwrap();
        let mut child_path = bin.into_os_string();
        child_path.push(":");
        child_path.push(inherited_path);
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_lifecycle_consumes_a_cold_vendor_generation_offline",
                "--nocapture",
            ])
            .env("AROS_CARGO_VENDOR_CREDENTIAL_TEST_CHILD", "1")
            .env("PATH", child_path)
            .env("CARGO_HOME", &poison)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "cold vendor lifecycle child failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let trace = fs::read_to_string(trace).unwrap();
        assert!(trace.lines().any(|line| line.starts_with("vendor ")));
        assert!(trace.lines().any(|line| line.starts_with("build ")));
        assert!(!trace.contains("poisoned-cargo-home"));
        return;
    }

    let fixture = Fixture::new_with_cold_vendor_generation();
    let result = executor::run(&fixture.request(), &CancellationToken::default()).unwrap();
    assert_eq!(result.commit_state, "committed");
    assert!(fixture
        .root
        .join("output/toolchain/bin/aros-collect")
        .is_file());
    assert_eq!(
        fs::read_dir(fixture.root.join("cache/cargo/v1"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .count(),
        1
    );
    assert!(!fixture.root.join("cache/cargo-vendor").exists());
}

fn cold_vendor_cargo_wrapper(real_cargo: &Path, poison: &Path, trace: &Path) -> String {
    let package_manifest =
        b"[package]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\nedition = \"2021\"\n";
    let package_source = b"pub fn answer() -> u8 { 42 }\n";
    let real_cargo = shell_quote(real_cargo);
    let poison = shell_quote(poison);
    let trace = shell_quote(trace);
    format!(
        r#"#!/bin/sh
set -eu
case "$1" in
  --version)
    exec {real_cargo} "$@"
    ;;
  vendor)
    test "${{CARGO_HOME:-}}" != {poison}
    printf '%s %s\n' "$1" "$CARGO_HOME" >> {trace}
    vendor=
    for argument in "$@"; do vendor="$argument"; done
    /bin/mkdir -p "$vendor/fixture-dependency-1.0.0/src"
    printf '%s' '[package]
name = "fixture-dependency"
version = "1.0.0"
edition = "2021"
' > "$vendor/fixture-dependency-1.0.0/Cargo.toml"
    printf '%s' 'pub fn answer() -> u8 {{ 42 }}
' > "$vendor/fixture-dependency-1.0.0/src/lib.rs"
    printf '%s' '{{"files":{{"Cargo.toml":"{}","src/lib.rs":"{}"}},"package":"{}"}}' > "$vendor/fixture-dependency-1.0.0/.cargo-checksum.json"
    printf '%s\n' '[source.crates-io]' 'replace-with = "vendored-sources"' '[source.vendored-sources]' "directory = \"$vendor\""
    ;;
  *)
    for argument in "$@"; do
      if [ "$argument" = build ]; then
        test "${{CARGO_HOME:-}}" != {poison}
        printf '%s %s\n' build "$CARGO_HOME" >> {trace}
        exec {real_cargo} "$@"
      fi
    done
    exit 64
    ;;
esac
"#,
        sha256_bytes(package_manifest),
        sha256_bytes(package_source),
        "d".repeat(64),
    )
}

fn shell_quote(path: &Path) -> String {
    format!(
        "'{}'",
        path.display().to_string().replace('\'', "'\\\"'\\\"'")
    )
}

#[test]
fn native_lifecycle_rejects_an_unprepared_cache_before_environment_or_source_execution() {
    let fixture = Fixture::new();
    let mut request = fixture.request();
    request.cache_dir = fixture.root.join("missing-cache");

    let error = executor::run(&request, &CancellationToken::default()).unwrap_err();
    let diagnostic = error.to_string();
    assert!(
        diagnostic.contains("prepared cache inputs only"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("aros cache sources fetch --source-lock"),
        "{diagnostic}"
    );
    let lifecycle = fixture.root.join("work/native-lifecycle");
    assert!(!lifecycle.join("receipts/preflight.json").exists());
    assert!(!lifecycle.join("receipts/environment.json").exists());
    assert!(!lifecycle.join("verified-source-usage.log").exists());
    assert!(!fixture.root.join("missing-cache").exists());
}

#[test]
fn native_lifecycle_rejects_a_tampered_cargo_generation_before_upstream_execution() {
    let fixture = Fixture::new();
    let generations = fixture.root.join("cache/cargo/v1");
    let generation = fs::read_dir(&generations)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(
        generation.join("cargo-vendor/fixture-dependency-1.0.0/src/lib.rs"),
        b"pub fn answer() -> u8 { 0 }\n",
    )
    .unwrap();

    let error = executor::run(&fixture.request(), &CancellationToken::default()).unwrap_err();
    let diagnostic = error.to_string();
    assert!(diagnostic.contains("AX0401"), "{diagnostic}");
    let lifecycle = fixture.root.join("work/native-lifecycle");
    assert!(lifecycle.join("receipts/preflight.json").is_file());
    assert!(!lifecycle.join("receipts/environment.json").exists());
    assert!(!lifecycle.join("build/configure.args").exists());
    assert!(!lifecycle.join("verified-source-usage.log").exists());
}

#[test]
fn explicit_collector_resume_revalidates_predecessors_and_uses_a_fresh_cargo_target() {
    let fixture = Fixture::new();
    executor::run(&fixture.request(), &CancellationToken::default()).unwrap();
    let lifecycle = fixture.root.join("work/native-lifecycle");
    let published = fixture.root.join("output/toolchain");
    let staging = fixture.root.join("output/.aros-native-toolchain-stage");
    fs::rename(&published, &staging).unwrap();
    fs::remove_file(lifecycle.join("receipts/publish.json")).unwrap();
    fs::remove_file(lifecycle.join("receipts/finished-candidate.json")).unwrap();
    let prefix = staging.join("bin");

    // Model an interruption after the compiler receipt but before a collector
    // receipt could be committed. The compiler receipt owns llvm-config; the
    // collector normalization would otherwise make the remeasurement fail.
    fs::remove_file(lifecycle.join("receipts/collector.json")).unwrap();
    for name in ["aros-collect", "collect-aros", "collect-aros32"] {
        fs::remove_file(prefix.join(name)).unwrap();
    }
    fs::write(prefix.join("llvm-config"), b"producer-only").unwrap();

    let mut request = fixture.request();
    request.resume_from = Some(ResumePhase::Compiler);
    let result = executor::run(&request, &CancellationToken::default()).unwrap();
    assert_eq!(result.commit_state, "committed");
    assert!(fixture
        .root
        .join("output/toolchain/bin/aros-collect")
        .is_file());
    assert!(!staging.exists());
    assert!(lifecycle.join("receipts/collector.json").is_file());
    assert!(lifecycle
        .join("logs/collector-resume-1.stdout.log")
        .is_file());
    assert!(lifecycle.join("rust-target-resume-1").is_dir());
}

#[test]
fn collector_resume_rejects_a_tampered_retained_snapshot_before_execution() {
    let fixture = Fixture::new();
    executor::run(&fixture.request(), &CancellationToken::default()).unwrap();
    let lifecycle = fixture.root.join("work/native-lifecycle");
    let published = fixture.root.join("output/toolchain");
    let staging = fixture.root.join("output/.aros-native-toolchain-stage");
    fs::rename(&published, &staging).unwrap();
    fs::remove_file(lifecycle.join("receipts/publish.json")).unwrap();
    fs::remove_file(lifecycle.join("receipts/finished-candidate.json")).unwrap();
    let prefix = staging.join("bin");
    fs::remove_file(lifecycle.join("receipts/collector.json")).unwrap();
    for name in ["aros-collect", "collect-aros", "collect-aros32"] {
        fs::remove_file(prefix.join(name)).unwrap();
    }
    fs::write(prefix.join("llvm-config"), b"producer-only").unwrap();
    fs::write(
        fixture.root.join("work/tools/src/main.rs"),
        "fn main() { panic!(\"tampered\"); }\n",
    )
    .unwrap();

    let mut request = fixture.request();
    request.resume_from = Some(ResumePhase::Compiler);
    let error = executor::run(&request, &CancellationToken::default()).unwrap_err();
    assert!(error
        .to_string()
        .contains("retained preflight receipt does not match the current verified input_sha256"));
    assert!(!lifecycle.join("rust-target-resume-1").exists());
}

#[test]
fn collector_resume_rejects_a_tampered_configure_output_before_execution() {
    let fixture = Fixture::new();
    executor::run(&fixture.request(), &CancellationToken::default()).unwrap();
    let lifecycle = fixture.root.join("work/native-lifecycle");
    let published = fixture.root.join("output/toolchain");
    let staging = fixture.root.join("output/.aros-native-toolchain-stage");
    fs::rename(&published, &staging).unwrap();
    fs::remove_file(lifecycle.join("receipts/publish.json")).unwrap();
    fs::remove_file(lifecycle.join("receipts/finished-candidate.json")).unwrap();
    fs::remove_file(lifecycle.join("receipts/collector.json")).unwrap();
    for name in ["aros-collect", "collect-aros", "collect-aros32"] {
        fs::remove_file(staging.join("bin").join(name)).unwrap();
    }
    fs::write(staging.join("bin/llvm-config"), b"producer-only").unwrap();
    fs::write(
        lifecycle.join("build/configure.args"),
        b"tampered configure boundary\n",
    )
    .unwrap();

    let mut request = fixture.request();
    request.resume_from = Some(ResumePhase::Compiler);
    let error = executor::run(&request, &CancellationToken::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("retained native phase output does not match its durable receipt"),
        "{error}"
    );
    assert!(!lifecycle.join("rust-target-resume-1").exists());
}

#[test]
fn native_configure_failure_retains_owned_roots_and_phase_logs() {
    let fixture = Fixture::new();
    fixture.replace_source_configure(
        "#!/bin/sh\nprintf 'fixture configure failure\\n' >&2\nexit 23\n",
    );

    let error = executor::run(&fixture.request(), &CancellationToken::default()).unwrap_err();
    assert!(error
        .to_string()
        .contains("configure phase exited unsuccessfully"));
    let lifecycle = fixture.root.join("work/native-lifecycle");
    assert!(fixture
        .root
        .join("work/.aros-toolchain-owner-v1.json")
        .is_file());
    assert!(fixture
        .root
        .join("output/.aros-toolchain-owner-v1.json")
        .is_file());
    assert!(lifecycle.join("receipts/environment.json").is_file());
    assert!(lifecycle.join("logs/configure.stderr.log").is_file());
    assert!(!lifecycle.join("receipts/configure.json").exists());
}

#[test]
fn native_publication_never_replaces_a_final_prefix_and_retains_staging() {
    let fixture = Fixture::new();
    fixture.replace_source_configure(
        r#"#!/bin/sh
set -eu
prefix=
cache=
for arg in "$@"; do
  case "$arg" in
    --with-aros-toolchain-install=*) prefix=${arg#*=} ;;
    --with-portssources=*) cache=${arg#*=} ;;
  esac
done
test -n "$prefix"
test -n "$cache"
parent=$(dirname "$prefix")
mkdir "$parent/toolchain"
printf retained > "$parent/toolchain/preexisting"
printf 'crosstools-release:\n\t@$(FETCH) -a llvm-11.0.0.src -s tar.xz -l %s\n\t@mkdir -p %s/bin %s/lib/cmake/llvm\n\t@printf compiler > %s/bin/clang\n\t@chmod 755 %s/bin/clang\n\t@printf producer-only > %s/bin/llvm-config\n' "$cache" "$prefix" "$prefix" "$prefix" "$prefix" "$prefix" > Makefile
"#,
    );

    let error = executor::run(&fixture.request(), &CancellationToken::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("publication refused an existing final prefix"),
        "{error}"
    );
    let output = fixture.root.join("output");
    assert_eq!(
        fs::read(output.join("toolchain/preexisting")).unwrap(),
        b"retained"
    );
    assert!(output
        .join(".aros-native-toolchain-stage/bin/aros-collect")
        .is_file());
    let receipts = fixture.root.join("work/native-lifecycle/receipts");
    assert!(receipts.join("collector.json").is_file());
    assert!(!receipts.join("publish.json").exists());
}

#[test]
fn native_cancellation_reaps_configure_process_group_and_retains_roots() {
    let fixture = Fixture::new();
    fixture.replace_source_configure(
        "#!/bin/sh\nset -eu\n( trap '' TERM; sleep 30 ) &\nprintf '%s\\n' \"$!\" > \"$TMPDIR/child-pid\"\ntrap '' TERM\nsleep 30\n",
    );
    let request = fixture.request();
    let token = CancellationToken::default();
    let canceller = token.clone();
    let child_pid = fixture.root.join("work/native-lifecycle/tmp/child-pid");
    let trigger = std::thread::spawn(move || {
        // Snapshot and environment preparation precede configure. Contended
        // hosts can spend more than five seconds there, so cancel only after
        // observing the descendant, within the 120-second request budget.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let observed = loop {
            if child_pid.is_file() {
                std::thread::sleep(std::time::Duration::from_millis(50));
                break true;
            }
            if canceller.is_cancelled() || std::time::Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        // A missing marker still requires bounded process cleanup, not a
        // successful cancellation proof or an unbounded configure process.
        canceller.cancel();
        observed
    });

    let result = executor::run(&request, &token);
    token.cancel();
    assert!(
        trigger.join().unwrap(),
        "configure descendant never became ready for the cancellation probe"
    );
    let error = result.unwrap_err();
    assert!(
        error.to_string().contains("configure phase cancelled"),
        "{error}"
    );
    let lifecycle = fixture.root.join("work/native-lifecycle");
    assert!(fixture.root.join("work/source").is_dir());
    assert!(fixture
        .root
        .join("output/.aros-toolchain-owner-v1.json")
        .is_file());
    assert!(lifecycle.join("receipts/environment.json").is_file());
    assert!(!lifecycle.join("receipts/configure.json").exists());
    assert!(lifecycle.join("logs/configure.stderr.log").is_file());
    let child = fs::read_to_string(lifecycle.join("tmp/child-pid"))
        .unwrap()
        .trim()
        .to_owned();
    for _ in 0..20 {
        if !Command::new("/bin/kill")
            .args(["-0", &child])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("configure descendant {child} survived native process-group cancellation");
}

#[test]
fn native_deadline_cancels_a_running_phase_without_committing_it() {
    let fixture = Fixture::new();
    fixture.replace_source_configure("#!/bin/sh\ntrap '' TERM\nsleep 30\n");
    let mut request = fixture.request();
    // Snapshot construction includes recursive Git-object validation.  It is
    // deliberately part of the whole-operation deadline and can take several
    // seconds on contended ARM runners, so leave it a real scheduling margin.
    // The sleeping configure phase still deterministically consumes the
    // shared deadline.
    request.timeout_seconds = 15;

    let error = executor::run(&request, &CancellationToken::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("configure phase exceeded the explicit build deadline"),
        "{error}"
    );
    let lifecycle = fixture.root.join("work/native-lifecycle");
    assert!(lifecycle.join("receipts/environment.json").is_file());
    assert!(!lifecycle.join("receipts/configure.json").exists());
    assert!(lifecycle.join("logs/configure.stderr.log").is_file());
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn python_archive(root: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut archive = Builder::new(&mut encoder);
        for (relative, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_cksum();
            archive
                .append_data(&mut header, format!("{root}/{relative}"), *content)
                .unwrap();
        }
        archive.finish().unwrap();
    }
    encoder.finish().unwrap()
}

fn current_cargo_channel() -> String {
    let output = Command::new("cargo").arg("--version").output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned()
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Native lifecycle fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Native lifecycle fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
