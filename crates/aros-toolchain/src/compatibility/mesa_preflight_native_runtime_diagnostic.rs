//! Read-only diagnosis of a retained native Source/runtime configuration join.

use super::{read_bytes, read_global_inputs};
use aros_common::{make_native_invocation::NativeMakeInvocation, TargetProfile};
use aros_transpiler::{
    metamake_project::{ConfiguredProject, ProjectGlobals},
    native_owner_projection::discover_meta_inputs,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[test]
#[ignore = "requires explicit retained native Source/runtime case JSON; data-only diagnostic"]
fn actual_source_runtime_configuration_diagnostic() {
    let case_path = PathBuf::from(
        std::env::var_os("AROS_TEST_MESA_PREFLIGHT_CASE").expect("set explicit case JSON"),
    );
    assert!(case_path.is_absolute());
    let case: Value = serde_json::from_slice(&read_bytes(&case_path).unwrap()).unwrap();
    let case_parent = case_path.parent().expect("case JSON has a parent");
    let source = case_path_value(&case, "source_root");
    let output = case_path_value(&case, "output_root");
    let build = output.join("configure");
    assert!(source.is_absolute() && output.is_absolute());

    let collection = &case["native_collection"];
    let selected_project = collection["project"].as_str().unwrap();
    let source_join = &collection["source_join"];
    let profile_name = source_join["source_profile"].as_str().unwrap();
    let host = source_join["host"].as_str().unwrap();

    // Require the parent-produced exact-one-call proof, then parse that same
    // retained invocation. No owner, observation, or source snapshot is rebuilt.
    let proof: Value = serde_json::from_slice(
        &read_bytes(&case_parent.join("native-root-collection.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(proof["calls"].as_u64(), Some(1));
    let proof_calls = proof["invocations"].as_array().unwrap();
    assert_eq!(proof_calls.len(), 1);
    let collection_output = PathBuf::from(collection["output_root"].as_str().unwrap());
    assert!(collection_output.is_absolute());
    assert!(
        proof["evidence"].as_str() == collection_output.to_str(),
        "retained proof evidence root differs from case"
    );
    let invocation_bytes = read_bytes(
        &collection_output
            .join("d")
            .join("call-0000")
            .join("invocation.json"),
    )
    .unwrap();
    let invocation = NativeMakeInvocation::parse(&invocation_bytes).unwrap();
    assert!(
        proof_calls[0]["cwd"].as_str() == invocation.working_directory().to_str(),
        "retained proof cwd differs from parsed invocation"
    );
    assert!(
        proof_calls[0]["argv0"].as_str() == invocation.argument_zero(),
        "retained proof argv0 differs from parsed invocation"
    );
    let proof_arguments =
        serde_json::from_value::<Vec<String>>(proof_calls[0]["argv"].clone()).unwrap();
    assert!(
        proof_arguments.as_slice() == invocation.arguments(),
        "retained proof argv differs from parsed invocation"
    );

    let launch: Value = serde_json::from_slice(
        &read_bytes(&case_parent.join("native-launch-inputs.json")).unwrap(),
    )
    .unwrap();
    let read_inputs: Vec<(PathBuf, aros_common::Sha256Digest)> =
        serde_json::from_value(launch["read_inputs"].clone()).unwrap();

    let target_config = TargetProfile::load_config(&source.join("aros-targets.toml")).unwrap();
    let target = target_config
        .targets
        .iter()
        .find(|target| target.name == profile_name)
        .expect("selected Source target exists");
    let source_profile =
        crate::compatibility::SourceMesaPreflightProfile::load(&source, target).unwrap();

    // Reconstruct the diagnostic's expected project/globals from the current
    // sealed Source policy, not from generated runtime data.
    let contract_path = target
        .sdk_consumer_contract
        .as_deref()
        .expect("selected Source target has an SDK consumer contract");
    let contract: Value =
        serde_json::from_slice(&read_bytes(&source.join(contract_path)).unwrap()).unwrap();
    let projection_path = contract["metamake_projection"].as_str().unwrap();
    let policy: Value =
        serde_json::from_slice(&read_bytes(&source.join(projection_path)).unwrap()).unwrap();
    let configuration_source = policy["configuration_source"].as_str().unwrap();
    let mut projected_configuration =
        String::from_utf8(read_bytes(&source.join(configuration_source)).unwrap()).unwrap();
    for substitution in policy["substitutions"].as_array().unwrap() {
        let token = substitution["name"].as_str().unwrap();
        let value = substitution["value"].as_str().unwrap();
        assert!(projected_configuration.contains(token));
        projected_configuration = projected_configuration.replace(token, value);
    }
    let expected_project = ConfiguredProject::parse(
        &projected_configuration,
        policy["project"].as_str().unwrap(),
    )
    .unwrap();

    let discovered = discover_meta_inputs(
        &source,
        &expected_project.ignored_directories,
        &BTreeSet::new(),
    )
    .unwrap();
    let logical_files = discovered
        .into_keys()
        .map(|logical| source.join(logical))
        .collect::<Vec<_>>();
    let projection = source_profile
        .native_owner_projection(&logical_files, host, &BTreeSet::new(), None)
        .unwrap();

    let actual_project = ConfiguredProject::parse_only_project(
        &String::from_utf8(read_bytes(&build.join("mmake.config")).unwrap()).unwrap(),
        selected_project,
    )
    .unwrap();
    let runtime_globals = read_global_inputs(
        &actual_project,
        invocation.environment().clone(),
        &read_inputs,
        &build,
        &source,
    )
    .unwrap();

    let mut differences = Vec::new();
    report_project_differences(&expected_project, &actual_project, &mut differences);

    let mut initial = expected_project.variables.clone();
    for binding in policy["environment"].as_array().unwrap().iter().chain(
        policy["host_environment"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["host"].as_str() == Some(host))
            .flat_map(|entry| entry["bindings"].as_array().unwrap().iter()),
    ) {
        let name = binding["name"].as_str().unwrap().to_owned();
        let value = binding["value"].as_str().unwrap().to_owned();
        assert!(initial.insert(name, value).is_none());
    }
    let expected_global_files = policy["global_snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|snapshot| {
            (
                snapshot["source"].as_str().unwrap().to_owned(),
                snapshot["text"].as_str().unwrap().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    let expected_globals = ProjectGlobals::parse(&initial, &expected_global_files).unwrap();
    for (name, value) in &expected_globals.values {
        if let Err(category) = runtime_globals.verify_selected_values(
            &BTreeMap::from([(name.clone(), value.clone())]),
            &BTreeSet::new(),
        ) {
            differences.push(format!("global.{name}: {category}"));
        }
    }
    for name in policy["declared_absent"].as_array().unwrap() {
        let name = name.as_str().unwrap();
        if let Err(category) = runtime_globals
            .verify_selected_values(&BTreeMap::new(), &BTreeSet::from([name.to_owned()]))
        {
            differences.push(format!("global.{name}: {category}"));
        }
    }
    for difference in &differences {
        eprintln!("runtime configuration difference: {difference}");
    }

    let matched =
        projection.match_runtime_configuration(&source, &actual_project, &runtime_globals);
    if let Err(category) = &matched {
        eprintln!("runtime configuration guard: {category}");
    }
    assert!(
        matched.is_ok(),
        "Source policy/runtime configuration mismatch; see identifier-only differences above"
    );
}

fn case_path_value(case: &Value, key: &str) -> PathBuf {
    let path = PathBuf::from(case[key].as_str().unwrap());
    assert!(path.is_absolute());
    path
}

fn report_project_differences(
    expected: &ConfiguredProject,
    actual: &ConfiguredProject,
    differences: &mut Vec<String>,
) {
    macro_rules! compare {
        ($field:ident) => {
            if expected.$field != actual.$field {
                differences.push(format!("project.{}: metadata mismatch", stringify!($field)));
            }
        };
    }
    compare!(name);
    compare!(top);
    compare!(default_makefile);
    compare!(default_target);
    compare!(ignored_directories);
    compare!(added_makefiles);
    compare!(global_variable_files);
    compare!(generator_inputs);
    compare!(generator_command);
    compare!(global_generator_command);
    compare!(variables);
    // make_command is deliberately excluded by match_runtime_configuration.
}
