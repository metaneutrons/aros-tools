//! Opt-in full-owner execution using synthetic Source and actual GNU Make sampling.
//! The fixture MetaMake is not upstream MetaMake; no Mesa/SDK proof is implied.
use super::*;
use crate::compatibility::{
    prepare_mesa_native_direct_collection, CollectedMesaNativeDirect,
    Mesa26GeneratedPathListSelection, Mesa26OriginalRecipeExecution, MesaNativeCollectionSelection,
    MesaNativeDirectOrigin, MesaNativeSourceSelection, MesaPreflightRequest,
    NativeGenmoduleInventorySelection, NativeGenmoduleMakeSelection, NativeMakeIncludeExecution,
    SourceMesaPreflightProfile,
};
use aros_transpiler::make_vars::MakeFileEventKind;
use aros_transpiler::native_owner_projection::RetainedChildSourceBatchLimits;
use std::os::unix::fs::PermissionsExt as _;

const OWNER: &str = "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/mmakefile.src";
const GOAL: &str = "linklibs-gallium_vc4-gca";
const INTRINSIC_DYNAMIC_COOUTPUT: &str =
    "$(if $(wildcard $(TOP)/stamp),$(TOP)/generated-list-b,$(TOP)/generated-list-a)";

const THREE_LINE_BODY: &str = "printf 'first\\n' > $(TOP)/recipe-result\nprintf 'second\\n' >> $(TOP)/recipe-result\nprintf 'third\\n' >> $(TOP)/recipe-result";
const LATE_FAILURE_BODY: &str = "printf 'first\\n' > $(TOP)/recipe-result\nprintf 'second\\n' >> $(TOP)/recipe-result; printf 'late failure\\n' >&2; exit 7\nprintf 'third\\n' >> $(TOP)/recipe-result";
const PLAN_DRIFT_BODY: &str = "printf 'first\\n' > $(TOP)/recipe-result\nprintf '%s\\n' '$(if $(wildcard $(TOP)/recipe-result),changed,missing)' >> $(TOP)/recipe-result\nprintf 'third\\n' >> $(TOP)/recipe-result";

#[derive(Clone, Copy)]
enum SequenceCase {
    Success,
    StartLaterLine,
    SkipNextLine,
    ReplayPreviousLine,
    TamperPreviousEvidence,
    TamperPreviousOutput,
    LateFailure,
    PlanDrift,
}

#[derive(Clone, Copy)]
enum DependentCase {
    Success,
    IncompleteProducer,
    ForgedSideFile,
    ReplayCompletedRule,
    DeclaredCoOutputAlias,
}

#[derive(Clone, Copy)]
enum GeneratedPathCase {
    Success,
    UndeclaredList,
    PreexistingMember,
    OutsideEntriesRoot,
    ShellSyntax,
    TamperedMember,
    ForeignNoopEffect,
}

#[derive(Clone, Copy)]
enum IntrinsicProducerCase {
    Success,
    NoPolicy,
    SelectionDrift,
}

#[derive(Clone, Copy)]
enum NativeInventoryCase {
    Success,
    WrongObservedOwner,
    WrongBinding,
    WrongHelperDigest,
    TamperedEvidence,
}

#[derive(Clone, Copy, Debug)]
enum NativeGenmoduleMakeCase {
    Success,
    ConfiguredOwnerPath,
    SourcePathAssignment,
    SourcePathUnexport,
    WrongTuple,
    WrongRecipeIndex,
    WrongRole,
    MissingRoleQuery,
    UndeclaredExtraOutput,
    TamperedDataEvidence,
    ForeignSiblingRecipe,
    TamperedSourceBeforeData,
    TamperedInventoryEvidenceBeforeData,
}

#[derive(Clone, Copy)]
enum RunCaseMode {
    Single {
        expect_success: bool,
        cancellation_before_launch: bool,
    },
    Sequence(SequenceCase),
    Dependent(DependentCase),
    GeneratedPaths(GeneratedPathCase),
    IntrinsicProducer(IntrinsicProducerCase),
    NativeInventory(NativeInventoryCase),
    NativeGenmoduleMake(NativeGenmoduleMakeCase),
    NativeGenmoduleIncludeReader,
    NativeGenmoduleIncludeExecution(IncludeExecutionCase),
}

#[derive(Clone, Copy)]
enum IncludeExecutionCase {
    Success,
    UnboundInterpreter,
    UnfinishedConfiguration,
    ExtraOutput,
    FailedGenerator,
    ChangedOutput,
}

struct SyntheticNativeInventoryHelper {
    path: PathBuf,
    actual_sha256: aros_common::Sha256Digest,
    selected_sha256: aros_common::Sha256Digest,
    scope: aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope,
    evidence_root: PathBuf,
    launched_marker: PathBuf,
}

#[derive(Clone, Copy)]
struct NativeGenmoduleMakeFixture<'a> {
    output_root: &'a Path,
    nominal_destination: &'a Path,
    inputs: &'a DispatchInputs,
    helper: &'a SyntheticNativeInventoryHelper,
    scenario: NativeGenmoduleMakeCase,
}

struct NativeGenmoduleIncludeReaderFixture {
    include_target: PathBuf,
    directory_target: PathBuf,
    runtime_makefile: PathBuf,
    runtime_makefile_bytes: Vec<u8>,
    source_configuration: PathBuf,
    nominal_destination: PathBuf,
    genmodule_program: PathBuf,
    genmodule_launched: PathBuf,
}

impl SyntheticNativeInventoryHelper {
    fn selection(&self) -> NativeGenmoduleInventorySelection<'_> {
        NativeGenmoduleInventorySelection {
            helper: &self.path,
            helper_sha256: &self.selected_sha256,
            scope: self.scope,
            evidence_root: &self.evidence_root,
        }
    }
}

fn source_fixture(fixture: &mut Fixture, body: &str) -> (SourceMesaPreflightProfile, String) {
    let recipe = body
        .lines()
        .map(|line| format!("\t@{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let rules = format!("{GOAL}:\n{recipe}");
    source_fixture_with_rules(fixture, &rules)
}

fn source_fixture_with_rules(
    fixture: &mut Fixture,
    rules: &str,
) -> (SourceMesaPreflightProfile, String) {
    source_fixture_with_rules_and_producer_policy(fixture, rules, &[], &[], "")
}

type SourceProducerPolicyEntry<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);

fn source_fixture_with_rules_and_producer_policy(
    fixture: &mut Fixture,
    rules: &str,
    producer_policy: &[SourceProducerPolicyEntry<'_>],
    additional_metamake_goals: &[&str],
    additional_global_config: &str,
) -> (SourceMesaPreflightProfile, String) {
    let source = &fixture.source_root;
    let context = aros_transpiler::TargetContext {
        cpu: Some("arm".into()),
        platform: Some("raspi".into()),
        family: Some("arm-native".into()),
        variant: Some("raspi".into()),
        toolchain: Some("llvm".into()),
        cpu32: Some(String::new()),
        use_mmu: Some("1".into()),
        float_abi: Some("hard".into()),
        mesa_version: Some("26.0.0".into()),
        ..Default::default()
    };
    let mut globals = String::new();
    let mut absent = Vec::new();
    for name in aros_common::native_build_contract::NATIVE_RESERVED_MAKE_VARIABLES {
        if let Some(value) = context.value_of(name) {
            writeln!(globals, "{name} := {value}").unwrap();
        } else {
            absent.push(*name);
        }
    }
    globals.push_str(additional_global_config);
    let configuration = "[Fixture]\ndefaultmakefilename mmakefile\nignoredir .git\nglobalvarfile $(TOP)/config/make.cfg\ngenmakefiledeps $(SRCDIR)/fixture.tmpl\ngenmakefilescript fixture-genmf\nmaketool make -r\n";
    let template = "%define common\n%end\n";
    let mut additional_declarations = String::new();
    for goal in additional_metamake_goals {
        writeln!(additional_declarations, "#MM {goal}").unwrap();
    }
    let program = format!(
        "#MM {GOAL}\n{additional_declarations}{globals}TOP := {}\nSHELL := /bin/sh\n.SHELLFLAGS := -c\n{rules}\n",
        fixture.output_root.join("configure").display()
    );
    fs::create_dir_all(source.join(OWNER).parent().unwrap()).unwrap();
    fs::write(source.join(OWNER), &program).unwrap();
    fs::write(source.join("fixture.config"), configuration).unwrap();
    fs::write(source.join("fixture.tmpl"), template).unwrap();
    fs::write(source.join("selectors.mk"), &globals).unwrap();
    let mut policy = serde_json::json!({
        "schema_version":1,"kind":"native-metamake-policy-v1","profile":PROFILE_NAME,
        "project":"Fixture","configuration_source":"fixture.config","template":"fixture.tmpl",
        "substitutions":[],"global_snapshots":[{"source":"selectors.mk",
            "configured_path":"$(TOP)/config/make.cfg","text":globals}],
        "environment":[],"host_environment":[{"host":"fixture-host","bindings":[]}],
        "declared_absent":absent,"evidence_sources":["selectors.mk"],
        "closed_environment":true,"out_of_source":true,
    });
    if !producer_policy.is_empty() {
        policy["mesa26_original_producers"] = serde_json::Value::Array(
            producer_policy
                .iter()
                .map(|(source_input, goal, producer, cooutput, consumer)| {
                    serde_json::json!({
                        "source_input":source_input,
                        "goal":goal,
                        "producer":producer,
                        "cooutput":cooutput,
                        "consumer":consumer,
                    })
                })
                .collect(),
        );
    }
    fs::write(
        source.join("policy.json"),
        serde_json::to_vec(&policy).unwrap(),
    )
    .unwrap();
    let profile_text = format!(
        "[[targets]]\nname='{PROFILE_NAME}'\ntoolchain_profile='{PROFILE_NAME}'\narch='arm'\nplatform='raspi'\nbsp='generic'\nfloat_abi='hard'\nsdk_consumer_contract='contract.json'\n[targets.transpiler]\nfamily='arm-native'\nvariant='raspi'\ntoolchain='llvm'\ncpu32=''\nuse_mmu=true\nmesa_version='26.0.0'\n"
    );
    fs::write(source.join("aros-targets.toml"), profile_text).unwrap();
    for name in ["probe.c", "probe.cpp"] {
        fs::write(source.join(name), "int probe(void) { return 0; }\n").unwrap();
    }
    let inputs = ["aros-targets.toml", "policy.json", "fixture.config", "fixture.tmpl", "selectors.mk", "probe.c", "probe.cpp"]
        .map(|name| serde_json::json!({"path":name,"sha256":sha256_bytes(&fs::read(source.join(name)).unwrap())}));
    let target = SdkTarget {
        target_triple: PROFILE_TRIPLE.into(),
        abi: BTreeMap::from([
            ("source_cpu".into(), SdkAbiValue::Text("arm".into())),
            ("pointer_width".into(), SdkAbiValue::Unsigned(32)),
            ("float_abi".into(), SdkAbiValue::Text("hard".into())),
            ("use_mmu".into(), SdkAbiValue::Boolean(true)),
            (
                "source_declared_isa".into(),
                SdkAbiValue::Text("fixture-isa".into()),
            ),
        ]),
    };
    fs::write(
        source.join("contract.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema":"aros-sdk-consumer-contract-v1","profile":PROFILE_NAME,
            "source_baseline":"0123456789abcdef0123456789abcdef01234567",
            "roots":["includes","linklibs"],"inputs":inputs,"target":target,
            "mesa_configure_target":PROFILE_NAME,"metamake_projection":"policy.json",
            "native_sdk_link_probes":{"c":{"source":"probe.c","libraries":[]},
                "cxx":{"source":"probe.cpp","libraries":[]}},
        }))
        .unwrap(),
    )
    .unwrap();
    let profile = aros_common::TargetProfile::load_config(&source.join("aros-targets.toml"))
        .unwrap()
        .targets
        .remove(0);
    let profile = SourceMesaPreflightProfile::load(source, &profile).unwrap();
    fixture.drivers = inspect_llvm_sdk_drivers(
        &LlvmSdkDriverRequest {
            compiler_root: fixture.compiler_root.clone(),
            compiler: ArosCompilerIdentity::Llvm {
                version: COMPILER_VERSION.into(),
            },
            target: profile.target().clone(),
            timeout: Duration::from_secs(10),
        },
        &CancellationToken::default(),
    )
    .unwrap();
    let mut configure = fs::OpenOptions::new()
        .append(true)
        .open(source.join("configure"))
        .unwrap();
    writeln!(
        configure,
        "printf '%s' {} > mmake.config",
        shell_quote(configuration)
    )
    .unwrap();
    writeln!(
        configure,
        "printf '%s' {} >> config/make.cfg",
        shell_quote(&globals)
    )
    .unwrap();
    let expanded = aros_transpiler::genmf_projection::expand_files(
        &source.join(OWNER),
        &source.join("fixture.tmpl"),
        aros_transpiler::genmf_projection::Limits::default(),
    )
    .unwrap()
    .native_bytes()
    .unwrap();
    let owner_dir = fixture
        .output_root
        .join("configure")
        .join(Path::new(OWNER).parent().unwrap());
    let lines = std::str::from_utf8(&expanded)
        .unwrap()
        .lines()
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ");
    let mmake = format!(
        "#!/bin/sh\nset -eu\nmkdir -p {}\nprintf '%s\\n' {lines} > {}\n: > {cache}\ncommand=\nwhile IFS= read -r line; do\ncase \"$line\" in 'maketool '*) command=${{line#maketool }};; esac\ndone < \"$MMAKE_CONFIG\"\n[ -n \"$command\" ] || exit 71\ncd {}\n# Fixture paths contain no whitespace; the selected launcher is checked literal data.\n$command -r -R -f mmakefile {GOAL}\n",
        shell_quote(owner_dir.to_str().unwrap()),
        shell_quote(owner_dir.join("mmakefile").to_str().unwrap()),
        shell_quote(owner_dir.to_str().unwrap()),
        cache = shell_quote(
            fixture
                .output_root
                .join("configure/mmake.cache")
                .to_str()
                .unwrap()
        )
    );
    (profile, mmake)
}

fn synthetic_native_inventory_helper(
    fixture: &Fixture,
    profile: &SourceMesaPreflightProfile,
    scenario: NativeInventoryCase,
    include_override: bool,
) -> SyntheticNativeInventoryHelper {
    use aros_common::mesa_configuration_input::{
        MESA_GENMODULE_BASE_CONFIG, MESA_GENMODULE_OVERRIDE_CONFIG,
    };
    use aros_common::mesa_genmodule_collection::MESA_GENMODULE_SELECTION_FILE;
    use aros_common::mesa_genmodule_inventory::{
        MesaGenmoduleInventoryScope, MESA_GENMODULE_INVENTORY_SCHEMA,
        MESA_GENMODULE_SELECTION_SCHEMA,
    };

    let scope = MesaGenmoduleInventoryScope::ContractRoots;
    let binding = profile.genmodule_inventory_binding(scope);
    let observed_owner = if matches!(scenario, NativeInventoryCase::WrongObservedOwner) {
        "foreign-observed-owner"
    } else {
        GOAL
    };
    let mut selected_binding = serde_json::to_value(&binding).unwrap();
    if matches!(scenario, NativeInventoryCase::WrongBinding) {
        selected_binding["profile"] = serde_json::Value::String("foreign-profile".into());
    }
    let declaration_directory = Path::new(OWNER)
        .parent()
        .expect("selected Source owner has a declaration directory")
        .to_string_lossy();
    let document = serde_json::to_string(&serde_json::json!({
        "schema": MESA_GENMODULE_SELECTION_SCHEMA,
        "selection": selected_binding,
        "inventory": {
            "schema": MESA_GENMODULE_INVENTORY_SCHEMA,
            "records": [{
                "owner": observed_owner,
                "declaration_directory": declaration_directory,
                "module": "mesa3dgl26-0",
                "module_type": "library",
                "base_configuration": MESA_GENMODULE_BASE_CONFIG,
                "override_configuration": include_override
                    .then_some(MESA_GENMODULE_OVERRIDE_CONFIG),
                "consumer_path": format!("runtime/{observed_owner}"),
                "components": [],
            }],
        },
    }))
    .unwrap();
    let path = fixture
        .output_root
        .with_file_name("selected-genmodule-helper");
    let launched_marker = path.with_extension("invoked");
    let evidence_root = fixture
        .output_root
        .with_file_name("selected-genmodule-evidence");
    let selection_path = evidence_root.join(MESA_GENMODULE_SELECTION_FILE);
    let persistent_lock = aros_transpiler::publication::Publication::for_output(selection_path)
        .persistent_lock_path()
        .unwrap();
    let arguments = profile.genmodule_inventory_arguments(scope).unwrap();
    let profile_flag = arguments
        .windows(2)
        .find(|pair| pair[0] == "--native-consumer-profile")
        .expect("contract-roots invocation must select its Source profile")[1]
        .as_str();
    let digest_flag = arguments
        .windows(2)
        .find(|pair| pair[0] == "--native-consumer-contract-sha256")
        .expect("contract-roots invocation must select its sealed contract")[1]
        .as_str();
    let script = format!(
        "#!/bin/sh\nset -eu\nprintf invoked > {}\noutput=\nprofile_seen=0\ncontract_seen=0\ninventory_seen=0\nwhile [ \"$#\" -gt 0 ]; do\n  case \"$1\" in\n    --mesa-genmodule-inventory-only) inventory_seen=1 ;;\n    --native-consumer-profile) shift; [ \"$1\" = {} ] || exit 41; profile_seen=1 ;;\n    --native-consumer-contract-sha256) shift; [ \"$1\" = {} ] || exit 42; contract_seen=1 ;;\n    --output) shift; output=$1 ;;\n  esac\n  shift\ndone\n[ \"$inventory_seen\" = 1 ] || exit 43\n[ \"$profile_seen\" = 1 ] || exit 44\n[ \"$contract_seen\" = 1 ] || exit 45\n[ -n \"$output\" ] || exit 46\nprintf '%s\\n' {} > \"$output\"\n: > {}\n",
        shell_quote(launched_marker.to_str().unwrap()),
        shell_quote(profile_flag),
        shell_quote(digest_flag),
        shell_quote(&document),
        shell_quote(persistent_lock.to_str().unwrap()),
    );
    write_executable(&path, script.as_bytes());
    let actual_sha256 = sha256_bytes(&fs::read(&path).unwrap());
    let selected_sha256 = if matches!(scenario, NativeInventoryCase::WrongHelperDigest) {
        sha256_bytes(b"different selected helper bytes")
    } else {
        actual_sha256.clone()
    };
    SyntheticNativeInventoryHelper {
        path,
        actual_sha256,
        selected_sha256,
        scope,
        evidence_root,
        launched_marker,
    }
}

fn native_genmodule_make_source_fixture(
    fixture: &mut Fixture,
    scenario: NativeGenmoduleMakeCase,
) -> (SourceMesaPreflightProfile, String, PathBuf) {
    use aros_common::mesa_configuration_input::MESA_GENMODULE_BASE_CONFIG;

    let generated_program = fixture
        .output_root
        .join("configure/tools/MetaMake/generated/genmodule");
    let command_program = if matches!(scenario, NativeGenmoduleMakeCase::WrongRole) {
        fixture
            .output_root
            .join("configure/tools/MetaMake/generated/other-genmodule")
    } else {
        generated_program
    };
    let base_configuration = fixture.source_root.join(MESA_GENMODULE_BASE_CONFIG);
    let nominal_destination = fixture
        .output_root
        .join("configure/native-genmodule-nominal");
    let module_type = if matches!(scenario, NativeGenmoduleMakeCase::WrongTuple) {
        "resource"
    } else {
        "library"
    };
    let command = format!(
        "{} -c {} -d {} writefiles mesa3dgl26-0 {module_type}",
        shell_quote(command_program.to_str().unwrap()),
        shell_quote(base_configuration.to_str().unwrap()),
        shell_quote(nominal_destination.to_str().unwrap()),
    );
    let configured_path = fixture.output_root.join("configure/configured-path");
    let selected_host_path = fixture.output_root.parent().unwrap().join("host-tools");
    let configured_global_path = format!(
        "PATH := {}:{}\n",
        configured_path.display(),
        selected_host_path.display()
    );
    let path_export = if matches!(
        scenario,
        NativeGenmoduleMakeCase::ConfiguredOwnerPath
            | NativeGenmoduleMakeCase::SourcePathAssignment
            | NativeGenmoduleMakeCase::SourcePathUnexport
    ) {
        "export PATH\n"
    } else {
        ""
    };
    let source_path_directive = match scenario {
        NativeGenmoduleMakeCase::SourcePathAssignment => {
            "export PATH := $(PATH):$(TOP)/source-owner-path\n"
        }
        NativeGenmoduleMakeCase::SourcePathUnexport => "unexport PATH\n",
        _ => "",
    };
    let rules = if matches!(scenario, NativeGenmoduleMakeCase::ForeignSiblingRecipe) {
        format!(
            "{path_export}{source_path_directive}unexport SOURCE_UNEXPORT_SENTINEL\n{GOAL}: selected-stamp\nselected-stamp:\n\t@{command}\nFOREIGN_GOAL: sibling-stamp\nsibling-stamp:\n\t@{command}"
        )
    } else {
        format!("{path_export}{source_path_directive}unexport SOURCE_UNEXPORT_SENTINEL\n{GOAL}: stamp\nstamp:\n\t@{command}")
    };
    let additional_metamake_goals =
        if matches!(scenario, NativeGenmoduleMakeCase::ForeignSiblingRecipe) {
            &["FOREIGN_GOAL"][..]
        } else {
            &[]
        };
    let (profile, mut mmake) = source_fixture_with_rules_and_producer_policy(
        fixture,
        &rules,
        &[],
        additional_metamake_goals,
        if matches!(
            scenario,
            NativeGenmoduleMakeCase::ConfiguredOwnerPath
                | NativeGenmoduleMakeCase::SourcePathAssignment
                | NativeGenmoduleMakeCase::SourcePathUnexport
        ) {
            &configured_global_path
        } else {
            ""
        },
    );
    if !path_export.is_empty() {
        let make_command = format!("$command -r -R -f mmakefile {GOAL}\n");
        let exported_path = format!(
            "{}:{}",
            configured_path.display(),
            selected_host_path.display()
        );
        let shell_export = format!(
            "export PATH={}\n{make_command}",
            shell_quote(&exported_path)
        );
        assert_eq!(mmake.matches(&make_command).count(), 1);
        mmake = mmake.replacen(&make_command, &shell_export, 1);
    }
    let original = format!("$command -r -R -f mmakefile {GOAL}\n");
    let selected_goals = if matches!(scenario, NativeGenmoduleMakeCase::ForeignSiblingRecipe) {
        format!("{GOAL} FOREIGN_GOAL")
    } else {
        GOAL.to_owned()
    };
    let sampled_call = format!(
        "$command -r -R -f mmakefile NATIVE_GENMODULE_CLI=selected-probe {selected_goals}\n"
    );
    assert_eq!(mmake.matches(&original).count(), 1);
    mmake = mmake.replacen(&original, &sampled_call, 1);
    assert!(mmake.contains(&sampled_call));
    (profile, mmake, nominal_destination)
}

fn native_genmodule_include_reader_source_fixture(
    fixture: &mut Fixture,
    unfinished_configuration: bool,
) -> (
    SourceMesaPreflightProfile,
    String,
    NativeGenmoduleIncludeReaderFixture,
    String,
) {
    let runtime_makefile = fixture.output_root.join("configure/runtime-config.mk");
    let nominal_destination = fixture.output_root.join("configure/genmodule-destination");
    let include_target = nominal_destination.join("Makefile.mesa3dgl26-0library");
    let directory_target = nominal_destination.clone();
    // `fixture.config` is a positively selected input in the synthetic Source
    // profile, unlike a freshly created path that the original reader could not
    // independently admit.
    let source_configuration = fixture.source_root.join("fixture.config");
    let genmodule_program = fixture
        .output_root
        .join("configure/tools/MetaMake/generated/genmodule");
    let genmodule_launched = fixture
        .output_root
        .with_file_name("genmodule-include-reader-launched");

    let runtime_makefile_bytes = format!(
        "NATIVE_GENMODULE_PROGRAM := {}\n",
        genmodule_program.display()
    )
    .into_bytes();
    let command = format!(
        "$(NATIVE_GENMODULE_PROGRAM) -c {} -d {} writemakefile mesa3dgl26-0 library",
        shell_quote(source_configuration.to_str().unwrap()),
        shell_quote(nominal_destination.to_str().unwrap()),
    );
    let rules = format!(
        concat!(
            "include {runtime_makefile}\n",
            "-include {include_target}\n",
            "{GOAL}: {include_target}\n",
            "{include_target}: {genmodule_program} {source_configuration} | {directory_target}\n",
            "\t@{command}\n",
            "{directory_target}:\n",
            "\t@mkdir -p {directory_target}\n",
        ),
        runtime_makefile = runtime_makefile.display(),
        include_target = include_target.display(),
        GOAL = GOAL,
        genmodule_program = genmodule_program.display(),
        source_configuration = source_configuration.display(),
        directory_target = directory_target.display(),
        command = command,
    );
    let rules = if unfinished_configuration {
        format!(
            "{rules}\n{}:\n\t@printf unexecuted-configuration-producer\n",
            source_configuration.display()
        )
    } else {
        rules
    };
    let (profile, mmake) = source_fixture_with_rules(fixture, &rules);
    // Produce runtime configuration inside the original configure stage, not
    // by adding an unowned file after the collection inventory was measured.
    let mut configure = fs::OpenOptions::new()
        .append(true)
        .open(fixture.source_root.join("configure"))
        .unwrap();
    writeln!(
        configure,
        "printf '%s' {} > runtime-config.mk",
        shell_quote(std::str::from_utf8(&runtime_makefile_bytes).unwrap())
    )
    .unwrap();
    let script = format!(
        "#!/bin/sh\nset -eu\nprintf launched > {}\nexit 91\n",
        shell_quote(genmodule_launched.to_str().unwrap()),
    );
    (
        profile,
        mmake,
        NativeGenmoduleIncludeReaderFixture {
            include_target,
            directory_target,
            runtime_makefile,
            runtime_makefile_bytes,
            source_configuration,
            nominal_destination,
            genmodule_program,
            genmodule_launched,
        },
        script,
    )
}

fn original_include_generator_source(
    fixture: &NativeGenmoduleIncludeReaderFixture,
    scenario: IncludeExecutionCase,
) -> String {
    let mut assignments = String::new();
    for suffix in aros_common::mesa_genmodule_make::MESA_GENMODULE_MAKE_SUFFIXES {
        writeln!(
            assignments,
            "if (fputs(\"mesa3dgl26-0_{suffix} +=\\n\", out) < 0) return 45;"
        )
        .unwrap();
    }
    let extra = match scenario {
        IncludeExecutionCase::ExtraOutput => "char extra[4096]; if (snprintf(extra, sizeof(extra), \"%s/unselected\", argv[4]) < 0) return 46; FILE *unexpected = fopen(extra, \"w\"); if (!unexpected) return 47; fclose(unexpected);",
        IncludeExecutionCase::FailedGenerator => "return 43;",
        _ => "",
    };
    format!(
        "#include <stdio.h>\n#include <string.h>\n#include <sys/stat.h>\nint main(int argc, char **argv) {{\nif (argc != 8 || strcmp(argv[1], \"-c\") || strcmp(argv[2], {configuration}) || strcmp(argv[3], \"-d\") || strcmp(argv[4], {directory}) || strcmp(argv[5], \"writemakefile\") || strcmp(argv[6], \"mesa3dgl26-0\") || strcmp(argv[7], \"library\")) return 40;\nstruct stat info; if (stat(argv[4], &info) || !S_ISDIR(info.st_mode)) return 41;\nFILE *marker = fopen({marker}, \"w\"); if (!marker) return 42; fclose(marker);\n{extra}\nchar name[4096]; int n = snprintf(name, sizeof(name), \"%s/Makefile.mesa3dgl26-0library\", argv[4]); if (n < 0 || (unsigned)n >= sizeof(name)) return 44;\nFILE *out = fopen(name, \"w\"); if (!out) return 44;\n{assignments}\nreturn fclose(out) ? 45 : 0;\n}}\n",
        configuration = serde_json::to_string(fixture.source_configuration.to_str().unwrap()).unwrap(),
        directory = serde_json::to_string(fixture.nominal_destination.to_str().unwrap()).unwrap(),
        marker = serde_json::to_string(fixture.genmodule_launched.to_str().unwrap()).unwrap(),
    )
}

fn run_original_include_execution(
    collected: CollectedMesaNativeDirect,
    control: (
        &MesaPreflightRequest<'_>,
        &MesaNativeDirectOrigin<'_, '_>,
        &MesaNativeCollectionSelection<'_>,
        &CancellationToken,
    ),
    limits: RetainedChildSourceBatchLimits,
    fixture: &NativeGenmoduleIncludeReaderFixture,
    scenario: IncludeExecutionCase,
) {
    assert!(!fixture.nominal_destination.exists());
    assert!(!fixture.include_target.exists());
    let children = collected
        .read_children(control.0, control.1, control.2, limits, control.3)
        .unwrap();
    let child = children
        .into_iter()
        .next()
        .unwrap()
        .trace_declared_rules()
        .unwrap();
    let working = collected.calls()[0].invocation().working_directory();
    let event = child.file().events.iter().position(|event| event.active && matches!(&event.kind, MakeFileEventKind::Include { requested, present: false, .. } if working.join(requested) == fixture.include_target)).unwrap();
    let evidence = fixture
        .nominal_destination
        .with_file_name("original-include-evidence");
    // Evidence must be outside the entire prepared output, not its sibling target.
    let evidence = control
        .0
        .output_root
        .with_file_name(evidence.file_name().unwrap());
    let result = collected.generate_original_make_include(
        control,
        limits,
        &NativeMakeIncludeExecution {
            child_slot: 0,
            include_event: event,
            evidence_root: &evidence,
        },
    );
    if matches!(scenario, IncludeExecutionCase::UnboundInterpreter) {
        let Err(error) = result else {
            panic!("unbound script interpreter was launched");
        };
        assert!(
            error.to_string().contains("must be native"),
            "unexpected script refusal: {error}"
        );
        assert!(!fixture.nominal_destination.exists());
        assert!(!fixture.genmodule_launched.exists());
        return;
    }
    if matches!(scenario, IncludeExecutionCase::UnfinishedConfiguration) {
        assert!(
            result.is_err(),
            "an unresolved configuration producer must not be skipped"
        );
        assert!(!fixture.nominal_destination.exists());
        assert!(!fixture.genmodule_launched.exists());
        return;
    }
    if matches!(
        scenario,
        IncludeExecutionCase::ExtraOutput | IncludeExecutionCase::FailedGenerator
    ) {
        let Err(error) = result else {
            panic!("invalid original include producer succeeded");
        };
        let expected = if matches!(scenario, IncludeExecutionCase::ExtraOutput) {
            "GENMODULE changed unselected outputs"
        } else {
            "child failed; inspect retained stage logs"
        };
        assert!(
            error.to_string().contains(expected),
            "unexpected producer refusal: {error}"
        );
        assert!(
            fixture.nominal_destination.is_dir(),
            "the original directory step must have executed first"
        );
        assert!(
            fixture.genmodule_launched.is_file(),
            "the original GENMODULE process must actually run"
        );
        return;
    }
    let generated =
        result.unwrap_or_else(|error| panic!("original include production failed: {error}"));
    assert_eq!(generated.include_path(), fixture.include_target);
    assert!(fixture.nominal_destination.is_dir());
    assert!(fixture.include_target.is_file());
    assert!(fixture.genmodule_launched.is_file());
    generated.revalidate(control).unwrap();
    if matches!(scenario, IncludeExecutionCase::ChangedOutput) {
        fs::write(&fixture.include_target, b"changed\n").unwrap();
        assert!(
            generated.revalidate(control).is_err(),
            "changed producer output must not be reused"
        );
        return;
    }
    let (generated, children) = generated
        .read_declared_children(control, limits)
        .expect("original include must be read at its original site after completion");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].coordinates().goals(), &[GOAL.to_owned()]);
    generated.revalidate(control).unwrap();
    let (generated, original_child) = generated.read_declared_child(control, limits).unwrap();
    assert_eq!(original_child.coordinates().goals(), &[GOAL.to_owned()]);
    generated.revalidate(control).unwrap();
}

fn native_genmodule_make_script(fixture: &Fixture, scenario: NativeGenmoduleMakeCase) -> String {
    use aros_common::{
        mesa_configuration_input::MESA_GENMODULE_BASE_CONFIG,
        mesa_genmodule_make::MESA_GENMODULE_MAKE_SUFFIXES,
    };

    let base_configuration = fixture.source_root.join(MESA_GENMODULE_BASE_CONFIG);
    let assignments = MESA_GENMODULE_MAKE_SUFFIXES.join(" ");
    let extra_output = if matches!(scenario, NativeGenmoduleMakeCase::UndeclaredExtraOutput) {
        ": > \"$destination/unexpected\"\n"
    } else {
        ""
    };
    let configured_path_check = if matches!(
        scenario,
        NativeGenmoduleMakeCase::ConfiguredOwnerPath
            | NativeGenmoduleMakeCase::SourcePathAssignment
            | NativeGenmoduleMakeCase::SourcePathUnexport
    ) {
        let configured = fixture.output_root.join("configure/configured-path");
        let closure = fixture.output_root.parent().unwrap().join("host-tools");
        format!(
            "[ \"$PATH\" = {} ] || exit 36\n",
            shell_quote(&format!("{}:{}", configured.display(), closure.display()))
        )
    } else {
        String::new()
    };
    format!(
        "#!/bin/sh\nset -eu\n[ \"${{DISPATCH_SENTINEL-}}\" = held ] || exit 31\n[ -z \"${{SOURCE_UNEXPORT_SENTINEL+x}}\" ] || exit 32\n[ \"${{NATIVE_GENMODULE_CLI-}}\" = selected-probe ] || exit 35\n{configured_path_check}[ \"$#\" -eq 7 ] && [ \"$1\" = -c ] && [ \"$2\" = {} ] && [ \"$3\" = -d ] || exit 33\ndestination=$4\n[ \"$5\" = writemakefile ] && [ \"$6\" = mesa3dgl26-0 ] && [ \"$7\" = library ] || exit 34\nmkdir -p \"$destination\"\nfor suffix in {assignments}; do printf '%s +=\\n' \"mesa3dgl26-0_$suffix\"; done > \"$destination/Makefile.mesa3dgl26-0library\"\nprintf 'dispatch=held source-unexport=unset\\n'\n{extra_output}",
        shell_quote(base_configuration.to_str().unwrap()),
    )
}

const NO_RECIPE_OUTPUTS: &[&str] = &[];
const RECIPE_RESULT_OUTPUT: &[&str] = &["recipe-result"];
const PRODUCER_OUTPUT: &[&str] = &["producer"];
const PRODUCER_AND_UNOWNED_OUTPUTS: &[&str] = &["producer", "unowned"];
const PRODUCER_AND_SIDE_OUTPUTS: &[&str] = &["producer", "side"];
const CONSUMER_OUTPUT: &[&str] = &["consumer"];
const GENERATED_LIST_OUTPUTS: &[&str] = &["recipe-result", "generated.list"];
const GENERATED_LIST_AND_OUTSIDE_OUTPUTS: &[&str] = &["recipe-result", "generated.list", "outside"];
const GENERATED_LIST_UNOWNED_OUTPUTS: &[&str] =
    &["recipe-result", "generated.list", "unowned.list"];
const MEMBERS_DIRECTORY: &[&str] = &["members"];

fn recipe_execution(
    recipe_index: usize,
    evidence_root: &Path,
) -> Mesa26OriginalRecipeExecution<'_> {
    Mesa26OriginalRecipeExecution {
        child_slot: 0,
        recipe_index,
        selected_shell: Path::new("/bin/sh"),
        evidence_root,
        mutable_files: if recipe_index == 0 {
            NO_RECIPE_OUTPUTS
        } else {
            RECIPE_RESULT_OUTPUT
        },
        replaceable_files: NO_RECIPE_OUTPUTS,
        fresh_files: if recipe_index == 0 {
            RECIPE_RESULT_OUTPUT
        } else {
            NO_RECIPE_OUTPUTS
        },
        fresh_directories: NO_RECIPE_OUTPUTS,
    }
}

fn fresh_output_execution<'a>(
    recipe_index: usize,
    fresh_files: &'a [&'a str],
    evidence_root: &'a Path,
) -> Mesa26OriginalRecipeExecution<'a> {
    fresh_effect_execution(recipe_index, fresh_files, NO_RECIPE_OUTPUTS, evidence_root)
}

fn fresh_effect_execution<'a>(
    recipe_index: usize,
    fresh_files: &'a [&'a str],
    fresh_directories: &'a [&'a str],
    evidence_root: &'a Path,
) -> Mesa26OriginalRecipeExecution<'a> {
    Mesa26OriginalRecipeExecution {
        child_slot: 0,
        recipe_index,
        selected_shell: Path::new("/bin/sh"),
        evidence_root,
        mutable_files: NO_RECIPE_OUTPUTS,
        replaceable_files: NO_RECIPE_OUTPUTS,
        fresh_files,
        fresh_directories,
    }
}

fn retained_child_limits() -> RetainedChildSourceBatchLimits {
    RetainedChildSourceBatchLimits {
        inputs: aros_common::make_input_snapshot::MakeInputBatchLimits {
            maximum_children: 1,
            maximum_read_bytes: 1024 * 1024,
            maximum_reads: 4096,
            maximum_queries: 4096,
            maximum_query_names: 4096,
            maximum_query_bytes: 1024 * 1024,
            maximum_record_bytes: 1024 * 1024,
        },
        maximum_retained_bytes: 8 * 1024 * 1024,
    }
}

fn run_sequence_after_collection(
    collected: CollectedMesaNativeDirect,
    control: (
        &MesaPreflightRequest<'_>,
        &MesaNativeDirectOrigin<'_, '_>,
        &MesaNativeCollectionSelection<'_>,
        &CancellationToken,
    ),
    limits: RetainedChildSourceBatchLimits,
    output_root: &Path,
    scenario: SequenceCase,
) {
    let target = output_root.join("configure/recipe-result");
    let evidence =
        |index: usize| output_root.with_file_name(format!("sequence-line-{index}-evidence"));

    if matches!(scenario, SequenceCase::StartLaterLine) {
        let later = collected.execute_mesa26_original_recipe(
            control,
            limits,
            &recipe_execution(1, &evidence(1)),
        );
        let Err(error) = later else {
            panic!("a later original recipe line ran without its predecessor");
        };
        assert!(error
            .to_string()
            .contains("unexecuted preceding recipe lines"));
        assert!(!evidence(1).exists());
        assert!(!target.exists());
        return;
    }

    let first = collected
        .execute_mesa26_original_recipe(control, limits, &recipe_execution(0, &evidence(0)))
        .unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"first\n");
    assert_eq!(first.evidence_snapshot().entry_count(), 2);

    match scenario {
        SequenceCase::Success => {
            let second = first
                .execute_next_mesa26_original_recipe(
                    control,
                    limits,
                    &recipe_execution(1, &evidence(1)),
                )
                .unwrap();
            assert_eq!(fs::read(&target).unwrap(), b"first\nsecond\n");
            assert_eq!(second.evidence_snapshot().entry_count(), 2);

            let third = second
                .execute_next_mesa26_original_recipe(
                    control,
                    limits,
                    &recipe_execution(2, &evidence(2)),
                )
                .unwrap();
            assert_eq!(fs::read(&target).unwrap(), b"first\nsecond\nthird\n");
            assert_eq!(third.evidence_snapshot().entry_count(), 2);
            assert!(evidence(0).is_dir());
            assert!(evidence(1).is_dir());
            assert!(evidence(2).is_dir());
            assert!(!output_root.join("receipt.json").exists());
        }
        SequenceCase::SkipNextLine | SequenceCase::ReplayPreviousLine => {
            let requested = if matches!(scenario, SequenceCase::SkipNextLine) {
                2
            } else {
                0
            };
            let result = first.execute_next_mesa26_original_recipe(
                control,
                limits,
                &recipe_execution(requested, &evidence(1)),
            );
            let Err(error) = result else {
                panic!("a skipped or replayed original recipe line was accepted");
            };
            assert!(error.to_string().contains("not the next owned line"));
            assert!(!evidence(1).exists());
            assert_eq!(fs::read(&target).unwrap(), b"first\n");
        }
        SequenceCase::TamperPreviousEvidence => {
            fs::write(evidence(0).join("stdout"), b"tampered evidence\n").unwrap();
            let result = first.execute_next_mesa26_original_recipe(
                control,
                limits,
                &recipe_execution(1, &evidence(1)),
            );
            let Err(error) = result else {
                panic!("changed prior evidence did not block the next original recipe line");
            };
            assert!(error
                .to_string()
                .contains("earlier original recipe evidence changed"));
            assert!(!evidence(1).exists());
            assert_eq!(fs::read(&target).unwrap(), b"first\n");
        }
        SequenceCase::TamperPreviousOutput => {
            fs::write(&target, b"first\nprevious-output-tamper\n").unwrap();
            let result = first.execute_next_mesa26_original_recipe(
                control,
                limits,
                &recipe_execution(1, &evidence(1)),
            );
            let Err(error) = result else {
                panic!("changed prior output did not block the next original recipe line");
            };
            assert!(!evidence(1).exists());
            assert_eq!(
                fs::read(&target).unwrap(),
                b"first\nprevious-output-tamper\n"
            );
            assert!(error
                .to_string()
                .contains("direct native output transition changed"));
        }
        SequenceCase::LateFailure => {
            let result = first.execute_next_mesa26_original_recipe(
                control,
                limits,
                &recipe_execution(1, &evidence(1)),
            );
            let Err(error) = result else {
                panic!("a failed second original recipe line returned sequence state");
            };
            assert!(error.to_string().contains("child failed"));
            assert_eq!(fs::read(&target).unwrap(), b"first\nsecond\n");
            assert_eq!(
                fs::read(evidence(1).join("stderr")).unwrap(),
                b"late failure\n"
            );
            assert!(
                !evidence(2).exists(),
                "the third line must not run after failure"
            );
        }
        SequenceCase::PlanDrift => {
            let result = first.execute_next_mesa26_original_recipe(
                control,
                limits,
                &recipe_execution(1, &evidence(1)),
            );
            let Err(error) = result else {
                panic!("a changed original rule plan was accepted for its next line");
            };
            assert!(error
                .to_string()
                .contains("original rule plan changed after its preceding line"));
            assert_eq!(fs::read(&target).unwrap(), b"first\n");
            assert!(
                !evidence(1).exists(),
                "plan drift must refuse before launch"
            );
        }
        SequenceCase::StartLaterLine => unreachable!("handled before line zero"),
    }
}

fn run_dependent_after_collection(
    collected: CollectedMesaNativeDirect,
    control: (
        &MesaPreflightRequest<'_>,
        &MesaNativeDirectOrigin<'_, '_>,
        &MesaNativeCollectionSelection<'_>,
        &CancellationToken,
    ),
    limits: RetainedChildSourceBatchLimits,
    output_root: &Path,
    scenario: DependentCase,
) {
    let build = output_root.join("configure");
    let producer_path = build.join("producer");
    let consumer_path = build.join("consumer");
    let evidence =
        |index: usize| output_root.with_file_name(format!("dependent-step-{index}-evidence"));
    let producer_outputs = match scenario {
        DependentCase::ForgedSideFile => PRODUCER_AND_UNOWNED_OUTPUTS,
        DependentCase::DeclaredCoOutputAlias => PRODUCER_AND_SIDE_OUTPUTS,
        _ => PRODUCER_OUTPUT,
    };
    let producer = collected
        .execute_mesa26_original_recipe(
            control,
            limits,
            &fresh_output_execution(0, producer_outputs, &evidence(0)),
        )
        .unwrap();

    match scenario {
        DependentCase::IncompleteProducer => {
            assert_eq!(fs::read(&producer_path).unwrap(), b"partial\n");
            let result = producer.execute_mesa26_dependent_recipe(
                control,
                limits,
                &fresh_output_execution(2, CONSUMER_OUTPUT, &evidence(1)),
            );
            let Err(error) = result else {
                panic!("a dependent rule ran before every producer recipe line completed");
            };
            assert!(error
                .to_string()
                .contains("dependent recipe has an incomplete or foreign producer"));
            assert!(!consumer_path.exists());
            assert!(!evidence(1).exists());
        }
        DependentCase::ReplayCompletedRule => {
            assert_eq!(fs::read(&producer_path).unwrap(), b"producer-bytes\n");
            let result = producer.execute_mesa26_dependent_recipe(
                control,
                limits,
                &fresh_output_execution(0, PRODUCER_OUTPUT, &evidence(1)),
            );
            let Err(error) = result else {
                panic!("a completed producer rule was accepted a second time");
            };
            assert!(error
                .to_string()
                .contains("dependent recipe repeats an already completed rule"));
            assert!(!evidence(1).exists());
            assert!(!consumer_path.exists());
        }
        DependentCase::Success
        | DependentCase::ForgedSideFile
        | DependentCase::DeclaredCoOutputAlias => {
            let expected_producer = if matches!(scenario, DependentCase::DeclaredCoOutputAlias) {
                b"co-output-bytes\n".as_slice()
            } else {
                b"producer-bytes\n".as_slice()
            };
            assert_eq!(fs::read(&producer_path).unwrap(), expected_producer);
            if matches!(scenario, DependentCase::ForgedSideFile) {
                assert_eq!(fs::read(build.join("unowned")).unwrap(), b"forged-side\n");
            }
            if matches!(scenario, DependentCase::DeclaredCoOutputAlias) {
                assert_eq!(fs::read(build.join("side")).unwrap(), expected_producer);
            }
            let result = producer.execute_mesa26_dependent_recipe(
                control,
                limits,
                &fresh_output_execution(1, CONSUMER_OUTPUT, &evidence(1)),
            );
            match scenario {
                DependentCase::Success | DependentCase::DeclaredCoOutputAlias => {
                    let consumer = result.unwrap_or_else(|error| {
                        panic!(
                            "synthetic dependent process failed: {error}; stderr: {}",
                            String::from_utf8_lossy(
                                &fs::read(evidence(1).join("stderr")).unwrap_or_default()
                            )
                        )
                    });
                    assert_eq!(fs::read(&consumer_path).unwrap(), expected_producer);
                    assert_eq!(consumer.evidence_snapshot().entry_count(), 2);
                    assert!(!output_root.join("receipt.json").exists());
                }
                DependentCase::ForgedSideFile => {
                    let Err(error) = result else {
                        panic!("an undeclared side file was accepted as producer lineage");
                    };
                    assert!(error
                        .to_string()
                        .contains("recipe prerequisite lacks a completed producer outcome"));
                    assert!(!consumer_path.exists());
                    assert!(!evidence(1).exists());
                }
                _ => unreachable!("handled above"),
            }
        }
    }
}

fn run_generated_path_list_after_collection(
    collected: CollectedMesaNativeDirect,
    control: (
        &MesaPreflightRequest<'_>,
        &MesaNativeDirectOrigin<'_, '_>,
        &MesaNativeCollectionSelection<'_>,
        &CancellationToken,
    ),
    limits: RetainedChildSourceBatchLimits,
    output_root: &Path,
    scenario: GeneratedPathCase,
) {
    let build = output_root.join("configure");
    let entries_root = build.join(
        if matches!(scenario, GeneratedPathCase::PreexistingMember) {
            "config"
        } else {
            "members"
        },
    );
    let preexisting_member = matches!(scenario, GeneratedPathCase::PreexistingMember)
        .then(|| fs::read(entries_root.join("make.cfg")).unwrap());
    let evidence = output_root.with_file_name("generated-list-producer-evidence");
    let (files, directories) = match scenario {
        GeneratedPathCase::UndeclaredList => (GENERATED_LIST_UNOWNED_OUTPUTS, MEMBERS_DIRECTORY),
        GeneratedPathCase::OutsideEntriesRoot => {
            (GENERATED_LIST_AND_OUTSIDE_OUTPUTS, MEMBERS_DIRECTORY)
        }
        _ => (GENERATED_LIST_OUTPUTS, MEMBERS_DIRECTORY),
    };
    let producer = collected
        .execute_mesa26_original_recipe(
            control,
            limits,
            &fresh_effect_execution(0, files, directories, &evidence),
        )
        .unwrap();
    assert!(
        entries_root.is_dir(),
        "the selected entries root is a real directory"
    );
    let producer = if matches!(scenario, GeneratedPathCase::ForeignNoopEffect) {
        let evidence = output_root.with_file_name("generated-list-foreign-evidence");
        let mut execution =
            fresh_effect_execution(1, NO_RECIPE_OUTPUTS, NO_RECIPE_OUTPUTS, &evidence);
        execution.mutable_files = &["members/a"];
        let next = producer
            .execute_mesa26_dependent_recipe(control, limits, &execution)
            .unwrap();
        assert_eq!(fs::read(entries_root.join("a")).unwrap(), b"A");
        next
    } else {
        producer
    };

    let list_file = build.join(if matches!(scenario, GeneratedPathCase::UndeclaredList) {
        "unowned.list"
    } else {
        "generated.list"
    });
    let member_a = entries_root.join("a");
    let member_b = entries_root.join("b");
    let selection = Mesa26GeneratedPathListSelection {
        list_file: &list_file,
        entries_root: &entries_root,
    };
    if matches!(scenario, GeneratedPathCase::TamperedMember) {
        fs::write(&member_a, b"tampered after producer completion").unwrap();
    }
    let result = producer.validate_mesa26_generated_path_list(control, &selection);
    match scenario {
        GeneratedPathCase::Success => {
            let paths = result.unwrap();
            assert_eq!(
                paths,
                vec![member_a.clone(), member_b.clone()],
                "the bounded Source list's order is retained"
            );
            assert_eq!(
                fs::read(&list_file).unwrap(),
                format!("{}\n{}\n", member_a.display(), member_b.display()).as_bytes()
            );
            assert_eq!(fs::read(build.join("recipe-result")).unwrap(), b"stamp");
            assert_eq!(fs::read(&member_a).unwrap(), b"A");
            assert_eq!(fs::read(&member_b).unwrap(), b"B");
        }
        GeneratedPathCase::UndeclaredList
        | GeneratedPathCase::PreexistingMember
        | GeneratedPathCase::OutsideEntriesRoot
        | GeneratedPathCase::ShellSyntax
        | GeneratedPathCase::TamperedMember
        | GeneratedPathCase::ForeignNoopEffect => {
            let Err(error) = result else {
                panic!("invalid generated path list/member provenance was accepted");
            };
            let expected = match scenario {
                GeneratedPathCase::UndeclaredList => {
                    "generated path list is not a declared completed producer output"
                }
                GeneratedPathCase::PreexistingMember | GeneratedPathCase::ForeignNoopEffect => {
                    "generated list member lacks its list producer's effects"
                }
                GeneratedPathCase::OutsideEntriesRoot => {
                    "generated list member escaped its selected directory"
                }
                GeneratedPathCase::ShellSyntax => {
                    "generated path list contains unsafe or excessive shell-word records"
                }
                GeneratedPathCase::TamperedMember => "direct native output transition changed",
                GeneratedPathCase::Success => unreachable!("handled above"),
            };
            assert!(
                error.to_string().contains(expected),
                "unexpected refusal: {error}"
            );
            assert_eq!(fs::read(build.join("recipe-result")).unwrap(), b"stamp");
            match scenario {
                GeneratedPathCase::UndeclaredList => {
                    assert!(list_file.is_file());
                    assert!(fs::read(&list_file).unwrap().is_empty());
                }
                GeneratedPathCase::PreexistingMember => {
                    assert_eq!(
                        fs::read(entries_root.join("make.cfg")).unwrap(),
                        preexisting_member.as_deref().unwrap()
                    );
                }
                GeneratedPathCase::OutsideEntriesRoot => {
                    assert_eq!(fs::read(build.join("outside")).unwrap(), b"outside");
                }
                GeneratedPathCase::ShellSyntax => {
                    assert!(fs::read(&list_file).unwrap().contains(&b';'));
                }
                GeneratedPathCase::TamperedMember => {
                    assert_eq!(
                        fs::read(&member_a).unwrap(),
                        b"tampered after producer completion"
                    );
                }
                GeneratedPathCase::ForeignNoopEffect => {
                    assert_eq!(fs::read(&member_a).unwrap(), b"A");
                }
                GeneratedPathCase::Success => unreachable!("handled above"),
            }
        }
    }
}

fn run_case(body: &str, expect_success: bool, cancellation_before_launch: bool) {
    run_case_with_mode(
        body,
        RunCaseMode::Single {
            expect_success,
            cancellation_before_launch,
        },
    );
}

fn run_sequence_case(scenario: SequenceCase) {
    let body = match scenario {
        SequenceCase::LateFailure => LATE_FAILURE_BODY,
        SequenceCase::PlanDrift => PLAN_DRIFT_BODY,
        _ => THREE_LINE_BODY,
    };
    run_case_with_mode(body, RunCaseMode::Sequence(scenario));
}

fn dependent_rule_program(scenario: DependentCase) -> String {
    let (producer, middle_rules, consumer_prerequisites) = match scenario {
        DependentCase::IncompleteProducer => (
            "\t@printf 'partial\\n' > $@\n\t@printf 'complete\\n' >> $@",
            String::new(),
            "$(TOP)/producer",
        ),
        DependentCase::ForgedSideFile => (
            "\t@printf 'producer-bytes\\n' > $@; printf 'forged-side\\n' > $(TOP)/unowned",
            String::new(),
            "$(TOP)/unowned $(TOP)/producer",
        ),
        DependentCase::DeclaredCoOutputAlias => (
            "\t@printf 'co-output-bytes\\n' > $@; printf 'co-output-bytes\\n' > $(TOP)/side",
            "$(TOP)/side: $(TOP)/producer\n".to_owned(),
            "$(TOP)/side",
        ),
        DependentCase::Success | DependentCase::ReplayCompletedRule => (
            "\t@printf 'producer-bytes\\n' > $@",
            String::new(),
            "$(TOP)/producer",
        ),
    };
    format!(
        "{GOAL}: $(TOP)/producer $(TOP)/consumer\n$(TOP)/producer:\n{producer}\n{middle_rules}$(TOP)/consumer: {consumer_prerequisites}\n\t@cat $< > $@\n"
    )
}

fn generated_path_rule_program(scenario: GeneratedPathCase) -> String {
    let listed_path = match scenario {
        GeneratedPathCase::PreexistingMember => "$(TOP)/config/make.cfg",
        GeneratedPathCase::OutsideEntriesRoot => "$(TOP)/outside",
        GeneratedPathCase::ShellSyntax => "$(TOP)/members/a;touch",
        _ => "$(TOP)/members/a' '$(TOP)/members/b",
    };
    let mut producer = format!(
        "\t@mkdir -p $(TOP)/members; printf A > $(TOP)/members/a; printf B > $(TOP)/members/b; printf '%s\\n' '{listed_path}' > $(TOP)/generated.list"
    );
    if matches!(scenario, GeneratedPathCase::UndeclaredList) {
        producer.push_str("; : > $(TOP)/unowned.list");
    }
    if matches!(scenario, GeneratedPathCase::OutsideEntriesRoot) {
        producer.push_str("; printf outside > $(TOP)/outside");
    }
    producer.push_str("; printf stamp > $@");
    format!(
        "{GOAL}: $(TOP)/recipe-result $(TOP)/consumer\n$(TOP)/recipe-result:\n{producer}\n$(TOP)/generated.list: $(TOP)/recipe-result\n$(TOP)/consumer: $(TOP)/generated.list\n\t@:\n"
    )
}

fn intrinsic_producer_policy(
    scenario: IntrinsicProducerCase,
) -> Vec<SourceProducerPolicyEntry<'static>> {
    match scenario {
        IntrinsicProducerCase::NoPolicy => Vec::new(),
        IntrinsicProducerCase::Success => vec![(
            OWNER,
            GOAL,
            "$(TOP)/stamp",
            "$(TOP)/generated.list",
            "$(TOP)/consumer",
        )],
        IntrinsicProducerCase::SelectionDrift => vec![(
            OWNER,
            GOAL,
            "$(TOP)/stamp",
            INTRINSIC_DYNAMIC_COOUTPUT,
            "$(TOP)/consumer",
        )],
    }
}

fn intrinsic_producer_rule_program(scenario: IntrinsicProducerCase) -> String {
    let (consumer_prerequisites, aliases, list_path) =
        if matches!(scenario, IntrinsicProducerCase::SelectionDrift) {
            (
                "$(TOP)/generated-list-a $(TOP)/generated-list-b",
                "$(TOP)/generated-list-a: $(TOP)/stamp\n$(TOP)/generated-list-b: $(TOP)/stamp\n",
                "$(TOP)/generated-list-a",
            )
        } else {
            (
                "$(TOP)/generated.list",
                "$(TOP)/generated.list: $(TOP)/stamp\n",
                "$(TOP)/generated.list",
            )
        };
    format!(
        "{GOAL}: $(TOP)/consumer\n$(TOP)/consumer: {consumer_prerequisites}\n\t@printf 'consumer\\n' > $@\n{aliases}$(TOP)/stamp:\n\t@printf 'first\\n' > $@; printf 'member\\n' > $(TOP)/member\n\t@printf '%s\\n' '$(TOP)/member' > {list_path}; printf 'second\\n' >> $@\n"
    )
}

fn intrinsic_recipe_execution(
    scenario: IntrinsicProducerCase,
    recipe_index: usize,
    evidence_root: &Path,
) -> Mesa26OriginalRecipeExecution<'_> {
    let (mutable_files, fresh_files): (&[&str], &[&str]) = match recipe_index {
        0 => (NO_RECIPE_OUTPUTS, &["stamp", "member"]),
        1 => (
            &["stamp"],
            if matches!(scenario, IntrinsicProducerCase::SelectionDrift) {
                &["generated-list-a"]
            } else {
                &["generated.list"]
            },
        ),
        _ => unreachable!("the selected producer has exactly two source recipe lines"),
    };
    Mesa26OriginalRecipeExecution {
        child_slot: 0,
        recipe_index,
        selected_shell: Path::new("/bin/sh"),
        evidence_root,
        mutable_files,
        replaceable_files: NO_RECIPE_OUTPUTS,
        fresh_files,
        fresh_directories: NO_RECIPE_OUTPUTS,
    }
}

fn run_intrinsic_producer_after_collection(
    collected: CollectedMesaNativeDirect,
    control: (
        &MesaPreflightRequest<'_>,
        &MesaNativeDirectOrigin<'_, '_>,
        &MesaNativeCollectionSelection<'_>,
        &CancellationToken,
    ),
    limits: RetainedChildSourceBatchLimits,
    output_root: &Path,
    scenario: IntrinsicProducerCase,
) {
    let build = output_root.join("configure");
    let stamp = build.join("stamp");
    let member = build.join("member");
    let list_file = build.join("generated.list");
    let evidence = |index: usize| {
        output_root.with_file_name(format!("intrinsic-producer-step-{index}-evidence"))
    };
    let first = collected.execute_mesa26_original_recipe(
        control,
        limits,
        &intrinsic_recipe_execution(scenario, 0, &evidence(0)),
    );
    if matches!(scenario, IntrinsicProducerCase::NoPolicy) {
        let Err(error) = first else {
            panic!("an unselected deep original producer was accepted");
        };
        assert!(!error.to_string().is_empty());
        assert!(!evidence(0).exists(), "refusal must precede launch");
        assert!(!stamp.exists());
        assert!(!member.exists());
        return;
    }

    let first = first.expect("the explicit Source producer selection should admit line one");
    assert_eq!(fs::read(&stamp).unwrap(), b"first\n");
    assert_eq!(fs::read(&member).unwrap(), b"member\n");
    assert!(evidence(0).is_dir());

    let second = first.execute_next_mesa26_original_recipe(
        control,
        limits,
        &intrinsic_recipe_execution(scenario, 1, &evidence(1)),
    );
    if matches!(scenario, IntrinsicProducerCase::SelectionDrift) {
        let Err(error) = second else {
            panic!("a changed source-selected cooutput was accepted for the next line");
        };
        assert!(error
            .to_string()
            .contains("original rule plan changed after its preceding line"));
        assert!(
            !evidence(1).exists(),
            "selection drift must refuse before launch"
        );
        assert_eq!(fs::read(&stamp).unwrap(), b"first\n");
        assert_eq!(fs::read(&member).unwrap(), b"member\n");
        assert!(!build.join("generated-list-a").exists());
        assert!(!build.join("generated-list-b").exists());
        return;
    }

    let second = second.expect("the second Source producer line should complete");
    assert_eq!(fs::read(&stamp).unwrap(), b"first\nsecond\n");
    assert_eq!(
        fs::read(&list_file).unwrap(),
        format!("{}\n", member.display()).as_bytes()
    );
    let selection = Mesa26GeneratedPathListSelection {
        list_file: &list_file,
        entries_root: &build,
    };
    assert_eq!(
        second
            .validate_mesa26_generated_path_list(control, &selection)
            .unwrap(),
        vec![member]
    );
    assert!(evidence(1).is_dir());
}

fn run_native_genmodule_inventory_after_collection(
    collected: CollectedMesaNativeDirect,
    control: (
        &MesaPreflightRequest<'_>,
        &MesaNativeDirectOrigin<'_, '_>,
        &MesaNativeCollectionSelection<'_>,
        &CancellationToken,
    ),
    helper: &SyntheticNativeInventoryHelper,
    scenario: NativeInventoryCase,
) {
    let selection = helper.selection();
    let result = collected.collect_genmodule_inventory(control, &selection);
    if matches!(scenario, NativeInventoryCase::WrongHelperDigest) {
        let Err(error) = result else {
            panic!("an inventory helper with the wrong selected digest was accepted");
        };
        assert!(!error.to_string().is_empty());
        assert_ne!(helper.actual_sha256, helper.selected_sha256);
        assert!(
            !helper.launched_marker.exists(),
            "helper digest refusal must precede launching the selected helper"
        );
        return;
    }

    if matches!(
        scenario,
        NativeInventoryCase::WrongObservedOwner | NativeInventoryCase::WrongBinding
    ) {
        let Err(error) = result else {
            panic!("a Source-inconsistent genmodule selection was accepted");
        };
        assert!(!error.to_string().is_empty());
        assert!(
            helper.launched_marker.is_file(),
            "the selected helper must emit the malformed or wrong-owner counter document"
        );
        return;
    }

    let (collected, selected) = result.expect("selected native genmodule inventory should bind");
    assert_eq!(selected.inventory().inventory().records.len(), 1);
    assert_eq!(selected.owners().len(), 1);
    assert_eq!(selected.owners()[0].owner, GOAL);
    assert_eq!(selected.owners()[0].call_index, 0);
    assert!(helper.launched_marker.is_file());
    selected
        .revalidate(&collected, control)
        .expect("the selected original inventory evidence should revalidate");

    if matches!(scenario, NativeInventoryCase::TamperedEvidence) {
        let evidence_file = helper
            .evidence_root
            .join(aros_common::mesa_genmodule_collection::MESA_GENMODULE_SELECTION_FILE);
        assert!(evidence_file.is_file());
        fs::write(&evidence_file, b"tampered selection evidence\n").unwrap();
        let result = selected.revalidate(&collected, control);
        let Err(error) = result else {
            panic!("tampered generated inventory evidence was accepted");
        };
        assert!(!error.to_string().is_empty());
    }
}

fn run_native_genmodule_include_reader_after_collection(
    collected: CollectedMesaNativeDirect,
    control: (
        &MesaPreflightRequest<'_>,
        &MesaNativeDirectOrigin<'_, '_>,
        &MesaNativeCollectionSelection<'_>,
        &CancellationToken,
    ),
    limits: RetainedChildSourceBatchLimits,
    roots: (&Path, &Path),
    inputs: &DispatchInputs,
    fixture: &NativeGenmoduleIncludeReaderFixture,
) {
    use aros_common::captured_programs::{
        CapturedProgramSet, ProgramRootSelection, ProgramSelection,
    };
    use aros_transpiler::make_shell_data::artifact_binding::{
        RecipeArtifactDirection, RecipeArtifactRequest, RecipeArtifactSlot,
    };

    let (output_root, source_root) = roots;
    let include_target = fixture.include_target.to_str().unwrap();
    let directory_target = fixture.directory_target.to_str().unwrap();
    let runtime_makefile = fixture.runtime_makefile.to_str().unwrap();
    let source_configuration = fixture.source_configuration.to_str().unwrap();
    let genmodule_program = fixture.genmodule_program.to_str().unwrap();
    let nominal_destination = fixture.nominal_destination.to_str().unwrap();
    let canonical_runtime_makefile = fixture.runtime_makefile.canonicalize().unwrap();
    assert!(fixture.genmodule_program.starts_with(output_root));
    assert!(fixture.source_configuration.starts_with(source_root));
    assert_eq!(
        inputs.host_genmodule.as_deref(),
        Some(genmodule_program),
        "the original Root query must select the literal Source GENMODULE role"
    );
    assert!(!fixture.nominal_destination.exists());
    assert!(!fixture.include_target.exists());

    // Resolve the event coordinate from the retained original owner's parsed
    // events, not from a replacement goal or an assumed event offset.
    let children = collected
        .read_children(control.0, control.1, control.2, limits, control.3)
        .expect("original child Source should remain readable before include projection");
    assert_eq!(children.len(), 1);
    let declared = children
        .into_iter()
        .next()
        .unwrap()
        .trace_declared_rules()
        .expect("original child declaration trace should retain its include event");
    assert_eq!(
        declared
            .coordinates()
            .goals()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![GOAL]
    );
    let include_event = declared
        .file()
        .events
        .iter()
        .position(|event| {
            event.active
                && matches!(
                    &event.kind,
                    MakeFileEventKind::Include {
                        requested,
                        present: false,
                        resolved_identity: None,
                    } if requested == include_target
                )
        })
        .expect("original child has the selected missing include event");
    drop(declared);

    let program_directory = fixture
        .genmodule_program
        .parent()
        .expect("captured GENMODULE role has a selected parent directory");
    let programs = CapturedProgramSet::capture(
        &[ProgramRootSelection {
            root: program_directory.to_owned(),
            content_sha256: aros_common::measure_tree_content_cas(program_directory)
                .expect("original GENMODULE output directory should be capturable")
                .payload_digest_excluding(None),
        }],
        &[ProgramSelection {
            role: "source-genmodule".into(),
            root_index: 0,
            invocation_path: fixture.genmodule_program.clone(),
            executable_sha256: sha256_bytes(&fs::read(&fixture.genmodule_program).unwrap()),
        }],
    )
    .expect("original GENMODULE role should bind to its captured output tree");
    let configuration_bytes = fs::read(&fixture.source_configuration).unwrap();
    let configuration_request = RecipeArtifactRequest {
        recipe: 0,
        command: 0,
        slot: RecipeArtifactSlot::Word {
            index: 2,
            range: 0..source_configuration.len(),
        },
        direction: RecipeArtifactDirection::Input,
    };

    let (_collected, bound) = collected
        .with_original_include_reader(control, (0, include_event), limits, |included, reader| {
            if included
                .coordinates()
                .goals()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                != vec![GOAL]
            {
                return Err("original child goals changed during include projection".into());
            }
            if !matches!(
                &included.include_event().kind,
                MakeFileEventKind::Include {
                    requested,
                    present: false,
                    resolved_identity: None,
                } if requested == include_target
            ) {
                return Err("include projection selected a different original event".into());
            }

            let runtime_event_is_original_output =
                included.declared().file().events.iter().any(|event| {
                    event.active
                        && matches!(
                            &event.kind,
                            MakeFileEventKind::Include {
                                requested,
                                present: true,
                                resolved_identity: Some(identity),
                            } if requested == runtime_makefile
                                && Path::new(identity) == canonical_runtime_makefile.as_path()
                        )
                });
            if !runtime_event_is_original_output {
                return Err(
                    "runtime Make configuration was not admitted from original outputs".into(),
                );
            }
            let runtime_bytes = reader
                .read_bytes(runtime_makefile)?
                .ok_or("original runtime Make configuration bytes are absent")?
                .bytes;
            if runtime_bytes != fixture.runtime_makefile_bytes {
                return Err("runtime Make configuration bytes changed".into());
            }

            let visits = included.traversal().visits();
            let find_visit = |target: &str| {
                visits
                    .iter()
                    .position(|visit| visit.target() == target)
                    .ok_or_else(|| format!("original include traversal lacks {target}"))
            };
            let target_visit = find_visit(include_target)?;
            let program_visit = find_visit(genmodule_program)?;
            let configuration_visit = find_visit(source_configuration)?;
            let directory_visit = find_visit(directory_target)?;
            let target = visits[target_visit]
                .instance()
                .ok_or("original include target declaration is absent")?;
            if target
                .prerequisites
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                != [genmodule_program, source_configuration]
                || target
                    .order_only
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    != [directory_target]
                || visits[target_visit].parent().is_some()
                || visits[program_visit].parent() != Some(target_visit)
                || visits[configuration_visit].parent() != Some(target_visit)
                || visits[directory_visit].parent() != Some(target_visit)
            {
                return Err("original include producer edges changed".into());
            }
            let directory = visits[directory_visit]
                .instance()
                .ok_or("original output-directory declaration is absent")?;
            if !directory.prerequisites.is_empty() || !directory.order_only.is_empty() {
                return Err("original output-directory edges changed".into());
            }

            let recipe_targets = included
                .recipes()
                .iter()
                .map(|recipe| visits[recipe.visit()].target().to_owned())
                .collect::<Vec<_>>();
            let mut sorted_recipe_targets = recipe_targets;
            sorted_recipe_targets.sort();
            let mut expected_recipe_targets =
                vec![include_target.to_owned(), directory_target.to_owned()];
            expected_recipe_targets.sort();
            if sorted_recipe_targets != expected_recipe_targets {
                return Err("original include and directory recipes changed".into());
            }
            let directory_recipe = included
                .recipes()
                .iter()
                .find(|recipe| visits[recipe.visit()].target() == directory_target)
                .ok_or("original directory recipe is absent")?;
            let directory_shell = directory_recipe.expression().parse_literal_shell()?;
            let [aros_transpiler::make_shell_data::LiteralRecipeStatement::Command(
                directory_command,
            )] = directory_shell.statements.as_slice()
            else {
                return Err("original directory recipe is not one literal command".into());
            };
            if directory_shell.prefixes.ignore_errors
                || directory_shell.prefixes.explicit_recursive
                || !directory_command.redirections().is_empty()
                || directory_command
                    .words()
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    != ["mkdir", "-p", directory_target]
            {
                return Err("original directory recipe argv or Make controls changed".into());
            }
            let recipe_index = included
                .recipes()
                .iter()
                .position(|recipe| visits[recipe.visit()].target() == include_target)
                .ok_or("original GENMODULE writemakefile recipe is absent")?;
            let bound = included.with_recipe_input_bindings_in_reader(
                recipe_index,
                &programs,
                std::slice::from_ref(&configuration_request),
                reader,
                |bindings, artifacts| {
                    if bindings.commands().len() != 1 || artifacts.len() != 1 {
                        return Err("original GENMODULE input binding is incomplete".into());
                    }
                    let selected = bindings.unique_role("source-genmodule")?;
                    let program = selected
                        .program()
                        .ok_or("original GENMODULE command role is unresolved")?;
                    let artifact = &artifacts[0];
                    Ok((
                        program.invocation_path().to_owned(),
                        selected.command().words().to_vec(),
                        artifact.path().to_owned(),
                        artifact.input_sha256().cloned(),
                    ))
                },
            )?;
            Ok((runtime_bytes, bound))
        })
        .expect("original owner include reader should project its exact GENMODULE rule");

    assert_eq!(bound.0, fixture.runtime_makefile_bytes);
    assert_eq!(bound.1 .0, fixture.genmodule_program);
    assert_eq!(
        bound.1 .1,
        [
            genmodule_program.to_owned(),
            "-c".into(),
            source_configuration.to_owned(),
            "-d".into(),
            nominal_destination.to_owned(),
            "writemakefile".into(),
            "mesa3dgl26-0".into(),
            "library".into(),
        ]
    );
    assert_eq!(bound.1 .2, fixture.source_configuration);
    assert_eq!(
        bound.1 .3,
        Some(sha256_bytes(&configuration_bytes)),
        "Source configuration must be read through the original include reader"
    );
    assert!(!fixture.nominal_destination.exists());
    assert!(!fixture.include_target.exists());
    assert!(!fixture.directory_target.exists());
    assert!(
        !fixture.genmodule_launched.exists(),
        "projecting original include data must not launch the selected producer"
    );
}

fn run_native_genmodule_make_after_collection(
    collected: CollectedMesaNativeDirect,
    control: (
        &MesaPreflightRequest<'_>,
        &MesaNativeDirectOrigin<'_, '_>,
        &MesaNativeCollectionSelection<'_>,
        &CancellationToken,
    ),
    limits: RetainedChildSourceBatchLimits,
    fixture: NativeGenmoduleMakeFixture<'_>,
) {
    use aros_common::{
        mesa_configuration_input::MESA_GENMODULE_BASE_CONFIG,
        mesa_genmodule_make::MesaGenmoduleMakefile,
    };
    let NativeGenmoduleMakeFixture {
        output_root,
        nominal_destination,
        inputs,
        helper,
        scenario,
    } = fixture;

    let (collected, inventory) = collected
        .collect_genmodule_inventory(control, &helper.selection())
        .expect("synthetic Source inventory should bind the original owner call");
    assert_eq!(inventory.inventory().inventory().records.len(), 1);
    assert_eq!(inventory.owners().len(), 1);
    assert_eq!(inventory.owners()[0].owner, GOAL);
    assert_eq!(inventory.owners()[0].call_index, 0);
    inventory
        .revalidate(&collected, control)
        .expect("selected inventory evidence should revalidate before data generation");
    if matches!(
        scenario,
        NativeGenmoduleMakeCase::ConfiguredOwnerPath
            | NativeGenmoduleMakeCase::SourcePathAssignment
            | NativeGenmoduleMakeCase::SourcePathUnexport
    ) {
        let configured = output_root.join("configure/configured-path");
        let selected_host_path = output_root.parent().unwrap().join("host-tools");
        let expected_owner_path =
            format!("{}:{}", configured.display(), selected_host_path.display());
        let observed_owner_path = collected.calls()[0]
            .invocation()
            .environment()
            .get("PATH")
            .expect("captured owner invocation should retain PATH");
        assert_eq!(observed_owner_path, &expected_owner_path);
        assert_ne!(observed_owner_path, selected_host_path.to_str().unwrap());
    }
    let selection_identity = collected.preparation().selection_sha256().to_string();

    let expected_program = output_root
        .join("configure/tools/MetaMake/generated/genmodule")
        .to_str()
        .unwrap()
        .to_owned();
    if matches!(scenario, NativeGenmoduleMakeCase::MissingRoleQuery) {
        assert_eq!(inputs.host_genmodule, None);
    } else {
        assert_eq!(
            inputs.host_genmodule.as_deref(),
            Some(expected_program.as_str())
        );
    }

    let evidence_root = output_root.with_file_name("native-genmodule-data-evidence");
    let recipe_index = usize::from(matches!(
        scenario,
        NativeGenmoduleMakeCase::WrongRecipeIndex | NativeGenmoduleMakeCase::ForeignSiblingRecipe
    ));
    let selection = NativeGenmoduleMakeSelection {
        owner: GOAL,
        recipe_index,
        nominal_destination,
        evidence_root: &evidence_root,
    };
    match scenario {
        NativeGenmoduleMakeCase::TamperedSourceBeforeData => {
            let source_owner = control.0.source_root.join(OWNER);
            let mut source_bytes = fs::read(&source_owner).unwrap();
            source_bytes.extend_from_slice(b"\n# deliberate prelaunch source drift\n");
            fs::write(source_owner, source_bytes).unwrap();
        }
        NativeGenmoduleMakeCase::TamperedInventoryEvidenceBeforeData => {
            let selection_file = helper
                .evidence_root
                .join(aros_common::mesa_genmodule_collection::MESA_GENMODULE_SELECTION_FILE);
            assert!(selection_file.is_file());
            fs::write(selection_file, b"tampered inventory selection evidence\n").unwrap();
        }
        _ => {}
    }
    let result = collected.produce_genmodule_make_data(control, &inventory, limits, &selection);
    assert!(
        !nominal_destination.exists(),
        "the original Source writefiles recipe must remain unexecuted"
    );

    let (expected_public_refusal, expected_private_reason) = match scenario {
        NativeGenmoduleMakeCase::WrongTuple => (
            Some(
                "cannot interpret original direct Source child batch; private diagnostic retained",
            ),
            Some("native GENMODULE module type does not match the producer"),
        ),
        NativeGenmoduleMakeCase::WrongRecipeIndex => (
            Some(
                "cannot interpret original direct Source child batch; private diagnostic retained",
            ),
            Some("declared Source recipe is absent"),
        ),
        NativeGenmoduleMakeCase::WrongRole => (
            Some(
                "cannot interpret original direct Source child batch; private diagnostic retained",
            ),
            Some("selected recipe program role is absent"),
        ),
        NativeGenmoduleMakeCase::ForeignSiblingRecipe => (
            Some(
                "cannot interpret original direct Source child batch; private diagnostic retained",
            ),
            Some("native genmodule recipe is outside the selected owner dependency path"),
        ),
        NativeGenmoduleMakeCase::MissingRoleQuery => (
            Some("native genmodule host role was not independently queried"),
            None,
        ),
        NativeGenmoduleMakeCase::TamperedSourceBeforeData => {
            (Some("direct native Source policy changed"), None)
        }
        NativeGenmoduleMakeCase::TamperedInventoryEvidenceBeforeData => {
            (Some("native genmodule inventory evidence changed"), None)
        }
        NativeGenmoduleMakeCase::UndeclaredExtraOutput => {
            (Some("native genmodule data has unexpected outputs"), None)
        }
        NativeGenmoduleMakeCase::SourcePathAssignment
        | NativeGenmoduleMakeCase::SourcePathUnexport => (
            Some("original recipe exports a path outside selected host tools"),
            None,
        ),
        NativeGenmoduleMakeCase::Success
        | NativeGenmoduleMakeCase::ConfiguredOwnerPath
        | NativeGenmoduleMakeCase::TamperedDataEvidence => (None, None),
    };
    if let Some(expected_public_refusal) = expected_public_refusal {
        let Err(error) = result else {
            panic!("a refused native Genmodule DATA case was accepted: {scenario:?}");
        };
        assert!(
            error.to_string().contains(expected_public_refusal),
            "expected `{expected_public_refusal}`, received `{error}`"
        );
        let private_diagnostic = output_root
            .parent()
            .expect("temporary output root has a private parent")
            .join(format!(
                "direct-source-child-refusal-{selection_identity}.json"
            ));
        if let Some(expected_private_reason) = expected_private_reason {
            // The composed child callback now retains the same reason with
            // its exact shared-batch coordinate; the public error stays redacted.
            let expected_private_reason =
                format!("retained child source batch at index 0: {expected_private_reason}");
            assert!(
                !error.to_string().contains(&expected_private_reason),
                "callback detail must remain redacted from the public refusal"
            );
            let bytes = fs::read(&private_diagnostic)
                .expect("callback refusal should retain private reason identity");
            let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(document["schema"], "aros-native-source-child-refusal-v1");
            assert_eq!(
                document["qualification"],
                "failed-original-owner-diagnostic-not-execution-or-sdk"
            );
            assert_eq!(document["selection"], selection_identity);
            assert_eq!(document["calls"], 1);
            assert_eq!(
                document["reason_sha256"],
                aros_common::sha256_bytes(expected_private_reason.as_bytes()).to_string()
            );
            assert_eq!(document["reason_bytes"], expected_private_reason.len());
            assert!(
                !String::from_utf8_lossy(&bytes).contains(&expected_private_reason),
                "private refusal records only the detail digest, not its text"
            );
        } else {
            assert!(
                !private_diagnostic.exists(),
                "direct prelaunch/process refusals should not emit callback diagnostics"
            );
        }
        if !matches!(scenario, NativeGenmoduleMakeCase::UndeclaredExtraOutput) {
            assert!(
                !evidence_root.exists(),
                "prelaunch refusal must not create native Genmodule DATA evidence"
            );
        }
        return;
    }

    let (collected, mut data) =
        result.expect("selected original-owner Genmodule DATA should succeed");
    let raw_makefile = data.raw_makefile_path();
    assert!(raw_makefile.is_file());
    let text = fs::read_to_string(&raw_makefile).unwrap();
    let parsed = MesaGenmoduleMakefile::parse("mesa3dgl26-0", &text)
        .expect("the synthetic original host tool emitted all bounded assignments");
    assert_eq!(data.parsed_makefile(), &parsed);
    assert_eq!(data.source_input(), Path::new(OWNER));
    assert_eq!(data.output_binding().producer.module, "mesa3dgl26-0");
    assert_eq!(data.output_binding().producer.module_type, "library");
    assert_eq!(
        data.output_binding().base_configuration_sha256,
        sha256_bytes(&fs::read(control.0.source_root.join(MESA_GENMODULE_BASE_CONFIG)).unwrap())
    );
    assert!(data
        .output_binding()
        .override_configuration_sha256
        .is_none());
    assert_eq!(
        fs::read_to_string(evidence_root.join("stdout")).unwrap(),
        "dispatch=held source-unexport=unset\n"
    );
    data.revalidate(&collected, control, &inventory)
        .expect("original owner, inventory, process and Make data should revalidate");

    if matches!(scenario, NativeGenmoduleMakeCase::Success) {
        let original = std::mem::replace(
            &mut data.source_input,
            PathBuf::from("foreign/declaration/source.mk"),
        );
        let error = data
            .revalidate(&collected, control, &inventory)
            .expect_err("retained data must not adopt another Source coordinate");
        assert!(error.to_string().contains(
            "native genmodule data differs from its original Source coordinate or tuple"
        ));
        data.source_input = original;
    }

    if matches!(scenario, NativeGenmoduleMakeCase::TamperedDataEvidence) {
        fs::write(raw_makefile, b"tampered\n").unwrap();
        let error = data
            .revalidate(&collected, control, &inventory)
            .expect_err("tampered Genmodule data must not revalidate");
        assert!(error
            .to_string()
            .contains("native genmodule data evidence changed"));
    }
}

fn run_intrinsic_producer_case(scenario: IntrinsicProducerCase) {
    run_case_with_mode("", RunCaseMode::IntrinsicProducer(scenario));
}

fn run_dependent_case(scenario: DependentCase) {
    run_case_with_mode("", RunCaseMode::Dependent(scenario));
}

fn run_generated_path_case(scenario: GeneratedPathCase) {
    run_case_with_mode("", RunCaseMode::GeneratedPaths(scenario));
}

fn run_native_inventory_case(scenario: NativeInventoryCase) {
    run_case_with_mode("", RunCaseMode::NativeInventory(scenario));
}

fn run_native_genmodule_make_case(scenario: NativeGenmoduleMakeCase) {
    run_case_with_mode("", RunCaseMode::NativeGenmoduleMake(scenario));
}

fn run_case_with_mode(body: &str, mode: RunCaseMode) {
    let observer = PathBuf::from(
        std::env::var_os("AROS_TEST_MESA_NATIVE_OBSERVER").expect("select observer explicitly"),
    );
    assert!(observer.is_absolute());
    let observer = observer.canonicalize().unwrap();
    let expected = std::env::var("AROS_TEST_MESA_NATIVE_OBSERVER_SHA256")
        .expect("select observer digest explicitly");
    let observer_sha = sha256_bytes(&fs::read(&observer).unwrap());
    assert_eq!(observer_sha.to_string(), expected);
    // Native MetaMake's 255-byte physical-line bound also applies to its
    // instrumented command. Keep this fixture short, not the production guard loose.
    let mut fixture = Fixture::configured_directory(
        PROFILE_TRIPLE,
        "arm",
        "raspi",
        ConfigureMode::Normal,
        FeatureMode::Complete,
        tempfile::Builder::new()
            .prefix("me")
            .rand_bytes(4)
            .tempdir_in("/tmp")
            .unwrap(),
    );
    if matches!(mode, RunCaseMode::NativeGenmoduleMake(_)) {
        let private_fixture_root = fixture
            .output_root
            .parent()
            .expect("synthetic output root has its owned temporary parent");
        fs::set_permissions(private_fixture_root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    if matches!(
        mode,
        RunCaseMode::NativeGenmoduleIncludeReader | RunCaseMode::NativeGenmoduleIncludeExecution(_)
    ) {
        let private_fixture_root = fixture
            .output_root
            .parent()
            .expect("synthetic output root has its owned temporary parent");
        fs::set_permissions(private_fixture_root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    if matches!(mode, RunCaseMode::Dependent(_)) {
        // Select recipe utilities before original capture; never fall back to
        // the operator's PATH at runtime.
        let utility = "cat";
        let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = &fixture.environment
        else {
            panic!("synthetic recipe needs sealed host tools");
        };
        let mut tools = host_tools
            .tools
            .iter()
            .map(|(name, identity)| CompatibilityHostTool {
                name: name.clone(),
                program: identity.program.clone(),
            })
            .collect::<Vec<_>>();
        tools.push(CompatibilityHostTool {
            name: utility.into(),
            program: which(utility),
        });
        let selected = prepare_host_tool_closure(&HostToolClosureRequest {
            output_root: fixture.output_root.with_file_name("dependent-host-tools"),
            tools,
        })
        .unwrap();
        let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = &mut fixture.environment
        else {
            unreachable!();
        };
        *host_tools = selected;
    }
    let mut native_inventory_helper = None;
    let mut native_genmodule_make_destination = None;
    let mut native_genmodule_script = None;
    let mut native_genmodule_include_reader_fixture = None;
    let (profile, mmake) = match mode {
        RunCaseMode::Dependent(scenario) => {
            let rules = dependent_rule_program(scenario);
            source_fixture_with_rules(&mut fixture, &rules)
        }
        RunCaseMode::GeneratedPaths(scenario) => {
            let rules = generated_path_rule_program(scenario);
            source_fixture_with_rules(&mut fixture, &rules)
        }
        RunCaseMode::IntrinsicProducer(scenario) => {
            let rules = intrinsic_producer_rule_program(scenario);
            let producer_policy = intrinsic_producer_policy(scenario);
            source_fixture_with_rules_and_producer_policy(
                &mut fixture,
                &rules,
                &producer_policy,
                &[],
                "",
            )
        }
        RunCaseMode::NativeInventory(scenario) => {
            let (profile, mmake) = source_fixture(&mut fixture, ":");
            native_inventory_helper = Some(synthetic_native_inventory_helper(
                &fixture, &profile, scenario, true,
            ));
            (profile, mmake)
        }
        RunCaseMode::NativeGenmoduleMake(scenario) => {
            let (profile, mmake, destination) =
                native_genmodule_make_source_fixture(&mut fixture, scenario);
            native_inventory_helper = Some(synthetic_native_inventory_helper(
                &fixture,
                &profile,
                NativeInventoryCase::Success,
                false,
            ));
            native_genmodule_make_destination = Some(destination);
            native_genmodule_script = Some(native_genmodule_make_script(&fixture, scenario));
            (profile, mmake)
        }
        RunCaseMode::NativeGenmoduleIncludeReader
        | RunCaseMode::NativeGenmoduleIncludeExecution(_) => {
            let scenario = if let RunCaseMode::NativeGenmoduleIncludeExecution(scenario) = mode {
                Some(scenario)
            } else {
                None
            };
            let (profile, mmake, include_fixture, script) =
                native_genmodule_include_reader_source_fixture(
                    &mut fixture,
                    matches!(
                        scenario,
                        Some(IncludeExecutionCase::UnfinishedConfiguration)
                    ),
                );
            let script = match scenario {
                Some(scenario) if !matches!(scenario, IncludeExecutionCase::UnboundInterpreter) => {
                    original_include_generator_source(&include_fixture, scenario)
                }
                _ => script,
            };
            native_genmodule_include_reader_fixture = Some(include_fixture);
            native_genmodule_script = Some(script);
            (profile, mmake)
        }
        _ => source_fixture(&mut fixture, body),
    };
    let (launch, mut inputs) = match mode {
        RunCaseMode::NativeGenmoduleMake(NativeGenmoduleMakeCase::MissingRoleQuery) => {
            observed_launch_with_unqueried_genmodule(
                &mut fixture,
                "build-quick",
                &mmake,
                native_genmodule_script
                    .as_deref()
                    .expect("Genmodule host script is prepared before the original query"),
            )
        }
        RunCaseMode::NativeGenmoduleMake(_)
        | RunCaseMode::NativeGenmoduleIncludeReader
        | RunCaseMode::NativeGenmoduleIncludeExecution(IncludeExecutionCase::UnboundInterpreter) => {
            observed_launch_with_genmodule(
                &mut fixture,
                "build-quick",
                &mmake,
                native_genmodule_script
                    .as_deref()
                    .expect("Genmodule host script is prepared before the original query"),
            )
        }
        RunCaseMode::NativeGenmoduleIncludeExecution(_) => observed_launch_with_native_genmodule(
            &mut fixture,
            "build-quick",
            &mmake,
            native_genmodule_script
                .as_deref()
                .expect("native GENMODULE Source is selected before preparation"),
        ),
        _ => observed_launch_with_program(&mut fixture, "build-quick", None, Some(&mmake)),
    };
    inputs.observer = observer;
    inputs.observer_sha256 = observer_sha;
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let host_query = inputs.host_query();
    let host_build = inputs.host_build();
    let launch_query = inputs.launch_query();
    let dispatch = inputs.dispatch_selection();
    let candidate = observe_mesa_native_dispatch(
        launch,
        &request,
        &host_query,
        &host_build,
        &launch_query,
        &dispatch,
        &cancellation,
    )
    .unwrap();
    let fenced = candidate
        .observe_fenced(
            &request,
            &host_query,
            &host_build,
            &launch_query,
            &dispatch,
            &cancellation,
        )
        .unwrap();
    let recorded = fenced
        .capture_recorder(
            &request,
            &host_query,
            &host_build,
            &launch_query,
            &dispatch,
            &cancellation,
        )
        .unwrap();
    let collection_root = fixture.output_root.with_file_name("direct-collection");
    let selected = MesaNativeCollectionSelection {
        output_root: &collection_root,
        observer: &inputs.observer,
        observer_sha256: &inputs.observer_sha256,
        project: "Fixture",
        source_command_word: "make",
        maximum_calls: if matches!(
            mode,
            RunCaseMode::NativeGenmoduleMake(NativeGenmoduleMakeCase::ForeignSiblingRecipe)
        ) {
            2
        } else {
            1
        },
    };
    let source = MesaNativeSourceSelection {
        collection: &selected,
        source_profile: &profile,
        host: "fixture-host",
    };
    let origin = MesaNativeDirectOrigin {
        host_query: &host_query,
        host_build: &host_build,
        launch_query: &launch_query,
        dispatch: &dispatch,
    };
    let selected_source_goals = if matches!(
        mode,
        RunCaseMode::NativeGenmoduleMake(NativeGenmoduleMakeCase::ForeignSiblingRecipe)
    ) {
        vec![GOAL.to_owned(), "FOREIGN_GOAL".to_owned()]
    } else {
        vec![GOAL.to_owned()]
    };
    let prepared = prepare_mesa_native_direct_collection(
        recorded,
        &request,
        &origin,
        &source,
        &selected_source_goals,
        &cancellation,
    )
    .unwrap();
    let collected = prepared
        .collect(&request, &origin, &selected, &cancellation)
        .unwrap();
    assert_eq!(collected.calls().len(), 1);
    let control = (&request, &origin, &selected, &cancellation);
    let limits = retained_child_limits();
    match mode {
        RunCaseMode::Single {
            expect_success,
            cancellation_before_launch,
        } => {
            let evidence = fixture.output_root.with_file_name("recipe-execution");
            if cancellation_before_launch {
                cancellation.cancel();
            }
            let result = collected.execute_mesa26_original_recipe(
                control,
                limits,
                &Mesa26OriginalRecipeExecution {
                    child_slot: 0,
                    recipe_index: 0,
                    selected_shell: Path::new("/bin/sh"),
                    evidence_root: &evidence,
                    mutable_files: &[],
                    replaceable_files: &[],
                    fresh_files: &["recipe-result"],
                    fresh_directories: &[],
                },
            );
            let target = fixture.output_root.join("configure/recipe-result");
            if expect_success {
                let result = result.unwrap();
                assert_eq!(fs::read(&target).unwrap(), b"original-recipe\n");
                assert_eq!(result.evidence_snapshot().entry_count(), 2);
                assert!(!fixture.output_root.join("receipt.json").exists());
            } else {
                let Err(error) = result else {
                    panic!("invalid original recipe execution was accepted");
                };
                let message = error.to_string();
                if !cancellation_before_launch {
                    assert!(evidence.is_dir(), "counter must reach the supervised child");
                    let expected = if body.contains("unselected") {
                        "unselected outputs"
                    } else {
                        "child failed"
                    };
                    assert!(
                        message.contains(expected),
                        "unexpected execution refusal: {message}"
                    );
                    if body.contains("unselected") {
                        assert_eq!(
                            fs::read(fixture.output_root.join("configure/unselected")).unwrap(),
                            b"changed"
                        );
                        assert_eq!(fs::read(&target).unwrap(), b"original-recipe\n");
                    } else {
                        assert_eq!(
                            fs::read(evidence.join("stderr")).unwrap(),
                            b"child failed\n"
                        );
                        assert!(!target.exists());
                    }
                }
                if cancellation_before_launch {
                    assert!(
                        message.contains("cancel"),
                        "unexpected cancellation refusal: {message}"
                    );
                    assert!(!evidence.exists());
                    assert!(!target.exists());
                }
            }
        }
        RunCaseMode::Sequence(scenario) => {
            run_sequence_after_collection(
                collected,
                control,
                limits,
                &fixture.output_root,
                scenario,
            );
        }
        RunCaseMode::Dependent(scenario) => {
            run_dependent_after_collection(
                collected,
                control,
                limits,
                &fixture.output_root,
                scenario,
            );
        }
        RunCaseMode::GeneratedPaths(scenario) => {
            run_generated_path_list_after_collection(
                collected,
                control,
                limits,
                &fixture.output_root,
                scenario,
            );
        }
        RunCaseMode::IntrinsicProducer(scenario) => {
            run_intrinsic_producer_after_collection(
                collected,
                control,
                limits,
                &fixture.output_root,
                scenario,
            );
        }
        RunCaseMode::NativeInventory(scenario) => {
            run_native_genmodule_inventory_after_collection(
                collected,
                control,
                native_inventory_helper
                    .as_ref()
                    .expect("inventory case selected its helper before source capture"),
                scenario,
            );
        }
        RunCaseMode::NativeGenmoduleMake(scenario) => {
            run_native_genmodule_make_after_collection(
                collected,
                control,
                limits,
                NativeGenmoduleMakeFixture {
                    output_root: &fixture.output_root,
                    nominal_destination: native_genmodule_make_destination
                        .as_deref()
                        .expect("DATA case selected its nominal destination before Source capture"),
                    inputs: &inputs,
                    helper: native_inventory_helper
                        .as_ref()
                        .expect("DATA case selected its inventory helper before Source capture"),
                    scenario,
                },
            );
        }
        RunCaseMode::NativeGenmoduleIncludeReader => {
            run_native_genmodule_include_reader_after_collection(
                collected,
                control,
                limits,
                (&fixture.output_root, &fixture.source_root),
                &inputs,
                native_genmodule_include_reader_fixture
                    .as_ref()
                    .expect("include reader case selected its fixture before Source capture"),
            );
        }
        RunCaseMode::NativeGenmoduleIncludeExecution(scenario) => {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_original_include_execution(
                    collected,
                    control,
                    limits,
                    native_genmodule_include_reader_fixture.as_ref().unwrap(),
                    scenario,
                );
            }));
            if let Err(payload) = outcome {
                eprintln!(
                    "retained failed original include fixture: {}",
                    fixture.temporary.keep().display()
                );
                std::panic::resume_unwind(payload);
            }
        }
    }
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn original_missing_include_executes_directory_and_genmodule_then_reimports() {
    run_case_with_mode(
        "",
        RunCaseMode::NativeGenmoduleIncludeExecution(IncludeExecutionCase::Success),
    );
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn original_missing_include_refuses_extra_output_failed_process_and_later_drift() {
    for scenario in [
        IncludeExecutionCase::UnboundInterpreter,
        IncludeExecutionCase::UnfinishedConfiguration,
        IncludeExecutionCase::ExtraOutput,
        IncludeExecutionCase::FailedGenerator,
        IncludeExecutionCase::ChangedOutput,
    ] {
        run_case_with_mode("", RunCaseMode::NativeGenmoduleIncludeExecution(scenario));
    }
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn original_recipe_executes_under_the_collected_owner() {
    run_case(
        "test \"$$LC_ALL\" = C && printf 'original-recipe\\n' > $(TOP)/recipe-result",
        true,
        false,
    );
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn original_recipe_failure_and_unselected_effect_are_rejected() {
    run_case("printf 'child failed\\n' >&2; exit 7", false, false);
    run_case(
        "printf changed > $(TOP)/unselected; printf 'original-recipe\\n' > $(TOP)/recipe-result",
        false,
        false,
    );
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn original_recipe_cancellation_prevents_launch() {
    run_case(
        "printf 'original-recipe\\n' > $(TOP)/recipe-result",
        false,
        true,
    );
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn original_recipe_sequence_executes_three_ordered_lines() {
    run_sequence_case(SequenceCase::Success);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn original_recipe_sequence_cannot_start_at_a_later_line() {
    run_sequence_case(SequenceCase::StartLaterLine);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn original_recipe_sequence_rejects_skip_and_replay() {
    run_sequence_case(SequenceCase::SkipNextLine);
    run_sequence_case(SequenceCase::ReplayPreviousLine);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn original_recipe_sequence_rejects_tampered_prior_evidence_or_output() {
    run_sequence_case(SequenceCase::TamperPreviousEvidence);
    run_sequence_case(SequenceCase::TamperPreviousOutput);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn original_recipe_sequence_stops_after_late_line_failure() {
    run_sequence_case(SequenceCase::LateFailure);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn original_recipe_sequence_rejects_rule_plan_drift() {
    run_sequence_case(SequenceCase::PlanDrift);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn dependent_recipe_consumes_a_completed_producer_output() {
    run_dependent_case(DependentCase::Success);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn dependent_recipe_refuses_an_incomplete_producer_rule() {
    run_dependent_case(DependentCase::IncompleteProducer);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn dependent_recipe_refuses_a_forged_side_file_as_producer_lineage() {
    run_dependent_case(DependentCase::ForgedSideFile);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn dependent_recipe_refuses_replaying_a_completed_rule() {
    run_dependent_case(DependentCase::ReplayCompletedRule);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn dependent_recipe_accepts_a_source_declared_co_output_alias() {
    run_dependent_case(DependentCase::DeclaredCoOutputAlias);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn generated_path_list_accepts_a_completed_source_declared_alias() {
    run_generated_path_case(GeneratedPathCase::Success);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn generated_path_list_rejects_an_undeclared_list_file() {
    run_generated_path_case(GeneratedPathCase::UndeclaredList);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn generated_path_list_rejects_a_preexisting_unowned_member() {
    run_generated_path_case(GeneratedPathCase::PreexistingMember);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn generated_path_list_rejects_a_member_outside_its_root() {
    run_generated_path_case(GeneratedPathCase::OutsideEntriesRoot);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn generated_path_list_rejects_shell_syntax_in_entries() {
    run_generated_path_case(GeneratedPathCase::ShellSyntax);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn generated_path_list_rejects_later_foreign_noop_effects() {
    run_generated_path_case(GeneratedPathCase::ForeignNoopEffect);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn generated_path_list_rejects_post_completion_member_tampering() {
    run_generated_path_case(GeneratedPathCase::TamperedMember);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn intrinsic_source_selected_producer_emits_a_validated_generated_path_list() {
    run_intrinsic_producer_case(IntrinsicProducerCase::Success);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn intrinsic_deep_producer_without_source_policy_refuses_before_launch() {
    run_intrinsic_producer_case(IntrinsicProducerCase::NoPolicy);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn intrinsic_producer_rejects_source_selected_cooutput_drift_between_lines() {
    run_intrinsic_producer_case(IntrinsicProducerCase::SelectionDrift);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn native_genmodule_inventory_binds_the_observed_source_owner_call() {
    run_native_inventory_case(NativeInventoryCase::Success);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn native_genmodule_inventory_rejects_wrong_owner_binding_and_helper_digest() {
    run_native_inventory_case(NativeInventoryCase::WrongObservedOwner);
    run_native_inventory_case(NativeInventoryCase::WrongBinding);
    run_native_inventory_case(NativeInventoryCase::WrongHelperDigest);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn native_genmodule_inventory_revalidates_its_original_evidence() {
    run_native_inventory_case(NativeInventoryCase::TamperedEvidence);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn native_genmodule_make_data_runs_fixed_writemakefile_under_original_owner() {
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::Success);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn native_genmodule_make_data_uses_captured_owner_path_from_configured_globals() {
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::ConfiguredOwnerPath);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn native_genmodule_make_data_rejects_recipe_path_assignment_and_unexport() {
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::SourcePathAssignment);
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::SourcePathUnexport);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn native_genmodule_make_data_rejects_wrong_tuple_recipe_and_program_role() {
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::WrongTuple);
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::WrongRecipeIndex);
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::WrongRole);
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::MissingRoleQuery);
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::ForeignSiblingRecipe);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn native_genmodule_make_data_rejects_extra_outputs_and_revalidates_tampering() {
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::UndeclaredExtraOutput);
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::TamperedDataEvidence);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn native_genmodule_make_data_rejects_prelaunch_source_and_inventory_tampering() {
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::TamperedSourceBeforeData);
    run_native_genmodule_make_case(NativeGenmoduleMakeCase::TamperedInventoryEvidenceBeforeData);
}

#[test]
#[ignore = "requires explicit observer binary and SHA256; synthetic full-owner process proof"]
fn original_missing_include_reader_projects_genmodule_and_order_only_directory() {
    run_case_with_mode("", RunCaseMode::NativeGenmoduleIncludeReader);
}
