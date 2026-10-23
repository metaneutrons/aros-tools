use super::*;
use crate::mesa_genmodule_inventory::{
    MesaGenmoduleInventory, MesaGenmoduleInventoryRecord, MesaGenmoduleInventoryScope,
    MesaGenmoduleSelectionDocument, MESA_GENMODULE_INVENTORY_SCHEMA,
    MESA_GENMODULE_SELECTION_SCHEMA,
};
use crate::{sha256_bytes, Sha256Digest};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::parse(&byte.to_string().repeat(64)).unwrap()
}

fn record(
    owner: &str,
    module_type: &str,
    declaration_directory: &str,
    override_configuration: Option<&str>,
    consumer_path: &str,
) -> MesaGenmoduleInventoryRecord {
    MesaGenmoduleInventoryRecord {
        owner: owner.into(),
        declaration_directory: declaration_directory.into(),
        module: "mesa3dgl26".into(),
        module_type: module_type.into(),
        base_configuration: "workbench/libs/gl/gl.conf".into(),
        override_configuration: override_configuration.map(str::to_owned),
        consumer_path: consumer_path.into(),
        components: Vec::new(),
    }
}

fn makefile(module: &str, owner: &str) -> Vec<u8> {
    let mut text = String::new();
    for suffix in crate::mesa_genmodule_make::MESA_GENMODULE_MAKE_SUFFIXES {
        text.push_str(module);
        text.push('_');
        text.push_str(suffix);
        text.push_str(" +=");
        if suffix == "LIBS" {
            text.push(' ');
            text.push_str(owner);
            text.push_str("_library");
        }
        text.push('\n');
    }
    text.into_bytes()
}

struct CollectionFixture {
    _temporary: TempDir,
    input_root: PathBuf,
    source_root: PathBuf,
    binding: MesaGenmoduleCollectionBinding,
}

impl CollectionFixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let input_root = temporary.path().join("input");
        let source_root = temporary.path().join("source");
        fs::create_dir_all(&input_root).unwrap();
        fs::create_dir_all(source_root.join("workbench/libs/gl")).unwrap();
        fs::create_dir_all(source_root.join("workbench/libs/mesa")).unwrap();

        let base_path = "workbench/libs/gl/gl.conf";
        let base_bytes = b"RELLIBS = z1 posixc stdc\n";
        fs::write(source_root.join(base_path), base_bytes).unwrap();
        let gallium_override = "workbench/libs/mesa/gallium.conf";
        let gallium_override_bytes = b"RELLIBS = gallium stdc\n";
        fs::write(source_root.join(gallium_override), gallium_override_bytes).unwrap();

        let gallium_owner = "mesa3dgl26-gallium";
        let hidd_owner = "mesa3dgl26-hidd";
        let records = vec![
            record(
                gallium_owner,
                "library",
                "workbench/libs/mesa/gallium",
                Some(gallium_override),
                &format!("runtime/{gallium_owner}"),
            ),
            record(
                gallium_owner,
                "library",
                "workbench/libs/mesa/gallium",
                Some(gallium_override),
                &format!("module-client/{gallium_owner}/normal"),
            ),
            record(
                gallium_owner,
                "library",
                "workbench/libs/mesa/gallium",
                Some(gallium_override),
                &format!("module-client/{gallium_owner}/rel"),
            ),
            record(
                hidd_owner,
                "hidd",
                "workbench/libs/mesa/hidd",
                None,
                &format!("runtime/{hidd_owner}"),
            ),
        ];
        let selection = MesaGenmoduleSelectionBinding {
            profile: "arm-raspi".into(),
            kind: "sdk-consumer".into(),
            scope: MesaGenmoduleInventoryScope::ContractRoots,
            contract_sha256: digest('a'),
        };
        let selection_document = MesaGenmoduleSelectionDocument {
            schema: MESA_GENMODULE_SELECTION_SCHEMA.into(),
            selection: selection.clone(),
            inventory: MesaGenmoduleInventory {
                schema: MESA_GENMODULE_INVENTORY_SCHEMA.into(),
                records,
            },
        };
        let inventory_bytes = serde_json::to_vec(&selection_document).unwrap();
        fs::write(
            input_root.join(MESA_GENMODULE_SELECTION_FILE),
            &inventory_bytes,
        )
        .unwrap();

        let mut outputs = Vec::new();
        for producer in selection_document.inventory.producers().unwrap() {
            let raw_make = makefile(&producer.module, &producer.owner);
            let relative = mesa_genmodule_output_relative_path(&producer.owner).unwrap();
            fs::create_dir_all(input_root.join(relative.parent().unwrap())).unwrap();
            fs::write(input_root.join(relative), &raw_make).unwrap();
            let override_configuration_sha256 = producer
                .override_configuration
                .as_ref()
                .map(|_| sha256_bytes(gallium_override_bytes));
            outputs.push(MesaGenmoduleOutputBinding {
                producer,
                makefile_sha256: sha256_bytes(&raw_make),
                base_configuration_sha256: sha256_bytes(base_bytes),
                override_configuration_sha256,
            });
        }

        Self {
            _temporary: temporary,
            input_root: input_root.canonicalize().unwrap(),
            source_root: source_root.canonicalize().unwrap(),
            binding: MesaGenmoduleCollectionBinding {
                selection,
                inventory_sha256: sha256_bytes(&inventory_bytes),
                helper_sha256: digest('b'),
                producer_sha256: digest('c'),
                outputs,
            },
        }
    }

    fn load(
        &self,
        binding: &MesaGenmoduleCollectionBinding,
    ) -> Result<LoadedMesaGenmoduleCollection> {
        load_genmodule_collection(&self.input_root, &self.source_root, binding)
    }
}

#[test]
fn loads_two_same_prefix_owners_and_deduplicates_multiple_consumers() {
    let fixture = CollectionFixture::new();
    let loaded = fixture.load(&fixture.binding).unwrap();
    assert_eq!(loaded.selection().inventory().records.len(), 4);
    assert_eq!(loaded.selection().producers().len(), 2);
    assert_eq!(loaded.outputs().len(), 2);
    assert_eq!(
        loaded.outputs()["mesa3dgl26-gallium"]
            .values("LIBS")
            .unwrap()
            .first()
            .map(String::as_str),
        Some("mesa3dgl26-gallium_library")
    );
    assert_eq!(
        loaded.outputs()["mesa3dgl26-hidd"]
            .values("LIBS")
            .unwrap()
            .first()
            .map(String::as_str),
        Some("mesa3dgl26-hidd_library")
    );
    assert_eq!(loaded.binding(), &fixture.binding);
}

#[test]
fn rejects_collection_omission_extra_order_and_inventory_scope_mismatch() {
    let fixture = CollectionFixture::new();

    let mut omitted = fixture.binding.clone();
    omitted.outputs.pop();
    assert!(fixture.load(&omitted).is_err());

    let mut extra = fixture.binding.clone();
    extra.outputs.push(extra.outputs[0].clone());
    assert!(fixture.load(&extra).is_err());

    let mut unsorted = fixture.binding.clone();
    unsorted.outputs.swap(0, 1);
    assert!(fixture.load(&unsorted).is_err());

    let mut wrong_scope = fixture.binding.clone();
    wrong_scope.selection.scope = MesaGenmoduleInventoryScope::FullTarget;
    assert!(fixture.load(&wrong_scope).is_err());
}

#[test]
fn rejects_inventory_and_raw_make_digest_drift() {
    let fixture = CollectionFixture::new();
    let inventory_path = fixture.input_root.join(MESA_GENMODULE_SELECTION_FILE);
    let mut inventory = fs::read(&inventory_path).unwrap();
    inventory.push(b' ');
    fs::write(&inventory_path, inventory).unwrap();
    assert!(fixture.load(&fixture.binding).is_err());

    let fixture = CollectionFixture::new();
    let output_path = fixture
        .input_root
        .join(mesa_genmodule_output_relative_path("mesa3dgl26-gallium").unwrap());
    let invalid_make = b"not a literal genmodule projection\n";
    fs::write(&output_path, invalid_make).unwrap();
    let mut rehashed = fixture.binding.clone();
    rehashed.outputs[0].makefile_sha256 = sha256_bytes(invalid_make);
    assert!(fixture.load(&rehashed).is_err());
}

#[test]
fn rejects_source_configuration_drift_and_override_presence_mismatch() {
    let fixture = CollectionFixture::new();
    fs::write(
        fixture.source_root.join("workbench/libs/gl/gl.conf"),
        b"changed\n",
    )
    .unwrap();
    assert!(fixture.load(&fixture.binding).is_err());

    let fixture = CollectionFixture::new();
    fs::write(
        fixture.source_root.join("workbench/libs/mesa/gallium.conf"),
        b"changed override\n",
    )
    .unwrap();
    assert!(fixture.load(&fixture.binding).is_err());

    let fixture = CollectionFixture::new();
    let mut wrong_presence = fixture.binding.clone();
    let hidd = wrong_presence
        .outputs
        .iter_mut()
        .find(|output| output.producer.owner == "mesa3dgl26-hidd")
        .unwrap();
    hidd.override_configuration_sha256 = Some(digest('d'));
    assert!(fixture.load(&wrong_presence).is_err());

    let fixture = CollectionFixture::new();
    let mut omitted_override_digest = fixture.binding.clone();
    let gallium = omitted_override_digest
        .outputs
        .iter_mut()
        .find(|output| output.producer.owner == "mesa3dgl26-gallium")
        .unwrap();
    gallium.override_configuration_sha256 = None;
    assert!(fixture.load(&omitted_override_digest).is_err());
}

#[test]
fn rejects_unsafe_owner_path_helper_input() {
    for owner in ["", ".", "..", "../escape", "owner/name", "owner\\name"] {
        assert!(
            mesa_genmodule_output_relative_path(owner).is_err(),
            "{owner:?}"
        );
    }
    assert!(mesa_genmodule_output_relative_path(&"a".repeat(256)).is_err());
    assert_eq!(
        mesa_genmodule_output_relative_path("mesa3dgl26-library").unwrap(),
        PathBuf::from("genmodule-outputs/mesa3dgl26-library/module.make")
    );
}

#[test]
fn collection_binding_wire_is_closed_and_requires_optional_presence_fields() {
    let fixture = CollectionFixture::new();
    let value = serde_json::to_value(&fixture.binding).unwrap();

    let mut unknown = value.clone();
    unknown["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<MesaGenmoduleCollectionBinding>(unknown).is_err());

    let mut missing_override = value.clone();
    missing_override["outputs"][0]
        .as_object_mut()
        .unwrap()
        .remove("override_configuration_sha256");
    assert!(serde_json::from_value::<MesaGenmoduleCollectionBinding>(missing_override).is_err());

    let mut missing_producer_override = value;
    missing_producer_override["outputs"][0]["producer"]
        .as_object_mut()
        .unwrap()
        .remove("override_configuration");
    assert!(
        serde_json::from_value::<MesaGenmoduleCollectionBinding>(missing_producer_override)
            .is_err()
    );
}
