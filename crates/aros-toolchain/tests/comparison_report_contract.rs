use aros_common::sha256_bytes;
use aros_toolchain::{canonical, release_index::PackageComparisonReport};
use serde_json::{json, Value};

const ARCHIVE: &str = "aros-toolchain-v1-llvm11.0.0-macos-aarch64-pc-x86_64.tar.xz";
const MAX_REPORT_BYTES: usize = 16 * 1024 * 1024;

fn member(name: &str, contents: &[u8]) -> Value {
    json!({
        "name": name,
        "sha256": sha256_bytes(contents).to_string(),
        "size": contents.len() as u64
    })
}

fn valid_report() -> Value {
    let members = json!([
        member(ARCHIVE, b"synthetic archive"),
        member(&format!("{ARCHIVE}.manifest.json"), b"synthetic manifest"),
        member(&format!("{ARCHIVE}.sha256"), b"synthetic checksum"),
        member(&format!("{ARCHIVE}.spdx.json"), b"synthetic SPDX")
    ]);
    let package_set_sha256 = sha256_bytes(&canonical::bytes(&members).unwrap()).to_string();
    json!({
        "schema": 1,
        "operation": "compare",
        "byte_identical": true,
        "package_set_sha256": package_set_sha256,
        "members": members
    })
}

#[test]
fn parses_closed_canonical_four_member_comparison_report() {
    let value = valid_report();
    let expected_digest = sha256_bytes(&canonical::bytes(&value["members"]).unwrap());
    let bytes = canonical::bytes(&value).unwrap();
    let report = PackageComparisonReport::parse(&bytes).unwrap();
    let expected_names = vec![
        ARCHIVE.to_owned(),
        format!("{ARCHIVE}.manifest.json"),
        format!("{ARCHIVE}.sha256"),
        format!("{ARCHIVE}.spdx.json"),
    ];

    assert_eq!(report.schema, 1);
    assert_eq!(report.operation, "compare");
    assert!(report.byte_identical);
    assert_eq!(report.members.len(), 4);
    assert_eq!(
        report
            .members
            .iter()
            .map(|member| member.name.clone())
            .collect::<Vec<_>>(),
        expected_names
    );
    assert!(report.members.iter().all(|member| member.size > 0));
    assert_eq!(report.package_set_sha256, expected_digest);
    assert_eq!(
        canonical::bytes(&serde_json::to_value(&report).unwrap()).unwrap(),
        bytes
    );
}

#[test]
fn rejects_open_or_inconsistent_comparison_reports() {
    let valid = valid_report();
    let mut unknown_top_level = valid.clone();
    unknown_top_level["extra"] = json!(true);

    let mut unknown_member_field = valid.clone();
    unknown_member_field["members"][0]["extra"] = json!(true);

    let mut missing_member = valid.clone();
    missing_member["members"].as_array_mut().unwrap().pop();

    let mut unsorted_members = valid.clone();
    unsorted_members["members"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);

    let mut duplicate_member = valid.clone();
    duplicate_member["members"][1] = duplicate_member["members"][0].clone();

    let mut false_byte_identity = valid.clone();
    false_byte_identity["byte_identical"] = json!(false);

    let mut wrong_operation = valid.clone();
    wrong_operation["operation"] = json!("write");

    let mut wrong_schema = valid.clone();
    wrong_schema["schema"] = json!(2);

    let mut inconsistent_package_digest = valid;
    let original_digest = inconsistent_package_digest["package_set_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    let replacement_prefix = if original_digest.starts_with('0') {
        "1"
    } else {
        "0"
    };
    inconsistent_package_digest["package_set_sha256"] =
        json!(format!("{replacement_prefix}{}", &original_digest[1..]));

    for (case, invalid) in [
        ("unknown top-level field", unknown_top_level),
        ("unknown member field", unknown_member_field),
        ("missing member", missing_member),
        ("unsorted members", unsorted_members),
        ("duplicate member", duplicate_member),
        ("false byte identity", false_byte_identity),
        ("wrong operation", wrong_operation),
        ("wrong schema", wrong_schema),
        ("inconsistent package digest", inconsistent_package_digest),
    ] {
        let bytes = serde_json::to_vec(&invalid).unwrap();
        assert!(
            PackageComparisonReport::parse(&bytes).is_err(),
            "accepted {case}"
        );
    }
}

#[test]
fn rejects_duplicate_top_level_and_member_json_keys() {
    let valid = valid_report();
    let serialized = serde_json::to_string(&valid).unwrap();
    let duplicate_schema = serialized.replacen("\"schema\":1", "\"schema\":1,\"schema\":1", 1);
    assert_ne!(duplicate_schema, serialized);
    assert!(PackageComparisonReport::parse(duplicate_schema.as_bytes()).is_err());

    let member_digest = valid["members"][0]["sha256"].as_str().unwrap();
    let member_key = format!("\"sha256\":\"{member_digest}\"");
    let duplicate_member_key = serialized.replacen(
        &member_key,
        &format!("{member_key},\"sha256\":\"{member_digest}\""),
        1,
    );
    assert_ne!(duplicate_member_key, serialized);
    assert!(PackageComparisonReport::parse(duplicate_member_key.as_bytes()).is_err());
}

#[test]
fn rejects_malformed_empty_and_over_limit_comparison_reports() {
    assert!(PackageComparisonReport::parse(b"{\"schema\":").is_err());
    assert!(PackageComparisonReport::parse(b"").is_err());

    let mut oversized = serde_json::to_vec(&valid_report()).unwrap();
    oversized.resize(MAX_REPORT_BYTES + 1, b' ');
    assert!(PackageComparisonReport::parse(&oversized).is_err());
}
