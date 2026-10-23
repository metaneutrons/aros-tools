use super::*;
use crate::metamake_project::{plan_source_inputs, ConfiguredProject, InventoryEntry};
use std::collections::BTreeMap;

fn project(extra: &str) -> ConfiguredProject {
    let config =
        format!("[probe]\ndefaultmakefilename rules\ngenmakefilescript test-generator\n{extra}");
    ConfiguredProject::parse(&config, "probe").unwrap()
}

fn inventory(entries: &[(&str, InventoryEntry)]) -> BTreeMap<String, InventoryEntry> {
    entries
        .iter()
        .map(|(path, kind)| ((*path).to_owned(), *kind))
        .collect()
}

fn targets(entries: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
    entries
        .iter()
        .map(|(path, target)| ((*path).to_owned(), target.as_bytes().to_vec()))
        .collect()
}

fn discovery_error(result: Result<ResolvedDiscovery, String>) -> String {
    match result {
        Ok(_) => panic!("expected discovery to refuse this input"),
        Err(error) => error,
    }
}

#[test]
fn directory_aliases_keep_logical_owners_and_bind_to_the_physical_source() {
    use InventoryEntry::{Directory, Link, RegularFile};

    let project = project("");
    let inventory = inventory(&[
        ("alias", Link),
        ("target", Directory),
        ("target/rules.src", RegularFile),
    ]);
    let resolved =
        resolve_source_discovery(&project, &inventory, &targets(&[("alias", "target")])).unwrap();

    assert_eq!(resolved.inventory["alias"], Directory);
    assert_eq!(resolved.inventory["alias/rules.src"], RegularFile);
    assert_eq!(
        resolved.original_inputs["alias/rules.src"],
        "target/rules.src"
    );
    assert_eq!(
        resolved.original_inputs["target/rules.src"],
        "target/rules.src"
    );
    assert_eq!(
        plan_source_inputs(&project, &resolved.inventory)
            .unwrap()
            .iter()
            .map(|owner| (owner.owner.as_str(), owner.input.as_str()))
            .collect::<Vec<_>>(),
        [
            ("alias/rules", "alias/rules.src"),
            ("target/rules", "target/rules.src"),
        ]
    );
}

#[test]
fn selected_file_link_resolves_to_its_regular_source_input() {
    use InventoryEntry::{Link, RegularFile};

    let project = project("");
    let inventory = inventory(&[("actual.src", RegularFile), ("rules.src", Link)]);
    let resolved = resolve_source_discovery(
        &project,
        &inventory,
        &targets(&[("rules.src", "actual.src")]),
    )
    .unwrap();

    assert_eq!(resolved.inventory["rules.src"], RegularFile);
    assert_eq!(resolved.original_inputs["rules.src"], "actual.src");
    let owners = plan_source_inputs(&project, &resolved.inventory).unwrap();
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].owner, "rules");
    assert_eq!(owners[0].input, "rules.src");
    assert!(owners[0].generated);
}

#[test]
fn ordinary_assets_allow_spaces_but_a_selected_owner_under_that_alias_is_rejected() {
    use InventoryEntry::{Directory, Link, RegularFile};

    let project = project("");
    let inventory = inventory(&[
        ("actual", Directory),
        ("actual/rules.src", RegularFile),
        ("asset dir", Directory),
        ("asset dir/Open Sans Italic.font", RegularFile),
        ("source files", Link),
    ]);
    let resolved = resolve_source_discovery(
        &project,
        &inventory,
        &targets(&[("source files", "actual")]),
    )
    .unwrap();

    assert_eq!(
        resolved.inventory["asset dir/Open Sans Italic.font"],
        RegularFile
    );
    assert_eq!(resolved.inventory["source files/rules.src"], RegularFile);
    let error = plan_source_inputs(&project, &resolved.inventory).unwrap_err();
    assert!(error.contains("literal basename"), "{error}");
}

#[test]
fn ignored_nonselected_link_is_skipped_without_following_its_external_target() {
    use InventoryEntry::{Link, RegularFile};

    let project = project("ignoredir ignored\nignoredir rules.src\n");
    let inventory = inventory(&[
        ("actual.src", RegularFile),
        ("ignored", Link),
        ("rules.src", Link),
    ]);
    let resolved = resolve_source_discovery(
        &project,
        &inventory,
        &targets(&[("ignored", "../../outside"), ("rules.src", "actual.src")]),
    )
    .unwrap();

    assert!(!resolved.inventory.contains_key("ignored"));
    assert_eq!(resolved.inventory["rules.src"], RegularFile);
    assert_eq!(resolved.original_inputs["rules.src"], "actual.src");
    assert_eq!(
        plan_source_inputs(&project, &resolved.inventory)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn slash_ignoredir_matches_exact_logical_subtree_before_link_resolution() {
    use InventoryEntry::{Directory, Link, RegularFile};

    let project = project("ignoredir build/arm-raspi\n");
    let inventory = inventory(&[
        ("alias", Link),
        ("build", Directory),
        ("build/arm-raspi", Link),
        ("build/arm-raspi-copy", Directory),
        ("build/arm-raspi-copy/rules.src", RegularFile),
        ("build/other", Directory),
        ("build/other/rules.src", RegularFile),
        ("physical", Directory),
        ("physical/arm-raspi", Directory),
        ("physical/arm-raspi/rules.src", RegularFile),
        ("vendor", Directory),
        ("vendor/arm-raspi", Directory),
        ("vendor/arm-raspi/rules.src", RegularFile),
    ]);
    let resolved = resolve_source_discovery(
        &project,
        &inventory,
        &targets(&[("alias", "physical"), ("build/arm-raspi", "/outside")]),
    )
    .unwrap();

    assert!(!resolved.inventory.contains_key("build/arm-raspi"));
    for path in [
        "build/arm-raspi-copy/rules.src",
        "build/other/rules.src",
        "vendor/arm-raspi/rules.src",
        "alias/arm-raspi/rules.src",
        "physical/arm-raspi/rules.src",
    ] {
        assert!(resolved.inventory.contains_key(path), "{path}");
    }
}

#[test]
fn bare_ignoredir_still_matches_directory_basenames_at_any_depth() {
    use InventoryEntry::{Directory, RegularFile};

    let project = project("ignoredir arm-raspi\n");
    let inventory = inventory(&[
        ("build", Directory),
        ("build/arm-raspi", Directory),
        ("build/arm-raspi/rules.src", RegularFile),
        ("vendor", Directory),
        ("vendor/arm-raspi", Directory),
        ("vendor/arm-raspi/rules.src", RegularFile),
        ("vendor/arm-raspi-copy", Directory),
        ("vendor/arm-raspi-copy/rules.src", RegularFile),
    ]);
    let resolved = resolve_source_discovery(&project, &inventory, &BTreeMap::new()).unwrap();
    assert!(!resolved.inventory.contains_key("build/arm-raspi"));
    assert!(!resolved.inventory.contains_key("vendor/arm-raspi"));
    assert!(resolved
        .inventory
        .contains_key("vendor/arm-raspi-copy/rules.src"));
}

#[test]
fn unmeasured_link_targets_are_refused_even_when_not_selected() {
    use InventoryEntry::Link;

    let project = project("");
    let dangling = inventory(&[("optional", Link)]);
    let error = discovery_error(resolve_source_discovery(
        &project,
        &dangling,
        &targets(&[("optional", "missing")]),
    ));
    assert!(error.contains("link target is unmeasured"), "{error}");

    let selected = inventory(&[("rules.src", Link)]);
    let error = discovery_error(resolve_source_discovery(
        &project,
        &selected,
        &targets(&[("rules.src", "missing")]),
    ));
    assert!(error.contains("link target is unmeasured"), "{error}");
}

#[test]
fn absolute_and_escaping_link_targets_are_refused() {
    use InventoryEntry::Link;

    let project = project("");
    for (target, expected) in [
        ("/outside", "unsupported MetaMake link target"),
        ("../outside", "escapes source"),
    ] {
        let inventory = inventory(&[("rules.src", Link)]);
        let error = discovery_error(resolve_source_discovery(
            &project,
            &inventory,
            &targets(&[("rules.src", target)]),
        ));
        assert!(error.contains(expected), "target {target:?}: {error}");
    }
}

#[test]
fn file_link_loops_stop_at_the_hop_limit() {
    use InventoryEntry::Link;

    let project = project("");
    let inventory = inventory(&[("first", Link), ("rules.src", Link), ("second", Link)]);
    let targets = targets(&[
        ("first", "second"),
        ("rules.src", "first"),
        ("second", "first"),
    ]);

    let error = discovery_error(resolve_source_discovery(&project, &inventory, &targets));
    assert!(error.contains("link cycle/hop limit"), "{error}");
}

#[test]
fn directory_aliases_cannot_reenter_an_ancestor() {
    use InventoryEntry::{Directory, Link};

    let project = project("");
    let inventory = inventory(&[("nested", Directory), ("nested/back", Link)]);
    let error = discovery_error(resolve_source_discovery(
        &project,
        &inventory,
        &targets(&[("nested/back", "..")]),
    ));

    assert!(error.contains("directory cycle/depth limit"), "{error}");
}

#[test]
fn parent_components_after_a_link_follow_the_resolved_directory() {
    use InventoryEntry::{Directory, Link, RegularFile};

    let project = project("");
    let inventory = inventory(&[
        ("actual", Directory),
        ("actual/child", Directory),
        ("actual/other", Directory),
        ("actual/other/rules.src", RegularFile),
        ("alias", Link),
        ("jump", Link),
        ("other", Directory),
        ("other/rules.src", RegularFile),
    ]);
    let resolved = resolve_source_discovery(
        &project,
        &inventory,
        &targets(&[("alias", "actual/child"), ("jump", "alias/../other")]),
    )
    .unwrap();

    assert_eq!(
        resolved.original_inputs["jump/rules.src"],
        "actual/other/rules.src"
    );
}

#[test]
fn inventory_provenance_and_link_target_data_must_match_exactly() {
    use InventoryEntry::{Link, RegularFile};

    let project = project("");

    let missing_parent = inventory(&[("missing/rules.src", RegularFile)]);
    assert!(discovery_error(resolve_source_discovery(
        &project,
        &missing_parent,
        &BTreeMap::new()
    ))
    .contains("missing directory provenance"));

    let extra_target = targets(&[("orphan", "target")]);
    assert!(discovery_error(resolve_source_discovery(
        &project,
        &BTreeMap::new(),
        &extra_target
    ))
    .contains("undeclared link target data"));

    let missing_target = inventory(&[("link", Link)]);
    assert!(discovery_error(resolve_source_discovery(
        &project,
        &missing_target,
        &BTreeMap::new()
    ))
    .contains("link target inventory differs"));
}

#[test]
fn repeated_alias_expansion_is_bounded_with_a_small_physical_inventory() {
    use InventoryEntry::{Directory, Link, RegularFile};

    let project = project("");
    let levels = 18;
    let mut inventory = BTreeMap::new();
    let mut targets = BTreeMap::new();
    for level in 0..levels {
        let directory = format!("d{level:02}");
        inventory.insert(directory.clone(), Directory);
        if level + 1 < levels {
            let next = format!("d{:02}", level + 1);
            for alias in ["a", "b"] {
                let path = format!("{directory}/{alias}");
                inventory.insert(path.clone(), Link);
                targets.insert(path, format!("../{next}").into_bytes());
            }
        } else {
            inventory.insert(format!("{directory}/asset"), RegularFile);
        }
    }
    assert!(
        inventory.len() < 100,
        "physical inventory should stay small"
    );

    let error = discovery_error(resolve_source_discovery(&project, &inventory, &targets));
    assert!(error.contains("resource limit"), "{error}");
}
