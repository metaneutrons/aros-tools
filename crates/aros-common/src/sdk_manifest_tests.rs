use super::{
    validate_inventory, SdkAbiValue, SdkIdentity, SdkManifest, SdkSourceIdentity, SdkTarget,
    SDK_MANIFEST_SCHEMA, SDK_MAX_METADATA_BYTES, SDK_MAX_REGULAR_BYTES,
};
use crate::{sha256_bytes, toolchain_inventory_sha256, ArosToolchainManifestEntry, Sha256Digest};
use std::collections::BTreeMap;

fn minimal_sdk_identity() -> SdkIdentity {
    SdkIdentity {
        sdk_id: "core".into(),
        version: "1.2.3".into(),
        target: SdkTarget {
            target_triple: "i686-aros".into(),
            abi: BTreeMap::from([("data_model".into(), SdkAbiValue::Text("ilp32".into()))]),
        },
        source: SdkSourceIdentity {
            commit: "1".repeat(40),
            git_tree: "2".repeat(40),
            content_sha256: sha256_bytes(b"selected source tree"),
            contract_sha256: sha256_bytes(b"selected source contract"),
        },
        source_date_epoch: 1,
    }
}

fn file(path: &str, size: u64) -> ArosToolchainManifestEntry {
    ArosToolchainManifestEntry {
        path: path.into(),
        mode: "0644".into(),
        kind: "file".into(),
        sha256: Some(sha256_bytes(path.as_bytes()).to_string()),
        size: Some(size),
        target: None,
    }
}

fn directory(path: &str) -> ArosToolchainManifestEntry {
    ArosToolchainManifestEntry {
        path: path.into(),
        mode: "0755".into(),
        kind: "directory".into(),
        sha256: None,
        size: None,
        target: None,
    }
}

fn symlink(path: &str, target: &str) -> ArosToolchainManifestEntry {
    ArosToolchainManifestEntry {
        path: path.into(),
        mode: "0777".into(),
        kind: "symlink".into(),
        sha256: None,
        size: None,
        target: Some(target.into()),
    }
}

fn minimal_sdk_manifest() -> SdkManifest {
    manifest(vec![directory("bin"), file("bin/aros", 3)])
}

fn manifest(files: Vec<ArosToolchainManifestEntry>) -> SdkManifest {
    let tree_sha256 = toolchain_inventory_sha256(&files).expect("inventory serialization");
    SdkManifest {
        schema: SDK_MANIFEST_SCHEMA.into(),
        identity: minimal_sdk_identity(),
        tree_sha256: Sha256Digest::parse(&tree_sha256).expect("SHA-256 digest"),
        files,
    }
}

fn manifest_bytes(value: &SdkManifest) -> Vec<u8> {
    serde_json::to_vec(value).expect("manifest serialization")
}

fn sorted(mut entries: Vec<ArosToolchainManifestEntry>) -> Vec<ArosToolchainManifestEntry> {
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

#[test]
fn sdk_asset_name_is_host_independent_and_target_digest_covers_abi() {
    let identity = minimal_sdk_identity();
    assert_eq!(identity.asset_name().unwrap(), "aros-sdk-core-1.2.3.tar.xz");

    let original_target_digest = identity.target.sha256().unwrap();
    let mut changed_abi = identity.clone();
    changed_abi
        .target
        .abi
        .insert("data_model".into(), SdkAbiValue::Text("lp64".into()));
    assert_ne!(original_target_digest, changed_abi.target.sha256().unwrap());
    assert_eq!(
        identity.asset_name().unwrap(),
        changed_abi.asset_name().unwrap()
    );

    let mut other_target = identity.clone();
    other_target.target.target_triple = "aarch64-aros".into();
    assert_eq!(
        identity.asset_name().unwrap(),
        other_target.asset_name().unwrap()
    );
}

#[test]
fn identity_requires_canonical_semver() {
    assert!(minimal_sdk_identity().validate().is_ok());
    let mut prerelease = minimal_sdk_identity();
    prerelease.version = "1.2.3-rc.1".into();
    assert!(prerelease.validate().is_ok());

    for version in ["v1.2.3", "01.2.3", "1.2", "1.2.3+local"] {
        let mut identity = minimal_sdk_identity();
        identity.version = version.into();
        assert!(
            identity.validate().is_err(),
            "accepted noncanonical {version}"
        );
    }

    let mut overlong = minimal_sdk_identity();
    overlong.version = format!("1.2.3-{}", "a".repeat(91));
    assert_eq!(overlong.version.len(), 97);
    assert!(overlong.validate().is_err());
}

#[test]
fn closed_manifest_rejects_unknown_and_duplicate_top_level_fields() {
    let valid = String::from_utf8(manifest_bytes(&minimal_sdk_manifest())).unwrap();
    let mut unknown = valid.clone();
    let end = unknown.rfind('}').unwrap();
    unknown.insert_str(end, ",\"unexpected\":true");
    assert!(SdkManifest::from_bytes(unknown.as_bytes()).is_err());

    let duplicate = valid.replacen(
        "\"schema\":",
        "\"schema\":\"aros-sdk-manifest-v2\",\"schema\":",
        1,
    );
    assert!(SdkManifest::from_bytes(duplicate.as_bytes()).is_err());
}

#[test]
fn closed_manifest_rejects_nested_unknown_identity_fields_and_wrong_schema() {
    let valid = String::from_utf8(manifest_bytes(&minimal_sdk_manifest())).unwrap();
    let identity_start = valid.find("\"identity\":{").unwrap();
    let identity_end = valid[identity_start..].find(",\"tree_sha256\"").unwrap() + identity_start;
    assert_eq!(valid.as_bytes()[identity_end - 1], b'}');
    let nested_unknown = format!(
        "{},\"unexpected\":true{}",
        &valid[..identity_end - 1],
        &valid[identity_end - 1..]
    );
    assert!(SdkManifest::from_bytes(nested_unknown.as_bytes()).is_err());

    let mut wrong_schema = minimal_sdk_manifest();
    wrong_schema.schema = "aros-sdk-manifest-v1".into();
    assert!(SdkManifest::from_bytes(&manifest_bytes(&wrong_schema)).is_err());
}

#[test]
fn manifest_decoder_rejects_documents_over_the_metadata_limit() {
    let oversized = vec![b' '; SDK_MAX_METADATA_BYTES + 1];
    assert!(SdkManifest::from_bytes(&oversized).is_err());
}

#[test]
fn abi_deserializer_rejects_duplicate_keys() {
    let valid = String::from_utf8(manifest_bytes(&minimal_sdk_manifest())).unwrap();
    let marker = "\"abi\":{";
    let start = valid.find(marker).unwrap() + marker.len();
    let end = start + valid[start..].find('}').unwrap();
    let facts = &valid[start..end];
    let duplicate = format!(
        "{},\"data_model\":\"ilp32\"{}",
        &valid[..end],
        &valid[end..]
    );
    assert!(facts.contains("data_model"));
    assert!(SdkManifest::from_bytes(duplicate.as_bytes()).is_err());
}

#[test]
fn abi_deserializer_rejects_arrays_null_and_negative_integers() {
    let valid = String::from_utf8(manifest_bytes(&minimal_sdk_manifest())).unwrap();
    for malformed in ["[]", "null", "-1"] {
        let json = valid.replacen(
            "\"data_model\":\"ilp32\"",
            &format!("\"data_model\":{malformed}"),
            1,
        );
        assert!(
            SdkManifest::from_bytes(json.as_bytes()).is_err(),
            "accepted ABI value {malformed}"
        );
    }
}

#[test]
fn identity_rejects_zero_git_and_content_digests() {
    let mut zero_commit = minimal_sdk_identity();
    zero_commit.source.commit = "0".repeat(40);
    assert!(zero_commit.validate().is_err());

    let mut zero_tree = minimal_sdk_identity();
    zero_tree.source.git_tree = "0".repeat(40);
    assert!(zero_tree.validate().is_err());

    let zero_digest = Sha256Digest::parse(&"0".repeat(64)).unwrap();
    let mut zero_content = minimal_sdk_identity();
    zero_content.source.content_sha256 = zero_digest.clone();
    assert!(zero_content.validate().is_err());

    let mut zero_contract = minimal_sdk_identity();
    zero_contract.source.contract_sha256 = zero_digest;
    assert!(zero_contract.validate().is_err());
}

#[test]
fn target_rejects_host_selectors_contradictions_and_long_abi_tokens() {
    for forbidden in ["host", "board", "compiler"] {
        let mut target = minimal_sdk_identity().target;
        target
            .abi
            .insert(forbidden.into(), SdkAbiValue::Text("value".into()));
        assert!(target.validate().is_err(), "accepted {forbidden} selector");
    }

    let mut contradictory = minimal_sdk_identity().target;
    contradictory.abi.insert(
        "target_triple".into(),
        SdkAbiValue::Text("x86_64-aros".into()),
    );
    assert!(contradictory.validate().is_err());

    let mut long_key = minimal_sdk_identity().target;
    long_key
        .abi
        .insert("k".repeat(65), SdkAbiValue::Boolean(true));
    assert!(long_key.validate().is_err());

    let mut long_value = minimal_sdk_identity().target;
    long_value
        .abi
        .insert("abi_name".into(), SdkAbiValue::Text("a".repeat(257)));
    assert!(long_value.validate().is_err());

    let mut long_triple = minimal_sdk_identity().target;
    long_triple.target_triple = "a".repeat(129);
    assert!(long_triple.validate().is_err());
}

#[test]
fn manifest_hash_covers_the_sorted_complete_payload_inventory() {
    let files = vec![
        directory("bin"),
        file("bin/aros", 3),
        directory("share"),
        file("share/data", 5),
    ];
    let complete = manifest(files.clone());
    assert!(complete.validate().is_ok());
    assert_eq!(
        complete.tree_sha256.as_str(),
        toolchain_inventory_sha256(&files).unwrap()
    );

    let unsorted = vec![file("z-tool", 1), file("a-tool", 1)];
    assert!(validate_inventory(&unsorted).is_err());

    let mut omitted_from_hash = complete;
    omitted_from_hash.files.push(file("z-extra", 1));
    assert!(omitted_from_hash.validate().is_err());
}

#[test]
fn inventory_rejects_casefold_and_unicode_normalization_collisions() {
    assert!(validate_inventory(&[file("same.txt", 1), file("same.txt", 2)]).is_err());
    assert!(validate_inventory(&[file("File.txt", 1), file("file.txt", 1)]).is_err());

    let unicode_collision = sorted(vec![file("é.txt", 1), file("e\u{301}.txt", 1)]);
    assert!(validate_inventory(&unicode_collision).is_err());
}

#[test]
fn inventory_requires_declared_real_parent_directories() {
    assert!(validate_inventory(&[file("bin/tool", 1)]).is_err());
    assert!(validate_inventory(&[directory("link"), file("link/tool", 1)]).is_ok());
}

#[test]
fn inventory_rejects_unsafe_paths() {
    let long_component = "x".repeat(256);
    for path in [
        "",
        "/absolute",
        "../escape",
        "a/../escape",
        "a//b",
        "./tool",
        "tool/",
        "a\\b",
        "bad\0name",
        "C:/absolute",
        "CON",
        "trailing.",
        long_component.as_str(),
    ] {
        assert!(
            validate_inventory(&[file(path, 1)]).is_err(),
            "accepted unsafe path {path:?}"
        );
    }
    assert!(validate_inventory(&[file("sdk-manifest.json", 1)]).is_err());
}

#[test]
fn inventory_rejects_invalid_types_modes_mixed_fields_and_digests() {
    let mut invalid_type = file("tool", 1);
    invalid_type.kind = "fifo".into();
    assert!(validate_inventory(&[invalid_type]).is_err());

    let mut invalid_file_mode = file("tool", 1);
    invalid_file_mode.mode = "0700".into();
    assert!(validate_inventory(&[invalid_file_mode]).is_err());

    let mut invalid_directory_mode = directory("data");
    invalid_directory_mode.mode = "0700".into();
    assert!(validate_inventory(&[invalid_directory_mode, file("tool", 1)]).is_err());

    let mut invalid_symlink_mode = symlink("link", "tool");
    invalid_symlink_mode.mode = "0755".into();
    assert!(validate_inventory(&[invalid_symlink_mode, file("tool", 1)]).is_err());

    let mut mixed_file_fields = file("tool", 1);
    mixed_file_fields.target = Some("other".into());
    assert!(validate_inventory(&[mixed_file_fields]).is_err());

    let mut mixed_directory_fields = directory("data");
    mixed_directory_fields.size = Some(0);
    assert!(validate_inventory(&[mixed_directory_fields, file("tool", 1)]).is_err());

    let mut mixed_symlink_fields = symlink("link", "tool");
    mixed_symlink_fields.sha256 = Some(sha256_bytes(b"tool").to_string());
    assert!(validate_inventory(&[mixed_symlink_fields, file("tool", 1)]).is_err());

    let mut invalid_digest = file("tool", 1);
    invalid_digest.sha256 = Some("not-a-digest".into());
    assert!(validate_inventory(&[invalid_digest]).is_err());

    let mut uppercase_digest = file("tool", 1);
    uppercase_digest.sha256 = Some("A".repeat(64));
    assert!(validate_inventory(&[uppercase_digest]).is_err());
}

#[test]
fn inventory_enforces_the_two_gibibyte_regular_file_limit() {
    assert!(validate_inventory(&[file("large", SDK_MAX_REGULAR_BYTES)]).is_ok());
    assert!(validate_inventory(&[file("large", SDK_MAX_REGULAR_BYTES + 1)]).is_err());

    let half_plus_one = SDK_MAX_REGULAR_BYTES / 2 + 1;
    let multiple_files = vec![file("a", half_plus_one), file("b", half_plus_one)];
    assert!(validate_inventory(&multiple_files).is_err());
}

#[test]
fn inventory_requires_at_least_one_regular_file() {
    assert!(validate_inventory(&[directory("empty")]).is_err());
    assert!(validate_inventory(&[symlink("link", "missing")]).is_err());
}

#[test]
fn inventory_accepts_direct_transitive_and_directory_alias_links() {
    let direct = sorted(vec![file("target", 1), symlink("direct", "target")]);
    assert!(validate_inventory(&direct).is_ok());

    let transitive = sorted(vec![
        symlink("a-link", "b-link"),
        symlink("b-link", "payload"),
        file("payload", 1),
    ]);
    assert!(validate_inventory(&transitive).is_ok());

    let directory_alias = sorted(vec![
        symlink("alias", "real"),
        directory("real"),
        file("real/data", 1),
        symlink("through-alias", "alias/data"),
        file("z-file", 1),
    ]);
    assert!(validate_inventory(&directory_alias).is_ok());
}

#[test]
fn inventory_accepts_links_to_the_payload_root() {
    let entries = sorted(vec![
        symlink("root-link", "."),
        directory("sub"),
        symlink("sub/root-link", ".."),
        file("tool", 1),
    ]);
    assert!(validate_inventory(&entries).is_ok());
}

#[test]
fn inventory_rejects_escaping_dangling_cyclic_and_non_directory_links() {
    let root_escape = sorted(vec![file("tool", 1), symlink("escape", "../outside")]);
    assert!(validate_inventory(&root_escape).is_err());

    let nested_escape = sorted(vec![
        directory("sub"),
        symlink("sub/escape", "../../outside"),
        file("tool", 1),
    ]);
    assert!(validate_inventory(&nested_escape).is_err());

    let dangling = sorted(vec![file("tool", 1), symlink("dangling", "missing")]);
    assert!(validate_inventory(&dangling).is_err());

    let cycle = sorted(vec![symlink("a", "b"), symlink("b", "a"), file("tool", 1)]);
    assert!(validate_inventory(&cycle).is_err());

    let non_directory = sorted(vec![
        symlink("link", "node/child"),
        file("node", 1),
        file("tool", 1),
    ]);
    assert!(validate_inventory(&non_directory).is_err());
}

#[test]
fn link_parent_components_do_not_hide_missing_or_non_directory_traversal() {
    for target in ["missing/../tool", "tool/../tool", "tool/", "tool/."] {
        assert!(
            validate_inventory(&sorted(vec![file("tool", 1), symlink("link", target)])).is_err(),
            "{target}"
        );
    }
    assert!(validate_inventory(&sorted(vec![
        directory("real"),
        file("tool", 1),
        symlink("alias", "real"),
        symlink("link", "alias/../tool")
    ]))
    .is_ok());
    assert!(validate_inventory(&sorted(vec![
        directory("real"),
        directory("real/sub"),
        file("real/tool", 1),
        file("tool", 1),
        symlink("alias", "real/sub"),
        symlink("link", "alias/../tool")
    ]))
    .is_ok());
    assert!(validate_inventory(&sorted(vec![
        file("tool", 1),
        symlink("alias", "missing"),
        symlink("link", "alias/../tool")
    ]))
    .is_err());
}
