//! Read-only, data-only replay of one retained native child Source call.
//!
//! This diagnostic binds current retained files to the recorded Source hash.
//! It does not recover the original preparation owner, authenticate an SDK, or
//! establish that a leaf producer ran.

#![cfg(all(test, unix))]

use super::read_bytes;
use crate::compatibility::SourceMesaPreflightProfile;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_native_invocation::NativeMakeInvocation,
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    measure_tree_content_cas_bounded,
    mesa_source::MesaSourceSnapshot,
    publication::filesystem_paths_overlap,
    sha256_bytes, Sha256Digest, TargetProfile, TreeTraversalLimits,
};
use aros_transpiler::{
    dirs::DirVars, make_vars::MakeProgramPaths, metamake_project::ConfiguredProject,
    native_owner_projection::discover_meta_inputs,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::{Component, Path, PathBuf},
};

const CASE_ENV: &str = "AROS_TEST_MESA_PREFLIGHT_CASE";
const ERROR_ENV: &str = "AROS_TEST_MESA_READER_ERROR";
const COLLECTION_QUALIFICATION: &str = "supervised-instrumented-data-not-source-owner-join-or-sdk";
const JOIN_QUALIFICATION: &str = "source-policy-runtime-coordinate-join-not-leaf-execution-or-sdk";
const MAX_TREE_ENTRIES: usize = 100_000;
const MAX_TREE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 1024 * 1024;

struct DiagnosticSummary {
    current_build_digest: Sha256Digest,
    read_count: usize,
    generated_owner: String,
    source_input: String,
    goals: Vec<String>,
    recipe_summary: Option<(usize, usize, usize)>,
    declared_unresolved_inputs: Option<usize>,
    declared_unparsed_shells: Option<usize>,
    recipe_input_value: Option<RecipeInputSummary>,
    output_links: Option<(usize, usize)>,
}

#[test]
#[ignore = "requires an explicit retained native child Source case; data-only diagnostic"]
fn actual_retained_child_source_reader_diagnostic() {
    match run_diagnostic() {
        Ok(summary) => println!(
            "qualification=current-retained-data-only-not-sdk-or-leaf-producer current_build_digest={} reads={} generated_owner={} source_input={} goals={} recipe_summary={:?} declared_unresolved_inputs={:?} declared_unparsed_shells={:?} recipe_input_value={:?} output_links={:?}",
            summary.current_build_digest,
            summary.read_count,
            summary.generated_owner,
            summary.source_input,
            summary.goals.join(","),
            summary.recipe_summary,
            summary.declared_unresolved_inputs,
            summary.declared_unparsed_shells,
            summary.recipe_input_value,
            summary.output_links
        ),
        Err(error) => {
            let _ = preserve_error_privately(&error);
            panic!("retained child Source reader diagnostic failed");
        }
    }
}

fn run_diagnostic() -> Result<DiagnosticSummary, String> {
    let case_path =
        PathBuf::from(std::env::var_os(CASE_ENV).ok_or("explicit retained case path is absent")?);
    if !case_path.is_absolute() {
        return Err("retained case path is not absolute".into());
    }
    let case_bytes = read_bytes(&case_path).map_err(|error| error.to_string())?;
    let case: Value =
        serde_json::from_slice(&case_bytes).map_err(|_| "retained case JSON is malformed")?;
    let case_parent = case_path
        .parent()
        .ok_or("retained case path has no parent")?;

    let source_root = case_path_value(&case, "source_root")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let output_root = case_path_value(&case, "output_root")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let build_root = output_root
        .join("configure")
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let local_source = diagnostic_local_source(&case, &source_root)?;
    let (collection, direct) = collection_selection(&case)?;
    let collection_output = PathBuf::from(required_string(collection, "output_root")?.to_owned());
    if !collection_output.is_absolute() {
        return Err("retained collection root is not absolute".into());
    }
    let collection_output = collection_output
        .canonicalize()
        .map_err(|error| error.to_string())?;

    let collection_proof_path = case_parent.join(if direct {
        "native-direct-collection.json"
    } else {
        "native-root-collection.json"
    });
    let collection_proof: Value = serde_json::from_slice(
        &read_bytes(&collection_proof_path).map_err(|error| error.to_string())?,
    )
    .map_err(|_| "retained collection proof JSON is malformed")?;
    let qualification = if direct {
        "direct-metamake-first-input-and-source-coverage-not-leaf-or-sdk"
    } else {
        COLLECTION_QUALIFICATION
    };
    let calls = collection_proof["calls"]
        .as_u64()
        .ok_or("retained collection call count is absent")?;
    if collection_proof["qualification"].as_str() != Some(qualification)
        || calls == 0
        || calls > 4096
        || (!direct && calls != 1)
        || (direct
            && collection["maximum_calls"]
                .as_u64()
                .is_none_or(|maximum| maximum > 4096 || calls > maximum))
    {
        return Err(
            "retained collection qualification or call count differs from selection".into(),
        );
    }
    let proof_invocations = collection_proof["invocations"]
        .as_array()
        .ok_or("retained collection proof has no invocation list")?;
    if u64::try_from(proof_invocations.len()).ok() != Some(calls)
        || collection_proof["evidence"].as_str()
            != collection.get("output_root").and_then(Value::as_str)
    {
        return Err("retained collection evidence root or call count differs from case".into());
    }
    let slot_text = std::env::var_os("AROS_TEST_MESA_CHILD_SLOT");
    let slot_text = slot_text
        .as_ref()
        .map(|value| value.to_str().ok_or("diagnostic child slot is not UTF-8"))
        .transpose()?;
    let slot = selected_diagnostic_slot(direct, calls, slot_text)?;
    let proof_invocation = &proof_invocations[slot];

    let source_join_path = case_parent.join(if direct {
        "native-direct-preparation.json"
    } else {
        "native-source-join.json"
    });
    let source_join: Value =
        serde_json::from_slice(&read_bytes(&source_join_path).map_err(|error| error.to_string())?)
            .map_err(|_| "retained Source join proof JSON is malformed")?;
    let join_qualification = if direct {
        "original-owner-direct-preparation-not-execution-or-sdk"
    } else {
        JOIN_QUALIFICATION
    };
    if source_join["qualification"].as_str() != Some(join_qualification)
        || (direct && source_join["selection"] != collection_proof["selection"])
    {
        return Err("retained Source join proof has an unsupported qualification".into());
    }
    if direct {
        serde_json::from_value::<Sha256Digest>(source_join["selection"].clone())
            .map_err(|_| "retained direct selection identity is malformed")?;
    }
    let coordinate_report = if direct {
        &collection_proof
    } else {
        &source_join
    };
    let coordinates = coordinate_report["coordinates"]
        .as_array()
        .ok_or("retained Source join proof has no coordinates")?;
    if u64::try_from(coordinates.len()).ok() != Some(calls) {
        return Err("retained Source coordinate count differs from call count".into());
    }
    let coordinate = &coordinates[slot];
    let generated_owner =
        required_string(coordinate, if direct { "owner" } else { "generated_owner" })?.to_owned();
    let source_input =
        required_string(coordinate, if direct { "source" } else { "source_input" })?.to_owned();
    let joined_goals = required_strings(coordinate, "goals")?;

    let root_observation: Value = serde_json::from_slice(
        &read_bytes(&case_parent.join("native-root-observation.json"))
            .map_err(|error| error.to_string())?,
    )
    .map_err(|_| "retained root observation JSON is malformed")?;
    let source_sha256: Sha256Digest =
        serde_json::from_value(root_observation["identity"]["source_inputs_sha256"].clone())
            .map_err(|_| "retained root observation has no selected Source hash")?;
    let compiler_sha256: Sha256Digest =
        serde_json::from_value(root_observation["identity"]["compiler_tree_sha256"].clone())
            .map_err(|_| "retained root observation has no selected compiler hash")?;
    let compiler_root = case_path_value(&case, "compiler_root")?
        .canonicalize()
        .map_err(|error| error.to_string())?;

    let roots = if direct {
        required_strings(collection, "roots")?
    } else {
        required_strings(
            case.get("native_launch_query")
                .ok_or("retained case has no native launch query")?,
            "goals",
        )?
    };
    let reported_roots = if direct {
        required_strings(&source_join, "roots")?
    } else {
        required_strings(&collection_proof, "goals")?
    };
    if roots.is_empty() || roots != reported_roots {
        return Err("retained collection goals differ from original launch selection".into());
    }

    // Bind all three retained packets to the supervisor's external proof. The
    // observation document's freshly computed digest is only a data-integrity
    // check; its binding is always deserialized from the retained proof.
    let call_directory = collection_output.join("d").join(format!("call-{slot:04}"));
    let invocation_bytes =
        read_bytes(&call_directory.join("invocation.json")).map_err(|error| error.to_string())?;
    let stdout_bytes =
        read_bytes(&call_directory.join("initial.stdout")).map_err(|error| error.to_string())?;
    let observation_bytes =
        read_bytes(&call_directory.join("observation.json")).map_err(|error| error.to_string())?;
    let binding: MakeObservationBinding = serde_json::from_value(
        proof_invocation
            .get("binding")
            .ok_or("retained call proof has no external observation binding")?
            .clone(),
    )
    .map_err(|_| "retained external observation binding is malformed")?;
    if sha256_bytes(&invocation_bytes) != binding.invocation_sha256
        || sha256_bytes(&stdout_bytes) != binding.stdout_sha256
    {
        return Err("retained invocation or stdout differs from external binding".into());
    }
    let parsed_invocation = NativeMakeInvocation::parse(&invocation_bytes)
        .map_err(|_| "retained raw invocation is malformed")?;
    let proof_cwd = proof_invocation["cwd"]
        .as_str()
        .ok_or("retained call proof has no cwd")?;
    let proof_argv0 = proof_invocation["argv0"]
        .as_str()
        .ok_or("retained call proof has no argv0")?;
    let proof_argv = required_strings(proof_invocation, "argv")?;
    if parsed_invocation.working_directory() != Path::new(proof_cwd)
        || parsed_invocation.argument_zero() != Some(proof_argv0)
        || parsed_invocation.arguments() != proof_argv.as_slice()
    {
        return Err("parsed raw invocation differs from retained call proof".into());
    }
    let observation = RetainedMakeObservation::load(
        &observation_bytes,
        &sha256_bytes(&observation_bytes),
        &binding,
    )?;
    if observation.binding() != &binding
        || observation.initial_stdout() != stdout_bytes.as_slice()
        || observation.invocation() != &parsed_invocation
        || binding.phase != MakeObservationPhase::FirstInputPrelude
    {
        return Err("retained observation packet differs from first-input proof".into());
    }

    let source_selection = source_goal_selection(&case)?;
    let profile_name = required_string(source_selection, "source_profile")?;
    let host = required_string(source_selection, "host")?;
    let projection = source_projection(&source_root, profile_name, host, local_source.as_ref())?;
    if direct {
        let derived = projection.select_source_metadata(&source_root, &roots)?;
        if serde_json::to_value(&derived)
            .map_err(|_| "cannot encode independent Source selection")?
            != source_join["source_metadata"]
        {
            return Err("retained preparation differs from independent Source metadata".into());
        }
    }

    // The retained Source hash is the selection. The build digest below is
    // freshly measured only to keep this current retained-data transaction
    // stable; it is not original-parent CAS or expired-owner authentication.
    let source_snapshot =
        MesaSourceSnapshot::capture(&source_root, &source_sha256, local_source.as_ref())?;
    let build_tree_before = measure_tree_content_cas_bounded(
        &build_root,
        tree_limits().map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let current_build_digest = build_tree_before.payload_digest_excluding(None);
    let mut input_roots = vec![
        MakeInputRootSelection {
            root: build_root.clone(),
            content_sha256: current_build_digest.clone(),
        },
        MakeInputRootSelection {
            root: compiler_root.clone(),
            content_sha256: compiler_sha256.clone(),
        },
    ];
    let snapshot = if let Some(local) = &local_source {
        MakeInputSnapshot::capture_with_local_source(
            parsed_invocation.working_directory(),
            &source_root,
            local,
            &input_roots,
        )?
    } else {
        input_roots.insert(
            0,
            MakeInputRootSelection {
                root: source_root.clone(),
                content_sha256: source_sha256.clone(),
            },
        );
        MakeInputSnapshot::capture(parsed_invocation.working_directory(), &input_roots)?
    };
    if !snapshot.matches_root_selection(&source_root, &source_sha256)
        || !snapshot.matches_root_selection(&build_root, &current_build_digest)
        || !snapshot.matches_root_selection(&compiler_root, &compiler_sha256)
    {
        return Err(
            "Make input snapshot differs from retained source/current build selections".into(),
        );
    }

    let dirs = DirVars::default();
    let relative_dir = Path::new(&generated_owner)
        .parent()
        .ok_or("selected generated owner has no relative directory")?;
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &source_root,
        relative_dir,
        top_directory: &build_root,
        working_directory: parsed_invocation.working_directory(),
    };
    let matched = projection.read_retained_child_source(
        &paths,
        &roots,
        &generated_owner,
        &observation,
        &snapshot,
    )?;
    let matched_coordinates = matched.coordinates();
    if matched_coordinates.generated_owner() != generated_owner
        || matched_coordinates.source_input() != source_input
        || matched_coordinates.goals() != joined_goals.as_slice()
        || (!direct && matched_coordinates.goals() != roots.as_slice())
    {
        return Err("interpreted child coordinates differ from retained selections".into());
    }
    let matched_goals = matched_coordinates.goals().to_vec();
    // A separate opt-in advances the data probe into dependency/recipe frames.
    // It never changes the original observed goals or executes a recipe.
    let mut declared_unresolved_inputs = None;
    let mut declared_unparsed_shells = None;
    let mut recipe_input_value = None;
    let mut output_links = None;
    let (read_count, recipe_summary) = match std::env::var_os("AROS_TEST_MESA_TRACE_RECIPES") {
        None => {
            let (_, _, _, reads) = matched.into_parts();
            (reads.reads().len(), None)
        }
        Some(value) if value == "1" => {
            let plan = matched.trace_explicit_recipes(&paths)?;
            let summary = (
                plan.recipes().len(),
                plan.implicit_events().len(),
                plan.non_rule_events().len(),
            );
            (plan.reads()[0].reads().len(), Some(summary))
        }
        Some(value) if value == "declared-recipes" || value == "recipe-inputs" => {
            let projected = matched.trace_declared_rules()?.project_recipes(&paths)?;
            let declared = projected.declared();
            declared_unresolved_inputs = Some(declared.traversal().unresolved_inputs().count());
            // This is an inert data projection. Unsupported shell expressions
            // remain held by the projection, never dropped or admitted as argv.
            declared_unparsed_shells = Some(
                projected
                    .recipes()
                    .iter()
                    .filter(|recipe| recipe.expression().parse_literal_shell().is_err())
                    .count(),
            );
            let summary = (
                projected.recipes().len(),
                declared.implicit_events().len(),
                declared.non_rule_events().len(),
            );
            if value == "recipe-inputs" {
                use aros_common::captured_programs::{CapturedProgramSet, ProgramRootSelection};
                let selection_path = PathBuf::from(
                    std::env::var_os("AROS_TEST_MESA_RECIPE_INPUT_SELECTION")
                        .ok_or("explicit recipe input selection is absent")?,
                );
                if !selection_path.is_absolute() {
                    return Err("recipe input selection path is not absolute".into());
                }
                let selection: RecipeInputSelection = serde_json::from_slice(
                    &read_bytes(&selection_path).map_err(|error| error.to_string())?,
                )
                .map_err(|_| "recipe input selection is malformed")?;
                if selection.aliases.len() + selection.programs.len() > 16
                    || selection
                        .programs
                        .iter()
                        .any(|program| program.root_index != 1)
                    || selection.command_role.as_ref().is_some_and(|role| {
                        role.is_empty()
                            || !selection
                                .programs
                                .iter()
                                .any(|program| program.role == *role)
                    })
                    || selection.aliases.iter().any(|selected| {
                        selected.program.root_index != 1
                            || selected.alias.alias_root_index != 0
                            || selected.alias.role != selected.program.role
                    })
                {
                    return Err("recipe alias selection differs from selected root roles".into());
                }
                // Root identities come from the already checked compiler and
                // current build selections, not this descriptor. Canonical
                // roles never replace the original recipe spellings.
                let programs = CapturedProgramSet::capture(
                    &[
                        ProgramRootSelection {
                            root: build_root.clone(),
                            content_sha256: current_build_digest.clone(),
                        },
                        ProgramRootSelection {
                            root: compiler_root,
                            content_sha256: compiler_sha256,
                        },
                    ],
                    &selection
                        .programs
                        .iter()
                        .cloned()
                        .chain(
                            selection
                                .aliases
                                .iter()
                                .map(|selected| selected.program.clone()),
                        )
                        .collect::<Vec<_>>(),
                )?;
                let aliases: Vec<_> = selection
                    .aliases
                    .iter()
                    .map(|selected| {
                        programs
                            .bind_alias(&selected.alias)
                            .map(|alias| (selected.word, alias))
                    })
                    .collect::<Result<_, _>>()?;
                recipe_input_value = Some(bind_selected_recipe_inputs(
                    &projected, &programs, &selection, &aliases,
                )?);
            }
            projected.verify()?;
            (projected.input_reads().reads().len(), Some(summary))
        }
        Some(value) if value == "declared-inputs" => {
            let (coordinates, _program, file, reads) = matched.into_parts();
            let interpreted = aros_transpiler::make_interpreted_rules::collect(&file)?;
            let projected = interpreted.project_explicit()?;
            let goals: Vec<_> = coordinates.goals().iter().map(String::as_str).collect();
            let declared = aros_transpiler::make_rule_graph::traversal::trace_declared(
                &projected.graph,
                &goals,
            )?;
            snapshot.transaction(|reader| {
                for (_, visit) in declared.unresolved_inputs() {
                    let kind = reader.entry_kind(visit.target()).map_err(|error| {
                        format!(
                            "declared input lookup refused for {}: {error}",
                            visit.target()
                        )
                    })?;
                    if kind != Some(aros_common::TreeContentEntryKind::RegularFile) {
                        return Err(format!(
                            "declared input lacks a captured regular file: {}",
                            visit.target()
                        ));
                    }
                }
                Ok(())
            })?;
            // This branch proves declared external inputs only, not recipes.
            (reads.reads().len(), None)
        }
        Some(value) if value == "declared-frontier" || value == "output-links" => {
            let declared = matched.trace_declared_rules()?;
            let trace = declared.traversal();
            declared_unresolved_inputs = Some(trace.unresolved_inputs().count());
            if let Some(expected) = std::env::var_os("AROS_TEST_MESA_EXPECT_DECLARED_INPUT") {
                let expected = expected
                    .to_str()
                    .ok_or("expected declared input is not UTF-8")?;
                let (index, _) = trace
                    .unresolved_inputs()
                    .find(|(_, visit)| visit.target() == expected)
                    .ok_or("expected declared input is absent from original frontier")?;
                let chain = trace.scope_chain(index)?;
                if chain.len() < 2
                    || !joined_goals.iter().any(|goal| chain[0] == goal)
                    || chain.last().copied() != Some(expected)
                {
                    return Err("declared input lost its original goal scope".into());
                }
            }
            let read_count = declared.input_reads().reads().len();
            if value == "output-links" {
                let linked = projection.link_declared_child_outputs(
                    &aros_transpiler::native_owner_projection::DeclaredChildJoinSelection {
                        source_root: &source_root,
                        source_inputs_sha256: &source_sha256,
                        build_root: &build_root,
                        prepared_tree_sha256: &current_build_digest,
                        roots: &roots,
                    },
                    vec![declared],
                )?;
                // This diagnostic requires one original observed child. It
                // must not invent an unseen producer for an external leaf.
                if !linked.candidates().is_empty()
                    || linked.unlinked_inputs().len() != declared_unresolved_inputs.unwrap_or(0)
                {
                    return Err("single-child data acquired a foreign output producer".into());
                }
                linked.verify()?;
                output_links = Some((linked.candidates().len(), linked.unlinked_inputs().len()));
            } else {
                declared.verify()?;
            }
            // No external input or recipe admission follows from this count.
            // Reads were verified by the original reader before construction.
            (read_count, None)
        }
        Some(_) => return Err("unsupported recipe diagnostic selection".into()),
    };

    snapshot.verify()?;
    source_snapshot.verify_selection(&source_root, &source_sha256, local_source.as_ref())?;
    projection.verify(&source_root)?;
    let build_tree_after = measure_tree_content_cas_bounded(
        &build_root,
        tree_limits().map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if build_tree_after != build_tree_before
        || build_tree_after.payload_digest_excluding(None) != current_build_digest
    {
        return Err("current retained build tree changed during child interpretation".into());
    }

    Ok(DiagnosticSummary {
        current_build_digest,
        read_count,
        generated_owner,
        source_input,
        goals: matched_goals,
        recipe_summary,
        declared_unresolved_inputs,
        declared_unparsed_shells,
        recipe_input_value,
        output_links,
    })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeInputSelection {
    target: String,
    recipe: usize,
    command: usize,
    inputs: Vec<RecipeInputWord>,
    value_option: String,
    #[serde(default)]
    programs: Vec<aros_common::captured_programs::ProgramSelection>,
    #[serde(default)]
    command_role: Option<String>,
    #[serde(default)]
    aliases: Vec<RecipeProgramAliasSelection>,
}

// These are consistency measurements only. In particular argv_sha256 is not
// the Source generator's ABI hash, and missing generated inputs remain open.
struct RecipeInputSummary {
    value: (Sha256Digest, usize),
    inputs: usize,
    unresolved_commands: usize,
    aliases: usize,
    argv: (Sha256Digest, usize),
    selected_command_role: bool,
}

impl std::fmt::Debug for RecipeInputSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecipeInputSummary")
            .field("value", &self.value)
            .field("inputs", &self.inputs)
            .field("unresolved_commands", &self.unresolved_commands)
            .field("aliases", &self.aliases)
            .field("argv", &self.argv)
            .field("selected_command_role", &self.selected_command_role)
            .finish()
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeProgramAliasSelection {
    word: usize,
    program: aros_common::captured_programs::ProgramSelection,
    alias: aros_common::captured_programs::ProgramAliasSelection,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeInputWord {
    word: usize,
    spelling: String,
    resolved_path: PathBuf,
    sha256: Sha256Digest,
}

fn bind_selected_recipe_inputs(
    projected: &aros_transpiler::native_owner_projection::DeclaredChildRecipeProjection,
    programs: &aros_common::captured_programs::CapturedProgramSet,
    selection: &RecipeInputSelection,
    aliases: &[(usize, aros_common::captured_programs::BoundProgramAlias<'_>)],
) -> Result<RecipeInputSummary, String> {
    use aros_transpiler::make_shell_data::artifact_binding::{
        RecipeArtifactDirection, RecipeArtifactRequest, RecipeArtifactSlot,
    };
    if selection.target.is_empty()
        || selection.target.len() > 8192
        || selection.recipe >= 65_536
        || selection.command >= 4096
        || (selection.inputs.is_empty() && selection.command_role.is_none())
        || selection.inputs.len() > 16
        || aliases.len() > 16
        || aliases.len() != selection.aliases.len()
        || aliases.iter().any(|(word, _)| *word == 0 || *word > 4096)
        || aliases
            .iter()
            .map(|(word, _)| *word)
            .collect::<BTreeSet<_>>()
            .len()
            != aliases.len()
        || selection.value_option.is_empty()
        || selection.value_option.len() > 128
        || selection.value_option.chars().any(char::is_control)
        || selection.inputs.iter().any(|input| {
            input.word == 0
                || input.word > 4096
                || input.spelling.is_empty()
                || input.spelling.len() > 8192
                || input.spelling.chars().any(char::is_control)
                || !input.resolved_path.is_absolute()
                || input.resolved_path.as_os_str().len() > 8192
        })
    {
        return Err("recipe input selection exceeds bounds or has invalid fields".into());
    }
    let recipe_index = unique_recipe_index(
        projected
            .recipes()
            .iter()
            .enumerate()
            .map(|(index, recipe)| {
                (
                    index,
                    projected.declared().traversal().visits()[recipe.visit()].target(),
                    recipe.recipe(),
                )
            }),
        &selection.target,
        selection.recipe,
    )?;
    let requests: Vec<_> = selection
        .inputs
        .iter()
        .map(|input| RecipeArtifactRequest {
            recipe: 0,
            command: selection.command,
            slot: RecipeArtifactSlot::Word {
                index: input.word,
                range: 0..input.spelling.len(),
            },
            direction: RecipeArtifactDirection::Input,
        })
        .collect();
    let (value, _) = projected.with_recipe_input_bindings(
        recipe_index,
        programs,
        &requests,
        |bindings, artifacts| {
            let command = selected_recipe_command(
                bindings,
                selection.command,
                selection.command_role.as_deref(),
            )?;
            if artifacts.len() != selection.inputs.len() {
                return Err("selected recipe input count differs".into());
            }
            for (input, artifact) in selection.inputs.iter().zip(artifacts) {
                if !recipe_input_matches(
                    input,
                    command.words().get(input.word),
                    artifact.path(),
                    artifact.input_sha256(),
                ) {
                    return Err("selected recipe input differs from independent selection".into());
                }
            }
            // Each proof was constructed above from this exact `programs`
            // object. The enclosing program-set transaction rechecks all roots
            // and canonical roles after this private-data callback too.
            for ((word, alias), selected) in aliases.iter().zip(&selection.aliases) {
                if !recipe_alias_word_matches(alias.invocation_path(), command.words().get(*word))
                    || alias.program().role() != selected.program.role
                {
                    return Err(
                        "recipe alias differs from original argument or selected role".into(),
                    );
                }
            }
            let value = unique_recipe_option_value(command.words(), &selection.value_option)?;
            Ok(RecipeInputSummary {
                value: (sha256_bytes(value.as_bytes()), value.len()),
                inputs: artifacts.len(),
                unresolved_commands: bindings.unresolved().count(),
                aliases: aliases.len(),
                argv: recipe_argv_measurement(command.words())?,
                selected_command_role: selection.command_role.is_some(),
            })
        },
    )?;
    Ok(value)
}

fn selected_recipe_command<'a>(
    bindings: &aros_transpiler::make_shell_data::program_binding::RecipeProgramBindings<'a>,
    index: usize,
    role: Option<&str>,
) -> Result<&'a aros_transpiler::make_shell_data::LiteralRecipeCommand, String> {
    let selected = bindings
        .commands()
        .get(index)
        .ok_or("selected recipe command is absent")?;
    if let Some(role) = role {
        if bindings.unique_role(role)?.position() != selected.position() {
            return Err(
                "selected recipe command coordinate differs from unique program role".into(),
            );
        }
    }
    Ok(selected.command())
}

fn recipe_argv_measurement(words: &[String]) -> Result<(Sha256Digest, usize), String> {
    // A framed complete vector preserves argv[0], order, empty words and
    // duplicates. No compiler vocabulary or source/output slot is inferred.
    let bytes = serde_json::to_vec(words).map_err(|_| "cannot frame selected recipe argv")?;
    Ok((sha256_bytes(&bytes), words.len()))
}

#[test]
fn recipe_argv_measurement_keeps_complete_word_boundaries_and_order() {
    let words = |values: &[&str]| {
        values
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>()
    };
    let original = words(&[
        "/compiler",
        "-DA",
        "",
        "-UA",
        "-DA",
        "-c",
        "generated.c",
        "-o",
        "object.o",
    ]);
    let measured = recipe_argv_measurement(&original).unwrap();
    assert_eq!(measured.1, original.len());
    for changed in [
        words(&[
            "/other",
            "-DA",
            "",
            "-UA",
            "-DA",
            "-c",
            "generated.c",
            "-o",
            "object.o",
        ]),
        words(&[
            "/compiler",
            "-DA",
            "-UA",
            "-DA",
            "-c",
            "generated.c",
            "-o",
            "object.o",
        ]),
        words(&[
            "/compiler",
            "-UA",
            "",
            "-DA",
            "-DA",
            "-c",
            "generated.c",
            "-o",
            "object.o",
        ]),
        words(&[
            "/compiler",
            "-DA",
            "",
            "-UA",
            "-c",
            "generated.c",
            "-o",
            "object.o",
        ]),
        words(&[
            "/compiler",
            "-DA ",
            "",
            "-UA",
            "-DA",
            "-c",
            "generated.c",
            "-o",
            "object.o",
        ]),
    ] {
        assert_ne!(measured.0, recipe_argv_measurement(&changed).unwrap().0);
    }
    assert_ne!(
        recipe_argv_measurement(&words(&["ab", "c"])).unwrap().0,
        recipe_argv_measurement(&words(&["a", "bc"])).unwrap().0
    );
}

#[test]
fn recipe_command_selection_requires_the_exact_unique_role_coordinate() {
    use aros_common::captured_programs::{
        CapturedProgramSet, ProgramRootSelection, ProgramSelection,
    };
    use aros_transpiler::make_shell_data::{
        parse_literal_recipe, program_binding::bind_literal_programs,
    };
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let compiler = root.join("compiler");
    let bytes = b"opaque selected compiler fixture; never executed";
    std::fs::write(&compiler, bytes).unwrap();
    std::fs::set_permissions(&compiler, std::fs::Permissions::from_mode(0o700)).unwrap();
    let programs = CapturedProgramSet::capture(
        &[ProgramRootSelection {
            content_sha256: aros_common::measure_tree_content_cas(&root)
                .unwrap()
                .payload_digest_excluding(None),
            root,
        }],
        &[ProgramSelection {
            role: "compiler".into(),
            root_index: 0,
            invocation_path: compiler.clone(),
            executable_sha256: sha256_bytes(bytes),
        }],
    )
    .unwrap();
    let recipe = parse_literal_recipe(&format!(
        "if '{}' -c absent.c -o absent.o > diagnostics; then echo success; else echo failure; fi",
        compiler.display()
    ))
    .unwrap();
    let bindings = bind_literal_programs(&recipe, &programs).unwrap();
    let command = selected_recipe_command(&bindings, 0, Some("compiler")).unwrap();
    assert_eq!(command.words()[0], compiler.to_str().unwrap());
    assert_eq!(command.redirections().len(), 1);
    assert_eq!(bindings.commands().len(), 3);
    assert_eq!(bindings.unresolved().count(), 2);
    assert!(selected_recipe_command(&bindings, 1, Some("compiler")).is_err());
    assert!(selected_recipe_command(&bindings, 0, Some("missing")).is_err());
    assert!(selected_recipe_command(&bindings, 3, Some("compiler")).is_err());
    let ambiguous = parse_literal_recipe(&format!(
        "'{}'; '{}'",
        compiler.display(),
        compiler.display()
    ))
    .unwrap();
    let bindings = bind_literal_programs(&ambiguous, &programs).unwrap();
    assert!(selected_recipe_command(&bindings, 0, Some("compiler")).is_err());
    assert!(!root_has_generated_objects(directory.path()));
}

fn root_has_generated_objects(root: &Path) -> bool {
    ["absent.c", "absent.o", "diagnostics"]
        .iter()
        .any(|name| root.join(name).exists())
}

fn unique_recipe_index<'a>(
    candidates: impl Iterator<Item = (usize, &'a str, usize)>,
    target: &str,
    ordinal: usize,
) -> Result<usize, String> {
    let mut selected = candidates.filter(|(_, name, recipe)| *name == target && *recipe == ordinal);
    let (index, _, _) = selected.next().ok_or("recipe input selection is absent")?;
    if selected.next().is_some() {
        return Err("recipe input selection is ambiguous".into());
    }
    Ok(index)
}

fn recipe_input_matches(
    expected: &RecipeInputWord,
    word: Option<&String>,
    resolved_path: &Path,
    digest: Option<&Sha256Digest>,
) -> bool {
    word == Some(&expected.spelling)
        && resolved_path == expected.resolved_path
        && digest == Some(&expected.sha256)
}

fn recipe_alias_word_matches(invocation: &Path, word: Option<&String>) -> bool {
    word.is_some_and(|word| invocation.as_os_str() == std::ffi::OsStr::new(word))
}

#[test]
fn recipe_input_selection_requires_unique_coordinates_and_all_three_file_bindings() {
    let alias = Path::new("/selected/original-alias");
    assert!(recipe_alias_word_matches(
        alias,
        Some(&"/selected/original-alias".into())
    ));
    assert!(!recipe_alias_word_matches(
        alias,
        Some(&"/selected/canonical-program".into())
    ));
    assert!(!recipe_alias_word_matches(
        alias,
        Some(&"/selected//original-alias".into())
    ));
    assert!(!recipe_alias_word_matches(alias, None));
    let candidates = [(0, "other", 2), (1, "selected", 1), (2, "selected", 2)];
    assert_eq!(
        unique_recipe_index(candidates.into_iter(), "selected", 2).unwrap(),
        2
    );
    assert!(unique_recipe_index(candidates.into_iter(), "absent", 2).is_err());
    assert!(unique_recipe_index(candidates.into_iter(), "selected", 3).is_err());
    assert!(unique_recipe_index(
        [(0, "selected", 2), (1, "selected", 2)].into_iter(),
        "selected",
        2
    )
    .is_err());
    let expected = RecipeInputWord {
        word: 1,
        spelling: "/selected/alias".into(),
        resolved_path: "/selected/file".into(),
        sha256: sha256_bytes(b"original"),
    };
    let wrong = sha256_bytes(b"changed");
    let word = Some(&expected.spelling);
    assert!(recipe_input_matches(
        &expected,
        word,
        &expected.resolved_path,
        Some(&expected.sha256)
    ));
    assert!(!recipe_input_matches(
        &expected,
        None,
        &expected.resolved_path,
        Some(&expected.sha256)
    ));
    assert!(!recipe_input_matches(
        &expected,
        Some(&"/other/alias".into()),
        &expected.resolved_path,
        Some(&expected.sha256)
    ));
    assert!(!recipe_input_matches(
        &expected,
        word,
        Path::new("/other/file"),
        Some(&expected.sha256)
    ));
    assert!(!recipe_input_matches(
        &expected,
        word,
        &expected.resolved_path,
        Some(&wrong)
    ));
    assert!(!recipe_input_matches(
        &expected,
        word,
        &expected.resolved_path,
        None
    ));
}

fn unique_recipe_option_value<'a>(words: &'a [String], option: &str) -> Result<&'a str, String> {
    let mut positions = words
        .iter()
        .enumerate()
        .filter(|(_, word)| word.as_str() == option)
        .map(|(index, _)| index);
    let position = positions
        .next()
        .ok_or("selected recipe value option is absent")?;
    if positions.next().is_some() {
        return Err("selected recipe value option is ambiguous".into());
    }
    words
        .get(position + 1)
        .map(String::as_str)
        .ok_or_else(|| "selected recipe option value is absent".into())
}

#[test]
fn recipe_option_selection_preserves_original_value_and_refuses_ambiguity() {
    let words = |values: &[&str]| {
        values
            .iter()
            .map(|value| (*value).into())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        unique_recipe_option_value(&words(&["tool", "--abi", " -DA  -UA -DA "]), "--abi").unwrap(),
        " -DA  -UA -DA "
    );
    for invalid in [
        words(&["tool", "--other", "value"]),
        words(&["tool", "--abi"]),
        words(&["tool", "--abi", "same", "--abi", "same"]),
    ] {
        assert!(unique_recipe_option_value(&invalid, "--abi").is_err());
    }
}

fn source_projection(
    source_root: &Path,
    profile_name: &str,
    host: &str,
    local_source: Option<&aros_common::local_source::LocalSourceIdentity>,
) -> Result<aros_transpiler::native_owner_projection::NativeOwnerProjection, String> {
    let target_config = TargetProfile::load_config(&source_root.join("aros-targets.toml"))
        .map_err(|error| error.to_string())?;
    let target = target_config
        .targets
        .iter()
        .find(|target| target.name == profile_name)
        .ok_or("selected Source target is absent")?;
    let source_profile =
        SourceMesaPreflightProfile::load(source_root, target).map_err(|error| error.to_string())?;
    let contract_path = target
        .sdk_consumer_contract
        .as_deref()
        .ok_or("selected Source target has no SDK consumer contract")?;
    let contract: Value = serde_json::from_slice(
        &read_bytes(&source_root.join(contract_path)).map_err(|error| error.to_string())?,
    )
    .map_err(|_| "Source SDK contract JSON is malformed")?;
    let projection_path = required_string(&contract, "metamake_projection")?;
    let policy: Value = serde_json::from_slice(
        &read_bytes(&source_root.join(projection_path)).map_err(|error| error.to_string())?,
    )
    .map_err(|_| "Source MetaMake policy JSON is malformed")?;
    let mut configured = String::from_utf8(
        read_bytes(&source_root.join(required_string(&policy, "configuration_source")?))
            .map_err(|error| error.to_string())?,
    )
    .map_err(|_| "Source MetaMake configuration is not UTF-8")?;
    for substitution in policy["substitutions"]
        .as_array()
        .ok_or("Source MetaMake policy has no substitutions")?
    {
        let token = required_string(substitution, "name")?;
        if !configured.contains(token) {
            return Err("Source MetaMake substitution token is absent".into());
        }
        configured = configured.replace(token, required_string(substitution, "value")?);
    }
    let expected_project =
        ConfiguredProject::parse(&configured, required_string(&policy, "project")?)?;
    let excluded = local_source
        .map(|local| BTreeSet::from([local.generated_subtree.clone()]))
        .unwrap_or_default();
    let logical_files = discover_meta_inputs(
        source_root,
        &expected_project.ignored_directories,
        &excluded,
    )?
    .into_keys()
    .map(|logical| source_root.join(logical))
    .collect::<Vec<_>>();
    source_profile
        .native_owner_projection(&logical_files, host, &excluded, local_source)
        .map_err(|error| error.to_string())
}

#[test]
#[ignore = "requires explicit real Source identity and requested Source goals; metadata only"]
fn actual_source_goal_roster_diagnostic() {
    if let Err(error) = source_goal_roster() {
        let _ = preserve_error_privately(&error);
        panic!("Source goal roster diagnostic failed");
    }
}

fn source_goal_roster() -> Result<(), String> {
    let case_path =
        PathBuf::from(std::env::var_os(CASE_ENV).ok_or("explicit retained case path is absent")?);
    if !case_path.is_absolute() {
        return Err("retained case path is not absolute".into());
    }
    let case: Value =
        serde_json::from_slice(&read_bytes(&case_path).map_err(|error| error.to_string())?)
            .map_err(|_| "retained case JSON is malformed")?;
    let source = case_path_value(&case, "source_root")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let retained: Value = serde_json::from_slice(
        &read_bytes(
            &case_path
                .parent()
                .ok_or("retained case path has no parent")?
                .join("native-root-observation.json"),
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|_| "retained root observation is malformed")?;
    let source_hash: Sha256Digest =
        serde_json::from_value(retained["identity"]["source_inputs_sha256"].clone())
            .map_err(|_| "retained source identity is absent")?;
    let local_source = diagnostic_local_source(&case, &source)?;
    let source_snapshot =
        MesaSourceSnapshot::capture(&source, &source_hash, local_source.as_ref())?;
    let join = source_goal_selection(&case)?;
    let projection = source_projection(
        &source,
        required_string(join, "source_profile")?,
        required_string(join, "host")?,
        local_source.as_ref(),
    )?;
    let text = std::env::var("AROS_TEST_MESA_SOURCE_GOALS")
        .map_err(|_| "explicit Source goals are absent")?;
    if text.len() > 8192 {
        return Err("Source goal selection exceeds diagnostic bound".into());
    }
    let goals: Vec<String> =
        serde_json::from_str(&text).map_err(|_| "Source goals are malformed")?;
    if goals.is_empty() || goals.len() > 64 {
        return Err("Source goal count exceeds diagnostic bound".into());
    }
    let selected = projection.select_source_metadata(&source, &goals)?;
    source_snapshot.verify_selection(&source, &source_hash, local_source.as_ref())?;
    println!(
        "qualification=source-goal-metadata-only-not-observed-calls-or-sdk selection={}",
        serde_json::to_string(&selected).map_err(|_| "cannot encode Source goal selection")?
    );
    if !selected.selection().missing_endpoints.is_empty()
        || !selected.selection().unbound_native_endpoints.is_empty()
    {
        return Err("requested Source goal roster has unresolved endpoints".into());
    }
    Ok(())
}

fn source_goal_selection(case: &Value) -> Result<&Value, String> {
    let (collection, direct) = collection_selection(case)?;
    if direct {
        Ok(collection)
    } else {
        collection
            .get("source_join")
            .ok_or_else(|| "retained collection has no Source selection".into())
    }
}

fn collection_selection(case: &Value) -> Result<(&Value, bool), String> {
    match (
        case.get("native_collection"),
        case.get("native_dispatch")
            .and_then(|value| value.get("direct_collection")),
    ) {
        (Some(collection), None) => Ok((collection, false)),
        (None, Some(collection)) => Ok((collection, true)),
        _ => Err("diagnostic requires one explicit native collection selection".into()),
    }
}

fn diagnostic_local_source(
    case: &Value,
    source: &Path,
) -> Result<Option<aros_common::local_source::LocalSourceIdentity>, String> {
    case.get("local_source")
        .filter(|value| !value.is_null())
        .map(|local| {
            aros_common::local_source::LocalSourceIdentity::capture(
                source,
                required_string(local, "preset")?,
            )
        })
        .transpose()
}

fn selected_diagnostic_slot(
    direct: bool,
    calls: u64,
    supplied: Option<&str>,
) -> Result<usize, String> {
    if calls == 0 || calls > 4096 || (!direct && calls != 1) {
        return Err("diagnostic call count is outside selected collection scope".into());
    }
    let Some(text) = supplied else {
        return if direct {
            Err("direct retained diagnostic requires an explicit child slot".into())
        } else {
            Ok(0)
        };
    };
    let slot = text
        .parse::<usize>()
        .map_err(|_| "diagnostic child slot is not a bounded index")?;
    if text.len() > 4 || text != slot.to_string() || slot as u64 >= calls {
        return Err("diagnostic child slot is outside the selected call roster".into());
    }
    Ok(slot)
}

#[test]
fn source_goal_diagnostic_requires_unique_explicit_selection() {
    let root = serde_json::json!({"native_collection":{"source_join":{"host":"root-host"}}});
    let direct =
        serde_json::json!({"native_dispatch":{"direct_collection":{"host":"direct-host"}}});
    assert_eq!(source_goal_selection(&root).unwrap()["host"], "root-host");
    assert_eq!(
        source_goal_selection(&direct).unwrap()["host"],
        "direct-host"
    );
    let both = serde_json::json!({
        "native_collection":root["native_collection"],
        "native_dispatch":direct["native_dispatch"],
    });
    assert!(source_goal_selection(&both).is_err());
    assert!(source_goal_selection(&serde_json::json!({})).is_err());
    let malformed_root_and_direct = serde_json::json!({
        "native_collection":{}, "native_dispatch":direct["native_dispatch"],
    });
    assert!(source_goal_selection(&malformed_root_and_direct).is_err());
    assert!(source_goal_selection(&serde_json::json!({"native_collection":{}})).is_err());
}

#[test]
fn retained_diagnostic_never_guesses_a_direct_child_slot() {
    assert_eq!(selected_diagnostic_slot(false, 1, None).unwrap(), 0);
    assert_eq!(
        selected_diagnostic_slot(true, 708, Some("673")).unwrap(),
        673
    );
    for (direct, calls, slot) in [
        (true, 708, None),
        (false, 2, None),
        (true, 0, Some("0")),
        (true, 4097, Some("0")),
        (true, 708, Some("708")),
        (true, 708, Some("0673")),
        (true, 708, Some("-1")),
        (true, 708, Some("+1")),
        (true, 708, Some(" 1")),
        (true, 708, Some("99999999999999999999999999999999999999")),
    ] {
        assert!(selected_diagnostic_slot(direct, calls, slot).is_err());
    }
}

fn case_path_value(case: &Value, key: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(required_string(case, key)?);
    if !path.is_absolute() {
        return Err("retained case root is not absolute".into());
    }
    Ok(path)
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

fn tree_limits() -> std::io::Result<TreeTraversalLimits> {
    TreeTraversalLimits::new(MAX_TREE_ENTRIES, MAX_TREE_BYTES)
}

fn preserve_error_privately(error: &str) -> Result<(), String> {
    if error.len() > MAX_ERROR_BYTES {
        return Err("diagnostic error exceeds private retention bound".into());
    }
    let case_path =
        PathBuf::from(std::env::var_os(CASE_ENV).ok_or("explicit retained case path is absent")?);
    if !case_path.is_absolute() {
        return Err("retained case path is not absolute".into());
    }
    let case_bytes = read_bytes(&case_path).map_err(|error| error.to_string())?;
    let case: Value =
        serde_json::from_slice(&case_bytes).map_err(|_| "retained case JSON is malformed")?;
    let case_parent = case_path
        .parent()
        .ok_or("retained case path has no parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let source = case_path_value(&case, "source_root")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let output = case_path_value(&case, "output_root")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let compiler = case_path_value(&case, "compiler_root")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let collection_root = PathBuf::from(required_string(
        collection_selection(&case)?.0,
        "output_root",
    )?)
    .canonicalize()
    .map_err(|error| error.to_string())?;

    let error_path =
        PathBuf::from(std::env::var_os(ERROR_ENV).ok_or("explicit private error path is absent")?);
    if !error_path.is_absolute()
        || error_path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("private error path must be normalized and absolute".into());
    }
    let parent = error_path
        .parent()
        .ok_or("private error path has no parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let name = error_path
        .file_name()
        .ok_or("private error path has no filename")?;
    let resolved_error = parent.join(name);
    for protected in [&case_parent, &source, &output, &compiler, &collection_root] {
        if filesystem_paths_overlap(&resolved_error, protected)
            .map_err(|error| error.to_string())?
        {
            return Err("private error path overlaps retained inputs or evidence".into());
        }
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&resolved_error)
        .map_err(|error| error.to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("private error destination is not a regular file".into());
    }
    file.write_all(error.as_bytes())
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}
