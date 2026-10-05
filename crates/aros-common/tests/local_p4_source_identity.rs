//! Opt-in measurement of the retained dirty P4 source, not payload qualification.

use aros_common::local_source::LocalSourceIdentity;
use std::{fs, path::PathBuf};

#[test]
#[ignore = "requires explicit retained P4 source and unused output evidence path"]
fn actual_local_p4_source_snapshot_is_repeatable_without_source_writes() {
    let root = PathBuf::from(std::env::var_os("AROS_TEST_P4_SOURCE").expect("explicit P4 source"));
    let expected_head = std::env::var("AROS_TEST_P4_SOURCE_EXPECTED_HEAD")
        .expect("explicit expected P4 checkout commit");
    let output = PathBuf::from(
        std::env::var_os("AROS_RV3_SOURCE_IDENTITY_OUTPUT").expect("unused evidence output"),
    );
    assert!(!output.exists(), "do not replace historical evidence");
    assert!(
        !output.starts_with(&root),
        "evidence must remain outside measured source"
    );
    let identity = LocalSourceIdentity::capture(&root, "esp32p4-d1001").unwrap();
    identity.verify(&root, "esp32p4-d1001").unwrap();
    assert_eq!(identity.head_baseline, expected_head);
    assert!(
        identity.entry_count > 1000,
        "measure whole source, not only exported inputs"
    );
    let bytes = serde_json::to_vec_pretty(&identity).unwrap();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .unwrap();
    std::io::Write::write_all(&mut file, &bytes).unwrap();
    file.sync_all().unwrap();
}
