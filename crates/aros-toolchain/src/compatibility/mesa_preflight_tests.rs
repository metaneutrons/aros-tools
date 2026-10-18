use super::*;
use crate::compatibility::{
    inspect_llvm_sdk_drivers, prepare_host_tool_closure, CompatibilityHostTool,
    HostToolClosureRequest, LlvmSdkDriverRequest,
};
use crate::profiles::Profiles;
use aros_common::sdk_manifest::{SdkAbiValue, SdkTarget};
use aros_common::{
    measure_tree_content_cas, sha256_bytes, ArosCompilerIdentity, CancellationToken, Sha256Digest,
};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const COMPILER_VERSION: &str = "18.1.8";
const PROFILE_NAME: &str = "arm-raspi";
const PROFILE_TRIPLE: &str = "arm-unknown-aros";

/// Explicit retained inputs only: this opt-in test runs source configure and
/// features, not a compiler/SDK build or release qualification. Host command
/// and Python inputs must already be selected and verified by the operator.
#[test]
#[ignore = "requires explicit retained source/compiler/host/Python case JSON"]
fn actual_source_preflight_with_original_compiler() {
    let case_path = PathBuf::from(
        std::env::var_os("AROS_TEST_MESA_PREFLIGHT_CASE").expect("set explicit case JSON"),
    );
    assert!(case_path.is_absolute());
    let case: serde_json::Value = serde_json::from_slice(&fs::read(case_path).unwrap()).unwrap();
    let path = |key: &str| {
        let path = PathBuf::from(case[key].as_str().unwrap());
        assert!(path.is_absolute());
        path
    };
    let source = path("source_root").canonicalize().unwrap();
    let compiler = path("compiler_root").canonicalize().unwrap();
    let output = path("output_root");
    let profiles = Profiles::parse(&fs::read(path("profiles_file")).unwrap()).unwrap();
    let profile = profiles.select(case["profile"].as_str().unwrap()).unwrap();
    let manifest = aros_common::ArosToolchainManifest::load(&compiler).unwrap();
    assert_eq!(manifest.target_triple, profile.target_triple());
    let cancellation = CancellationToken::default();
    let drivers = inspect_llvm_sdk_drivers(
        &LlvmSdkDriverRequest {
            compiler_root: compiler,
            compiler: manifest.compiler_identity().unwrap(),
            target: sdk_target(profile.target_triple()),
            timeout: Duration::from_secs(30),
        },
        &cancellation,
    )
    .unwrap();
    let host_document: serde_json::Value =
        serde_json::from_slice(&fs::read(path("host_tools_manifest")).unwrap()).unwrap();
    let host_tools = prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: output.parent().unwrap().join("host-tools"),
        tools: host_document["host_tools"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, program)| CompatibilityHostTool {
                name: name.clone(),
                program: PathBuf::from(program.as_str().unwrap()),
            })
            .collect(),
    })
    .unwrap();
    let python: serde_json::Value =
        serde_json::from_slice(&fs::read(path("python_environment_request")).unwrap()).unwrap();
    let mut variables = BTreeMap::from([
        ("PATH".into(), "/nonexistent".into()),
        ("AROS_FETCH_OFFLINE".into(), "1".into()),
        ("LC_ALL".into(), "C".into()),
        ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
        ("PYTHONHASHSEED".into(), "0".into()),
        ("PYTHONNOUSERSITE".into(), "1".into()),
    ]);
    for name in ["PYTHON", "PYTHONPATH"] {
        variables.insert(
            name.into(),
            python["environment"][name].as_str().unwrap().into(),
        );
    }
    let environment = CompatibilityEnvironment::SealedHostTools {
        variables,
        host_tools,
    };
    let source_tree_sha256 = measure_tree_content_cas(&source)
        .unwrap()
        .payload_digest_excluding(None);
    let request = MesaPreflightRequest {
        source_root: &source,
        source_tree_sha256: &source_tree_sha256,
        profile,
        drivers: &drivers,
        environment: &environment,
        output_root: &output,
        timeout: Duration::from_secs(600),
    };
    let result = execute_mesa_preflight(&request, &cancellation).unwrap();
    result.revalidate(&request, &cancellation).unwrap();
    assert!(!result.configuration().target_isa_flags().is_empty());
    eprintln!(
        "local source preflight {} PASS receipt {}",
        profile.name(),
        result.receipt_sha256()
    );
}

#[derive(Clone, Copy)]
enum ConfigureMode {
    Normal,
    Fail,
    Spin,
}

#[derive(Clone, Copy)]
enum FeatureMode {
    Complete,
    MissingFirstCompilerOption,
    MalformedFirstCompilerOption,
}

struct Fixture {
    _temporary: TempDir,
    source_root: PathBuf,
    compiler_root: PathBuf,
    output_root: PathBuf,
    source_tree_sha256: Sha256Digest,
    profiles: Profiles,
    drivers: LlvmSdkDriverBinding,
    environment: CompatibilityEnvironment,
}

impl Fixture {
    fn new() -> Self {
        Self::configured(
            PROFILE_TRIPLE,
            "arm",
            "raspi",
            ConfigureMode::Normal,
            FeatureMode::Complete,
        )
    }

    fn configured(
        driver_triple: &str,
        configured_cpu: &str,
        configured_arch: &str,
        configure_mode: ConfigureMode,
        feature_mode: FeatureMode,
    ) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let compiler_root = root.join("compiler");
        let clang_path = compiler_root.join("bin/clang");
        let source_root = root.join("source");
        fs::create_dir_all(source_root.join("config")).unwrap();
        write_executable(
            &source_root.join("configure"),
            configure_script(
                &source_root,
                &clang_path,
                configured_cpu,
                configured_arch,
                configure_mode,
            )
            .as_bytes(),
        );
        write_executable(
            &source_root.join("config/features"),
            feature_script(configured_cpu, &clang_path, feature_mode).as_bytes(),
        );
        let source_root = source_root.canonicalize().unwrap();
        let source_tree_sha256 = measure_tree_content_cas(&source_root)
            .unwrap()
            .payload_digest_excluding(None);

        let bin = compiler_root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        write_executable(&bin.join("clang"), driver_script(driver_triple).as_bytes());
        symlink("clang", bin.join("clang++")).unwrap();
        let compiler_root = compiler_root.canonicalize().unwrap();
        let drivers = inspect_llvm_sdk_drivers(
            &LlvmSdkDriverRequest {
                compiler_root: compiler_root.clone(),
                compiler: ArosCompilerIdentity::Llvm {
                    version: COMPILER_VERSION.into(),
                },
                target: sdk_target(driver_triple),
                timeout: Duration::from_secs(10),
            },
            &CancellationToken::default(),
        )
        .unwrap();

        let host_tools = prepare_host_tool_closure(&HostToolClosureRequest {
            output_root: root.join("host-tools"),
            tools: [
                ("bash", which("bash")),
                ("make", gnu_make()),
                ("cc", which("cc")),
                ("c++", which("c++")),
                ("mkdir", which("mkdir")),
            ]
            .into_iter()
            .map(|(name, program)| CompatibilityHostTool {
                name: name.into(),
                program,
            })
            .collect(),
        })
        .unwrap();
        let environment = CompatibilityEnvironment::SealedHostTools {
            variables: BTreeMap::from([
                ("PATH".into(), "/nonexistent".into()),
                ("AROS_FETCH_OFFLINE".into(), "1".into()),
            ]),
            host_tools,
        };
        let profiles = Profiles::parse(
            &serde_json::to_vec(&serde_json::json!({
                "schema": "aros-toolchain-profiles-v1",
                "upstream_commit": "a".repeat(40),
                "profiles": [{
                    "name": PROFILE_NAME,
                    "configure_target": PROFILE_NAME,
                    "upstream_output_target": PROFILE_NAME,
                    "target_triple": PROFILE_TRIPLE,
                    "cpu": "arm",
                    "platform": "raspi",
                    "float_abi": "hard",
                    "capabilities": ["c", "cxx"]
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        Self {
            _temporary: temporary,
            source_root,
            compiler_root,
            output_root: root.join("output"),
            source_tree_sha256,
            profiles,
            drivers,
            environment,
        }
    }

    fn request(&self) -> MesaPreflightRequest<'_> {
        self.request_with_timeout(Duration::from_secs(10))
    }

    fn request_with_timeout(&self, timeout: Duration) -> MesaPreflightRequest<'_> {
        MesaPreflightRequest {
            source_root: &self.source_root,
            source_tree_sha256: &self.source_tree_sha256,
            profile: self.profiles.select(PROFILE_NAME).unwrap(),
            drivers: &self.drivers,
            environment: &self.environment,
            output_root: &self.output_root,
            timeout,
        }
    }
}

#[test]
fn confines_unconditional_make_roots_and_remeasured_sysroots() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let build = root.join("build");
    let source = root.join("source");
    let compiler = root.join("compiler");
    for path in [&build, &source, &compiler] {
        fs::create_dir(path).unwrap();
    }
    let top = format!("TOP := {}\n", build.display());
    let src = format!("SRCDIR := {}\n", source.display());
    let include = "include $(TOP)/config/make.cfg\n";
    validate_make_roots(&format!("{top}{src}{include}"), &build, &source).unwrap();
    assert!(validate_make_roots(
        &format!("ifeq (0,1)\n{top}endif\n{src}{include}"),
        &build,
        &source
    )
    .is_err());
    assert!(
        validate_make_roots(&format!("TOP := /wrong\n{src}{include}"), &build, &source).is_err()
    );
    assert!(validate_make_roots(&format!("{top}{top}{src}{include}"), &build, &source).is_err());
    assert!(validate_make_path(&root.join("$(shell touch escaped)")).is_err());
    validate_sysroot("", &source, &compiler).unwrap();
    validate_sysroot(
        &format!("--sysroot={}", compiler.display()),
        &source,
        &compiler,
    )
    .unwrap();
    assert!(
        validate_sysroot(&format!("--sysroot={}", root.display()), &source, &compiler).is_err()
    );
    assert!(validate_sysroot("--sysroot=/missing extra-flag", &source, &compiler).is_err());
}

#[test]
fn rejects_ambient_make_overrides_and_undefined_baseline_flags() {
    let mut ambient = Fixture::new();
    let CompatibilityEnvironment::SealedHostTools { variables, .. } = &mut ambient.environment
    else {
        unreachable!()
    };
    variables.insert("MAKEFLAGS".into(), "-f /unmeasured".into());
    assert_preflight_error(
        execute_mesa_preflight(&ambient.request(), &CancellationToken::default()),
        "Mesa preflight environment contains undeclared inputs or permits fetching",
    );
    assert!(!ambient.output_root.exists());
    let mut missing = Fixture::new();
    let configure = missing.source_root.join("configure");
    let previous = fs::read_to_string(&configure).unwrap();
    assert!(previous.contains("CONFIG_CFLAGS := -Dconfig-c"));
    fs::write(
        &configure,
        previous.replace(
            "CONFIG_CFLAGS := -Dconfig-c",
            "FIXTURE_UNUSED := -Dconfig-c",
        ),
    )
    .unwrap();
    missing.source_tree_sha256 = measure_tree_content_cas(&missing.source_root)
        .unwrap()
        .payload_digest_excluding(None);
    assert_preflight_error(
        execute_mesa_preflight(&missing.request(), &CancellationToken::default()),
        "Mesa preflight child failed; inspect retained stage logs",
    );
    assert!(
        fs::read_to_string(missing.output_root.join("configured-values.stderr"))
            .unwrap()
            .contains("missing configured Mesa variable CONFIG_CFLAGS")
    );
}

#[test]
fn measures_configuration_without_running_or_adopting_the_default_target() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let completed = execute_mesa_preflight(&request, &cancellation).unwrap();

    assert_eq!(
        completed.configuration().identity().target_profile,
        PROFILE_NAME
    );
    assert_eq!(
        completed.configuration().identity().target_triple,
        PROFILE_TRIPLE
    );
    assert_eq!(
        completed
            .configuration()
            .target_isa_flags()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["-march=armv7", "-mfloat-abi=hard"]
    );
    assert_eq!(completed.make_values()["CONFIG_CFLAGS"], "-Dconfig-c");
    assert_eq!(
        completed.make_values()["MESA_COMPILEOPTIONS"],
        "-Dmesa-feature"
    );
    assert_eq!(completed.make_values()["CFLAGS_GNU11"], "-std=gnu11");
    assert_eq!(
        completed.make_values()["LLVM_NOWARN"],
        "-Wno-implicit-int",
        "recursive warnings must resolve after the generated feature include"
    );
    let transported = completed.configuration().make_variables();
    for name in MESA_BASELINE_VARIABLES {
        assert_eq!(
            transported.get(*name),
            completed.make_values().get(*name),
            "measured baseline lost at transport boundary: {name}"
        );
    }
    assert_eq!(
        completed.make_values()["CFLAGS_NO_BUILTIN_FFS"],
        "-fno-builtin-ffs"
    );
    assert_eq!(completed.make_values()["CFLAGS_NO_BUILTIN_FFSLL"], "");
    for key in MESA_COMPILER_OPTION_VARIABLES {
        assert!(completed.make_values().contains_key(*key), "missing {key}");
    }
    for (key, _) in BUILTINS {
        assert!(completed.make_values().contains_key(*key), "missing {key}");
    }

    let receipt = fs::read(fixture.output_root.join("receipt.json")).unwrap();
    assert_eq!(completed.receipt_sha256(), &sha256_bytes(&receipt));
    completed.revalidate(&request, &cancellation).unwrap();
    assert!(
        !fixture
            .output_root
            .join("configure/poison-target-ran")
            .exists(),
        "the ordinary Makefile target after the include boundary must not run"
    );

    let occupied = Fixture::new();
    fs::create_dir(&occupied.output_root).unwrap();
    fs::write(occupied.output_root.join("keep"), b"existing output").unwrap();
    let error = execute_mesa_preflight(&occupied.request(), &CancellationToken::default());
    assert_preflight_error(error, "Mesa preflight output must be fresh");
    assert_eq!(
        fs::read(occupied.output_root.join("keep")).unwrap(),
        b"existing output"
    );
}

#[test]
fn rejects_missing_or_malformed_compiler_features_without_inferring_empty_values() {
    let missing = Fixture::configured(
        PROFILE_TRIPLE,
        "arm",
        "raspi",
        ConfigureMode::Normal,
        FeatureMode::MissingFirstCompilerOption,
    );
    let error = execute_mesa_preflight(&missing.request(), &CancellationToken::default());
    assert_preflight_error(
        error,
        "Mesa preflight child failed; inspect retained stage logs",
    );
    assert!(
        fs::read_to_string(missing.output_root.join("configured-values.stderr"))
            .unwrap()
            .contains("missing configured Mesa variable CFLAGS_GNU11")
    );
    let measured =
        fs::read_to_string(missing.output_root.join("configured-values.stdout")).unwrap();
    assert!(measured.lines().any(|line| line == "CFLAGS_GNU11="));

    let malformed = Fixture::configured(
        PROFILE_TRIPLE,
        "arm",
        "raspi",
        ConfigureMode::Normal,
        FeatureMode::MalformedFirstCompilerOption,
    );
    let error = execute_mesa_preflight(&malformed.request(), &CancellationToken::default());
    assert_preflight_error(error, "source produced invalid Mesa configuration values");
}

#[test]
fn rejects_stale_source_and_compiler_before_creating_output() {
    let stale_source = Fixture::new();
    fs::write(
        stale_source.source_root.join("changed-after-measurement"),
        b"changed",
    )
    .unwrap();
    let error = execute_mesa_preflight(&stale_source.request(), &CancellationToken::default());
    assert_preflight_error(error, "Mesa preflight source changed");
    assert!(!stale_source.output_root.exists());

    let stale_compiler = Fixture::new();
    fs::write(
        stale_compiler.compiler_root.join("post-query-change"),
        b"changed",
    )
    .unwrap();
    let error = execute_mesa_preflight(&stale_compiler.request(), &CancellationToken::default());
    assert_preflight_error(error, "LLVM compiler tree changed after driver inspection");
    assert!(!stale_compiler.output_root.exists());
}

#[test]
fn rejects_driver_and_configured_target_disagreement() {
    let driver_mismatch = Fixture::configured(
        "arm-other-aros",
        "arm",
        "raspi",
        ConfigureMode::Normal,
        FeatureMode::Complete,
    );
    let error = execute_mesa_preflight(&driver_mismatch.request(), &CancellationToken::default());
    assert_preflight_error(error, "invalid or cancelled Mesa preflight selection");
    assert!(!driver_mismatch.output_root.exists());

    let configured_mismatch = Fixture::configured(
        PROFILE_TRIPLE,
        "aarch64",
        "raspi",
        ConfigureMode::Normal,
        FeatureMode::Complete,
    );
    let error = execute_mesa_preflight(
        &configured_mismatch.request(),
        &CancellationToken::default(),
    );
    assert_preflight_error(
        error,
        "configured Mesa CPU/platform differs from selected profile",
    );
    assert!(configured_mismatch.output_root.exists());
}

#[test]
fn retains_failed_child_logs_and_detects_tampered_receipt() {
    let failed = Fixture::configured(
        PROFILE_TRIPLE,
        "arm",
        "raspi",
        ConfigureMode::Fail,
        FeatureMode::Complete,
    );
    let error = execute_mesa_preflight(&failed.request(), &CancellationToken::default());
    assert_preflight_error(
        error,
        "Mesa preflight child failed; inspect retained stage logs",
    );
    assert_eq!(
        fs::read(failed.output_root.join("configure.stderr")).unwrap(),
        b"fixture configure failed\n"
    );
    assert!(failed.output_root.join("configure.stdout").exists());

    let successful = Fixture::new();
    let request = successful.request();
    let completed = execute_mesa_preflight(&request, &CancellationToken::default()).unwrap();
    completed
        .revalidate(&request, &CancellationToken::default())
        .unwrap();
    fs::write(
        successful.output_root.join("receipt.json"),
        b"tampered receipt",
    )
    .unwrap();
    assert_preflight_error(
        completed.revalidate(&request, &CancellationToken::default()),
        "Mesa preflight output changed",
    );
}

#[test]
fn enforces_the_total_deadline_and_cancels_a_running_child() {
    let expired = Fixture::new();
    let error = execute_mesa_preflight(
        &expired.request_with_timeout(Duration::from_nanos(1)),
        &CancellationToken::default(),
    );
    assert_preflight_error(error, "Mesa preflight deadline expired");
    assert!(expired.output_root.exists());
    assert!(!expired.output_root.join("configure.stdout").exists());

    let running = Fixture::configured(
        PROFILE_TRIPLE,
        "arm",
        "raspi",
        ConfigureMode::Spin,
        FeatureMode::Complete,
    );
    let cancellation = CancellationToken::default();
    let trigger = cancellation.clone();
    let started = running.output_root.join("configure/started");
    let canceller = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !started.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        trigger.cancel();
    });
    let error = execute_mesa_preflight(
        &running.request_with_timeout(Duration::from_secs(10)),
        &cancellation,
    );
    canceller.join().unwrap();
    assert_preflight_error(
        error,
        "Mesa preflight child failed; inspect retained stage logs",
    );
    assert!(running.output_root.join("configure.stdout").exists());
    assert!(running.output_root.join("configure.stderr").exists());
}

fn configure_script(
    source_root: &Path,
    clang_path: &Path,
    configured_cpu: &str,
    configured_arch: &str,
    mode: ConfigureMode,
) -> String {
    match mode {
        ConfigureMode::Fail => {
            return "#!/bin/bash\nprintf 'fixture configure failed\\n' >&2\nexit 23\n".into();
        }
        ConfigureMode::Spin => {
            return "#!/bin/bash\nprintf started > started\nwhile :; do :; done\n".into();
        }
        ConfigureMode::Normal => {}
    }

    let mut script = String::from(
        "#!/bin/bash\nset -eu\n[ \"$1\" = \"--target=arm-raspi\" ] || exit 91\n\
         mkdir -p config gen/config\n",
    );
    writeln!(script,
        r#"printf 'default: all\nTOP := %s\nSRCDIR := %s\ninclude $(TOP)/config/make.cfg\nall:\n\t@: > poison-target-ran\n' "$PWD" {} > Makefile"#,
        shell_quote(&source_root.to_string_lossy())
    ).unwrap();
    for line in make_configuration_lines(configured_cpu, configured_arch, clang_path) {
        writeln!(
            script,
            "printf '%s\\n' {} >> config/make.cfg",
            shell_quote(&line)
        )
        .unwrap();
    }
    script
}

fn make_configuration_lines(
    configured_cpu: &str,
    configured_arch: &str,
    clang_path: &Path,
) -> Vec<String> {
    let clang = clang_path.to_string_lossy();
    let clangxx = clang_path
        .with_file_name("clang++")
        .to_string_lossy()
        .into_owned();
    vec![
        format!("AROS_TARGET_CPU := {configured_cpu}"),
        format!("AROS_TARGET_ARCH := {configured_arch}"),
        format!("TARGET_CC := {clang}"),
        "TARGET_SYSROOT :=".into(),
        "TARGET_CFLAGS := -target-c".into(),
        "DEBUG_CFLAGS := -g".into(),
        "OPTIMIZATION_CFLAGS := -O2".into(),
        format!("TARGET_CPP := {clang} -E"),
        format!("TARGET_CXX := {clangxx}"),
        "TARGET_CXXFLAGS := -target-cxx".into(),
        "AROS_DEVELOPER := fixture-developer".into(),
        "AROS_BUILD_HOST := x86_64-unknown-linux-gnu".into(),
        "GENDIR := $(TOP)/gen".into(),
        "PORTSDIR := /fixture/ports".into(),
        "TARGET_ISA_CFLAGS := -march=armv7 -mfloat-abi=hard".into(),
        "TARGET_ISA_CXXFLAGS := -march=armv7 -mfloat-abi=hard".into(),
        "CONFIG_CFLAGS := -Dconfig-c".into(),
        "CONFIG_CXXFLAGS := -Dconfig-cxx".into(),
        "CONFIG_CPPFLAGS := -Dconfig-cpp".into(),
        "TARGET_CPPFLAGS := -target-cpp".into(),
        "DEBUG_CXXFLAGS := -g".into(),
        "LLVM_NOWARN = $(NOWARN_IMPLICIT_INT)".into(),
        "MESA_COMPILEOPTIONS := -Dmesa-feature".into(),
    ]
}

fn feature_script(configured_cpu: &str, clang_path: &Path, mode: FeatureMode) -> String {
    let clang = clang_path.to_string_lossy();
    let clangxx = clang_path
        .with_file_name("clang++")
        .to_string_lossy()
        .into_owned();
    let mut script = format!(
        "#!/bin/bash\nset -eu\n\
         [ \"$1\" = \"--host=x86_64-unknown-linux-gnu\" ] || exit 30\n\
         [ \"$AROS_TARGET_CPU\" = {configured_cpu} ] || exit 31\n\
         [ \"$CC\" = \"{clang} \" ] || exit 32\n\
         [ \"$CONFIG_STATUS\" = features.status ] || exit 33\n\
         [ \"$CPP\" = \"{clang} -E \" ] || exit 35\n\
         [ \"$CXX\" = \"{clangxx} \" ] || exit 36\n\
         [ \"$CFLAGS\" = \"-target-c -g -O2\" ] || exit 37\n\
         [ \"$AROS_DEVELOPER\" = fixture-developer ] || exit 38\n\
         case \"$2\" in --with-config-dir=*) config_dir=${{2#*=}} ;; *) exit 34 ;; esac\n\
         mkdir -p \"$config_dir\"\n\
         : > \"$config_dir/compiler.cfg\"\n"
    );
    for (index, name) in MESA_COMPILER_OPTION_VARIABLES.iter().enumerate() {
        if index == 0 && matches!(mode, FeatureMode::MissingFirstCompilerOption) {
            continue;
        }
        let value = if index == 0 {
            match mode {
                FeatureMode::MalformedFirstCompilerOption => "@malformed".into(),
                FeatureMode::Complete | FeatureMode::MissingFirstCompilerOption => {
                    "-std=gnu11".into()
                }
            }
        } else if index == 1 {
            "-std=c++17".into()
        } else {
            String::new()
        };
        script.push_str(&assignment_command(name, &value));
    }
    for (index, (name, value)) in BUILTINS.iter().enumerate() {
        let value = if index % 2 == 0 { *value } else { "" };
        script.push_str(&assignment_command(name, value));
    }
    script.push_str(&assignment_command(
        "NOWARN_IMPLICIT_INT",
        "-Wno-implicit-int",
    ));
    script
}

fn assignment_command(name: &str, value: &str) -> String {
    format!(
        "printf '%s\\n' {} >> \"$config_dir/compiler.cfg\"\n",
        shell_quote(&format!("{name} := {value}"))
    )
}

fn driver_script(triple: &str) -> String {
    format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then\n\
           printf 'clang version {COMPILER_VERSION}\\n'\n\
         elif [ \"$1\" = \"--target={triple}\" ] && [ \"$2\" = \"-dumpmachine\" ]; then\n\
           printf '%s\\n' '{triple}'\n\
         else\n\
           exit 12\n\
         fi\n"
    )
}

fn sdk_target(triple: &str) -> SdkTarget {
    SdkTarget {
        target_triple: triple.into(),
        abi: BTreeMap::from([(
            "source_declared_isa".into(),
            SdkAbiValue::Text("fixture-isa".into()),
        )]),
    }
}

fn which(name: &str) -> PathBuf {
    let output = Command::new("which")
        .arg(name)
        .output()
        .unwrap_or_else(|error| panic!("cannot run which for {name}: {error}"));
    assert!(
        output.status.success(),
        "required host tool {name} is unavailable"
    );
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
        .canonicalize()
        .unwrap()
}

fn gnu_make() -> PathBuf {
    for name in ["gmake", "make"] {
        let output = Command::new("which").arg(name).output().unwrap();
        if !output.status.success() {
            continue;
        }
        let path = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
            .canonicalize()
            .unwrap();
        let version = Command::new(&path).arg("--version").output().unwrap();
        if version.status.success()
            && String::from_utf8_lossy(&version.stdout)
                .lines()
                .next()
                .is_some_and(|line| line.starts_with("GNU Make "))
        {
            return path;
        }
    }
    panic!("tests require GNU Make, found through which gmake or which make");
}

fn write_executable(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn assert_preflight_error<T>(result: Result<T, ContractError>, expected_message: &str) {
    let Err(error) = result else {
        panic!("fixture must be rejected with {expected_message}")
    };
    let diagnostics = error.diagnostics();
    assert_eq!(diagnostics.diagnostics.len(), 1);
    assert_eq!(diagnostics.diagnostics[0].message, expected_message);
}
