use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const PROFILE_WITH_CONTRACT: &str = "[[targets]]\nname='example-sdk'\narch='riscv64'\nplatform='opensbi'\nbsp='example'\nfloat_abi='lp64d'\nnative_consumer_contract='consumer.json'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='gnu'\ncpu32=''\nuse_mmu=true\nmesa_version='26.0.0'\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=true\n";
const PROFILE_WITHOUT_CONTRACT: &str = "[[targets]]\nname='example-sdk'\narch='riscv64'\nplatform='opensbi'\nbsp='example'\nfloat_abi='lp64d'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='gnu'\ncpu32=''\nuse_mmu=true\nmesa_version='26.0.0'\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=true\n";
const C_PROBE: &[u8] = b"/* sealed C contract fixture */\n";
const CXX_PROBE: &[u8] = b"/* sealed C++ contract fixture */\n";

fn write_fixture(with_contract: bool, schema_v2: bool) -> (tempfile::TempDir, String) {
    let directory = tempfile::tempdir().unwrap();
    let profile_text = if with_contract {
        PROFILE_WITH_CONTRACT
    } else {
        PROFILE_WITHOUT_CONTRACT
    };
    fs::write(directory.path().join("aros-targets.toml"), profile_text).unwrap();
    fs::write(directory.path().join("policy.json"), "{}").unwrap();

    let mut inputs = vec![
        json!({
            "path": "aros-targets.toml",
            "sha256": aros_common::sha256_bytes(profile_text.as_bytes()),
        }),
        json!({
            "path": "policy.json",
            "sha256": aros_common::sha256_bytes(b"{}"),
        }),
    ];
    let mut document = json!({
        "schema": if schema_v2 {
            "aros-native-consumer-contract-v2"
        } else {
            "aros-native-consumer-contract-v1"
        },
        "profile": "example-sdk",
        "source_baseline": "0123456789abcdef0123456789abcdef01234567",
        "roots": ["includes", "linklibs"],
        "metamake_projection": "policy.json",
        "inputs": inputs,
        "abi": {
            "source_cpu": "riscv64",
            "target_triple": "riscv64-aros",
            "isa": "rva22u64",
            "abi": "lp64d",
            "code_model": "medany",
            "flavour": "native",
            "platform_smp": true,
            "use_mmu": true,
        },
    });

    if schema_v2 {
        fs::write(directory.path().join("probe.c"), C_PROBE).unwrap();
        fs::write(directory.path().join("probe.cpp"), CXX_PROBE).unwrap();
        inputs = document["inputs"].as_array().unwrap().clone();
        inputs.extend([
            json!({"path": "probe.c", "sha256": aros_common::sha256_bytes(C_PROBE)}),
            json!({"path": "probe.cpp", "sha256": aros_common::sha256_bytes(CXX_PROBE)}),
        ]);
        document["inputs"] = Value::Array(inputs);
        document["native_sdk_link_probes"] = json!({
            "c": {"source": "probe.c", "libraries": []},
            "cxx": {"source": "probe.cpp", "libraries": []},
        });
    }

    if with_contract {
        fs::write(
            directory.path().join("consumer.json"),
            serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
    }
    (directory, profile_text.to_owned())
}

fn source_snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

#[test]
fn explicit_epoch_accepts_zero_and_portable_maximum_for_supported_contracts() {
    let source = tempfile::tempdir().unwrap();
    let (selected, _) = write_fixture(true, true);
    for epoch in [
        Some(0),
        Some(aros_common::sdk_manifest::SDK_MAX_SOURCE_DATE_EPOCH),
    ] {
        super::preflight_consumer_epoch(source.path(), None, epoch).unwrap();
        super::preflight_consumer_epoch(selected.path(), Some("example-sdk"), epoch).unwrap();
    }
}

#[test]
fn generic_sdk_contract_is_rejected_even_with_an_explicit_epoch_without_mutation() {
    let (source, text) = write_fixture(false, false);
    let text = text.replace(
        "[targets.transpiler]",
        "sdk_consumer_contract='sdk-consumer.json'\n[targets.transpiler]",
    );
    fs::write(source.path().join("aros-targets.toml"), text).unwrap();
    let before = source_snapshot(source.path());
    for epoch in [None, Some(0), Some(946_684_800)] {
        let error =
            super::preflight_consumer_epoch(source.path(), Some("example-sdk"), epoch).unwrap_err();
        assert!(error
            .to_string()
            .contains("does not yet support a generic SDK consumer contract"));
        assert_eq!(source_snapshot(source.path()), before);
    }
}

#[test]
fn sdk_compiler_selection_rejects_unsupported_family_and_profile_before_preparation() {
    use aros_toolchain::source_lock::CompilerFamily;

    super::preflight_sdk_compiler_selection(CompilerFamily::Gnu, "compiler", "compiler").unwrap();
    let error =
        super::preflight_sdk_compiler_selection(CompilerFamily::Llvm, "compiler", "compiler")
            .unwrap_err();
    assert!(error.to_string().contains("GNU package"));
    let error = super::preflight_sdk_compiler_selection(CompilerFamily::Gnu, "compiler", "other")
        .unwrap_err();
    assert!(error.to_string().contains("different compiler profile"));
}

#[test]
fn sdk_forbidden_prefixes_are_validated_but_not_treated_as_owned_roots() {
    let directory = tempfile::tempdir().unwrap();
    let selected_input = directory.path().join("selected-input");
    fs::create_dir(&selected_input).unwrap();
    let package_output = directory.path().join("package-output");

    // A byte-scan pattern may be a namespace ancestor of the output, provided
    // the actual selected inputs and execution roots remain disjoint.
    super::preflight_sdk_destination(
        "package output",
        &package_output,
        &[selected_input.as_path()],
        &[directory.path().to_path_buf()],
    )
    .unwrap();

    let error = super::preflight_sdk_destination(
        "package output",
        &package_output,
        &[selected_input.as_path()],
        &[PathBuf::from("relative-prefix")],
    )
    .unwrap_err();
    assert!(error.to_string().contains("absolute paths"));
}

#[test]
fn missing_epoch_rejects_v2_before_mutating_source_or_creating_output_roots() {
    let (source, _) = write_fixture(true, true);
    let before = source_snapshot(source.path());
    let error = super::preflight_consumer_epoch(source.path(), Some("example-sdk"), None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("--consumer-source-date-epoch"), "{error}");
    assert_eq!(source_snapshot(source.path()), before);
    // This preflight receives no output-root paths; its failure must leave the
    // source fixture as the only filesystem state it can affect.
    assert_eq!(fs::read_dir(source.path()).unwrap().count(), before.len());
}

#[test]
fn legacy_v1_no_contract_and_unselected_source_keep_missing_epoch_compatible() {
    let (v1, _) = write_fixture(true, false);
    super::preflight_consumer_epoch(v1.path(), Some("example-sdk"), None).unwrap();

    let (without_contract, _) = write_fixture(false, false);
    super::preflight_consumer_epoch(without_contract.path(), Some("example-sdk"), None).unwrap();

    let empty_source = tempfile::tempdir().unwrap();
    super::preflight_consumer_epoch(empty_source.path(), None, None).unwrap();
}

#[test]
fn unsealed_probe_and_unknown_preset_are_rejected() {
    let (source, _) = write_fixture(true, true);
    fs::write(
        source.path().join("probe.c"),
        b"/* changed after sealing */\n",
    )
    .unwrap();
    assert!(super::preflight_consumer_epoch(source.path(), Some("example-sdk"), None).is_err());

    let (source, _) = write_fixture(true, false);
    let error = super::preflight_consumer_epoch(source.path(), Some("unknown"), None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("source preset is not declared"), "{error}");
}

#[test]
fn out_of_range_epoch_is_rejected_even_when_explicit() {
    let source = tempfile::tempdir().unwrap();
    let too_large = aros_common::sdk_manifest::SDK_MAX_SOURCE_DATE_EPOCH + 1;
    let error =
        super::preflight_consumer_epoch(source.path(), Some("example-sdk"), Some(too_large))
            .unwrap_err()
            .to_string();
    assert!(error.contains("portable tar timestamp range"), "{error}");
}
