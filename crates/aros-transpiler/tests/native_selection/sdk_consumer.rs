//! Actual CLI graph/binding probes, not native compiler or SDK qualification.
use super::*;

fn fixture(cpu: &str) -> Fixture {
    let mut fixture = native_consumer::consumer_fixture(false);
    let profile_path = fixture.root.path().join("aros-targets.toml");
    let profile = fs::read_to_string(&profile_path)
        .unwrap()
        .replace("native_consumer_contract", "sdk_consumer_contract")
        .replace("arch = \"riscv32\"", &format!("arch = {cpu:?}"))
        .replace("toolchain = \"gnu\"", "toolchain = \"llvm\"")
        .replace("use_mmu = false", "use_mmu = true")
        .replace(
            "float_abi = \"ilp32f\"\n",
            if cpu == "arm" {
                "float_abi = \"hard\"\n"
            } else {
                ""
            },
        );
    fs::write(&profile_path, profile).unwrap();
    fs::write(
        fixture.root.path().join("probe.cpp"),
        b"int main() { return 0; }\n",
    )
    .unwrap();
    bind_fixture_input(&mut fixture, "probe.c");
    bind_fixture_input(&mut fixture, "probe.cpp");
    fixture.contract.as_object_mut().unwrap().remove("abi");
    fixture.contract["schema"] = json!("aros-sdk-consumer-contract-v1");
    let mut abi = json!({
        "source_cpu": cpu,
        "pointer_width": if cpu == "arm" { 32 } else { 64 },
        "use_mmu": true,
        "flavour": "standalone",
        "platform_smp": false
    });
    if cpu == "arm" {
        abi["float_abi"] = json!("hard");
    }
    fixture.contract["target"] =
        json!({ "target_triple": format!("{cpu}-unknown-aros"), "abi": abi });
    fixture.contract["native_sdk_link_probes"] = json!({
        "c": { "source": "probe.c", "libraries": [] },
        "cxx": { "source": "probe.cpp", "libraries": [] }
    });
    let mut context = fixture_native_selector_context();
    context.cpu = Some(cpu.to_owned());
    context.toolchain = Some("llvm".into());
    context.use_mmu = Some("1".into());
    context.float_abi = (cpu == "arm").then(|| "hard".to_owned());
    let globals = fixture_native_selector_globals(&context);
    fs::write(fixture.root.path().join("meta/globals.snapshot"), &globals).unwrap();
    let policy_path = fixture.root.path().join("meta/policy.json");
    let mut policy: Value = serde_json::from_slice(&fs::read(&policy_path).unwrap()).unwrap();
    policy["global_snapshots"][0]["text"] = json!(globals);
    fs::write(&policy_path, serde_json::to_vec_pretty(&policy).unwrap()).unwrap();
    for input in [
        "aros-targets.toml",
        "meta/globals.snapshot",
        "meta/policy.json",
    ] {
        rebind_native_fixture_input(&mut fixture, input);
    }
    fixture.write_contract();
    fixture
}

fn invoke(fixture: &Fixture, cpu: &str, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aros-transpiler"))
        .arg("--source-dir")
        .arg(fixture.root.path())
        .arg("--output")
        .arg(fixture.output())
        .args([
            "--native-consumer-profile",
            "fixture-native",
            "--cpu",
            cpu,
            "--platform",
            "fixture",
            "--family",
            "",
            "--variant",
            "",
            "--cpu32",
            "",
            "--toolchain",
            "llvm",
            "--use-mmu",
            "1",
            "--float-abi",
            if cpu == "arm" { "hard" } else { "" },
        ])
        .args(extra)
        .output()
        .unwrap()
}

fn genmodule_inventory_fixture() -> Fixture {
    let mut fixture = fixture("arm");
    let profiles = fixture.root.path().join("aros-targets.toml");
    let profile_text = fs::read_to_string(&profiles)
        .unwrap()
        .replace("platform = \"fixture\"", "platform = \"raspi\"")
        .replace(
            "use_mmu = true",
            "use_mmu = true\nmesa_version = \"26.0.0\"",
        );
    fs::write(&profiles, profile_text).unwrap();
    let mut context = fixture_native_selector_context();
    context.cpu = Some("arm".into());
    context.platform = Some("raspi".into());
    context.toolchain = Some("llvm".into());
    context.use_mmu = Some("1".into());
    context.float_abi = Some("hard".into());
    context.mesa_version = Some("26.0.0".into());
    let globals = fixture_native_selector_globals(&context);
    fs::write(fixture.root.path().join("meta/globals.snapshot"), &globals).unwrap();
    let policy_path = fixture.root.path().join("meta/policy.json");
    let mut policy: Value = serde_json::from_slice(&fs::read(&policy_path).unwrap()).unwrap();
    policy["global_snapshots"][0]["text"] = json!(globals);
    fs::write(&policy_path, serde_json::to_vec_pretty(&policy).unwrap()).unwrap();
    for path in [
        "aros-targets.toml",
        "meta/globals.snapshot",
        "meta/policy.json",
    ] {
        rebind_native_fixture_input(&mut fixture, path);
    }
    fixture.write_contract();
    fixture
}

fn invoke_genmodule_inventory(fixture: &Fixture) -> Output {
    invoke_genmodule_selection(fixture, true)
}

fn invoke_genmodule_selection(fixture: &Fixture, inventory_only: bool) -> Output {
    invoke_genmodule_scope(fixture, inventory_only, false)
}

fn invoke_genmodule_scope(fixture: &Fixture, inventory_only: bool, full: bool) -> Output {
    invoke_genmodule_scope_args(fixture, inventory_only, full, &[])
}

fn invoke_genmodule_scope_args(
    fixture: &Fixture,
    inventory_only: bool,
    full: bool,
    extra: &[&str],
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aros-transpiler"))
        .arg("--source-dir")
        .arg(fixture.root.path())
        .arg("--output")
        .arg(fixture.output())
        .args([
            if full {
                "--mesa-genmodule-full-inventory-profile"
            } else {
                "--native-consumer-profile"
            },
            "fixture-native",
            "--cpu",
            "arm",
            "--platform",
            "raspi",
            "--family",
            "",
            "--variant",
            "",
            "--toolchain",
            "llvm",
            "--cpu32",
            "",
            "--use-mmu",
            "1",
            "--float-abi",
            "hard",
            "--mesa-version",
            "26.0.0",
        ])
        .args(inventory_only.then_some("--mesa-genmodule-inventory-only"))
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn genmodule_full_scope_rejects_direct_unowned_port_inputs() {
    let mut fixture = genmodule_inventory_fixture();
    let initial = invoke_genmodule_scope(&fixture, true, true);
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let retained = fs::read(fixture.output()).unwrap();
    fixture.append("USER_INCLUDES := -I$(PORTSDIR)/unowned-direct\n%build_linklib mmake=direct-port-consumer libname=direct files=probe\n");
    rebind_native_fixture_input(&mut fixture, "mmakefile.src");
    fixture.write_contract();
    let rejected = invoke_genmodule_scope(&fixture, true, true);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("full-target source input has no owning %fetch declaration"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("unowned-direct"));
    assert_eq!(fs::read(fixture.output()).unwrap(), retained);
}

#[test]
fn genmodule_full_scope_is_explicit_and_does_not_adopt_sdk_roots() {
    let mut fixture = genmodule_inventory_fixture();
    // A full-target pass binds configuration, not SDK reachability. Deliberate
    // absent roots distinguish it from the selected-contract pass.
    fixture.contract["roots"] = json!(["absent-sdk-root"]);
    fixture.write_contract();
    let selected = invoke_genmodule_inventory(&fixture);
    assert!(!selected.status.success());
    assert!(String::from_utf8_lossy(&selected.stderr).contains("absent-sdk-root"));
    assert!(!fixture.output().exists());

    let full = invoke_genmodule_scope(&fixture, true, true);
    assert!(
        full.status.success(),
        "{}",
        String::from_utf8_lossy(&full.stderr)
    );
    let retained = fs::read(fixture.output()).unwrap();
    let document: Value = serde_json::from_slice(&retained).unwrap();
    assert_eq!(document["selection"]["scope"], "full-target");
    assert_eq!(document["selection"]["kind"], "sdk-consumer");
    assert_eq!(document["selection"]["profile"], "fixture-native");
    assert!(!fixture
        .output()
        .with_extension("native-invocation.json")
        .exists());
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
    let wrong_digest = "0".repeat(64);
    let rejected = invoke_genmodule_scope_args(
        &fixture,
        true,
        true,
        &["--mesa-genmodule-full-contract-sha256", &wrong_digest],
    );
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("digest differs from the caller binding"));
    assert_eq!(fs::read(fixture.output()).unwrap(), retained);
    fs::write(
        fixture.root.path().join("probe.c"),
        "changed sealed source\n",
    )
    .unwrap();
    let changed = invoke_genmodule_scope(&fixture, true, true);
    assert!(!changed.status.success());
    assert!(
        String::from_utf8_lossy(&changed.stderr).contains("measured source input digest differs")
    );
    assert_eq!(fs::read(fixture.output()).unwrap(), retained);
}

#[test]
fn genmodule_inventory_keeps_pending_sources_data_only_and_requires_fetch_ownership() {
    let mut fixture = genmodule_inventory_fixture();
    fixture.append("\
%fetch mmake=pending-fetch archive=probe-1 destination=$(PORTSDIR)/probe location=$(PORTSSOURCEDIR) archive_origins=https://example.invalid suffixes=tar.gz\n\
SOURCES := $(wildcard $(PORTSDIR)/probe/probe-1/lib/*.c)\n\
%build_linklib mmake=pending-lib libname=probe files=\"$(SOURCES:.c=)\"\n\
#MM- fixture-helper : pending-lib\n");
    rebind_native_fixture_input(&mut fixture, "mmakefile.src");
    fixture.write_contract();
    let result = invoke_genmodule_inventory(&fixture);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let retained = fs::read(fixture.output()).unwrap();
    let document: Value = serde_json::from_slice(&retained).unwrap();
    assert!(document["inventory"]["records"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
    assert!(!fixture
        .output()
        .with_extension("native-invocation.json")
        .exists());

    // Data-only export is not permission to emit a partial compilation graph.
    let result = invoke_genmodule_selection(&fixture, false);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("selected dependency pending-lib has no proven endpoint"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(fixture.output()).unwrap(), retained);

    fixture.append(
        "\
UNOWNED := $(wildcard $(PORTSDIR)/unowned/*.c)\n\
%build_linklib mmake=unowned-lib libname=unowned files=\"$(UNOWNED:.c=)\"\n\
#MM- fixture-helper : unowned-lib\n",
    );
    rebind_native_fixture_input(&mut fixture, "mmakefile.src");
    fixture.write_contract();
    let result = invoke_genmodule_inventory(&fixture);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("no owning %fetch declaration"));
    assert_eq!(fs::read(fixture.output()).unwrap(), retained);
    let full = invoke_genmodule_scope(&fixture, true, true);
    assert!(!full.status.success());
    assert!(String::from_utf8_lossy(&full.stderr).contains("no owning %fetch declaration"));
    assert_eq!(fs::read(fixture.output()).unwrap(), retained);
}

#[test]
fn genmodule_inventory_is_selected_data_only_and_preserves_output_on_source_drift() {
    let fixture = genmodule_inventory_fixture();
    let old_report = fixture.output().with_extension("skipped-flags.txt");
    fs::write(&old_report, "prior report retained\n").unwrap();
    let result = invoke_genmodule_inventory(&fixture);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let retained = fs::read(fixture.output()).unwrap();
    let document: Value = serde_json::from_slice(&retained).unwrap();
    assert_eq!(document["schema"], "aros-mesa-genmodule-selection-v1");
    assert_eq!(document["selection"]["profile"], "fixture-native");
    assert_eq!(document["selection"]["kind"], "sdk-consumer");
    assert_eq!(document["selection"]["scope"], "contract-roots");
    assert_eq!(
        document["selection"]["contract_sha256"],
        json!(aros_common::sha256_bytes(
            &fs::read(fixture.root.path().join("native.json")).unwrap()
        ))
    );
    // This selected graph has only an ordinary linklib. Empty module data is
    // truthful; it must not acquire an invented default MesaGL declaration.
    assert!(document["inventory"]["records"]
        .as_array()
        .unwrap()
        .is_empty());
    for extension in [
        "native-invocation.json",
        "source-inventory.cmake",
        "coverage.json",
    ] {
        assert!(
            !fixture.output().with_extension(extension).exists(),
            "{extension}"
        );
    }
    assert_eq!(
        fs::read_to_string(&old_report).unwrap(),
        "prior report retained\n"
    );
    fs::write(
        fixture.root.path().join("probe.c"),
        "changed sealed input\n",
    )
    .unwrap();
    let result = invoke_genmodule_inventory(&fixture);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("measured source input digest differs")
    );
    assert_eq!(fs::read(fixture.output()).unwrap(), retained);
    assert_eq!(
        fs::read_to_string(&old_report).unwrap(),
        "prior report retained\n"
    );
}

#[test]
fn generic_sdk_cli_selects_and_exports_three_llvm_graphs_without_boot_authority() {
    for cpu in ["x86_64", "arm", "aarch64"] {
        let fixture = fixture(cpu);
        let binding = invoke(&fixture, cpu, &["--validate-native-consumer-only"]);
        assert!(
            binding.status.success(),
            "{}",
            String::from_utf8_lossy(&binding.stderr)
        );
        let document: Value = serde_json::from_slice(&binding.stdout).unwrap();
        assert_eq!(document["schema"], "aros-sdk-consumer-validation-v1");
        assert_eq!(document["target"], fixture.contract["target"]);
        assert!(document.get("abi").is_none());
        assert!(!fixture.output().exists());
        let inventory = invoke(&fixture, cpu, &["--source-inventory-only"]);
        assert!(
            inventory.status.success(),
            "{}",
            String::from_utf8_lossy(&inventory.stderr)
        );
        assert!(!fixture.output().exists());
        assert!(fixture
            .output()
            .with_extension("source-inventory.cmake")
            .is_file());
        let graph = invoke(&fixture, cpu, &[]);
        assert!(
            graph.status.success(),
            "{}",
            String::from_utf8_lossy(&graph.stderr)
        );
        let output = fs::read_to_string(fixture.output()).unwrap();
        assert!(output.contains("fixture-helper"));
        assert!(!output.contains("aros_add_module(\n  fixture-kernel"));
        let report: Value = serde_json::from_slice(
            &fs::read(fixture.output().with_extension("native-invocation.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(report["native_contract_kind"], "sdk-consumer");
    }
}

#[test]
fn generic_sdk_cli_does_not_turn_valid_descriptor_into_missing_graph_proof() {
    let mut fixture = fixture("x86_64");
    fixture.contract["roots"] = json!(["missing-sdk-endpoint"]);
    fixture.write_contract();
    assert!(
        invoke(&fixture, "x86_64", &["--validate-native-consumer-only"])
            .status
            .success()
    );
    for args in [&["--source-inventory-only"][..], &[][..]] {
        let result = invoke(&fixture, "x86_64", args);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("missing-sdk-endpoint"));
        assert!(!fixture.output().exists());
        assert!(!fixture
            .output()
            .with_extension("source-inventory.cmake")
            .exists());
    }
}

#[test]
fn generic_sdk_cli_rejects_changed_sealed_inputs_and_root_overrides() {
    let changed = fixture("aarch64");
    fs::write(changed.root.path().join("probe.cpp"), b"changed\n").unwrap();
    let output = invoke(&changed, "aarch64", &["--validate-native-consumer-only"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("measured source input digest differs")
    );

    let mut override_root = fixture("arm");
    override_root.contract["make_variables"] = json!({"AROS_DEVELOPER": "foreign"});
    override_root.write_contract();
    let output = invoke(&override_root, "arm", &["--validate-native-consumer-only"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("must not override source SDK root dependency variable AROS_DEVELOPER"));
    assert!(!override_root.output().exists());
}

#[test]
fn generic_sdk_cli_and_engine_validate_same_binding_and_both_graph_passes() {
    for cpu in ["x86_64", "arm", "aarch64"] {
        let fixture = fixture(cpu);
        let source = fixture.root.path().canonicalize().unwrap();
        let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../aros-cmake-engine/engine")
            .canonicalize()
            .unwrap();
        let float = if cpu == "arm" { "hard" } else { "" };
        let bytes = if cpu == "arm" { 4 } else { 8 };
        let flags = if cpu == "arm" { "-mfloat-abi=hard" } else { "" };
        let script = source.join("generic-cmake-check.cmake");
        fs::write(
            &script,
            format!(
                r#"
cmake_minimum_required(VERSION 3.22)
include("${{ENGINE}}/NativeConsumerContract.cmake")
set(AROS_SOURCE_DIR "${{SOURCE}}")
set(AROS_NATIVE_CONSUMER_CONTRACT "${{SOURCE}}/native.json")
file(SHA256 "${{AROS_NATIVE_CONSUMER_CONTRACT}}" AROS_NATIVE_CONSUMER_CONTRACT_SHA256)
set(AROS_TARGET_PROFILE fixture-native)
set(AROS_TARGET_CPU {cpu})
set(AROS_TARGET_PLATFORM fixture)
set(AROS_TARGET_TRIPLE {cpu}-unknown-aros)
set(AROS_TOOLCHAIN llvm)
set(AROS_TARGET_FAMILY "")
set(AROS_TARGET_VARIANT "")
set(AROS_TARGET_CPU32 "")
set(GCC_CONFIG_FLOAT_ABI "{float}")
set(AROS_ABI_FLAVOUR standalone)
set(AROS_ABI_PLATFORM_SMP OFF)
set(AROS_ENABLE_MMU ON)
set(AROS_TRANSPILER_BIN "${{TRANSPILER}}")
aros_validate_native_consumer_contract()
aros_lock_native_consumer_contract_identity()
# Synthetic detection facts test admission, not actual compiler qualification.
set(CMAKE_C_COMPILER_ID Clang)
set(CMAKE_CXX_COMPILER_ID Clang)
set(CMAKE_ASM_COMPILER_ID Clang)
set(CMAKE_C_COMPILER_TARGET {cpu}-unknown-aros)
set(CMAKE_CXX_COMPILER_TARGET {cpu}-unknown-aros)
set(CMAKE_ASM_COMPILER_TARGET {cpu}-unknown-aros)
set(CMAKE_SIZEOF_VOID_P {bytes})
set(CMAKE_C_FLAGS "{flags}")
set(CMAKE_CXX_FLAGS "{flags}")
set(CMAKE_ASM_FLAGS "{flags}")
aros_validate_native_consumer_compiler_target()
aros_native_transpiler_arguments(native_args)
foreach(mode IN ITEMS inventory export)
    set(preparation "")
    if(mode STREQUAL inventory)
        set(preparation --source-inventory-only)
    endif()
    execute_process(COMMAND "${{TRANSPILER}}" ${{preparation}} ${{native_args}}
        --source-dir "${{SOURCE}}" --output "${{OUTPUT}}"
        --cpu {cpu} --platform fixture --family "" --variant "" --cpu32 ""
        --toolchain llvm --use-mmu 1 --float-abi "{float}"
        RESULT_VARIABLE result ERROR_VARIABLE error)
    if(NOT result STREQUAL "0")
        message(FATAL_ERROR "selected ${{mode}} failed: ${{error}}")
    endif()
endforeach()
file(WRITE "${{SOURCE}}/generic-passed" "same selection")
"#
            ),
        )
        .unwrap();
        let result = Command::new("cmake")
            .arg(format!("-DENGINE={}", engine.display()))
            .arg(format!("-DSOURCE={}", source.display()))
            .arg(format!("-DOUTPUT={}", fixture.output().display()))
            .arg(format!(
                "-DTRANSPILER={}",
                env!("CARGO_BIN_EXE_aros-transpiler")
            ))
            .arg("-P")
            .arg(script)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(source.join("generic-passed").is_file());
        assert!(fixture.output().is_file());
    }
}
