//! Synthetic consumer graphs prove selection, not compiler execution or boot.
use super::*;

fn consumer_fixture(extra_recipe: bool) -> Fixture {
    let mut fixture = Fixture::new();
    if extra_recipe {
        add_unowned_literal_object_failure(&fixture, None);
    }
    let recipes = if extra_recipe {
        vec!["mmakefile.src", "extra/mmakefile.src"]
    } else {
        vec!["mmakefile.src"]
    };
    install_native_metamake_projection(&mut fixture, &recipes);
    let profile_path = fixture.root.path().join("aros-targets.toml");
    let profiles = fs::read_to_string(&profile_path)
        .unwrap()
        .replace("native_build_contract", "native_consumer_contract");
    fs::write(profile_path, profiles).unwrap();
    bind_fixture_input(&mut fixture, "aros-targets.toml");
    bind_fixture_input(&mut fixture, "config/make.cfg.in");
    let allowed = [
        "schema",
        "profile",
        "source_baseline",
        "inputs",
        "abi",
        "metamake_projection",
        "make_variables",
        "host_make_variables",
        "make_include_bindings",
        "generated_make_templates",
        "optional_meta_dependencies",
        "host_file_generators",
        "kernel_compiler_role",
    ];
    fixture
        .contract
        .as_object_mut()
        .unwrap()
        .retain(|key, _| allowed.contains(&key.as_str()));
    fixture.contract["schema"] = json!("aros-native-consumer-contract-v1");
    fixture.contract["roots"] = json!(["fixture-helper"]);
    fixture.write_contract();
    fixture
}

fn reseal_make_config(fixture: &mut Fixture, config: &str) {
    fs::write(fixture.root.path().join("config/make.cfg.in"), config).unwrap();
    rebind_native_fixture_input(fixture, "config/make.cfg.in");
}

fn invoke_consumer(fixture: &Fixture, extra: &[&str]) -> Output {
    let mut args = vec!["--native-consumer-profile", "fixture-native"];
    args.extend_from_slice(extra);
    fixture.invoke(false, &args)
}

#[test]
fn consumer_exports_real_selected_archive_without_boot_or_package_facts() {
    let fixture = consumer_fixture(false);
    let result = invoke_consumer(&fixture, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let cmake = fs::read_to_string(fixture.output()).unwrap();
    assert!(cmake.contains("fixture-helper"));
    assert!(!cmake.contains("aros_add_module(\n  fixture-kernel"));
    let report: Value = serde_json::from_slice(
        &fs::read(fixture.output().with_extension("native-invocation.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["native_contract_kind"], "consumer");
    assert_eq!(report["native_profile"], "fixture-native");
    assert_eq!(
        report["qualification"],
        "source-invocation-scope-not-build-proof"
    );
}

#[test]
fn consumer_prepares_inventory_and_scopes_only_source_proven_uninvoked_errors() {
    let fixture = consumer_fixture(true);
    let result = invoke_consumer(&fixture, &["--source-inventory-only"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fixture.output().exists());
    assert!(fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
    let report: Value = serde_json::from_slice(
        &fs::read(fixture.output().with_extension("native-invocation.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        report["source_uninvoked_capability_failures"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let exported = invoke_consumer(&fixture, &[]);
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
}

#[test]
fn consumer_does_not_waive_required_missing_source_endpoints() {
    let fixture = consumer_fixture(false);
    fixture.append("#MM- fixture-helper : required-missing-sdk\n");
    let mut fixture = fixture;
    rebind_native_fixture_input(&mut fixture, "mmakefile.src");
    for arguments in [vec![], vec!["--source-inventory-only"]] {
        let result = invoke_consumer(&fixture, &arguments);
        assert_failure(&result, &fixture.output(), "required-missing-sdk");
        assert!(!fixture
            .output()
            .with_extension("source-inventory.cmake")
            .exists());
    }
}

#[test]
fn consumer_roots_cannot_fall_back_to_a_guessed_graph() {
    let mut fixture = consumer_fixture(false);
    fixture.contract["roots"] = json!(["nonexistent-root"]);
    fixture.write_contract();
    let result = invoke_consumer(&fixture, &[]);
    assert_failure(&result, &fixture.output(), "nonexistent-root");
}

#[test]
fn consumer_required_recipe_capability_failure_cannot_be_scoped_out() {
    let mut fixture = consumer_fixture(true);
    add_unowned_literal_object_failure(&fixture, Some("uninvoked-owner"));
    rebind_native_fixture_input(&mut fixture, "extra/mmakefile.src");
    fixture.append("#MM- fixture-helper : uninvoked-owner\n");
    rebind_native_fixture_input(&mut fixture, "mmakefile.src");
    for overrides in [&["--source-inventory-only"][..], &[][..]] {
        let result = invoke_consumer(&fixture, overrides);
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("exactly one source prerequisite"),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        for path in [
            fixture.output(),
            fixture.output().with_extension("source-inventory.cmake"),
            fixture.output().with_extension("native-invocation.json"),
        ] {
            assert!(
                !path.exists(),
                "a failed selected recipe published {}",
                path.display()
            );
        }
    }
}

#[test]
fn consumer_validation_only_is_closed_json_without_published_outputs() {
    let fixture = consumer_fixture(false);
    let output = invoke_consumer(&fixture, &["--validate-native-consumer-only"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["schema"], "aros-native-consumer-validation-v1");
    assert_eq!(
        document["qualification"],
        "source-binding-not-graph-or-build-proof"
    );
    assert_eq!(document["profile"], "fixture-native");
    assert_eq!(document["sdk_include_relative"], "SYS/Developer/include");
    assert!(document["input_paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == "config/make.cfg.in"));
    assert_eq!(
        document["contract_sha256"],
        aros_common::sha256_bytes(&fs::read(fixture.root.path().join("native.json")).unwrap())
            .to_string()
    );
    assert_eq!(document["abi"], fixture.contract["abi"]);
    assert_no_consumer_outputs(&fixture);

    // Contract validity is not graph closure: the read-only mode must not
    // claim that this absent SDK endpoint can build.
    let mut fixture = fixture;
    fixture.contract["roots"] = json!(["missing-sdk"]);
    fixture.write_contract();
    assert!(
        invoke_consumer(&fixture, &["--validate-native-consumer-only"])
            .status
            .success()
    );
    assert!(!invoke_consumer(&fixture, &[]).status.success());
}

#[test]
fn consumer_validation_requires_a_sealed_safe_source_sdk_include_root() {
    // An unsealed configuration cannot supply include-root authority.
    let mut unsealed = consumer_fixture(false);
    unsealed.contract["inputs"]
        .as_array_mut()
        .unwrap()
        .retain(|input| input["path"] != "config/make.cfg.in");
    unsealed.write_contract();
    assert_validation_rejected(&unsealed, "consumer contract must seal config/make.cfg.in");

    // The loader must reject a changed configuration before the new root is
    // read, even when its old digest remains in the contract.
    let changed_unsealed = consumer_fixture(false);
    fs::write(
        changed_unsealed.root.path().join("config/make.cfg.in"),
        "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\n",
    )
    .unwrap();
    assert_validation_rejected(&changed_unsealed, "measured source input digest differs");

    // Resealed but unsupported source forms still fail closed, including stale
    // values hidden by late placeholders, undecidable branches, or ignored
    // Make directives. `${...}` is unsupported only in raw Make RHS text; the
    // trusted TARGETDIR seed remains valid in the fixture's $(TARGETDIR) form.
    for (config, expected) in [
        (
            "AROS_DEVELOPER := ${AROS_BUILD_DIR}/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "BUILD_ROOT = ${AROS_BUILD_DIR}\nAROS_DEVELOPER := $(BUILD_ROOT)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nAROS_INCLUDES := @configured_includes@\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nifeq ($(AROS_TARGET_ARCH),fixture)\nAROS_INCLUDES := $(AROS_DEVELOPER)/other\nendif\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\n$(eval AROS_INCLUDES := $(AROS_DEVELOPER)/foreign)\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\n$(subst x,y,$(eval AROS_INCLUDES := $(AROS_DEVELOPER)/foreign))\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\n$(eval AROS_INCLUDES := $(AROS_DEVELOPER)/foreign\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nLAYOUT_RESULT := $(eval AROS_INCLUDES := $(AROS_DEVELOPER)/foreign)\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nifeq ($(eval AROS_INCLUDES := $(AROS_DEVELOPER)/foreign),yes)\nUNRELATED_CONDITION := evaluated\nendif\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\ndefine CHANGE_LAYOUT\n$(eval AROS_INCLUDES := $(AROS_DEVELOPER)/foreign)\nendef\n$(CHANGE_LAYOUT)\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nEARLY = $(LATE)\nLATE = $(eval AROS_INCLUDES := $(AROS_DEVELOPER)/foreign)\n$(EARLY)\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_DIR_INCLUDE := include\nAROS_INCLUDES = $(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)\nAROS_DIR_INCLUDE := @configured_include@\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nAROS_INCLUDES += /other\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nSIDE_EFFECT := initial\nSIDE_EFFECT += $(eval AROS_INCLUDES := $(AROS_DEVELOPER)/foreign)\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/foreign\nAROS_INCLUDES ?= $(AROS_DEVELOPER)/include\n",
            "source AROS_INCLUDES must be exactly AROS_DEVELOPER/include",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nSDK_LEAF := foreign\nAROS_INCLUDES := $(AROS_DEVELOPER)/$(SDK_LEAF)\nSDK_LEAF := include\n",
            "source AROS_INCLUDES must be exactly AROS_DEVELOPER/include",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\noverride AROS_INCLUDES := $(AROS_DEVELOPER)/foreign\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nundefine AROS_INCLUDES\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\ndefine AROS_INCLUDES\n$(AROS_DEVELOPER)/other\nendef\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nifdef OPTIONAL_LAYOUT\nAROS_INCLUDES := $(AROS_DEVELOPER)/foreign\nendif\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nexport AROS_INCLUDES := $(AROS_DEVELOPER)/foreign\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nAROS_DEVELOPER != printf '%s\\n' /tmp/foreign\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\noverride AROS_DEVELOPER != printf '%s\\n' /tmp/foreign\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\noverride TARGETDIR != printf '%s\\n' /tmp/foreign\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nTARGETDIR != printf '%s\\n' /tmp/foreign\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "TARGETDIR != printf '%s\\n' /tmp/foreign\nAROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nAROS_INCLUDES extra != printf '%s\\n' /tmp/foreign\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER ::= $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES ::= $(AROS_DEVELOPER)/include\nAROS_INCLUDES :::= $(AROS_DEVELOPER)/foreign\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER ::= $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES ::= $(AROS_DEVELOPER)/include\noverride AROS_INCLUDES :::= $(AROS_DEVELOPER)/foreign\nAROS_INCLUDES ::= $(AROS_DEVELOPER)/include\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER ::= $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES ::= $(AROS_DEVELOPER)/include\nTARGETDIR :::= /tmp/foreign\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "TARGETDIR :::= /tmp/foreign\nAROS_DEVELOPER ::= $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES ::= $(AROS_DEVELOPER)/include\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER ::= $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES ::= $(AROS_DEVELOPER)/include\nUNRELATED :::= $(eval AROS_INCLUDES := $(AROS_DEVELOPER)/foreign)\n",
            "source AROS_DEVELOPER root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nifeq (left,right)\nAROS_INCLUDES := $(AROS_DEVELOPER)/inactive\nelse ifeq ($(AROS_TARGET_ARCH),fixture)\nAROS_INCLUDES := $(AROS_DEVELOPER)/foreign\nendif\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nifeq (left,right)\nAROS_INCLUDES := $(AROS_DEVELOPER)/inactive\nelse ifdef OPTIONAL_LAYOUT\nAROS_INCLUDES := $(AROS_DEVELOPER)/foreign\nendif\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\nifeq (left,right)\nAROS_INCLUDES := $(AROS_DEVELOPER)/inactive\nelse ifndef OPTIONAL_LAYOUT\nAROS_INCLUDES := $(AROS_DEVELOPER)/foreign\nendif\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            r"AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer
AROS_INCLUDES := $(AROS_DEVELOPER)/\
include
",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/$(UNKNOWN_INCLUDE)\n",
            "source AROS_INCLUDES root cannot be resolved",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/headers\n",
            "source AROS_INCLUDES must be exactly AROS_DEVELOPER/include",
        ),
        (
            "AROS_DEVELOPER := $(TARGETDIR)/SYS/../../outside/Developer\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\n",
            "source AROS_DEVELOPER contains an unsafe or noncanonical path component",
        ),
    ] {
        let mut fixture = consumer_fixture(false);
        reseal_make_config(&mut fixture, config);
        assert_validation_rejected(&fixture, expected);
    }

    // Definite later replacement clears prior placeholder uncertainty. An
    // undecidable condition on an unrelated variable and an assignment in a
    // definitely false branch must not invalidate the proven SDK root.
    let mut source_proven = consumer_fixture(false);
    fs::write(
        source_proven.root.path().join("config/make.cfg.in"),
        "AROS_DEVELOPER ::= $(TARGETDIR)/SYS/Developer\n\
         SDK_LEAF := include\n\
         AROS_INCLUDES := @configured_includes@\n\
         AROS_INCLUDES := $(AROS_DEVELOPER)/$(SDK_LEAF)\n\
         SDK_LEAF := @late_configured_include@\n\
         UNRELATED_SHELL_ASSIGNMENT != printf '%s\\n' harmless\n\
         UNRELATED_IMMEDIATE ::= harmless\n\
         ifeq ($(AROS_TARGET_ARCH),fixture)\n\
         UNRELATED_PATH := source-specific\n\
         endif\n\
         ifeq (left,right)\n\
         AROS_DEVELOPER != printf '%s\\n' /tmp/inactive\n\
         endif\n\
         ifdef OPTIONAL_LAYOUT\n\
         UNRELATED_CONDITIONAL := source-specific\n\
         endif\n\
         define INERT_LAYOUT\n\
         $(eval AROS_INCLUDES := $(AROS_DEVELOPER)/foreign)\n\
         endef\n\
         ifeq (left,right)\n\
         $(eval AROS_INCLUDES := $(AROS_DEVELOPER)/inactive)\n\
         endif\n\
         ifeq (left,right)\n\
         AROS_INCLUDES := $(AROS_DEVELOPER)/inactive\n\
         else ifeq (left,left)\n\
         AROS_INCLUDES := $(AROS_DEVELOPER)/include\n\
         else\n\
         AROS_INCLUDES := $(AROS_DEVELOPER)/other\n\
         endif\n",
    )
    .unwrap();
    rebind_native_fixture_input(&mut source_proven, "config/make.cfg.in");
    source_proven.contract["make_variables"] = json!({"UNRELATED_SOURCE_VARIABLE": "safe"});
    source_proven.write_contract();
    let output = invoke_consumer(&source_proven, &["--validate-native-consumer-only"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["sdk_include_relative"], "SYS/Developer/include");
    assert_no_consumer_outputs(&source_proven);

    // Reject overrides in shared configuration and even in an unselected
    // host table: neither may shadow the source-derived path later.
    let mut shared_override = consumer_fixture(false);
    shared_override.contract["make_variables"] = json!({ "AROS_INCLUDES": "elsewhere" });
    shared_override.write_contract();
    assert_validation_rejected(
        &shared_override,
        "must not override source SDK root dependency variable AROS_INCLUDES",
    );

    let mut host_override = consumer_fixture(false);
    let current_host = aros_common::target::native_host_key().unwrap();
    let other_host = if current_host == "linux-x86_64" {
        "macos-aarch64"
    } else {
        "linux-x86_64"
    };
    let mut host_variables = serde_json::Map::new();
    host_variables.insert(current_host.into(), json!({}));
    host_variables.insert(other_host.into(), json!({ "AROS_DEVELOPER": "elsewhere" }));
    host_override.contract["host_make_variables"] = Value::Object(host_variables);
    host_override.write_contract();
    assert_validation_rejected(
        &host_override,
        "must not override source SDK root dependency variable AROS_DEVELOPER",
    );

    let alias_config = "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nINNER := include\nSDK_LEAF := $(INNER)\nAROS_INCLUDES := $(AROS_DEVELOPER)/$(SDK_LEAF)\nSDK_LEAF := include\n";
    let mut shared_alias_override = consumer_fixture(false);
    reseal_make_config(&mut shared_alias_override, alias_config);
    shared_alias_override.contract["make_variables"] = json!({"INNER": "elsewhere"});
    shared_alias_override.write_contract();
    assert_validation_rejected(
        &shared_alias_override,
        "must not override source SDK root dependency variable INNER",
    );

    let current_host = aros_common::target::native_host_key().unwrap();
    let mut current_host_alias_override = consumer_fixture(false);
    reseal_make_config(&mut current_host_alias_override, alias_config);
    let mut current_host_variables = serde_json::Map::new();
    current_host_variables.insert(current_host.to_owned(), json!({"SDK_LEAF": "elsewhere"}));
    current_host_alias_override.contract["host_make_variables"] =
        Value::Object(current_host_variables);
    current_host_alias_override.write_contract();
    assert_validation_rejected(
        &current_host_alias_override,
        "must not override source SDK root dependency variable SDK_LEAF",
    );

    let other_host = if current_host == "linux-x86_64" {
        "macos-aarch64"
    } else {
        "linux-x86_64"
    };
    let mut other_host_alias_override = consumer_fixture(false);
    reseal_make_config(&mut other_host_alias_override, alias_config);
    let mut other_host_variables = serde_json::Map::new();
    other_host_variables.insert(current_host.to_owned(), json!({}));
    other_host_variables.insert(other_host.into(), json!({"SDK_LEAF": "elsewhere"}));
    other_host_alias_override.contract["host_make_variables"] = Value::Object(other_host_variables);
    other_host_alias_override.write_contract();
    assert_validation_rejected(
        &other_host_alias_override,
        "must not override source SDK root dependency variable SDK_LEAF",
    );
}

fn assert_validation_rejected(fixture: &Fixture, expected: &str) {
    let output = invoke_consumer(fixture, &["--validate-native-consumer-only"]);
    assert!(
        !output.status.success(),
        "accepted invalid SDK root: {expected}"
    );
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains(expected), "expected {expected}: {error}");
    assert_no_consumer_outputs(fixture);
}

fn assert_no_consumer_outputs(fixture: &Fixture) {
    for path in [
        fixture.output(),
        fixture.output().with_extension("native-invocation.json"),
        fixture.output().with_extension("source-inventory.cmake"),
    ] {
        assert!(
            !path.exists(),
            "validation-only mode published {}",
            path.display()
        );
    }
}

fn consumer_cmake(fixture: &Fixture, before: &str, after: &str) -> Output {
    let source = fixture.root.path().canonicalize().unwrap();
    let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine")
        .canonicalize()
        .unwrap();
    let script = source.join("consumer-cmake-check.cmake");
    let isa = fixture.contract["abi"]["isa"].as_str().unwrap();
    let abi = fixture.contract["abi"]["abi"].as_str().unwrap();
    let code_model = fixture.contract["abi"]["code_model"].as_str().unwrap();
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
set(AROS_TARGET_CPU riscv)
set(AROS_TARGET_PLATFORM fixture)
set(AROS_TARGET_TRIPLE riscv-aros)
set(AROS_TOOLCHAIN gnu)
set(AROS_TARGET_FAMILY "")
set(AROS_TARGET_VARIANT "")
set(AROS_TARGET_CPU32 "")
set(GCC_CONFIG_FLOAT_ABI ilp32f)
set(AROS_ABI_FLAVOUR standalone)
set(AROS_ABI_PLATFORM_SMP OFF)
set(AROS_ENABLE_MMU OFF)
set(AROS_TRANSPILER_BIN "${{TRANSPILER}}")
{before}
aros_validate_native_consumer_contract()
aros_lock_native_consumer_contract_identity()
file(WRITE "${{SOURCE}}/preproject-passed" "binding checked")
set(AROS_GNU_TARGET_COMPILE_OPTIONS -march={isa} -mabi={abi} -mcmodel={code_model})
{after}
aros_validate_native_consumer_compiler_target()
aros_native_transpiler_arguments(native_args)
if(DEFINED AROS_NATIVE_BUILD_CORE_RECIPE OR DEFINED AROS_NATIVE_BUILD_PACKAGE_FORMAT OR
   DEFINED AROS_NATIVE_BUILD_MEDIA_CHIP)
    message(FATAL_ERROR "consumer manufactured full-build facts")
endif()
foreach(mode IN ITEMS inventory export)
    set(preparation "")
    if(mode STREQUAL inventory)
        set(preparation --source-inventory-only)
    endif()
    execute_process(COMMAND "${{TRANSPILER}}" ${{preparation}} ${{native_args}}
        --source-dir "${{SOURCE}}" --output "${{OUTPUT}}"
        --cpu riscv --platform fixture --family "" --variant "" --cpu32 ""
        --toolchain gnu --use-mmu 0 --float-abi ilp32f
        RESULT_VARIABLE result ERROR_VARIABLE error)
    if(NOT result STREQUAL "0")
        message(FATAL_ERROR "selected ${{mode}} failed: ${{error}}")
    endif()
endforeach()
file(WRITE "${{SOURCE}}/consumer-args.txt" "${{native_args}}")
"#
        ),
    )
    .unwrap();
    Command::new("cmake")
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
        .unwrap()
}

#[test]
fn cmake_forwards_consumer_binding_to_both_real_graph_passes() {
    let fixture = consumer_fixture(false);
    let output = consumer_cmake(&fixture, "", "");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let args = fs::read_to_string(fixture.root.path().join("consumer-args.txt")).unwrap();
    assert!(args.starts_with(
        "--native-consumer-profile;fixture-native;--native-consumer-contract-sha256;"
    ));
    assert!(!args.contains("--native-profile;"));
    let cmake = fs::read_to_string(fixture.output()).unwrap();
    assert!(cmake.contains("fixture-helper"));
    let engine = include_str!("../../../aros-cmake-engine/engine/CMakeLists.txt");
    assert_eq!(engine.matches("${_aros_native_transpiler_args}").count(), 3);
    assert!(engine.contains("if(AROS_NATIVE_BUILD_CONTRACT OR AROS_NATIVE_CONSUMER_CONTRACT)"));
    assert!(
        engine
            .find("aros_validate_native_consumer_contract()")
            .unwrap()
            < engine.find("project(AROS-NX").unwrap()
    );
}

#[test]
fn cmake_rejects_consumer_digest_and_selector_errors_before_compiler_boundary() {
    for before in [
        "set(AROS_NATIVE_CONSUMER_CONTRACT_SHA256 0000000000000000000000000000000000000000000000000000000000000000)",
        "set(AROS_TARGET_CPU riscv64)",
        "set(AROS_ABI_FLAVOUR native)",
        "set(AROS_ABI_PLATFORM_SMP ON)",
        "set(AROS_TARGET_TRIPLE riscv64-aros)",
        "unset(AROS_TARGET_VARIANT)",
        "set(AROS_NATIVE_BUILD_CONTRACT invalid.json)",
        "set(AROS_NATIVE_CONSUMER_CONTRACT_LOCKED_PATH different.json)",
    ] {
        let fixture = consumer_fixture(false);
        let output = consumer_cmake(&fixture, before, "");
        assert!(!output.status.success(), "accepted {before}");
        assert!(!fixture.root.path().join("preproject-passed").exists());
        assert_no_consumer_outputs(&fixture);
    }
}

#[test]
fn cmake_rejects_consumer_compiler_option_and_binding_drift() {
    for (after, expected) in [
        ("list(APPEND AROS_GNU_TARGET_COMPILE_OPTIONS -mabi=ilp32f)", "exactly one GNU -mabi"),
        ("list(FILTER AROS_GNU_TARGET_COMPILE_OPTIONS EXCLUDE REGEX ^-mabi)\nlist(APPEND AROS_GNU_TARGET_COMPILE_OPTIONS -mabi=ilp32)", "GNU -mabi differs"),
        ("list(FILTER AROS_GNU_TARGET_COMPILE_OPTIONS EXCLUDE REGEX ^-march)\nlist(APPEND AROS_GNU_TARGET_COMPILE_OPTIONS -march=rv64gc)", "GNU -march differs"),
        ("list(FILTER AROS_GNU_TARGET_COMPILE_OPTIONS EXCLUDE REGEX ^-mcmodel)", "exactly one GNU -mcmodel"),
        ("set(AROS_NATIVE_CONSUMER_CONTRACT_SHA256 invalid)", "configuration changed after validation"),
        ("set(AROS_TARGET_PROFILE another-profile)", "configuration changed after validation"),
        ("file(APPEND \"${AROS_NATIVE_CONSUMER_CONTRACT}\" \" \")", "contract changed after validation"),
        ("set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATION_CURRENT FALSE)", "requires fresh validation"),
        ("set(AROS_ABI_FLAVOUR emulation)", "AROS_ABI_FLAVOUR changed after validation"),
        ("set(AROS_ABI_PLATFORM_SMP ON)", "ABI or consumer selectors changed after validation"),
        ("set(AROS_ENABLE_MMU ON)", "ABI or consumer selectors changed after validation"),
        ("set(AROS_TARGET_TRIPLE riscv64-aros)", "AROS_TARGET_TRIPLE changed after validation"),
        ("set(AROS_TARGET_FAMILY unix)", "AROS_TARGET_FAMILY changed after validation"),
        ("set(AROS_NATIVE_CONSUMER_EXEC_SMP ON)", "ABI or consumer selectors changed after validation"),
    ] {
        let fixture = consumer_fixture(false);
        let output = consumer_cmake(&fixture, "", after);
        assert!(!output.status.success(), "accepted {after}");
        let message = String::from_utf8_lossy(&output.stderr).split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(message.contains(expected), "expected {expected}: {message}");
        assert!(fixture.root.path().join("preproject-passed").exists());
        assert_no_consumer_outputs(&fixture);
    }
}

#[test]
fn source_selection_helper_requires_fresh_consumer_binding_without_build_authority() {
    let valid = consumer_fixture(false);
    let output = consumer_cmake(
        &valid,
        "",
        r#"
_aros_native_source_selection_validated(_selected)
if(NOT _selected)
    message(FATAL_ERROR "fresh consumer selection was not admitted")
endif()
if(AROS_NATIVE_BUILD_CONTRACT OR AROS_NATIVE_BUILD_CONTRACT_VALIDATED OR
   DEFINED AROS_NATIVE_BUILD_CORE_RECIPE OR DEFINED AROS_NATIVE_BUILD_PACKAGE_FORMAT OR
   DEFINED AROS_NATIVE_BUILD_MEDIA_CHIP)
    message(FATAL_ERROR "consumer selection manufactured build authority")
endif()
file(WRITE "${SOURCE}/source-selection-validated" "consumer")
"#,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(valid.root.path().join("source-selection-validated")).unwrap(),
        "consumer"
    );

    for (mutation, expected) in [
        (
            "set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATION_CURRENT FALSE)\n_aros_native_source_selection_validated(_selected)",
            "requires fresh validation in this configure process",
        ),
        (
            "set(AROS_NATIVE_CONSUMER_CONTRACT_SHA256 0000000000000000000000000000000000000000000000000000000000000000)\n_aros_native_source_selection_validated(_selected)",
            "configuration changed after validation",
        ),
        (
            "set(AROS_NATIVE_BUILD_CONTRACT build.json)\n_aros_native_source_selection_validated(_selected)",
            "build and consumer selections are mutually exclusive",
        ),
        (
            "set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED TRUE)\n_aros_native_source_selection_validated(_selected)",
            "build and consumer selections are mutually exclusive",
        ),
    ] {
        let fixture = consumer_fixture(false);
        let output = consumer_cmake(&fixture, "", mutation);
        assert!(!output.status.success(), "accepted {mutation}");
        let message = String::from_utf8_lossy(&output.stderr)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(message.contains(expected), "expected {expected}: {message}");
        assert!(
            !fixture
                .root
                .path()
                .join("source-selection-validated")
                .exists(),
            "a rejected selection reached the post-validation marker"
        );
        assert_no_consumer_outputs(&fixture);
    }
}

#[test]
fn generated_sdk_object_groups_bind_program_inputs_for_consumer_selection() {
    use aros_transpiler::{
        sdk_objects::{SdkObjectDecl, SdkObjectGroupDecl},
        DependencyGraph,
    };

    let mut graph = DependencyGraph::default();
    graph.sdk_object_groups.push(SdkObjectGroupDecl {
        owner: "startup-objects".into(),
        file: "compiler/startup/mmakefile.src".into(),
        line: 12,
        objects: vec![SdkObjectDecl {
            source: "compiler/startup/startup.c".into(),
            intermediate: "${AROS_GENERATED_DIR}/compiler/startup/startup.o".into(),
            output: "${AROS_DEVELOPER_LIB_DIR}/startup.o".into(),
            language: "C".into(),
            defines: vec![],
            undefines: vec![],
            options: vec![],
            includes: vec![],
            line: 10,
        }],
    });

    let generated = aros_transpiler::generate_cmake(&graph);
    assert!(
        generated.contains("aros_compile_sdk_object("),
        "{generated}"
    );
    assert!(generated.contains("aros_sdk_object_group("), "{generated}");
    assert!(
        generated.contains(
            "if(AROS_NATIVE_BUILD_CONTRACT_VALIDATED OR AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED)\n    aros_bind_source_sdk_program_inputs()\nendif()"
        ),
        "consumer-bound graphs must bind program startup roles only after native selection validation: {generated}"
    );
}

#[test]
#[cfg(unix)]
fn engine_rejects_dropped_or_mixed_retained_selection_before_any_compiler() {
    use std::os::unix::fs::PermissionsExt;
    let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine")
        .canonicalize()
        .unwrap();
    for (arguments, expected) in [
        (
            vec!["-DAROS_NATIVE_CONSUMER_CONTRACT_LOCKED_PATH=old.json"],
            "Retained native CONSUMER binding",
        ),
        (
            vec!["-DAROS_NATIVE_BUILD_CONTRACT_LOCKED_SOURCE_DIR=old-source"],
            "Retained native BUILD binding",
        ),
        (
            vec![
                "-DAROS_NATIVE_CONSUMER_CONTRACT=consumer.json",
                "-DAROS_NATIVE_BUILD_CONTRACT=build.json",
            ],
            "never both",
        ),
        (
            vec![
                "-DAROS_NATIVE_BUILD_CONTRACT=build.json",
                "-DAROS_NATIVE_CONSUMER_CONTRACT_LOCKED_SHA256=old-digest",
            ],
            "Retained native CONSUMER binding",
        ),
        (
            vec![
                "-DAROS_NATIVE_CONSUMER_CONTRACT=consumer.json",
                "-DAROS_NATIVE_BUILD_CONTRACT_LOCKED_PATH=old.json",
            ],
            "Retained native BUILD binding",
        ),
        (
            vec!["-DAROS_NATIVE_CONSUMER_CONTRACT_SHA256=unbound"],
            "requires its contract",
        ),
    ] {
        let fixture = consumer_fixture(false);
        let marker = fixture.root.path().join("compiler-invoked");
        let compiler = fixture.root.path().join("compiler-marker");
        fs::write(
            &compiler,
            format!(
                "#!/bin/sh\nprintf invoked > '{}'\nexit 1\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&compiler, fs::Permissions::from_mode(0o755)).unwrap();
        let output = Command::new("cmake")
            .arg("-S")
            .arg(&engine)
            .arg("-B")
            .arg(fixture.root.path().join("cmake-build"))
            .arg(format!(
                "-DAROS_SOURCE_DIR={}",
                fixture.root.path().display()
            ))
            .arg(format!("-DCMAKE_C_COMPILER={}", compiler.display()))
            .arg(format!("-DCMAKE_CXX_COMPILER={}", compiler.display()))
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(error.contains(expected), "expected {expected}: {error}");
        assert!(
            !marker.exists(),
            "invalid selection reached compiler detection"
        );
        assert_no_consumer_outputs(&fixture);
    }
}
