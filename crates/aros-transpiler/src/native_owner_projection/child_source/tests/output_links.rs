use super::super::{DeclaredChildJoinSelection, MatchedChildDeclaredRules};
use super::*;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    Sha256Digest,
};
use std::path::Path;

struct SelectedContext {
    roots: Vec<String>,
    source_inputs_sha256: Sha256Digest,
    prepared_tree_sha256: Sha256Digest,
}

impl SelectedContext {
    fn new(fixture: &Fixture) -> Self {
        Self {
            roots: vec!["root".into()],
            source_inputs_sha256: root_digest(&fixture.source_root),
            prepared_tree_sha256: root_digest(&fixture.build_root),
        }
    }

    fn selection<'a>(&'a self, fixture: &'a Fixture) -> DeclaredChildJoinSelection<'a> {
        DeclaredChildJoinSelection {
            source_root: &fixture.source_root,
            source_inputs_sha256: &self.source_inputs_sha256,
            build_root: &fixture.build_root,
            prepared_tree_sha256: &self.prepared_tree_sha256,
            roots: &self.roots,
        }
    }
}

fn root_digest(root: &Path) -> Sha256Digest {
    require(
        measure_tree_content_cas(root).map(|tree| tree.payload_digest_excluding(None)),
        "could not measure synthetic selected root",
    )
}

fn install_owner_makefile(fixture: &Fixture, owner: &str, contents: &str) {
    let path = fixture.build_root.join(owner);
    require(
        std::fs::create_dir_all(path.parent().expect("owner path has a parent")),
        "could not create synthetic owner cwd",
    );
    require(
        std::fs::write(path, contents),
        "could not write synthetic owner Makefile",
    );
}

fn snapshot_for(fixture: &Fixture, working_directory: &Path) -> MakeInputSnapshot {
    let selections = [&fixture.source_root, &fixture.build_root]
        .into_iter()
        .map(|root| MakeInputRootSelection {
            root: root.to_owned(),
            content_sha256: root_digest(root),
        })
        .collect::<Vec<_>>();
    require(
        MakeInputSnapshot::capture(working_directory, &selections),
        "could not capture synthetic child input roots",
    )
}

fn declared_child(fixture: &Fixture, owner: &str) -> MatchedChildDeclaredRules {
    declared_child_for_goal(fixture, owner, "root")
}

fn declared_child_for_goal(
    fixture: &Fixture,
    owner: &str,
    goal: &str,
) -> MatchedChildDeclaredRules {
    let relative_directory = Path::new(owner)
        .parent()
        .expect("generated owner has a source directory");
    let working_directory = require(
        fixture.build_root.join(relative_directory).canonicalize(),
        "could not resolve synthetic owner cwd",
    );
    let environment = synthetic_environment();
    let ordinary = invocation(&working_directory, &environment);
    let child_invocation = NativeMakeInvocation::new(
        ordinary.arguments().iter().map(|argument| {
            if argument == "root" {
                goal.to_owned()
            } else {
                argument.clone()
            }
        }),
        working_directory.clone(),
        environment
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    )
    .unwrap();
    let (initial_stdout, environment_names) = pre_file_observation(&child_invocation);
    let snapshot = snapshot_for(fixture, &working_directory);
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, relative_directory, &working_directory);
    read_child(
        &fixture.projection,
        &paths,
        owner,
        &child_invocation,
        &initial_stdout,
        &environment_names,
        &snapshot,
    )
    .unwrap_or_else(|error| panic!("synthetic child source match refused: {error}"))
    .trace_declared_rules()
    .unwrap_or_else(|_| panic!("synthetic declared rules could not be traced"))
}

fn output_name(fixture: &Fixture) -> String {
    fixture
        .build_root
        .join("generated/output.h")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn reached_output_candidate_keeps_each_owner_scope_and_does_not_run_recipes() {
    let fixture = Fixture::new();
    let output = output_name(&fixture);
    install_owner_makefile(&fixture, "one/mmakefile", &format!("root: {output}\n"));
    install_owner_makefile(
        &fixture,
        "two/mmakefile",
        &format!("root: {output}\n{output}:\n\t@touch producer-ran\n"),
    );
    let context = SelectedContext::new(&fixture);
    let links = require(
        fixture.projection.link_declared_child_outputs(
            &context.selection(&fixture),
            vec![
                declared_child(&fixture, "one/mmakefile"),
                declared_child(&fixture, "two/mmakefile"),
            ],
        ),
        "selected child outputs could not be linked",
    );

    assert_eq!(links.children().len(), 2);
    assert_eq!(
        links.children()[0].coordinates().generated_owner(),
        "one/mmakefile"
    );
    assert_eq!(
        links.children()[1].coordinates().generated_owner(),
        "two/mmakefile"
    );
    assert_eq!(
        links.children()[0].input_reads().reads()[0].path,
        fixture.build_root.join("one/mmakefile"),
        "consumer reads must remain rooted in its own cwd",
    );
    assert_eq!(
        links.children()[1].input_reads().reads()[0].path,
        fixture.build_root.join("two/mmakefile"),
        "producer reads must remain rooted in its own cwd",
    );
    assert_eq!(
        links.children()[0]
            .traversal()
            .scope_chain(1)
            .expect("consumer scope chain is retained"),
        vec!["root", output.as_str()],
    );
    assert_eq!(
        links.candidates(),
        &[super::super::DeclaredChildOutputCandidate {
            consumer_child: 0,
            consumer_visit: 1,
            producer_child: 1,
            producer_visit: 1,
        }],
    );
    assert!(links.unlinked_inputs().is_empty());
    assert_eq!(
        links.children()[0].traversal().unresolved_inputs().count(),
        1
    );
    assert!(!Path::new(&output).exists());
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    assert!(
        declared_child(&fixture, "one/mmakefile")
            .require_captured_recipes(&paths)
            .is_err(),
        "a declared output candidate must not admit a physical input"
    );
    assert!(links.verify().is_ok());
    assert!(
        !fixture.build_root.join("two/producer-ran").exists(),
        "linking declared outputs must not execute the producer recipe",
    );
}

#[test]
fn matching_text_in_an_unreached_producer_rule_does_not_link_a_leaf() {
    let fixture = Fixture::new();
    let output = output_name(&fixture);
    install_owner_makefile(&fixture, "one/mmakefile", &format!("root: {output}\n"));
    install_owner_makefile(
        &fixture,
        "two/mmakefile",
        &format!("root:\n{output}:\n\t@touch unrelated-producer-ran\n"),
    );
    let context = SelectedContext::new(&fixture);
    let links = require(
        fixture.projection.link_declared_child_outputs(
            &context.selection(&fixture),
            vec![
                declared_child(&fixture, "one/mmakefile"),
                declared_child(&fixture, "two/mmakefile"),
            ],
        ),
        "selected child declarations could not be joined",
    );

    assert_eq!(
        links.children()[1]
            .traversal()
            .visits()
            .iter()
            .map(crate::make_rule_graph::traversal::TargetVisit::target)
            .collect::<Vec<_>>(),
        ["root"],
        "the producer-looking declaration is outside the reached graph",
    );
    assert!(links.candidates().is_empty());
    assert_eq!(links.unlinked_inputs(), &[(0, 1)]);
    assert!(
        !fixture
            .build_root
            .join("two/unrelated-producer-ran")
            .exists(),
        "linking declarations must not execute an unrelated recipe",
    );
}

#[test]
fn missing_selected_owner_goal_is_rejected() {
    let fixture = Fixture::new();
    install_owner_makefile(&fixture, "one/mmakefile", "root:\n");
    install_owner_makefile(&fixture, "two/mmakefile", "root:\n");
    let context = SelectedContext::new(&fixture);
    assert!(
        fixture
            .projection
            .link_declared_child_outputs(
                &context.selection(&fixture),
                vec![declared_child(&fixture, "one/mmakefile")],
            )
            .is_err(),
        "join accepted a batch missing the selected two/mmakefile goal",
    );
}

#[test]
fn a_producer_matched_under_a_broader_selection_cannot_fill_an_unselected_owner() {
    let mut fixture = Fixture::new();
    std::fs::write(
        fixture.source_root.join("one/mmakefile.src"),
        "#MM root: solo\n#MM solo\n",
    )
    .unwrap();
    fixture.projection =
        super::super::super::tests::projection_for_discovery(&fixture.source_root, BTreeSet::new());
    // Discovery-only fixtures give every owner just `root`; this counter must
    // instead use the narrower goal declared in the synthetic Source itself.
    let metadata = ["one", "two"]
        .into_iter()
        .map(|owner| {
            (
                format!("{owner}/mmakefile"),
                std::fs::read_to_string(fixture.source_root.join(owner).join("mmakefile.src"))
                    .unwrap(),
            )
        })
        .collect();
    fixture.projection.graph =
        crate::metamake_owner_graph::MetaMakeOwnerGraph::parse_with_declared_absence(
            &metadata,
            &BTreeMap::new(),
            &BTreeSet::new(),
            crate::metamake_owner_graph::Limits::default(),
        )
        .unwrap();
    let output = output_name(&fixture);
    install_owner_makefile(
        &fixture,
        "one/mmakefile",
        &format!("root: solo\nsolo: {output}\n"),
    );
    install_owner_makefile(
        &fixture,
        "two/mmakefile",
        &format!("root: {output}\n{output}:\n\t@touch unselected-producer-ran\n"),
    );
    let mut context = SelectedContext::new(&fixture);
    context.roots = vec!["solo".into()];
    let result = fixture.projection.link_declared_child_outputs(
        &context.selection(&fixture),
        vec![
            declared_child_for_goal(&fixture, "one/mmakefile", "solo"),
            declared_child(&fixture, "two/mmakefile"),
        ],
    );
    assert_eq!(
        result.err().expect("unselected producer was accepted"),
        "declared child producer owner is not selected"
    );
    assert!(!fixture
        .build_root
        .join("two/unselected-producer-ran")
        .exists());
}

#[test]
fn foreign_prepared_tree_hash_is_rejected() {
    let fixture = Fixture::new();
    install_owner_makefile(&fixture, "one/mmakefile", "root:\n");
    install_owner_makefile(&fixture, "two/mmakefile", "root:\n");
    let context = SelectedContext::new(&fixture);
    let foreign_hash = aros_common::sha256_bytes(b"different prepared build tree");
    let mut selection = context.selection(&fixture);
    selection.prepared_tree_sha256 = &foreign_hash;

    assert!(
        fixture
            .projection
            .link_declared_child_outputs(
                &selection,
                vec![
                    declared_child(&fixture, "one/mmakefile"),
                    declared_child(&fixture, "two/mmakefile")
                ],
            )
            .is_err(),
        "join accepted a prepared-tree hash outside the captured roots",
    );
}

#[test]
fn duplicate_reached_outputs_across_selected_owners_are_ambiguous() {
    let fixture = Fixture::new();
    let output = output_name(&fixture);
    for owner in ["one", "two"] {
        install_owner_makefile(
            &fixture,
            &format!("{owner}/mmakefile"),
            &format!("root: {output}\n{output}:\n\t@touch {owner}-producer-ran\n"),
        );
    }
    let context = SelectedContext::new(&fixture);
    assert!(
        fixture
            .projection
            .link_declared_child_outputs(
                &context.selection(&fixture),
                vec![
                    declared_child(&fixture, "one/mmakefile"),
                    declared_child(&fixture, "two/mmakefile"),
                ],
            )
            .is_err(),
        "join accepted the same reached literal output from two selected owners",
    );
}

#[test]
fn changed_common_input_after_child_capture_is_rejected() {
    let fixture = Fixture::new();
    install_owner_makefile(&fixture, "one/mmakefile", "root:\n");
    install_owner_makefile(&fixture, "two/mmakefile", "root:\n");
    let context = SelectedContext::new(&fixture);
    let children = vec![
        declared_child(&fixture, "one/mmakefile"),
        declared_child(&fixture, "two/mmakefile"),
    ];
    require(
        std::fs::write(
            fixture.source_root.join("one/mmakefile.src"),
            "#MM changed after child capture\n",
        ),
        "could not change synthetic source input",
    );

    assert!(
        fixture
            .projection
            .link_declared_child_outputs(&context.selection(&fixture), children)
            .is_err(),
        "join accepted a changed common Source input",
    );
}

#[cfg(unix)]
#[test]
fn changed_original_input_metadata_after_capture_is_rejected() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    install_owner_makefile(&fixture, "one/mmakefile", "root:\n");
    install_owner_makefile(&fixture, "two/mmakefile", "root:\n");
    let context = SelectedContext::new(&fixture);
    let children = vec![
        declared_child(&fixture, "one/mmakefile"),
        declared_child(&fixture, "two/mmakefile"),
    ];
    let input = fixture.build_root.join("one/mmakefile");
    let mut permissions = require(
        std::fs::metadata(&input),
        "could not inspect synthetic original input metadata",
    )
    .permissions();
    permissions.set_mode(permissions.mode() ^ 0o100);
    require(
        std::fs::set_permissions(&input, permissions),
        "could not change synthetic original input metadata",
    );

    assert!(
        fixture
            .projection
            .link_declared_child_outputs(&context.selection(&fixture), children)
            .is_err(),
        "join accepted changed metadata in a captured original input root",
    );
}
