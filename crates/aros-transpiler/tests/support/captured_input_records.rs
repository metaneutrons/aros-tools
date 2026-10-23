//! Private data evidence, never a native execution or SDK authorization.

use aros_common::{make_input_snapshot::VerifiedMakeInputReads, sha256_bytes, Sha256Digest};
use std::path::Path;

pub fn retain(
    output: &Path,
    reads: &VerifiedMakeInputReads,
    roots_sha256: &Sha256Digest,
    initial_stdout_sha256: &Sha256Digest,
    capture_sha256: &Sha256Digest,
) -> Sha256Digest {
    let bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "schema":"aros-local-make-input-reads-v1",
        "roots_sha256":roots_sha256,
        "initial_stdout_sha256":initial_stdout_sha256,
        "capture_sha256":capture_sha256,
        "sdk_qualification":false,
        "reads":reads.reads().iter().map(|read| serde_json::json!({
            "requested":read.requested, "identity":read.identity,
            "path":read.path, "content_sha256":read.content_sha256
        })).collect::<Vec<_>>()
    }))
    .unwrap();
    super::write_private(&output.join("source-input-reads.json"), &bytes);
    sha256_bytes(&bytes)
}

#[test]
fn retained_read_records_bind_exact_reads_absence_bytes_and_private_mode() {
    use aros_common::{
        make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
        measure_tree_content_cas,
    };
    use std::{fs, os::unix::fs::PermissionsExt as _};
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let input = root.join("input");
    let output = root.join("output");
    fs::create_dir(&input).unwrap();
    fs::create_dir(&output).unwrap();
    fs::write(input.join("file.mk"), b"VALUE := selected\n").unwrap();
    let snapshot = MakeInputSnapshot::capture(
        &input,
        &[MakeInputRootSelection {
            root: input.clone(),
            content_sha256: measure_tree_content_cas(&input)
                .unwrap()
                .payload_digest_excluding(None),
        }],
    )
    .unwrap();
    let ((), reads) = snapshot
        .transaction(|reader| {
            reader.read("file.mk")?;
            reader.read("absent.mk")?;
            Ok(())
        })
        .unwrap();
    let binding = sha256_bytes(b"synthetic binding");
    let digest = retain(&output, &reads, &binding, &binding, &binding);
    let path = output.join("source-input-reads.json");
    let bytes = fs::read(&path).unwrap();
    assert_eq!(sha256_bytes(&bytes), digest);
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["roots_sha256"], serde_json::json!(binding));
    assert_eq!(value["reads"][0]["requested"], "file.mk");
    assert_eq!(
        value["reads"][0]["content_sha256"],
        serde_json::json!(sha256_bytes(b"VALUE := selected\n"))
    );
    assert_eq!(value["reads"][1]["requested"], "absent.mk");
    assert!(value["reads"][1]["content_sha256"].is_null());
    assert_eq!(value["sdk_qualification"], false);
}
