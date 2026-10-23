use super::*;
use crate::Sha256Digest;
use serde_json::{json, Value};

fn expected() -> MesaGenmoduleSelectionBinding {
    MesaGenmoduleSelectionBinding {
        profile: "arm-raspi".into(),
        kind: "sdk-consumer".into(),
        scope: MesaGenmoduleInventoryScope::ContractRoots,
        contract_sha256: Sha256Digest::parse(&"a".repeat(64)).unwrap(),
    }
}

fn record(owner: &str, consumer_path: &str) -> Value {
    json!({
        "owner": owner,
        "declaration_directory": "workbench/libs/mesa",
        "module": "mesa3dgl26",
        "module_type": "library",
        "base_configuration": "workbench/libs/gl/gl.conf",
        "override_configuration": "workbench/libs/mesa/mesa3dgl.conf",
        "consumer_path": consumer_path,
        "components": [],
    })
}

fn document(records: Vec<Value>) -> Value {
    let records = Value::Array(records);
    json!({
        "schema": MESA_GENMODULE_SELECTION_SCHEMA,
        "selection": {
            "profile": "arm-raspi",
            "kind": "sdk-consumer",
            "scope": "contract-roots",
            "contract_sha256": "a".repeat(64),
        },
        "inventory": {
            "schema": MESA_GENMODULE_INVENTORY_SCHEMA,
            "records": records,
        }
    })
}

fn parse(value: &Value) -> Result<ValidatedMesaGenmoduleSelection, String> {
    ValidatedMesaGenmoduleSelection::parse_bound(&serde_json::to_vec(value).unwrap(), &expected())
}

fn assert_rejected(value: &Value) {
    assert!(parse(value).is_err(), "accepted invalid document: {value}");
}

#[test]
fn actual_shaped_normal_and_relative_gl_rows_deduplicate_to_one_producer() {
    let owner = "mesa3dgl26-0";
    let selection = parse(&document(vec![
        record(owner, &format!("runtime/{owner}")),
        record(owner, &format!("module-client/{owner}/normal")),
        record(owner, &format!("module-client/{owner}/rel")),
    ]))
    .unwrap();

    assert_eq!(selection.binding(), &expected());
    assert_eq!(selection.inventory().records.len(), 3);
    assert_eq!(selection.producers().len(), 1);
    assert_eq!(selection.producers()[0].owner, owner);
    assert_eq!(selection.producers()[0].module, "mesa3dgl26");
    assert_eq!(
        selection.producers()[0].base_configuration,
        "workbench/libs/gl/gl.conf"
    );
    assert_eq!(
        selection.producers()[0].override_configuration.as_deref(),
        Some("workbench/libs/mesa/mesa3dgl.conf")
    );
}

#[test]
fn gallium_and_hidd_with_shared_module_prefix_remain_distinct_producers() {
    let mut gallium = record("mesa3dgl26-gallium", "runtime/mesa3dgl26-gallium");
    gallium["declaration_directory"] = json!("workbench/libs/mesa/gallium");
    let mut hidd = record("mesa3dgl26-hidd", "runtime/mesa3dgl26-hidd");
    hidd["module_type"] = json!("hidd");
    hidd["declaration_directory"] = json!("workbench/libs/mesa/hidd");

    let selection = parse(&document(vec![gallium, hidd])).unwrap();
    assert_eq!(selection.producers().len(), 2);
    assert_eq!(selection.producers()[0].module, "mesa3dgl26");
    assert_eq!(selection.producers()[1].module, "mesa3dgl26");
    assert_ne!(
        selection.producers()[0].owner,
        selection.producers()[1].owner
    );
}

#[test]
fn generated_client_marker_roles_preserve_component_order() {
    let mut marker = record("mesa3dgl26-writefiles", "marker/mesa3dgl26-library/rel");
    marker["components"] = json!(["stackstubs", "autoinit", "getlibbase"]);
    let selection = parse(&document(vec![marker])).unwrap();
    assert_eq!(
        selection.inventory().records[0].components,
        vec![
            "stackstubs".to_owned(),
            "autoinit".to_owned(),
            "getlibbase".to_owned()
        ]
    );
}

#[test]
fn selection_binding_and_closed_schemas_are_exact() {
    let valid = document(vec![record("mesa3dgl26-0", "runtime/mesa3dgl26-0")]);
    for field in ["profile", "kind", "scope", "contract_sha256"] {
        let mut expected = expected();
        match field {
            "profile" => expected.profile = "other-profile".into(),
            "kind" => expected.kind = "build".into(),
            "scope" => expected.scope = MesaGenmoduleInventoryScope::FullTarget,
            "contract_sha256" => {
                expected.contract_sha256 = Sha256Digest::parse(&"b".repeat(64)).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(ValidatedMesaGenmoduleSelection::parse_bound(
            &serde_json::to_vec(&valid).unwrap(),
            &expected,
        )
        .is_err());
    }

    let mut bad_document_schema = valid.clone();
    bad_document_schema["schema"] = json!("other-schema");
    assert_rejected(&bad_document_schema);
    let mut bad_inventory_schema = valid.clone();
    bad_inventory_schema["inventory"]["schema"] = json!("other-schema");
    assert_rejected(&bad_inventory_schema);

    let mut unknown_top_level = valid.clone();
    unknown_top_level["unexpected"] = json!(true);
    assert_rejected(&unknown_top_level);
    let mut unknown_binding = valid.clone();
    unknown_binding["selection"]["unexpected"] = json!(true);
    assert_rejected(&unknown_binding);
    let mut missing_scope = valid.clone();
    missing_scope["selection"]
        .as_object_mut()
        .unwrap()
        .remove("scope");
    assert_rejected(&missing_scope);
    let mut unknown_scope = valid.clone();
    unknown_scope["selection"]["scope"] = json!("unbounded");
    assert_rejected(&unknown_scope);
    let mut unknown_inventory_field = valid.clone();
    unknown_inventory_field["inventory"]["unexpected"] = json!(true);
    assert_rejected(&unknown_inventory_field);
    let mut unknown_record_field = valid;
    unknown_record_field["inventory"]["records"][0]["unexpected"] = json!(true);
    assert_rejected(&unknown_record_field);

    let mut missing_optional_field = document(vec![record("mesa3dgl26-0", "runtime/mesa3dgl26-0")]);
    missing_optional_field["inventory"]["records"][0]
        .as_object_mut()
        .unwrap()
        .remove("override_configuration");
    assert_rejected(&missing_optional_field);
}

#[test]
fn full_target_scope_remains_distinct_from_sdk_consumer_kind() {
    let mut full_target = document(vec![record("mesa3dgl26-0", "runtime/mesa3dgl26-0")]);
    full_target["selection"]["scope"] = json!("full-target");
    let mut expected = expected();
    expected.scope = MesaGenmoduleInventoryScope::FullTarget;

    let selection = ValidatedMesaGenmoduleSelection::parse_bound(
        &serde_json::to_vec(&full_target).unwrap(),
        &expected,
    )
    .unwrap();
    assert_eq!(
        selection.binding().scope,
        MesaGenmoduleInventoryScope::FullTarget
    );
    assert_eq!(selection.binding().kind, "sdk-consumer");
}

#[test]
fn rejects_invalid_binding_kinds_types_and_consumer_roles() {
    let mut invalid_kind = document(vec![]);
    invalid_kind["selection"]["kind"] = json!("other");
    assert_rejected(&invalid_kind);

    let mut unknown_type = document(vec![record("mesa3dgl26-0", "runtime/mesa3dgl26-0")]);
    unknown_type["inventory"]["records"][0]["module_type"] = json!("unknown");
    assert_rejected(&unknown_type);

    let mismatched_runtime_owner =
        document(vec![record("mesa3dgl26-0", "runtime/different-owner")]);
    assert_rejected(&mismatched_runtime_owner);

    let invalid_lane = document(vec![record(
        "mesa3dgl26-0",
        "module-client/mesa3dgl26-0/alternate",
    )]);
    assert_rejected(&invalid_lane);

    let marker_without_components =
        document(vec![record("mesa3dgl26-0", "marker/library-client/normal")]);
    assert_rejected(&marker_without_components);

    let mut runtime_with_components = record("mesa3dgl26-0", "runtime/mesa3dgl26-0");
    runtime_with_components["components"] = json!(["stackstubs"]);
    assert_rejected(&document(vec![runtime_with_components]));
}

#[test]
fn rejects_nonliteral_or_non_normalized_source_paths() {
    for invalid in [
        "/workbench/libs/gl/gl.conf",
        "${AROS_SOURCE_DIR}/workbench/libs/gl/gl.conf",
        "../gl.conf",
        "workbench/./gl.conf",
        "workbench//gl.conf",
        "workbench/libs/gl/gl.conf;touch",
        "workbench/libs/gl/gl.conf\nother",
    ] {
        let mut value = document(vec![record("mesa3dgl26-0", "runtime/mesa3dgl26-0")]);
        value["inventory"]["records"][0]["base_configuration"] = json!(invalid);
        assert_rejected(&value);
    }

    let unsafe_owner = document(vec![record("mesa$bad", "runtime/mesa$bad")]);
    assert_rejected(&unsafe_owner);
    let mut unsafe_marker_owner =
        document(vec![record("mesa3dgl26-0", "marker/client$bad/normal")]);
    unsafe_marker_owner["inventory"]["records"][0]["components"] = json!(["autoinit"]);
    assert_rejected(&unsafe_marker_owner);
}

#[test]
fn rejects_duplicate_lanes_and_conflicting_producer_tuples() {
    let owner = "mesa3dgl26-0";
    let lane = record(owner, &format!("module-client/{owner}/normal"));
    assert_rejected(&document(vec![lane.clone(), lane]));

    let mut conflicting = record(owner, &format!("module-client/{owner}/rel"));
    conflicting["base_configuration"] = json!("workbench/libs/gl/other.conf");
    assert_rejected(&document(vec![
        record(owner, &format!("module-client/{owner}/normal")),
        conflicting,
    ]));
}

#[test]
fn rejects_component_duplicates_and_noncanonical_order() {
    for components in [
        json!(["autoinit", "stackstubs"]),
        json!(["stackstubs", "stackstubs"]),
        json!(["unknown"]),
    ] {
        let mut marker = record("mesa3dgl26-writefiles", "marker/gl-client/normal");
        marker["components"] = components;
        assert_rejected(&document(vec![marker]));
    }
}

#[test]
fn rejects_oversized_selection_bytes_and_excessive_rows() {
    let valid = serde_json::to_vec(&document(vec![])).unwrap();
    let mut oversized = valid;
    oversized.resize(MESA_GENMODULE_SELECTION_MAX_BYTES + 1, b' ');
    assert!(ValidatedMesaGenmoduleSelection::parse_bound(&oversized, &expected()).is_err());

    let inventory = MesaGenmoduleInventory {
        schema: MESA_GENMODULE_INVENTORY_SCHEMA.into(),
        records: vec![
            serde_json::from_value(record("mesa3dgl26-0", "runtime/mesa3dgl26-0"))
                .unwrap();
            MESA_GENMODULE_INVENTORY_MAX_ROWS + 1
        ],
    };
    assert!(inventory.validate().is_err());
}
