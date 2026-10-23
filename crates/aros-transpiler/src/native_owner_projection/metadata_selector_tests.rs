use super::tests::projection_for_discovery;
use crate::metamake_owner_graph::{Limits, MetaMakeOwnerGraph};
use crate::native_owner_projection::NativeOwnerProjection;
use crate::parser::TargetContext;
use aros_common::native_build_contract::{NativeMetaAbsence, NativeOptionalMetaDependency};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

const ROOT_TARGET: &str = "root";
const RAW_DEPENDENCY: &str = "optional-$(CPU)";
const CONCRETE_DEPENDENCY: &str = "optional-fixture-cpu";
const CONTRACT_DEPENDENCY: &str = "optional-${AROS_TARGET_CPU}";
const CPU: &str = "fixture-cpu";

struct Fixture {
    root: tempfile::TempDir,
    projection: NativeOwnerProjection,
}

impl Fixture {
    fn new(recipes: &[(&str, &str)]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut graph_files = BTreeMap::new();
        for &(directory, metadata) in recipes {
            let recipe = format!("{directory}/mmakefile.src");
            let owner = format!("{directory}/mmakefile");
            fs::create_dir_all(root.path().join(directory)).unwrap();
            fs::write(root.path().join(&recipe), metadata).unwrap();
            graph_files.insert(owner, metadata.to_owned());
        }

        let mut projection = projection_for_discovery(root.path(), BTreeSet::new());
        // The discovery-only helper intentionally has no source seals. These
        // selector fixtures exercise the source-bound selection API instead.
        projection.snapshots = recipes
            .iter()
            .map(|(directory, metadata)| {
                (
                    format!("{directory}/mmakefile.src"),
                    aros_common::sha256_bytes(metadata.as_bytes()).to_string(),
                )
            })
            .collect();
        projection.architecture_context = TargetContext {
            cpu: Some(CPU.into()),
            platform: Some("fixture-platform".into()),
            ..TargetContext::default()
        };
        projection.graph = MetaMakeOwnerGraph::parse_with_declared_absence(
            &graph_files,
            &BTreeMap::from([("CPU".into(), CPU.into())]),
            &BTreeSet::new(),
            Limits::default(),
        )
        .unwrap();

        Self { root, projection }
    }

    fn select(&self, roots: &[&str]) -> super::SourceMetadataSelection {
        let roots = roots
            .iter()
            .map(|root| (*root).to_owned())
            .collect::<Vec<_>>();
        self.projection
            .select_source_metadata(self.root.path(), &roots)
            .unwrap()
    }
}

fn selector(recipe: &str, target: &str, dependency: &str) -> NativeOptionalMetaDependency {
    NativeOptionalMetaDependency {
        recipe: recipe.into(),
        target: target.into(),
        dependency: dependency.into(),
        absence: NativeMetaAbsence::Selector,
    }
}

fn one_recipe_fixture(metadata: &str) -> Fixture {
    Fixture::new(&[("one", metadata)])
}

#[test]
fn exact_source_selector_omits_a_missing_virtual_leaf_with_explicit_proof() {
    let mut fixture = one_recipe_fixture(&format!("#MM- {ROOT_TARGET} : {RAW_DEPENDENCY}\n"));
    fixture.projection.optional_meta_dependencies = vec![selector(
        "one/mmakefile.src",
        ROOT_TARGET,
        CONTRACT_DEPENDENCY,
    )];

    let selected = fixture.select(&[ROOT_TARGET]);

    assert!(selected.selection().missing_endpoints.is_empty());
    assert_eq!(selected.selector_omissions().len(), 1);
    let omission = &selected.selector_omissions()[0];
    assert_eq!(omission.dependency, CONCRETE_DEPENDENCY);
    assert_eq!(omission.proofs.len(), 1);
    let proof = omission.proofs.iter().next().unwrap();
    assert_eq!(proof.recipe, "one/mmakefile.src");
    assert_eq!(proof.target, ROOT_TARGET);
    assert_eq!(proof.expression, CONTRACT_DEPENDENCY);
}

#[test]
fn missing_virtual_leaf_without_a_source_selector_record_stays_missing() {
    let fixture = one_recipe_fixture(&format!("#MM- {ROOT_TARGET} : {RAW_DEPENDENCY}\n"));

    let selected = fixture.select(&[ROOT_TARGET]);

    assert!(selected
        .selection()
        .missing_endpoints
        .contains(CONCRETE_DEPENDENCY));
    assert!(selected.selector_omissions().is_empty());
}

#[test]
fn selector_omission_requires_exact_recipe_target_expression_and_context() {
    let mismatches = [
        (
            selector("other/mmakefile.src", ROOT_TARGET, CONTRACT_DEPENDENCY),
            CPU,
        ),
        (
            selector("one/mmakefile.src", "another-target", CONTRACT_DEPENDENCY),
            CPU,
        ),
        (
            selector("one/mmakefile.src", ROOT_TARGET, "other-${AROS_TARGET_CPU}"),
            CPU,
        ),
        (
            selector("one/mmakefile.src", ROOT_TARGET, CONTRACT_DEPENDENCY),
            "other-cpu",
        ),
    ];

    for (contract, context_cpu) in mismatches {
        let mut fixture = one_recipe_fixture(&format!("#MM- {ROOT_TARGET} : {RAW_DEPENDENCY}\n"));
        fixture.projection.architecture_context.cpu = Some(context_cpu.into());
        assert_eq!(contract.absence, NativeMetaAbsence::Selector);
        fixture.projection.optional_meta_dependencies = vec![contract];

        let selected = fixture.select(&[ROOT_TARGET]);

        assert!(selected
            .selection()
            .missing_endpoints
            .contains(CONCRETE_DEPENDENCY));
        assert!(selected.selector_omissions().is_empty());
    }
}

#[test]
fn disabled_owner_contract_does_not_omit_a_missing_leaf() {
    let mut fixture = one_recipe_fixture(&format!("#MM- {ROOT_TARGET} : {RAW_DEPENDENCY}\n"));
    let mut contract = selector("one/mmakefile.src", ROOT_TARGET, CONTRACT_DEPENDENCY);
    contract.absence = NativeMetaAbsence::DisabledOwner;
    fixture.projection.optional_meta_dependencies = vec![contract];

    let selected = fixture.select(&[ROOT_TARGET]);

    assert!(selected
        .selection()
        .missing_endpoints
        .contains(CONCRETE_DEPENDENCY));
    assert!(selected.selector_omissions().is_empty());
}

#[test]
fn requested_missing_root_is_never_waived_by_a_selector() {
    let mut fixture = one_recipe_fixture(&format!("#MM- {ROOT_TARGET} : {RAW_DEPENDENCY}\n"));
    fixture.projection.optional_meta_dependencies = vec![selector(
        "one/mmakefile.src",
        ROOT_TARGET,
        CONTRACT_DEPENDENCY,
    )];

    let selected = fixture.select(&[ROOT_TARGET, CONCRETE_DEPENDENCY]);

    assert!(selected
        .selection()
        .missing_endpoints
        .contains(CONCRETE_DEPENDENCY));
    assert!(selected.selector_omissions().is_empty());
}

#[test]
fn every_incoming_edge_needs_its_own_selector_proof() {
    let metadata = format!("#MM- {ROOT_TARGET} : {RAW_DEPENDENCY}\n");
    let mut fixture = Fixture::new(&[("one", &metadata), ("two", &metadata)]);
    fixture.projection.optional_meta_dependencies = vec![selector(
        "one/mmakefile.src",
        ROOT_TARGET,
        CONTRACT_DEPENDENCY,
    )];

    let selected = fixture.select(&[ROOT_TARGET]);

    assert!(selected
        .selection()
        .missing_endpoints
        .contains(CONCRETE_DEPENDENCY));
    assert!(selected.selector_omissions().is_empty());
}

#[test]
fn mandatory_physical_declaration_cannot_use_a_selector_omission() {
    let mut fixture = one_recipe_fixture(&format!("#MM {ROOT_TARGET} : {RAW_DEPENDENCY}\n"));
    fixture.projection.optional_meta_dependencies = vec![selector(
        "one/mmakefile.src",
        ROOT_TARGET,
        CONTRACT_DEPENDENCY,
    )];

    let selected = fixture.select(&[ROOT_TARGET]);

    assert!(selected
        .selection()
        .missing_endpoints
        .contains(CONCRETE_DEPENDENCY));
    assert!(selected.selector_omissions().is_empty());
}

#[test]
fn bare_marker_does_not_create_a_selector_omission() {
    let metadata = format!("#MM-\n{ROOT_TARGET} : {RAW_DEPENDENCY}\n");
    let mut fixture = one_recipe_fixture(&metadata);
    fixture.projection.optional_meta_dependencies = vec![selector(
        "one/mmakefile.src",
        ROOT_TARGET,
        CONTRACT_DEPENDENCY,
    )];
    let declaration = fixture.projection.graph.declarations().first().unwrap();
    assert!(declaration.bare_marker);
    assert!(declaration.dependencies.is_empty());

    let selected = fixture.select(&[ROOT_TARGET]);

    assert!(selected.selection().missing_endpoints.is_empty());
    assert!(selected.selector_omissions().is_empty());
}

#[test]
fn present_physical_endpoint_remains_selected_and_is_not_omitted() {
    let metadata = format!("#MM- {ROOT_TARGET} : {RAW_DEPENDENCY}\n#MM {CONCRETE_DEPENDENCY}\n");
    let mut fixture = one_recipe_fixture(&metadata);
    fixture.projection.optional_meta_dependencies = vec![selector(
        "one/mmakefile.src",
        ROOT_TARGET,
        CONTRACT_DEPENDENCY,
    )];

    let selected = fixture.select(&[ROOT_TARGET]);

    assert!(selected.selection().missing_endpoints.is_empty());
    assert!(selected.selector_omissions().is_empty());
    assert!(selected
        .selection()
        .selected_source_files
        .contains("one/mmakefile.src"));
    assert!(selected
        .selection()
        .selected_owner_targets
        .get("one/mmakefile")
        .is_some_and(|targets| targets.contains(CONCRETE_DEPENDENCY)));
}
