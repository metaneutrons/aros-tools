//! Process-boundary checks for data-only input, not an actual preflight proof.

use aros_common::mesa_configuration::{
    ValidatedMesaConfiguration, MESA_BASELINE_VARIABLES, MESA_COMPILER_OPTION_VARIABLES,
};
use aros_common::mesa_configuration_input::{MesaConfigurationInputDocument, INPUT_FILE};
use aros_common::{
    measure_tree_content_cas, run_output_with_control, sha256_bytes, CancellationToken,
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

fn synthetic_input(
    source: &Path,
    root: &Path,
    local: Option<aros_common::local_source::LocalSourceIdentity>,
) -> (PathBuf, String) {
    let generated = root.join("configure/gen");
    fs::create_dir_all(generated.join("config")).unwrap();
    let identity = aros_common::mesa_configuration::MesaConfigurationIdentity {
        target_profile: "arm-raspi".into(),
        target_triple: "arm-unknown-aros".into(),
        compiler: aros_common::ArosCompilerIdentity::Llvm {
            version: "11.0.0".into(),
        },
        compiler_tree_sha256: sha256_bytes(b"synthetic compiler"),
        source_inputs_sha256: local.as_ref().map_or_else(
            || {
                measure_tree_content_cas(source)
                    .unwrap()
                    .payload_digest_excluding(None)
            },
            |local| local.content_sha256.clone(),
        ),
        configure_invocation_sha256: sha256_bytes(b"not an executed configure"),
    };
    let baseline = MESA_BASELINE_VARIABLES
        .iter()
        .map(|key| ((*key).into(), json!([])))
        .collect::<serde_json::Map<_, _>>();
    let options = MESA_COMPILER_OPTION_VARIABLES
        .iter()
        .map(|key| ((*key).into(), json!([])))
        .collect::<serde_json::Map<_, _>>();
    let selectors = [
        "FFS",
        "FFSLL",
        "BSWAP32",
        "BSWAP64",
        "CLZ",
        "CLZLL",
        "CTZ",
        "EXPECT",
        "POPCOUNT",
        "POPCOUNTLL",
        "UNREACHABLE",
    ]
    .into_iter()
    .map(|name| (format!("CFLAGS_NO_BUILTIN_{name}"), json!(false)))
    .collect::<serde_json::Map<_, _>>();
    let configuration = serde_json::to_vec(&json!({
        "version":1, "identity":identity, "baseline_variables":baseline,
        "undefined_variables":["CUR_MESADIR"],
            "recipe_configuration": {
                "make_coordinates": {
                    "source_root": source,
                    "build_root": root.join("configure"),
                    "ports_root": root.join("configure/Ports"),
                    "generated_root": generated,
                },
                "safety_cflags": {"state":"defined","value":[]}, "target_lto":"no",
                "lto_cflags":[], "cflags_iquote":["-iquote"], "cflags_iquote_end":[],
                "target_cc":"/compiler/bin/clang", "target_cxx":"/compiler/bin/clang++",
                "target_sysroot":[]
            },
            "llvm_configuration": {
            "opt_llvm": { "state": "undefined" },
            "target_llvm_ver": { "state": "defined", "value": "11.0.0" }
        },
        "target_isa_flags":["-marm"], "builtin_selectors":selectors,
        "compiler_options":options, "mesa_compile_options":[]
    }))
    .unwrap();
    let parsed = ValidatedMesaConfiguration::parse_bound(&configuration, &identity).unwrap();
    let compiler_cfg = b"synthetic config";
    let receipt = serde_json::to_vec(&json!({
        "schema":if local.is_some() { "aros-mesa-preflight-v2" } else { "aros-mesa-preflight-v1" }, "qualification":"local-configure-only",
        "identity":identity, "configuration_sha256":sha256_bytes(&configuration),
        "compiler_cfg_sha256":sha256_bytes(compiler_cfg),
        "configured_paths":{"GENDIR":generated,"PORTSDIR":root.join("configure/Ports")}, "make_values":parsed.make_variables()
    }))
    .unwrap();
    fs::write(root.join("mesa-configuration.json"), &configuration).unwrap();
    fs::write(root.join("receipt.json"), &receipt).unwrap();
    fs::write(generated.join("config/compiler.cfg"), compiler_cfg).unwrap();
    let input = serde_json::to_vec(&MesaConfigurationInputDocument {
        version: if local.is_some() { 2 } else { 1 },
        local_source: local,
        genmodule: None,
        genmodule_collection: None,
        identity,
        configuration_sha256: sha256_bytes(&configuration),
        receipt_sha256: sha256_bytes(&receipt),
    })
    .unwrap();
    let path = root.join(INPUT_FILE);
    fs::write(&path, &input).unwrap();
    (path, sha256_bytes(&input).to_string())
}

#[test]
fn binds_only_data_and_rejects_drift_without_replacing_existing_outputs() {
    exercise_process_binding(false);
}

#[test]
fn local_build_subtree_survives_two_process_passes_without_hiding_source_drift() {
    exercise_process_binding(true);
}

fn exercise_process_binding(local_mode: bool) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    let local = if local_mode {
        for args in [
            vec!["init", "-q"],
            vec!["commit", "--allow-empty", "-qm", "Synthetic Mesa source"],
        ] {
            let result = Command::new("git")
                .args([
                    "-c",
                    "user.name=Mesa fixture",
                    "-c",
                    "user.email=mesa@example.invalid",
                    "-C",
                ])
                .arg(&source)
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        fs::create_dir_all(source.join("build/fixture")).unwrap();
        Some(aros_common::local_source::LocalSourceIdentity::capture(&source, "fixture").unwrap())
    } else {
        None
    };
    let (input, digest) = synthetic_input(&source, &root.join("preflight"), local);
    let output = if local_mode {
        source.join("build/fixture/generated.cmake")
    } else {
        root.join("generated.cmake")
    };
    let invoke = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"));
        command.args([
            "--source-dir",
            source.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--cpu",
            "arm",
            "--toolchain",
            "llvm",
            "--mesa-version",
            "26.0.0",
            "--diagnostic-format",
            "json",
            "--mesa-configuration-input",
            input.to_str().unwrap(),
            "--mesa-configuration-input-sha256",
            &digest,
        ]);
        if local_mode {
            command.arg("--build-dir").arg(source.join("build/fixture"));
        }
        run_output_with_control(
            &mut command,
            1024 * 1024,
            Duration::from_secs(20),
            &CancellationToken::default(),
        )
        .unwrap()
    };
    for _ in 0..2 {
        let result = invoke();
        assert!(
            result.status.success() && !result.timed_out && !result.cancelled,
            "{}",
            result.stderr.rendered_lossy()
        );
        let binding: serde_json::Value = serde_json::from_slice(
            &fs::read(output.with_extension("mesa-configuration-binding.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(binding["qualification"], "data-consistency-only");
        assert_eq!(
            binding["target_selection"],
            "not-established-by-this-data-channel"
        );
        assert_eq!(binding["configuration_input_sha256"], digest);
    }
    let before = fs::read(&output).unwrap();
    let binding_before =
        fs::read(output.with_extension("mesa-configuration-binding.json")).unwrap();
    if local_mode {
        fs::write(source.join("outside-generated.c"), b"source drift").unwrap();
        let rejected = invoke();
        assert!(!rejected.status.success());
        assert_eq!(fs::read(&output).unwrap(), before);
        assert_eq!(
            fs::read(output.with_extension("mesa-configuration-binding.json")).unwrap(),
            binding_before
        );
        return;
    }
    fs::write(
        input.parent().unwrap().join("receipt.json"),
        b"changed receipt",
    )
    .unwrap();
    let rejected = invoke();
    assert!(!rejected.status.success());
    let diagnostics: serde_json::Value =
        serde_json::from_slice(rejected.stderr.exact_bytes().unwrap()).unwrap();
    assert_eq!(diagnostics["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(fs::read(&output).unwrap(), before);
    assert_eq!(
        fs::read(output.with_extension("mesa-configuration-binding.json")).unwrap(),
        binding_before
    );
}
