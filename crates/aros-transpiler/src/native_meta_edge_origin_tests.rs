use super::{NativeMetaSemanticsEvidence, NativeMetaSemanticsSelection, NativeOwnerProjection};
use crate::ast::MetaTargetRule;
use crate::genmf_projection::{expand_bytes, Limits as GenmfLimits};
use crate::metamake_owner_graph::{Limits as MetaMakeLimits, MetaMakeOwnerGraph};
use crate::{DependencyGraph, TargetContext};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

const GENERIC_OWNER: &str = "fixture/generic/mmakefile";
const GENERIC_RECIPE: &str = "fixture/generic/mmakefile.src";
const EXTRA_OWNER: &str = "fixture/extra/mmakefile";
const EXTRA_RECIPE: &str = "fixture/extra/mmakefile.src";
const TEMPLATE: &str = "config/make.tmpl";

const EMPTY_LIBRARY_TEMPLATE: &str = include_str!("../tests/fixtures/empty_library_lists.tmpl");

struct Fixture {
    root: tempfile::TempDir,
    projection: NativeOwnerProjection,
    context: TargetContext,
}

fn two_recipe_fixture(extra_dependency: &str) -> Fixture {
    let temp_root = std::env::temp_dir().canonicalize().unwrap();
    let root = tempfile::tempdir_in(temp_root).unwrap();
    fs::create_dir_all(root.path().join("fixture/generic")).unwrap();
    fs::create_dir_all(root.path().join("fixture/extra")).unwrap();
    fs::create_dir_all(root.path().join("config")).unwrap();

    let generic_source = "%build_module mmake=kernel-example modname=example modtype=library\n";
    let extra_source = format!("#MM- kernel-example : {extra_dependency}\n");
    let template_path = root.path().join(TEMPLATE);
    fs::write(&template_path, EMPTY_LIBRARY_TEMPLATE.as_bytes()).unwrap();

    let sources = [
        (GENERIC_OWNER, GENERIC_RECIPE, generic_source),
        (EXTRA_OWNER, EXTRA_RECIPE, extra_source.as_str()),
    ];
    let globals = BTreeMap::from([
        ("ARCH".into(), "esp32p4".into()),
        ("CPU".into(), "riscv".into()),
        ("FAMILY".into(), String::new()),
        ("AROS_TARGET_VARIANT".into(), String::new()),
    ]);
    let mut expanded_files = BTreeMap::new();
    let mut expansion_origins = BTreeMap::new();
    let mut owners = BTreeMap::new();
    let mut snapshots = BTreeMap::new();

    for (owner, recipe, source) in sources {
        let recipe_path = root.path().join(recipe);
        fs::write(&recipe_path, source.as_bytes()).unwrap();
        snapshots.insert(
            recipe.to_owned(),
            aros_common::sha256_bytes(source.as_bytes()).to_string(),
        );
        let expanded = expand_bytes(
            source.as_bytes(),
            &recipe_path,
            &template_path,
            GenmfLimits::default(),
        )
        .unwrap();
        for origin in expanded.provenance {
            let fragment = &expanded.text[origin.output_start_byte..origin.output_end_byte];
            if fragment.starts_with("#MM") {
                assert!(expansion_origins
                    .insert((owner.to_owned(), origin.output_line), origin)
                    .is_none());
            }
        }
        expanded_files.insert(owner.to_owned(), expanded.text);
        owners.insert(owner.to_owned(), recipe.to_owned());
    }

    let graph = MetaMakeOwnerGraph::parse_expanded_files(
        &expanded_files,
        &globals,
        MetaMakeLimits {
            preserve_empty_endpoints: true,
            ..MetaMakeLimits::default()
        },
    )
    .unwrap();
    snapshots.insert(
        TEMPLATE.to_owned(),
        aros_common::sha256_bytes(EMPTY_LIBRARY_TEMPLATE.as_bytes()).to_string(),
    );
    let projection = NativeOwnerProjection {
        sealed_configuration_inputs: BTreeMap::new(),
        architecture_context: TargetContext::default(),
        metamake_globals: std::collections::BTreeMap::new(),
        graph,
        expansion_origins,
        architecture_hook_families: BTreeSet::new(),
        owners,
        discovered_inputs: BTreeSet::from([GENERIC_RECIPE.to_owned(), EXTRA_RECIPE.to_owned()]),
        ignored_directories: BTreeSet::new(),
        excluded_paths: BTreeSet::new(),
        snapshots,
        policy_sha256: "test-policy".into(),
        expanded_bytes: expanded_files.values().map(String::len).sum(),
    };

    Fixture {
        root,
        projection,
        context: TargetContext {
            cpu: Some("riscv".into()),
            platform: Some("esp32p4".into()),
            family: Some(String::new()),
            variant: Some(String::new()),
            toolchain: Some("gnu".into()),
            ..TargetContext::default()
        },
    }
}

fn module_graph() -> (DependencyGraph, Vec<String>) {
    let mut graph = DependencyGraph::default();
    graph.add_meta_rule(MetaTargetRule {
        name: "requested".into(),
        dependencies: vec!["kernel-example".into()],
    });
    // A real native target can receive additional virtual metadata edges.
    graph.add_meta_rule(MetaTargetRule {
        name: "kernel-example".into(),
        dependencies: Vec::new(),
    });
    (graph, vec!["requested".into()])
}

fn bind(
    fixture: &Fixture,
    graph: &mut DependencyGraph,
    roots: &[String],
) -> NativeMetaSemanticsEvidence {
    let mut parser_origins = BTreeMap::new();
    fixture
        .projection
        .bind_native_meta_semantics(
            fixture.root.path(),
            graph,
            NativeMetaSemanticsSelection {
                context: &fixture.context,
                roots,
                declarations: &[],
                diagnostics: &[],
                meta_edge_origins: &BTreeMap::new(),
            },
            &mut parser_origins,
        )
        .unwrap()
}

#[test]
fn imported_empty_library_edge_uses_only_the_recipe_that_claims_it() {
    let fixture = two_recipe_fixture("other-required");
    let (mut graph, roots) = module_graph();

    let evidence = bind(&fixture, &mut graph, &roots);

    assert!(evidence
        .empty_library_list_omissions
        .iter()
        .any(|proof| proof.target == "kernel-example" && proof.dependency == "linklibs-"));
    let dependencies = &graph.meta_targets["kernel-example"];
    assert!(!dependencies.contains("linklibs-"));
    assert!(dependencies.contains("other-required"));
    assert!(graph
        .audit_native_dependency_graph(&roots, &fixture.context, &[])
        .missing_endpoints
        .iter()
        .any(|endpoint| endpoint.name == "other-required"));
}

#[test]
fn handwritten_empty_library_claim_still_vetoes_normalization() {
    let fixture = two_recipe_fixture("linklibs-");
    let (mut graph, roots) = module_graph();

    let evidence = bind(&fixture, &mut graph, &roots);

    assert!(!evidence
        .empty_library_list_omissions
        .iter()
        .any(|proof| proof.target == "kernel-example"));
    assert!(graph.meta_targets["kernel-example"].contains("linklibs-"));
    assert!(graph
        .audit_native_dependency_graph(&roots, &fixture.context, &[])
        .missing_endpoints
        .iter()
        .any(|endpoint| endpoint.name == "linklibs-"));
}
