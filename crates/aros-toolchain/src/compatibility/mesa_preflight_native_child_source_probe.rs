//! Bounded, data-only diagnosis of retained native children through Source.
//!
//! By default this uses the public retained-child batch API against call 0 from
//! an explicitly selected bounded collection. An additional opt-in requests
//! all retained children in one batch. A separate opt-in consumes children
//! sequentially and projects only a checked count, not a productive graph.
//! Its transient limit is this case's original vector admission ceiling.
//! None of these modes recovers the original
//! preparation owner or deadline, launches Make/compiler/Source, or qualifies
//! an SDK or original-owner execution.

#![cfg(all(test, unix))]

use crate::compatibility::SourceMesaPreflightProfile;
use aros_common::{
    directory_entry_names_nofollow_bounded,
    local_source::LocalSourceIdentity,
    make_input_snapshot::{MakeInputBatchLimits, MakeInputRootSelection, MakeInputSnapshot},
    make_native_invocation::{NativeMakeInvocation, PrivateNativeCaptureDirectory},
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    measure_regular_file_bounded, measure_tree_content_cas_bounded,
    publication::filesystem_paths_overlap,
    sha256_bytes, Sha256Digest, TargetProfile, TreeTraversalLimits,
};
use aros_transpiler::{
    dirs::DirVars,
    make_vars::MakeProgramPaths,
    metamake_project::ConfiguredProject,
    native_owner_projection::{
        discover_meta_inputs, MatchedChildSource, NativeOwnerProjection,
        RetainedChildSourceBatchLimits, RetainedChildSourceBatchRequest,
        RetainedChildSourceFoldLimits,
    },
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

const CASE_ENV: &str = "AROS_TEST_MESA_PREFLIGHT_CASE";
const REPORT_ENV: &str = "AROS_TEST_NATIVE_CHILD_SOURCE_REPORT";
const ALL_CALLS_ENV: &str = "AROS_TEST_NATIVE_CHILD_SOURCE_ALL";
const FOLD_ENV: &str = "AROS_TEST_NATIVE_CHILD_SOURCE_FOLD";
const FOLD_PROJECTED_BYTES: usize = 8 * 1024 * 1024;
const QUALIFICATION: &str = "retained-child-source-current-build-data-not-original-owner-or-sdk";
const CURRENT_BUILD_BINDING: &str =
    "fresh-configure-tree-data-consistency; retained-Source-and-compiler-root-digests";
const COLLECTION_QUALIFICATION: &str =
    "direct-metamake-first-input-and-source-coverage-not-leaf-or-sdk";
const PREPARATION_QUALIFICATION: &str = "original-owner-direct-preparation-not-execution-or-sdk";
const MAX_CALLS: usize = 4096;
const MAX_CONTROL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_INVOCATION_BYTES: usize = 64 * 1024;
const MAX_STDOUT_BYTES: usize = 256 * 1024;
const MAX_OBSERVATION_BYTES: usize = 4 * 1024 * 1024;
const MAX_BATCH_RETAINED_BYTES: usize = 512 * 1024 * 1024;
const MAX_SENSITIVE_VALUE_COUNT: usize = 16_384;
const MAX_SENSITIVE_VALUE_BYTES: usize = 8 * 1024 * 1024;
const MAX_REASON_BYTES: usize = 4096;
const MAX_TREE_ENTRIES: usize = 100_000;
const MAX_TREE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const SINGLE_CALL_BUDGET: Duration = Duration::from_secs(300);
const ALL_CALLS_BUDGET: Duration = Duration::from_secs(600);
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

#[derive(Serialize)]
struct DiagnosticReport {
    refused_child_index: Option<usize>,
    /// Calls submitted to the batch API, not a claim that all were interpreted.
    submitted_calls: usize,
    fold_working_refusal: Option<[usize; 3]>,
    sequential_fold: bool,
    fold_limits_bytes: Option<[usize; 2]>,
    fold_result_bytes: Option<[usize; 2]>,
    qualification: &'static str,
    current_build_binding: &'static str,
    status: &'static str,
    selected_call_index: Option<usize>,
    requested_calls: usize,
    attempted_calls: usize,
    redaction_value_count: usize,
    redaction_value_bytes: usize,
    retained_call_count: Option<usize>,
    source_policy_sha256: Option<Sha256Digest>,
    refusal_reason: Option<String>,
    refusal_reason_sha256: Option<Sha256Digest>,
    refusal_reason_redacted: bool,
    retained_storage_refusal: Option<RetainedStorageRefusal>,
    elapsed_ms: u128,
    diagnostic_budget_seconds: u64,
}

struct ProbeState {
    sequential_fold: bool,
    fold_result_bytes: Option<[usize; 2]>,
    all_calls: bool,
    diagnostic_budget: Duration,
    requested_calls: usize,
    attempted_calls: usize,
    retained_call_count: Option<usize>,
    source_policy_sha256: Option<Sha256Digest>,
    retained_storage_limit: Option<usize>,
    sensitive_strings: SensitiveStrings,
}

/// Checked numeric reader accounting only, never raw error or input values.
/// These conservative estimates are not allocator sizes, RSS or authentication.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct RetainedStorageRefusal {
    child_index: usize,
    prior_estimate_bytes: usize,
    variable_estimate_bytes: usize,
    file_estimate_bytes: usize,
    next_estimate_bytes: usize,
    limit_bytes: usize,
}

impl RetainedStorageRefusal {
    fn parse(reason: &str, calls: usize, expected_limit: usize) -> Option<Self> {
        let decimal = |text: &str| {
            (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| text.parse::<usize>().ok())
                .flatten()
        };
        let (index, rest) = reason
            .strip_prefix("retained child source batch at index ")?
            .split_once(": retained-data limit exceeded (prior=")?;
        let (prior, rest) = rest.split_once(", variable=")?;
        let (variable, rest) = rest.split_once(", file=")?;
        let (file, rest) = rest.split_once(", next=")?;
        let (next, limit) = rest.split_once(", limit=")?;
        let value = Self {
            child_index: decimal(index)?,
            prior_estimate_bytes: decimal(prior)?,
            variable_estimate_bytes: decimal(variable)?,
            file_estimate_bytes: decimal(file)?,
            next_estimate_bytes: decimal(next)?,
            limit_bytes: decimal(limit.strip_suffix(')')?)?,
        };
        let hard_limit = 1024 * 1024 * 1024;
        if calls == 0
            || calls > MAX_CALLS
            || value.child_index >= calls
            || expected_limit == 0
            || expected_limit > hard_limit
            || value.limit_bytes != expected_limit
            || value.prior_estimate_bytes > value.limit_bytes
            || (value.child_index == 0 && value.prior_estimate_bytes != 0)
            || value.variable_estimate_bytes > hard_limit
            || value.file_estimate_bytes > hard_limit
            || value.next_estimate_bytes <= value.limit_bytes
            || value
                .variable_estimate_bytes
                .checked_add(value.file_estimate_bytes)?
                .checked_add(value.prior_estimate_bytes)?
                != value.next_estimate_bytes
        {
            return None;
        }
        Some(value)
    }
}

struct RetainedChildCall {
    generated_owner: String,
    source_input: String,
    goals: Vec<String>,
    invocation: NativeMakeInvocation,
    observation: RetainedMakeObservation,
}

#[derive(Default)]
struct RetainedReadTotals {
    bytes: usize,
}

struct SensitiveStrings {
    values: BTreeSet<String>,
    bytes: usize,
    maximum_values: usize,
    maximum_bytes: usize,
}

impl SensitiveStrings {
    const fn with_limits(maximum_values: usize, maximum_bytes: usize) -> Self {
        Self {
            values: BTreeSet::new(),
            bytes: 0,
            maximum_values,
            maximum_bytes,
        }
    }

    fn insert(&mut self, value: &str) -> Result<(), String> {
        if value.trim().is_empty() || self.values.contains(value) {
            return Ok(());
        }
        let next_count = self
            .values
            .len()
            .checked_add(1)
            .ok_or("redaction value count overflowed")?;
        let next_bytes = self
            .bytes
            .checked_add(value.len())
            .ok_or("redaction value byte count overflowed")?;
        if next_count > self.maximum_values || next_bytes > self.maximum_bytes {
            return Err("redaction coverage exceeds the bounded diagnostic limit".into());
        }
        self.values.insert(value.to_owned());
        self.bytes = next_bytes;
        Ok(())
    }

    const fn byte_count(&self) -> usize {
        self.bytes
    }

    fn value_count(&self) -> usize {
        self.values.len()
    }
}

impl Default for SensitiveStrings {
    fn default() -> Self {
        Self::with_limits(MAX_SENSITIVE_VALUE_COUNT, MAX_SENSITIVE_VALUE_BYTES)
    }
}

impl RetainedReadTotals {
    fn account(&mut self, bytes: usize) -> Result<(), String> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or("retained child evidence byte count overflowed")?;
        if self.bytes > MAX_BATCH_RETAINED_BYTES {
            return Err(
                "retained child evidence exceeds the aggregate diagnostic byte bound".into(),
            );
        }
        Ok(())
    }
}

#[test]
#[ignore = "requires an explicit retained bounded-call case and fresh private report path; data-only"]
fn actual_retained_child_source_batch_failure_probe() {
    let report_path = std::env::var_os(REPORT_ENV).map_or_else(
        || panic!("{REPORT_ENV} must name a fresh private report file"),
        PathBuf::from,
    );
    let started = Instant::now();
    let mut state = ProbeState {
        sequential_fold: false,
        fold_result_bytes: None,
        all_calls: false,
        diagnostic_budget: SINGLE_CALL_BUDGET,
        requested_calls: 0,
        attempted_calls: 0,
        retained_call_count: None,
        source_policy_sha256: None,
        retained_storage_limit: None,
        sensitive_strings: SensitiveStrings::default(),
    };
    let result = run_probe(&mut state, started);
    let retained_storage_refusal = result.as_ref().err().and_then(|reason| {
        RetainedStorageRefusal::parse(reason, state.attempted_calls, state.retained_storage_limit?)
    });
    let fold_working_refusal = result.as_ref().err().and_then(|reason| {
        state
            .sequential_fold
            .then(|| {
                parse_fold_working_refusal(
                    reason,
                    state.attempted_calls,
                    state.retained_storage_limit?,
                )
            })
            .flatten()
    });
    let refused_child_index = result
        .as_ref()
        .err()
        .and_then(|reason| parse_refused_child_index(reason, state.attempted_calls));
    let (status, refusal_reason, refusal_reason_sha256, refusal_reason_redacted) = match result {
        Ok(()) => ("accepted-requested-call-data", None, None, false),
        Err(reason) => {
            let reason_sha256 = sha256_bytes(reason.as_bytes());
            let (safe_reason, redacted) = safe_reason(&reason, &state.sensitive_strings);
            ("refused", Some(safe_reason), Some(reason_sha256), redacted)
        }
    };
    let report = DiagnosticReport {
        refused_child_index,
        submitted_calls: state.attempted_calls,
        fold_working_refusal,
        sequential_fold: state.sequential_fold,
        fold_limits_bytes: state
            .sequential_fold
            .then_some(state.retained_storage_limit)
            .flatten()
            .map(|limit| [limit, FOLD_PROJECTED_BYTES]),
        fold_result_bytes: state.fold_result_bytes,
        qualification: QUALIFICATION,
        current_build_binding: CURRENT_BUILD_BINDING,
        status,
        selected_call_index: (!state.all_calls && state.requested_calls == 1).then_some(0),
        requested_calls: state.requested_calls,
        attempted_calls: state.attempted_calls,
        redaction_value_count: state.sensitive_strings.value_count(),
        redaction_value_bytes: state.sensitive_strings.byte_count(),
        retained_call_count: state.retained_call_count,
        source_policy_sha256: state.source_policy_sha256,
        refusal_reason,
        refusal_reason_sha256,
        refusal_reason_redacted,
        retained_storage_refusal,
        elapsed_ms: started.elapsed().as_millis(),
        diagnostic_budget_seconds: state.diagnostic_budget.as_secs(),
    };
    write_report(&report_path, &report)
        .unwrap_or_else(|_| panic!("cannot create fresh private child-source report"));
    println!(
        "qualification={QUALIFICATION} status={status} requested_calls={} attempted_calls={}",
        report.requested_calls, report.attempted_calls
    );
}

// Only checked scalar interpreter estimates; never expose raw Source errors.
fn parse_refused_child_index(reason: &str, calls: usize) -> Option<usize> {
    let text = [
        "retained child source batch at index ",
        "retained child source fold at index ",
    ]
    .into_iter()
    .find_map(|prefix| reason.strip_prefix(prefix))?;
    let (index, _) = text.split_once(':')?;
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let index = index.parse::<usize>().ok()?;
    (calls > 0 && calls <= MAX_CALLS && index < calls).then_some(index)
}

#[test]
fn refusal_index_is_a_bounded_scalar_without_source_text() {
    for prefix in [
        "retained child source batch at index ",
        "retained child source fold at index ",
    ] {
        assert_eq!(
            parse_refused_child_index(&format!("{prefix}206: private text"), 550),
            Some(206)
        );
        for value in ["550", "-1", "+1", "", "99999999999999999999999999999999"] {
            assert_eq!(
                parse_refused_child_index(&format!("{prefix}{value}: private text"), 550),
                None
            );
        }
    }
    assert_eq!(
        parse_refused_child_index("other 206: private text", 550),
        None
    );
    assert_eq!(
        parse_refused_child_index("retained child source batch at index 0: private", 0),
        None
    );
}

fn parse_fold_working_refusal(
    reason: &str,
    calls: usize,
    expected_limit: usize,
) -> Option<[usize; 3]> {
    let (index, rest) = reason
        .strip_prefix("retained child source fold at index ")?
        .split_once(": working child limit exceeded (estimate=")?;
    let (estimate, limit) = rest.strip_suffix(')')?.split_once(", limit=")?;
    let decimal = |text: &str| {
        (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse::<usize>().ok())
            .flatten()
    };
    let value = [decimal(index)?, decimal(estimate)?, decimal(limit)?];
    (calls > 0
        && calls <= MAX_CALLS
        && value[0] < calls
        && expected_limit > 0
        && expected_limit <= 1024 * 1024 * 1024
        && value[2] == expected_limit
        && value[1] > value[2])
        .then_some(value)
}

#[test]
fn fold_working_refusal_reports_only_exact_bounded_scalar_grammar() {
    const LIMIT: usize = 16 * 1024 * 1024;
    let estimate = LIMIT + 1;
    let valid = format!(
        "retained child source fold at index 148: working child limit exceeded (estimate={estimate}, limit={LIMIT})"
    );
    assert_eq!(
        parse_fold_working_refusal(&valid, 550, LIMIT),
        Some([148, estimate, LIMIT])
    );
    assert_eq!(parse_fold_working_refusal(&valid, 148, LIMIT), None);
    assert_eq!(
        parse_fold_working_refusal(&valid, MAX_CALLS + 1, LIMIT),
        None
    );
    assert_eq!(parse_fold_working_refusal(&valid, 550, 0), None);
    assert_eq!(
        parse_fold_working_refusal(&valid, 550, 1024 * 1024 * 1024 + 1),
        None
    );
    for invalid in [
        valid.replace(&format!("estimate={estimate}"), "estimate=16"),
        valid.replace(&format!("limit={LIMIT}"), "limit=1"),
        valid.replace("148:", "-1:"),
        format!("{valid} extra"),
        valid.replace(
            &format!("estimate={estimate}"),
            "estimate=999999999999999999999999999999999",
        ),
    ] {
        assert_eq!(parse_fold_working_refusal(&invalid, 550, LIMIT), None);
    }
}

fn run_probe(state: &mut ProbeState, started: Instant) -> Result<(), String> {
    state.sequential_fold = match std::env::var_os(FOLD_ENV) {
        None => false,
        Some(value) if value == "1" => true,
        Some(_) => return Err("fold opt-in must be exactly 1 when present".into()),
    };
    state.all_calls = match std::env::var_os(ALL_CALLS_ENV) {
        None => false,
        Some(value) if value == "1" => true,
        Some(_) => return Err("all-call opt-in must be exactly 1 when present".into()),
    };
    state.diagnostic_budget = if state.all_calls {
        ALL_CALLS_BUDGET
    } else {
        SINGLE_CALL_BUDGET
    };
    check_budget(started, state.diagnostic_budget)?;
    let case_path =
        PathBuf::from(std::env::var_os(CASE_ENV).ok_or("explicit retained case path is absent")?);
    let case_path = canonical_file(&case_path, "retained case path")?;
    let case_parent = case_path
        .parent()
        .ok_or("retained case path has no parent")?
        .canonicalize()
        .map_err(|_| "retained case parent is unavailable")?;
    let case: Value = read_json(&case_path, "retained case JSON")?;
    let source = canonical_directory(&case_path_value(&case, "source_root")?, "Source root")?;
    let output = canonical_directory(&case_path_value(&case, "output_root")?, "output root")?;
    let compiler = canonical_directory(&case_path_value(&case, "compiler_root")?, "compiler root")?;
    let local_source_preset = selected_local_source_preset(&case)?;
    let build = canonical_directory(&output.join("configure"), "configured build root")?;
    check_budget(started, state.diagnostic_budget)?;

    let dispatch = case
        .get("native_dispatch")
        .ok_or("retained case has no native dispatch selection")?;
    let direct = dispatch
        .get("direct_collection")
        .ok_or("retained case has no direct collection selection")?;
    let project_name = required_string(direct, "project")?;
    let host = required_string(direct, "host")?;
    let profile_name = required_string(direct, "source_profile")?;
    let roots = required_strings(direct, "roots")?;
    if roots.is_empty() || roots.len() > 64 {
        return Err("retained direct Source roots are outside the diagnostic bound".into());
    }
    let case_maximum_calls = required_usize(direct, "maximum_calls")?;
    if case_maximum_calls == 0 || case_maximum_calls > MAX_CALLS {
        return Err("retained direct collection call bound is outside the diagnostic limit".into());
    }
    let limits = retained_batch_limits(
        direct
            .get("read_children")
            .ok_or("retained case has no child-read limits")?,
    )?;
    state.retained_storage_limit = Some(limits.maximum_retained_bytes);
    if limits.inputs.maximum_children != case_maximum_calls {
        return Err("retained child-read limit differs from the case call bound".into());
    }

    let collection_path = canonical_directory(
        Path::new(required_string(direct, "output_root")?),
        "retained direct collection directory",
    )?;
    let collection_directory = PrivateNativeCaptureDirectory::open(&collection_path)
        .map_err(|_| "retained direct collection directory is not private")?;
    let calls_path = collection_path.join("d");
    let namespace_directory = PrivateNativeCaptureDirectory::open(&calls_path)
        .map_err(|_| "retained direct call namespace is not private")?;
    let collection_report: Value = read_json(
        &case_parent.join("native-direct-collection.json"),
        "retained direct collection report",
    )?;
    let invocations = collection_report
        .get("invocations")
        .and_then(Value::as_array)
        .ok_or("retained direct collection report has no invocations")?;
    let coordinates = collection_report
        .get("coordinates")
        .and_then(Value::as_array)
        .ok_or("retained direct collection report has no coordinates")?;
    let retained_call_count =
        retained_collection_call_count(&collection_report, case_maximum_calls)?;
    state.retained_call_count = Some(retained_call_count);
    if limits.inputs.maximum_children < retained_call_count {
        return Err("retained child-read limit is below the actual collection count".into());
    }
    state.requested_calls = if state.all_calls {
        retained_call_count
    } else {
        1
    };
    if collection_report["qualification"].as_str() != Some(COLLECTION_QUALIFICATION)
        || collection_report["evidence"].as_str() != Some(collection_path.to_str().unwrap_or(""))
        || required_strings(&collection_report, "roots")? != roots
        || invocations.len() != retained_call_count
        || coordinates.len() != retained_call_count
    {
        return Err("retained collection report differs from the bounded case selection".into());
    }
    let expected_slots = expected_slot_names(retained_call_count)?;
    let slot_names = directory_entry_names_nofollow_bounded(
        &calls_path,
        retained_call_count
            .checked_mul(2)
            .ok_or("retained call namespace bound overflowed")?,
    )
    .map_err(|_| "cannot enumerate retained direct call namespace")?;
    if slot_names.len() != expected_slots.len()
        || slot_names.into_iter().collect::<BTreeSet<_>>() != expected_slots
    {
        return Err("retained direct call namespace is not the exact bounded slot set".into());
    }
    check_budget(started, state.diagnostic_budget)?;
    let requested_calls = state.requested_calls;
    let mut read_totals = RetainedReadTotals::default();
    let mut retained_calls = Vec::with_capacity(requested_calls);
    for call_index in 0..requested_calls {
        check_budget(started, state.diagnostic_budget)?;
        retained_calls.push(
            load_retained_child_call(
                call_index,
                &calls_path,
                &build,
                &invocations[call_index],
                &coordinates[call_index],
                state,
                &mut read_totals,
            )
            .map_err(|reason| format!("retained call {call_index}: {reason}"))?,
        );
    }
    namespace_directory
        .revalidate()
        .map_err(|_| "retained direct call namespace changed while reading")?;
    collection_directory
        .revalidate()
        .map_err(|_| "retained direct collection directory changed while reading")?;
    check_budget(started, state.diagnostic_budget)?;

    let root_observation: Value = read_json(
        &case_parent.join("native-root-observation.json"),
        "retained root observation",
    )?;
    let source_sha256: Sha256Digest =
        serde_json::from_value(root_observation["identity"]["source_inputs_sha256"].clone())
            .map_err(|_| "retained root observation has no selected Source digest")?;
    let compiler_sha256: Sha256Digest =
        serde_json::from_value(root_observation["identity"]["compiler_tree_sha256"].clone())
            .map_err(|_| "retained root observation has no selected compiler digest")?;
    let local_source = local_source_preset
        .map(|preset| LocalSourceIdentity::capture(&source, preset))
        .transpose()
        .map_err(|_| "cannot capture the selected current LocalSource identity")?;
    if local_source
        .as_ref()
        .is_some_and(|identity| identity.content_sha256 != source_sha256)
    {
        return Err("current LocalSource inputs differ from the retained root digest".into());
    }

    let preparation: Value = read_json(
        &case_parent.join("native-direct-preparation.json"),
        "retained direct preparation",
    )?;
    if preparation["qualification"].as_str() != Some(PREPARATION_QUALIFICATION)
        || required_strings(&preparation, "roots")? != roots
    {
        return Err("retained direct preparation differs from the case roots".into());
    }
    let retained_selection = preparation
        .get("source_metadata")
        .and_then(|value| value.get("selection"))
        .ok_or("retained direct preparation has no Source selection")?;
    let retained_policy_sha256 = required_string(retained_selection, "policy_sha256")?;
    if selected_pair_count(retained_selection)? != retained_call_count {
        return Err(
            "retained Source owner-goal pair count differs from the collection count".into(),
        );
    }

    let (projection, source_policy_sha256, derived_pairs) = derive_source_projection(
        &source,
        profile_name,
        project_name,
        host,
        &roots,
        local_source.as_ref(),
        (started, state.diagnostic_budget),
    )?;
    state.source_policy_sha256 = Some(source_policy_sha256.clone());
    if source_policy_sha256.to_string() != retained_policy_sha256 {
        return Err("derived Source policy digest differs from retained preparation".into());
    }
    if derived_pairs != retained_call_count {
        return Err(
            "derived Source owner-goal pair count differs from the collection count".into(),
        );
    }
    check_budget(started, state.diagnostic_budget)?;

    let build_limits = TreeTraversalLimits::new(MAX_TREE_ENTRIES, MAX_TREE_BYTES)
        .map_err(|_| "invalid current build-tree diagnostic limits")?;
    let current_build_tree = measure_tree_content_cas_bounded(&build, build_limits)
        .map_err(|_| "cannot measure current configured build for data consistency")?;
    let current_build_sha256 = current_build_tree.payload_digest_excluding(None);
    check_budget(started, state.diagnostic_budget)?;
    let selected_roots = [
        MakeInputRootSelection {
            root: build.clone(),
            content_sha256: current_build_sha256,
        },
        MakeInputRootSelection {
            root: compiler,
            content_sha256: compiler_sha256,
        },
    ];
    let snapshot = local_source
        .as_ref()
        .map_or_else(
            || {
                MakeInputSnapshot::capture(
                    retained_calls[0].invocation.working_directory(),
                    &[
                        MakeInputRootSelection {
                            root: source.clone(),
                            content_sha256: source_sha256,
                        },
                        selected_roots[0].clone(),
                        selected_roots[1].clone(),
                    ],
                )
            },
            |identity| {
                MakeInputSnapshot::capture_with_local_source(
                    retained_calls[0].invocation.working_directory(),
                    &source,
                    identity,
                    &selected_roots,
                )
            },
        )
        .map_err(|_| "cannot capture the explicitly selected Source/build/compiler roots")?;
    check_budget(started, state.diagnostic_budget)?;

    let dirs = DirVars::default();
    let paths = retained_calls
        .iter()
        .map(|call| {
            let relative_dir = Path::new(&call.generated_owner)
                .parent()
                .ok_or("retained owner has no parent directory")?;
            Ok(MakeProgramPaths {
                dirs: &dirs,
                source_root: &source,
                relative_dir,
                top_directory: &build,
                working_directory: call.invocation.working_directory(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let requests = retained_calls
        .iter()
        .zip(&paths)
        .map(|(call, paths)| RetainedChildSourceBatchRequest {
            paths,
            generated_owner: &call.generated_owner,
            observation: &call.observation,
        })
        .collect::<Vec<_>>();
    state.attempted_calls = requests.len();
    if state.sequential_fold {
        let matched = projection.fold_retained_child_sources(
            &roots,
            &requests,
            &snapshot,
            RetainedChildSourceFoldLimits {
                inputs: limits.inputs,
                // Preserve this selected case's original individual admission ceiling.
                maximum_child_retained_bytes: limits.maximum_retained_bytes,
                maximum_projected_bytes: FOLD_PROJECTED_BYTES,
            },
            || check_budget(started, state.diagnostic_budget).map_err(str::to_owned),
            (
                0usize,
                |count: &mut usize, index, child: MatchedChildSource| {
                    validate_matched_coordinates(&child, index, &retained_calls[index])?;
                    *count = count.checked_add(1).ok_or("fold count overflow")?;
                    Ok(std::mem::size_of::<usize>())
                },
            ),
        )?;
        if matched.children() != retained_calls.len() {
            return Err("fold returned an unexpected child count".into());
        }
        state.fold_result_bytes = Some([
            matched.maximum_child_retained_bytes(),
            matched.projected_bytes(),
        ]);
        if matched.into_value() != retained_calls.len() {
            return Err("fold returned an unexpected projected count".into());
        }
        check_budget(started, state.diagnostic_budget)?;
        return Ok(());
    }
    let result =
        projection.read_retained_child_sources(&roots, &requests, &snapshot, limits, || {
            check_budget(started, state.diagnostic_budget).map_err(str::to_owned)
        });
    let matched = result?;
    if matched.len() != retained_calls.len() {
        return Err("public Source reader returned an unexpected batch result count".into());
    }
    for (call_index, (matched, call)) in matched.iter().zip(&retained_calls).enumerate() {
        validate_matched_coordinates(matched, call_index, call)?;
    }
    check_budget(started, state.diagnostic_budget)?;
    Ok(())
}

fn load_retained_child_call(
    call_index: usize,
    calls_path: &Path,
    build: &Path,
    retained_call: &Value,
    retained_coordinate: &Value,
    state: &mut ProbeState,
    read_totals: &mut RetainedReadTotals,
) -> Result<RetainedChildCall, String> {
    let generated_owner = required_string(retained_coordinate, "owner")?.to_owned();
    let source_input = required_string(retained_coordinate, "source")?.to_owned();
    let goals = required_strings(retained_coordinate, "goals")?;
    if generated_owner.is_empty() || source_input.is_empty() || goals.is_empty() {
        return Err("retained Source coordinates are empty".into());
    }

    let binding: MakeObservationBinding = serde_json::from_value(
        retained_call
            .get("binding")
            .ok_or("external observation binding is absent")?
            .clone(),
    )
    .map_err(|_| "external observation binding is malformed")?;
    if binding.phase != MakeObservationPhase::FirstInputPrelude {
        return Err("call is not a first-input observation".into());
    }

    let call_path = calls_path.join(format!("call-{call_index:04}"));
    let call_directory = PrivateNativeCaptureDirectory::open(&call_path)
        .map_err(|_| "call directory is not private")?;
    let expected_files = CALL_DIRECTORY_FILES
        .iter()
        .map(OsString::from)
        .collect::<BTreeSet<_>>();
    let actual_files =
        directory_entry_names_nofollow_bounded(&call_path, CALL_DIRECTORY_FILES.len())
            .map_err(|_| "cannot enumerate call files")?;
    if actual_files.len() != expected_files.len()
        || actual_files.into_iter().collect::<BTreeSet<_>>() != expected_files
    {
        return Err("call directory has an unexpected file set".into());
    }

    let invocation_bytes = call_directory
        .read_selected(
            OsStr::new("invocation.json"),
            &binding.invocation_sha256,
            MAX_INVOCATION_BYTES,
        )
        .map_err(|_| "invocation differs from its external binding")?;
    read_totals.account(invocation_bytes.len())?;
    let stdout_bytes = call_directory
        .read_selected(
            OsStr::new("initial.stdout"),
            &binding.stdout_sha256,
            MAX_STDOUT_BYTES,
        )
        .map_err(|_| "initial packet differs from its external binding")?;
    read_totals.account(stdout_bytes.len())?;
    let invocation =
        NativeMakeInvocation::parse(&invocation_bytes).map_err(|_| "invocation is malformed")?;
    for argument in invocation.arguments() {
        state.sensitive_strings.insert(argument)?;
    }
    if let Some(argument_zero) = invocation.argument_zero() {
        state.sensitive_strings.insert(argument_zero)?;
    }
    for value in invocation.environment().values() {
        state.sensitive_strings.insert(value)?;
    }
    let proof_cwd = required_string(retained_call, "cwd")?;
    let proof_argv0 = required_string(retained_call, "argv0")?;
    let proof_argv = required_strings(retained_call, "argv")?;
    if invocation.working_directory() != Path::new(proof_cwd)
        || invocation.argument_zero() != Some(proof_argv0)
        || invocation.arguments() != proof_argv.as_slice()
    {
        return Err("invocation differs from its collection-report binding".into());
    }
    let cwd = invocation.working_directory();
    let canonical_cwd = cwd
        .canonicalize()
        .map_err(|_| "working directory is unavailable")?;
    if !cwd.is_absolute() || canonical_cwd != cwd || !canonical_cwd.starts_with(build) {
        return Err("working directory is outside the configured build".into());
    }

    let selection_candidate = read_regular_file(
        &call_path.join("observation-selection.json"),
        MAX_OBSERVATION_BYTES as u64,
        "retained observation selection",
    )?;
    read_totals.account(selection_candidate.len())?;
    let selection_digest = sha256_bytes(&selection_candidate);
    let selection_bytes = call_directory
        .read_selected(
            OsStr::new("observation-selection.json"),
            &selection_digest,
            MAX_OBSERVATION_BYTES,
        )
        .map_err(|_| "observation selection is not a stable private file")?;
    read_totals.account(selection_bytes.len())?;
    if selection_bytes != selection_candidate {
        return Err("observation selection changed while being read".into());
    }
    let observation_selection: Value = serde_json::from_slice(&selection_bytes)
        .map_err(|_| "observation selection is malformed")?;
    let selected_observation_digest: Sha256Digest = serde_json::from_value(
        observation_selection
            .get("document_sha256")
            .ok_or("observation selection has no document digest")?
            .clone(),
    )
    .map_err(|_| "observation document digest is malformed")?;
    let selected_binding: MakeObservationBinding = serde_json::from_value(
        observation_selection
            .get("binding")
            .ok_or("observation selection has no binding")?
            .clone(),
    )
    .map_err(|_| "observation selection binding is malformed")?;
    if observation_selection["qualification"].as_str()
        != Some("instrumented-first-input-observation-only")
        || selected_binding != binding
    {
        return Err("observation selection differs from the collection binding".into());
    }
    let observation_bytes = call_directory
        .read_selected(
            OsStr::new("observation.json"),
            &selected_observation_digest,
            MAX_OBSERVATION_BYTES,
        )
        .map_err(|_| "observation document differs from its selection")?;
    read_totals.account(observation_bytes.len())?;
    let observation =
        RetainedMakeObservation::load(&observation_bytes, &selected_observation_digest, &binding)
            .map_err(|_| "observation differs from its external binding")?;
    if observation.invocation() != &invocation
        || observation.initial_stdout() != stdout_bytes.as_slice()
        || observation.binding() != &binding
    {
        return Err("observation payload differs from the retained invocation".into());
    }
    for line in std::str::from_utf8(&stdout_bytes)
        .unwrap_or("")
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        state.sensitive_strings.insert(line)?;
    }
    call_directory
        .revalidate()
        .map_err(|_| "call directory changed while reading")?;
    Ok(RetainedChildCall {
        generated_owner,
        source_input,
        goals,
        invocation,
        observation,
    })
}

fn derive_source_projection(
    source: &Path,
    profile_name: &str,
    project_name: &str,
    host: &str,
    roots: &[String],
    local_source: Option<&LocalSourceIdentity>,
    control: (Instant, Duration),
) -> Result<(NativeOwnerProjection, Sha256Digest, usize), String> {
    let (started, diagnostic_budget) = control;
    let target_config = TargetProfile::load_config(&source.join("aros-targets.toml"))
        .map_err(|_| "cannot load retained Source target profiles")?;
    let target = target_config
        .targets
        .iter()
        .find(|target| target.name == profile_name)
        .ok_or("selected Source target profile is absent")?;
    let source_profile = SourceMesaPreflightProfile::load(source, target)
        .map_err(|_| "cannot load selected Source target profile")?;
    let contract_path = target
        .sdk_consumer_contract
        .as_deref()
        .ok_or("selected Source target has no consumer contract")?;
    let contract: Value = read_json(&source.join(contract_path), "selected Source contract")?;
    let projection_path = required_string(&contract, "metamake_projection")?;
    let policy: Value = read_json(&source.join(projection_path), "selected Source policy")?;
    let configuration_source = required_string(&policy, "configuration_source")?;
    let configuration_bytes = read_regular_file(
        &source.join(configuration_source),
        MAX_CONTROL_BYTES,
        "selected Source MetaMake configuration",
    )?;
    let mut projected_configuration = String::from_utf8(configuration_bytes)
        .map_err(|_| "selected Source MetaMake configuration is not UTF-8")?;
    for substitution in policy
        .get("substitutions")
        .and_then(Value::as_array)
        .ok_or("selected Source policy substitutions are malformed")?
    {
        let token = required_string(substitution, "name")?;
        let value = required_string(substitution, "value")?;
        if !projected_configuration.contains(token) {
            return Err("selected Source configuration lacks a sealed substitution".into());
        }
        projected_configuration = projected_configuration.replace(token, value);
    }
    let expected_project_name = required_string(&policy, "project")?;
    if expected_project_name != project_name {
        return Err("Source policy project differs from the retained direct selection".into());
    }
    let expected_project =
        ConfiguredProject::parse(&projected_configuration, expected_project_name)
            .map_err(|_| "cannot parse selected Source policy project")?;
    if expected_project.name != project_name {
        return Err("Source policy project identity changed".into());
    }
    check_budget(started, diagnostic_budget)?;

    let excluded = local_source
        .map(|identity| BTreeSet::from([identity.generated_subtree.clone()]))
        .unwrap_or_default();
    let discovered = discover_meta_inputs(source, &expected_project.ignored_directories, &excluded)
        .map_err(|_| "cannot independently discover Source MetaMake inputs")?;
    let logical_files = discovered
        .into_keys()
        .map(|logical| source.join(logical))
        .collect::<Vec<_>>();
    let projection = source_profile
        .native_owner_projection(&logical_files, host, &excluded, local_source)
        .map_err(|_| "cannot independently derive Source native owner projection")?;
    let source_policy_sha256 = Sha256Digest::parse(projection.policy_sha256())
        .map_err(|_| "derived Source policy digest is malformed")?;
    let selected = projection
        .select_source_metadata(source, roots)
        .map_err(|_| "cannot select Source owner-goal pairs")?;
    let derived_pairs = selected
        .selection()
        .selected_owner_targets
        .values()
        .try_fold(0usize, |count, targets| count.checked_add(targets.len()))
        .ok_or("derived Source owner-goal pair count overflowed")?;
    Ok((projection, source_policy_sha256, derived_pairs))
}

fn validate_matched_coordinates(
    matched: &MatchedChildSource,
    call_index: usize,
    expected: &RetainedChildCall,
) -> Result<(), String> {
    let coordinates = matched.coordinates();
    if coordinates.generated_owner() != expected.generated_owner
        || coordinates.source_input() != expected.source_input
        || coordinates.goals() != expected.goals
    {
        return Err(format!(
            "public Source reader coordinates differ from retained call {call_index}"
        ));
    }
    Ok(())
}

fn retained_batch_limits(value: &Value) -> Result<RetainedChildSourceBatchLimits, String> {
    let inputs = MakeInputBatchLimits {
        maximum_children: required_usize(value, "maximum_children")?,
        maximum_read_bytes: required_usize(value, "maximum_read_bytes")?,
        maximum_reads: required_usize(value, "maximum_reads")?,
        maximum_queries: required_usize(value, "maximum_queries")?,
        maximum_query_names: required_usize(value, "maximum_query_names")?,
        maximum_query_bytes: required_usize(value, "maximum_query_bytes")?,
        maximum_record_bytes: required_usize(value, "maximum_record_bytes")?,
    };
    Ok(RetainedChildSourceBatchLimits {
        inputs,
        maximum_retained_bytes: required_usize(value, "maximum_retained_bytes")?,
    })
}

fn selected_pair_count(selection: &Value) -> Result<usize, String> {
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
                .ok_or("retained Source owner-goal count overflowed")
        })
        .map_err(str::to_owned)
}

fn expected_slot_names(count: usize) -> Result<BTreeSet<OsString>, String> {
    if count == 0 || count > MAX_CALLS {
        return Err("retained call count is outside its diagnostic bound".into());
    }
    let mut names = BTreeSet::new();
    for slot in 0..count {
        let call_name = format!("call-{slot:04}");
        names.insert(OsString::from(&call_name));
        names.insert(OsString::from(format!("{call_name}.claim")));
    }
    Ok(names)
}

fn canonical_directory(path: &Path, label: &'static str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{label} is not absolute"));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| format!("{label} is unavailable"))?;
    if canonical != path || !canonical.is_dir() {
        return Err(format!("{label} is not a canonical directory"));
    }
    Ok(canonical)
}

fn canonical_file(path: &Path, label: &'static str) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(format!("{label} must be normalized and absolute"));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| format!("{label} is unavailable"))?;
    if canonical != path || !canonical.is_file() {
        return Err(format!("{label} is not a canonical regular file"));
    }
    Ok(canonical)
}

fn case_path_value(case: &Value, key: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(required_string(case, key)?);
    if !path.is_absolute() {
        return Err("retained case root is not absolute".into());
    }
    Ok(path)
}

fn read_json(path: &Path, label: &'static str) -> Result<Value, String> {
    let bytes = read_regular_file(path, MAX_CONTROL_BYTES, label)?;
    serde_json::from_slice(&bytes).map_err(|_| format!("{label} is malformed"))
}

fn read_regular_file(path: &Path, limit: u64, label: &'static str) -> Result<Vec<u8>, String> {
    measure_regular_file_bounded(path, limit)
        .map_err(|_| format!("cannot safely read {label}"))?
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| format!("{label} is absent"))
}

fn selected_local_source_preset(case: &Value) -> Result<Option<&str>, String> {
    let Some(value) = case.get("local_source") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(fields) = value.as_object() else {
        return Err("retained LocalSource selection is malformed".into());
    };
    if fields.len() != 1 {
        return Err("retained LocalSource selection has unexpected fields".into());
    }
    let preset = fields
        .get("preset")
        .and_then(Value::as_str)
        .filter(|preset| !preset.trim().is_empty())
        .ok_or("retained LocalSource preset is malformed")?;
    Ok(Some(preset))
}

fn retained_collection_call_count(
    report: &Value,
    case_maximum_calls: usize,
) -> Result<usize, String> {
    if case_maximum_calls == 0 || case_maximum_calls > MAX_CALLS {
        return Err("retained direct collection call bound is outside the diagnostic limit".into());
    }
    let count = required_usize(report, "calls")?;
    if count == 0 || count > case_maximum_calls || count > MAX_CALLS {
        return Err("retained collection count is outside its bounded case selection".into());
    }
    Ok(count)
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| "retained data lacks a required string field".into())
}

fn required_strings(value: &Value, key: &str) -> Result<Vec<String>, String> {
    serde_json::from_value(
        value
            .get(key)
            .ok_or("retained data lacks a required string-list field")?
            .clone(),
    )
    .map_err(|_| "retained data string-list field is malformed".into())
}

fn required_usize(value: &Value, key: &str) -> Result<usize, String> {
    usize::try_from(
        value
            .get(key)
            .and_then(Value::as_u64)
            .ok_or("retained data lacks a bounded unsigned integer")?,
    )
    .map_err(|_| "retained data unsigned integer is outside platform bounds".into())
}

fn check_budget(started: Instant, budget: Duration) -> Result<(), &'static str> {
    if started.elapsed() > budget {
        Err("fresh data-only diagnostic budget expired")
    } else {
        Ok(())
    }
}

fn safe_reason(reason: &str, sensitive_strings: &SensitiveStrings) -> (String, bool) {
    let mut boundary = reason.len().min(MAX_REASON_BYTES);
    while !reason.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let truncated_input = boundary < reason.len();
    let bounded_reason = &reason[..boundary];
    let mut output = String::with_capacity(bounded_reason.len());
    let mut redacted = truncated_input;
    let mut chars = bounded_reason.char_indices();
    while let Some((_, character)) = chars.next() {
        if character == '`' {
            let mut token = String::new();
            for (_, next) in chars.by_ref() {
                if next == '`' {
                    break;
                }
                token.push(next);
            }
            if is_safe_diagnostic_token(&token) {
                output.push('`');
                output.push_str(&token);
                output.push('`');
            } else {
                redacted = true;
                output.push_str("[source expression redacted]");
            }
        } else {
            output.push(character);
        }
    }
    let mut sensitive_values = sensitive_strings.values.iter().collect::<Vec<_>>();
    sensitive_values.sort_unstable_by_key(|value| std::cmp::Reverse(value.len()));
    for sensitive in sensitive_values {
        if output.contains(sensitive.as_str()) {
            output = output.replace(sensitive.as_str(), &"*".repeat(sensitive.len()));
            redacted = true;
        }
    }
    if output.len() > MAX_REASON_BYTES {
        let mut boundary = MAX_REASON_BYTES;
        while !output.is_char_boundary(boundary) {
            boundary -= 1;
        }
        output.truncate(boundary);
        output.push_str(" [reason truncated]");
        redacted = true;
    }
    (output, redacted)
}

fn is_safe_diagnostic_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 80
        && !token.starts_with('$')
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

#[test]
fn retained_storage_metrics_are_checked_scalars_not_unredacted_reason_text() {
    let good = "retained child source batch at index 1: retained-data limit exceeded (prior=10, variable=7, file=4, next=21, limit=20)";
    let parsed = RetainedStorageRefusal::parse(good, 2, 20).unwrap();
    assert_eq!(
        parsed,
        RetainedStorageRefusal {
            child_index: 1,
            prior_estimate_bytes: 10,
            variable_estimate_bytes: 7,
            file_estimate_bytes: 4,
            next_estimate_bytes: 21,
            limit_bytes: 20,
        }
    );
    let mut sensitive = SensitiveStrings::default();
    sensitive.insert("1").unwrap();
    assert!(safe_reason(good, &sensitive).0.contains('*'));
    let encoded = serde_json::to_string(&parsed).unwrap();
    assert!(!encoded.contains("retained-data"));
    assert!(!encoded.contains("variable="));
    for malformed in [
        good.replace("index 1", "index 2"),
        good.replace("prior=10", "prior=21"),
        good.replace("next=21", "next=20"),
        good.replace("next=21", "next=22"),
        good.replace("variable=7", "variable=secret"),
        good.replace("variable=7", "variable=184467440737095516160"),
        good.replace("variable=7", "variable=1073741825"),
        good.replace("file=4", "file=-4"),
        format!("{good} trailing-private-text"),
        good.replace("limit=20)", "limit=20, extra=secret)"),
    ] {
        assert!(RetainedStorageRefusal::parse(&malformed, 2, 20).is_none());
    }
    assert!(RetainedStorageRefusal::parse(good, 0, 20).is_none());
    assert!(RetainedStorageRefusal::parse(good, 1, 20).is_none());
    assert!(RetainedStorageRefusal::parse(&good.replace("index 1", "index 0"), 2, 20).is_none());
    assert!(RetainedStorageRefusal::parse(good, MAX_CALLS + 1, 20).is_none());
    assert!(RetainedStorageRefusal::parse(good, 2, 19).is_none());
}

#[test]
fn sensitive_strings_redact_values_and_count_unique_bytes() {
    let mut sensitive = SensitiveStrings::with_limits(3, 12);
    sensitive.insert("alpha").unwrap();
    sensitive.insert("alpha").unwrap();
    sensitive.insert("beta").unwrap();

    assert_eq!(sensitive.value_count(), 2);
    assert_eq!(sensitive.byte_count(), 9);
    let (reason, redacted) = safe_reason("cause alpha beta", &sensitive);
    assert!(redacted);
    assert_eq!(reason, "cause ***** ****");
}

#[test]
fn local_source_case_selection_accepts_only_a_single_preset_field() {
    assert_eq!(
        selected_local_source_preset(&serde_json::json!({})).unwrap(),
        None
    );
    assert_eq!(
        selected_local_source_preset(&serde_json::json!({"local_source": null})).unwrap(),
        None
    );
    assert_eq!(
        selected_local_source_preset(&serde_json::json!({
            "local_source": {"preset": "arm-raspi"}
        }))
        .unwrap(),
        Some("arm-raspi")
    );
    for malformed in [
        serde_json::json!({"local_source": "arm-raspi"}),
        serde_json::json!({"local_source": {}}),
        serde_json::json!({"local_source": {"preset": 7}}),
        serde_json::json!({"local_source": {"preset": "arm-raspi", "extra": true}}),
    ] {
        assert!(selected_local_source_preset(&malformed).is_err());
    }
}

#[test]
fn retained_collection_count_is_nonzero_and_within_both_bounds() {
    assert_eq!(
        retained_collection_call_count(&serde_json::json!({"calls": 3}), 5).unwrap(),
        3
    );
    assert!(retained_collection_call_count(&serde_json::json!({"calls": 0}), 5).is_err());
    assert!(retained_collection_call_count(&serde_json::json!({"calls": 6}), 5).is_err());
    assert!(retained_collection_call_count(
        &serde_json::json!({"calls": MAX_CALLS + 1}),
        MAX_CALLS
    )
    .is_err());
}

#[test]
fn sensitive_strings_fail_closed_at_count_and_byte_limits() {
    let mut count_limited = SensitiveStrings::with_limits(1, 16);
    count_limited.insert("first").unwrap();
    assert!(count_limited.insert("second").is_err());
    assert_eq!(count_limited.value_count(), 1);
    assert_eq!(count_limited.byte_count(), 5);

    let mut byte_limited = SensitiveStrings::with_limits(4, 5);
    byte_limited.insert("abc").unwrap();
    assert!(byte_limited.insert("def").is_err());
    assert_eq!(byte_limited.value_count(), 1);
    assert_eq!(byte_limited.byte_count(), 3);
}

fn write_report(path: &Path, report: &DiagnosticReport) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("private report path must be normalized and absolute".into());
    }
    let requested_parent = path.parent().ok_or("private report path has no parent")?;
    let parent = requested_parent
        .canonicalize()
        .map_err(|_| "private report parent is unavailable")?;
    if parent.as_path() != requested_parent {
        return Err("private report parent is not canonical".into());
    }
    let case_path = std::env::var_os(CASE_ENV)
        .map(PathBuf::from)
        .ok_or("explicit retained case path is absent")?;
    let case_path = canonical_file(&case_path, "retained case path")?;
    let case_parent = case_path
        .parent()
        .ok_or("retained case path has no parent")?
        .canonicalize()
        .map_err(|_| "retained case parent is unavailable")?;
    let case: Value = read_json(&case_path, "retained case JSON")?;
    let source = canonical_directory(&case_path_value(&case, "source_root")?, "Source root")?;
    let output = canonical_directory(&case_path_value(&case, "output_root")?, "output root")?;
    let compiler = canonical_directory(&case_path_value(&case, "compiler_root")?, "compiler root")?;
    let direct = case
        .get("native_dispatch")
        .and_then(|value| value.get("direct_collection"))
        .ok_or("retained case has no direct collection selection")?;
    let collection = canonical_directory(
        Path::new(required_string(direct, "output_root")?),
        "retained direct collection directory",
    )?;
    for protected in [
        source.as_path(),
        output.as_path(),
        compiler.as_path(),
        collection.as_path(),
        case_parent.as_path(),
    ] {
        if filesystem_paths_overlap(&parent, protected)
            .map_err(|_| "cannot compare child-source report scope")?
        {
            return Err("private child-source report overlaps retained inputs".into());
        }
    }
    let report_directory = PrivateNativeCaptureDirectory::open(&parent)
        .map_err(|_| "private child-source report parent is not an owned mode-0700 directory")?;
    let name = path
        .file_name()
        .ok_or("private report path has no filename")?;
    let bytes = serde_json::to_vec_pretty(report)
        .map_err(|_| "cannot encode private child-source report")?;
    report_directory
        .write_new(name, &bytes)
        .map_err(|_| "cannot create fresh private child-source report")?;
    report_directory
        .revalidate()
        .map_err(|_| "private child-source report directory changed")?;
    Ok(())
}
