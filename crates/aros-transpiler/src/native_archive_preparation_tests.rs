use super::*;
use crate::ast::MetaTargetRule;
use crate::ast::TargetDefinition;
use crate::graph::private_source_object_owner;
use crate::literal_objects::{LiteralObjectDecl, LiteralObjectGroupDecl};
use crate::metamake_owner_graph::{Limits as MetaMakeLimits, MetaMakeOwnerGraph};
use crate::source_archive_binding::BoundSourceArchive;
use crate::source_archive_command::SourceArchiveCommand;
use crate::source_archive_rules::{ArchiveMembers, SourceArchiveDecl};
use crate::source_compile_rules::SourceCompileGroupDecl;
use crate::{DependencyGraph, TargetContext};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;

const GENERATED_FILE: &str = "fixture/compiler/libfixture/mmakefile";
const RECIPE: &str = "fixture/compiler/libfixture/mmakefile.src";
const FOREIGN_FILE: &str = "fixture/other/mmakefile";
const FOREIGN_RECIPE: &str = "fixture/other/mmakefile.src";
const WRAPPER: &str = "prepare-fixture";
const ARCHIVE: &str = "quick";
const ARCHIVE_OUTPUT: &str = "${AROS_DEVELOPER_LIB_DIR}/libfixture.a";
const OBJECT_OUTPUT: &str = "${AROS_BUILD_DIR}/gen/libfixture/object.o";

struct Fixture {
    _root: tempfile::TempDir,
    projection: NativeOwnerProjection,
    graph: DependencyGraph,
    producer: String,
    recipe: String,
}

fn fixture(prerequisites: &[&str], foreign_wrapper: bool) -> Fixture {
    fixture_with_wrappers(prerequisites, foreign_wrapper, &[])
}

fn fixture_with_wrappers(
    prerequisites: &[&str],
    foreign_wrapper: bool,
    additional_wrappers: &[(&str, &[&str])],
) -> Fixture {
    let temp_root = std::env::temp_dir().canonicalize().unwrap();
    let root = tempfile::tempdir_in(temp_root).unwrap();
    let object_source = "fixture/compiler/libfixture/object.c";
    let object_path = root.path().join(object_source);
    fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    fs::write(&object_path, b"int fixture_object;\n").unwrap();

    let mut wrappers = format!("#MM- {WRAPPER} : {}\n", prerequisites.join(" "));
    for (target, dependencies) in additional_wrappers {
        writeln!(wrappers, "#MM- {target} : {}", dependencies.join(" ")).unwrap();
    }
    let local_source = if foreign_wrapper {
        format!("#MM {ARCHIVE}\n")
    } else {
        format!("{wrappers}#MM {ARCHIVE}\n")
    };
    let sources = if foreign_wrapper {
        vec![
            (GENERATED_FILE, RECIPE, local_source.as_str()),
            (FOREIGN_FILE, FOREIGN_RECIPE, wrappers.as_str()),
        ]
    } else {
        vec![(GENERATED_FILE, RECIPE, local_source.as_str())]
    };

    let mut expanded_files = BTreeMap::new();
    let mut owners = BTreeMap::new();
    let mut snapshots = BTreeMap::new();
    let mut discovered_inputs = BTreeSet::new();
    let mut expanded_bytes = 0;
    for (generated_file, recipe, source) in sources {
        let path = root.path().join(recipe);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source.as_bytes()).unwrap();
        expanded_files.insert(generated_file.to_owned(), source.to_owned());
        owners.insert(generated_file.to_owned(), recipe.to_owned());
        snapshots.insert(
            recipe.to_owned(),
            aros_common::sha256_bytes(source.as_bytes()).to_string(),
        );
        discovered_inputs.insert(recipe.to_owned());
        expanded_bytes += source.len();
    }
    let source_graph = MetaMakeOwnerGraph::parse_expanded_files(
        &expanded_files,
        &BTreeMap::new(),
        MetaMakeLimits {
            preserve_empty_endpoints: true,
            ..MetaMakeLimits::default()
        },
    )
    .unwrap();
    let projection = NativeOwnerProjection {
        sealed_configuration_inputs: BTreeMap::new(),
        architecture_context: TargetContext::default(),
        metamake_globals: BTreeMap::new(),
        graph: source_graph,
        expansion_origins: BTreeMap::new(),
        architecture_hook_families: BTreeSet::new(),
        owners,
        discovered_inputs,
        ignored_directories: BTreeSet::new(),
        excluded_paths: BTreeSet::new(),
        snapshots,
        policy_sha256: "fixture-policy".into(),
        expanded_bytes,
    };

    let object = LiteralObjectDecl {
        source: format!("${{AROS_SOURCE_DIR}}/{object_source}"),
        output: OBJECT_OUTPUT.into(),
        language: "C".into(),
        arguments: Vec::new(),
        line: 8,
    };
    let compile_group = SourceCompileGroupDecl {
        owner: ARCHIVE.into(),
        parent: None,
        file: RECIPE.into(),
        line: 8,
        archive_output: Some(ARCHIVE_OUTPUT.into()),
        objects: vec![object],
    };
    let producer = private_source_object_owner(&compile_group);
    let archive_decl = SourceArchiveDecl {
        owner: ARCHIVE.into(),
        file: RECIPE.into(),
        line: 12,
        owner_line: 2,
        output: ARCHIVE_OUTPUT.into(),
        members: ArchiveMembers::Exact(vec![OBJECT_OUTPUT.into()]),
    };
    let mut graph = DependencyGraph::default();
    graph.literal_object_groups.push(LiteralObjectGroupDecl {
        owner: producer.clone(),
        file: RECIPE.into(),
        line: compile_group.line,
        objects: compile_group.objects.clone(),
    });
    graph.source_archives.push(BoundSourceArchive {
        declaration: archive_decl,
        command: SourceArchiveCommand {
            flags: vec!["cr".into()],
        },
        members: vec![OBJECT_OUTPUT.into()],
        compile_groups: vec![compile_group],
        producer_owners: BTreeSet::new(),
    });
    graph.add_meta_rule(MetaTargetRule {
        name: ARCHIVE.into(),
        dependencies: vec![producer.clone()],
    });

    Fixture {
        _root: root,
        projection,
        graph,
        producer,
        recipe: RECIPE.into(),
    }
}

fn bind(
    fixture: &mut Fixture,
) -> (
    Result<Vec<SourceArchivePreparation>, String>,
    BTreeMap<String, BTreeSet<String>>,
) {
    bind_with_omissions(fixture, &BTreeSet::new())
}

fn bind_with_omissions(
    fixture: &mut Fixture,
    omitted_dependencies: &BTreeSet<(String, String, String)>,
) -> (
    Result<Vec<SourceArchivePreparation>, String>,
    BTreeMap<String, BTreeSet<String>>,
) {
    let selected_endpoints = BTreeSet::from([WRAPPER.to_owned()]);
    bind_with_selection(fixture, omitted_dependencies, &selected_endpoints)
}

fn bind_with_selection(
    fixture: &mut Fixture,
    omitted_dependencies: &BTreeSet<(String, String, String)>,
    selected_endpoints: &BTreeSet<String>,
) -> (
    Result<Vec<SourceArchivePreparation>, String>,
    BTreeMap<String, BTreeSet<String>>,
) {
    let mut parser_origins = BTreeMap::new();
    let context = TargetContext::default();
    let result = fixture.projection.bind_archive_preparations(
        &mut fixture.graph,
        &mut parser_origins,
        omitted_dependencies,
        selected_endpoints,
        &context,
    );
    (result, parser_origins)
}

fn native_consumer_target(name: &str, link_libs: &[&str]) -> TargetDefinition {
    serde_json::from_value(serde_json::json!({
        "mmake_name": name,
        "target_name": name,
        "module_type": "Program",
        "source_files": [],
        "use_libs": [],
        "dependencies": [],
        "dir_path": ".",
        "link_libs": link_libs,
        "compiler_flags": [],
        "include_dirs": [],
        "arch_modules": [],
        "arch_includes": [],
        "defines": [],
        "undefines": [],
        "compile_options": [],
        "arch_sources": [],
        "arch_defines": [],
        "arch_compile_options": [],
        "selection_headers_only": false
    }))
    .unwrap()
}

#[test]
fn ordered_source_wrapper_binds_preparation_to_the_exact_archive_compiler() {
    let mut fixture = fixture(&["includes", ARCHIVE], false);

    let (bindings, parser_origins) = bind(&mut fixture);
    let bindings = bindings.unwrap();

    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].recipe, fixture.recipe);
    assert_eq!(bindings[0].wrapper, WRAPPER);
    assert_eq!(bindings[0].archive, ARCHIVE);
    assert_eq!(bindings[0].producer, fixture.producer);
    assert_eq!(bindings[0].prerequisite, "includes");
    assert!(fixture.graph.meta_targets[&fixture.producer].contains("includes"));
    assert!(fixture
        .graph
        .explicit_meta_edges
        .contains(&(fixture.producer.clone(), "includes".into())));
    assert_eq!(
        parser_origins[&fixture.producer],
        BTreeSet::from([fixture.recipe])
    );
}

#[test]
fn archive_before_preparation_does_not_add_a_compiler_edge() {
    let mut fixture = fixture(&[ARCHIVE, "includes"], false);
    let before_targets = fixture.graph.meta_targets.clone();
    let before_explicit = fixture.graph.explicit_meta_edges.clone();

    let (bindings, parser_origins) = bind(&mut fixture);

    assert!(bindings.unwrap().is_empty());
    assert_eq!(fixture.graph.meta_targets, before_targets);
    assert_eq!(fixture.graph.explicit_meta_edges, before_explicit);
    assert!(parser_origins.is_empty());
}

#[test]
fn a_wrapper_in_a_foreign_source_file_does_not_bind_to_the_archive() {
    let mut fixture = fixture(&["includes", ARCHIVE], true);
    let before_targets = fixture.graph.meta_targets.clone();
    let before_explicit = fixture.graph.explicit_meta_edges.clone();

    let (bindings, parser_origins) = bind(&mut fixture);

    assert!(bindings.unwrap().is_empty());
    assert_eq!(fixture.graph.meta_targets, before_targets);
    assert_eq!(fixture.graph.explicit_meta_edges, before_explicit);
    assert!(parser_origins.is_empty());
}

#[test]
fn only_selected_same_recipe_wrappers_add_preparation_prerequisites() {
    let mut fixture = fixture_with_wrappers(
        &["includes", ARCHIVE],
        false,
        &[("unselected-prepare", &["unselected-input", ARCHIVE])],
    );
    let selected_endpoints = BTreeSet::from([WRAPPER.to_owned()]);

    let (bindings, parser_origins) =
        bind_with_selection(&mut fixture, &BTreeSet::new(), &selected_endpoints);
    let bindings = bindings.unwrap();

    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].wrapper, WRAPPER);
    assert_eq!(bindings[0].prerequisite, "includes");
    let dependencies = &fixture.graph.meta_targets[&fixture.producer];
    assert_eq!(dependencies.len(), 1);
    assert!(dependencies.contains("includes"));
    assert!(!dependencies.contains("unselected-input"));
    assert_eq!(
        parser_origins[&fixture.producer],
        BTreeSet::from([fixture.recipe])
    );
}

#[test]
fn native_consumer_linking_the_archive_closes_a_cycle_before_mutation() {
    let consumer = "fixture-native-consumer";
    let mut fixture = fixture(&[consumer, ARCHIVE], false);
    fixture.graph.targets.insert(
        consumer.into(),
        native_consumer_target(consumer, &[ARCHIVE]),
    );
    let before_targets = fixture.graph.meta_targets.clone();
    let before_explicit = fixture.graph.explicit_meta_edges.clone();
    let mut parser_origins = BTreeMap::new();

    let result = fixture.projection.bind_archive_preparations(
        &mut fixture.graph,
        &mut parser_origins,
        &BTreeSet::new(),
        &BTreeSet::from([WRAPPER.to_owned()]),
        &TargetContext::default(),
    );

    assert!(result.unwrap_err().contains("would form a producer cycle"));
    assert_eq!(fixture.graph.meta_targets, before_targets);
    assert_eq!(fixture.graph.explicit_meta_edges, before_explicit);
    assert_eq!(
        fixture.graph.targets[consumer].link_libs,
        [ARCHIVE.to_owned()]
    );
    assert!(parser_origins.is_empty());
}

#[test]
fn native_consumer_not_linking_the_archive_is_a_valid_preparation() {
    let consumer = "fixture-native-consumer";
    let mut fixture = fixture(&[consumer, ARCHIVE], false);
    fixture
        .graph
        .targets
        .insert(consumer.into(), native_consumer_target(consumer, &[]));

    let (bindings, parser_origins) = bind(&mut fixture);
    let bindings = bindings.unwrap();

    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].prerequisite, consumer);
    assert!(fixture.graph.meta_targets[&fixture.producer].contains(consumer));
    assert_eq!(
        parser_origins[&fixture.producer],
        BTreeSet::from([fixture.recipe])
    );
}

#[test]
fn missing_preparation_prerequisite_remains_a_strict_missing_endpoint() {
    let missing = "fixture-generated-preparation";
    let mut fixture = fixture(&[missing, ARCHIVE], false);
    let (bindings, _) = bind(&mut fixture);
    assert!(bindings.is_ok());
    assert!(!fixture.graph.meta_targets.contains_key(missing));

    let report = fixture.graph.audit_native_dependency_graph(
        &[ARCHIVE.into()],
        &TargetContext::default(),
        &[],
    );
    assert!(report
        .missing_endpoints
        .iter()
        .any(|endpoint| endpoint.name == missing));
}

#[test]
fn optional_preparation_omission_matches_only_its_recipe_wrapper_and_dependency() {
    let mut fixture = fixture(&["includes", "setup", ARCHIVE], false);
    let omitted_dependencies =
        BTreeSet::from([(fixture.recipe.clone(), WRAPPER.into(), "includes".into())]);

    let (bindings, parser_origins) = bind_with_omissions(&mut fixture, &omitted_dependencies);
    let bindings = bindings.unwrap();

    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].prerequisite, "setup");
    let dependencies = &fixture.graph.meta_targets[&fixture.producer];
    assert_eq!(dependencies.len(), 1);
    assert!(dependencies.contains("setup"));
    assert!(!dependencies.contains("includes"));
    assert_eq!(
        parser_origins[&fixture.producer],
        BTreeSet::from([fixture.recipe])
    );
}

#[test]
fn an_omission_for_a_different_recipe_target_or_dependency_does_not_hide_a_missing_endpoint() {
    let missing = "fixture-generated-preparation";
    let wrong_omissions = [
        ("fixture/other/mmakefile.src", WRAPPER, missing),
        (RECIPE, "other-wrapper", missing),
        (RECIPE, WRAPPER, "another-prerequisite"),
    ];

    for (recipe, wrapper, prerequisite) in wrong_omissions {
        let mut fixture = fixture(&[missing, ARCHIVE], false);
        let omitted_dependencies =
            BTreeSet::from([(recipe.into(), wrapper.into(), prerequisite.into())]);

        let (bindings, _) = bind_with_omissions(&mut fixture, &omitted_dependencies);

        let bindings = bindings.unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].prerequisite, missing);
        assert!(fixture.graph.meta_targets[&fixture.producer].contains(missing));
        let report = fixture.graph.audit_native_dependency_graph(
            &[ARCHIVE.into()],
            &TargetContext::default(),
            &[],
        );
        assert!(report
            .missing_endpoints
            .iter()
            .any(|endpoint| endpoint.name == missing));
    }
}

#[test]
fn missing_or_nonmatching_literal_producer_fails_before_mutation() {
    for wrong_producer in [false, true] {
        let mut fixture = fixture(&["includes", ARCHIVE], false);
        if wrong_producer {
            fixture.graph.literal_object_groups[0].file = "fixture/foreign/mmakefile.src".into();
        } else {
            fixture.graph.literal_object_groups.clear();
        }
        let before_targets = fixture.graph.meta_targets.clone();
        let before_explicit = fixture.graph.explicit_meta_edges.clone();
        let mut parser_origins = BTreeMap::new();

        let result = fixture.projection.bind_archive_preparations(
            &mut fixture.graph,
            &mut parser_origins,
            &BTreeSet::new(),
            &BTreeSet::from([WRAPPER.to_owned()]),
            &TargetContext::default(),
        );

        assert!(result
            .unwrap_err()
            .contains("ordered archive preparation has no exact object producer"));
        assert_eq!(fixture.graph.meta_targets, before_targets);
        assert_eq!(fixture.graph.explicit_meta_edges, before_explicit);
        assert!(parser_origins.is_empty());
    }
}

#[test]
fn duplicate_literal_producer_fails_before_mutation() {
    let mut fixture = fixture(&["includes", ARCHIVE], false);
    fixture
        .graph
        .literal_object_groups
        .push(fixture.graph.literal_object_groups[0].clone());
    let before_targets = fixture.graph.meta_targets.clone();
    let before_explicit = fixture.graph.explicit_meta_edges.clone();
    let mut parser_origins = BTreeMap::new();

    let result = fixture.projection.bind_archive_preparations(
        &mut fixture.graph,
        &mut parser_origins,
        &BTreeSet::new(),
        &BTreeSet::from([WRAPPER.to_owned()]),
        &TargetContext::default(),
    );

    assert!(result
        .unwrap_err()
        .contains("ordered archive preparation has no exact object producer"));
    assert_eq!(fixture.graph.meta_targets, before_targets);
    assert_eq!(fixture.graph.explicit_meta_edges, before_explicit);
    assert!(parser_origins.is_empty());
}

#[test]
fn wrong_literal_producer_line_fails_before_mutation() {
    let mut fixture = fixture(&["includes", ARCHIVE], false);
    fixture.graph.literal_object_groups[0].line += 1;
    let before_targets = fixture.graph.meta_targets.clone();
    let before_explicit = fixture.graph.explicit_meta_edges.clone();
    let mut parser_origins = BTreeMap::new();

    let result = fixture.projection.bind_archive_preparations(
        &mut fixture.graph,
        &mut parser_origins,
        &BTreeSet::new(),
        &BTreeSet::from([WRAPPER.to_owned()]),
        &TargetContext::default(),
    );

    assert!(result
        .unwrap_err()
        .contains("ordered archive preparation has no exact object producer"));
    assert_eq!(fixture.graph.meta_targets, before_targets);
    assert_eq!(fixture.graph.explicit_meta_edges, before_explicit);
    assert!(parser_origins.is_empty());
}

#[test]
fn repeated_and_multiple_prefixes_bind_once_per_distinct_prerequisite() {
    let mut fixture = fixture(&["includes", "includes", "setup", ARCHIVE], false);

    let (bindings, parser_origins) = bind(&mut fixture);
    let bindings = bindings.unwrap();

    assert_eq!(bindings.len(), 2);
    assert_eq!(
        bindings
            .iter()
            .map(|binding| binding.prerequisite.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["includes", "setup"])
    );
    let dependencies = &fixture.graph.meta_targets[&fixture.producer];
    assert_eq!(dependencies.len(), 2);
    assert!(dependencies.contains("includes"));
    assert!(dependencies.contains("setup"));
    assert_eq!(
        parser_origins[&fixture.producer],
        BTreeSet::from([fixture.recipe])
    );
}

#[test]
fn preparation_that_reaches_its_compiler_is_rejected_without_mutation() {
    let mut fixture = fixture(&["includes", ARCHIVE], false);
    fixture.graph.add_meta_rule(MetaTargetRule {
        name: "includes".into(),
        dependencies: vec![fixture.producer.clone()],
    });
    let before_targets = fixture.graph.meta_targets.clone();
    let before_explicit = fixture.graph.explicit_meta_edges.clone();
    let mut parser_origins = BTreeMap::new();

    let result = fixture.projection.bind_archive_preparations(
        &mut fixture.graph,
        &mut parser_origins,
        &BTreeSet::new(),
        &BTreeSet::from([WRAPPER.to_owned()]),
        &TargetContext::default(),
    );

    assert!(result.unwrap_err().contains("would form a producer cycle"));
    assert_eq!(fixture.graph.meta_targets, before_targets);
    assert_eq!(fixture.graph.explicit_meta_edges, before_explicit);
    assert!(parser_origins.is_empty());
}

#[test]
fn shared_cycle_walks_obey_the_global_work_budget_before_mutation() {
    const PREPARATION_COUNT: usize = 202;
    const SHARED_CHAIN_LENGTH: usize = 19_900;

    let preparation_names = (0..PREPARATION_COUNT)
        .map(|index| format!("fixture-preparation-{index:03}"))
        .collect::<Vec<_>>();
    let prerequisites = preparation_names
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(ARCHIVE))
        .collect::<Vec<_>>();
    let mut fixture = fixture(&prerequisites, false);

    // Every distinct preparation target shares one acyclic path of 19,900
    // endpoints. Each walk stays below the per-walk limit, while their total
    // work crosses the helper's global budget.
    let chain_head = "fixture-preparation-walk-00000";
    for preparation in &preparation_names {
        fixture.graph.add_meta_rule(MetaTargetRule {
            name: preparation.clone(),
            dependencies: vec![chain_head.into()],
        });
    }
    for index in 0..SHARED_CHAIN_LENGTH {
        let name = format!("fixture-preparation-walk-{index:05}");
        let dependencies = (index + 1 < SHARED_CHAIN_LENGTH)
            .then(|| format!("fixture-preparation-walk-{:05}", index + 1))
            .into_iter()
            .collect();
        fixture
            .graph
            .add_meta_rule(MetaTargetRule { name, dependencies });
    }

    let before_targets = fixture.graph.meta_targets.clone();
    let before_explicit = fixture.graph.explicit_meta_edges.clone();
    let mut parser_origins = BTreeMap::new();

    let result = fixture.projection.bind_archive_preparations(
        &mut fixture.graph,
        &mut parser_origins,
        &BTreeSet::new(),
        &BTreeSet::from([WRAPPER.to_owned()]),
        &TargetContext::default(),
    );

    assert!(result
        .unwrap_err()
        .contains("ordered archive preparation work limit exceeded"));
    assert_eq!(fixture.graph.meta_targets, before_targets);
    assert_eq!(fixture.graph.explicit_meta_edges, before_explicit);
    assert!(parser_origins.is_empty());
}
