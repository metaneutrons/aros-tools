use super::*;
use crate::native_owner_projection::DeclaredChildRecipeProjection;
use crate::parser::TargetContext;
use std::{fmt::Write as _, path::Path};

mod captured_inputs;
mod producer_scope;

const MESA26_GCA_SOURCE: &str = "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/mmakefile.src";
const FOREIGN_SOURCE: &str = "arch/arm-native/soc/broadcom/2708/hidd/v3d/mmakefile.src";
const MESA26_GCA_GOAL: &str = "linklibs-gallium_vc4-gca";
const OTHER_PARENT: &str = "gca-parent";
const ARCHIVE_FINALIZER: &str = "archive-finalizer";
const DEEP_TARGET: &str = "deep-target";

#[derive(Clone, Copy)]
enum RecipeGraph {
    GoalOnly,
    DirectArchiveFinalizer,
    DeepLoop,
}

fn mesa26_arm_context() -> TargetContext {
    TargetContext {
        cpu: Some("arm".into()),
        platform: Some("raspi".into()),
        toolchain: Some("llvm".into()),
        cpu32: Some(String::new()),
        use_mmu: Some("1".into()),
        float_abi: Some("hard".into()),
        mesa_version: Some("26.0.0".into()),
        ..TargetContext::default()
    }
}

fn source_program(
    context: &TargetContext,
    other_parent_goal: bool,
    target_overrides: &[(&str, &str)],
    recipe_graph: RecipeGraph,
) -> String {
    let mut source = if other_parent_goal {
        format!(
            "#MM- root : {OTHER_PARENT} {MESA26_GCA_GOAL}\n#MM {OTHER_PARENT}\n#MM {MESA26_GCA_GOAL}\n"
        )
    } else {
        format!("#MM- root : {MESA26_GCA_GOAL}\n#MM {MESA26_GCA_GOAL}\n")
    };

    for name in aros_common::native_build_contract::NATIVE_RESERVED_MAKE_VARIABLES {
        if let Some(value) = context.value_of(name) {
            writeln!(source, "{name} := {value}").expect("writing to String cannot fail");
        }
    }
    source.push_str(
        "export MESA_BINDING_EXPORT := source-export\nSHELL := /bin/sh\n.SHELLFLAGS := -c\n",
    );
    for (name, value) in target_overrides {
        writeln!(source, "{MESA26_GCA_GOAL}: {name} := {value}")
            .expect("writing to String cannot fail");
    }
    if other_parent_goal {
        writeln!(source, "{OTHER_PARENT}: {MESA26_GCA_GOAL}")
            .expect("writing to String cannot fail");
    }
    match recipe_graph {
        RecipeGraph::GoalOnly => writeln!(
            source,
            "{MESA26_GCA_GOAL}:\n\t@printf '%s\\n' '$@' 'captured original recipe'"
        )
        .expect("writing to String cannot fail"),
        RecipeGraph::DirectArchiveFinalizer => writeln!(
            source,
            "{MESA26_GCA_GOAL}: {ARCHIVE_FINALIZER}\n\
             \t@printf '%s\\n' '$@' 'captured GCA recipe'\n\
             {ARCHIVE_FINALIZER}:\n\
             \t@for object in `cat generated.list`; do printf '%s\\n' \"$$object\"; done"
        )
        .expect("writing to String cannot fail"),
        RecipeGraph::DeepLoop => writeln!(
            source,
            "{MESA26_GCA_GOAL}: parent-target\n\
             parent-target: {DEEP_TARGET}\n\
             {DEEP_TARGET}:\n\
             \t@for part in one two; do printf '%s\\n' \"$$part\"; done"
        )
        .expect("writing to String cannot fail"),
    }
    source
}

fn install_case(
    source_input: &str,
    context: TargetContext,
    other_parent_goal: bool,
    target_overrides: &[(&str, &str)],
) -> Fixture {
    install_graph_case(
        source_input,
        context,
        other_parent_goal,
        target_overrides,
        RecipeGraph::GoalOnly,
    )
}

fn install_graph_case(
    source_input: &str,
    context: TargetContext,
    other_parent_goal: bool,
    target_overrides: &[(&str, &str)],
    recipe_graph: RecipeGraph,
) -> Fixture {
    use crate::metamake_owner_graph::{Limits, MetaMakeOwnerGraph};
    use crate::native_owner_projection::expand_owners;

    let mut fixture = Fixture::new();
    let source_file = fixture.source_root.join(source_input);
    require(
        std::fs::create_dir_all(source_file.parent().expect("Source has a parent")),
        "could not create synthetic Mesa source owner",
    );
    require(
        std::fs::write(
            &source_file,
            source_program(&context, other_parent_goal, target_overrides, recipe_graph),
        ),
        "could not write synthetic Mesa Source owner",
    );

    let generated_owner = source_input
        .strip_suffix(".src")
        .expect("synthetic Source owner has the expected suffix")
        .to_owned();
    let relative_directory = Path::new(&generated_owner)
        .parent()
        .expect("generated owner has a directory");
    let working_directory = fixture.build_root.join(relative_directory);
    require(
        std::fs::create_dir_all(&working_directory),
        "could not create synthetic Mesa build owner",
    );
    fixture.working_directory = require(
        working_directory.canonicalize(),
        "could not resolve synthetic Mesa build owner",
    );

    fixture.projection = crate::native_owner_projection::tests::projection_for_discovery(
        &fixture.source_root,
        BTreeSet::new(),
    );
    fixture.projection.architecture_context = context;

    let template = fixture.source_root.join("fixture.tmpl");
    let template_bytes = b"%define common\n%end\n";
    require(
        std::fs::write(&template, template_bytes),
        "could not write synthetic GenMF template",
    );
    let sealed = BTreeMap::from([(
        "fixture.tmpl".into(),
        aros_common::sha256_bytes(template_bytes).to_string(),
    )]);
    let mut snapshots = BTreeMap::new();
    let (expanded, expanded_bytes, origins, program_digests) = expand_owners(
        &fixture.source_root,
        &fixture.projection.owners,
        &fixture.projection.discovery_original_inputs,
        &template,
        &sealed,
        &mut snapshots,
    )
    .expect("synthetic Mesa Source expansion failed");
    fixture.projection.snapshots = snapshots;
    fixture.projection.expanded_program_sha256 = program_digests;
    fixture.projection.expansion_origins = origins;
    fixture.projection.expanded_bytes = expanded_bytes;
    fixture.projection.graph =
        MetaMakeOwnerGraph::parse_expanded_files(&expanded, &BTreeMap::new(), Limits::default())
            .expect("synthetic expanded owner graph was invalid");
    require(
        std::fs::write(
            fixture.working_directory.join("mmakefile"),
            expanded
                .get(&generated_owner)
                .expect("expanded Mesa owner is absent"),
        ),
        "could not write synthetic generated Mesa Makefile",
    );
    fixture
}

fn declared_recipes(
    fixture: &Fixture,
    source_input: &str,
    goals: &[&str],
) -> DeclaredChildRecipeProjection {
    declared_recipes_with_dirs(fixture, source_input, goals, &DirVars::default())
}

fn declared_recipes_with_dirs(
    fixture: &Fixture,
    source_input: &str,
    goals: &[&str],
    dirs: &DirVars,
) -> DeclaredChildRecipeProjection {
    let environment = synthetic_environment();
    let arguments = [
        "-r",
        "-R",
        "-n",
        "-s",
        "--no-print-directory",
        "--file=mmakefile",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(goals.iter().copied().map(str::to_owned))
    .collect::<Vec<_>>();
    let child = require(
        NativeMakeInvocation::new(arguments, fixture.working_directory.clone(), environment),
        "could not construct synthetic Mesa native invocation",
    );
    let (initial_stdout, environment_names) = pre_file_observation(&child);
    let snapshot = fixture.snapshot();
    let generated_owner = source_input
        .strip_suffix(".src")
        .expect("synthetic Source owner has the expected suffix");
    let relative_directory = Path::new(generated_owner)
        .parent()
        .expect("generated owner has a directory");
    let paths = fixture.paths(dirs, relative_directory, &fixture.working_directory);
    read_child(
        &fixture.projection,
        &paths,
        generated_owner,
        &child,
        &initial_stdout,
        &environment_names,
        &snapshot,
    )
    .and_then(MatchedChildSource::trace_declared_rules)
    .and_then(|declared| declared.project_recipes(&paths))
    .unwrap_or_else(|error| {
        panic!(
            "synthetic Mesa recipe projection failed: {error}\n{}",
            std::fs::read_to_string(fixture.working_directory.join("mmakefile")).unwrap()
        )
    })
}

fn binding_refusal_at(
    recipes: &mut DeclaredChildRecipeProjection,
    fixture: &Fixture,
    recipe_index: usize,
) -> String {
    let dirs = DirVars::default();
    let generated_owner = recipes
        .declared()
        .coordinates()
        .source_input()
        .strip_suffix(".src")
        .expect("synthetic Source owner has the expected suffix")
        .to_owned();
    let relative_directory = Path::new(&generated_owner)
        .parent()
        .expect("generated owner has a directory");
    let paths = fixture.paths(&dirs, relative_directory, &fixture.working_directory);
    match recipes.bind_mesa26_original_recipe(&fixture.projection, &paths, recipe_index) {
        Err(error) => error,
        Ok(_) => panic!("Mesa 26 binding unexpectedly accepted a refused recipe"),
    }
}

fn binding_refusal(fixture: &Fixture, source_input: &str, goals: &[&str]) -> String {
    let mut recipes = declared_recipes(fixture, source_input, goals);
    binding_refusal_at(&mut recipes, fixture, 0)
}

fn recipe_index_for_target(recipes: &DeclaredChildRecipeProjection, target: &str) -> usize {
    let matches = recipes
        .recipes()
        .iter()
        .enumerate()
        .filter(|(_, recipe)| {
            recipes.declared().traversal().visits()[recipe.visit()].target() == target
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "selected target recipe is not unique");
    matches[0]
}

#[test]
fn mesa26_gca_binding_retains_the_source_recipe_and_complete_frame() {
    let fixture = install_case(MESA26_GCA_SOURCE, mesa26_arm_context(), false, &[]);
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    assert_eq!(recipes.recipes().len(), 1);
    let dirs = DirVars::default();
    let relative_directory = Path::new(MESA26_GCA_SOURCE)
        .parent()
        .expect("Mesa GCA source has a directory");
    let paths = fixture.paths(&dirs, relative_directory, &fixture.working_directory);

    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .expect("selected Mesa 26 GCA source recipe should bind");
    assert!(
        binding
            .recipe()
            .expression()
            .contains("captured original recipe")
    );
    assert!(binding.recipe().expression().contains(MESA26_GCA_GOAL));
    assert_eq!(
        binding
            .recipe()
            .source_exports()
            .get("MESA_BINDING_EXPORT")
            .map(String::as_str),
        Some("source-export")
    );
    assert_eq!(binding.shell(), "/bin/sh");
    assert_eq!(binding.flags(), "-c");
    binding
        .revalidate()
        .expect("unchanged Source-generated entry should revalidate");
}

#[test]
fn mesa26_gca_binding_allows_the_direct_archive_finalizer_recipe() {
    let fixture = install_graph_case(
        MESA26_GCA_SOURCE,
        mesa26_arm_context(),
        false,
        &[],
        RecipeGraph::DirectArchiveFinalizer,
    );
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let archive_index = recipe_index_for_target(&recipes, ARCHIVE_FINALIZER);
    let dirs = DirVars::default();
    let relative_directory = Path::new(MESA26_GCA_SOURCE)
        .parent()
        .expect("Mesa GCA source has a directory");
    let paths = fixture.paths(&dirs, relative_directory, &fixture.working_directory);

    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, archive_index)
        .expect("direct archive finalizer recipe should bind");
    assert!(binding.recipe().expression().contains("generated.list"));
    assert!(binding.recipe().expression().contains("$object"));
    assert!(binding.recipe().parse_literal_shell().is_err());
}

#[test]
fn mesa26_binding_checks_output_delta_without_renewing_its_original_inputs() {
    let fixture = install_case(MESA26_GCA_SOURCE, mesa26_arm_context(), false, &[]);
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .unwrap();
    std::fs::write(
        fixture.build_root.join("gca-output"),
        "selected synthetic output",
    )
    .unwrap();
    let after = fixture.snapshot();
    binding
        .verify_output_transition(&after, (&[], &["gca-output"], &[]))
        .unwrap();
    assert!(
        binding.revalidate().is_err(),
        "the original tree was not renewed"
    );
    let entry = fixture.working_directory.join("mmakefile");
    let bytes = std::fs::read(&entry).unwrap();
    std::fs::write(&entry, bytes).unwrap();
    let after = fixture.snapshot();
    let relative_entry = entry
        .strip_prefix(&fixture.build_root)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        binding
            .verify_output_transition(&after, (&[relative_entry], &["gca-output"], &[]))
            .unwrap_err()
            .contains("mutate a retained input")
    );
}

#[test]
fn mesa26_binding_refuses_replaced_or_original_non_native_directory_tables() {
    let fixture = install_case(MESA26_GCA_SOURCE, mesa26_arm_context(), false, &[]);
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let native = DirVars::default();
    let foreign = DirVars::from_config_text("CUSTOM := foreign");
    for (initial, later) in [(&native, &foreign), (&foreign, &native)] {
        let mut recipes =
            declared_recipes_with_dirs(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL], initial);
        let paths = fixture.paths(later, relative, &fixture.working_directory);
        let refusal = recipes
            .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
            .err()
            .unwrap();
        assert!(refusal.contains("original native directory context"));
    }
}

#[test]
fn mesa26_binding_distinguishes_owned_output_replacement_from_input_replacement() {
    let fixture = install_case(MESA26_GCA_SOURCE, mesa26_arm_context(), false, &[]);
    std::fs::write(fixture.build_root.join("archive.a"), "old archive").unwrap();
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let pending = temporary.path().join("pending");
    std::fs::write(&pending, "new archive").unwrap();
    std::fs::rename(&pending, fixture.build_root.join("archive.a")).unwrap();
    let after = fixture.snapshot();
    binding
        .verify_output_transition_with_replacements(&after, (&[], &[], &[]), &["archive.a"])
        .unwrap();
    assert!(
        binding
            .verify_output_transition(&after, (&["archive.a"], &[], &[]))
            .is_err()
    );
    let entry = fixture.working_directory.join("mmakefile");
    std::fs::write(&pending, std::fs::read(&entry).unwrap()).unwrap();
    std::fs::rename(&pending, &entry).unwrap();
    let after = fixture.snapshot();
    let relative_entry = entry
        .strip_prefix(&fixture.build_root)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        binding
            .verify_output_transition_with_replacements(
                &after,
                (&[], &[], &[]),
                &["archive.a", relative_entry]
            )
            .unwrap_err()
            .contains("mutate a retained input")
    );
}

#[test]
fn mesa26_gca_binding_rejects_a_deeper_same_owner_loop_recipe() {
    let fixture = install_graph_case(
        MESA26_GCA_SOURCE,
        mesa26_arm_context(),
        false,
        &[],
        RecipeGraph::DeepLoop,
    );
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let deep_index = recipe_index_for_target(&recipes, DEEP_TARGET);

    assert!(
        binding_refusal_at(&mut recipes, &fixture, deep_index)
            .contains("another goal or deeper dependency")
    );
}

#[test]
fn mesa26_gca_binding_rejects_other_policy_and_target_frame_values() {
    let mut mesa20 = mesa26_arm_context();
    mesa20.mesa_version = Some("20.0.8".into());
    let mesa20_fixture = install_case(MESA26_GCA_SOURCE, mesa20, false, &[]);
    assert!(
        binding_refusal(&mesa20_fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL])
            .contains("requires selected Mesa 26 policy")
    );

    let non_mesa_fixture = install_case(MESA26_GCA_SOURCE, TargetContext::default(), false, &[]);
    assert!(
        binding_refusal(&non_mesa_fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL])
            .contains("requires selected Mesa 26 policy")
    );

    let mut non_llvm = mesa26_arm_context();
    non_llvm.toolchain = Some("gcc".into());
    let non_llvm_fixture = install_case(MESA26_GCA_SOURCE, non_llvm, false, &[]);
    assert!(
        binding_refusal(&non_llvm_fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL])
            .contains("does not support target profile")
    );

    let mesa20_frame = install_case(
        MESA26_GCA_SOURCE,
        mesa26_arm_context(),
        false,
        &[("OPT_MESAGL", "20.0.8")],
    );
    assert!(
        binding_refusal(&mesa20_frame, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL])
            .contains("target selectors differ")
    );

    let abi_frame = install_case(
        MESA26_GCA_SOURCE,
        mesa26_arm_context(),
        false,
        &[("GCC_CONFIG_FLOAT_ABI", "soft")],
    );
    assert!(
        binding_refusal(&abi_frame, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL])
            .contains("target selectors differ")
    );

    for value in ["foreign", ""] {
        let unknown_target_selector = install_case(
            MESA26_GCA_SOURCE,
            mesa26_arm_context(),
            false,
            &[("TARGET_RUST_VER", value)],
        );
        assert!(
            binding_refusal(
                &unknown_target_selector,
                MESA26_GCA_SOURCE,
                &[MESA26_GCA_GOAL]
            )
            .contains("unselected target selector")
        );
    }
}

#[test]
fn mesa26_gca_binding_rejects_foreign_owner_and_another_first_parent() {
    let foreign_owner = install_case(FOREIGN_SOURCE, mesa26_arm_context(), false, &[]);
    assert!(
        binding_refusal(&foreign_owner, FOREIGN_SOURCE, &[MESA26_GCA_GOAL])
            .contains("exact Mesa26 GCA owner/goal")
    );

    let other_parent = install_case(MESA26_GCA_SOURCE, mesa26_arm_context(), true, &[]);
    assert!(
        binding_refusal(
            &other_parent,
            MESA26_GCA_SOURCE,
            &[OTHER_PARENT, MESA26_GCA_GOAL]
        )
        .contains("another goal or deeper dependency")
    );
}

#[test]
fn mesa26_gca_binding_revalidates_source_and_generated_entry_drift() {
    let fixture = install_case(MESA26_GCA_SOURCE, mesa26_arm_context(), false, &[]);
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let dirs = DirVars::default();
    let relative_directory = Path::new(MESA26_GCA_SOURCE)
        .parent()
        .expect("Mesa GCA source has a directory");
    let paths = fixture.paths(&dirs, relative_directory, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .expect("selected Mesa 26 GCA source recipe should bind before drift");
    require(
        std::fs::write(
            fixture.source_root.join(MESA26_GCA_SOURCE),
            "#MM changed after binding\n",
        ),
        "could not change synthetic Mesa Source owner",
    );
    assert!(binding.revalidate().is_err());
    drop(binding);

    let generated_drift = install_case(MESA26_GCA_SOURCE, mesa26_arm_context(), false, &[]);
    let mut recipes = declared_recipes(&generated_drift, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    require(
        std::fs::write(
            generated_drift.working_directory.join("mmakefile"),
            "# newly captured foreign generated bytes\n",
        ),
        "could not change synthetic generated Mesa entry",
    );
    let dirs = DirVars::default();
    let relative_directory = Path::new(MESA26_GCA_SOURCE)
        .parent()
        .expect("Mesa GCA source has a directory");
    let paths = generated_drift.paths(
        &dirs,
        relative_directory,
        &generated_drift.working_directory,
    );
    assert!(
        recipes
            .bind_mesa26_original_recipe(&generated_drift.projection, &paths, 0)
            .is_err()
    );
}

#[test]
fn mesa26_gca_binding_rejects_an_extra_goal_even_with_gca_first_parent() {
    let fixture = install_case(MESA26_GCA_SOURCE, mesa26_arm_context(), true, &[]);
    assert!(
        binding_refusal(
            &fixture,
            MESA26_GCA_SOURCE,
            &[MESA26_GCA_GOAL, OTHER_PARENT]
        )
        .contains("exact single GCA goal")
    );
}
