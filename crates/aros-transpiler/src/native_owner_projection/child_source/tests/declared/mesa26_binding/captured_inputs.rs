use super::*;
use crate::metamake_owner_graph::{Limits, MetaMakeOwnerGraph};
use crate::native_owner_projection::expand_owners;
use std::path::Path;

fn install_prerequisite_case(prerequisites: &str, trailing_rules: &str) -> Fixture {
    let mut fixture = install_case(MESA26_GCA_SOURCE, mesa26_arm_context(), false, &[]);
    let source_path = fixture.source_root.join(MESA26_GCA_SOURCE);
    let source = std::fs::read_to_string(&source_path)
        .expect("synthetic Mesa Source owner should be readable");
    let target_header = format!("{MESA26_GCA_GOAL}:\n");
    assert_eq!(source.matches(&target_header).count(), 1);
    let replacement_header = format!("{MESA26_GCA_GOAL}: {prerequisites}\n");
    let source = source.replacen(&target_header, &replacement_header, 1);
    let source = format!("{source}{trailing_rules}");
    std::fs::write(&source_path, source).expect("synthetic Mesa Source owner should be updated");
    refresh_mesa26_source(&mut fixture);
    fixture
}

pub(super) fn refresh_mesa26_source(fixture: &mut Fixture) {
    fixture.projection = crate::native_owner_projection::tests::projection_for_discovery(
        &fixture.source_root,
        BTreeSet::new(),
    );
    fixture.projection.architecture_context = mesa26_arm_context();

    let template = fixture.source_root.join("fixture.tmpl");
    let template_bytes = std::fs::read(&template).expect("synthetic template should be readable");
    let sealed = BTreeMap::from([(
        "fixture.tmpl".into(),
        aros_common::sha256_bytes(&template_bytes).to_string(),
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
    .expect("synthetic Mesa Source expansion should include its prerequisites");
    fixture.projection.snapshots = snapshots;
    fixture.projection.expanded_program_sha256 = program_digests;
    fixture.projection.expansion_origins = origins;
    fixture.projection.expanded_bytes = expanded_bytes;
    fixture.projection.graph =
        MetaMakeOwnerGraph::parse_expanded_files(&expanded, &BTreeMap::new(), Limits::default())
            .expect("synthetic Mesa Source graph should remain valid");
    let generated_owner = MESA26_GCA_SOURCE
        .strip_suffix(".src")
        .expect("synthetic Mesa owner should end in .src");
    std::fs::write(
        fixture.working_directory.join("mmakefile"),
        expanded
            .get(generated_owner)
            .expect("expanded Mesa owner should be present"),
    )
    .expect("synthetic generated Makefile should be updated");
}

fn selected_input_path(name: &str) -> String {
    Path::new(MESA26_GCA_SOURCE)
        .parent()
        .expect("Mesa GCA source has a directory")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

#[test]
fn mesa26_binding_accepts_captured_normal_and_order_only_prerequisites() {
    let fixture = install_prerequisite_case("normal-input normal-input | order-only-input", "");
    for input in ["normal-input", "order-only-input"] {
        std::fs::write(fixture.working_directory.join(input), "captured input")
            .expect("synthetic prerequisite should be written before projection");
    }
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .expect("the exact Mesa 26 Source recipe should bind");

    let output = fixture.working_directory.join(MESA26_GCA_GOAL);
    assert_eq!(
        binding.declared_output_path().unwrap(),
        Some(output.clone())
    );
    assert!(output.starts_with(&fixture.build_root));
    assert!(!output.exists(), "the declared path need not exist");
    assert_eq!(
        binding.captured_prerequisite_paths().unwrap(),
        vec![
            fixture.working_directory.join("normal-input"),
            fixture.working_directory.join("normal-input"),
            fixture.working_directory.join("order-only-input"),
        ],
        "direct prerequisites retain normal/order-only order and repetitions"
    );
    binding
        .verify_captured_prerequisites((&[], &[], &[]), &[])
        .expect("captured regular normal and order-only prerequisites should be accepted");
}

#[test]
fn mesa26_binding_declines_a_non_normalized_direct_recipe_target() {
    let mut fixture = install_graph_case(
        MESA26_GCA_SOURCE,
        mesa26_arm_context(),
        false,
        &[],
        RecipeGraph::DirectArchiveFinalizer,
    );
    let source_path = fixture.source_root.join(MESA26_GCA_SOURCE);
    let source = std::fs::read_to_string(&source_path).unwrap();
    let goal_header = format!("{MESA26_GCA_GOAL}: {ARCHIVE_FINALIZER}\n");
    let archive_header = format!("{ARCHIVE_FINALIZER}:\n");
    assert_eq!(source.matches(&goal_header).count(), 1);
    assert_eq!(source.matches(&archive_header).count(), 1);
    let source = source
        .replacen(&goal_header, &format!("{MESA26_GCA_GOAL}: ../escape\n"), 1)
        .replacen(&archive_header, "../escape:\n", 1);
    std::fs::write(source_path, source).unwrap();
    refresh_mesa26_source(&mut fixture);

    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let escape_index = recipe_index_for_target(&recipes, "../escape");
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, escape_index)
        .expect("the direct dependency recipe should remain bindable as source data");

    assert_eq!(
        binding.declared_output_path().unwrap(),
        None,
        "an unsafe target spelling has no normalized in-root output path"
    );
}

#[test]
fn mesa26_binding_lists_only_reached_recipe_less_direct_aliases() {
    let trailing_rules = ".PHONY: generated-phony\n\
.gca.stamp:\n\
\t@printf '%s\\n' 'stamp'\n\
generated-z: .gca.stamp\n\
generated-a: .gca.stamp\n\
generated-many: .gca.stamp missing-input\n\
generated-order: .gca.stamp | order-only-input\n\
generated-phony: .gca.stamp\n\
generated-own-recipe: .gca.stamp\n\
\t@printf '%s\\n' 'not run'\n\
../unsafe: .gca.stamp\n\
unreached: .gca.stamp\n";
    let fixture = install_prerequisite_case(
        ".gca.stamp generated-z generated-a generated-many generated-order generated-phony generated-own-recipe ../unsafe",
        trailing_rules,
    );
    for input in ["missing-input", "order-only-input"] {
        std::fs::write(fixture.working_directory.join(input), "captured input").unwrap();
    }
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let stamp_index = recipe_index_for_target(&recipes, ".gca.stamp");
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, stamp_index)
        .expect("the direct stamp recipe should bind as source data");

    let cooutputs = binding
        .declared_cooutput_paths()
        .expect("reached direct recipe-less aliases should be projectable");
    assert_eq!(
        cooutputs,
        vec![
            fixture.working_directory.join("generated-a"),
            fixture.working_directory.join("generated-z"),
        ],
        "only normalized, reached, recipe-less single-prerequisite aliases are listed in sorted order"
    );
    assert!(
        cooutputs.iter().all(|path| !path.exists()),
        "cooutput paths do not claim that aliases were written"
    );
}

#[test]
fn mesa26_binding_rejects_an_absent_phony_prerequisite_as_uncaptured() {
    let fixture = install_prerequisite_case("absent-input", ".PHONY: absent-input\n");
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .expect("the exact Mesa 26 Source recipe should bind");

    let refusal = binding
        .verify_captured_prerequisites((&[], &[], &[]), &[])
        .expect_err("an absent prerequisite must not count as a captured file");
    assert!(refusal.contains("prerequisite is phony"), "{refusal}");
    let path_refusal = binding
        .captured_prerequisite_paths()
        .expect_err("an absent phony prerequisite must not produce a captured path");
    assert!(
        path_refusal.contains("prerequisite is phony"),
        "{path_refusal}"
    );
}

#[test]
fn mesa26_binding_rejects_a_present_regular_prerequisite_marked_phony_later() {
    let fixture = install_prerequisite_case(
        "held-input",
        "PHONY_NAME := held-input\n.PHONY: $(PHONY_NAME)\n",
    );
    std::fs::write(
        fixture.working_directory.join("held-input"),
        "real file exists",
    )
    .unwrap();
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .unwrap();
    let refusal = binding
        .verify_captured_prerequisites((&[], &[], &[]), &[])
        .unwrap_err();
    assert!(refusal.contains("prerequisite is phony"), "{refusal}");
    let path_refusal = binding.captured_prerequisite_paths().unwrap_err();
    assert!(
        path_refusal.contains("prerequisite is phony"),
        "{path_refusal}"
    );
}

#[cfg(unix)]
#[test]
fn mesa26_binding_rejects_a_symbolic_link_prerequisite() {
    let fixture = install_prerequisite_case("linked-input", "");
    std::fs::write(
        fixture.working_directory.join("real-input"),
        "captured target",
    )
    .expect("synthetic link target should be written");
    std::os::unix::fs::symlink("real-input", fixture.working_directory.join("linked-input"))
        .expect("synthetic prerequisite link should be created");
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .expect("the exact Mesa 26 Source recipe should bind");

    let refusal = binding
        .verify_captured_prerequisites((&[], &[], &[]), &[])
        .expect_err("a symbolic link prerequisite must not count as a regular file");
    assert!(
        refusal.contains("not a captured regular input"),
        "{refusal}"
    );
    let path_refusal = binding.captured_prerequisite_paths().unwrap_err();
    assert!(
        path_refusal.contains("not a captured regular input"),
        "{path_refusal}"
    );
}

#[test]
fn mesa26_binding_rejects_a_directory_prerequisite() {
    let fixture = install_prerequisite_case("directory-input", "");
    std::fs::create_dir(fixture.working_directory.join("directory-input"))
        .expect("synthetic directory prerequisite should be created");
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .expect("the exact Mesa 26 Source recipe should bind");

    let refusal = binding
        .verify_captured_prerequisites((&[], &[], &[]), &[])
        .expect_err("a directory prerequisite must not count as a regular file");
    assert!(
        refusal.contains("not a captured regular input"),
        "{refusal}"
    );
    let path_refusal = binding.captured_prerequisite_paths().unwrap_err();
    assert!(
        path_refusal.contains("not a captured regular input"),
        "{path_refusal}"
    );
}

#[test]
fn mesa26_binding_rejects_prerequisites_overlapping_each_output_class() {
    let fixture = install_prerequisite_case("held-input", "");
    std::fs::write(
        fixture.working_directory.join("held-input"),
        "captured input",
    )
    .expect("synthetic prerequisite should be written before projection");
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .expect("the exact Mesa 26 Source recipe should bind");
    let selected_input = selected_input_path("held-input");

    let mutable_refusal = binding
        .verify_captured_prerequisites((&[selected_input.as_str()], &[], &[]), &[])
        .expect_err("a captured prerequisite cannot also be a mutable output");
    assert!(
        mutable_refusal.contains("overlaps a selected output"),
        "{mutable_refusal}"
    );

    let replacement_refusal = binding
        .verify_captured_prerequisites((&[], &[], &[]), &[selected_input.as_str()])
        .expect_err("a captured prerequisite cannot also be replaceable");
    assert!(
        replacement_refusal.contains("overlaps a selected output"),
        "{replacement_refusal}"
    );

    let directory_ancestor = relative.to_str().unwrap();
    let directory_refusal = binding
        .verify_captured_prerequisites((&[], &[], &[directory_ancestor]), &[])
        .expect_err("a fresh directory output cannot contain a captured prerequisite");
    assert!(
        directory_refusal.contains("selected a retained Make input as output"),
        "{directory_refusal}"
    );
}

#[test]
fn mesa26_prerequisite_preflight_rejects_invalid_or_retained_output_selections() {
    let fixture = install_prerequisite_case("held-input", "");
    std::fs::write(
        fixture.working_directory.join("held-input"),
        "captured input",
    )
    .unwrap();
    std::fs::create_dir(fixture.build_root.join("mutable-directory")).unwrap();
    std::fs::write(
        fixture.build_root.join("existing-fresh-output"),
        "already present",
    )
    .unwrap();
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .expect("the exact Mesa 26 Source recipe should bind");

    let unnormalized = binding
        .verify_captured_prerequisites((&["./bad-output"], &[], &[]), &[])
        .unwrap_err();
    assert!(
        unnormalized.contains("not normalized/unique"),
        "{unnormalized}"
    );

    let duplicate = binding
        .verify_captured_prerequisites((&["same-output"], &["same-output"], &[]), &[])
        .unwrap_err();
    assert!(duplicate.contains("not normalized/unique"), "{duplicate}");

    let ancestry = binding
        .verify_captured_prerequisites((&["parent-output"], &[], &["parent-output/child"]), &[])
        .unwrap_err();
    assert!(ancestry.contains("overlap by ancestry"), "{ancestry}");

    let missing_mutable = binding
        .verify_captured_prerequisites((&["missing-mutable-output"], &[], &[]), &[])
        .unwrap_err();
    assert!(
        missing_mutable.contains("mutable output is not a captured regular file"),
        "{missing_mutable}"
    );

    let directory_mutable = binding
        .verify_captured_prerequisites((&["mutable-directory"], &[], &[]), &[])
        .unwrap_err();
    assert!(
        directory_mutable.contains("mutable output is not a captured regular file"),
        "{directory_mutable}"
    );

    let existing_fresh = binding
        .verify_captured_prerequisites((&[], &["existing-fresh-output"], &[]), &[])
        .unwrap_err();
    assert!(
        existing_fresh.contains("fresh output already exists"),
        "{existing_fresh}"
    );

    let retained_makefile = selected_input_path("mmakefile");
    let retained_input = binding
        .verify_captured_prerequisites((&[retained_makefile.as_str()], &[], &[]), &[])
        .unwrap_err();
    assert!(
        retained_input.contains("selected a retained Make input as output"),
        "{retained_input}"
    );
}

#[test]
fn mesa26_binding_no_effect_observation_rejects_changed_captured_input() {
    let fixture = install_prerequisite_case("held-input", "");
    let input = fixture.working_directory.join("held-input");
    std::fs::write(&input, "original captured bytes")
        .expect("synthetic prerequisite should be written before projection");
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .expect("the exact Mesa 26 Source recipe should bind");
    binding
        .verify_captured_prerequisites((&[], &[], &[]), &[])
        .expect("the original prerequisite should be captured before mutation");

    std::fs::write(&input, "changed captured bytes")
        .expect("synthetic captured prerequisite should be changed");
    let refusal = binding
        .observe_output_transition((&[], &[], &[]), &[])
        .expect_err("a no-effect observation must revalidate held inputs");
    assert!(refusal.contains("Make input root changed"), "{refusal}");
}

#[test]
fn mesa26_binding_observes_an_explicitly_selected_archive_replacement() {
    let fixture = install_case(MESA26_GCA_SOURCE, mesa26_arm_context(), false, &[]);
    std::fs::write(fixture.build_root.join("archive.a"), "old archive")
        .expect("synthetic archive should be captured before binding");
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE).parent().unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, 0)
        .expect("the exact Mesa 26 Source recipe should bind");

    let temporary = tempfile::tempdir().expect("synthetic archive staging directory should exist");
    let pending = temporary.path().join("pending");
    std::fs::write(&pending, "new archive").expect("replacement archive should be written");
    std::fs::rename(&pending, fixture.build_root.join("archive.a"))
        .expect("synthetic archive should be replaced");

    binding
        .observe_output_transition((&[], &[], &[]), &["archive.a"])
        .expect("the explicitly selected regular archive replacement should pass its guard");
}
