//! Synthetic end-to-end coverage for ordinary native SDK application links.
//!
//! Declared shell drivers inspect the compiler arguments and SDK inputs, then
//! copy target-contract ELF fixtures. These tests exercise executor evidence
//! and rejection paths; they do not qualify a real compiler or runtime.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::Path;
use std::time::Duration;

use aros_common::native_consumer_contract::load_bound_native_consumer_contract;
use aros_common::toolchain_layout::ToolchainToolLayout;
use aros_common::{
    elf, measure_tree_content_cas, sha256_bytes, ArosCompilerIdentity, CancellationToken,
    TargetProfile,
};
use serde_json::{json, Value};

use super::{execute_native_sdk_links, readback_native_sdk_links, NativeSdkLinkRequest};

const CONTRACT_FILE: &str = "native-consumer-v1.json";
const BINDING_FILE: &str = "aros-native-consumer-binding.json";
const RECEIPT_FILE: &str = "native-sdk-links.receipt.json";
const INVENTORY_FILE: &str = "native-sdk-inventory.json";

struct Harness {
    _temporary: tempfile::TempDir,
    request: NativeSdkLinkRequest,
}

#[test]
fn executes_four_default_driver_sdk_links_and_persists_relocation_evidence() {
    let temporary = tempfile::tempdir().unwrap();
    let harness = harness(temporary, DriverMode::Normal);
    let report = execute_native_sdk_links(&harness.request, &CancellationToken::default())
        .expect("synthetic native SDK links should complete");

    let receipt_bytes = fs::read(&report.receipt).unwrap();
    assert_eq!(sha256_bytes(&receipt_bytes), report.receipt_sha256);
    let receipt: Value = serde_json::from_slice(&receipt_bytes).unwrap();
    assert_eq!(receipt["schema"], "aros-native-sdk-link-receipt-v1");
    assert_eq!(
        receipt["qualification"],
        "local-links-not-release-admission"
    );
    assert_eq!(receipt["outputs"].as_object().unwrap().len(), 4);
    assert_eq!(receipt["maps"].as_object().unwrap().len(), 4);
    assert_eq!(receipt["reports"].as_object().unwrap().len(), 2);
    for language in ["c", "cxx"] {
        let original_name = format!("original-{language}");
        let relocated_name = format!("relocated-{language}");
        let original = &receipt["outputs"][original_name.as_str()];
        let relocated = &receipt["outputs"][relocated_name.as_str()];
        assert_eq!(original["sha256"], relocated["sha256"]);
        assert_eq!(original["size"], relocated["size"]);
        for name in [&original_name, &relocated_name] {
            let bytes = fs::read(harness.request.output_root.join(format!("{name}.elf"))).unwrap();
            assert_eq!(
                receipt["outputs"][name.as_str()]["sha256"],
                sha256_bytes(&bytes).to_string()
            );
            assert_eq!(
                receipt["outputs"][name.as_str()]["size"],
                bytes.len() as u64
            );
            let map = fs::read(harness.request.output_root.join(format!("{name}.map"))).unwrap();
            assert_eq!(
                receipt["maps"][name.as_str()]["sha256"],
                sha256_bytes(&map).to_string()
            );
            assert_eq!(receipt["maps"][name.as_str()]["size"], map.len() as u64);
        }
    }

    let inventory_path = harness.request.output_root.join(INVENTORY_FILE);
    let inventory_bytes = fs::read(&inventory_path).unwrap();
    assert_eq!(
        receipt["inventory"]["sha256"],
        sha256_bytes(&inventory_bytes).to_string()
    );
    assert_eq!(
        receipt["sdk_inventory_sha256"],
        report.sdk_inventory_sha256.to_string()
    );
    let inventory: Vec<Value> = serde_json::from_slice(&inventory_bytes).unwrap();
    let archive_alias = inventory
        .iter()
        .find(|entry| entry["path"] == "lib/libclient.a")
        .unwrap();
    assert_eq!(archive_alias["type"], "symlink");
    assert_eq!(archive_alias["target"], "client.a");
    assert_eq!(
        inventory
            .iter()
            .find(|entry| entry["path"] == "lib/client.a")
            .unwrap()["mode"],
        "0644"
    );
    let source_archive = harness
        .request
        .cmake_build_root
        .join("SYS/Developer/lib/client.a");
    let relocated_archive = harness
        .request
        .output_root
        .join("relocated-sdk/lib/client.a");
    for archive in [source_archive, relocated_archive] {
        assert_eq!(
            fs::metadata(archive).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
    let relocated_alias = harness
        .request
        .output_root
        .join("relocated-sdk/lib/libclient.a");
    assert_eq!(
        fs::read_link(relocated_alias).unwrap(),
        Path::new("client.a")
    );

    // The retained maps carry a marker written by each launched synthetic
    // driver, in addition to the SDK archive path used by the executor.
    for name in ["original-c", "relocated-c", "original-cxx", "relocated-cxx"] {
        let map = fs::read(harness.request.output_root.join(format!("{name}.map"))).unwrap();
        assert!(map.starts_with(b"synthetic-driver-executed\n"));
        assert!(String::from_utf8_lossy(&map).contains("lib/libclient.a"));
    }
}

#[test]
fn rejects_wrong_abi_without_publishing_a_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let harness = harness(temporary, DriverMode::WrongAbi);
    let result = execute_native_sdk_links(&harness.request, &CancellationToken::default());
    assert!(result.is_err(), "wrong-ABI fixture must be rejected");
    assert_no_receipt(&harness.request);
}

#[test]
fn rejects_foreign_linker_trace_without_publishing_a_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let harness = harness(temporary, DriverMode::ForeignTrace);
    let result = execute_native_sdk_links(&harness.request, &CancellationToken::default());
    assert!(result.is_err(), "foreign linker input must be rejected");
    assert_no_receipt(&harness.request);
}

#[test]
fn rejects_stale_validator_binding_before_creating_proof_output() {
    let temporary = tempfile::tempdir().unwrap();
    let harness = harness(temporary, DriverMode::Normal);
    let binding_path = harness.request.cmake_build_root.join(BINDING_FILE);
    let mut binding: Value = serde_json::from_slice(&fs::read(&binding_path).unwrap()).unwrap();
    binding["profile"] = json!("stale-source-profile");
    fs::write(&binding_path, serde_json::to_vec(&binding).unwrap()).unwrap();

    let result = execute_native_sdk_links(&harness.request, &CancellationToken::default());
    assert!(
        result.is_err(),
        "stale persisted source binding must be rejected"
    );
    assert!(!harness.request.output_root.exists());
    assert_no_receipt(&harness.request);
}

#[test]
fn refuses_existing_output_root_without_overwriting_or_publishing_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let harness = harness(temporary, DriverMode::Normal);
    fs::create_dir(&harness.request.output_root).unwrap();
    let sentinel = harness.request.output_root.join("operator-owned.txt");
    fs::write(&sentinel, b"leave this output alone\n").unwrap();

    let result = execute_native_sdk_links(&harness.request, &CancellationToken::default());
    assert!(result.is_err(), "pre-existing output root must be refused");
    assert_eq!(fs::read(&sentinel).unwrap(), b"leave this output alone\n");
    assert_no_receipt(&harness.request);
}

#[test]
fn readback_rejects_tampered_members_and_extra_output() {
    for mutation in [
        ProofMutation::Elf,
        ProofMutation::Map,
        ProofMutation::Report,
        ProofMutation::Inventory,
        ProofMutation::ExtraMember,
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let harness = harness(temporary, DriverMode::Normal);
        let report = execute_native_sdk_links(&harness.request, &CancellationToken::default())
            .expect("baseline synthetic proof should be created");
        match mutation {
            ProofMutation::Elf => append_byte(&harness.request.output_root.join("original-c.elf")),
            ProofMutation::Map => append_byte(&harness.request.output_root.join("original-c.map")),
            ProofMutation::Report => append_byte(
                &harness
                    .request
                    .output_root
                    .join("reports/standalone-c.report.json"),
            ),
            ProofMutation::Inventory => {
                append_byte(&harness.request.output_root.join(INVENTORY_FILE));
            }
            ProofMutation::ExtraMember => fs::write(
                harness.request.output_root.join("unexpected-proof-member"),
                b"unexpected\n",
            )
            .unwrap(),
        }
        assert!(
            readback_native_sdk_links(&harness.request, &report).is_err(),
            "readback must reject {mutation:?}"
        );
    }
}

#[test]
fn rejects_compiler_library_fallback_even_when_another_sdk_archive_is_traced() {
    let temporary = tempfile::tempdir().unwrap();
    let harness = harness(temporary, DriverMode::LibraryFallback);
    let result = execute_native_sdk_links(&harness.request, &CancellationToken::default());
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("source-declared native SDK library"),
        "unexpected fallback rejection: {message}"
    );
    assert_no_receipt(&harness.request);
}

#[test]
fn rejects_later_cpp_driver_mutation_of_earlier_c_output_before_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let harness = harness(temporary, DriverMode::LaterCxxMutatesEarlierElf);
    let result = execute_native_sdk_links(&harness.request, &CancellationToken::default());
    assert!(
        result.is_err(),
        "cross-language evidence mutation must fail"
    );
    assert_no_receipt(&harness.request);
}

#[derive(Clone, Copy)]
enum DriverMode {
    Normal,
    WrongAbi,
    ForeignTrace,
    LibraryFallback,
    LaterCxxMutatesEarlierElf,
}

#[derive(Debug, Clone, Copy)]
enum ProofMutation {
    Elf,
    Map,
    Report,
    Inventory,
    ExtraMember,
}

fn harness(temporary: tempfile::TempDir, mode: DriverMode) -> Harness {
    let root = temporary.path();
    let mut compatibility_request =
        crate::compatibility::execution::native_sdk_links_test_request(root);
    let source_root = compatibility_request.preparation.source_root.clone();
    let source_profile = source_profile(&source_root);
    let contract_path = source_root.join(CONTRACT_FILE);
    let contract_bytes = fs::read(&contract_path).unwrap();
    let mut contract: Value = serde_json::from_slice(&contract_bytes).unwrap();
    assert_eq!(contract["schema"], "aros-native-consumer-contract-v1");

    let c_source = b"#include <fixture.h>\nint main(void) { return fixture_value(); }\n";
    let cxx_source = b"#include <fixture.h>\nint main() { return fixture_value(); }\n";
    fs::write(source_root.join("sdk-probe.c"), c_source).unwrap();
    fs::write(source_root.join("sdk-probe.cpp"), cxx_source).unwrap();
    contract["schema"] = json!("aros-native-consumer-contract-v2");
    contract["native_sdk_link_probes"] = json!({
        "c": {"source": "sdk-probe.c", "libraries": ["client"]},
        "cxx": {"source": "sdk-probe.cpp", "libraries": ["client"]}
    });
    let inputs = contract["inputs"].as_array_mut().unwrap();
    inputs.push(json!({"path": "sdk-probe.c", "sha256": sha256_bytes(c_source)}));
    inputs.push(json!({"path": "sdk-probe.cpp", "sha256": sha256_bytes(cxx_source)}));
    fs::write(
        &contract_path,
        serde_json::to_vec_pretty(&contract).unwrap(),
    )
    .unwrap();

    compatibility_request.preparation.source_tree_sha256 = measure_tree_content_cas(&source_root)
        .unwrap()
        .payload_digest_excluding(None);
    let loaded = load_bound_native_consumer_contract(
        &source_root,
        Path::new(CONTRACT_FILE),
        &source_profile,
    )
    .unwrap();
    let probes = loaded.contract.require_native_sdk_link_probes().unwrap();
    assert_eq!(probes.c.source, "sdk-probe.c");
    assert_eq!(probes.cxx.source, "sdk-probe.cpp");

    let compiler = compatibility_request
        .relocation
        .second
        .verified
        .manifest
        .compiler
        .clone()
        .unwrap();
    let compiler_root = compatibility_request.relocation.second.root.clone();
    let layout = ToolchainToolLayout::load(&compiler_root).unwrap();
    let tools = layout.resolve_tools(&compiler_root).unwrap();
    let c_driver = tools
        .iter()
        .find(|(role, _)| *role == "c")
        .unwrap()
        .1
        .clone();
    let cxx_driver = tools
        .iter()
        .find(|(role, _)| *role == "cxx")
        .unwrap()
        .1
        .clone();

    let good_elf = root.join("native-sdk-good.elf");
    let wrong_abi_elf = root.join("native-sdk-wrong-abi.elf");
    let target = match &compiler {
        ArosCompilerIdentity::Gnu { target, .. } => target,
        ArosCompilerIdentity::Llvm { .. } => panic!("fixture must use GNU"),
    };
    fs::write(
        &good_elf,
        crate::compatibility::fixture_riscv_elf(
            elf::Class::Elf32,
            "native_sdk_application_fixture",
            target,
            false,
        ),
    )
    .unwrap();
    fs::write(
        &wrong_abi_elf,
        crate::compatibility::fixture_riscv_elf(
            elf::Class::Elf32,
            "native_sdk_application_fixture",
            target,
            true,
        ),
    )
    .unwrap();
    let foreign_trace = root.join("foreign-input.a");
    fs::write(&foreign_trace, b"foreign input outside SDK/compiler/tmp\n").unwrap();
    let compiler_fallback = if matches!(mode, DriverMode::LibraryFallback) {
        let path = compiler_root.join("lib/fallback/libclient.a");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"synthetic compiler fallback archive\n").unwrap();
        Some(path)
    } else {
        None
    };

    let selected_elf = match mode {
        DriverMode::Normal
        | DriverMode::ForeignTrace
        | DriverMode::LibraryFallback
        | DriverMode::LaterCxxMutatesEarlierElf => &good_elf,
        DriverMode::WrongAbi => &wrong_abi_elf,
    };
    let foreign = matches!(mode, DriverMode::ForeignTrace).then_some(foreign_trace.as_path());
    write_driver(
        &c_driver,
        selected_elf,
        foreign,
        compiler_fallback.as_deref(),
        matches!(mode, DriverMode::LaterCxxMutatesEarlierElf),
    );
    write_driver(
        &cxx_driver,
        selected_elf,
        foreign,
        compiler_fallback.as_deref(),
        matches!(mode, DriverMode::LaterCxxMutatesEarlierElf),
    );
    refresh_helper_identity(&mut compatibility_request, &loaded, &source_profile);

    let build_root = compatibility_request.cmake_build_root.clone();
    fs::create_dir_all(build_root.join("SYS/Developer/include")).unwrap();
    fs::create_dir_all(build_root.join("SYS/Developer/lib")).unwrap();
    fs::write(
        build_root.join("SYS/Developer/include/fixture.h"),
        b"static inline int fixture_value(void) { return 0; }\n",
    )
    .unwrap();
    if matches!(mode, DriverMode::LibraryFallback) {
        fs::write(
            build_root.join("SYS/Developer/lib/other.a"),
            b"unrelated SDK archive\n",
        )
        .unwrap();
    } else {
        fs::write(
            build_root.join("SYS/Developer/lib/client.a"),
            b"synthetic client archive\n",
        )
        .unwrap();
        fs::set_permissions(
            build_root.join("SYS/Developer/lib/client.a"),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        symlink("client.a", build_root.join("SYS/Developer/lib/libclient.a")).unwrap();
    }

    let source_dir = source_root.canonicalize().unwrap();
    let binding = json!({
        "schema": "aros-native-consumer-validation-v1",
        "qualification": "source-binding-not-graph-or-build-proof",
        "source_dir": source_dir.to_string_lossy(),
        "contract_path": loaded.path.to_string_lossy(),
        "contract_sha256": loaded.sha256,
        "profile": loaded.contract.profile,
        "abi": loaded.contract.abi,
        "exec_smp": false,
        "input_paths": loaded.contract.inputs.iter().map(|input| input.path.clone()).collect::<Vec<_>>(),
        "sdk_include_relative": "SYS/Developer/include"
    });
    fs::write(
        build_root.join(BINDING_FILE),
        serde_json::to_vec_pretty(&binding).unwrap(),
    )
    .unwrap();

    let output_root = root.join("native-sdk-link-proof");
    let request = NativeSdkLinkRequest {
        preparation: compatibility_request.preparation,
        source_profile,
        cmake_build_root: build_root,
        compiler_root,
        compiler,
        output_root,
        timeout: Duration::from_secs(10),
    };
    Harness {
        _temporary: temporary,
        request,
    }
}

fn source_profile(source_root: &Path) -> TargetProfile {
    let source_config = fs::read_to_string(source_root.join("aros-targets.toml")).unwrap();
    TargetProfile::parse_config(&source_config, "aros-targets.toml")
        .unwrap()
        .targets
        .into_iter()
        .find(|profile| profile.native_consumer_contract.as_deref() == Some(CONTRACT_FILE))
        .unwrap()
}

fn refresh_helper_identity(
    request: &mut crate::compatibility::NativeCompatibilityRequest,
    loaded: &aros_common::native_consumer_contract::LoadedNativeConsumerContract,
    source_profile: &TargetProfile,
) {
    let input_paths = loaded
        .contract
        .inputs
        .iter()
        .map(|input| input.path.clone())
        .collect::<Vec<_>>();
    let exec_smp = loaded
        .contract
        .generated_make_templates
        .values()
        .filter_map(|item| item.substitutions.get("@ENABLE_EXECSMP@"))
        .any(|value| value == "#define __AROSEXEC_SMP__");
    let binding = json!({
        "schema": "aros-native-consumer-validation-v1",
        "qualification": "source-binding-not-graph-or-build-proof",
        "source_dir": request.preparation.source_root.canonicalize().unwrap().to_string_lossy(),
        "contract_path": loaded.path.to_string_lossy(),
        "contract_sha256": loaded.sha256,
        "profile": loaded.contract.profile,
        "abi": loaded.contract.abi,
        "exec_smp": exec_smp,
        "input_paths": input_paths,
        "sdk_include_relative": "SYS/Developer/include"
    });
    let rendered = serde_json::to_string(&binding).unwrap();
    let helper = request
        .preparation
        .helpers
        .get_mut("aros-transpiler")
        .unwrap();
    let script = format!("#!/bin/sh\nprintf '%s\\n' {}\n", shell_quote(&rendered));
    fs::write(&helper.path, script.as_bytes()).unwrap();
    fs::set_permissions(&helper.path, fs::Permissions::from_mode(0o755)).unwrap();
    helper.sha256 = sha256_bytes(script.as_bytes());
    helper.size = script.len() as u64;

    // The response is source-profile-specific; this asserts the fixture helper
    // was derived from the selected declaration rather than a board constant.
    assert_eq!(
        source_profile.native_consumer_contract.as_deref(),
        Some(CONTRACT_FILE)
    );
}

fn write_driver(
    path: &Path,
    elf_fixture: &Path,
    foreign_trace: Option<&Path>,
    compiler_fallback: Option<&Path>,
    mutate_earlier_c_elf: bool,
) {
    let fixture = shell_quote(&elf_fixture.to_string_lossy());
    let trace_override =
        foreign_trace.map_or_else(String::new, |path| shell_quote(&path.to_string_lossy()));
    let trace_expression = if foreign_trace.is_some() {
        format!("trace={trace_override}")
    } else if let Some(fallback) = compiler_fallback {
        format!(
            "trace=$(printf '%s\\n%s\\n' \"$sysroot/lib/other.a\" {})",
            shell_quote(&fallback.to_string_lossy())
        )
    } else {
        "trace=$sysroot/lib/libclient.a".into()
    };
    let sdk_library_check = if compiler_fallback.is_some() {
        "[ -f \"$sysroot/lib/other.a\" ]"
    } else {
        "[ -f \"$sysroot/lib/libclient.a\" ]"
    };
    let later_mutation = if mutate_earlier_c_elf {
        "case \"$source\" in *.cpp) proof_root=${out%/*}; printf 'tampered by later C++ driver\\n' > \"$proof_root/original-c.elf\" || exit 47 ;; esac"
    } else {
        ""
    };
    let script = format!(
        "#!/bin/sh\n\
         [ \"$PATH\" = /nonexistent ] || exit 41\n\
         sysroot= map= out= source= library= previous=\n\
         for arg do\n\
         case \"$arg\" in\n\
         --sysroot=*) sysroot=${{arg#--sysroot=}} ;;\n\
         -Wl,-Map=*,--cref,--trace) map=${{arg#-Wl,-Map=}}; map=${{map%,--cref,--trace}} ;;\n\
         -lclient) library=yes ;;\n\
         *.c|*.cpp) source=$arg ;;\n\
         -c|-r|-nostdlib|-nostartfiles|-nodefaultlibs) exit 42 ;;\n\
         esac\n\
         if [ \"$previous\" = -o ]; then out=$arg; fi\n\
         previous=$arg\n\
         done\n\
         [ -n \"$sysroot\" ] && [ -n \"$map\" ] && [ -n \"$out\" ] && [ -n \"$source\" ] && [ \"$library\" = yes ] || exit 43\n\
         [ -f \"$source\" ] && [ -f \"$sysroot/include/fixture.h\" ] && {sdk_library_check} || exit 44\n\
         {trace_expression}\n\
         printf 'synthetic-driver-executed\\n%s\\n' \"$trace\" > \"$map\" || exit 45\n\
         printf '%s\\n' \"$trace\"\n\
         /bin/cp {fixture} \"$out\" || exit 46\n\
         {later_mutation}\n"
    );
    fs::write(path, script.as_bytes()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn assert_no_receipt(request: &NativeSdkLinkRequest) {
    assert!(!request.output_root.join(RECEIPT_FILE).exists());
}

fn append_byte(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    bytes.push(b'!');
    fs::write(path, bytes).unwrap();
}
