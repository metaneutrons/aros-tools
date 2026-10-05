//! Owner metadata is independent of GNU Make evaluation and native admission.

use aros_transpiler::{
    genmf_projection::{expand_files, Limits as GenmfLimits},
    metamake_owner_graph::{Limits as GraphLimits, MetaMakeOwnerGraph},
    metamake_project::{plan_source_inputs, ConfiguredProject, InventoryEntry, ProjectGlobals},
};
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn source_config_genmf_and_metamake_share_one_explicit_owner_projection() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let source = root.join("rules.src");
    let template = root.join("root.tmpl");
    std::fs::write(&source, b"%graph\n").unwrap();
    std::fs::write(&template, b"%define graph\n#MM- root-$(CPU) : mid-$(CPU) bare-$(CPU)\n#MM- mid-$(CPU) : \\\n#MM leaf-$(CPU)\n#MM leaf-$(CPU) : tail-$(CPU)\n#MM- tail-$(CPU)\n#MM-\nbare-$(CPU) : ignored-make-prerequisite\n#MM bare-$(CPU) : tail-$(CPU)\n%end\n").unwrap();
    let project = ConfiguredProject::parse(
        "[probe]\ndefaultmakefilename rules\ngenmakefilescript not-executed\nglobalvarfile project.vars\nCPU ignored-lowercase\n", "probe",
    ).unwrap();
    let globals = ProjectGlobals::parse(
        &project.variables,
        &[("project.vars".into(), "CPU arm\n".into())],
    )
    .unwrap();
    let inputs = plan_source_inputs(
        &project,
        &BTreeMap::from([("rules.src".into(), InventoryEntry::RegularFile)]),
    )
    .unwrap();
    assert_eq!(inputs.len(), 1);
    let expanded = expand_files(&source, &template, GenmfLimits::default()).unwrap();
    assert_eq!(expanded.template_snapshots.len(), 1);
    let graph = MetaMakeOwnerGraph::parse_expanded_files(
        &BTreeMap::from([(inputs[0].owner.clone(), expanded.text)]),
        &globals.values,
        GraphLimits::default(),
    )
    .unwrap();
    let selection = graph
        .select(&["root-arm".into()], GraphLimits::default())
        .unwrap();
    assert_eq!(
        selection.reached_targets,
        BTreeSet::from([
            "root-arm".into(),
            "mid-arm".into(),
            "leaf-arm".into(),
            "tail-arm".into(),
            "bare-arm".into(),
        ])
    );
    assert_eq!(
        selection.selected_owner_files,
        BTreeSet::from(["rules".into()])
    );
    assert!(selection.missing_endpoints.is_empty());
    assert_eq!(globals.values["cpu"], "ignored-lowercase");
    assert_eq!(globals.values["CPU"], "arm");
    assert!(!root.join("not-executed").exists());
}

#[test]
fn gnu_make_conditions_and_local_assignments_do_not_select_metamake_owners() {
    let graph = MetaMakeOwnerGraph::parse_expanded_files(
        &BTreeMap::from([(
            "rules".into(),
            "CPU := incorrect\nifeq (never,ever)\n#MM root-$(CPU) : absent\nendif\n".into(),
        )]),
        &BTreeMap::from([("CPU".into(), "arm".into())]),
        GraphLimits::default(),
    )
    .unwrap();
    let selection = graph
        .select(&["root-arm".into()], GraphLimits::default())
        .unwrap();
    assert_eq!(
        selection.selected_owner_files,
        BTreeSet::from(["rules".into()])
    );
    assert_eq!(
        selection.missing_endpoints,
        BTreeSet::from(["absent".into()])
    );
}

#[test]
fn variable_binding_and_missing_owner_errors_are_not_build_producers() {
    assert!(MetaMakeOwnerGraph::parse_expanded_files(
        &BTreeMap::from([("rules".into(), "#MM root-$(UNBOUND)\n".into())]),
        &BTreeMap::new(),
        GraphLimits::default(),
    )
    .is_err());
    let graph = MetaMakeOwnerGraph::parse_expanded_files(
        &BTreeMap::from([("rules".into(), "#MM- root : absent\n".into())]),
        &BTreeMap::new(),
        GraphLimits::default(),
    )
    .unwrap();
    let selection = graph
        .select(&["root".into()], GraphLimits::default())
        .unwrap();
    assert!(selection.selected_owner_files.is_empty());
    assert_eq!(
        selection.missing_endpoints,
        BTreeSet::from(["absent".into()])
    );
    let native = aros_transpiler::DependencyGraph::new();
    assert!(native.make_meta_providers.is_empty());
}
