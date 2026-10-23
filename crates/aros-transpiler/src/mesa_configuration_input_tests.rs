//! Synthetic child data consistency, not a configured-source execution proof.

use super::*;
use aros_common::mesa_configuration::{
    ValidatedMesaConfiguration, MESA_BASELINE_VARIABLES, MESA_COMPILER_OPTION_VARIABLES,
};
use aros_common::mesa_configuration_input::{MesaConfigurationInputDocument, INPUT_FILE};
use aros_common::{measure_tree_content_cas, sha256_bytes, Sha256Digest};
use clap::Parser;
use serde_json::json;
use std::fs;

struct Fixture {
    _temporary: tempfile::TempDir,
    source: PathBuf,
    input: PathBuf,
    output: PathBuf,
    digest: Sha256Digest,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("source");
        let input_root = root.join("preflight");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("mmakefile.src"), "# synthetic source\n").unwrap();
        let generated = input_root.join("configure/gen");
        fs::create_dir_all(generated.join("config")).unwrap();
        let identity = aros_common::mesa_configuration::MesaConfigurationIdentity {
            target_profile: "arm-raspi".into(),
            target_triple: "arm-unknown-aros".into(),
            compiler: ArosCompilerIdentity::Llvm {
                version: "11.0.0".into(),
            },
            compiler_tree_sha256: sha256_bytes(b"synthetic compiler identity"),
            source_inputs_sha256: measure_tree_content_cas(&source)
                .unwrap()
                .payload_digest_excluding(None),
            configure_invocation_sha256: sha256_bytes(b"synthetic configure identity"),
        };
        let baseline = MESA_BASELINE_VARIABLES
            .iter()
            .map(|name| ((*name).into(), json!([])))
            .collect::<serde_json::Map<_, _>>();
        let options = MESA_COMPILER_OPTION_VARIABLES
            .iter()
            .map(|name| ((*name).into(), json!([])))
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
        let bytes = serde_json::to_vec(&json!({
            "version":1, "identity":identity, "baseline_variables":baseline,
            "undefined_variables":["CUR_MESADIR"],
            "recipe_configuration": {
                "make_coordinates": {
                    "source_root": source,
                    "build_root": input_root.join("configure"),
                    "ports_root": input_root.join("configure/Ports"),
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
        let configuration = ValidatedMesaConfiguration::parse_bound(&bytes, &identity).unwrap();
        let compiler_cfg = b"synthetic compiler configuration\n";
        let receipt = serde_json::to_vec(&json!({
            "schema":"aros-mesa-preflight-v1", "qualification":"local-configure-only",
            "identity":identity, "configuration_sha256":sha256_bytes(&bytes),
            "compiler_cfg_sha256":sha256_bytes(compiler_cfg),
            "configured_paths":{"GENDIR":generated,"PORTSDIR":input_root.join("configure/Ports")}, "make_values":configuration.make_variables()
        }))
        .unwrap();
        fs::write(input_root.join("mesa-configuration.json"), &bytes).unwrap();
        fs::write(input_root.join("receipt.json"), &receipt).unwrap();
        fs::write(generated.join("config/compiler.cfg"), compiler_cfg).unwrap();
        let document = serde_json::to_vec(&MesaConfigurationInputDocument {
            version: 1,
            local_source: None,
            genmodule: None,
            genmodule_collection: None,
            identity,
            configuration_sha256: sha256_bytes(&bytes),
            receipt_sha256: sha256_bytes(&receipt),
        })
        .unwrap();
        let input = input_root.join(INPUT_FILE);
        fs::write(&input, &document).unwrap();
        let output = root.join("generated.cmake");
        Self {
            _temporary: temporary,
            source,
            input,
            output,
            digest: sha256_bytes(&document),
        }
    }

    fn args(&self) -> Args {
        Args::try_parse_from([
            "aros-transpiler",
            "--source-dir",
            self.source.to_str().unwrap(),
            "--output",
            self.output.to_str().unwrap(),
            "--cpu",
            "arm",
            "--toolchain",
            "llvm",
            "--mesa-version",
            "26.0.0",
            "--mesa-configuration-input",
            self.input.to_str().unwrap(),
            "--mesa-configuration-input-sha256",
            self.digest.as_str(),
        ])
        .unwrap()
    }

    fn select_local_build(&mut self, preset: &str) {
        for args in [
            vec!["init", "-q"],
            vec!["add", "mmakefile.src"],
            vec!["commit", "-qm", "Synthetic Mesa source"],
        ] {
            let output = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=Mesa fixture",
                    "-c",
                    "user.email=mesa@example.invalid",
                    "-C",
                ])
                .arg(&self.source)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fs::create_dir_all(self.source.join("build").join(preset)).unwrap();
        let local =
            aros_common::local_source::LocalSourceIdentity::capture(&self.source, preset).unwrap();
        let root = self.input.parent().unwrap();
        let mut config: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("mesa-configuration.json")).unwrap())
                .unwrap();
        config["identity"]["source_inputs_sha256"] = json!(local.content_sha256);
        let bytes = serde_json::to_vec(&config).unwrap();
        fs::write(root.join("mesa-configuration.json"), &bytes).unwrap();
        let mut receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("receipt.json")).unwrap()).unwrap();
        receipt["schema"] = json!("aros-mesa-preflight-v2");
        receipt["identity"] = config["identity"].clone();
        receipt["configuration_sha256"] = json!(sha256_bytes(&bytes));
        let receipt_bytes = serde_json::to_vec(&receipt).unwrap();
        fs::write(root.join("receipt.json"), &receipt_bytes).unwrap();
        let input = MesaConfigurationInputDocument {
            version: 2,
            local_source: Some(local),
            genmodule: None,
            genmodule_collection: None,
            identity: serde_json::from_value(config["identity"].clone()).unwrap(),
            configuration_sha256: sha256_bytes(&bytes),
            receipt_sha256: sha256_bytes(&receipt_bytes),
        };
        let bytes = serde_json::to_vec(&input).unwrap();
        fs::write(&self.input, &bytes).unwrap();
        self.digest = sha256_bytes(&bytes);
        self.output = self
            .source
            .join("build")
            .join(preset)
            .join("generated.cmake");
    }
}

#[test]
fn rehashed_configuration_cannot_relabel_the_selected_make_source_root() {
    let mut fixture = Fixture::new();
    assert!(load(&fixture.args()).unwrap().is_some());
    let root = fixture.input.parent().unwrap();
    let configuration_path = root.join("mesa-configuration.json");
    let mut configuration: serde_json::Value =
        serde_json::from_slice(&fs::read(&configuration_path).unwrap()).unwrap();
    configuration["recipe_configuration"]["make_coordinates"]["source_root"] =
        json!("/other-source");
    let configuration = serde_json::to_vec(&configuration).unwrap();
    fs::write(configuration_path, &configuration).unwrap();
    let receipt_path = root.join("receipt.json");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    receipt["configuration_sha256"] = json!(sha256_bytes(&configuration));
    let receipt = serde_json::to_vec(&receipt).unwrap();
    fs::write(receipt_path, &receipt).unwrap();
    let mut document: MesaConfigurationInputDocument =
        serde_json::from_slice(&fs::read(&fixture.input).unwrap()).unwrap();
    document.configuration_sha256 = sha256_bytes(&configuration);
    document.receipt_sha256 = sha256_bytes(&receipt);
    let document = serde_json::to_vec(&document).unwrap();
    fs::write(&fixture.input, &document).unwrap();
    fixture.digest = sha256_bytes(&document);
    let Err(error) = load(&fixture.args()) else {
        panic!("mismatched nominal source root accepted");
    };
    assert!(
        error.to_string().contains("Make source root differs"),
        "{error}"
    );
}

#[test]
fn graph_projection_requires_templates_only_for_selected_mesa_targets() {
    use aros_transpiler::capability::mesa::configured_graph;
    use aros_transpiler::{dirs::DirVars, DependencyGraph, TargetContext};

    let fixture = Fixture::new();
    let binding = load(&fixture.args()).unwrap().unwrap();
    let recipe = fixture.output.with_extension("src");
    fs::write(
        &recipe,
        "%build_linklib mmake=ordinary libname=ordinary files=probe\n",
    )
    .unwrap();
    let ordinary = aros_transpiler::parse_mmakefile(&recipe, recipe.parent().unwrap())
        .unwrap()
        .targets
        .pop()
        .unwrap();
    let target = TargetContext {
        cpu: Some("arm".into()),
        platform: Some("raspi".into()),
        toolchain: Some("llvm".into()),
        cpu32: Some(String::new()),
        use_mmu: Some("1".into()),
        float_abi: Some("hard".into()),
        mesa_version: Some("26.0.0".into()),
        ..TargetContext::default()
    };
    let configuration = binding.input.configuration();
    let coordinates = &configuration.recipe_configuration().make_coordinates;
    let dirs = DirVars::load(&fixture.source);
    for selected in [false, true] {
        let mut declaration = ordinary.clone();
        if selected {
            declaration.mmake_name = "hidd-gallium".into();
            declaration.dir_path = "workbench/hidds/gallium".into();
        }
        let mut graph = DependencyGraph::new();
        graph.add_target(declaration);
        assert_eq!(
            configured_graph::required(&graph, &target).unwrap(),
            selected
        );
        let result = configured_graph::project(
            &mut graph,
            &fixture.source,
            &dirs,
            &target,
            configuration,
            std::path::Path::new(&coordinates.build_root),
            std::path::Path::new(&coordinates.ports_root),
        );
        if selected {
            assert!(result
                .unwrap_err()
                .contains("cannot resolve reviewed target-option templates"));
            assert!(
                graph.configured_mesa.is_none(),
                "failed projection must stay atomic"
            );
        } else {
            assert!(result.unwrap().is_empty());
            assert!(graph.configured_mesa.unwrap().targets.is_empty());
        }
    }
}

#[test]
fn staged_projection_rechecks_present_and_absent_inventory_before_output_replacement() {
    use aros_transpiler::capability::mesa::configured_inventory::InventoryObservation;

    for initially_present in [false, true] {
        let fixture = Fixture::new();
        let ports = fixture.output.parent().unwrap().join("Ports");
        fs::create_dir_all(&ports).unwrap();
        let inventory = ports.join("Makefile.sources");
        if initially_present {
            fs::write(&inventory, b"C_SOURCES = original.c\n").unwrap();
        }
        let mut binding = load(&fixture.args()).unwrap().unwrap();
        binding
            .inventories
            .push(InventoryObservation::capture(&inventory, &ports).unwrap());
        let mut publication = Publication::for_output(&fixture.output);
        publication.present(fixture.output.clone(), "original graph");
        stage(Some(&binding), &mut publication, &fixture.output).unwrap();
        publication.publish().unwrap();
        let sidecar = fixture
            .output
            .with_extension("mesa-configuration-binding.json");
        let original_sidecar = fs::read(&sidecar).unwrap();

        fs::write(&inventory, b"C_SOURCES = changed.c\n").unwrap();
        let mut publication = Publication::for_output(&fixture.output);
        publication.present(fixture.output.clone(), "unpublished changed graph");
        assert!(stage(Some(&binding), &mut publication, &fixture.output).is_err());
        assert_eq!(fs::read(&fixture.output).unwrap(), b"original graph");
        assert_eq!(fs::read(&sidecar).unwrap(), original_sidecar);
    }
}

#[test]
fn local_input_accepts_only_its_selected_build_and_rechecks_source() {
    let mut fixture = Fixture::new();
    fixture.select_local_build("fixture");
    let mut args = fixture.args();
    args.build_dir = Some(fixture.source.join("build/fixture"));
    let binding = load(&args).unwrap();
    fs::write(
        fixture.source.join("build/fixture/intermediate"),
        b"generated",
    )
    .unwrap();
    let mut publication = Publication::for_output(&fixture.output);
    publication.present(fixture.output.clone(), "local graph");
    stage(binding.as_ref(), &mut publication, &fixture.output).unwrap();
    publication.publish().unwrap();
    let report: serde_json::Value = serde_json::from_slice(
        &fs::read(
            fixture
                .output
                .with_extension("mesa-configuration-binding.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(report["schema"], "aros-mesa-transpiler-binding-v2");
    assert_eq!(report["local_source"]["generated_subtree"], "build/fixture");
    for output in [
        fixture.source.join("graph.cmake"),
        fixture.source.join("build/fixture-copy/graph.cmake"),
    ] {
        let mut wrong = fixture.args();
        wrong.output = output;
        assert!(load(&wrong).is_err());
    }
    // Keep the selected source snapshot stable until this deliberate counter.
    fs::write(fixture.source.join("untracked.c"), b"changed source").unwrap();
    let mut publication = Publication::for_output(&fixture.output);
    assert!(stage(binding.as_ref(), &mut publication, &fixture.output).is_err());
    assert_eq!(fs::read(&fixture.output).unwrap(), b"local graph");
}

#[test]
fn local_input_refuses_mixed_versions_and_mismatched_source_identity() {
    for mutation in [
        "downgrade",
        "missing-local",
        "source-digest",
        "receipt-schema",
    ] {
        let mut fixture = Fixture::new();
        fixture.select_local_build("fixture");
        let mut document: serde_json::Value =
            serde_json::from_slice(&fs::read(&fixture.input).unwrap()).unwrap();
        match mutation {
            "downgrade" => document["version"] = json!(1),
            "missing-local" => {
                document.as_object_mut().unwrap().remove("local_source");
            }
            "source-digest" => {
                document["local_source"]["content_sha256"] =
                    json!(sha256_bytes(b"wrong local source"));
            }
            "receipt-schema" => {
                let path = fixture.input.parent().unwrap().join("receipt.json");
                let mut receipt: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                receipt["schema"] = json!("aros-mesa-preflight-v1");
                let bytes = serde_json::to_vec(&receipt).unwrap();
                fs::write(path, &bytes).unwrap();
                document["receipt_sha256"] = json!(sha256_bytes(&bytes));
            }
            _ => unreachable!(),
        }
        let bytes = serde_json::to_vec(&document).unwrap();
        fs::write(&fixture.input, &bytes).unwrap();
        fixture.digest = sha256_bytes(&bytes);
        assert!(
            load(&fixture.args()).is_err(),
            "{mutation} must reject even with recomputed file digests"
        );
        assert!(!fixture.output.exists());
    }
}

#[test]
fn carries_data_without_claiming_execution_and_removes_stale_binding_on_omission() {
    let fixture = Fixture::new();
    let args = fixture.args();
    let binding = load(&args).unwrap();
    assert_eq!(
        binding
            .as_ref()
            .unwrap()
            .input
            .configuration()
            .identity()
            .target_triple,
        "arm-unknown-aros"
    );
    let sidecar = fixture
        .output
        .with_extension("mesa-configuration-binding.json");
    let mut publication = Publication::for_output(&fixture.output);
    publication.present(fixture.output.clone(), "new graph");
    stage(binding.as_ref(), &mut publication, &fixture.output).unwrap();
    publication.publish().unwrap();
    let report: serde_json::Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!(report["qualification"], "data-consistency-only");
    assert_eq!(
        report["configuration_input_sha256"],
        fixture.digest.as_str()
    );
    let mut publication = Publication::for_output(&fixture.output);
    publication.present(fixture.output.clone(), "unconfigured graph");
    stage(None, &mut publication, &fixture.output).unwrap();
    publication.publish().unwrap();
    assert!(!sidecar.exists());
}

#[test]
fn rejects_wrong_selectors_source_and_output_overlap() {
    let fixture = Fixture::new();
    for selector in ["cpu", "family", "version"] {
        let mut args = fixture.args();
        match selector {
            "cpu" => args.cpu = Some("aarch64".into()),
            "family" => args.toolchain = Some("gnu".into()),
            "version" => args.mesa_version = Some("20.0.8".into()),
            _ => unreachable!(),
        }
        assert!(load(&args).is_err());
    }
    let mut args = fixture.args();
    args.build_dir = Some(fixture.source.join("build"));
    assert!(load(&args).is_err());
    let mut args = fixture.args();
    args.output = fixture.input.parent().unwrap().join("graph.cmake");
    assert!(load(&args).is_err());
    for build in [
        fixture.input.parent().unwrap().to_path_buf(),
        fixture.input.parent().unwrap().join("build"),
        fixture
            .input
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf(),
    ] {
        let mut args = fixture.args();
        args.build_dir = Some(build);
        assert!(
            load(&args).is_err(),
            "build must not overlap preflight inputs"
        );
    }
    fs::write(fixture.source.join("changed"), "changed source").unwrap();
    assert!(load(&fixture.args()).is_err());
}

#[test]
fn rejects_mid_pass_input_or_source_drift_before_publication() {
    for change_source in [false, true] {
        let fixture = Fixture::new();
        fs::write(&fixture.output, b"previous graph").unwrap();
        let binding = load(&fixture.args()).unwrap();
        let changed = if change_source {
            fixture.source.join("drift")
        } else {
            fixture.input.parent().unwrap().join("drift")
        };
        fs::write(changed, "unexpected drift").unwrap();
        let mut publication = Publication::for_output(&fixture.output);
        publication.present(fixture.output.clone(), "invalid new graph");
        assert!(stage(binding.as_ref(), &mut publication, &fixture.output).is_err());
        assert_eq!(fs::read(&fixture.output).unwrap(), b"previous graph");
    }
}

#[test]
fn requires_the_paired_arguments_and_keeps_omission_compatible() {
    assert!(Args::try_parse_from([
        "aros-transpiler",
        "--mesa-configuration-input",
        "/data/input"
    ])
    .is_err());
    assert!(Args::try_parse_from([
        "aros-transpiler",
        "--mesa-configuration-input-sha256",
        "abc"
    ])
    .is_err());
    let args = Args::try_parse_from(["aros-transpiler"]).unwrap();
    assert!(load(&args).unwrap().is_none());
}
