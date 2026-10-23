use super::*;

use std::fmt::Write as _;

use aros_common::local_source::LocalSourceIdentity;
use aros_common::mesa_configuration::{MESA_BASELINE_VARIABLES, MESA_COMPILER_OPTION_VARIABLES};
use aros_common::mesa_configuration_input::{MesaConfigurationInputDocument, MesaGenmoduleBinding};
use aros_common::mesa_genmodule_collection::{
    MesaGenmoduleCollectionBinding, MesaGenmoduleOutputBinding,
};
use aros_common::mesa_genmodule_inventory::{
    MesaGenmoduleInventory, MesaGenmoduleInventoryRecord, MesaGenmoduleInventoryScope,
    MesaGenmoduleProducer, MesaGenmoduleSelectionBinding, MesaGenmoduleSelectionDocument,
    MESA_GENMODULE_INVENTORY_SCHEMA, MESA_GENMODULE_SELECTION_SCHEMA,
};
use aros_common::mesa_genmodule_make::MESA_GENMODULE_MAKE_SUFFIXES;
use aros_common::{sha256_bytes, ArosCompilerIdentity};
use serde_json::json;

const BUILTIN_NAMES: &[&str] = &[
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
];

struct Fixture {
    files: BTreeMap<String, Vec<u8>>,
    profile_name: String,
    target_triple: String,
    profile_document_sha256: Sha256Digest,
    compiler: ArosCompilerIdentity,
    compiler_tree_sha256: Sha256Digest,
    source_tree_sha256: Sha256Digest,
    cpu: String,
    platform: String,
    clang_path: PathBuf,
    clangxx_path: PathBuf,
    output_root: PathBuf,
    genmodule_selection: Option<MesaGenmoduleSelectionBinding>,
    genmodule_helper_sha256: Option<Sha256Digest>,
}

impl Fixture {
    fn expectations(&self) -> MesaPreflightEvidenceExpectations<'_> {
        MesaPreflightEvidenceExpectations {
            profile_name: &self.profile_name,
            target_triple: &self.target_triple,
            profile_document_sha256: &self.profile_document_sha256,
            compiler: &self.compiler,
            compiler_tree_sha256: &self.compiler_tree_sha256,
            compiler_root: Some(Path::new("/selected/compiler")),
            source_tree_sha256: &self.source_tree_sha256,
            cpu: &self.cpu,
            platform: &self.platform,
            clang_path: &self.clang_path,
            clangxx_path: &self.clangxx_path,
            output_root: &self.output_root,
            genmodule_selection: self.genmodule_selection.clone(),
            genmodule_helper_sha256: self.genmodule_helper_sha256.clone(),
        }
    }
}

fn evidence_fixture() -> Fixture {
    let profile_name = "arm-raspi".to_owned();
    let target_triple = "arm-unknown-aros".to_owned();
    let profile_document_sha256 = digest("selected profiles document");
    let compiler = ArosCompilerIdentity::Llvm {
        version: "18.1.8".into(),
    };
    let compiler_tree_sha256 = digest("selected compiler tree");
    let source_tree_sha256 = digest("selected Mesa source tree");
    let cpu = "arm".to_owned();
    let platform = "raspi".to_owned();
    let clang_path = PathBuf::from("/selected/compiler/bin/clang");
    let clangxx_path = PathBuf::from("/selected/compiler/bin/clang++");
    let output_root = PathBuf::from("/tmp/mesa-preflight-evidence-run");
    let gendir = output_root.join("configure/bin/raspi-arm/gen");

    let baseline_variables: BTreeMap<_, _> = MESA_BASELINE_VARIABLES
        .iter()
        .map(|name| ((*name).to_owned(), Vec::<String>::new()))
        .collect();
    let mut compiler_options: BTreeMap<_, _> = MESA_COMPILER_OPTION_VARIABLES
        .iter()
        .map(|name| ((*name).to_owned(), Vec::<String>::new()))
        .collect();
    compiler_options.insert("CFLAGS_GNU11".into(), vec!["-std=gnu11".into()]);
    let builtin_selectors: BTreeMap<_, _> = BUILTIN_NAMES
        .iter()
        .map(|name| ((*name).to_owned(), false))
        .collect();

    let stage_logs: Vec<_> = LEGACY_STAGES
        .iter()
        .map(|stage_name| {
            (
                format!("{stage_name} bounded synthetic stdout\n").into_bytes(),
                Vec::new(),
            )
        })
        .collect();
    let stages: Vec<_> = LEGACY_STAGES
        .iter()
        .zip(&stage_logs)
        .map(|(stage_name, (stdout, stderr))| Stage {
            exit_status: None,
            program: digest(&format!("{stage_name} selected program")),
            invocation: digest(&format!("{stage_name} selected invocation")),
            environment: digest(&format!("{stage_name} selected environment")),
            stdout: sha256_bytes(stdout),
            stderr: sha256_bytes(stderr),
        })
        .collect();
    let invocation_bytes = crate::canonical::bytes(&json!({
        "stages": &stages,
        "profile": &profile_document_sha256,
    }))
    .unwrap();
    let identity = MesaConfigurationIdentity {
        target_profile: profile_name.clone(),
        target_triple: target_triple.clone(),
        compiler: compiler.clone(),
        compiler_tree_sha256: compiler_tree_sha256.clone(),
        source_inputs_sha256: source_tree_sha256.clone(),
        configure_invocation_sha256: sha256_bytes(&invocation_bytes),
    };
    let configuration = crate::canonical::bytes(&json!({
        "version": 1,
        "identity": identity,
        "target_isa_flags": ["-march=armv7-a"],
        "builtin_selectors": builtin_selectors,
        "compiler_options": compiler_options,
        "baseline_variables": baseline_variables,
        "undefined_variables": ["CUR_MESADIR"],
            "recipe_configuration": {
                "make_coordinates": {
                    "source_root": "/source",
                    "build_root": output_root.join("configure"),
                    "ports_root": gendir.parent().unwrap().join("Ports"),
                    "generated_root": gendir,
                },
                "safety_cflags": {"state":"defined","value":[]}, "target_lto":"no",
                "lto_cflags":[], "cflags_iquote":["-iquote"], "cflags_iquote_end":[],
                "target_cc":clang_path, "target_cxx":clangxx_path,
                "target_sysroot":[]
            },
            "llvm_configuration": {
            "opt_llvm": { "state": "undefined" },
            "target_llvm_ver": { "state": "defined", "value": "11.0.0" },
        },
        "mesa_compile_options": [],
    }))
    .unwrap();
    let parsed = ValidatedMesaConfiguration::parse_bound(
        &configuration,
        &MesaConfigurationIdentity {
            target_profile: profile_name.clone(),
            target_triple: target_triple.clone(),
            compiler: compiler.clone(),
            compiler_tree_sha256: compiler_tree_sha256.clone(),
            source_inputs_sha256: source_tree_sha256.clone(),
            configure_invocation_sha256: sha256_bytes(&invocation_bytes),
        },
    )
    .unwrap();
    let make_values = parsed.make_variables();

    let mut compiler_cfg = String::new();
    for name in MESA_COMPILER_OPTION_VARIABLES
        .iter()
        .copied()
        .chain(BUILTIN_NAMES.iter().copied())
    {
        writeln!(compiler_cfg, "{name} := {}", make_values.get(name).unwrap()).unwrap();
    }
    let compiler_cfg = compiler_cfg.into_bytes();
    let paths = ConfiguredPaths {
        aros_build_host: "aarch64-apple-darwin".into(),
        aros_developer: gendir
            .parent()
            .unwrap()
            .join("AROS/Developer")
            .to_string_lossy()
            .into_owned(),
        aros_target_arch: platform.clone(),
        aros_target_cpu: cpu.clone(),
        debug_cflags: String::new(),
        gendir: gendir.to_string_lossy().into_owned(),
        optimization_cflags: "-O2".into(),
        portsdir: gendir
            .parent()
            .unwrap()
            .join("Ports")
            .to_string_lossy()
            .into_owned(),
        target_cc: clang_path.to_string_lossy().into_owned(),
        target_cflags: "-O2".into(),
        target_cpp: format!("{} -E", clang_path.display()),
        target_cxx: clangxx_path.to_string_lossy().into_owned(),
        target_cxxflags: "-O2".into(),
        target_sysroot: String::new(),
    };
    let receipt = Receipt {
        compiler_programs: None,
        schema: "aros-mesa-preflight-v1".into(),
        qualification: "local-configure-only".into(),
        identity: identity.clone(),
        configuration_sha256: sha256_bytes(&configuration),
        compiler_cfg_sha256: sha256_bytes(&compiler_cfg),
        make_values,
        configured_paths: paths,
        stages,
        genmodule: None,
        genmodule_collection: None,
        native_make: None,
    };
    let receipt = crate::canonical::bytes(&serde_json::to_value(&receipt).unwrap()).unwrap();
    let input = crate::canonical::bytes(
        &serde_json::to_value(MesaConfigurationInputDocument {
            version: 1,
            local_source: None,
            genmodule: None,
            genmodule_collection: None,
            identity,
            configuration_sha256: sha256_bytes(&configuration),
            receipt_sha256: sha256_bytes(&receipt),
        })
        .unwrap(),
    )
    .unwrap();

    let mut files = BTreeMap::from([
        (RECEIPT_MEMBER.into(), receipt),
        (CONFIGURATION_MEMBER.into(), configuration),
        (INPUT_MEMBER.into(), input),
        (COMPILER_CFG_MEMBER.into(), compiler_cfg),
    ]);
    for (stage, (stdout, stderr)) in LEGACY_STAGES.iter().zip(stage_logs) {
        files.insert(format!("mesa-preflight-{stage}.stdout"), stdout);
        files.insert(format!("mesa-preflight-{stage}.stderr"), stderr);
    }

    Fixture {
        files,
        profile_name,
        target_triple,
        profile_document_sha256,
        compiler,
        compiler_tree_sha256,
        source_tree_sha256,
        cpu,
        platform,
        clang_path,
        clangxx_path,
        output_root,
        genmodule_selection: None,
        genmodule_helper_sha256: None,
    }
}

fn genmodule_evidence_fixture(local_source_enabled: bool) -> Fixture {
    let mut fixture = evidence_fixture();
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();

    let build_stdout = b"genmodule build bounded stdout\n".to_vec();
    let build_stderr = Vec::new();
    let make_stdout = b"genmodule make bounded stdout\n".to_vec();
    let make_stderr = Vec::new();
    let build_stage = Stage {
        exit_status: None,
        program: digest("selected genmodule build program"),
        invocation: digest("genmodule build invocation"),
        environment: digest("genmodule build environment"),
        stdout: sha256_bytes(&build_stdout),
        stderr: sha256_bytes(&build_stderr),
    };
    let make_stage = Stage {
        exit_status: None,
        program: digest("selected genmodule make producer"),
        invocation: digest("genmodule make invocation"),
        environment: digest("genmodule make environment"),
        stdout: sha256_bytes(&make_stdout),
        stderr: sha256_bytes(&make_stderr),
    };
    receipt.stages.extend([build_stage, make_stage]);

    let invocation_bytes = crate::canonical::bytes(&json!({
        "stages": &receipt.stages,
        "profile": &fixture.profile_document_sha256,
    }))
    .unwrap();
    receipt.identity.configure_invocation_sha256 = sha256_bytes(&invocation_bytes);

    let mut configuration_value: serde_json::Value =
        serde_json::from_slice(&fixture.files[CONFIGURATION_MEMBER]).unwrap();
    configuration_value["identity"] = serde_json::to_value(&receipt.identity).unwrap();
    let configuration = crate::canonical::bytes(&configuration_value).unwrap();
    receipt.configuration_sha256 = sha256_bytes(&configuration);

    let mut makefile = String::new();
    for suffix in MESA_GENMODULE_MAKE_SUFFIXES {
        writeln!(makefile, "{GENMODULE_MODULE}_{suffix} +=").unwrap();
    }
    let makefile = makefile.into_bytes();
    let binding = MesaGenmoduleBinding {
        module: GENMODULE_MODULE.into(),
        makefile_sha256: sha256_bytes(&makefile),
        base_configuration_sha256: digest("selected base genmodule configuration"),
        override_configuration_sha256: digest("selected Mesa override configuration"),
        producer_sha256: receipt.stages[LEGACY_STAGES.len() + 1].program.clone(),
    };
    receipt.schema = if local_source_enabled {
        "aros-mesa-preflight-v4".into()
    } else {
        "aros-mesa-preflight-v3".into()
    };
    receipt.genmodule = Some(binding.clone());
    let receipt_bytes = crate::canonical::bytes(&serde_json::to_value(&receipt).unwrap()).unwrap();

    let local_source = local_source_enabled.then(|| LocalSourceIdentity {
        schema: "aros-local-source-v1".into(),
        head_baseline: "0123456789abcdef0123456789abcdef01234567".into(),
        submodules_sha256: digest("selected submodule state"),
        generated_subtree: "build/mesa".into(),
        content_sha256: fixture.source_tree_sha256.clone(),
        entry_count: 1,
        regular_file_bytes: 42,
    });
    let input = crate::canonical::bytes(
        &serde_json::to_value(MesaConfigurationInputDocument {
            version: if local_source_enabled { 4 } else { 3 },
            local_source,
            genmodule: Some(binding),
            genmodule_collection: None,
            identity: receipt.identity.clone(),
            configuration_sha256: receipt.configuration_sha256.clone(),
            receipt_sha256: sha256_bytes(&receipt_bytes),
        })
        .unwrap(),
    )
    .unwrap();

    fixture.files.insert(RECEIPT_MEMBER.into(), receipt_bytes);
    fixture
        .files
        .insert(CONFIGURATION_MEMBER.into(), configuration);
    fixture.files.insert(INPUT_MEMBER.into(), input);
    fixture.files.insert(GENMODULE_MAKE_MEMBER.into(), makefile);
    fixture
        .files
        .insert("mesa-preflight-genmodule-build.stdout".into(), build_stdout);
    fixture
        .files
        .insert("mesa-preflight-genmodule-build.stderr".into(), build_stderr);
    fixture
        .files
        .insert("mesa-preflight-genmodule-make.stdout".into(), make_stdout);
    fixture
        .files
        .insert("mesa-preflight-genmodule-make.stderr".into(), make_stderr);
    fixture
}

fn collection_evidence_fixture(scope: MesaGenmoduleInventoryScope) -> Fixture {
    let mut fixture = evidence_fixture();
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    let selection = MesaGenmoduleSelectionBinding {
        // Source preset identity is deliberately distinct from the compiler profile.
        profile: "source-arm-raspi".into(),
        kind: "build".into(),
        scope,
        contract_sha256: digest("independently selected Mesa source contract"),
    };
    let producers = vec![
        MesaGenmoduleProducer {
            owner: "mesa-gl".into(),
            declaration_directory: "workbench/libs/mesa".into(),
            module: "mesa3dgl26-0".into(),
            module_type: "library".into(),
            base_configuration: "workbench/libs/gl/gl.conf".into(),
            override_configuration: Some("workbench/libs/mesa/mesa3dgl.conf".into()),
        },
        MesaGenmoduleProducer {
            owner: "mesa-v3d".into(),
            declaration_directory: "workbench/hidds/v3d".into(),
            module: "v3d".into(),
            module_type: "hidd".into(),
            base_configuration: "workbench/hidds/v3d/v3d.conf".into(),
            override_configuration: None,
        },
    ];
    let inventory = MesaGenmoduleInventory {
        schema: MESA_GENMODULE_INVENTORY_SCHEMA.into(),
        records: vec![
            MesaGenmoduleInventoryRecord {
                owner: producers[0].owner.clone(),
                declaration_directory: producers[0].declaration_directory.clone(),
                module: producers[0].module.clone(),
                module_type: producers[0].module_type.clone(),
                base_configuration: producers[0].base_configuration.clone(),
                override_configuration: producers[0].override_configuration.clone(),
                consumer_path: "runtime/mesa-gl".into(),
                components: Vec::new(),
            },
            MesaGenmoduleInventoryRecord {
                owner: producers[1].owner.clone(),
                declaration_directory: producers[1].declaration_directory.clone(),
                module: producers[1].module.clone(),
                module_type: producers[1].module_type.clone(),
                base_configuration: producers[1].base_configuration.clone(),
                override_configuration: producers[1].override_configuration.clone(),
                consumer_path: "marker/gl-client/rel".into(),
                components: vec!["getlibbase".into()],
            },
        ],
    };
    let selection_bytes = crate::canonical::bytes(
        &serde_json::to_value(MesaGenmoduleSelectionDocument {
            schema: MESA_GENMODULE_SELECTION_SCHEMA.into(),
            selection: selection.clone(),
            inventory,
        })
        .unwrap(),
    )
    .unwrap();

    let helper_sha256 = digest("selected Mesa genmodule inventory helper");
    let producer_sha256 = digest("selected original genmodule producer");
    let mut outputs = Vec::new();
    let mut raw_outputs = Vec::new();
    for producer in producers {
        let makefile = literal_makefile(&producer.module);
        let output = MesaGenmoduleOutputBinding {
            producer: producer.clone(),
            makefile_sha256: sha256_bytes(&makefile),
            base_configuration_sha256: digest(&format!("base config for {}", producer.owner)),
            override_configuration_sha256: producer
                .override_configuration
                .as_ref()
                .map(|path| digest(&format!("override config {path}"))),
        };
        raw_outputs.push((producer.owner.clone(), makefile));
        outputs.push(output);
    }

    let mut stage_records = Vec::new();
    let mut stage_logs = Vec::new();
    let mut append_stage = |name: &str, program: Sha256Digest| {
        let stdout = format!("{name} bounded output\n").into_bytes();
        let stderr = Vec::new();
        stage_records.push(Stage {
            exit_status: None,
            program,
            invocation: digest(&format!("{name} invocation")),
            environment: digest(&format!("{name} environment")),
            stdout: sha256_bytes(&stdout),
            stderr: sha256_bytes(&stderr),
        });
        stage_logs.push((name.to_owned(), stdout, stderr));
    };
    append_stage("genmodule-inventory", helper_sha256.clone());
    append_stage(
        "genmodule-build",
        digest("selected genmodule build program"),
    );
    for output in &outputs {
        append_stage(
            &format!("genmodule-make-{}", output.producer.owner),
            producer_sha256.clone(),
        );
    }
    receipt.stages.extend(stage_records);
    receipt.schema = "aros-mesa-preflight-v5".into();
    receipt.genmodule = None;
    receipt.genmodule_collection = Some(MesaGenmoduleCollectionBinding {
        selection: selection.clone(),
        inventory_sha256: sha256_bytes(&selection_bytes),
        helper_sha256: helper_sha256.clone(),
        producer_sha256,
        outputs,
    });

    let invocation_bytes = crate::canonical::bytes(&json!({
        "stages": &receipt.stages,
        "profile": &fixture.profile_document_sha256,
    }))
    .unwrap();
    receipt.identity.configure_invocation_sha256 = sha256_bytes(&invocation_bytes);
    let mut configuration_value: serde_json::Value =
        serde_json::from_slice(&fixture.files[CONFIGURATION_MEMBER]).unwrap();
    configuration_value["identity"] = serde_json::to_value(&receipt.identity).unwrap();
    let configuration = crate::canonical::bytes(&configuration_value).unwrap();
    receipt.configuration_sha256 = sha256_bytes(&configuration);

    let receipt_bytes = crate::canonical::bytes(&serde_json::to_value(&receipt).unwrap()).unwrap();
    let input = crate::canonical::bytes(
        &serde_json::to_value(MesaConfigurationInputDocument {
            version: 5,
            local_source: None,
            genmodule: None,
            genmodule_collection: receipt.genmodule_collection.clone(),
            identity: receipt.identity.clone(),
            configuration_sha256: receipt.configuration_sha256.clone(),
            receipt_sha256: sha256_bytes(&receipt_bytes),
        })
        .unwrap(),
    )
    .unwrap();

    fixture.files.insert(RECEIPT_MEMBER.into(), receipt_bytes);
    fixture
        .files
        .insert(CONFIGURATION_MEMBER.into(), configuration);
    fixture.files.insert(INPUT_MEMBER.into(), input);
    fixture
        .files
        .insert(GENMODULE_SELECTION_MEMBER.into(), selection_bytes);
    for (owner, makefile) in raw_outputs {
        fixture
            .files
            .insert(format!("mesa-preflight-genmodule-{owner}.make"), makefile);
    }
    for (name, stdout, stderr) in stage_logs {
        fixture
            .files
            .insert(format!("mesa-preflight-{name}.stdout"), stdout);
        fixture
            .files
            .insert(format!("mesa-preflight-{name}.stderr"), stderr);
    }
    fixture.genmodule_selection = Some(selection);
    fixture.genmodule_helper_sha256 = Some(helper_sha256);
    fixture
}

fn literal_makefile(module: &str) -> Vec<u8> {
    let mut makefile = String::new();
    for suffix in MESA_GENMODULE_MAKE_SUFFIXES {
        writeln!(makefile, "{module}_{suffix} +=").unwrap();
    }
    makefile.into_bytes()
}

fn digest(label: &str) -> Sha256Digest {
    sha256_bytes(label.as_bytes())
}

fn refresh_receipt_and_input(files: &mut BTreeMap<String, Vec<u8>>) {
    let receipt: Receipt = serde_json::from_slice(files.get(RECEIPT_MEMBER).unwrap()).unwrap();
    let receipt_bytes = crate::canonical::bytes(&serde_json::to_value(&receipt).unwrap()).unwrap();
    files.insert(RECEIPT_MEMBER.into(), receipt_bytes.clone());
    let input: MesaConfigurationInputDocument =
        serde_json::from_slice(files.get(INPUT_MEMBER).unwrap()).unwrap();
    let input_bytes = crate::canonical::bytes(
        &serde_json::to_value(MesaConfigurationInputDocument {
            receipt_sha256: sha256_bytes(&receipt_bytes),
            ..input
        })
        .unwrap(),
    )
    .unwrap();
    files.insert(INPUT_MEMBER.into(), input_bytes);
}

fn refresh_collection_identity(
    profile_document_sha256: &Sha256Digest,
    files: &mut BTreeMap<String, Vec<u8>>,
) {
    let mut receipt: Receipt = serde_json::from_slice(&files[RECEIPT_MEMBER]).unwrap();
    let invocation_bytes = crate::canonical::bytes(&json!({
        "stages": &receipt.stages,
        "profile": profile_document_sha256,
    }))
    .unwrap();
    receipt.identity.configure_invocation_sha256 = sha256_bytes(&invocation_bytes);

    let mut configuration: serde_json::Value =
        serde_json::from_slice(&files[CONFIGURATION_MEMBER]).unwrap();
    configuration["identity"] = serde_json::to_value(&receipt.identity).unwrap();
    let configuration_bytes = crate::canonical::bytes(&configuration).unwrap();
    receipt.configuration_sha256 = sha256_bytes(&configuration_bytes);
    let receipt_bytes = crate::canonical::bytes(&serde_json::to_value(&receipt).unwrap()).unwrap();

    let input: MesaConfigurationInputDocument =
        serde_json::from_slice(&files[INPUT_MEMBER]).unwrap();
    let input_bytes = crate::canonical::bytes(
        &serde_json::to_value(MesaConfigurationInputDocument {
            identity: receipt.identity,
            configuration_sha256: sha256_bytes(&configuration_bytes),
            receipt_sha256: sha256_bytes(&receipt_bytes),
            ..input
        })
        .unwrap(),
    )
    .unwrap();
    files.insert(CONFIGURATION_MEMBER.into(), configuration_bytes);
    files.insert(RECEIPT_MEMBER.into(), receipt_bytes);
    files.insert(INPUT_MEMBER.into(), input_bytes);
}

fn native_collection_evidence_fixture(
    scope: MesaGenmoduleInventoryScope,
    output_root: &Path,
) -> Fixture {
    use aros_common::make_native_invocation::NativeMakeInvocation;
    use aros_common::make_native_observation::MakeObservationPhase;
    use aros_common::make_native_sampler::{
        observe_first_input_make, InitialMakeObservationRequest,
    };
    use aros_common::mesa_native_make::{
        native_make_context_bytes, native_packet_file, MesaNativeMakeBinding,
        MesaNativeMakeCallBinding, MesaNativeOwnerCall, NATIVE_CONTEXT_FILE, NATIVE_CONTROL_FILE,
        NATIVE_PREPARATION_STAGES,
    };
    use aros_common::process::CancellationToken;
    use std::process::Command;
    use std::time::Duration;

    fn gnu_make() -> PathBuf {
        let mut candidates = vec![
            PathBuf::from("/opt/homebrew/bin/gmake"),
            PathBuf::from("/usr/bin/make"),
        ];
        for name in ["gmake", "make"] {
            if let Ok(path) = which::which(name) {
                candidates.push(path);
            }
        }
        for path in candidates {
            let Ok(path) = path.canonicalize() else {
                continue;
            };
            let Ok(version) = Command::new(&path).arg("--version").output() else {
                continue;
            };
            if version.status.success()
                && String::from_utf8_lossy(&version.stdout)
                    .lines()
                    .next()
                    .is_some_and(|line| line.starts_with("GNU Make "))
            {
                return path;
            }
        }
        panic!("tests require a real GNU Make executable");
    }

    let mut fixture = collection_evidence_fixture(scope);
    let output_root = output_root.canonicalize().unwrap();
    let build_root = output_root.join("configure");
    let source_root = output_root.parent().unwrap().join("source");
    let gendir = build_root.join("bin/raspi-arm/gen");
    let ports_root = build_root.join("bin/raspi-arm/Ports");
    let developer = build_root.join("bin/raspi-arm/AROS/Developer");
    for directory in [&build_root, &source_root, &gendir, &ports_root, &developer] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let coordinates = aros_common::mesa_configuration::MesaMakeCoordinates {
        source_root: source_root.to_string_lossy().into_owned(),
        build_root: build_root.to_string_lossy().into_owned(),
        ports_root: ports_root.to_string_lossy().into_owned(),
        generated_root: gendir.to_string_lossy().into_owned(),
    };

    let stdout = b"AROS_MAKE_INITIAL_NAMESPACE=v1:3;.VARIABLES|default;MAKE_VERSION|default;SOURCE_RAW|file;end\nAROS_MAKE_INITIAL_VALUES=v1;MAKE_VERSION|default|simple|4.4.1;SOURCE_RAW|file|recursive|$(LATER);end\n";
    let namespace =
        aros_common::make_namespace::ValidatedMakeNamespace::from_make_stdout(stdout).unwrap();
    let values = aros_common::make_initial_values::ValidatedMakeInitialValues::from_make_stdout(
        stdout, &namespace,
    )
    .unwrap();
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    let mut configuration: serde_json::Value =
        serde_json::from_slice(&fixture.files[CONFIGURATION_MEMBER]).unwrap();
    configuration["version"] = json!(3);
    configuration["initial_namespace"] = serde_json::to_value(namespace.to_observation()).unwrap();
    configuration["initial_values"] = serde_json::to_value(values.to_observation()).unwrap();
    configuration["recipe_configuration"]["make_coordinates"] =
        serde_json::to_value(&coordinates).unwrap();
    let configuration = crate::canonical::bytes(&configuration).unwrap();
    let parsed =
        ValidatedMesaConfiguration::parse_bound(&configuration, &receipt.identity).unwrap();
    receipt.make_values = parsed.make_variables();
    receipt.configured_paths.gendir = coordinates.generated_root.clone();
    receipt.configured_paths.portsdir = coordinates.ports_root;
    receipt.configured_paths.aros_developer = developer.to_string_lossy().into_owned();
    fixture.output_root = output_root.clone();

    let program = gnu_make();
    let program_sha256 = sha256_bytes(&std::fs::read(&program).unwrap());
    let configured_environment = digest("explicit empty configured Make environment");
    let configured_stdout = sha256_bytes(stdout);
    receipt.stages.truncate(4);
    receipt.stages[3].program = program_sha256.clone();
    receipt.stages[3].environment = configured_environment.clone();
    receipt.stages[3].invocation = digest("selected configured Make invocation");
    receipt.stages[3].stdout = configured_stdout.clone();
    fixture.files.insert(
        "mesa-preflight-configured-values.stdout".into(),
        stdout.to_vec(),
    );
    fixture
        .files
        .insert("mesa-preflight-configured-values.stderr".into(), Vec::new());

    let mut compiler_cfg = String::new();
    for name in MESA_COMPILER_OPTION_VARIABLES
        .iter()
        .copied()
        .chain(BUILTIN_NAMES.iter().copied())
    {
        writeln!(compiler_cfg, "{name} := {}", receipt.make_values[name]).unwrap();
    }
    let compiler_cfg = compiler_cfg.into_bytes();
    receipt.compiler_cfg_sha256 = sha256_bytes(&compiler_cfg);
    fixture
        .files
        .insert(COMPILER_CFG_MEMBER.into(), compiler_cfg);
    let configuration_sha256 = sha256_bytes(&configuration);
    receipt.configuration_sha256 = configuration_sha256.clone();
    fixture
        .files
        .insert(CONFIGURATION_MEMBER.into(), configuration);

    let mut binding = MesaNativeMakeBinding {
        version: 1,
        source_inputs_sha256: fixture.source_tree_sha256.clone(),
        compiler_tree_sha256: fixture.compiler_tree_sha256.clone(),
        configuration_stdout_sha256: configured_stdout,
        // Retained metadata only; this portable fixture does not claim a
        // physical prepared-tree validation.
        prepared_tree_sha256: digest("retained prepared-tree metadata only"),
        parent_program_sha256: program_sha256.clone(),
        parent_invocation_sha256: digest("selected native root launch invocation"),
        parent_environment_sha256: configured_environment.clone(),
        host_metamake_sha256: digest("selected host metamake data"),
        context_sha256: digest("native Make context placeholder"),
        control_sha256: digest("native Make control placeholder"),
        calls: Vec::new(),
        owners: Vec::new(),
        source_calls: Vec::new(),
    };
    let context_bytes = native_make_context_bytes(&binding).unwrap();
    binding.context_sha256 = sha256_bytes(&context_bytes);

    let mut native_files = BTreeMap::from([(
        format!("mesa-preflight-{NATIVE_CONTEXT_FILE}"),
        context_bytes,
    )]);
    let producers: Vec<_> = receipt
        .genmodule_collection
        .as_ref()
        .unwrap()
        .outputs
        .iter()
        .map(|output| output.producer.clone())
        .collect();
    for (call_index, producer) in producers.iter().enumerate() {
        let working_directory = build_root.join(&producer.declaration_directory);
        std::fs::create_dir_all(&working_directory).unwrap();
        let working_directory = working_directory.canonicalize().unwrap();
        let invocation = NativeMakeInvocation::new(
            [
                "-f".to_owned(),
                "selected-input.mk".to_owned(),
                "stop-before-input".to_owned(),
            ],
            working_directory,
            std::iter::empty::<(String, String)>(),
        )
        .unwrap()
        .with_argument_zero("make".to_owned())
        .unwrap();
        let request = InitialMakeObservationRequest {
            program: &program,
            executable_sha256: &program_sha256,
            invocation: &invocation,
            context_sha256: &binding.context_sha256,
            timeout: Duration::from_secs(20),
        };
        let observation =
            observe_first_input_make(&request, &CancellationToken::default(), |_, _| Ok(()))
                .unwrap_or_else(|error| panic!("GNU Make first-input sample failed: {error}"));
        let packet = observation.to_bytes().unwrap();
        binding.calls.push(MesaNativeMakeCallBinding {
            document_sha256: sha256_bytes(&packet),
            binding: observation.binding().clone(),
        });
        binding.owners.push(MesaNativeOwnerCall {
            owner: producer.owner.clone(),
            call_index,
        });
        binding
            .source_calls
            .push(aros_common::mesa_native_make::MesaNativeSourceCall {
                generated_owner: format!("{}/selected-input.mk", producer.declaration_directory),
                call_index,
            });
        native_files.insert(
            format!("mesa-preflight-{}", native_packet_file(call_index).unwrap()),
            packet,
        );
    }
    let control_bytes = serde_json::to_vec(&json!({
        "version": 4,
        "phase": MakeObservationPhase::FirstInputPrelude,
        "maximum_calls": binding.calls.len(),
        "output_root": output_root.join("native-make-calls"),
        "program": program,
        "executable_sha256": program_sha256,
        "context_sha256": binding.context_sha256,
        "argument_zero": "make",
    }))
    .unwrap();
    binding.control_sha256 = sha256_bytes(&control_bytes);
    native_files.insert(
        format!("mesa-preflight-{NATIVE_CONTROL_FILE}"),
        control_bytes,
    );

    let mut stages = Vec::new();
    let mut append_stage = |name: &str,
                            program: Sha256Digest,
                            invocation: Sha256Digest,
                            environment: Sha256Digest,
                            exit_status: Option<i32>| {
        let stage_stdout = format!("{name} retained bounded output\n").into_bytes();
        let stage_stderr = Vec::new();
        stages.push(Stage {
            program,
            invocation,
            environment,
            stdout: sha256_bytes(&stage_stdout),
            stderr: sha256_bytes(&stage_stderr),
            exit_status,
        });
        fixture
            .files
            .insert(format!("mesa-preflight-{name}.stdout"), stage_stdout);
        fixture
            .files
            .insert(format!("mesa-preflight-{name}.stderr"), stage_stderr);
    };
    for (index, name) in NATIVE_PREPARATION_STAGES.iter().enumerate() {
        append_stage(
            name,
            program_sha256.clone(),
            if index == 2 {
                binding.parent_invocation_sha256.clone()
            } else {
                digest(name)
            },
            configured_environment.clone(),
            Some(if index == 0 { 2 } else { 0 }),
        );
    }
    let collection = receipt.genmodule_collection.as_ref().unwrap();
    append_stage(
        "genmodule-inventory",
        collection.helper_sha256.clone(),
        digest("selected native inventory invocation"),
        digest("selected native inventory environment"),
        None,
    );
    append_stage(
        "genmodule-build",
        program_sha256,
        digest("selected native generator build invocation"),
        configured_environment,
        None,
    );
    for output in &collection.outputs {
        append_stage(
            &format!("genmodule-make-{}", output.producer.owner),
            collection.producer_sha256.clone(),
            digest(&format!("{} producer invocation", output.producer.owner)),
            digest(&format!("{} producer environment", output.producer.owner)),
            None,
        );
    }
    receipt.stages.extend(stages);
    receipt.schema = "aros-mesa-preflight-v9".into();
    receipt.genmodule = None;
    receipt.native_make = Some(binding);
    let input: MesaConfigurationInputDocument =
        serde_json::from_slice(&fixture.files[INPUT_MEMBER]).unwrap();
    fixture.files.insert(
        INPUT_MEMBER.into(),
        crate::canonical::bytes(
            &serde_json::to_value(MesaConfigurationInputDocument {
                version: 9,
                local_source: None,
                genmodule: None,
                genmodule_collection: receipt.genmodule_collection.clone(),
                identity: receipt.identity.clone(),
                configuration_sha256,
                receipt_sha256: input.receipt_sha256,
            })
            .unwrap(),
        )
        .unwrap(),
    );
    fixture.files.extend(native_files);
    fixture.files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
    );
    refresh_collection_identity(&fixture.profile_document_sha256, &mut fixture.files);
    fixture
}

#[test]
fn opaque_mesa_data_proof_cannot_be_rebound_to_another_expectation_set() {
    let fixture = evidence_fixture();
    let proof = readback(&fixture.files, &fixture.expectations()).unwrap();
    assert!(proof.matches_expectations(&fixture.expectations()).unwrap());
    let mut changed = evidence_fixture();
    changed.output_root = PathBuf::from("/other-owner/mesa-preflight");
    assert!(!proof.matches_expectations(&changed.expectations()).unwrap());
    changed = evidence_fixture();
    changed.profile_document_sha256 = digest("another profile document");
    assert!(!proof.matches_expectations(&changed.expectations()).unwrap());
    changed = evidence_fixture();
    changed.compiler_tree_sha256 = digest("another compiler content tree");
    assert!(!proof.matches_expectations(&changed.expectations()).unwrap());
}

#[test]
fn mesa_preflight_portable_compiler_roles_need_no_local_compiler_prefix() {
    use aros_common::captured_programs::{
        ProgramRootSelection, ProgramSelection, ProgramSetSelection,
    };
    let mut fixture = evidence_fixture();
    let selected = ProgramSetSelection {
        roots: vec![ProgramRootSelection {
            root: PathBuf::from("/selected/compiler"),
            content_sha256: fixture.compiler_tree_sha256.clone(),
        }],
        programs: [
            ("target-c", &fixture.clang_path),
            ("target-cxx", &fixture.clangxx_path),
        ]
        .into_iter()
        .map(|(role, path)| ProgramSelection {
            role: role.to_owned(),
            root_index: 0,
            invocation_path: path.clone(),
            executable_sha256: digest(role),
        })
        .collect(),
    };
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    receipt.compiler_programs = Some(selected);
    fixture
        .files
        .insert(RECEIPT_MEMBER.into(), serde_json::to_vec(&receipt).unwrap());
    refresh_receipt_and_input(&mut fixture.files);
    readback(&fixture.files, &fixture.expectations()).unwrap();

    let mut wrong_root: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    wrong_root.compiler_programs.as_mut().unwrap().roots[0].root = PathBuf::from("/");
    fixture.files.insert(
        RECEIPT_MEMBER.into(),
        serde_json::to_vec(&wrong_root).unwrap(),
    );
    refresh_receipt_and_input(&mut fixture.files);
    assert!(readback(&fixture.files, &fixture.expectations())
        .unwrap_err()
        .to_string()
        .contains("independently selected root"));

    receipt.compiler_programs.as_mut().unwrap().programs[1].invocation_path =
        PathBuf::from("/selected/compiler/bin/other-driver");
    fixture
        .files
        .insert(RECEIPT_MEMBER.into(), serde_json::to_vec(&receipt).unwrap());
    refresh_receipt_and_input(&mut fixture.files);
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());
}

#[test]
fn mesa_preflight_portable_readback_checks_fixed_data_bindings() {
    let fixture = evidence_fixture();
    let proof = readback(&fixture.files, &fixture.expectations()).unwrap();
    assert_eq!(
        proof.receipt_sha256(),
        &sha256_bytes(&fixture.files[RECEIPT_MEMBER])
    );
    assert_eq!(
        proof.configuration_sha256(),
        &sha256_bytes(&fixture.files[CONFIGURATION_MEMBER])
    );
    assert_eq!(
        proof.input_sha256(),
        &sha256_bytes(&fixture.files[INPUT_MEMBER])
    );
    assert_eq!(proof.configuration_identity().target_profile, "arm-raspi");
}

#[test]
fn legacy_portable_receipts_refuse_native_fields_without_reinterpreting_stages() {
    for mutation in [
        json!({"native_make":null}),
        json!({"native_make":{"version":1}}),
        json!({"exit_status":null}),
        json!({"exit_status":0}),
    ] {
        let mut fixture = evidence_fixture();
        let mut receipt: Value = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
        if let Some(status) = mutation.get("exit_status") {
            receipt["stages"][0]["exit_status"] = status.clone();
        } else {
            receipt["native_make"] = mutation["native_make"].clone();
        }
        match serde_json::from_value::<Receipt>(receipt.clone()) {
            Ok(parsed) => assert!(validate_receipt_header(&parsed).is_err()),
            Err(_) => assert!(mutation.get("exit_status").is_none_or(Value::is_null)),
        }
        fixture
            .files
            .insert(RECEIPT_MEMBER.into(), serde_json::to_vec(&receipt).unwrap());
        assert!(readback(&fixture.files, &fixture.expectations()).is_err());
    }
}

#[test]
fn initial_values_readback_rejects_a_changed_raw_value_after_json_rebinding() {
    use aros_common::make_initial_values::ValidatedMakeInitialValues;
    use aros_common::make_namespace::ValidatedMakeNamespace;
    let mut fixture = evidence_fixture();
    let stdout = b"AROS_MAKE_INITIAL_NAMESPACE=v1:3;.VARIABLES|default;MAKE_VERSION|default;SOURCE_RAW|file;end\nAROS_MAKE_INITIAL_VALUES=v1;MAKE_VERSION|default|simple|4.4.1;SOURCE_RAW|file|recursive|$(LATER);end\n";
    let namespace = ValidatedMakeNamespace::from_make_stdout(stdout).unwrap();
    let values = ValidatedMakeInitialValues::from_make_stdout(stdout, &namespace).unwrap();
    fixture.files.insert(
        "mesa-preflight-configured-values.stdout".into(),
        stdout.to_vec(),
    );
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    receipt.stages[3].stdout = sha256_bytes(stdout);
    fixture.files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
    );
    let mut configuration: serde_json::Value =
        serde_json::from_slice(&fixture.files[CONFIGURATION_MEMBER]).unwrap();
    configuration["version"] = json!(3);
    configuration["initial_namespace"] = serde_json::to_value(namespace.to_observation()).unwrap();
    configuration["initial_values"] = serde_json::to_value(values.to_observation()).unwrap();
    fixture.files.insert(
        CONFIGURATION_MEMBER.into(),
        crate::canonical::bytes(&configuration).unwrap(),
    );
    refresh_collection_identity(&fixture.profile_document_sha256, &mut fixture.files);
    assert!(readback(&fixture.files, &fixture.expectations()).is_ok());
    let mut configuration: serde_json::Value =
        serde_json::from_slice(&fixture.files[CONFIGURATION_MEMBER]).unwrap();
    configuration["initial_values"]["entries"][1]["value"] = json!("forged raw value");
    fixture.files.insert(
        CONFIGURATION_MEMBER.into(),
        crate::canonical::bytes(&configuration).unwrap(),
    );
    refresh_collection_identity(&fixture.profile_document_sha256, &mut fixture.files);
    assert!(readback(&fixture.files, &fixture.expectations())
        .err()
        .unwrap()
        .to_string()
        .contains("initial values differ from the retained configured-values stage"));
}

#[test]
fn namespace_readback_requires_original_stage_bytes_not_a_rebound_json_claim() {
    use aros_common::make_namespace::ValidatedMakeNamespace;
    let mut fixture = evidence_fixture();
    let mut stdout = String::from(
        "AROS_MAKE_INITIAL_NAMESPACE=v1:1002;.VARIABLES|default;MAKE_VERSION|default;",
    );
    for index in 0..1000 {
        write!(stdout, "MEASURED_{index:04}|file;").unwrap();
    }
    stdout.push_str("end\n");
    let stdout = stdout.as_bytes();
    let namespace = ValidatedMakeNamespace::from_make_stdout(stdout).unwrap();
    fixture.files.insert(
        "mesa-preflight-configured-values.stdout".into(),
        stdout.to_vec(),
    );
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    receipt.stages[3].stdout = sha256_bytes(stdout);
    fixture.files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
    );
    let mut configuration: serde_json::Value =
        serde_json::from_slice(&fixture.files[CONFIGURATION_MEMBER]).unwrap();
    configuration["version"] = json!(2);
    configuration["initial_namespace"] = serde_json::to_value(namespace.to_observation()).unwrap();
    fixture.files.insert(
        CONFIGURATION_MEMBER.into(),
        crate::canonical::bytes(&configuration).unwrap(),
    );
    refresh_collection_identity(&fixture.profile_document_sha256, &mut fixture.files);
    assert!(fixture.files[CONFIGURATION_MEMBER].len() > 32 * 1024);
    assert!(readback(&fixture.files, &fixture.expectations()).is_ok());
    configuration = serde_json::from_slice(&fixture.files[CONFIGURATION_MEMBER]).unwrap();
    configuration["initial_namespace"] = serde_json::to_value(
        ValidatedMakeNamespace::from_measurement(
            "v1:3;.VARIABLES|default;MAKE_VERSION|default;EXTRA|file;end",
        )
        .unwrap()
        .to_observation(),
    )
    .unwrap();
    fixture.files.insert(
        CONFIGURATION_MEMBER.into(),
        crate::canonical::bytes(&configuration).unwrap(),
    );
    refresh_collection_identity(&fixture.profile_document_sha256, &mut fixture.files);
    assert!(readback(&fixture.files, &fixture.expectations())
        .err()
        .unwrap()
        .to_string()
        .contains("Mesa namespace differs"));
}

#[test]
fn mesa_preflight_portable_readback_rejects_log_and_configuration_changes() {
    let mut fixture = evidence_fixture();
    fixture
        .files
        .get_mut("mesa-preflight-features.stdout")
        .unwrap()
        .push(b'!');
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());

    let mut fixture = evidence_fixture();
    fixture
        .files
        .get_mut(CONFIGURATION_MEMBER)
        .unwrap()
        .push(b' ');
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());
}

#[test]
fn mesa_preflight_portable_readback_requires_independently_selected_identity() {
    let fixture = evidence_fixture();
    let mut expected = fixture.expectations();
    let alternate_source = digest("different selected source tree");
    expected.source_tree_sha256 = &alternate_source;
    assert!(readback(&fixture.files, &expected).is_err());
}

#[test]
fn mesa_preflight_portable_readback_rejects_missing_extra_and_oversized_members() {
    let mut fixture = evidence_fixture();
    fixture.files.remove(INPUT_MEMBER);
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());

    let mut fixture = evidence_fixture();
    fixture.files.insert("unexpected.json".into(), Vec::new());
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());

    let mut fixture = evidence_fixture();
    fixture.files.insert(
        "mesa-preflight-configure.stdout".into(),
        vec![b'x'; MAX_STAGE_BYTES + 1],
    );
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());
}

#[test]
fn mesa_preflight_portable_readback_checks_make_values_and_compiler_cfg() {
    let mut fixture = evidence_fixture();
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    receipt
        .make_values
        .insert("CFLAGS_GNU11".into(), "-forged".into());
    fixture.files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
    );
    refresh_receipt_and_input(&mut fixture.files);
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());

    let mut fixture = evidence_fixture();
    fixture
        .files
        .get_mut(COMPILER_CFG_MEMBER)
        .unwrap()
        .extend_from_slice(b"CFLAGS_GNU11 := -forged\n");
    let compiler_cfg_sha256 = sha256_bytes(&fixture.files[COMPILER_CFG_MEMBER]);
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    receipt.compiler_cfg_sha256 = compiler_cfg_sha256;
    fixture.files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
    );
    refresh_receipt_and_input(&mut fixture.files);
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());
}

#[test]
fn mesa_preflight_member_selection_is_flat_fixed_and_bounded() {
    let fixture = evidence_fixture();
    let members = expected_members(&fixture.output_root, &fixture.files[RECEIPT_MEMBER]).unwrap();
    assert_eq!(members.len(), 12);
    assert_eq!(
        members[COMPILER_CFG_MEMBER].0,
        fixture
            .output_root
            .join("configure/bin/raspi-arm/gen/config/compiler.cfg")
    );
    assert_eq!(
        members["mesa-preflight-configure.stdout"].1,
        MAX_STAGE_BYTES
    );
}

#[test]
fn mesa_preflight_genmodule_readback_accepts_complete_tree_and_refuses_local_snapshot() {
    {
        let fixture = genmodule_evidence_fixture(false);
        let proof = readback(&fixture.files, &fixture.expectations()).unwrap();
        assert_eq!(
            proof.receipt_sha256(),
            &sha256_bytes(&fixture.files[RECEIPT_MEMBER])
        );
        let members =
            expected_members(&fixture.output_root, &fixture.files[RECEIPT_MEMBER]).unwrap();
        assert_eq!(members.len(), 17);
        assert_eq!(
            members[GENMODULE_MAKE_MEMBER].0,
            fixture.output_root.join(MESA_GENMODULE_MAKE_FILE)
        );
        assert_eq!(members[GENMODULE_MAKE_MEMBER].1, MAX_GENMODULE_MAKE_BYTES);
    }
    let local = genmodule_evidence_fixture(true);
    assert!(readback(&local.files, &local.expectations()).is_err());
}

#[test]
fn mesa_preflight_genmodule_readback_rejects_raw_mutation_and_stage_mismatch() {
    let mut fixture = genmodule_evidence_fixture(false);
    fixture
        .files
        .get_mut(GENMODULE_MAKE_MEMBER)
        .unwrap()
        .push(b'!');
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());

    let mut fixture = genmodule_evidence_fixture(false);
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    receipt.genmodule.as_mut().unwrap().producer_sha256 = digest("different producer digest");
    fixture.files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(&receipt).unwrap()).unwrap(),
    );
    refresh_receipt_and_input(&mut fixture.files);
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());
}

#[test]
fn mesa_preflight_genmodule_readback_rejects_version_schema_mismatches() {
    let mut fixture = genmodule_evidence_fixture(false);
    let mut input: MesaConfigurationInputDocument =
        serde_json::from_slice(&fixture.files[INPUT_MEMBER]).unwrap();
    input.version = 1;
    fixture.files.insert(
        INPUT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(input).unwrap()).unwrap(),
    );
    refresh_receipt_and_input(&mut fixture.files);
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());

    let mut fixture = genmodule_evidence_fixture(true);
    let mut input: MesaConfigurationInputDocument =
        serde_json::from_slice(&fixture.files[INPUT_MEMBER]).unwrap();
    input.local_source = None;
    fixture.files.insert(
        INPUT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(input).unwrap()).unwrap(),
    );
    refresh_receipt_and_input(&mut fixture.files);
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());

    let mut fixture = genmodule_evidence_fixture(false);
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    receipt.schema = "aros-mesa-preflight-v1".into();
    fixture.files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
    );
    refresh_receipt_and_input(&mut fixture.files);
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());
}

#[test]
fn mesa_preflight_collection_readback_accepts_independently_selected_scopes() {
    for scope in [
        MesaGenmoduleInventoryScope::FullTarget,
        MesaGenmoduleInventoryScope::ContractRoots,
    ] {
        let fixture = collection_evidence_fixture(scope);
        assert_ne!(
            fixture.genmodule_selection.as_ref().unwrap().profile,
            fixture.profile_name,
            "Source preset and compiler profile are independent selections"
        );
        let proof = readback(&fixture.files, &fixture.expectations()).unwrap();
        assert_eq!(
            proof.receipt_sha256(),
            &sha256_bytes(&fixture.files[RECEIPT_MEMBER])
        );
        let members =
            expected_members(&fixture.output_root, &fixture.files[RECEIPT_MEMBER]).unwrap();
        assert_eq!(members.len(), 23);
        assert!(members.contains_key(GENMODULE_SELECTION_MEMBER));
        assert!(members.contains_key("mesa-preflight-genmodule-mesa-gl.make"));
        assert!(members.contains_key("mesa-preflight-genmodule-mesa-v3d.make"));
    }
}

#[test]
fn mesa_preflight_collection_requires_independently_selected_source_binding() {
    let fixture = collection_evidence_fixture(MesaGenmoduleInventoryScope::FullTarget);
    let mut expected = fixture.expectations();
    expected.genmodule_selection = None;
    assert!(readback(&fixture.files, &expected).is_err());

    let mut expected = fixture.expectations();
    let mut selection = expected.genmodule_selection.clone().unwrap();
    selection.scope = MesaGenmoduleInventoryScope::ContractRoots;
    expected.genmodule_selection = Some(selection);
    assert!(readback(&fixture.files, &expected).is_err());

    let mut expected = fixture.expectations();
    let mut selection = expected.genmodule_selection.clone().unwrap();
    selection.profile = "another-source-preset".into();
    expected.genmodule_selection = Some(selection);
    assert!(readback(&fixture.files, &expected).is_err());

    let mut expected = fixture.expectations();
    let mut selection = expected.genmodule_selection.clone().unwrap();
    selection.contract_sha256 = digest("another selected source contract");
    expected.genmodule_selection = Some(selection);
    assert!(readback(&fixture.files, &expected).is_err());

    let mut expected = fixture.expectations();
    expected.genmodule_helper_sha256 = None;
    assert!(readback(&fixture.files, &expected).is_err());
}

#[test]
fn mesa_preflight_collection_readback_rejects_inventory_outputs_and_owner_stage_drift() {
    let mut fixture = collection_evidence_fixture(MesaGenmoduleInventoryScope::FullTarget);
    fixture
        .files
        .get_mut(GENMODULE_SELECTION_MEMBER)
        .unwrap()
        .push(b' ');
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());

    let mut fixture = collection_evidence_fixture(MesaGenmoduleInventoryScope::FullTarget);
    fixture
        .files
        .remove("mesa-preflight-genmodule-mesa-v3d.make");
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());

    let mut fixture = collection_evidence_fixture(MesaGenmoduleInventoryScope::FullTarget);
    fixture
        .files
        .insert("unexpected-output.make".into(), Vec::new());
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());

    let mut fixture = collection_evidence_fixture(MesaGenmoduleInventoryScope::FullTarget);
    fixture
        .files
        .get_mut("mesa-preflight-genmodule-mesa-gl.make")
        .unwrap()
        .push(b'!');
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());

    let mut fixture = collection_evidence_fixture(MesaGenmoduleInventoryScope::FullTarget);
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    receipt.stages[LEGACY_STAGES.len()].program = digest("unbound inventory helper stage");
    fixture.files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
    );
    refresh_collection_identity(&fixture.profile_document_sha256, &mut fixture.files);
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());

    // Re-sealing the receipt and both helper references must not let the
    // artifact choose its own helper identity. The expectation remains the
    // independently selected fixture digest.
    let mut fixture = collection_evidence_fixture(MesaGenmoduleInventoryScope::FullTarget);
    let forged_helper = digest("different inventory helper, consistently resealed");
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    receipt.genmodule_collection.as_mut().unwrap().helper_sha256 = forged_helper.clone();
    receipt.stages[LEGACY_STAGES.len()].program = forged_helper.clone();
    fixture.files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
    );
    refresh_collection_identity(&fixture.profile_document_sha256, &mut fixture.files);
    assert_ne!(
        fixture.genmodule_helper_sha256.as_ref().unwrap(),
        &forged_helper
    );
    assert!(readback(&fixture.files, &fixture.expectations()).is_err());
}

#[test]
fn mesa_preflight_expected_inventory_helper_requires_source_selection() {
    let fixture = evidence_fixture();
    let mut expected = fixture.expectations();
    expected.genmodule_helper_sha256 = Some(digest("orphaned expected inventory helper"));
    assert!(readback(&fixture.files, &expected).is_err());
}

#[test]
fn mesa_preflight_local_collection_v6_is_never_a_portable_proof() {
    let fixture = collection_evidence_fixture(MesaGenmoduleInventoryScope::FullTarget);
    let mut receipt: Receipt = serde_json::from_slice(&fixture.files[RECEIPT_MEMBER]).unwrap();
    receipt.schema = "aros-mesa-preflight-v6".into();
    let receipt_bytes = crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap();
    let mut input: MesaConfigurationInputDocument =
        serde_json::from_slice(&fixture.files[INPUT_MEMBER]).unwrap();
    input.version = 6;
    input.local_source = Some(LocalSourceIdentity {
        schema: "aros-local-source-v1".into(),
        head_baseline: "0123456789abcdef0123456789abcdef01234567".into(),
        submodules_sha256: digest("selected submodule state"),
        generated_subtree: "build/mesa".into(),
        content_sha256: fixture.source_tree_sha256.clone(),
        entry_count: 1,
        regular_file_bytes: 42,
    });
    input.receipt_sha256 = sha256_bytes(&receipt_bytes);
    let mut files = fixture.files.clone();
    files.insert(RECEIPT_MEMBER.into(), receipt_bytes);
    files.insert(
        INPUT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(input).unwrap()).unwrap(),
    );
    assert!(readback(&files, &fixture.expectations()).is_err());
}

#[test]
fn mesa_preflight_native_v9_accepts_actual_first_input_packets_and_rejects_mutations() {
    use aros_common::make_native_invocation::NativeMakeInvocation;
    use aros_common::make_native_observation::{MakeObservationBinding, RetainedMakeObservation};
    use aros_common::mesa_native_make::{native_packet_file, NATIVE_CONTROL_FILE};

    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().join("evidence");
    std::fs::create_dir_all(&output_root).unwrap();
    let fixture = native_collection_evidence_fixture(
        MesaGenmoduleInventoryScope::ContractRoots,
        &output_root,
    );
    let proof = readback(&fixture.files, &fixture.expectations()).unwrap();
    assert_eq!(
        proof.receipt_sha256(),
        &sha256_bytes(&fixture.files[RECEIPT_MEMBER])
    );
    let members = expected_members(&fixture.output_root, &fixture.files[RECEIPT_MEMBER]).unwrap();
    assert_eq!(members.len(), 33);
    assert!(members.contains_key("mesa-preflight-native-make-context.json"));
    assert!(members.contains_key("mesa-preflight-native-make-control.json"));
    assert!(members.contains_key(&format!(
        "mesa-preflight-{}",
        native_packet_file(0).unwrap()
    )));

    let mut rejected = 0;
    let mut check_rejected = |files: &BTreeMap<String, Vec<u8>>, expected: &str| {
        let error = readback(files, &fixture.expectations())
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "unexpected error: {error}");
        rejected += 1;
    };

    let mut files = fixture.files.clone();
    let mut receipt: Receipt = serde_json::from_slice(&files[RECEIPT_MEMBER]).unwrap();
    receipt.stages[4].program = digest("wrong native root-query program");
    files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
    );
    refresh_collection_identity(&fixture.profile_document_sha256, &mut files);
    check_rejected(&files, "Mesa native preparation stage identity is invalid");

    let mut files = fixture.files.clone();
    let mut receipt: Receipt = serde_json::from_slice(&files[RECEIPT_MEMBER]).unwrap();
    receipt.stages[4].exit_status = Some(0);
    files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
    );
    refresh_collection_identity(&fixture.profile_document_sha256, &mut files);
    check_rejected(&files, "Mesa native preparation stage identity is invalid");

    let mut files = fixture.files.clone();
    files
        .get_mut("mesa-preflight-native-make-context.json")
        .unwrap()
        .push(b' ');
    check_rejected(&files, "Mesa native Make packet data is inconsistent");

    let mut files = fixture.files.clone();
    let mut control: serde_json::Value =
        serde_json::from_slice(&files[&format!("mesa-preflight-{NATIVE_CONTROL_FILE}")]).unwrap();
    control["maximum_calls"] = json!(1);
    let control_bytes = serde_json::to_vec(&control).unwrap();
    files.insert(
        format!("mesa-preflight-{NATIVE_CONTROL_FILE}"),
        control_bytes.clone(),
    );
    let mut receipt: Receipt = serde_json::from_slice(&files[RECEIPT_MEMBER]).unwrap();
    receipt.native_make.as_mut().unwrap().control_sha256 = sha256_bytes(&control_bytes);
    files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
    );
    refresh_collection_identity(&fixture.profile_document_sha256, &mut files);
    check_rejected(&files, "Mesa native Make packet data is inconsistent");

    for call_index in 0..2 {
        let mut files = fixture.files.clone();
        let receipt: Receipt = serde_json::from_slice(&files[RECEIPT_MEMBER]).unwrap();
        let call = &receipt.native_make.as_ref().unwrap().calls[call_index];
        let packet_name = format!("mesa-preflight-{}", native_packet_file(call_index).unwrap());
        let packet = RetainedMakeObservation::load(
            &files[&packet_name],
            &call.document_sha256,
            &call.binding,
        )
        .unwrap();
        let wrong_cwd = temporary.path().join("wrong-native-cwd");
        std::fs::create_dir_all(&wrong_cwd).unwrap();
        let invocation = NativeMakeInvocation::new(
            packet.invocation().arguments().to_vec(),
            wrong_cwd.canonicalize().unwrap(),
            packet
                .invocation()
                .environment()
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        )
        .unwrap()
        .with_argument_zero(packet.invocation().argument_zero().unwrap().to_owned())
        .unwrap();
        let invocation_bytes = invocation.to_bytes().unwrap();
        let changed_binding = MakeObservationBinding {
            phase: call.binding.phase,
            context_sha256: call.binding.context_sha256.clone(),
            invocation_sha256: sha256_bytes(&invocation_bytes),
            stdout_sha256: call.binding.stdout_sha256.clone(),
        };
        let changed_packet = RetainedMakeObservation::from_captured_bytes(
            &invocation_bytes,
            packet.initial_stdout(),
            packet.environment_names(),
            &changed_binding,
        )
        .unwrap();
        let changed_bytes = changed_packet.to_bytes().unwrap();
        files.insert(packet_name, changed_bytes.clone());
        let mut receipt: Receipt = serde_json::from_slice(&files[RECEIPT_MEMBER]).unwrap();
        let changed_call = &mut receipt.native_make.as_mut().unwrap().calls[call_index];
        changed_call.binding = changed_packet.binding().clone();
        changed_call.document_sha256 = sha256_bytes(&changed_bytes);
        files.insert(
            RECEIPT_MEMBER.into(),
            crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
        );
        refresh_collection_identity(&fixture.profile_document_sha256, &mut files);
        check_rejected(&files, "Mesa native Make packet data is inconsistent");
    }

    let mut files = fixture.files.clone();
    files.remove(&format!(
        "mesa-preflight-{}",
        native_packet_file(0).unwrap()
    ));
    check_rejected(
        &files,
        "Mesa preflight portable evidence differs from its exact member set",
    );

    let mut files = fixture.files.clone();
    let mut receipt: Receipt = serde_json::from_slice(&files[RECEIPT_MEMBER]).unwrap();
    receipt.schema = "aros-mesa-preflight-v10".into();
    files.insert(
        RECEIPT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(receipt).unwrap()).unwrap(),
    );
    let mut input: MesaConfigurationInputDocument =
        serde_json::from_slice(&files[INPUT_MEMBER]).unwrap();
    input.version = 10;
    input.local_source = Some(LocalSourceIdentity {
        schema: "aros-local-source-v1".into(),
        head_baseline: "0123456789abcdef0123456789abcdef01234567".into(),
        submodules_sha256: digest("selected submodule state"),
        generated_subtree: "build/mesa".into(),
        content_sha256: fixture.source_tree_sha256.clone(),
        entry_count: 1,
        regular_file_bytes: 42,
    });
    files.insert(
        INPUT_MEMBER.into(),
        crate::canonical::bytes(&serde_json::to_value(input).unwrap()).unwrap(),
    );
    refresh_collection_identity(&fixture.profile_document_sha256, &mut files);
    check_rejected(
        &files,
        "Mesa preflight receipt schema or qualification is invalid",
    );
    assert_eq!(rejected, 8);
}
