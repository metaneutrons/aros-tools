//! Explicit local captured-startup experiment, not a portable execution receipt.
#![cfg(unix)]

use aros_common::{
    make_initial_values::{
        MakeInitialValueFlavor, ValidatedMakeCommandLineValues, ValidatedMakeEnvironmentValues,
        ValidatedMakeInitialValues, ValidatedMakeParserControls,
    },
    make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace},
    make_native_invocation::NativeMakeInvocation,
    make_observer::{
        command_line_values_fragment, environment_values_fragment, pre_file_fragment,
        validate_source_prelude_inputs,
    },
    make_startup_arguments::LiteralMakeStartupArguments,
    sha256_bytes, CancellationToken, Sha256Digest,
};
use aros_transpiler::{
    dirs::DirVars,
    make_vars::{MakeProgramPaths, MakeVariableProgram},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::Write as _,
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const STOP: &str = "__aros_initial_stop_before_source";

#[path = "support/captured_input_records.rs"]
mod captured_input_records;

#[path = "support/captured_rule_inventory.rs"]
mod captured_rule_inventory;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    capture: PathBuf,
    capture_sha256: Sha256Digest,
    program: String,
    executable: PathBuf,
    executable_sha256: Sha256Digest,
    source_root: PathBuf,
    build_root: PathBuf,
    relative_directory: PathBuf,
    output: PathBuf,
    #[serde(default)]
    read_source_inputs: bool,
    #[serde(default)]
    additional_roots: Vec<AdditionalInputRoot>,
    #[serde(default)]
    target_expressions: Vec<String>,
}

/// Explicit caller-selected data, not authority inferred from a failed lookup.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdditionalInputRoot {
    root: PathBuf,
    content_sha256: Sha256Digest,
}

#[test]
fn additional_input_root_requires_an_explicit_closed_digest_record() {
    let valid = serde_json::json!({
        "root": "/synthetic/selected-input", "content_sha256": "a".repeat(64)
    });
    assert!(serde_json::from_value::<AdditionalInputRoot>(valid.clone()).is_ok());
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("content_sha256");
    assert!(serde_json::from_value::<AdditionalInputRoot>(missing).is_err());
    let mut invalid = valid.clone();
    invalid["content_sha256"] = "not-a-digest".into();
    assert!(serde_json::from_value::<AdditionalInputRoot>(invalid).is_err());
    let mut unknown = valid;
    unknown["adopt_observed_digest"] = true.into();
    assert!(serde_json::from_value::<AdditionalInputRoot>(unknown).is_err());
}

fn regular(path: &Path, max: u64) -> Vec<u8> {
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(
        metadata.is_file() && metadata.len() <= max,
        "unsupported selected input"
    );
    let bytes = fs::read(path).unwrap();
    assert!(bytes.len() as u64 <= max, "selected input exceeds limit");
    bytes
}

fn bound_capture(
    bytes: &[u8],
    expected: &Sha256Digest,
) -> Result<NativeMakeInvocation, &'static str> {
    // Hash the exact buffer passed to the parser, not a second path read.
    if &sha256_bytes(bytes) != expected {
        return Err("selected capture bytes differ");
    }
    NativeMakeInvocation::parse(bytes).map_err(|_| "selected capture is invalid")
}

#[test]
fn capture_parser_binds_the_used_buffer_and_refuses_substitution() {
    let original = NativeMakeInvocation::new(
        vec!["-f".into(), "first.mk".into()],
        PathBuf::from("/synthetic/child"),
        Vec::new(),
    )
    .unwrap();
    let replacement = NativeMakeInvocation::new(
        vec!["-f".into(), "second.mk".into()],
        PathBuf::from("/synthetic/child"),
        Vec::new(),
    )
    .unwrap();
    let bytes = original.to_bytes().unwrap();
    let other_bytes = replacement.to_bytes().unwrap();
    let expected = sha256_bytes(&bytes);
    assert_eq!(bound_capture(&bytes, &expected).unwrap(), original);
    assert_eq!(
        bound_capture(&other_bytes, &expected).unwrap_err(),
        "selected capture bytes differ"
    );
    assert!(bound_capture(&bytes, &sha256_bytes(&other_bytes)).is_err());
    assert!(bound_capture(b"invalid", &sha256_bytes(b"invalid")).is_err());
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn lookup_program(program: &str, capture: &NativeMakeInvocation) -> Result<PathBuf, &'static str> {
    for path in std::env::split_paths(
        capture
            .environment()
            .get("PATH")
            .ok_or("captured PATH absent")?,
    )
    .map(|directory| {
        let directory = if directory.is_absolute() {
            directory
        } else {
            capture.working_directory().join(directory)
        };
        directory.join(program)
    }) {
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("captured program lookup inaccessible"),
            Ok(_) => {}
        }
        // Conservatively refuse, rather than emulate execvp's skip/error rules.
        // Launch uses this bound executable with the caller-selected argv[0].
        let metadata = fs::metadata(&path).map_err(|_| "captured program target inaccessible")?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err("captured first program candidate is not executable");
        }
        return Ok(path);
    }
    Err("captured program lookup absent")
}

fn check_inputs(case: &Case, capture: &NativeMakeInvocation) {
    assert!(case.target_expressions.len() <= 64);
    assert!(case
        .target_expressions
        .iter()
        .all(|expression| expression.len() <= 65_536));
    assert_eq!(
        sha256_bytes(&regular(&case.capture, 2 * 1024 * 1024)),
        case.capture_sha256
    );
    assert!(case.executable.is_absolute());
    assert_eq!(fs::canonicalize(&case.executable).unwrap(), case.executable);
    assert_eq!(
        sha256_bytes(&regular(&case.executable, 16 * 1024 * 1024)),
        case.executable_sha256
    );
    assert!(
        !case.program.is_empty() && !case.program.contains('/'),
        "select a literal program spelling"
    );
    let selected = lookup_program(&case.program, capture).expect("captured executable lookup");
    assert_eq!(
        fs::canonicalize(selected).unwrap(),
        case.executable,
        "captured program selection differs"
    );
}

#[test]
fn captured_path_lookup_uses_child_cwd_for_relative_and_empty_entries() {
    let temporary = tempfile::tempdir().unwrap();
    let cwd = temporary.path().canonicalize().unwrap().join("child");
    let tools = cwd.join("tools");
    fs::create_dir_all(&tools).unwrap();
    let name = "synthetic-captured-program-419b";
    for parent in [&cwd, &tools] {
        fs::write(parent.join(name), b"fixture, never executed").unwrap();
        fs::set_permissions(parent.join(name), fs::Permissions::from_mode(0o700)).unwrap();
    }
    for (path, expected) in [
        ("tools".to_owned(), tools.join(name)),
        (String::new(), cwd.join(name)),
        (tools.to_str().unwrap().to_owned(), tools.join(name)),
    ] {
        let capture =
            NativeMakeInvocation::new(Vec::new(), cwd.clone(), [("PATH".into(), path)]).unwrap();
        assert_eq!(lookup_program(name, &capture).unwrap(), expected);
    }
    fs::set_permissions(tools.join(name), fs::Permissions::from_mode(0o600)).unwrap();
    let capture =
        NativeMakeInvocation::new(Vec::new(), cwd, [("PATH".into(), "tools:".into())]).unwrap();
    assert!(
        lookup_program(name, &capture).is_err(),
        "must not attribute a later executable to a nonexecutable first candidate"
    );
}

fn is_expected_stop(stderr: &str) -> bool {
    stderr.lines().count() == 1 && stderr.trim_end().ends_with(&format!("{STOP}.  Stop."))
}

#[test]
fn native_stop_marker_requires_only_the_deliberate_gnu_error() {
    assert!(is_expected_stop(&format!("gmake[1]: *** {STOP}.  Stop.\n")));
    assert!(!is_expected_stop(&format!(
        "warning\ngmake: *** {STOP}.  Stop.\n"
    )));
    assert!(!is_expected_stop("gmake: *** missing separator.  Stop.\n"));
    assert!(!is_expected_stop(""));
}

fn observe(case: &Case, captured: &NativeMakeInvocation, fragment: &str, label: &str) -> Vec<u8> {
    check_inputs(case, captured);
    // Execute the independently pinned file with the caller-selected argv[0].
    // NativeMakeInvocation excludes argv[0]; its original spelling still needs
    // independent owner binding. PATH lookup alone cannot establish that fact.
    // This does not claim exec-fd identity against concurrent same-UID writes.
    let mut command = Command::new(&case.executable);
    command
        .arg0(&case.program)
        .env_clear()
        .envs(captured.environment())
        .current_dir(captured.working_directory())
        .args(["--eval", &format!("{fragment}$(error {STOP})\n")])
        .args(captured.arguments());
    let result = aros_common::run_output_with_control(
        &mut command,
        256 * 1024,
        Duration::from_secs(20),
        &CancellationToken::default(),
    )
    .unwrap();
    let stdout = result.stdout.exact_bytes().expect("bounded native stdout");
    let stderr = result.stderr.exact_bytes().expect("bounded native stderr");
    write_private(&case.output.join(format!("{label}.stdout")), stdout);
    write_private(&case.output.join(format!("{label}.stderr")), stderr);
    assert!(
        !result.timed_out && !result.cancelled && result.status.code() == Some(2),
        "native pre-file stop failed; inspect private logs"
    );
    let stderr_text = std::str::from_utf8(stderr).unwrap();
    assert!(
        is_expected_stop(stderr_text),
        "native stop marker differs; inspect private logs"
    );
    check_inputs(case, captured);
    stdout.to_vec()
}

#[test]
#[ignore = "requires explicitly selected retained native capture and GNU executable"]
fn captured_root_child_initial_state_enters_source_interpreter() {
    let case: Case = serde_json::from_slice(&regular(
        &PathBuf::from(
            std::env::var_os("AROS_TEST_CAPTURED_MAKE_STARTUP_CASE").expect("select explicit case"),
        ),
        64 * 1024,
    ))
    .unwrap();
    let capture_bytes = regular(&case.capture, 2 * 1024 * 1024);
    let capture = bound_capture(&capture_bytes, &case.capture_sha256).unwrap();
    check_inputs(&case, &capture);
    assert!(case.read_source_inputs || case.additional_roots.is_empty());
    for path in [&case.source_root, &case.build_root].into_iter().chain(
        case.additional_roots
            .iter()
            .map(|selection| &selection.root),
    ) {
        assert_eq!(fs::canonicalize(path).unwrap(), *path);
    }
    assert!(
        !case.relative_directory.is_absolute()
            && case
                .relative_directory
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
    );
    assert_eq!(
        case.build_root.join(&case.relative_directory),
        capture.working_directory()
    );
    let startup = LiteralMakeStartupArguments::admit(capture.arguments()).unwrap();
    let assignments: BTreeMap<_, _> = capture
        .arguments()
        .iter()
        .filter_map(|argument| argument.split_once('='))
        .filter(|(name, _)| startup.command_line_names().contains(*name))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    validate_source_prelude_inputs(capture.environment(), &assignments).unwrap();
    assert!(case.output.is_absolute());
    let output_parent = fs::canonicalize(case.output.parent().unwrap()).unwrap();
    assert_eq!(
        output_parent.join(case.output.file_name().unwrap()),
        case.output
    );
    for root in [&case.source_root, &case.build_root].into_iter().chain(
        case.additional_roots
            .iter()
            .map(|selection| &selection.root),
    ) {
        assert!(
            !case.output.starts_with(root) && !root.starts_with(&case.output),
            "observation output overlaps selected inputs"
        );
    }
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&case.output)
        .unwrap();
    assert_eq!(fs::canonicalize(&case.output).unwrap(), case.output);
    let base = format!(
        "{}{}",
        pre_file_fragment(),
        command_line_values_fragment(startup.command_line_names()).unwrap()
    );
    let discovery = observe(&case, &capture, &base, "discovery");
    let namespace = ValidatedMakeNamespace::from_make_stdout(&discovery).unwrap();
    let environment_names = namespace
        .to_observation()
        .entries
        .into_iter()
        .filter(|entry| entry.origin == MakeVariableOrigin::Environment)
        .map(|entry| entry.name)
        .collect();
    let stdout = observe(
        &case,
        &capture,
        &format!(
            "{base}{}",
            environment_values_fragment(&environment_names).unwrap()
        ),
        "initial",
    );
    let namespace = ValidatedMakeNamespace::from_make_stdout(&stdout).unwrap();
    let defaults = ValidatedMakeInitialValues::from_make_stdout(&stdout, &namespace).unwrap();
    let controls = ValidatedMakeParserControls::from_make_stdout(&stdout, &namespace).unwrap();
    let command = ValidatedMakeCommandLineValues::from_make_stdout(
        &stdout,
        &namespace,
        startup.command_line_names(),
    )
    .unwrap();
    let environment =
        ValidatedMakeEnvironmentValues::from_make_stdout(&stdout, &namespace, &environment_names)
            .unwrap();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &case.source_root,
        relative_dir: &case.relative_directory,
        top_directory: &case.build_root,
        working_directory: capture.working_directory(),
    };
    let mut program = MakeVariableProgram::from_native_invocation_observation(
        &paths,
        &stdout,
        &capture,
        &environment_names,
    )
    .unwrap();
    let entries = defaults
        .entries()
        .chain(controls.entries())
        .chain(command.entries())
        .chain(environment.entries())
        .collect::<Vec<_>>();
    let queries = entries
        .iter()
        .flat_map(|entry| {
            [
                format!("$(origin {})", entry.name),
                format!("$(flavor {})", entry.name),
                format!("$(value {})", entry.name),
            ]
        })
        .collect::<Vec<_>>();
    let results = program
        .evaluate_many(
            &queries.iter().map(String::as_str).collect::<Vec<_>>(),
            &paths,
        )
        .unwrap();
    assert_eq!(results.len(), entries.len() * 3);
    let (results, remainder) = results.as_chunks::<3>();
    assert!(remainder.is_empty());
    for (entry, result) in entries.iter().zip(results) {
        let flavor = match entry.flavor {
            MakeInitialValueFlavor::Simple => "simple",
            MakeInitialValueFlavor::Recursive => "recursive",
        };
        assert!(
            result[0] == entry.origin.as_gnu_label(),
            "captured source origin differs"
        );
        assert!(result[1] == flavor, "captured source flavor differs");
        assert!(
            result[2] == entry.value,
            "captured source raw value differs"
        );
    }
    if case.read_source_inputs {
        read_source_inputs(
            &case,
            &capture,
            &stdout,
            &environment_names,
            &paths,
            &startup,
        );
    }
    check_inputs(&case, &capture);
    write_private(&case.output.join("result.json"), &serde_json::to_vec_pretty(&serde_json::json!({
        "schema":"aros-captured-startup-experiment-v1", "capture_sha256":case.capture_sha256,
        "program_sha256":case.executable_sha256, "initial_stdout_sha256":sha256_bytes(&stdout),
        "verified_entries":entries.len(), "argument_count":capture.arguments().len(), "original_environment_count":capture.environment().len(),
        "phase":"before-first-source-file", "native_status":2, "source_recipes_executed":false,
        "instrumentation":"prepended-eval-observer-with-deliberate-parse-stop", "uninstrumented_state_equivalence_proven":false,
        "qualification":"controlled-local-data-consistency-only", "sdk_qualification":false
    })).unwrap());
}

fn read_source_inputs(
    case: &Case,
    capture: &NativeMakeInvocation,
    stdout: &[u8],
    environment_names: &std::collections::BTreeSet<String>,
    paths: &MakeProgramPaths<'_>,
    startup: &LiteralMakeStartupArguments,
) {
    use aros_common::make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot};
    let mut selected = Vec::new();
    for root in [&case.source_root, &case.build_root] {
        let tree = aros_common::measure_tree_content_cas_bounded(
            root,
            aros_common::TreeTraversalLimits {
                max_entries: 100_000,
                max_regular_file_bytes: 4 * 1024 * 1024 * 1024,
            },
        )
        .unwrap();
        selected.push(MakeInputRootSelection {
            root: root.clone(),
            content_sha256: tree.payload_digest_excluding(None),
        });
    }
    // The extra roots retain independently supplied expected digests. Never
    // measure a failed search path and silently adopt it as another capability.
    selected.extend(
        case.additional_roots
            .iter()
            .map(|selection| MakeInputRootSelection {
                root: selection.root.clone(),
                content_sha256: selection.content_sha256.clone(),
            }),
    );
    let snapshot = MakeInputSnapshot::capture(capture.working_directory(), &selected).unwrap();
    let root_bytes = serde_json::to_vec_pretty(
        &selected
            .iter()
            .map(|selection| {
                serde_json::json!({
                    "root":selection.root, "content_sha256":selection.content_sha256,
                    "binding":"explicit-current-root-consistency-not-historical-authority"
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let roots_sha256 = sha256_bytes(&root_bytes);
    write_private(&case.output.join("source-input-roots.json"), &root_bytes);
    // No guessed include-search list is installed. An unresolved relative
    // include search must refuse until its native search context is bound.
    let result = MakeVariableProgram::read_native_captured_inputs(
        paths,
        stdout,
        capture,
        environment_names,
        &snapshot,
    );
    match result {
        Ok((mut program, file, reads)) => {
            let rules = aros_transpiler::make_interpreted_rules::collect(&file)
                .expect("interpreted source rule association failed");
            let implicit_rule_count = rules
                .rules
                .iter()
                .filter(|rule| matches!(rule.header(), aros_transpiler::make_rule_header::RuleHeader::Rule { targets, target_pattern: None, .. } if targets.contains('%')))
                .count();
            let target_assignment_count = rules
                .other_events
                .iter()
                .filter(|index| {
                    matches!(
                        file.events[**index].kind,
                        aros_transpiler::make_vars::MakeFileEventKind::Rule(
                            aros_transpiler::make_rule_header::RuleHeader::TargetAssignment { .. }
                        )
                    )
                })
                .count();
            let read_records_sha256 = captured_input_records::retain(
                &case.output,
                &reads,
                &roots_sha256,
                &sha256_bytes(stdout),
                &case.capture_sha256,
            );
            let rule_inventory_sha256 =
                captured_rule_inventory::retain(&case.output, &file, startup, &read_records_sha256);
            let projection = rules.project_explicit().unwrap_or_else(|error| {
                write_private(
                    &case.output.join("source-projection-error.txt"),
                    error.as_bytes(),
                );
                panic!("finite source projection refused; inspect private artifact");
            });
            let explicit_declaration_count = projection
                .graph
                .targets
                .values()
                .map(|target| match target {
                    aros_transpiler::make_rule_graph::TargetRules::Ordinary(target) => {
                        target.declarations().len()
                    }
                    aros_transpiler::make_rule_graph::TargetRules::DoubleColon(declarations) => {
                        declarations.len()
                    }
                })
                .sum::<usize>();
            let goals = startup
                .goals()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let trace = aros_transpiler::make_rule_graph::traversal::trace_declared(
                &projection.graph,
                &goals,
            )
            .expect("declared source trace refused");
            let trace_bytes = serde_json::to_vec_pretty(&serde_json::json!({
                "rule_inventory_sha256":rule_inventory_sha256,
                "scope":"declared-graph-only-all-external-inputs-unresolved",
                "visits": trace.visits().iter().map(|visit| serde_json::json!({
                    "target":visit.target(), "parent":visit.parent(),
                    "rule_event":visit.instance().map(|instance| instance.output_line),
                    "external_input_unresolved":visit.instance().is_none()
                })).collect::<Vec<_>>(),
                "native_selection_proven":false, "sdk_qualification":false
            }))
            .unwrap();
            write_private(
                &case.output.join("source-declared-trace.json"),
                &trace_bytes,
            );
            let expressions = case
                .target_expressions
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let mut target_queries = Vec::new();
            let mut query_bytes = 0usize;
            if !expressions.is_empty() {
                let chains = (0..trace.visits().len())
                    .map(|index| trace.scope_chain(index).unwrap())
                    .collect::<Vec<_>>();
                let groups = program
                    .evaluate_target_contexts(&chains, &expressions, paths)
                    .unwrap_or_else(|error| {
                        write_private(
                            &case.output.join("source-target-query-error.txt"),
                            error.to_string().as_bytes(),
                        );
                        panic!("declared target data query refused; inspect private artifact");
                    });
                for (index, (visit, values)) in trace.visits().iter().zip(groups).enumerate() {
                    query_bytes = query_bytes
                        .checked_add(visit.target().len())
                        .and_then(|bytes| {
                            values
                                .iter()
                                .try_fold(bytes, |total, value| total.checked_add(value.len()))
                        })
                        .filter(|bytes| *bytes <= 16 * 1024 * 1024)
                        .expect("private target-query inventory exceeded byte bound");
                    target_queries.push(serde_json::json!({"visit":index, "values":values}));
                }
            }
            let target_query_bytes = serde_json::to_vec_pretty(&serde_json::json!({
                "declared_trace_sha256":sha256_bytes(&trace_bytes),
                "scope":"caller-selected-data-expressions-not-native-recipe-or-compiler-proof",
                "expressions":case.target_expressions, "queries":target_queries,
                "native_selection_proven":false, "source_recipes_executed":false,
                "sdk_qualification":false
            }))
            .unwrap();
            write_private(
                &case.output.join("source-target-queries.json"),
                &target_query_bytes,
            );
            let search_paths = aros_transpiler::make_vpath::project_interpreted(&file)
                .expect("interpreted search path projection refused");
            // GNU freezes general VPATH globally before target traversal.
            // Recipe-visible target values must not replace that search list.
            // Current membership still proves no native path retention/remake.
            let search_paths = search_paths
                .freeze_global(&mut program, paths)
                .unwrap_or_else(|error| {
                    write_private(
                        &case.output.join("source-search-error.txt"),
                        error.as_bytes(),
                    );
                    panic!("global search scope refused; inspect private artifact");
                });
            let mut leaf_names = std::collections::BTreeSet::new();
            for target in projection.graph.targets.values() {
                let declarations = match target {
                    aros_transpiler::make_rule_graph::TargetRules::Ordinary(target) => {
                        target.declarations()
                    }
                    aros_transpiler::make_rule_graph::TargetRules::DoubleColon(declarations) => {
                        declarations
                    }
                };
                for declaration in declarations {
                    for name in declaration
                        .prerequisites
                        .iter()
                        .chain(&declaration.order_only)
                    {
                        if !projection.graph.targets.contains_key(name) {
                            leaf_names.insert(name.as_str());
                        }
                    }
                }
            }
            let ((frontier, search_results), search_reads) = snapshot.transaction(|reader| {
                let frontier = search_paths.observe_declared_inputs(&trace, reader)?;
                let all_graph = leaf_names.iter().map(|name| {
                    search_paths.find_regular_input(name, reader)
                        .map(|selected| serde_json::json!({"requested": name, "selected": selected}))
                }).collect::<Result<Vec<_>, String>>()?;
                Ok((frontier, all_graph))
            }).unwrap_or_else(|error| {
                write_private(&case.output.join("source-search-error.txt"), error.as_bytes());
                panic!("captured regular-input search refused; inspect private artifact");
            });
            assert!(
                search_reads.reads().is_empty(),
                "membership queries must not claim content reads"
            );
            let search_found = search_results
                .iter()
                .filter(|row| !row["selected"].is_null())
                .count();
            let frontier_found = frontier
                .iter()
                .filter(|row| row.observed().is_some())
                .count();
            let frontier_bytes = serde_json::to_vec_pretty(&serde_json::json!({
                "declared_trace_sha256":sha256_bytes(&trace_bytes),
                "rule_inventory_sha256":rule_inventory_sha256,
                "global_vpath_sha256":sha256_bytes(search_paths.global_vpath().as_bytes()),
                "scope":"declared-frontier-current-regular-membership-not-admitted-native-inputs",
                "inputs":frontier.iter().map(|row| serde_json::json!({
                    "visit":row.visit(), "observed":row.observed()
                })).collect::<Vec<_>>(),
                "native_selection_proven":false, "source_recipes_executed":false,
                "sdk_qualification":false
            }))
            .unwrap();
            write_private(
                &case.output.join("source-declared-input-membership.json"),
                &frontier_bytes,
            );
            let search_bytes = serde_json::to_vec_pretty(&serde_json::json!({
                "read_records_sha256":read_records_sha256,
                "rule_inventory_sha256":rule_inventory_sha256,
                "global_vpath_sha256":sha256_bytes(search_paths.global_vpath().as_bytes()),
                "scope":"diagnostic-global-vpath-only-not-target-local",
                "inputs":search_results,
                "native_selection_proven":false, "source_recipes_executed":false,
                "sdk_qualification":false
            }))
            .unwrap();
            write_private(&case.output.join("source-input-search.json"), &search_bytes);
            write_private(&case.output.join("source-result.json"), &serde_json::to_vec_pretty(&serde_json::json!({
                "event_count":file.events.len(), "read_count":reads.reads().len(),
                "ordinary_rule_count":rules.rules.len(), "implicit_rule_count":implicit_rule_count,
                "target_assignment_count":target_assignment_count, "other_event_count":rules.other_events.len(),
                "read_records_sha256":read_records_sha256,
                "rule_inventory_sha256":rule_inventory_sha256,
                "explicit_target_count":projection.graph.targets.len(),
                "explicit_declaration_count":explicit_declaration_count,
                "explicit_goal_count":startup.goals().iter().filter(|goal| projection.graph.targets.contains_key(*goal)).count(),
                "implicit_rule_events":projection.implicit_rule_events,
                "declared_visit_count":trace.visits().len(),
                "declared_unresolved_count":trace.unresolved_inputs().count(),
                "declared_trace_sha256":sha256_bytes(&trace_bytes),
                "target_queries_sha256":sha256_bytes(&target_query_bytes),
                "declared_membership_sha256":sha256_bytes(&frontier_bytes),
                "declared_found_count":frontier_found,
                "search_input_count":leaf_names.len(), "search_found_count":search_found,
                "search_inventory_sha256":sha256_bytes(&search_bytes),
                "native_selection_proven":false, "policy_discharged":false,
                "source_recipes_executed":false, "qualification":"current-source-data-interpretation-only",
                "historical_source_binding_proven":false, "sdk_qualification":false
            })).unwrap());
        }
        Err(error) => {
            write_private(&case.output.join("source-error.txt"), error.as_bytes());
            panic!("native source interpretation refused; inspect private artifact");
        }
    }
}
