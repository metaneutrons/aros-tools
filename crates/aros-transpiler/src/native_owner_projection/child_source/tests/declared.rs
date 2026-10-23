use super::*;

mod include_recipe_binding;
mod included_input_reader;
mod included_makefiles;
mod mesa26_binding;
mod reader_composition;

fn install_makefile(fixture: &Fixture, contents: &str) {
    require(
        std::fs::write(fixture.working_directory.join("mmakefile"), contents),
        "could not write synthetic declared-rule Makefile",
    );
}

fn matched_child(fixture: &Fixture) -> MatchedChildSource {
    let environment = synthetic_environment();
    let child = invocation(&fixture.working_directory, &environment);
    let (initial_stdout, environment_names) = pre_file_observation(&child);
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    read_child(
        &fixture.projection,
        &paths,
        "one/mmakefile",
        &child,
        &initial_stdout,
        &environment_names,
        &snapshot,
    )
    .unwrap_or_else(|error| panic!("synthetic child source could not be matched: {error}"))
}

fn declared_child(fixture: &Fixture) -> super::super::MatchedChildDeclaredRules {
    require(
        matched_child(fixture).trace_declared_rules(),
        "synthetic declared rules could not be traced",
    )
}

fn install_source_expanded_program(fixture: &mut Fixture, source: &str) {
    use crate::native_owner_projection::expand_owners;
    let source_file = fixture.source_root.join("one/mmakefile.src");
    std::fs::write(&source_file, source).unwrap();
    let template = fixture.source_root.join("fixture.tmpl");
    let template_bytes = b"%define common\n%end\n";
    std::fs::write(&template, template_bytes).unwrap();
    let sealed = BTreeMap::from([(
        "fixture.tmpl".into(),
        aros_common::sha256_bytes(template_bytes).to_string(),
    )]);
    let mut snapshots = BTreeMap::new();
    let (expanded, _, _, program_digests) = expand_owners(
        &fixture.source_root,
        &fixture.projection.owners,
        &fixture.projection.discovery_original_inputs,
        &template,
        &sealed,
        &mut snapshots,
    )
    .unwrap();
    fixture.projection.snapshots = snapshots;
    fixture.projection.expanded_program_sha256 = program_digests;
    let selected = crate::genmf_projection::expand_files(
        &source_file,
        &template,
        crate::genmf_projection::Limits::default(),
    )
    .unwrap()
    .native_bytes()
    .unwrap();
    assert!(expanded.contains_key("one/mmakefile"));
    std::fs::write(fixture.working_directory.join("mmakefile"), selected).unwrap();
}

#[test]
fn source_generated_recipe_matches_source_expansion_and_refuses_fresh_foreign_bytes() {
    let mut fixture = Fixture::new();
    install_source_expanded_program(
        &mut fixture,
        "#MM root\n# UTF-8 comment — kept byte-for-byte\nroot:\n\t@for o in one two; do printf '%s' \"$$o\"; done\n",
    );
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let original = declared_child(&fixture).project_recipes(&paths).unwrap();
    let body = original
        .with_source_generated_recipe(&fixture.source_root, &fixture.projection, 0, |r, _, _| {
            Ok(r.expression().original_shell_text()?.1.to_owned())
        })
        .unwrap();
    assert_eq!(body, "for o in one two; do printf '%s' \"$o\"; done");

    // A newly captured but altered generated Makefile is still not Source.
    install_makefile(&fixture, "root:\n\t@printf foreign\n");
    let foreign = declared_child(&fixture).project_recipes(&paths).unwrap();
    let error = foreign
        .with_source_generated_recipe(
            &fixture.source_root,
            &fixture.projection,
            0,
            |_, _, _| -> Result<(), String> { panic!("foreign program reached callback") },
        )
        .unwrap_err();
    assert!(error.contains("differs from original Source expansion"));
}

#[test]
fn source_generated_recipe_does_not_admit_included_program_or_source_drift() {
    let mut fixture = Fixture::new();
    install_source_expanded_program(&mut fixture, "#MM root\ninclude child.mk\n");
    std::fs::write(
        fixture.working_directory.join("child.mk"),
        "root:\n\t@printf from-foreign-include\n",
    )
    .unwrap();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let child = declared_child(&fixture).project_recipes(&paths).unwrap();
    let error = child
        .with_source_generated_recipe(
            &fixture.source_root,
            &fixture.projection,
            0,
            |_, _, _| -> Result<(), String> { panic!("foreign include reached callback") },
        )
        .unwrap_err();
    assert!(error.contains("origin is outside"));
    std::fs::write(
        fixture.source_root.join("one/mmakefile.src"),
        "# changed Source\n",
    )
    .unwrap();
    assert!(
        child
            .with_source_generated_recipe(
                &fixture.source_root,
                &fixture.projection,
                0,
                |_, _, _| -> Result<(), String> { panic!("changed Source reached callback") },
            )
            .is_err()
    );
}

#[cfg(unix)]
fn captured_abi_program() -> (
    tempfile::TempDir,
    aros_common::captured_programs::CapturedProgramSet,
    PathBuf,
) {
    use aros_common::{
        captured_programs::{ProgramRootSelection, ProgramSelection},
        measure_tree_content_cas, sha256_bytes,
    };
    use std::os::unix::fs::PermissionsExt;

    const TOOL_BYTES: &[u8] = b"selected ABI tool fixture; never executed";
    let directory = tempfile::tempdir().expect("could not create synthetic program root");
    let root = directory
        .path()
        .canonicalize()
        .expect("could not resolve synthetic program root");
    let tool = root.join("gca-abi-tool");
    std::fs::write(&tool, TOOL_BYTES).expect("could not write synthetic ABI tool");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))
        .expect("could not mark synthetic ABI tool executable");
    let programs = aros_common::captured_programs::CapturedProgramSet::capture(
        &[ProgramRootSelection {
            content_sha256: measure_tree_content_cas(&root)
                .expect("could not measure synthetic program root")
                .payload_digest_excluding(None),
            root,
        }],
        &[ProgramSelection {
            role: "source-abi".into(),
            root_index: 0,
            invocation_path: tool.clone(),
            executable_sha256: sha256_bytes(TOOL_BYTES),
        }],
    )
    .expect("could not capture synthetic ABI program");
    (directory, programs, tool)
}

#[cfg(unix)]
fn recipe_binding_fixture(
    fixture: &Fixture,
    tool: &Path,
) -> (
    PathBuf,
    PathBuf,
    super::super::DeclaredChildRecipeProjection,
) {
    let script = fixture.working_directory.join("abi-script.py");
    let script_bytes = b"ABI script input; never interpreted or executed\n";
    require(
        std::fs::write(&script, script_bytes),
        "could not write synthetic ABI script input",
    );
    let tool_word = tool.to_str().expect("synthetic tool path is not UTF-8");
    let script_word = script.to_str().expect("synthetic script path is not UTF-8");
    install_makefile(
        fixture,
        &format!(
            concat!(
                "ABI_SOURCE := -DGCA_ORIGINAL\n",
                "ABI_FLAGS := $(ABI_SOURCE)\n",
                "ABI_SOURCE = -DGCA_LATER\n",
                "root: stage\n",
                "\t@'{tool}' --abi-flags=\"$(ABI_FLAGS)\" '{script}'\n",
                "stage: generated/deep/missing.h\n",
                "\t@for part in one two; do echo opaque > declared-input-binding-executed; done\n",
            ),
            tool = tool_word,
            script = script_word,
        ),
    );
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let projection = require(
        declared_child(fixture).project_recipes(&paths),
        "declared recipe input fixture could not be projected",
    );
    (
        script,
        fixture
            .working_directory
            .join("declared-input-binding-executed"),
        projection,
    )
}

#[cfg(unix)]
fn declared_recipe_index_for(
    projection: &super::super::DeclaredChildRecipeProjection,
    target: &str,
) -> usize {
    let matches: Vec<_> = projection
        .recipes()
        .iter()
        .enumerate()
        .filter(|(_, recipe)| {
            projection.declared().traversal().visits()[recipe.visit()].target() == target
        })
        .map(|(index, _)| index)
        .collect();
    assert_eq!(matches.len(), 1, "declared recipe selection is not unique");
    matches[0]
}

#[cfg(unix)]
fn recipe_input_request(
    path: &Path,
) -> crate::make_shell_data::artifact_binding::RecipeArtifactRequest {
    use crate::make_shell_data::artifact_binding::{
        RecipeArtifactDirection, RecipeArtifactRequest, RecipeArtifactSlot,
    };

    let word = path.to_str().expect("synthetic input path is not UTF-8");
    RecipeArtifactRequest {
        recipe: 0,
        command: 0,
        slot: RecipeArtifactSlot::Word {
            index: 2,
            range: 0..word.len(),
        },
        direction: RecipeArtifactDirection::Input,
    }
}

#[test]
fn original_recipe_read_keeps_dynamic_text_origin_and_owner_together_without_execution() {
    let fixture = Fixture::new();
    install_makefile(
        &fixture,
        "root:\n\t@for o in one two; do printf '%s' \"$$o\" > never-executed; done\n",
    );
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let projection = declared_child(&fixture).project_recipes(&paths).unwrap();
    assert_eq!(projection.recipes().len(), 1);
    let (expression, input, line, source, owner) = projection
        .with_original_recipe(0, |recipe, origin, coordinates| {
            assert!(recipe.expression().parse_literal_shell().is_err());
            Ok((
                recipe.expression().expression().to_owned(),
                origin.input.clone(),
                origin.source_line,
                coordinates.source_input().to_owned(),
                coordinates.generated_owner().to_owned(),
            ))
        })
        .unwrap();
    assert_eq!(
        expression,
        "@for o in one two; do printf '%s' \"$o\" > never-executed; done"
    );
    assert_eq!(input, "mmakefile");
    assert_eq!(line, 1); // Source positions are zero-based: physical line 2.
    assert_eq!(source, "one/mmakefile.src");
    assert_eq!(owner, "one/mmakefile");
    assert!(!fixture.working_directory.join("never-executed").exists());
    assert!(
        projection
            .with_original_recipe(1, |_, _, _| -> Result<(), String> {
                panic!("absent recipe must not reach callback")
            })
            .is_err()
    );
}

#[test]
fn original_recipe_read_rejects_original_input_drift_before_and_during_callback() {
    let fixture = Fixture::new();
    install_makefile(&fixture, "root:\n\t@printf original\n");
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let projection = declared_child(&fixture).project_recipes(&paths).unwrap();
    let result = projection.with_original_recipe(0, |_, _, _| {
        // Deliberately corrupt only this synthetic fixture after entry.
        install_makefile(&fixture, "root:\n\t@printf changed\n");
        Ok("must not escape the changed transaction")
    });
    assert!(result.is_err());
    assert!(
        projection
            .with_original_recipe(0, |_, _, _| -> Result<(), String> {
                panic!("changed original inputs must not reach callback")
            })
            .is_err()
    );
}

#[test]
fn original_recipe_read_preserves_callback_refusal_without_running_the_recipe() {
    let fixture = Fixture::new();
    install_makefile(&fixture, "root:\n\t@printf not-executed > never-executed\n");
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let projection = declared_child(&fixture).project_recipes(&paths).unwrap();
    let result = projection.with_original_recipe(0, |_, _, _| -> Result<(), String> {
        Err("selected policy refuses this recipe".into())
    });
    assert_eq!(result.unwrap_err(), "selected policy refuses this recipe");
    assert!(!fixture.working_directory.join("never-executed").exists());
}

#[test]
fn declared_trace_retains_absent_nested_leaf_and_its_original_child_scope() {
    let fixture = Fixture::new();
    let marker = fixture.working_directory.join("declared-trace-executed");
    install_makefile(
        &fixture,
        concat!(
            "ABI_SOURCE := -DGCA_ORIGINAL\n",
            "ABI_FLAGS := $(ABI_SOURCE)\n",
            "ABI_SOURCE = -DGCA_LATER\n",
            "root: ABI_FLAGS += -DROOT_FRAME\n",
            "stage: ABI_FLAGS += -DSTAGE_FRAME\n",
            "root: stage\n",
            "\t@echo \"$(ABI_FLAGS)|$@|$<\"\n",
            "stage: generated/deep/missing.h\n",
            "\t@for part in one two; do echo \"$(ABI_FLAGS)|$@|$<\" > declared-trace-executed; done\n",
        ),
    );
    assert!(
        !fixture
            .working_directory
            .join("generated/deep/missing.h")
            .exists(),
        "synthetic nested prerequisite must be absent before tracing"
    );

    let declared = declared_child(&fixture);
    assert_eq!(declared.coordinates().generated_owner(), "one/mmakefile");
    assert_eq!(declared.coordinates().source_input(), "one/mmakefile.src");
    assert_eq!(declared.coordinates().goals(), ["root"]);
    assert!(!declared.file().events.is_empty());
    assert!(declared.implicit_events().is_empty());
    assert!(
        !declared.non_rule_events().is_empty(),
        "source assignments and include policy must remain retained"
    );

    let trace = declared.traversal();
    assert_eq!(trace.goals(), [0]);
    assert_eq!(trace.visits()[0].target(), "root");
    let (leaf_index, leaf) = trace
        .unresolved_inputs()
        .next()
        .expect("absent declared leaf was not retained as unresolved");
    assert_eq!(leaf.target(), "generated/deep/missing.h");
    assert_eq!(trace.unresolved_inputs().count(), 1);
    assert_eq!(
        trace.scope_chain(leaf_index).unwrap(),
        ["root", "stage", "generated/deep/missing.h"]
    );
    assert!(!marker.exists(), "declared tracing executed a recipe");

    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let projection = require(
        declared.project_recipes(&paths),
        "declared recipe expressions could not be projected",
    );
    assert_eq!(
        projection
            .declared()
            .traversal()
            .unresolved_inputs()
            .map(|(_, visit)| visit.target())
            .collect::<Vec<_>>(),
        ["generated/deep/missing.h"]
    );
    let recipe_for = |target: &str| {
        let selected: Vec<_> = projection
            .recipes()
            .iter()
            .filter(|recipe| {
                projection.declared().traversal().visits()[recipe.visit()].target() == target
            })
            .collect();
        assert_eq!(selected.len(), 1, "declared recipe selection is not unique");
        selected[0]
    };
    let root_recipe = recipe_for("root");
    assert_eq!(
        root_recipe.expression().expression(),
        "@echo \"-DGCA_ORIGINAL -DROOT_FRAME|root|stage\""
    );
    assert!(root_recipe.expression().parse_literal_shell().is_ok());
    let stage_recipe = recipe_for("stage");
    assert_eq!(
        stage_recipe.expression().expression(),
        "@for part in one two; do echo \"-DGCA_ORIGINAL -DROOT_FRAME -DSTAGE_FRAME|stage|generated/deep/missing.h\" > declared-trace-executed; done"
    );
    assert!(
        stage_recipe.expression().parse_literal_shell().is_err(),
        "opaque shell syntax was unexpectedly admitted by literal parsing"
    );
    assert!(projection.verify().is_ok());
    assert!(
        !marker.exists(),
        "declared recipe projection executed a recipe"
    );

    let strict = declared_child(&fixture);
    assert!(
        strict.require_captured_recipes(&paths).is_err(),
        "strict recipe upgrade admitted the absent nested input"
    );
    assert!(!marker.exists(), "strict projection executed a recipe");
}

#[test]
fn declared_recipe_projection_rejects_changed_inputs_and_wrong_build_root() {
    let fixture = Fixture::new();
    install_makefile(&fixture, "root:\n\t@echo $@\n");
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);

    let wrong_fixture = Fixture::new();
    let wrong_dirs = DirVars::default();
    let wrong_paths = wrong_fixture.paths(
        &wrong_dirs,
        Path::new("one"),
        &wrong_fixture.working_directory,
    );
    assert!(
        declared_child(&fixture)
            .project_recipes(&wrong_paths)
            .is_err(),
        "declared recipe projection accepted a replacement build root"
    );

    let projection = require(
        declared_child(&fixture).project_recipes(&paths),
        "declared recipe projection could not be captured",
    );
    install_makefile(&fixture, "root:\n\t@echo replacement\n");
    assert!(
        projection.verify().is_err(),
        "declared recipe projection accepted changed original inputs"
    );
}

#[test]
fn declared_projection_retains_opaque_shell_while_strict_projection_rejects_it() {
    let fixture = Fixture::new();
    require(
        std::fs::write(
            fixture.working_directory.join("present.h"),
            b"captured input\n",
        ),
        "could not write synthetic regular input",
    );
    install_makefile(
        &fixture,
        concat!(
            "ABI_FLAGS := -DGCA_ORIGINAL\n",
            "root: stage\n",
            "stage: present.h\n",
            "\t@echo \"$(ABI_FLAGS)|$@|$<\"\n",
            "\t@for part in one two; do echo \"$(ABI_FLAGS)|$@|$<\"; done\n",
        ),
    );
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let projection = require(
        declared_child(&fixture).project_recipes(&paths),
        "declaration-only projection rejected an opaque shell recipe",
    );
    assert_eq!(projection.recipes().len(), 2);
    assert!(
        projection.recipes()[0]
            .expression()
            .parse_literal_shell()
            .is_ok()
    );
    assert!(
        projection.recipes()[1]
            .expression()
            .parse_literal_shell()
            .is_err()
    );
    assert!(
        declared_child(&fixture)
            .require_captured_recipes(&paths)
            .is_err(),
        "strict recipe projection accepted a batch containing opaque shell syntax"
    );
}

#[cfg(unix)]
#[test]
fn declared_recipe_binding_joins_one_literal_generator_and_script_input() {
    use aros_common::sha256_bytes;
    use std::os::unix::fs::PermissionsExt;

    const SCRIPT_BYTES: &[u8] = b"ABI script input; never interpreted or executed\n";
    let fixture = Fixture::new();
    let (_program_directory, programs, tool) = captured_abi_program();
    let (script, marker, projection) = recipe_binding_fixture(&fixture, &tool);
    let recipe_index = declared_recipe_index_for(&projection, "root");
    let requests = [recipe_input_request(&script)];

    assert_eq!(projection.recipes().len(), 2);
    assert_eq!(
        projection
            .declared()
            .traversal()
            .unresolved_inputs()
            .map(|(_, visit)| visit.target())
            .collect::<Vec<_>>(),
        ["generated/deep/missing.h"]
    );
    let stage_recipe = projection
        .recipes()
        .iter()
        .find(|recipe| {
            projection.declared().traversal().visits()[recipe.visit()].target() == "stage"
        })
        .expect("opaque stage recipe was not retained");
    assert!(stage_recipe.expression().parse_literal_shell().is_err());

    let (derived, reads) = projection
        .with_recipe_input_bindings(recipe_index, &programs, &requests, |binding, artifacts| {
            let command = binding.unique_role("source-abi")?;
            let program = command
                .program()
                .ok_or("selected ABI role is not attached to the command")?;
            Ok((
                command.command().words().to_vec(),
                program.role().to_owned(),
                program.invocation_path().to_owned(),
                artifacts
                    .iter()
                    .map(|artifact| (artifact.path().to_owned(), artifact.input_sha256().cloned()))
                    .collect::<Vec<_>>(),
            ))
        })
        .expect("selected declared recipe input binding failed");
    let (words, role, invocation, artifacts) = derived;
    assert_eq!(
        words,
        [
            tool.to_str().unwrap(),
            "--abi-flags=-DGCA_ORIGINAL",
            script.to_str().unwrap(),
        ]
    );
    assert_eq!(role, "source-abi");
    assert_eq!(invocation, tool);
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].0, script);
    assert_eq!(artifacts[0].1, Some(sha256_bytes(SCRIPT_BYTES)));
    assert_eq!(reads.reads().len(), 1);
    assert_eq!(
        std::fs::metadata(&script).unwrap().permissions().mode() & 0o111,
        0,
        "binding a script input must not require executable mode"
    );
    assert!(!marker.exists(), "declared recipe binding ran a recipe");
    assert!(projection.verify().is_ok());
}

#[cfg(unix)]
#[test]
fn declared_recipe_binding_rejects_opaque_invalid_and_output_requests_before_callback() {
    use crate::make_shell_data::{
        artifact_binding::{BoundRecipeArtifact, RecipeArtifactDirection},
        program_binding::RecipeProgramBindings,
    };
    use std::cell::Cell;

    let fixture = Fixture::new();
    let (_program_directory, programs, tool) = captured_abi_program();
    let (script, marker, projection) = recipe_binding_fixture(&fixture, &tool);
    let root_index = declared_recipe_index_for(&projection, "root");
    let stage_index = declared_recipe_index_for(&projection, "stage");
    let requests = [recipe_input_request(&script)];
    let callback_called = Cell::new(false);
    let callback = |_: &RecipeProgramBindings<'_>, _: &[BoundRecipeArtifact]| {
        callback_called.set(true);
        Ok(())
    };

    assert!(
        projection
            .with_recipe_input_bindings(usize::MAX, &programs, &requests, callback)
            .is_err()
    );
    assert!(!callback_called.get());
    assert!(
        projection
            .with_recipe_input_bindings(stage_index, &programs, &requests, callback)
            .is_err()
    );
    assert!(!callback_called.get());

    let mut output_request = requests[0].clone();
    output_request.direction = RecipeArtifactDirection::Output;
    assert!(
        projection
            .with_recipe_input_bindings(root_index, &programs, &[output_request], callback)
            .is_err()
    );
    assert!(!callback_called.get());

    let mut wrong_batch_coordinate = requests[0].clone();
    wrong_batch_coordinate.recipe = 1;
    assert!(
        projection
            .with_recipe_input_bindings(root_index, &programs, &[wrong_batch_coordinate], callback)
            .is_err()
    );
    assert!(!callback_called.get());
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn declared_recipe_binding_rejects_source_and_program_drift_before_callback() {
    use std::cell::Cell;

    let source_fixture = Fixture::new();
    let (_source_program_directory, source_programs, source_tool) = captured_abi_program();
    let (source_script, _, source_projection) =
        recipe_binding_fixture(&source_fixture, &source_tool);
    let source_recipe_index = declared_recipe_index_for(&source_projection, "root");
    let source_requests = [recipe_input_request(&source_script)];
    std::fs::write(
        source_fixture.source_root.join("one/mmakefile.src"),
        "#MM changed after declaration\n",
    )
    .expect("could not change captured Source input");
    let source_callback_called = Cell::new(false);
    assert!(
        source_projection
            .with_recipe_input_bindings(
                source_recipe_index,
                &source_programs,
                &source_requests,
                |_, _| {
                    source_callback_called.set(true);
                    Ok(())
                },
            )
            .is_err()
    );
    assert!(!source_callback_called.get());

    let program_fixture = Fixture::new();
    let (_program_directory, program_set, program_tool) = captured_abi_program();
    let (program_script, _, program_projection) =
        recipe_binding_fixture(&program_fixture, &program_tool);
    let program_recipe_index = declared_recipe_index_for(&program_projection, "root");
    let program_requests = [recipe_input_request(&program_script)];
    std::fs::write(&program_tool, b"changed selected ABI tool fixture")
        .expect("could not change captured ABI program");
    let program_callback_called = Cell::new(false);
    assert!(
        program_projection
            .with_recipe_input_bindings(
                program_recipe_index,
                &program_set,
                &program_requests,
                |_, _| {
                    program_callback_called.set(true);
                    Ok(())
                },
            )
            .is_err()
    );
    assert!(!program_callback_called.get());
}

#[cfg(unix)]
#[test]
fn declared_recipe_binding_withholds_callback_results_after_source_or_program_drift() {
    use std::cell::Cell;

    for (mutate_source, callback_succeeds) in
        [(true, true), (true, false), (false, true), (false, false)]
    {
        let fixture = Fixture::new();
        let (_program_directory, programs, tool) = captured_abi_program();
        let (script, _, projection) = recipe_binding_fixture(&fixture, &tool);
        let recipe_index = declared_recipe_index_for(&projection, "root");
        let requests = [recipe_input_request(&script)];
        let calls = Cell::new(0usize);
        let result =
            projection.with_recipe_input_bindings(recipe_index, &programs, &requests, |_, _| {
                calls.set(calls.get() + 1);
                if mutate_source {
                    std::fs::write(
                        fixture.source_root.join("one/mmakefile.src"),
                        "#MM changed during recipe binding\n",
                    )
                    .map_err(|_| "could not mutate original Source during callback")?;
                } else {
                    std::fs::write(&tool, b"changed ABI tool during callback")
                        .map_err(|_| "could not mutate selected program during callback")?;
                }
                if callback_succeeds {
                    Ok("private-derived-sentinel".to_owned())
                } else {
                    Err("private-callback-error-sentinel".into())
                }
            });

        assert_eq!(calls.get(), 1, "derived-data callback was not called once");
        let error = result.expect_err("input drift allowed a callback result to escape");
        assert!(
            !error.contains("private-derived-sentinel")
                && !error.contains("private-callback-error-sentinel"),
            "callback result escaped after transaction drift"
        );
    }
}

#[test]
fn declared_leaf_that_is_an_original_regular_input_allows_strict_upgrade() {
    let fixture = Fixture::new();
    let marker = fixture.working_directory.join("strict-upgrade-executed");
    require(
        std::fs::create_dir_all(fixture.working_directory.join("generated/deep")),
        "could not create synthetic nested input directory",
    );
    require(
        std::fs::write(
            fixture.working_directory.join("generated/deep/present.h"),
            "synthetic regular input\n",
        ),
        "could not create synthetic regular input",
    );
    install_makefile(
        &fixture,
        concat!(
            "root: generated/deep/present.h\n",
            "\t@touch strict-upgrade-executed\n",
        ),
    );

    let declared = declared_child(&fixture);
    assert_eq!(
        declared
            .traversal()
            .unresolved_inputs()
            .map(|(_, visit)| visit.target())
            .collect::<Vec<_>>(),
        ["generated/deep/present.h"]
    );
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let plan = require(
        declared.require_captured_recipes(&paths),
        "original regular input did not permit strict recipe projection",
    );
    assert_eq!(plan.coordinates().goals(), ["root"]);
    assert_eq!(plan.traversal().visits()[0].target(), "root");
    assert_eq!(plan.recipes().len(), 1);
    assert!(
        !marker.exists(),
        "strict recipe projection executed a recipe"
    );
}

#[test]
fn fresh_snapshots_do_not_replace_the_original_build_or_source_selection() {
    let build_fixture = Fixture::new();
    install_makefile(&build_fixture, "root:\n\t@touch original-build-recipe\n");
    let build_verify = declared_child(&build_fixture);
    let build_upgrade = declared_child(&build_fixture);
    let build_project = declared_child(&build_fixture);
    install_makefile(&build_fixture, "root:\n\t@touch replacement-build-recipe\n");
    let fresh_build_snapshot = build_fixture.snapshot();
    assert!(fresh_build_snapshot.verify().is_ok());
    assert!(build_verify.verify().is_err());
    let build_dirs = DirVars::default();
    let build_paths = build_fixture.paths(
        &build_dirs,
        Path::new("one"),
        &build_fixture.working_directory,
    );
    assert!(build_project.project_recipes(&build_paths).is_err());
    assert!(
        build_upgrade
            .require_captured_recipes(&build_paths)
            .is_err()
    );

    let source_fixture = Fixture::new();
    install_makefile(&source_fixture, "root:\n\t@touch original-source-recipe\n");
    let source_verify = declared_child(&source_fixture);
    let source_upgrade = declared_child(&source_fixture);
    let source_project = declared_child(&source_fixture);
    require(
        std::fs::write(
            source_fixture.source_root.join("one/mmakefile.src"),
            "#MM changed after child capture\n",
        ),
        "could not change the original synthetic source input",
    );
    let fresh_source_snapshot = source_fixture.snapshot();
    assert!(fresh_source_snapshot.verify().is_ok());
    assert!(source_verify.verify().is_err());
    let source_dirs = DirVars::default();
    let source_paths = source_fixture.paths(
        &source_dirs,
        Path::new("one"),
        &source_fixture.working_directory,
    );
    assert!(source_project.project_recipes(&source_paths).is_err());
    assert!(
        source_upgrade
            .require_captured_recipes(&source_paths)
            .is_err()
    );
}

#[test]
fn declared_trace_refuses_dependency_cycles() {
    let fixture = Fixture::new();
    install_makefile(&fixture, "root: stage\nstage: root\n");
    assert!(
        matched_child(&fixture).trace_declared_rules().is_err(),
        "declared tracing accepted a dependency cycle"
    );
}

#[test]
fn existing_child_match_does_not_accept_a_foreign_owner() {
    let fixture = Fixture::new();
    let environment = synthetic_environment();
    let child = invocation(&fixture.working_directory, &environment);
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let foreign_paths = fixture.paths(&dirs, Path::new("two"), &fixture.working_directory);
    assert_static_error(
        read_child(
            &fixture.projection,
            &foreign_paths,
            "two/mmakefile",
            &child,
            b"invalid observation packet",
            &BTreeSet::new(),
            &snapshot,
        ),
        "child cwd differs from selected owner directory",
        "existing child match accepted a foreign owner",
    );
}
