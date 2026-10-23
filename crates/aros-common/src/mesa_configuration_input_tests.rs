use super::{
    load_mesa_configuration_input, MesaConfigurationInputDocument, MesaGenmoduleBinding,
    INPUT_FILE, MESA_GENMODULE_BASE_CONFIG, MESA_GENMODULE_MAKE_FILE,
    MESA_GENMODULE_OVERRIDE_CONFIG,
};
use crate::local_source::LocalSourceIdentity;
use crate::mesa_configuration::{
    MesaConfigurationIdentity, ValidatedMesaConfiguration, MESA_BASELINE_VARIABLES,
    MESA_COMPILER_OPTION_VARIABLES,
};
use crate::mesa_genmodule_collection::{
    mesa_genmodule_output_relative_path, MesaGenmoduleCollectionBinding,
    MesaGenmoduleOutputBinding, MESA_GENMODULE_SELECTION_FILE,
};
use crate::mesa_genmodule_inventory::{
    MesaGenmoduleInventory, MesaGenmoduleInventoryRecord, MesaGenmoduleInventoryScope,
    MesaGenmoduleSelectionBinding, MesaGenmoduleSelectionDocument, MESA_GENMODULE_INVENTORY_SCHEMA,
    MESA_GENMODULE_SELECTION_SCHEMA,
};
use crate::{sha256_bytes, ArosCompilerIdentity, ArosError, Sha256Digest};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[cfg(unix)]
#[path = "mesa_configuration_input_tests/compiler_programs.rs"]
mod compiler_programs;

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::parse(&byte.to_string().repeat(64)).unwrap()
}

struct Fixture {
    _temporary: TempDir,
    root: PathBuf,
    identity: MesaConfigurationIdentity,
    configuration_bytes: Vec<u8>,
    source_root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let gendir = root.join("configure").join("gendir");
        fs::create_dir_all(gendir.join("config")).unwrap();
        let source_root = root.join("source");
        fs::create_dir_all(&source_root).unwrap();

        let identity = MesaConfigurationIdentity {
            target_profile: "aarch64".into(),
            target_triple: "aarch64-unknown-aros".into(),
            compiler: ArosCompilerIdentity::Llvm {
                version: "18.1.0".into(),
            },
            compiler_tree_sha256: digest('a'),
            source_inputs_sha256: digest('b'),
            configure_invocation_sha256: digest('c'),
        };
        let baseline_variables = MESA_BASELINE_VARIABLES
            .iter()
            .map(|name| ((*name).to_owned(), json!([])))
            .collect::<serde_json::Map<_, _>>();
        let compiler_options = MESA_COMPILER_OPTION_VARIABLES
            .iter()
            .map(|name| ((*name).to_owned(), json!([])))
            .collect::<serde_json::Map<_, _>>();
        let selectors = [
            "CFLAGS_NO_BUILTIN_FFS",
            "CFLAGS_NO_BUILTIN_FFSLL",
            "CFLAGS_NO_BUILTIN_BSWAP32",
            "CFLAGS_NO_BUILTIN_BSWAP64",
            "CFLAGS_NO_BUILTIN_CLZ",
            "CFLAGS_NO_BUILTIN_CLZLL",
            "CFLAGS_NO_BUILTIN_CTZ",
            "CFLAGS_NO_BUILTIN_EXPECT",
            "CFLAGS_NO_BUILTIN_POPCOUNT",
            "CFLAGS_NO_BUILTIN_POPCOUNTLL",
            "CFLAGS_NO_BUILTIN_UNREACHABLE",
        ]
        .into_iter()
        .map(|name| (name.to_owned(), json!(false)))
        .collect::<serde_json::Map<_, _>>();
        let configuration = json!({
            "version": 1,
            "identity": identity,
            "undefined_variables": ["CUR_MESADIR"],
            "recipe_configuration": {
                "make_coordinates": {
                    "source_root": source_root.to_string_lossy().into_owned(),
                    "build_root": root.join("configure"),
                    "ports_root": root.join("configure/Ports"),
                    "generated_root": gendir,
                },
                "safety_cflags": {"state":"defined","value":[]}, "target_lto":"no",
                "lto_cflags":[], "cflags_iquote":["-iquote"], "cflags_iquote_end":[],
                "target_cc":"/compiler/bin/clang", "target_cxx":"/compiler/bin/clang++",
                "target_sysroot":[]
            },
            "llvm_configuration": {
                "opt_llvm": { "state": "undefined" },
                "target_llvm_ver": { "state": "defined", "value": "11.0.0" },
            },
            "baseline_variables": baseline_variables,
            "target_isa_flags": [],
            "builtin_selectors": selectors,
            "compiler_options": compiler_options,
            "mesa_compile_options": [],
        });
        let configuration_bytes = serde_json::to_vec(&configuration).unwrap();
        let parsed =
            ValidatedMesaConfiguration::parse_bound(&configuration_bytes, &identity).unwrap();
        let compiler_cfg_bytes = b"CC = clang\n";
        let receipt = json!({
            "schema": "aros-mesa-preflight-v1",
            "qualification": "local-configure-only",
            "identity": identity,
            "configuration_sha256": sha256_bytes(&configuration_bytes),
            "compiler_cfg_sha256": sha256_bytes(compiler_cfg_bytes),
            "configured_paths": { "GENDIR": gendir, "PORTSDIR": root.join("configure/Ports") },
            "make_values": parsed.make_variables(),
        });
        let receipt_bytes = serde_json::to_vec(&receipt).unwrap();

        fs::write(root.join("mesa-configuration.json"), &configuration_bytes).unwrap();
        fs::write(root.join("receipt.json"), &receipt_bytes).unwrap();
        fs::write(gendir.join("config/compiler.cfg"), compiler_cfg_bytes).unwrap();
        let input = MesaConfigurationInputDocument {
            version: 1,
            local_source: None,
            genmodule: None,
            genmodule_collection: None,
            identity: identity.clone(),
            configuration_sha256: sha256_bytes(&configuration_bytes),
            receipt_sha256: sha256_bytes(&receipt_bytes),
        };
        fs::write(root.join(INPUT_FILE), serde_json::to_vec(&input).unwrap()).unwrap();

        Self {
            _temporary: temporary,
            root,
            identity,
            configuration_bytes,
            source_root,
        }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn input_path(&self) -> PathBuf {
        self.root().join(INPUT_FILE)
    }

    fn expected_input_sha256(&self) -> Sha256Digest {
        sha256_bytes(&fs::read(self.input_path()).unwrap())
    }

    fn write_receipt_and_refresh_input(&self, receipt: &Value) -> Sha256Digest {
        let receipt_bytes = serde_json::to_vec(&receipt).unwrap();
        fs::write(self.root().join("receipt.json"), &receipt_bytes).unwrap();
        let input = MesaConfigurationInputDocument {
            version: 1,
            local_source: None,
            genmodule: None,
            genmodule_collection: None,
            identity: self.identity.clone(),
            configuration_sha256: sha256_bytes(&self.configuration_bytes),
            receipt_sha256: sha256_bytes(&receipt_bytes),
        };
        let input_bytes = serde_json::to_vec(&input).unwrap();
        fs::write(self.input_path(), &input_bytes).unwrap();
        sha256_bytes(&input_bytes)
    }

    fn write_input(
        &self,
        version: u32,
        identity: MesaConfigurationIdentity,
        configuration_sha256: Sha256Digest,
        receipt_sha256: Sha256Digest,
    ) -> Sha256Digest {
        let input = MesaConfigurationInputDocument {
            version,
            local_source: None,
            genmodule: None,
            genmodule_collection: None,
            identity,
            configuration_sha256,
            receipt_sha256,
        };
        let bytes = serde_json::to_vec(&input).unwrap();
        fs::write(self.input_path(), &bytes).unwrap();
        sha256_bytes(&bytes)
    }

    fn write_genmodule(
        &self,
        version: u32,
        local_source: Option<LocalSourceIdentity>,
        makefile: &[u8],
    ) -> (Sha256Digest, MesaGenmoduleBinding) {
        for relative in [MESA_GENMODULE_BASE_CONFIG, MESA_GENMODULE_OVERRIDE_CONFIG] {
            fs::create_dir_all(self.source_root.join(relative).parent().unwrap()).unwrap();
        }
        let base = b"RELLIBS = z1 posixc stdc\n";
        let override_config = b"RELLIBS = z1 posixc stdc\nRELLINKLIB = true\n";
        fs::write(self.source_root.join(MESA_GENMODULE_BASE_CONFIG), base).unwrap();
        fs::write(
            self.source_root.join(MESA_GENMODULE_OVERRIDE_CONFIG),
            override_config,
        )
        .unwrap();
        fs::write(self.root().join(MESA_GENMODULE_MAKE_FILE), makefile).unwrap();

        let binding = MesaGenmoduleBinding {
            module: "mesa3dgl26-0".into(),
            makefile_sha256: sha256_bytes(makefile),
            base_configuration_sha256: sha256_bytes(base),
            override_configuration_sha256: sha256_bytes(override_config),
            producer_sha256: digest('d'),
        };
        let mut receipt = receipt_value(self);
        receipt["schema"] = json!(format!("aros-mesa-preflight-v{version}"));
        receipt["genmodule"] = serde_json::to_value(&binding).unwrap();
        let expected =
            self.write_genmodule_receipt(version, local_source, binding.clone(), &receipt);
        (expected, binding)
    }

    fn write_genmodule_receipt(
        &self,
        version: u32,
        local_source: Option<LocalSourceIdentity>,
        binding: MesaGenmoduleBinding,
        receipt: &Value,
    ) -> Sha256Digest {
        let receipt_bytes = serde_json::to_vec(receipt).unwrap();
        fs::write(self.root().join("receipt.json"), &receipt_bytes).unwrap();
        let input = MesaConfigurationInputDocument {
            version,
            local_source,
            genmodule: Some(binding),
            genmodule_collection: None,
            identity: self.identity.clone(),
            configuration_sha256: sha256_bytes(&self.configuration_bytes),
            receipt_sha256: sha256_bytes(&receipt_bytes),
        };
        let input_bytes = serde_json::to_vec(&input).unwrap();
        fs::write(self.input_path(), &input_bytes).unwrap();
        sha256_bytes(&input_bytes)
    }

    fn write_collection(
        &self,
        version: u32,
        local_source: Option<LocalSourceIdentity>,
    ) -> (Sha256Digest, MesaGenmoduleCollectionBinding) {
        let owner = "mesa3dgl26-library";
        let base_bytes = b"RELLIBS = z1 posixc stdc\n";
        let override_bytes = b"RELLIBS = z1 posixc stdc\nRELLINKLIB = true\n";
        fs::create_dir_all(
            self.source_root
                .join(MESA_GENMODULE_BASE_CONFIG)
                .parent()
                .unwrap(),
        )
        .unwrap();
        fs::create_dir_all(
            self.source_root
                .join(MESA_GENMODULE_OVERRIDE_CONFIG)
                .parent()
                .unwrap(),
        )
        .unwrap();
        fs::write(
            self.source_root.join(MESA_GENMODULE_BASE_CONFIG),
            base_bytes,
        )
        .unwrap();
        fs::write(
            self.source_root.join(MESA_GENMODULE_OVERRIDE_CONFIG),
            override_bytes,
        )
        .unwrap();

        let producer = crate::mesa_genmodule_inventory::MesaGenmoduleProducer {
            owner: owner.into(),
            declaration_directory: "workbench/libs/mesa".into(),
            module: "mesa3dgl26".into(),
            module_type: "library".into(),
            base_configuration: MESA_GENMODULE_BASE_CONFIG.into(),
            override_configuration: Some(MESA_GENMODULE_OVERRIDE_CONFIG.into()),
        };
        let raw_make = String::from_utf8(genmodule_makefile())
            .unwrap()
            .replace("mesa3dgl26-0", &producer.module)
            .into_bytes();
        let output_relative = mesa_genmodule_output_relative_path(&producer.owner).unwrap();
        fs::create_dir_all(self.root.join(output_relative.parent().unwrap())).unwrap();
        fs::write(self.root.join(&output_relative), &raw_make).unwrap();

        let selection = MesaGenmoduleSelectionBinding {
            profile: "fixture-source-profile".into(),
            kind: "sdk-consumer".into(),
            scope: MesaGenmoduleInventoryScope::ContractRoots,
            contract_sha256: digest('e'),
        };
        let selection_document = MesaGenmoduleSelectionDocument {
            schema: MESA_GENMODULE_SELECTION_SCHEMA.into(),
            selection: selection.clone(),
            inventory: MesaGenmoduleInventory {
                schema: MESA_GENMODULE_INVENTORY_SCHEMA.into(),
                records: vec![MesaGenmoduleInventoryRecord {
                    owner: producer.owner.clone(),
                    declaration_directory: producer.declaration_directory.clone(),
                    module: producer.module.clone(),
                    module_type: producer.module_type.clone(),
                    base_configuration: producer.base_configuration.clone(),
                    override_configuration: producer.override_configuration.clone(),
                    consumer_path: format!("runtime/{}", producer.owner),
                    components: Vec::new(),
                }],
            },
        };
        let inventory_bytes = serde_json::to_vec(&selection_document).unwrap();
        fs::write(
            self.root.join(MESA_GENMODULE_SELECTION_FILE),
            &inventory_bytes,
        )
        .unwrap();
        let binding = MesaGenmoduleCollectionBinding {
            selection,
            inventory_sha256: sha256_bytes(&inventory_bytes),
            helper_sha256: digest('f'),
            producer_sha256: digest('d'),
            outputs: vec![MesaGenmoduleOutputBinding {
                producer,
                makefile_sha256: sha256_bytes(&raw_make),
                base_configuration_sha256: sha256_bytes(base_bytes),
                override_configuration_sha256: Some(sha256_bytes(override_bytes)),
            }],
        };
        let mut receipt = receipt_value(self);
        receipt["schema"] = json!(format!("aros-mesa-preflight-v{version}"));
        receipt["genmodule_collection"] = serde_json::to_value(&binding).unwrap();
        let receipt_bytes = serde_json::to_vec(&receipt).unwrap();
        fs::write(self.root.join("receipt.json"), &receipt_bytes).unwrap();
        let input = MesaConfigurationInputDocument {
            version,
            local_source,
            genmodule: None,
            genmodule_collection: Some(binding.clone()),
            identity: self.identity.clone(),
            configuration_sha256: sha256_bytes(&self.configuration_bytes),
            receipt_sha256: sha256_bytes(&receipt_bytes),
        };
        let input_bytes = serde_json::to_vec(&input).unwrap();
        fs::write(self.input_path(), &input_bytes).unwrap();
        (sha256_bytes(&input_bytes), binding)
    }
}

fn genmodule_makefile() -> Vec<u8> {
    b"mesa3dgl26-0_STARTFILES += mesa3dgl26-0_start\n\
      mesa3dgl26-0_ENDFILES += mesa3dgl26-0_end\n\
      mesa3dgl26-0_MODDIR += Libs\n\
      mesa3dgl26-0_LINKLIBFILES += stub_a stub_b\n\
      mesa3dgl26-0_RELLINKLIBFILES += relstub_a relstub_b\n\
      mesa3dgl26-0_LINKLIBAFILES +=\n\
      mesa3dgl26-0_RELLINKLIBAFILES +=\n\
      mesa3dgl26-0_INCLUDES += proto.h\n\
      mesa3dgl26-0_CPPFLAGS += -D__Z1_RELLIBBASE__ -D__MESA3DGL26_0_NOLIBBASE__\n\
      mesa3dgl26-0_LINKLIBCPPFLAGS += -D__Z1_RELLIBBASE__\n\
      mesa3dgl26-0_CFLAGS +=\n\
      mesa3dgl26-0_LINKLIBCFLAGS +=\n\
      mesa3dgl26-0_CXXFLAGS +=\n\
      mesa3dgl26-0_LINKLIBCXXFLAGS +=\n\
      mesa3dgl26-0_LDFLAGS +=\n\
      mesa3dgl26-0_LIBS += z1_rel stdc_rel\n"
        .to_vec()
}

fn receipt_value(fixture: &Fixture) -> Value {
    serde_json::from_slice(&fs::read(fixture.root().join("receipt.json")).unwrap()).unwrap()
}

#[test]
fn legacy_receipts_refuse_native_status_and_explicit_null_fields() {
    let fixture = Fixture::new();
    let original = receipt_value(&fixture);
    for mutation in [
        json!({"native_make": null}),
        json!({"native_make": {"version": 1}}),
        json!({"stages": [{"stdout":digest('1'), "exit_status":null}]}),
        json!({"stages": [{"stdout":digest('1'), "exit_status":0}]}),
    ] {
        let mut receipt = original.clone();
        for (name, value) in mutation.as_object().unwrap() {
            receipt[name] = value.clone();
        }
        let expected = fixture.write_receipt_and_refresh_input(&receipt);
        assert!(load_mesa_configuration_input(&fixture.input_path(), &expected).is_err());
    }
    let expected = fixture.write_receipt_and_refresh_input(&original);
    load_mesa_configuration_input(&fixture.input_path(), &expected).unwrap();
}

#[test]
fn native_versions_cannot_relabel_legacy_collection_data() {
    let fixture = Fixture::new();
    for version in [9, 10] {
        let local = (version == 10).then(|| LocalSourceIdentity {
            schema: "aros-local-source-v1".into(),
            head_baseline: "a".repeat(40),
            submodules_sha256: digest('e'),
            generated_subtree: "build/mesa-test".into(),
            content_sha256: fixture.identity.source_inputs_sha256.clone(),
            entry_count: 1,
            regular_file_bytes: 1,
        });
        let _ = fixture.write_collection(version, local);
        let expected = fixture.expected_input_sha256();
        let error = load_mesa_configuration_input(&fixture.input_path(), &expected).unwrap_err();
        assert!(
            error.to_string().contains("native binding/status"),
            "{error}"
        );
    }
}

#[cfg(unix)]
#[test]
fn native_inputs_join_real_make_packets_and_recheck_the_prepared_tree() {
    use crate::make_initial_values::ValidatedMakeInitialValues;
    use crate::make_namespace::ValidatedMakeNamespace;
    use crate::mesa_native_make::{
        native_preflight_stage_names, tests::actual_native_make_tests::NativeMakeFixture,
    };
    use std::os::unix::fs::PermissionsExt;
    for version in [9, 10] {
        let mut fixture = Fixture::new();
        let local = (version == 10).then(|| LocalSourceIdentity {
            schema: "aros-local-source-v1".into(),
            head_baseline: "a".repeat(40),
            submodules_sha256: digest('e'),
            generated_subtree: "build/mesa-test".into(),
            content_sha256: fixture.identity.source_inputs_sha256.clone(),
            entry_count: 1,
            regular_file_bytes: 1,
        });
        let (_, collection) = fixture.write_collection(version, local);
        let stdout = b"AROS_MAKE_INITIAL_NAMESPACE=v1:3;.VARIABLES|default;MAKE_VERSION|default;SOURCE_RAW|file;end\nAROS_MAKE_INITIAL_VALUES=v1;MAKE_VERSION|default|simple|4.4.1;SOURCE_RAW|file|recursive|$(LATER);end\n";
        let namespace = ValidatedMakeNamespace::from_make_stdout(stdout).unwrap();
        let values = ValidatedMakeInitialValues::from_make_stdout(stdout, &namespace).unwrap();
        let mut configuration: Value =
            serde_json::from_slice(&fixture.configuration_bytes).unwrap();
        configuration["version"] = json!(3);
        configuration["initial_namespace"] =
            serde_json::to_value(namespace.to_observation()).unwrap();
        configuration["initial_values"] = serde_json::to_value(values.to_observation()).unwrap();
        fixture.configuration_bytes = serde_json::to_vec(&configuration).unwrap();
        fs::write(
            fixture.root.join("mesa-configuration.json"),
            &fixture.configuration_bytes,
        )
        .unwrap();
        let parsed = ValidatedMesaConfiguration::parse_bound(
            &fixture.configuration_bytes,
            &fixture.identity,
        )
        .unwrap();
        let producers: Vec<_> = collection
            .outputs
            .iter()
            .map(|output| output.producer.clone())
            .collect();
        let mut native = NativeMakeFixture::from_selection(
            &fixture.identity,
            &sha256_bytes(stdout),
            &parsed.recipe_configuration().make_coordinates,
            &producers,
        );
        native.binding.prepared_tree_sha256 =
            crate::measure_tree_content_cas(&fixture.root.join("configure"))
                .unwrap()
                .payload_digest_excluding(None);
        for (name, bytes) in &native.files {
            let path = fixture.root.join(name);
            fs::write(&path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let names = native_preflight_stage_names(&producers).unwrap();
        let mut stages = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let output = if index == 3 { stdout.as_slice() } else { b"" };
            fs::write(fixture.root.join(format!("{name}.stdout")), output).unwrap();
            fs::write(fixture.root.join(format!("{name}.stderr")), b"").unwrap();
            let program = if index == 7 {
                &collection.helper_sha256
            } else if index >= 9 {
                &collection.producer_sha256
            } else {
                &native.binding.parent_program_sha256
            };
            let mut stage = json!({"program":program, "invocation":native.binding.parent_invocation_sha256,
                "environment":native.binding.parent_environment_sha256, "stdout":sha256_bytes(output), "stderr":sha256_bytes(b"")});
            if (4..7).contains(&index) {
                stage["exit_status"] = json!(if index == 4 { 2 } else { 0 });
            }
            stages.push(stage);
        }
        let mut receipt = receipt_value(&fixture);
        receipt["configuration_sha256"] = json!(sha256_bytes(&fixture.configuration_bytes));
        receipt["native_make"] = serde_json::to_value(&native.binding).unwrap();
        receipt["stages"] = json!(stages);
        let receipt_bytes = serde_json::to_vec(&receipt).unwrap();
        fs::write(fixture.root.join("receipt.json"), &receipt_bytes).unwrap();
        let mut input: Value =
            serde_json::from_slice(&fs::read(fixture.input_path()).unwrap()).unwrap();
        input["configuration_sha256"] = json!(sha256_bytes(&fixture.configuration_bytes));
        input["receipt_sha256"] = json!(sha256_bytes(&receipt_bytes));
        fs::write(fixture.input_path(), serde_json::to_vec(&input).unwrap()).unwrap();
        let expected = fixture.expected_input_sha256();
        let loaded = load_mesa_configuration_input(&fixture.input_path(), &expected).unwrap();
        assert!(loaded
            .native_make()
            .unwrap()
            .observation_for_owner(&producers[0].owner)
            .unwrap()
            .is_some());
        for (index, field, value, message) in [
            (4, "exit_status", json!(0), "query/build stage identity"),
            (6, "invocation", json!(digest('8')), "launch stage identity"),
            (8, "program", json!(digest('9')), "generator stage identity"),
            (
                9,
                "exit_status",
                json!(0),
                "status outside its native stages",
            ),
        ] {
            let mut changed_receipt = receipt.clone();
            changed_receipt["stages"][index][field] = value;
            let bytes = serde_json::to_vec(&changed_receipt).unwrap();
            fs::write(fixture.root.join("receipt.json"), &bytes).unwrap();
            let mut changed_input = input.clone();
            changed_input["receipt_sha256"] = json!(sha256_bytes(&bytes));
            fs::write(
                fixture.input_path(),
                serde_json::to_vec(&changed_input).unwrap(),
            )
            .unwrap();
            let error = load_mesa_configuration_input(
                &fixture.input_path(),
                &fixture.expected_input_sha256(),
            )
            .unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
        let mut missing_owner_stage = receipt.clone();
        missing_owner_stage["stages"].as_array_mut().unwrap().pop();
        let mut missing_status = receipt.clone();
        missing_status["stages"][4]
            .as_object_mut()
            .unwrap()
            .remove("exit_status");
        for (changed_receipt, message) in [
            (missing_owner_stage, "exact configured stage sequence"),
            (missing_status, "lacks explicit exit status"),
        ] {
            let bytes = serde_json::to_vec(&changed_receipt).unwrap();
            fs::write(fixture.root.join("receipt.json"), &bytes).unwrap();
            let mut changed_input = input.clone();
            changed_input["receipt_sha256"] = json!(sha256_bytes(&bytes));
            fs::write(
                fixture.input_path(),
                serde_json::to_vec(&changed_input).unwrap(),
            )
            .unwrap();
            let error = load_mesa_configuration_input(
                &fixture.input_path(),
                &fixture.expected_input_sha256(),
            )
            .unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
        fs::write(fixture.root.join("receipt.json"), &receipt_bytes).unwrap();
        let mut mixed_input = input.clone();
        // The opposite version expects the opposite source-identity shape.
        // Rehashing the top-level document must not bypass that boundary.
        mixed_input["version"] = json!(if version == 9 { 10 } else { 9 });
        fs::write(
            fixture.input_path(),
            serde_json::to_vec(&mixed_input).unwrap(),
        )
        .unwrap();
        let error =
            load_mesa_configuration_input(&fixture.input_path(), &fixture.expected_input_sha256())
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported Mesa configuration input version"),
            "{error}"
        );
        fs::write(fixture.input_path(), serde_json::to_vec(&input).unwrap()).unwrap();
        fs::write(
            fixture.root.join("configure/changed.mk"),
            b"changed prepared input",
        )
        .unwrap();
        assert!(loaded.revalidate().is_err());
        let error = load_mesa_configuration_input(&fixture.input_path(), &expected).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("native prepared input tree differs"),
            "{error}"
        );
    }
}

fn assert_configuration_error(result: crate::Result<super::LoadedMesaConfigurationInput>) {
    match result {
        Err(ArosError::Configuration { file, message }) => {
            assert!(!file.is_empty());
            assert!(!message.is_empty());
        }
        Err(error) => panic!("expected a contextual configuration error, got {error}"),
        Ok(_) => panic!("expected Mesa input loading to fail"),
    }
}

#[test]
fn initial_values_cannot_be_changed_by_rebinding_only_configuration_and_receipt() {
    use crate::make_initial_values::ValidatedMakeInitialValues;
    use crate::make_namespace::ValidatedMakeNamespace;
    let mut fixture = Fixture::new();
    let stdout = b"AROS_MAKE_INITIAL_NAMESPACE=v1:3;.VARIABLES|default;MAKE_VERSION|default;SOURCE_RAW|file;end\nAROS_MAKE_INITIAL_VALUES=v1;MAKE_VERSION|default|simple|4.4.1;SOURCE_RAW|file|recursive|$(LATER);end\n";
    let namespace = ValidatedMakeNamespace::from_make_stdout(stdout).unwrap();
    let values = ValidatedMakeInitialValues::from_make_stdout(stdout, &namespace).unwrap();
    let mut configuration: Value = serde_json::from_slice(&fixture.configuration_bytes).unwrap();
    configuration["version"] = json!(3);
    configuration["initial_namespace"] = serde_json::to_value(namespace.to_observation()).unwrap();
    configuration["initial_values"] = serde_json::to_value(values.to_observation()).unwrap();
    fixture.configuration_bytes = serde_json::to_vec(&configuration).unwrap();
    fs::write(
        fixture.root().join("mesa-configuration.json"),
        &fixture.configuration_bytes,
    )
    .unwrap();
    let mut receipt = receipt_value(&fixture);
    receipt["configuration_sha256"] = json!(sha256_bytes(&fixture.configuration_bytes));
    receipt["stages"] = json!([
        {"stdout":digest('1')}, {"stdout":digest('2')}, {"stdout":digest('3')},
        {"stdout":sha256_bytes(stdout)}
    ]);
    fs::write(fixture.root().join("configured-values.stdout"), stdout).unwrap();
    let expected = fixture.write_receipt_and_refresh_input(&receipt);
    load_mesa_configuration_input(&fixture.input_path(), &expected).unwrap();
    configuration["initial_values"]["entries"][1]["value"] = json!("forged literal");
    fixture.configuration_bytes = serde_json::to_vec(&configuration).unwrap();
    fs::write(
        fixture.root().join("mesa-configuration.json"),
        &fixture.configuration_bytes,
    )
    .unwrap();
    receipt["configuration_sha256"] = json!(sha256_bytes(&fixture.configuration_bytes));
    let rebound = fixture.write_receipt_and_refresh_input(&receipt);
    assert!(
        load_mesa_configuration_input(&fixture.input_path(), &rebound)
            .err()
            .unwrap()
            .to_string()
            .contains("initial values differ from retained Make stdout")
    );
}

#[test]
fn namespace_requires_matching_retained_stdout_and_stage_digest() {
    use crate::make_namespace::ValidatedMakeNamespace;
    let mut fixture = Fixture::new();
    let stdout = b"AROS_MAKE_INITIAL_NAMESPACE=v1:2;.VARIABLES|default;MAKE_VERSION|default;end\n";
    let namespace = ValidatedMakeNamespace::from_make_stdout(stdout).unwrap();
    let mut configuration: Value = serde_json::from_slice(&fixture.configuration_bytes).unwrap();
    configuration["version"] = json!(2);
    configuration["initial_namespace"] = serde_json::to_value(namespace.to_observation()).unwrap();
    fixture.configuration_bytes = serde_json::to_vec(&configuration).unwrap();
    fs::write(
        fixture.root().join("mesa-configuration.json"),
        &fixture.configuration_bytes,
    )
    .unwrap();
    let mut receipt = receipt_value(&fixture);
    receipt["configuration_sha256"] = json!(sha256_bytes(&fixture.configuration_bytes));
    receipt["stages"] = json!([
        {"stdout":digest('1')}, {"stdout":digest('2')}, {"stdout":digest('3')},
        {"stdout":sha256_bytes(stdout)}
    ]);
    fs::write(fixture.root().join("configured-values.stdout"), stdout).unwrap();
    let expected = fixture.write_receipt_and_refresh_input(&receipt);
    load_mesa_configuration_input(&fixture.input_path(), &expected).unwrap();
    let alternate = b"AROS_MAKE_INITIAL_NAMESPACE=v1:3;.VARIABLES|default;MAKE_VERSION|default;EXTRA|file;end\n";
    fs::write(fixture.root().join("configured-values.stdout"), alternate).unwrap();
    assert!(
        load_mesa_configuration_input(&fixture.input_path(), &expected)
            .err()
            .unwrap()
            .to_string()
            .contains("namespace Make stdout differs")
    );
    receipt["stages"][3]["stdout"] = json!(sha256_bytes(alternate));
    let rebound = fixture.write_receipt_and_refresh_input(&receipt);
    assert!(
        load_mesa_configuration_input(&fixture.input_path(), &rebound)
            .err()
            .unwrap()
            .to_string()
            .contains("configuration namespace differs")
    );
    receipt["stages"] = json!([]);
    let missing = fixture.write_receipt_and_refresh_input(&receipt);
    assert!(
        load_mesa_configuration_input(&fixture.input_path(), &missing)
            .err()
            .unwrap()
            .to_string()
            .contains("namespace requires the exact configured stage sequence")
    );
}

#[test]
fn loads_bound_configuration_and_revalidates_the_complete_root() {
    let fixture = Fixture::new();
    let expected = fixture.expected_input_sha256();
    let input_path = fixture.input_path().canonicalize().unwrap();
    let loaded = load_mesa_configuration_input(&input_path, &expected).unwrap();

    assert_eq!(loaded.input_sha256(), &expected);
    assert_eq!(
        loaded.receipt_sha256(),
        &sha256_bytes(&fs::read(fixture.root().join("receipt.json")).unwrap())
    );
    let canonical_root = fixture.root().canonicalize().unwrap();
    assert_eq!(loaded.root(), canonical_root.as_path());
    assert_eq!(loaded.configuration().identity(), &fixture.identity);
    assert_eq!(
        loaded
            .configuration()
            .undefined_variables()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["CUR_MESADIR"]
    );
    assert!(!loaded
        .configuration()
        .make_variables()
        .contains_key("CUR_MESADIR"));
    assert_eq!(
        loaded.configuration().make_variables()["TARGET_ISA_CFLAGS"],
        ""
    );
    loaded.revalidate().unwrap();
}

#[test]
fn loads_v3_genmodule_binding_and_v4_local_binding() {
    let fixture = Fixture::new();
    let (expected, binding) = fixture.write_genmodule(3, None, &genmodule_makefile());
    let loaded =
        load_mesa_configuration_input(&fixture.input_path().canonicalize().unwrap(), &expected)
            .unwrap();
    assert_eq!(loaded.genmodule_binding(), Some(&binding));
    let parsed = loaded.genmodule_makefile().unwrap();
    assert_eq!(parsed.module(), "mesa3dgl26-0");
    assert_eq!(
        parsed.values("CPPFLAGS").unwrap().join(" "),
        "-D__Z1_RELLIBBASE__ -D__MESA3DGL26_0_NOLIBBASE__"
    );
    assert_eq!(
        parsed.values("LINKLIBCPPFLAGS").unwrap().join(" "),
        "-D__Z1_RELLIBBASE__"
    );
    assert!(loaded.local_source().is_none());

    let fixture = Fixture::new();
    let local_source = LocalSourceIdentity {
        schema: "aros-local-source-v1".into(),
        head_baseline: "a".repeat(40),
        submodules_sha256: digest('e'),
        generated_subtree: "build/mesa-test".into(),
        content_sha256: fixture.identity.source_inputs_sha256.clone(),
        entry_count: 1,
        regular_file_bytes: 1,
    };
    let (expected, binding) =
        fixture.write_genmodule(4, Some(local_source.clone()), &genmodule_makefile());
    let loaded =
        load_mesa_configuration_input(&fixture.input_path().canonicalize().unwrap(), &expected)
            .unwrap();
    assert_eq!(loaded.genmodule_binding(), Some(&binding));
    assert_eq!(loaded.local_source(), Some(&local_source));
}

#[test]
fn loads_v5_and_v6_multi_producer_collection_bindings() {
    for version in [5, 6] {
        let fixture = Fixture::new();
        let local_source = (version == 6).then(|| LocalSourceIdentity {
            schema: "aros-local-source-v1".into(),
            head_baseline: "a".repeat(40),
            submodules_sha256: digest('e'),
            generated_subtree: "build/mesa-test".into(),
            content_sha256: fixture.identity.source_inputs_sha256.clone(),
            entry_count: 1,
            regular_file_bytes: 1,
        });
        let (expected, binding) = fixture.write_collection(version, local_source.clone());
        let loaded =
            load_mesa_configuration_input(&fixture.input_path().canonicalize().unwrap(), &expected)
                .unwrap();
        let collection = loaded.genmodule_collection().unwrap();
        assert_eq!(collection.binding(), &binding);
        assert_eq!(collection.outputs().len(), 1);
        assert!(loaded.genmodule_makefile().is_none());
        assert!(loaded.genmodule_binding().is_none());
        assert_eq!(loaded.local_source(), local_source.as_ref());
    }
}

#[test]
fn requires_collection_only_for_v5_v6_and_matches_receipt_binding() {
    let fixture = Fixture::new();
    fixture.write_collection(5, None);

    let mut receipt = receipt_value(&fixture);
    receipt["genmodule_collection"]["producer_sha256"] = json!(digest('a'));
    let receipt_bytes = serde_json::to_vec(&receipt).unwrap();
    fs::write(fixture.root().join("receipt.json"), &receipt_bytes).unwrap();
    let mut input: Value =
        serde_json::from_slice(&fs::read(fixture.input_path()).unwrap()).unwrap();
    input["receipt_sha256"] = json!(sha256_bytes(&receipt_bytes));
    let input_bytes = serde_json::to_vec(&input).unwrap();
    fs::write(fixture.input_path(), &input_bytes).unwrap();
    let result = load_mesa_configuration_input(
        &fixture.input_path().canonicalize().unwrap(),
        &sha256_bytes(&input_bytes),
    );
    assert!(result.is_err(), "receipt binding drift must be rejected");

    let fixture = Fixture::new();
    let (_expected, _binding) = fixture.write_collection(5, None);
    let mut input: Value =
        serde_json::from_slice(&fs::read(fixture.input_path()).unwrap()).unwrap();
    input["version"] = json!(4);
    let bytes = serde_json::to_vec(&input).unwrap();
    fs::write(fixture.input_path(), &bytes).unwrap();
    assert!(load_mesa_configuration_input(
        &fixture.input_path().canonicalize().unwrap(),
        &sha256_bytes(&bytes),
    )
    .is_err());

    let fixture = Fixture::new();
    let _ = fixture.write_input(5, fixture.identity.clone(), digest('a'), digest('b'));
    assert!(load_mesa_configuration_input(
        &fixture.input_path().canonicalize().unwrap(),
        &fixture.expected_input_sha256(),
    )
    .is_err());

    let fixture = Fixture::new();
    let local_source = LocalSourceIdentity {
        schema: "aros-local-source-v1".into(),
        head_baseline: "a".repeat(40),
        submodules_sha256: digest('e'),
        generated_subtree: "build/mesa-test".into(),
        content_sha256: fixture.identity.source_inputs_sha256.clone(),
        entry_count: 1,
        regular_file_bytes: 1,
    };
    let _ = fixture.write_collection(6, Some(local_source));
    let mut input: Value =
        serde_json::from_slice(&fs::read(fixture.input_path()).unwrap()).unwrap();
    input
        .as_object_mut()
        .unwrap()
        .remove("genmodule_collection");
    let bytes = serde_json::to_vec(&input).unwrap();
    fs::write(fixture.input_path(), &bytes).unwrap();
    assert!(load_mesa_configuration_input(
        &fixture.input_path().canonicalize().unwrap(),
        &sha256_bytes(&bytes),
    )
    .is_err());
}

#[test]
fn rejects_genmodule_source_configuration_drift() {
    let fixture = Fixture::new();
    let (expected, _) = fixture.write_genmodule(3, None, &genmodule_makefile());
    fs::write(
        fixture.source_root.join(MESA_GENMODULE_BASE_CONFIG),
        b"RELLIBS = changed\n",
    )
    .unwrap();
    let error =
        load_mesa_configuration_input(&fixture.input_path().canonicalize().unwrap(), &expected)
            .unwrap_err();
    match error {
        ArosError::Configuration { file, message } => {
            assert_eq!(file, MESA_GENMODULE_BASE_CONFIG);
            assert!(message.contains("SHA-256 differs"), "{message}");
        }
        other => panic!("wrong refusal: {other}"),
    }
}

#[test]
fn rejects_changed_genmodule_make_bytes() {
    let fixture = Fixture::new();
    let (expected, _) = fixture.write_genmodule(3, None, &genmodule_makefile());
    fs::write(
        fixture.root().join(MESA_GENMODULE_MAKE_FILE),
        b"changed after the binding\n",
    )
    .unwrap();
    let error =
        load_mesa_configuration_input(&fixture.input_path().canonicalize().unwrap(), &expected)
            .unwrap_err();
    match error {
        ArosError::Configuration { file, message } => {
            assert_eq!(file, MESA_GENMODULE_MAKE_FILE);
            assert!(message.contains("SHA-256 differs"), "{message}");
        }
        other => panic!("wrong refusal: {other}"),
    }
}

#[test]
fn rejects_rehashed_duplicate_and_effectful_genmodule_make_data() {
    let mut duplicate = genmodule_makefile();
    duplicate.extend_from_slice(b"mesa3dgl26-0_STARTFILES += duplicate\n");
    let fixture = Fixture::new();
    let (expected, _) = fixture.write_genmodule(3, None, &duplicate);
    let error =
        load_mesa_configuration_input(&fixture.input_path().canonicalize().unwrap(), &expected)
            .unwrap_err();
    match error {
        ArosError::Configuration { message, .. } => {
            assert!(message.contains("duplicate assignment"), "{message}");
        }
        other => panic!("wrong refusal: {other}"),
    }

    let effectful = String::from_utf8(genmodule_makefile())
        .unwrap()
        .replace("z1_rel stdc_rel", "$z1_rel stdc_rel");
    let fixture = Fixture::new();
    let (expected, _) = fixture.write_genmodule(3, None, effectful.as_bytes());
    let error =
        load_mesa_configuration_input(&fixture.input_path().canonicalize().unwrap(), &expected)
            .unwrap_err();
    match error {
        ArosError::Configuration { message, .. } => {
            assert!(message.contains("nonliteral genmodule atom"), "{message}");
        }
        other => panic!("wrong refusal: {other}"),
    }
}

#[test]
fn rejects_genmodule_receipt_metadata_mismatch_and_unknown_binding_fields() {
    let fixture = Fixture::new();
    let (_, binding) = fixture.write_genmodule(3, None, &genmodule_makefile());
    let mut receipt = receipt_value(&fixture);
    receipt["genmodule"]["producer_sha256"] = json!(digest('f'));
    let expected = fixture.write_genmodule_receipt(3, None, binding.clone(), &receipt);
    let error =
        load_mesa_configuration_input(&fixture.input_path().canonicalize().unwrap(), &expected)
            .unwrap_err();
    match error {
        ArosError::Configuration { file, message } => {
            assert_eq!(file, "receipt.json");
            assert!(message.contains("genmodule binding differs"), "{message}");
        }
        other => panic!("wrong refusal: {other}"),
    }

    let mut value = serde_json::to_value(binding).unwrap();
    value["unexpected"] = json!("not allowed");
    assert!(serde_json::from_value::<MesaGenmoduleBinding>(value).is_err());
}

#[test]
fn rejects_genmodule_binding_on_legacy_versions_and_missing_new_binding() {
    let fixture = Fixture::new();
    let binding = MesaGenmoduleBinding {
        module: "mesa3dgl26-0".into(),
        makefile_sha256: digest('d'),
        base_configuration_sha256: digest('e'),
        override_configuration_sha256: digest('f'),
        producer_sha256: digest('a'),
    };
    let mut document: Value =
        serde_json::from_slice(&fs::read(fixture.input_path()).unwrap()).unwrap();
    document["genmodule"] = serde_json::to_value(binding).unwrap();
    let bytes = serde_json::to_vec(&document).unwrap();
    fs::write(fixture.input_path(), &bytes).unwrap();
    let expected = sha256_bytes(&bytes);
    let error =
        load_mesa_configuration_input(&fixture.input_path().canonicalize().unwrap(), &expected)
            .unwrap_err();
    match error {
        ArosError::Configuration { message, .. } => {
            assert!(message.contains("unsupported Mesa configuration input version"));
        }
        other => panic!("wrong refusal: {other}"),
    }

    let fixture = Fixture::new();
    let expected = fixture.write_input(
        3,
        fixture.identity.clone(),
        sha256_bytes(&fixture.configuration_bytes),
        sha256_bytes(&fs::read(fixture.root().join("receipt.json")).unwrap()),
    );
    let error =
        load_mesa_configuration_input(&fixture.input_path().canonicalize().unwrap(), &expected)
            .unwrap_err();
    assert!(matches!(error, ArosError::Configuration { .. }));
}

#[test]
fn rejects_rehashed_receipt_coordinate_disagreement_without_using_missing_files() {
    let fixture = Fixture::new();
    let mut receipt = receipt_value(&fixture);
    receipt["configured_paths"]["PORTSDIR"] = json!(fixture.root().join("configure/other-ports"));
    let expected = fixture.write_receipt_and_refresh_input(&receipt);
    let error =
        load_mesa_configuration_input(&fixture.input_path().canonicalize().unwrap(), &expected)
            .unwrap_err();
    match error {
        ArosError::Configuration { message, .. } => {
            assert!(message.contains("coordinates differ"), "{message}");
        }
        other => panic!("wrong refusal: {other}"),
    }
}

#[test]
fn rejects_changed_input_configuration_receipt_and_compiler_cfg_bytes() {
    for changed in ["input", "configuration", "receipt", "compiler.cfg"] {
        let fixture = Fixture::new();
        let expected = fixture.expected_input_sha256();
        let (path, original) = match changed {
            "input" => (
                fixture.input_path(),
                fs::read(fixture.input_path()).unwrap(),
            ),
            "configuration" => (
                fixture.root().join("mesa-configuration.json"),
                fixture.configuration_bytes.clone(),
            ),
            "receipt" => (
                fixture.root().join("receipt.json"),
                fs::read(fixture.root().join("receipt.json")).unwrap(),
            ),
            "compiler.cfg" => (
                fixture.root().join("configure/gendir/config/compiler.cfg"),
                fs::read(fixture.root().join("configure/gendir/config/compiler.cfg")).unwrap(),
            ),
            _ => unreachable!(),
        };
        let mut altered = original;
        altered.push(b' ');
        fs::write(path, altered).unwrap();
        assert_configuration_error(load_mesa_configuration_input(
            &fixture.input_path().canonicalize().unwrap(),
            &expected,
        ));
    }
}

#[test]
fn rejects_wrong_document_version_receipt_schema_and_receipt_identity() {
    let fixture = Fixture::new();
    let receipt = receipt_value(&fixture);
    let receipt_bytes = serde_json::to_vec(&receipt).unwrap();
    let wrong_version = fixture.write_input(
        2,
        fixture.identity.clone(),
        sha256_bytes(&fixture.configuration_bytes),
        sha256_bytes(&receipt_bytes),
    );
    assert_configuration_error(load_mesa_configuration_input(
        &fixture.input_path().canonicalize().unwrap(),
        &wrong_version,
    ));

    let fixture = Fixture::new();
    let mut receipt = receipt_value(&fixture);
    receipt["schema"] = json!("aros-mesa-preflight-v2");
    let expected = fixture.write_receipt_and_refresh_input(&receipt);
    assert_configuration_error(load_mesa_configuration_input(
        &fixture.input_path().canonicalize().unwrap(),
        &expected,
    ));

    let fixture = Fixture::new();
    let mut receipt = receipt_value(&fixture);
    receipt["identity"]["target_profile"] = json!("armv7");
    let expected = fixture.write_receipt_and_refresh_input(&receipt);
    assert_configuration_error(load_mesa_configuration_input(
        &fixture.input_path().canonicalize().unwrap(),
        &expected,
    ));
}

#[test]
fn rejects_configuration_identity_and_receipt_make_projection_mismatches() {
    let fixture = Fixture::new();
    let mut configuration: Value = serde_json::from_slice(&fixture.configuration_bytes).unwrap();
    configuration["identity"]["target_profile"] = json!("armv7");
    let configuration_bytes = serde_json::to_vec(&configuration).unwrap();
    fs::write(
        fixture.root().join("mesa-configuration.json"),
        &configuration_bytes,
    )
    .unwrap();
    let mut receipt = receipt_value(&fixture);
    receipt["configuration_sha256"] = json!(sha256_bytes(&configuration_bytes));
    let receipt_bytes = serde_json::to_vec(&receipt).unwrap();
    fs::write(fixture.root().join("receipt.json"), &receipt_bytes).unwrap();
    let expected = fixture.write_input(
        1,
        fixture.identity.clone(),
        sha256_bytes(&configuration_bytes),
        sha256_bytes(&receipt_bytes),
    );
    assert_configuration_error(load_mesa_configuration_input(
        &fixture.input_path().canonicalize().unwrap(),
        &expected,
    ));

    let fixture = Fixture::new();
    let mut receipt = receipt_value(&fixture);
    receipt["make_values"]["TARGET_ISA_CFLAGS"] = json!("-mavx");
    let expected = fixture.write_receipt_and_refresh_input(&receipt);
    assert_configuration_error(load_mesa_configuration_input(
        &fixture.input_path().canonicalize().unwrap(),
        &expected,
    ));
}

#[test]
fn rejects_gendir_escape_and_symlinked_gendir_or_compiler_cfg() {
    let fixture = Fixture::new();
    let external = TempDir::new().unwrap();
    fs::create_dir_all(external.path().join("gendir/config")).unwrap();
    let mut receipt = receipt_value(&fixture);
    receipt["configured_paths"]["GENDIR"] = json!(external.path().join("gendir"));
    let expected = fixture.write_receipt_and_refresh_input(&receipt);
    assert_configuration_error(load_mesa_configuration_input(
        &fixture.input_path().canonicalize().unwrap(),
        &expected,
    ));

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let outside = TempDir::new().unwrap();
        fs::create_dir_all(outside.path().join("gendir/config")).unwrap();
        fs::remove_dir_all(fixture.root().join("configure/gendir")).unwrap();
        symlink(
            outside.path().join("gendir"),
            fixture.root().join("configure/gendir"),
        )
        .unwrap();
        assert_configuration_error(load_mesa_configuration_input(
            &fixture.input_path().canonicalize().unwrap(),
            &fixture.expected_input_sha256(),
        ));

        let fixture = Fixture::new();
        let compiler_cfg = fixture.root().join("configure/gendir/config/compiler.cfg");
        fs::remove_file(&compiler_cfg).unwrap();
        let target = fixture.root().join("compiler.cfg.outside");
        fs::write(&target, b"CC = clang\n").unwrap();
        symlink(target, compiler_cfg).unwrap();
        assert_configuration_error(load_mesa_configuration_input(
            &fixture.input_path().canonicalize().unwrap(),
            &fixture.expected_input_sha256(),
        ));
    }
}

#[test]
fn rejects_missing_files_and_noncanonical_input_path() {
    for missing in [
        "mesa-configuration.json",
        "receipt.json",
        "configure/gendir/config/compiler.cfg",
    ] {
        let fixture = Fixture::new();
        let expected = fixture.expected_input_sha256();
        fs::remove_file(fixture.root().join(missing)).unwrap();
        assert_configuration_error(load_mesa_configuration_input(
            &fixture.input_path().canonicalize().unwrap(),
            &expected,
        ));
    }

    let fixture = Fixture::new();
    assert_configuration_error(load_mesa_configuration_input(
        &fixture.root().join(".").join(INPUT_FILE),
        &fixture.expected_input_sha256(),
    ));
    let wrong_leaf = fixture.root().join("different-input.json");
    fs::copy(fixture.input_path(), &wrong_leaf).unwrap();
    assert_configuration_error(load_mesa_configuration_input(
        &wrong_leaf.canonicalize().unwrap(),
        &fixture.expected_input_sha256(),
    ));

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let target = fixture.root().join("alternate-input.json");
        fs::rename(fixture.input_path(), &target).unwrap();
        symlink(&target, fixture.input_path()).unwrap();
        assert_configuration_error(load_mesa_configuration_input(
            &fixture.input_path(),
            &fixture.expected_input_sha256(),
        ));
    }
}

#[test]
fn revalidate_detects_unrelated_output_added_under_the_input_root() {
    let fixture = Fixture::new();
    let loaded = load_mesa_configuration_input(
        &fixture.input_path().canonicalize().unwrap(),
        &fixture.expected_input_sha256(),
    )
    .unwrap();
    fs::write(
        fixture.root().join("configure/new-output.txt"),
        b"changed\n",
    )
    .unwrap();
    assert!(matches!(
        loaded.revalidate(),
        Err(ArosError::Configuration { .. })
    ));
}
