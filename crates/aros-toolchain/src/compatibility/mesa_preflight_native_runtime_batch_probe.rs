//! Data-only batch check of retained native Source/runtime configuration.
//!
//! This does not recover the original preparation owner or clock and does not
//! qualify an SDK. It reads retained invocations and independently selected
//! Source policy inputs; it never launches Source, a compiler, or a recipe.

#![cfg(all(test, unix))]

use super::{read_bytes, read_global_inputs};
use aros_common::{
    directory_entry_names_nofollow_bounded,
    make_environment_prefix::LiteralMakeEnvironmentPrefix,
    make_native_invocation::{NativeMakeInvocation, PrivateNativeCaptureDirectory},
    sha256_bytes, TargetProfile,
};
use aros_transpiler::{
    metamake_project::ConfiguredProject,
    native_owner_projection::{discover_meta_inputs, NativeOwnerProjection},
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const CASE_ENV: &str = "AROS_TEST_MESA_PREFLIGHT_CASE";
const REPORT_ENV: &str = "AROS_TEST_NATIVE_RUNTIME_BATCH_REPORT";
const EXPECTED_CASE_CALLS: usize = 551;
const MAX_DIAGNOSTIC_CALLS: usize = 4096;
const MAX_INVOCATION_BYTES: usize = 64 * 1024;
const MAX_BATCH_INVOCATION_BYTES: usize = 64 * 1024 * 1024;
const DIAGNOSTIC_BUDGET: Duration = Duration::from_secs(60);
const CALL_DIRECTORY_FILES: [&str; 11] = [
    "call-completed.json",
    "discovery.status.json",
    "discovery.stderr",
    "discovery.stdout",
    "initial.status.json",
    "initial.stderr",
    "initial.stdout",
    "invocation.json",
    "observation-selection.json",
    "observation-started.json",
    "observation.json",
];

#[test]
#[ignore = "requires an explicit retained 551-call case and fresh private report path; data-only"]
fn actual_retained_native_runtime_policy_batch_probe() {
    run_probe().unwrap_or_else(|error| {
        panic!("retained native runtime policy batch probe failed: {error}")
    });
}

fn run_probe() -> Result<(), &'static str> {
    // This diagnostic owns only a fresh monotonic work budget. It does not
    // recreate or extend the preparation clock retained by the original run.
    let started = Instant::now();
    check_budget(started)?;

    let case_path =
        PathBuf::from(std::env::var_os(CASE_ENV).ok_or("explicit retained case path is absent")?);
    if !case_path.is_absolute() {
        return Err("retained case path is not absolute");
    }
    let case_bytes = read_bytes(&case_path).map_err(|_| "cannot read retained case JSON")?;
    let case: Value =
        serde_json::from_slice(&case_bytes).map_err(|_| "retained case JSON is malformed")?;
    let case_parent = case_path
        .parent()
        .ok_or("retained case path has no parent")?;
    let source = canonical_case_path(&case, "source_root")?;
    let output = canonical_case_path(&case, "output_root")?;
    let compiler = canonical_case_path(&case, "compiler_root")?;
    let build = output
        .join("configure")
        .canonicalize()
        .map_err(|_| "original configured build directory is unavailable")?;
    if !build.is_dir() {
        return Err("original configured build path is not a directory");
    }

    let dispatch = case
        .get("native_dispatch")
        .ok_or("case has no native dispatch selection")?;
    let direct = dispatch
        .get("direct_collection")
        .ok_or("case has no direct collection selection")?;
    let project_name = required_string(direct, "project")?;
    let host = required_string(direct, "host")?;
    let profile_name = required_string(direct, "source_profile")?;
    let roots = required_strings(direct, "roots")?;
    if roots.is_empty() {
        return Err("direct collection has no selected Source roots");
    }
    let maximum_calls = usize::try_from(
        direct["maximum_calls"]
            .as_u64()
            .ok_or("direct collection has no call bound")?,
    )
    .map_err(|_| "direct collection call bound is invalid")?;
    if maximum_calls != EXPECTED_CASE_CALLS || maximum_calls > MAX_DIAGNOSTIC_CALLS {
        return Err("direct collection call bound differs from the explicit 551-call case");
    }
    check_budget(started)?;

    let collection_path = PathBuf::from(required_string(direct, "output_root")?);
    if !collection_path.is_absolute()
        || collection_path
            .canonicalize()
            .map_err(|_| "retained direct collection directory is unavailable")?
            != collection_path
    {
        return Err("retained direct collection directory is not canonical");
    }
    let collection_directory = PrivateNativeCaptureDirectory::open(&collection_path)
        .map_err(|_| "retained direct collection directory is not private")?;
    let calls_path = collection_path.join("d");
    let namespace_directory = PrivateNativeCaptureDirectory::open(&calls_path)
        .map_err(|_| "retained direct call namespace is not private")?;
    let expected_slots = expected_slot_names(maximum_calls)?;
    let slot_names = directory_entry_names_nofollow_bounded(
        &calls_path,
        maximum_calls
            .checked_mul(2)
            .ok_or("retained call namespace bound overflowed")?,
    )
    .map_err(|_| "cannot enumerate retained direct call namespace")?;
    if slot_names.len() != expected_slots.len()
        || slot_names.into_iter().collect::<BTreeSet<_>>() != expected_slots
    {
        return Err("retained direct call namespace is not the exact bounded slot set");
    }

    let launch_path = case_parent.join("native-launch-inputs.json");
    let launch_bytes =
        read_bytes(&launch_path).map_err(|_| "cannot read retained launch inputs")?;
    let launch: Value = serde_json::from_slice(&launch_bytes)
        .map_err(|_| "retained launch inputs JSON is malformed")?;
    let read_inputs: Vec<(PathBuf, aros_common::Sha256Digest)> =
        serde_json::from_value(launch["read_inputs"].clone())
            .map_err(|_| "retained launch input digest pairs are malformed")?;
    let prefix_variable = required_string(dispatch, "prefix_variable")?;
    let prefix_text = launch["values"][prefix_variable]
        .as_str()
        .ok_or("retained launch inputs lack the selected Source prefix")?;
    let prefix = LiteralMakeEnvironmentPrefix::parse(prefix_text)
        .map_err(|_| "retained Source prefix is not a literal environment prefix")?;

    let mut invocations = Vec::with_capacity(maximum_calls);
    let mut invocation_bytes_total = 0usize;
    for slot in 0..maximum_calls {
        check_budget(started)?;
        namespace_directory
            .revalidate()
            .map_err(|_| "retained direct call namespace changed")?;
        let call_name = format!("call-{slot:04}");
        let call_path = calls_path.join(&call_name);
        let call_directory = PrivateNativeCaptureDirectory::open(&call_path)
            .map_err(|_| "retained direct call directory is not private")?;
        let expected_files = CALL_DIRECTORY_FILES
            .iter()
            .map(OsString::from)
            .collect::<BTreeSet<_>>();
        let actual_files =
            directory_entry_names_nofollow_bounded(&call_path, CALL_DIRECTORY_FILES.len())
                .map_err(|_| "cannot enumerate retained direct call files")?;
        if actual_files.len() != expected_files.len()
            || actual_files.into_iter().collect::<BTreeSet<_>>() != expected_files
        {
            return Err("retained direct call directory has an unexpected file set");
        }

        let invocation_path = call_path.join("invocation.json");
        let bytes =
            read_bytes(&invocation_path).map_err(|_| "cannot read retained native invocation")?;
        if bytes.is_empty() || bytes.len() > MAX_INVOCATION_BYTES {
            return Err("retained native invocation exceeds its byte bound");
        }
        invocation_bytes_total = invocation_bytes_total
            .checked_add(bytes.len())
            .filter(|total| *total <= MAX_BATCH_INVOCATION_BYTES)
            .ok_or("retained native invocation batch exceeds its byte bound")?;
        let digest = sha256_bytes(&bytes);
        let selected_bytes = call_directory
            .read_selected(OsStr::new("invocation.json"), &digest, MAX_INVOCATION_BYTES)
            .map_err(|_| "retained native invocation is not a private bounded file")?;
        if selected_bytes != bytes {
            return Err("retained native invocation changed while being read");
        }
        let invocation = NativeMakeInvocation::parse(&bytes)
            .map_err(|_| "retained native invocation is malformed")?;
        if prefix
            .assignments()
            .iter()
            .any(|(name, value)| invocation.environment().get(name) != Some(value))
        {
            return Err("retained native invocation differs from its selected Source prefix");
        }
        let cwd = invocation.working_directory();
        let canonical_cwd = fs::canonicalize(cwd)
            .map_err(|_| "retained native invocation working directory is unavailable")?;
        if !cwd.is_absolute()
            || canonical_cwd != cwd
            || !canonical_cwd.starts_with(&build)
            || !canonical_cwd.is_dir()
        {
            return Err("retained native invocation is outside the original build tree");
        }
        call_directory
            .revalidate()
            .map_err(|_| "retained direct call directory changed")?;
        invocations.push(invocation);
    }
    if invocations.len() != maximum_calls {
        return Err("retained native invocation count differs from the exact slot set");
    }
    collection_directory
        .revalidate()
        .map_err(|_| "retained direct collection directory changed")?;
    check_budget(started)?;

    let preparation_path = case_parent.join("native-direct-preparation.json");
    let preparation_bytes =
        read_bytes(&preparation_path).map_err(|_| "cannot read retained direct preparation")?;
    let preparation: Value = serde_json::from_slice(&preparation_bytes)
        .map_err(|_| "retained direct preparation JSON is malformed")?;
    if preparation["qualification"].as_str()
        != Some("original-owner-direct-preparation-not-execution-or-sdk")
        || required_strings(&preparation, "roots")? != roots
    {
        return Err("retained direct preparation does not match the explicit Source roots");
    }
    let retained_selection = preparation
        .get("source_metadata")
        .and_then(|value| value.get("selection"))
        .ok_or("retained direct preparation has no Source metadata selection")?;
    let retained_policy_sha256 = required_string(retained_selection, "policy_sha256")?;
    let retained_pair_count = selected_pair_count(retained_selection)?;
    if retained_pair_count != maximum_calls {
        return Err("retained direct preparation owner-goal pair count differs from the case");
    }

    let target_config = TargetProfile::load_config(&source.join("aros-targets.toml"))
        .map_err(|_| "cannot load Source target profiles")?;
    let target = target_config
        .targets
        .iter()
        .find(|target| target.name == profile_name)
        .ok_or("selected Source target profile is absent")?;
    let source_profile = crate::compatibility::SourceMesaPreflightProfile::load(&source, target)
        .map_err(|_| "cannot load selected Source target profile")?;

    // Reconstruct only the expected policy project from sealed Source inputs.
    // This is independent of the retained native-direct preparation object.
    let contract_path = target
        .sdk_consumer_contract
        .as_deref()
        .ok_or("selected Source target lacks an SDK consumer contract")?;
    let contract: Value = serde_json::from_slice(
        &read_bytes(&source.join(contract_path))
            .map_err(|_| "cannot read selected Source consumer contract")?,
    )
    .map_err(|_| "selected Source consumer contract is malformed")?;
    let projection_path = contract["metamake_projection"]
        .as_str()
        .ok_or("selected Source contract has no MetaMake projection")?;
    let policy: Value = serde_json::from_slice(
        &read_bytes(&source.join(projection_path))
            .map_err(|_| "cannot read selected Source MetaMake policy")?,
    )
    .map_err(|_| "selected Source MetaMake policy is malformed")?;
    let configuration_source = policy["configuration_source"]
        .as_str()
        .ok_or("selected Source policy has no configuration source")?;
    let mut projected_configuration = String::from_utf8(
        read_bytes(&source.join(configuration_source))
            .map_err(|_| "cannot read selected Source configuration")?,
    )
    .map_err(|_| "selected Source configuration is not UTF-8")?;
    for substitution in policy["substitutions"]
        .as_array()
        .ok_or("selected Source policy substitutions are malformed")?
    {
        let token = substitution["name"]
            .as_str()
            .ok_or("selected Source policy substitution name is malformed")?;
        let value = substitution["value"]
            .as_str()
            .ok_or("selected Source policy substitution value is malformed")?;
        if !projected_configuration.contains(token) {
            return Err("selected Source configuration lacks a sealed substitution");
        }
        projected_configuration = projected_configuration.replace(token, value);
    }
    let expected_project_name = policy["project"]
        .as_str()
        .ok_or("selected Source policy has no project name")?;
    let expected_project =
        ConfiguredProject::parse(&projected_configuration, expected_project_name)
            .map_err(|_| "cannot parse selected Source policy project")?;
    if expected_project.name != project_name {
        return Err("Source policy project differs from the retained direct selection");
    }

    let discovered = discover_meta_inputs(
        &source,
        &expected_project.ignored_directories,
        &BTreeSet::new(),
    )
    .map_err(|_| "cannot independently discover Source MetaMake inputs")?;
    let logical_files = discovered
        .into_keys()
        .map(|logical| source.join(logical))
        .collect::<Vec<_>>();
    let projection = source_profile
        .native_owner_projection(&logical_files, host, &BTreeSet::new(), None)
        .map_err(|_| "cannot independently derive Source native owner projection")?;
    if projection.policy_sha256() != retained_policy_sha256 {
        return Err("derived Source policy digest differs from retained preparation");
    }
    let metadata_selection = projection
        .select_source_metadata(&source, &roots)
        .map_err(|_| "cannot select Source owner-goal pairs")?;
    let derived_pair_count = metadata_selection
        .selection()
        .selected_owner_targets
        .values()
        .try_fold(0usize, |total, targets| total.checked_add(targets.len()))
        .ok_or("derived Source owner-goal pair count overflowed")?;
    if derived_pair_count != maximum_calls {
        return Err("derived Source owner-goal pair count differs from the 551-call case");
    }
    check_budget(started)?;

    let configuration_path = build.join("mmake.config");
    let actual_configuration_bytes =
        read_bytes(&configuration_path).map_err(|_| "cannot read original mmake.config")?;
    let actual_project = ConfiguredProject::parse_only_project(
        std::str::from_utf8(&actual_configuration_bytes)
            .map_err(|_| "original mmake.config is not UTF-8")?,
        project_name,
    )
    .map_err(|_| "cannot parse selected project from original mmake.config")?;
    let coordinate_data_count = compare_coordinate_data(
        &projection,
        &source,
        &roots,
        &build,
        &actual_project.default_makefile,
        &invocations,
    )?;
    check_budget(started)?;

    let mut positive_comparisons = 0usize;
    let positive_result = projection.with_runtime_configuration_checks(&source, |check| {
        for invocation in &invocations {
            check_budget(started).map_err(str::to_owned)?;
            let globals = read_global_inputs(
                &actual_project,
                invocation.environment().clone(),
                &read_inputs,
                &build,
                &source,
            )
            .map_err(|_| "cannot read retained invocation global inputs".to_owned())?;
            check.compare(&actual_project, &globals).map_err(|_| {
                "retained runtime configuration differs from Source policy".to_owned()
            })?;
            positive_comparisons += 1;
        }
        Ok(())
    });
    if positive_result.is_err() || positive_comparisons != maximum_calls {
        return Err("positive retained runtime batch did not compare every call successfully");
    }
    check_budget(started)?;

    // A second Source transaction deliberately alters only an in-memory copy
    // of actual_project, and only for the final call. The comparison must fail.
    let mut altered_project = actual_project.clone();
    altered_project
        .default_target
        .push_str("-retained-runtime-batch-negative");
    let mut negative_comparisons = 0usize;
    let mut controlled_rejection = false;
    let negative_result = projection.with_runtime_configuration_checks(&source, |check| {
        for (index, invocation) in invocations.iter().enumerate() {
            check_budget(started).map_err(str::to_owned)?;
            let globals = read_global_inputs(
                &actual_project,
                invocation.environment().clone(),
                &read_inputs,
                &build,
                &source,
            )
            .map_err(|_| "cannot read retained invocation global inputs".to_owned())?;
            let is_last = index + 1 == invocations.len();
            let candidate = if is_last {
                &altered_project
            } else {
                &actual_project
            };
            let comparison = check.compare(candidate, &globals);
            negative_comparisons += 1;
            if is_last {
                controlled_rejection = comparison
                    == Err("runtime owner project metadata differs from Source policy".into());
                return Err(if controlled_rejection {
                    "controlled in-memory default_target change was rejected".to_owned()
                } else {
                    "controlled in-memory default_target change was unexpectedly accepted"
                        .to_owned()
                });
            }
            comparison.map_err(|_| {
                "negative batch differed before its controlled final comparison".to_owned()
            })?;
        }
        Err("negative batch did not reach its final retained invocation".to_owned())
    });
    if negative_result
        != Err::<(), String>("controlled in-memory default_target change was rejected".into())
        || !controlled_rejection
        || negative_comparisons != maximum_calls
    {
        return Err("controlled negative batch did not reject on its final comparison");
    }
    check_budget(started)?;

    if read_bytes(&configuration_path).map_err(|_| "cannot recheck original mmake.config")?
        != actual_configuration_bytes
    {
        return Err("controlled negative changed the original mmake.config file");
    }
    namespace_directory
        .revalidate()
        .map_err(|_| "retained direct call namespace changed after comparison")?;

    let report_path =
        PathBuf::from(std::env::var_os(REPORT_ENV).ok_or("fresh private report path is absent")?);
    if !report_path.is_absolute() {
        return Err("runtime batch report path is not absolute");
    }
    let report_parent = report_path
        .parent()
        .ok_or("runtime batch report path has no parent")?;
    if report_parent
        .canonicalize()
        .map_err(|_| "runtime batch report parent is unavailable")?
        != report_parent
    {
        return Err("runtime batch report parent is not canonical");
    }
    let report_directory = PrivateNativeCaptureDirectory::open(report_parent)
        .map_err(|_| "runtime batch report parent is not a private directory")?;
    for retained_root in [
        source.as_path(),
        output.as_path(),
        compiler.as_path(),
        collection_path.as_path(),
        case_parent,
    ] {
        if aros_common::publication::filesystem_paths_overlap(report_parent, retained_root)
            .map_err(|_| "cannot compare runtime probe report scope")?
        {
            return Err("runtime probe report overlaps original input or evidence");
        }
    }
    let report_name = report_path
        .file_name()
        .ok_or("runtime batch report path has no filename")?;
    let elapsed_ms = started.elapsed().as_millis();
    let report = serde_json::json!({
        "qualification": "retained-runtime-policy-data-not-original-owner-or-sdk",
        "call_count": maximum_calls,
        "source_policy_sha256": projection.policy_sha256(),
        "coordinate_data": {
            "matched_calls": coordinate_data_count,
            "omitted_last_call_rejected": true,
            "qualification": "current-source-retained-invocation-consistency-not-live-owner",
        },
        "elapsed_ms": elapsed_ms,
        "diagnostic_budget_seconds": DIAGNOSTIC_BUDGET.as_secs(),
        "positive": {
            "result": "pass",
            "comparisons": positive_comparisons,
        },
        "controlled_counter": {
            "result": "rejected",
            "comparisons_attempted": negative_comparisons,
            "rejected_on_last_comparison": true,
            "in_memory_field": "default_target",
            "on_disk_configuration_unchanged": true,
        },
    });
    let report_bytes =
        serde_json::to_vec_pretty(&report).map_err(|_| "cannot encode runtime batch report")?;
    report_directory
        .write_new(report_name, &report_bytes)
        .map_err(|_| "cannot create fresh private runtime batch report")?;
    report_directory
        .revalidate()
        .map_err(|_| "runtime batch report directory changed")?;
    Ok(())
}

fn compare_coordinate_data(
    projection: &NativeOwnerProjection,
    source: &Path,
    roots: &[String],
    build: &Path,
    makefile: &str,
    invocations: &[NativeMakeInvocation],
) -> Result<usize, &'static str> {
    let owners = invocations
        .iter()
        .map(|invocation| {
            invocation
                .working_directory()
                .strip_prefix(build)
                .map_err(|_| "retained coordinate escaped original build")?
                .join(makefile)
                .to_str()
                .map(str::to_owned)
                .ok_or("retained owner coordinate is not UTF-8")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let calls = owners
        .iter()
        .zip(invocations)
        .map(|(owner, invocation)| (owner.as_str(), invocation))
        .collect::<Vec<_>>();
    let matched = projection
        .match_complete_child_coordinates(source, roots, build, &calls)
        .map_err(|_| "retained coordinate data differs from selected Source coverage")?;
    let partial = projection.match_complete_child_coordinates(
        source,
        roots,
        build,
        &calls[..calls.len() - 1],
    );
    if !partial
        .is_err_and(|error| error == "child coordinate coverage omits a selected owner target")
    {
        return Err("omitted-call coordinate counter did not fail for the expected reason");
    }
    Ok(matched.len())
}

fn canonical_case_path(case: &Value, key: &str) -> Result<PathBuf, &'static str> {
    let path = PathBuf::from(
        case.get(key)
            .and_then(Value::as_str)
            .ok_or("retained case path is absent")?,
    );
    if !path.is_absolute() {
        return Err("retained case path is not absolute");
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "retained case directory is unavailable")?;
    if canonical != path || !canonical.is_dir() {
        return Err("retained case directory is not canonical");
    }
    Ok(canonical)
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, &'static str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or("retained case field is absent or malformed")
}

fn required_strings(value: &Value, key: &str) -> Result<Vec<String>, &'static str> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or("retained case string list is absent or malformed")?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or("retained case string list contains a malformed value")
        })
        .collect()
}

fn selected_pair_count(selection: &Value) -> Result<usize, &'static str> {
    selection
        .get("selected_owner_targets")
        .and_then(Value::as_object)
        .ok_or("retained Source selection has no owner-goal map")?
        .values()
        .try_fold(0usize, |total, targets| {
            let count = targets
                .as_array()
                .ok_or("retained Source owner goals are malformed")?
                .len();
            total
                .checked_add(count)
                .ok_or("retained Source owner-goal pair count overflowed")
        })
}

fn expected_slot_names(count: usize) -> Result<BTreeSet<OsString>, &'static str> {
    if count == 0 || count > MAX_DIAGNOSTIC_CALLS {
        return Err("retained call count is outside its diagnostic bound");
    }
    let mut names = BTreeSet::new();
    for slot in 0..count {
        let call_name = format!("call-{slot:04}");
        names.insert(OsString::from(&call_name));
        names.insert(OsString::from(format!("{call_name}.claim")));
    }
    Ok(names)
}

fn check_budget(started: Instant) -> Result<(), &'static str> {
    if started.elapsed() >= DIAGNOSTIC_BUDGET {
        Err("fresh retained runtime diagnostic budget expired")
    } else {
        Ok(())
    }
}
