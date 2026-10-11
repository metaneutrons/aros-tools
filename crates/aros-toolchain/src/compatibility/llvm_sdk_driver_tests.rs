use super::*;
use aros_common::sdk_manifest::{SdkAbiValue, SdkTarget};
use std::collections::BTreeMap;
use std::os::unix::fs::{symlink, PermissionsExt};
use tempfile::TempDir;

const COMPILER_VERSION: &str = "18.1.8";

struct Fixture {
    _temporary: TempDir,
    root: PathBuf,
    target: SdkTarget,
}

impl Fixture {
    fn new(triple: &str) -> Self {
        Self::with_script(
            triple,
            &driver_script(
                "clang version 18.1.8",
                triple,
                "clang version 18.1.8",
                triple,
                "printf '%s\\n' \"$version\"",
                "",
            ),
        )
    }

    fn with_script(triple: &str, script: &str) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("compiler");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        write_executable(&bin.join("clang"), script.as_bytes());
        symlink("clang", bin.join("clang++")).unwrap();
        let root = root.canonicalize().unwrap();
        Self {
            _temporary: temporary,
            root,
            target: target(triple),
        }
    }

    fn request(&self) -> LlvmSdkDriverRequest {
        LlvmSdkDriverRequest {
            compiler_root: self.root.clone(),
            compiler: ArosCompilerIdentity::Llvm {
                version: COMPILER_VERSION.to_owned(),
            },
            target: self.target.clone(),
            timeout: Duration::from_secs(2),
        }
    }
}

fn target(triple: &str) -> SdkTarget {
    SdkTarget {
        target_triple: triple.to_owned(),
        abi: BTreeMap::from([(
            "source_declared_isa".to_owned(),
            SdkAbiValue::Text("fixture-isa".to_owned()),
        )]),
    }
}

fn valid_gnu_target_contract() -> aros_common::elf::riscv::TargetContract {
    let serialized = br#"{"schema":"aros-riscv-target-v1","isa":"rv32imac","abi":"ilp32","code_model":"medany","architecture":"rv32i2p1_m2p0_a2p1_c2p0","unaligned_access":false,"atomic_abi":0,"x3_reg_usage":0}"#;
    let serialized_value: serde_json::Value = serde_json::from_slice(serialized).unwrap();
    let contract = aros_common::elf::riscv::TargetContract::parse(serialized).unwrap();
    assert_eq!(serde_json::to_value(&contract).unwrap(), serialized_value);
    contract
}

fn driver_script(
    clang_version: &str,
    clang_triple: &str,
    clangxx_version: &str,
    clangxx_triple: &str,
    version_action: &str,
    stderr_action: &str,
) -> String {
    format!(
        r#"#!/bin/sh
role=${{0##*/}}
if [ "$role" = "clang++" ]; then
  version={}
  triple={}
else
  version={}
  triple={}
fi
if [ "$1" = "--version" ]; then
  {version_action}
else
  [ "$2" = "-dumpmachine" ] || exit 12
  printf '%s\n' "$triple"
fi
{stderr_action}
"#,
        shell_quote(clangxx_version),
        shell_quote(clangxx_triple),
        shell_quote(clang_version),
        shell_quote(clang_triple),
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn write_executable(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn assert_rejection(result: Result<LlvmSdkDriverBinding, ContractError>, expected_message: &str) {
    let error = result.expect_err("fixture must be rejected");
    let diagnostics = error.diagnostics();
    assert_eq!(diagnostics.diagnostics.len(), 1);
    assert_eq!(diagnostics.diagnostics[0].message, expected_message);
}

fn expected_script(triple: &str) -> String {
    driver_script(
        "clang version 18.1.8",
        triple,
        "clang version 18.1.8",
        triple,
        "printf '%s\\n' \"$version\"",
        "",
    )
}

#[test]
fn measures_both_historical_driver_roles_for_source_defined_target_triples() {
    for triple in [
        "x86_64-unknown-aros",
        "aarch64-unknown-aros",
        "armv7-unknown-aros",
        "futurecpu-vendor-aros",
    ] {
        let fixture = Fixture::new(triple);
        let binding =
            inspect_llvm_sdk_drivers(&fixture.request(), &CancellationToken::default()).unwrap();

        assert_eq!(binding.target().target_triple, triple);
        assert_eq!(binding.clang_path(), fixture.root.join("bin/clang"));
        assert_eq!(binding.clangxx_path(), fixture.root.join("bin/clang++"));
        assert_eq!(
            binding.clangxx_identity().invocation_path(),
            fixture.root.join("bin/clang++")
        );
        assert_eq!(
            binding.clangxx_identity().resolved_executable(),
            fixture.root.join("bin/clang")
        );
        assert_eq!(
            binding.clang_identity().file_sha256(),
            binding.clangxx_identity().file_sha256()
        );
        assert!(binding.clang_identity().file_size() > 0);
        assert_eq!(
            binding.clang_identity().version_query().stdout_bytes(),
            b"clang version 18.1.8\n"
        );
        assert_eq!(
            binding.clang_identity().target_query().stdout_bytes(),
            format!("{triple}\n").as_bytes()
        );
        assert_eq!(
            binding.clang_identity().target_query().size(),
            binding.clang_identity().target_query().stdout_bytes().len() as u64
        );
        assert_eq!(
            binding.clang_identity().target_query().sha256(),
            &aros_common::sha256_bytes(binding.clang_identity().target_query().stdout_bytes())
        );
        assert_eq!(
            binding.clangxx_identity().version_query().stdout_bytes(),
            b"clang version 18.1.8\n"
        );
        assert_eq!(
            binding.clangxx_identity().target_query().stdout_bytes(),
            format!("{triple}\n").as_bytes()
        );
        assert_eq!(binding.compiler_tree_sha256().as_str().len(), 64);
        binding.revalidate(&CancellationToken::default()).unwrap();
    }
}

#[test]
fn rejects_wrong_c_or_cxx_version_and_target_reports() {
    let valid = "riscv-unknown-aros";
    let cases = [
        ("clang version 17.0.0", valid, "clang version 18.1.8", valid),
        (
            "clang version 18.1.8",
            "riscv-other-aros",
            "clang version 18.1.8",
            valid,
        ),
        ("clang version 18.1.8", valid, "clang version 17.0.0", valid),
        (
            "clang version 18.1.8",
            valid,
            "clang version 18.1.8",
            "riscv-other-aros",
        ),
    ];
    for (c_version, c_triple, cxx_version, cxx_triple) in cases {
        let script = driver_script(
            c_version,
            c_triple,
            cxx_version,
            cxx_triple,
            "printf '%s\\n' \"$version\"",
            "",
        );
        let fixture = Fixture::with_script(valid, &script);
        let expected_message =
            if c_version != "clang version 18.1.8" || cxx_version != "clang version 18.1.8" {
                "LLVM SDK driver version does not match the selected compiler identity"
            } else {
                "LLVM SDK driver dumpmachine differs from the selected target triple"
            };
        assert_rejection(
            inspect_llvm_sdk_drivers(&fixture.request(), &CancellationToken::default()),
            expected_message,
        );
    }
}

#[test]
fn rejects_stderr_non_utf8_and_truncated_output() {
    let triple = "riscv-unknown-aros";
    let stderr_script = driver_script(
        "clang version 18.1.8",
        triple,
        "clang version 18.1.8",
        triple,
        "printf '%s\\n' \"$version\"",
        "printf 'warning\\n' >&2",
    );
    let stderr_fixture = Fixture::with_script(triple, &stderr_script);
    assert_rejection(
        inspect_llvm_sdk_drivers(&stderr_fixture.request(), &CancellationToken::default()),
        "LLVM SDK driver query wrote to stderr",
    );

    let invalid_utf8_script = driver_script(
        "clang version 18.1.8",
        triple,
        "clang version 18.1.8",
        triple,
        r"printf '\377\n'",
        "",
    );
    let invalid_utf8_fixture = Fixture::with_script(triple, &invalid_utf8_script);
    assert_rejection(
        inspect_llvm_sdk_drivers(
            &invalid_utf8_fixture.request(),
            &CancellationToken::default(),
        ),
        "LLVM SDK driver stdout is not UTF-8",
    );

    let truncated_script = driver_script(
        "clang version 18.1.8",
        triple,
        "clang version 18.1.8",
        triple,
        "printf '%100000s' ''",
        "",
    );
    let truncated_fixture = Fixture::with_script(triple, &truncated_script);
    assert_rejection(
        inspect_llvm_sdk_drivers(&truncated_fixture.request(), &CancellationToken::default()),
        "LLVM SDK driver stdout exceeded its capture bound",
    );
}

#[test]
fn rejects_nonzero_status_and_ambiguous_version_or_triple_output() {
    let triple = "riscv-unknown-aros";
    let failed = Fixture::with_script(triple, "#!/bin/sh\nexit 7\n");
    assert_rejection(
        inspect_llvm_sdk_drivers(&failed.request(), &CancellationToken::default()),
        "LLVM SDK driver query returned a nonzero status",
    );
    let wrong_version = Fixture::with_script(
        triple,
        &driver_script(
            "clang version 18.1.80",
            triple,
            "clang version 18.1.8",
            triple,
            "printf '%s\\n' \"$version\"",
            "",
        ),
    );
    assert_rejection(
        inspect_llvm_sdk_drivers(&wrong_version.request(), &CancellationToken::default()),
        "LLVM SDK driver version does not match the selected compiler identity",
    );
    for invalid in [format!(" {triple}"), format!("{triple}\nextra")] {
        let fixture = Fixture::with_script(
            triple,
            &driver_script(
                "clang version 18.1.8",
                &invalid,
                "clang version 18.1.8",
                triple,
                "printf '%s\\n' \"$version\"",
                "",
            ),
        );
        assert_rejection(
            inspect_llvm_sdk_drivers(&fixture.request(), &CancellationToken::default()),
            "LLVM SDK driver dumpmachine differs from the selected target triple",
        );
    }
}

#[test]
fn preserves_only_explicit_query_environment_and_arguments() {
    let triple = "riscv-unknown-aros";
    let script = expected_script(triple).replace(
        "role=${0##*/}",
        r#"[ "$PATH" = "/nonexistent" ] || exit 21
[ "$LC_ALL" = "C" ] || exit 22
[ -z "${HOME+x}" ] || exit 23
if [ "$1" = "--version" ]; then
  [ "$#" = 1 ] || exit 24
else
  [ "$#" = 2 ] || exit 25
  [ "$1" = "--target=riscv-unknown-aros" ] || exit 26
fi
role=${0##*/}"#,
    );
    let fixture = Fixture::with_script(triple, &script);
    inspect_llvm_sdk_drivers(&fixture.request(), &CancellationToken::default()).unwrap();
}

#[test]
fn rejects_compiler_tree_mutation_during_queries() {
    let triple = "riscv-unknown-aros";
    let script = expected_script(triple).replace(
        "role=${0##*/}",
        "printf 'changed\\n' > query-side-effect\nrole=${0##*/}",
    );
    let fixture = Fixture::with_script(triple, &script);
    assert_rejection(
        inspect_llvm_sdk_drivers(&fixture.request(), &CancellationToken::default()),
        "LLVM compiler tree changed during driver inspection",
    );
}

#[test]
fn rejects_timeout_and_cancellation() {
    let triple = "riscv-unknown-aros";
    let slow_script = driver_script(
        "clang version 18.1.8",
        triple,
        "clang version 18.1.8",
        triple,
        "/bin/sleep 2",
        "",
    );
    let timed = Fixture::with_script(triple, &slow_script);
    let mut request = timed.request();
    request.timeout = Duration::from_millis(20);
    assert_rejection(
        inspect_llvm_sdk_drivers(&request, &CancellationToken::default()),
        "LLVM SDK driver query exceeded its explicit timeout",
    );

    let cancelled = Fixture::with_script(triple, &slow_script);
    let token = CancellationToken::default();
    let child_token = token.clone();
    let cancellation = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        child_token.cancel();
    });
    assert_rejection(
        inspect_llvm_sdk_drivers(&cancelled.request(), &token),
        "LLVM SDK driver inspection was cancelled",
    );
    cancellation.join().unwrap();
}

#[test]
fn rejects_gnu_identity_invalid_target_triple_and_invalid_timeout() {
    let triple = "riscv-unknown-aros";
    let fixture = Fixture::new(triple);

    let mut request = fixture.request();
    request.compiler = ArosCompilerIdentity::Gnu {
        gcc_version: "16.2.0".into(),
        binutils_version: "2.47".into(),
        target: valid_gnu_target_contract(),
    };
    assert_rejection(
        inspect_llvm_sdk_drivers(&request, &CancellationToken::default()),
        "LLVM SDK driver inspection requires an LLVM compiler identity",
    );

    let mut request = fixture.request();
    request.compiler_root = PathBuf::from("relative/compiler");
    assert_rejection(
        inspect_llvm_sdk_drivers(&request, &CancellationToken::default()),
        "LLVM compiler root must be an absolute path",
    );

    for invalid in ["riscv--aros", "riscv-aros-extra", "riscv-aros-", "aros"] {
        let mut request = fixture.request();
        request.target = target(invalid);
        assert_rejection(
            inspect_llvm_sdk_drivers(&request, &CancellationToken::default()),
            "SDK target triple must have nonempty components and end in an exact -aros component",
        );
    }

    let mut request = fixture.request();
    request.timeout = Duration::ZERO;
    assert_rejection(
        inspect_llvm_sdk_drivers(&request, &CancellationToken::default()),
        "LLVM SDK driver query timeout must be positive and at most 60 seconds",
    );
    request.timeout = Duration::from_secs(61);
    assert_rejection(
        inspect_llvm_sdk_drivers(&request, &CancellationToken::default()),
        "LLVM SDK driver query timeout must be positive and at most 60 seconds",
    );
}

#[test]
fn rejects_missing_non_executable_and_escaping_driver_paths() {
    let triple = "riscv-unknown-aros";

    let missing = Fixture::new(triple);
    fs::remove_file(missing.root.join("bin/clang++")).unwrap();
    assert_rejection(
        inspect_llvm_sdk_drivers(&missing.request(), &CancellationToken::default()),
        "required LLVM SDK driver is missing or unresolved",
    );

    let non_executable = Fixture::new(triple);
    let driver = non_executable.root.join("bin/clang");
    let mut permissions = fs::metadata(&driver).unwrap().permissions();
    permissions.set_mode(0o644);
    fs::set_permissions(driver, permissions).unwrap();
    assert_rejection(
        inspect_llvm_sdk_drivers(&non_executable.request(), &CancellationToken::default()),
        "LLVM SDK driver is not an executable regular file",
    );

    let escaping = Fixture::new(triple);
    let external = tempfile::tempdir().unwrap();
    write_executable(
        &external.path().join("clang"),
        expected_script(triple).as_bytes(),
    );
    fs::remove_file(escaping.root.join("bin/clang")).unwrap();
    symlink(
        external.path().join("clang"),
        escaping.root.join("bin/clang"),
    )
    .unwrap();
    assert_rejection(
        inspect_llvm_sdk_drivers(&escaping.request(), &CancellationToken::default()),
        "LLVM SDK driver symlink escapes the selected compiler root",
    );
}

#[test]
fn revalidation_detects_file_and_internal_symlink_mutation() {
    let triple = "riscv-unknown-aros";
    let file_changed = Fixture::new(triple);
    fs::create_dir_all(file_changed.root.join("lib")).unwrap();
    fs::write(file_changed.root.join("lib/other-tool"), b"initial\n").unwrap();
    let binding =
        inspect_llvm_sdk_drivers(&file_changed.request(), &CancellationToken::default()).unwrap();
    fs::write(file_changed.root.join("lib/other-tool"), b"changed\n").unwrap();
    assert_eq!(
        binding
            .revalidate(&CancellationToken::default())
            .unwrap_err()
            .diagnostics()
            .diagnostics[0]
            .message,
        "LLVM compiler tree changed after driver inspection"
    );

    let symlink_changed = Fixture::new(triple);
    write_executable(
        &symlink_changed.root.join("bin/clang-alternate"),
        expected_script(triple).as_bytes(),
    );
    let binding =
        inspect_llvm_sdk_drivers(&symlink_changed.request(), &CancellationToken::default())
            .unwrap();
    fs::remove_file(symlink_changed.root.join("bin/clang++")).unwrap();
    symlink("clang-alternate", symlink_changed.root.join("bin/clang++")).unwrap();
    assert_eq!(
        binding
            .revalidate(&CancellationToken::default())
            .unwrap_err()
            .diagnostics()
            .diagnostics[0]
            .message,
        "LLVM compiler tree changed after driver inspection"
    );
}
