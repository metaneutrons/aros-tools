//! Maintained native positive/negative conformance tests, never compiler proof.

use aros_common::diagnostic::{DiagnosticCode, DiagnosticSet};
use aros_common::digest::sha256_bytes;
use serde_json::{json, Value};

use crate::{canonical, Recipe};

fn material() -> Value {
    json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": "1".repeat(40), "source_tree": "2".repeat(40),
        "producer_commit": "3".repeat(40), "producer_tree": "4".repeat(40),
        "tools_commit": "5".repeat(40), "tools_tree": "6".repeat(40),
        "source_date_epoch": 0,
        "source_lock_sha256": "a".repeat(64), "profiles_sha256": "b".repeat(64),
        "patches": [{"path": "patches/Größe.patch", "sha256": "c".repeat(64)}]
    })
}

fn signed(mut value: Value) -> Vec<u8> {
    value["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&value).unwrap()));
    serde_json::to_vec(&value).unwrap()
}

fn rejected(bytes: &[u8], code: DiagnosticCode) {
    let error = Recipe::parse(bytes).unwrap_err();
    let envelope = error.diagnostics();
    assert_eq!(envelope.schema, DiagnosticSet::SCHEMA);
    assert_eq!(envelope.diagnostics.len(), 1);
    assert_eq!(envelope.diagnostics[0].code, code);
    assert!(envelope.diagnostics[0]
        .hint
        .as_ref()
        .is_some_and(|hint| !hint.is_empty()));
    let rendered = serde_json::to_vec(envelope).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&rendered).unwrap()["schema"],
        DiagnosticSet::SCHEMA
    );
}

#[test]
fn recipe_round_trip_exposes_only_validated_claims() {
    let input = signed(material());
    let parsed = Recipe::parse(&input).unwrap();
    let record: Value = serde_json::from_slice(&input).unwrap();
    assert_eq!(parsed.sha256().as_str(), record["recipe_sha256"]);
    assert_eq!(parsed.source().0.as_str(), "1".repeat(40));
    assert_eq!(parsed.producer().1.as_str(), "4".repeat(40));
    assert_eq!(parsed.tools().0.as_str(), "5".repeat(40));
    assert_eq!(parsed.source_lock_sha256().as_str(), "a".repeat(64));
    assert_eq!(parsed.profiles_sha256().as_str(), "b".repeat(64));
    assert_eq!(parsed.source_date_epoch(), 0);
    assert_eq!(parsed.patches()[0].path(), "patches/Größe.patch");
    assert_eq!(parsed.patches()[0].sha256().as_str(), "c".repeat(64));
}

#[test]
fn recipe_material_matches_an_independent_python_known_answer() {
    // Measured with Python hashlib/json's legacy encoding rule on the explicit
    // synthetic material() above. This is a fixture, not a runtime source pin.
    let encoded = canonical::bytes(&material()).unwrap();
    assert_eq!(encoded.len(), 704);
    assert_eq!(
        sha256_bytes(&encoded).as_str(),
        "fb879dd81e2508dc3528958a39b24326556e93859ce9b8d8f38c2da558348a1a"
    );
}

#[test]
fn safe_failure_context_names_the_actual_contract_problem() {
    let mut value = material();
    value.as_object_mut().unwrap().remove("tools_tree");
    let error = Recipe::parse(&signed(value)).unwrap_err();
    assert!(error.to_string().contains("missing field `tools_tree`"));
    let mut value = material();
    value["schema"] = json!("unsupported-private-value");
    let error = Recipe::parse(&signed(value)).unwrap_err();
    assert!(error.to_string().contains("unsupported schema"));
    assert!(!error.to_string().contains("unsupported-private-value"));
}

#[test]
fn untrusted_values_cannot_spoof_the_reported_parse_reason() {
    let mut value = material();
    value["source_date_epoch"] =
        json!("SHA-256 digest expected `aros-toolchain-recipe-v2` fixture-private-value");
    let error = Recipe::parse(&signed(value)).unwrap_err();
    let message = &error.diagnostics().diagnostics[0].message;
    assert!(message.contains("source_date_epoch integer"));
    assert!(!message.contains("unsupported schema"));
    assert!(!message.contains("SHA-256 digest"));
    assert!(!message.contains("fixture-private-value"));
}

#[test]
fn unknown_missing_duplicate_and_trailing_fields_fail() {
    let mut value = material();
    value["surprise"] = json!(true);
    rejected(&signed(value), DiagnosticCode::ProducerContract);
    let mut value = material();
    value.as_object_mut().unwrap().remove("tools_tree");
    rejected(&signed(value), DiagnosticCode::ProducerContract);
    let original = String::from_utf8(signed(material())).unwrap();
    let duplicate = original.replacen('{', "{\"source_date_epoch\":0,", 1);
    rejected(duplicate.as_bytes(), DiagnosticCode::ProducerContract);
    rejected(
        format!("{original} {{}}").as_bytes(),
        DiagnosticCode::ProducerContract,
    );
}

#[test]
fn unknown_and_duplicate_patch_members_fail_before_digest_validation() {
    let mut value = material();
    value["patches"][0]["unknown"] = json!("not permitted");
    rejected(&signed(value), DiagnosticCode::ProducerContract);
    let input = String::from_utf8(signed(material())).unwrap();
    let duplicate = input.replacen("\"path\":", "\"path\":\"other.patch\",\"path\":", 1);
    rejected(duplicate.as_bytes(), DiagnosticCode::ProducerContract);
}

#[test]
fn unsupported_schema_and_noncanonical_identities_fail() {
    for (field, value) in [
        ("schema", "aros-toolchain-recipe-v3".to_owned()),
        ("source_commit", "F".repeat(40)),
        ("source_tree", "1".repeat(39)),
        ("tools_commit", "main".to_owned()),
        ("source_lock_sha256", "A".repeat(64)),
        ("profiles_sha256", "b".repeat(63)),
    ] {
        let mut input = material();
        input[field] = json!(value);
        rejected(&signed(input), DiagnosticCode::ProducerContract);
    }
}

#[test]
fn bool_negative_float_and_overflow_epochs_fail() {
    let original = String::from_utf8(signed(material())).unwrap();
    for spelling in ["true", "-1", "1.0", "1e1", "18446744073709551616", "null"] {
        let input = original.replace(
            "\"source_date_epoch\":0",
            &format!("\"source_date_epoch\":{spelling}"),
        );
        rejected(input.as_bytes(), DiagnosticCode::ProducerContract);
    }
    let mut value = material();
    value["source_date_epoch"] = json!(u64::MAX);
    assert_eq!(
        Recipe::parse(&signed(value)).unwrap().source_date_epoch(),
        u64::MAX
    );
}

#[test]
fn complete_self_digest_binds_every_changed_material_field() {
    let original: Value = serde_json::from_slice(&signed(material())).unwrap();
    for field in [
        "source_commit",
        "source_tree",
        "producer_commit",
        "producer_tree",
        "tools_commit",
        "tools_tree",
    ] {
        let mut changed = original.clone();
        changed[field] = json!("9".repeat(40));
        rejected(
            &serde_json::to_vec(&changed).unwrap(),
            DiagnosticCode::ProducerIdentity,
        );
    }
    let mut changed = original;
    changed["patches"][0]["sha256"] = json!("d".repeat(64));
    rejected(
        &serde_json::to_vec(&changed).unwrap(),
        DiagnosticCode::ProducerIdentity,
    );
}

#[test]
fn patch_paths_reject_traversal_and_noncanonical_spellings() {
    for path in [
        "",
        "/absolute.patch",
        "../escape",
        "a/../b",
        "a//b",
        "a/./b",
        "a/",
        "C:/a",
        "a\\b",
        "a\nb",
        "a\0b",
    ] {
        let mut value = material();
        value["patches"][0]["path"] = json!(path);
        rejected(&signed(value), DiagnosticCode::ProducerContract);
    }
    let mut value = material();
    value["patches"][0]["path"] = json!("patches/space and Größe.patch");
    Recipe::parse(&signed(value)).unwrap();
}

#[test]
fn patch_order_is_strict_but_empty_patch_sets_are_valid_claims() {
    for paths in [["z.patch", "a.patch"], ["a.patch", "a.patch"]] {
        let mut value = material();
        value["patches"] = json!(paths.map(|path| json!({"path":path,"sha256":"c".repeat(64)})));
        rejected(&signed(value), DiagnosticCode::ProducerContract);
    }
    let mut value = material();
    value["patches"] = json!([]);
    assert!(Recipe::parse(&signed(value)).unwrap().patches().is_empty());
}

#[test]
fn canonical_encoding_matches_the_independent_m0_utf8_vector() {
    let golden: Value = serde_json::from_str(include_str!(
        "../../../scripts/fixtures/toolchain-producer/package-v1.json"
    ))
    .unwrap();
    let entry = &golden["manifest"]["files"][2];
    let bytes = canonical::bytes(entry).unwrap();
    assert_eq!(
        std::str::from_utf8(&bytes).unwrap(),
        golden["canonical_entry_utf8"]
    );
    assert_eq!(
        sha256_bytes(&bytes).as_str(),
        golden["canonical_entry_sha256"]
    );
    assert!(!bytes.windows(2).any(|part| part == b"\\u"));
}

#[test]
fn canonical_encoding_matches_m0_receipt_without_implementing_receipt_reuse() {
    let mut example: Value = serde_json::from_str(include_str!(
        "../../../scripts/fixtures/toolchain-producer/receipt-v1.json"
    ))
    .unwrap();
    let expected = example
        .as_object_mut()
        .unwrap()
        .remove("receipt_sha256")
        .unwrap();
    assert_eq!(
        sha256_bytes(&canonical::bytes(&example).unwrap()).as_str(),
        expected
    );
}

#[test]
fn canonical_domain_depth_and_size_limits_fail_closed() {
    for value in [json!(1.0), json!(-1)] {
        assert!(canonical::bytes(&value).is_err());
    }
    let mut value = Value::Null;
    for _ in 0..66 {
        value = json!([value]);
    }
    assert!(canonical::bytes(&value).is_err());
    assert!(canonical::bytes(&json!("x".repeat(canonical::MAX_DOCUMENT_BYTES))).is_err());
    rejected(
        &vec![b' '; canonical::MAX_DOCUMENT_BYTES + 1],
        DiagnosticCode::ProducerContract,
    );
    rejected(b"\xff", DiagnosticCode::ProducerContract);
}

#[test]
fn diagnostic_never_echoes_an_untrusted_input_value() {
    let mut value = material();
    value["source_date_epoch"] = json!("fixture-private-value-do-not-emit");
    let error = Recipe::parse(&signed(value)).unwrap_err();
    let rendered = serde_json::to_string(error.diagnostics()).unwrap();
    assert!(!rendered.contains("fixture-private-value-do-not-emit"));
    assert!(rendered.contains("line"));
    assert!(rendered.contains("column"));
    assert!(rendered.contains("AX0101"));
}

#[test]
fn registered_diagnostics_match_the_versioned_contract() {
    let contract: toml::Value = toml::from_str(include_str!(
        "../../../contracts/toolchain-producer-v1.toml"
    ))
    .unwrap();
    let actual: Vec<_> = [
        DiagnosticCode::ProducerContract,
        DiagnosticCode::ProducerIdentity,
        DiagnosticCode::ProducerPrerequisite,
        DiagnosticCode::ProducerPreflight,
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    let expected: Vec<_> = contract["diagnostics"]["registered_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(actual, expected);
    for code in [
        DiagnosticCode::ProducerContract,
        DiagnosticCode::ProducerIdentity,
        DiagnosticCode::ProducerPrerequisite,
        DiagnosticCode::ProducerPreflight,
    ] {
        assert_eq!(serde_json::to_value(code).unwrap(), code.to_string());
    }
}
