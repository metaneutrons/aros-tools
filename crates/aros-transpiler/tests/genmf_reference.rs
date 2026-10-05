//! Explicit local differential against previously generated reference outputs.
//! This checks text equality, not configured discovery or native graph closure.

use aros_common::{local_source::LocalSourceIdentity, measure_regular_file_bounded, sha256_bytes};
use aros_transpiler::genmf_projection::{expand_files, Limits};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

fn bounded(path: &Path, ceiling: u64) -> Vec<u8> {
    measure_regular_file_bounded(path, ceiling)
        .unwrap()
        .expect("regular snapshot exists")
        .1
}

fn reference_utf8(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| match byte {
            0xa4 => '€',
            0xa6 => 'Š',
            0xa8 => 'š',
            0xb4 => 'Ž',
            0xb8 => 'ž',
            0xbc => 'Œ',
            0xbd => 'œ',
            0xbe => 'Ÿ',
            value => char::from(*value),
        })
        .collect()
}

fn append_reference_identity(identity: &mut Vec<u8>, relative_output: &Path, text: &str) {
    identity.extend_from_slice(relative_output.to_str().unwrap().as_bytes());
    identity.push(0);
    let digest = sha256_bytes(text.as_bytes());
    for pair in digest.as_str().as_bytes().as_chunks::<2>().0 {
        identity.push(u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap());
    }
    assert!(identity.len() <= 16 * 1024 * 1024);
}

#[test]
fn reference_identity_binds_decoded_output_bytes_and_relative_output_name() {
    let mut original = Vec::new();
    append_reference_identity(&mut original, Path::new("rules.rust.txt"), "output\n");
    for (name, content) in [
        ("other.rust.txt", "output\n"),
        ("rules.rust.txt", "changed\n"),
    ] {
        let mut changed = Vec::new();
        append_reference_identity(&mut changed, Path::new(name), content);
        assert_ne!(sha256_bytes(&original), sha256_bytes(&changed));
    }
    assert_eq!(
        reference_utf8(&[0xa4, 0xa6, 0xa8, 0xb4, 0xb8, 0xbc, 0xbd, 0xbe]),
        "€ŠšŽžŒœŸ"
    );
}

#[test]
#[ignore = "requires explicit isolated source and independently generated classic GenMF reference corpus"]
fn explicit_classic_genmf_corpus_matches_current_crate_expansion() {
    let source =
        PathBuf::from(std::env::var_os("AROS_TEST_P4_SOURCE").expect("select isolated source"))
            .canonicalize()
            .unwrap();
    let references = PathBuf::from(
        std::env::var_os("AROS_TEST_GENMF_REFERENCE_ROOT").expect("select fresh reference root"),
    )
    .canonicalize()
    .unwrap();
    let expected_manifest = std::env::var("AROS_TEST_GENMF_REFERENCE_MANIFEST_SHA256")
        .expect("pin reference pair-list bytes");
    let expected_outputs = std::env::var("AROS_TEST_GENMF_REFERENCE_OUTPUTS_SHA256")
        .expect("pin independently measured decoded reference output identity");
    let before = LocalSourceIdentity::capture(&source, "esp32p4-d1001").unwrap();
    let manifest = bounded(&references.join("reference-inputs.txt"), 8 * 1024 * 1024);
    assert_eq!(sha256_bytes(&manifest).as_str(), expected_manifest);
    let manifest = std::str::from_utf8(&manifest).unwrap();
    let mut seen = BTreeSet::new();
    let mut imports = BTreeMap::new();
    let mut count = 0usize;
    let mut total_bytes = 0usize;
    let mut output_identity = Vec::new();
    for line in manifest.lines() {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        assert_eq!(fields.len(), 2, "unambiguous reference pair");
        let input = Path::new(fields[0]);
        let output = Path::new(fields[1]);
        assert!(input.starts_with(&source) && output.starts_with(&references));
        assert_eq!(
            input.canonicalize().unwrap(),
            input,
            "canonical source pair"
        );
        assert_eq!(
            output.canonicalize().unwrap(),
            output,
            "canonical reference pair"
        );
        assert_eq!(input.extension().unwrap(), "src");
        assert!(seen.insert(input.to_owned()), "duplicate reference input");
        let relative = input.strip_prefix(&source).unwrap();
        assert_eq!(
            output,
            references.join(relative.with_extension("reference.txt"))
        );
        let expanded =
            expand_files(input, &source.join("config/make.tmpl"), Limits::default()).unwrap();
        for snapshot in expanded.template_snapshots {
            assert!(snapshot.path.starts_with(&source));
            let digest = sha256_bytes(&snapshot.bytes);
            if let Some(previous) = imports.insert(snapshot.path, digest.clone()) {
                assert_eq!(previous, digest, "shared template changed between inputs");
            }
        }
        let expected = reference_utf8(&bounded(output, 16 * 1024 * 1024));
        assert_eq!(expanded.text, expected, "{}", input.display());
        append_reference_identity(
            &mut output_identity,
            &relative.with_extension("rust.txt"),
            &expected,
        );
        total_bytes = total_bytes.checked_add(expanded.text.len()).unwrap();
        assert!(total_bytes <= 128 * 1024 * 1024);
        count += 1;
        assert!(count <= 20_000);
    }
    assert!(count > 0, "empty corpus is not evidence");
    assert_eq!(sha256_bytes(&output_identity).as_str(), expected_outputs);
    before.verify(&source, "esp32p4-d1001").unwrap();
    eprintln!("reference text equality: {count} inputs, {total_bytes} UTF-8 bytes, {} stable imports; not graph admission", imports.len());
}
