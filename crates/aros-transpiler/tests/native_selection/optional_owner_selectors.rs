//! Process-boundary probes for selector-qualified virtual MetaMake owners.

use super::*;

const SOURCE_OWNER: &str = "optional-$(ARCH)-$(CPU)";
const SOURCE_DEPENDENCY: &str = "missing-$(ARCH)-$(CPU)-$(AROS_TARGET_VARIANT)";
const CONTRACT_OWNER: &str = "optional-fixture-riscv";
const CONTRACT_DEPENDENCY: &str =
    "missing-${AROS_TARGET_PLATFORM}-${AROS_TARGET_CPU}-${AROS_TARGET_VARIANT}";

fn append_selector_route(fixture: &Fixture) {
    fixture.append(&format!(
        "#MM- fixture-kernel : {SOURCE_OWNER}\n#MM- {SOURCE_OWNER} : {SOURCE_DEPENDENCY}\n"
    ));
}

fn bind_selector_route(fixture: &mut Fixture, target: &str) {
    install_native_metamake_projection(fixture, &["mmakefile.src"]);
    bind_optional_meta_contract_edge(fixture, "mmakefile.src", target, CONTRACT_DEPENDENCY, true);
}

fn assert_selector_route_omitted(fixture: &Fixture) {
    let result = fixture.invoke(true, &["--source-inventory-only"]);

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fixture.output().exists());
    let inventory_path = fixture.output().with_extension("source-inventory.cmake");
    assert!(inventory_path.is_file());
    let receipt: Value = serde_json::from_slice(
        &fs::read(fixture.output().with_extension("native-invocation.json")).unwrap(),
    )
    .unwrap();
    let semantics = &receipt["source_meta_semantics"];
    assert!(
        semantics["verified_selector_contracts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|proof| {
                proof["recipe"] == "mmakefile.src"
                    && proof["target"] == CONTRACT_OWNER
                    && proof["expression"] == CONTRACT_DEPENDENCY
            }),
        "{receipt}"
    );
    assert!(
        semantics["architecture_hook_omissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|omission| {
                omission["target"] == CONTRACT_OWNER
                    && omission["dependency"] == "missing-fixture-riscv-"
                    && omission["recipes"].as_array().is_some_and(|recipes| {
                        recipes.iter().any(|recipe| recipe == "mmakefile.src")
                    })
            }),
        "{receipt}"
    );
}

#[test]
fn selector_qualified_virtual_owner_omits_only_its_exact_missing_leaf() {
    let mut fixture = Fixture::new();
    append_selector_route(&fixture);
    bind_selector_route(&mut fixture, CONTRACT_OWNER);
    assert_selector_route_omitted(&fixture);
}

#[test]
fn selector_qualified_virtual_owner_inside_empty_variant_conditional_is_proven() {
    let mut fixture = Fixture::new();
    fixture.append(&format!(
        "ifneq ($(AROS_TARGET_VARIANT),)\n#MM- fixture-kernel : {SOURCE_OWNER}\n#MM- {SOURCE_OWNER} : {SOURCE_DEPENDENCY}\nendif\n"
    ));
    bind_selector_route(&mut fixture, CONTRACT_OWNER);
    assert_selector_route_omitted(&fixture);
}

#[test]
fn selector_owner_contract_must_match_the_concrete_native_owner() {
    let mut fixture = Fixture::new();
    append_selector_route(&fixture);
    bind_selector_route(&mut fixture, "optional-other-riscv");

    let result = fixture.invoke(true, &["--source-inventory-only"]);

    assert_failure_at_stage(
        &result,
        &fixture.output(),
        "is not declared by this bound source recipe",
        "graph_validation",
    );
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
}

#[test]
fn selector_contract_cannot_reclassify_the_same_nonvirtual_owner_as_optional() {
    let mut fixture = Fixture::new();
    fixture.append(&format!(
        "#MM- fixture-kernel : {SOURCE_OWNER}\n#MM {SOURCE_OWNER} : {SOURCE_DEPENDENCY}\n"
    ));
    bind_selector_route(&mut fixture, CONTRACT_OWNER);

    let result = fixture.invoke(true, &["--source-inventory-only"]);

    assert_failure_at_stage(
        &result,
        &fixture.output(),
        "cannot reclassify a mandatory source dependency",
        "graph_validation",
    );
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
}

#[test]
fn selector_contract_cannot_waive_the_same_raw_edge_from_another_sealed_recipe() {
    let mut fixture = Fixture::new();
    append_selector_route(&fixture);
    let other = fixture.root.path().join("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(
        other.join("mmakefile.src"),
        format!("#MM- {SOURCE_OWNER} : {SOURCE_DEPENDENCY}\n"),
    )
    .unwrap();
    install_native_metamake_projection(&mut fixture, &["mmakefile.src", "other/mmakefile.src"]);
    bind_optional_meta_contract_edge(
        &mut fixture,
        "mmakefile.src",
        CONTRACT_OWNER,
        CONTRACT_DEPENDENCY,
        true,
    );

    let result = fixture.invoke(true, &["--source-inventory-only"]);

    assert_failure_at_stage(
        &result,
        &fixture.output(),
        "source declaration without explicit optionality",
        "graph_validation",
    );
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
}

#[test]
fn selector_contract_cannot_waive_a_distinct_expression_for_the_same_concrete_leaf() {
    let mut fixture = Fixture::new();
    append_selector_route(&fixture);
    let other = fixture.root.path().join("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(
        other.join("mmakefile.src"),
        format!("#MM- {SOURCE_OWNER} : missing-$(AROS_TARGET_PLATFORM)-$(AROS_TARGET_VARIANT)\n"),
    )
    .unwrap();
    install_native_metamake_projection(&mut fixture, &["mmakefile.src", "other/mmakefile.src"]);
    bind_optional_meta_contract_edge(
        &mut fixture,
        "mmakefile.src",
        CONTRACT_OWNER,
        CONTRACT_DEPENDENCY,
        true,
    );
    let result = fixture.invoke(true, &["--source-inventory-only"]);
    assert_failure_at_stage(
        &result,
        &fixture.output(),
        "missing-fixture-riscv-",
        "graph_validation",
    );
    assert!(!fixture
        .output()
        .with_extension("source-inventory.cmake")
        .exists());
}
