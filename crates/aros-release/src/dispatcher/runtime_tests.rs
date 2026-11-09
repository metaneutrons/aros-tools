//! Positive probe and counter-probes for the frozen runtime closure.

use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::claim::sha256_hex;
use super::runtime::{
    validate_lock, validate_manifest, verify_extracted_frozen, MANIFEST_NAME, RECEIPT_NAME,
};
use super::testing::write_runtime;

type Probe = (&'static str, Box<dyn Fn(&Closure)>);
type Mutation = (&'static str, Box<dyn Fn(&mut Value)>);

struct Closure {
    _dir: tempfile::TempDir,
    root: PathBuf,
    lock: Value,
    lock_sha: String,
    manifest: Vec<u8>,
}

impl Closure {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let fixture = write_runtime(&base);
        Self {
            root: base.join("runtime"),
            lock: serde_json::from_slice(&fixture.lock).unwrap(),
            lock_sha: sha256_hex(&fixture.lock),
            manifest: fixture.manifest,
            _dir: dir,
        }
    }

    fn verify(&self) -> super::Result<Value> {
        verify_extracted_frozen(&self.lock, &self.lock_sha, "linux-x86_64", &self.root)
    }
}

fn rewrite(path: &Path, mutate: impl Fn(&mut Value)) {
    let mut value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    mutate(&mut value);
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

#[test]
fn accepts_the_exact_locked_closure() {
    let closure = Closure::new();
    let manifest = closure.verify().expect("exact closure must verify");
    assert_eq!(manifest["files"].as_array().unwrap().len(), 10);
    assert_eq!(
        sha256_hex(&closure.manifest),
        closure.lock["hosts"]["linux-x86_64"]["manifest"]["sha256"]
    );
}

#[test]
fn rejects_every_deviation_of_the_held_closure() {
    let probes: Vec<Probe> = vec![
        (
            "extra root file",
            Box::new(|c| std::fs::write(c.root.join("extra"), b"x").unwrap()),
        ),
        (
            "extra bin file",
            Box::new(|c| std::fs::write(c.root.join("bin/extra"), b"x").unwrap()),
        ),
        (
            "missing file",
            Box::new(|c| std::fs::remove_file(c.root.join("LICENSE")).unwrap()),
        ),
        (
            "missing executable",
            Box::new(|c| std::fs::remove_file(c.root.join("bin/aros")).unwrap()),
        ),
        (
            "changed bytes",
            Box::new(|c| std::fs::write(c.root.join("README.md"), b"changed").unwrap()),
        ),
        (
            "same size different bytes",
            Box::new(|c| {
                let path = c.root.join("bin/aros-fetch");
                let mut bytes = std::fs::read(&path).unwrap();
                bytes[0] ^= 1;
                std::fs::write(path, bytes).unwrap();
            }),
        ),
        (
            "executable mode lost",
            Box::new(|c| {
                std::fs::set_permissions(
                    c.root.join("bin/aros"),
                    std::fs::Permissions::from_mode(0o644),
                )
                .unwrap();
            }),
        ),
        (
            "writable data file",
            Box::new(|c| {
                std::fs::set_permissions(
                    c.root.join("LICENSE"),
                    std::fs::Permissions::from_mode(0o664),
                )
                .unwrap();
            }),
        ),
        (
            "symlinked file",
            Box::new(|c| {
                let path = c.root.join("LICENSE");
                let target = c.root.join("README.md");
                std::fs::remove_file(&path).unwrap();
                symlink(target, path).unwrap();
            }),
        ),
        (
            "hard-linked file",
            Box::new(|c| {
                std::fs::hard_link(c.root.join("LICENSE"), c.root.join("bin/elsewhere")).unwrap();
            }),
        ),
        (
            "symlinked bin directory",
            Box::new(|c| {
                let other = c.root.join("other");
                std::fs::rename(c.root.join("bin"), &other).unwrap();
                symlink(&other, c.root.join("bin")).unwrap();
            }),
        ),
        (
            "receipt of another host",
            Box::new(|c| {
                rewrite(&c.root.join(RECEIPT_NAME), |v| {
                    v["host"] = "linux-aarch64".into();
                });
            }),
        ),
        (
            "receipt of another lock",
            Box::new(|c| {
                rewrite(&c.root.join(RECEIPT_NAME), |v| {
                    v["runtime_lock_sha256"] = "0".repeat(64).into();
                });
            }),
        ),
        (
            "receipt with an extra field",
            Box::new(|c| rewrite(&c.root.join(RECEIPT_NAME), |v| v["extra"] = 1.into())),
        ),
        (
            "tampered manifest",
            Box::new(|c| {
                rewrite(&c.root.join(MANIFEST_NAME), |v| {
                    v["files"][0]["size"] = 99.into();
                });
            }),
        ),
        (
            "manifest swapped for a self-consistent forgery",
            Box::new(|c| {
                let mut forged: Value = serde_json::from_slice(&c.manifest).unwrap();
                forged["files"][2]["sha256"] = sha256_hex(b"forged").into();
                std::fs::write(
                    c.root.join(MANIFEST_NAME),
                    serde_json::to_vec(&forged).unwrap(),
                )
                .unwrap();
            }),
        ),
    ];
    for (label, probe) in probes {
        let closure = Closure::new();
        probe(&closure);
        assert!(closure.verify().is_err(), "{label} must be rejected");
    }
}

#[test]
fn rejects_wrong_selectors() {
    let closure = Closure::new();
    assert!(
        verify_extracted_frozen(
            &closure.lock,
            &closure.lock_sha,
            "linux-aarch64",
            &closure.root
        )
        .is_err(),
        "a host with another manifest"
    );
    assert!(
        verify_extracted_frozen(
            &closure.lock,
            &closure.lock_sha,
            "windows-x86_64",
            &closure.root
        )
        .is_err(),
        "an unsupported host"
    );
    assert!(
        verify_extracted_frozen(
            &closure.lock,
            &"0".repeat(64),
            "linux-x86_64",
            &closure.root
        )
        .is_err(),
        "a lock digest that differs from the receipt"
    );
    assert!(
        verify_extracted_frozen(&closure.lock, "short", "linux-x86_64", &closure.root).is_err(),
        "a malformed lock digest"
    );
    let missing = closure.root.join("absent");
    assert!(
        verify_extracted_frozen(&closure.lock, &closure.lock_sha, "linux-x86_64", &missing)
            .is_err()
    );
    let linked = closure.root.with_file_name("linked-runtime");
    symlink(&closure.root, &linked).unwrap();
    assert!(
        verify_extracted_frozen(&closure.lock, &closure.lock_sha, "linux-x86_64", &linked).is_err(),
        "a runtime root that is a symlink"
    );
}

#[test]
fn lock_contract_rejects_deviations() {
    let closure = Closure::new();
    let mutations: Vec<Mutation> = vec![
        ("schema", Box::new(|v| v["schema"] = "other".into())),
        (
            "repository",
            Box::new(|v| v["repository"] = "noslash".into()),
        ),
        ("release id", Box::new(|v| v["release_id"] = 0.into())),
        ("tag", Box::new(|v| v["tag"] = "v0.0.1".into())),
        ("tag object", Box::new(|v| v["tag_object"] = "XYZ".into())),
        ("epoch", Box::new(|v| v["source_date_epoch"] = (-1).into())),
        (
            "signer",
            Box::new(|v| v["signer_workflow"] = "x/y/.github/workflows/release.yml".into()),
        ),
        (
            "identity",
            Box::new(|v| v["certificate_identity"] = "x".into()),
        ),
        (
            "issuer",
            Box::new(|v| v["certificate_oidc_issuer"] = "https://example.org".into()),
        ),
        ("extra field", Box::new(|v| v["extra"] = 1.into())),
        (
            "host set",
            Box::new(|v| {
                v["hosts"].as_object_mut().unwrap().remove("macos-aarch64");
            }),
        ),
        (
            "target",
            Box::new(|v| v["hosts"]["linux-x86_64"]["target"] = "x".into()),
        ),
        (
            "archive name",
            Box::new(|v| v["hosts"]["linux-x86_64"]["archive"]["name"] = "x.tar.gz".into()),
        ),
        (
            "reused asset name",
            Box::new(|v| {
                let name = v["hosts"]["linux-x86_64"]["archive"]["name"].clone();
                v["hosts"]["linux-aarch64"]["archive"]["name"] = name;
            }),
        ),
        (
            "asset size",
            Box::new(|v| v["hosts"]["linux-x86_64"]["manifest"]["size"] = 0.into()),
        ),
        (
            "asset digest",
            Box::new(|v| v["hosts"]["linux-x86_64"]["manifest"]["sha256"] = "AA".into()),
        ),
    ];
    assert!(validate_lock(&closure.lock).is_ok());
    for (label, mutate) in mutations {
        let mut lock = closure.lock.clone();
        mutate(&mut lock);
        assert!(validate_lock(&lock).is_err(), "{label} must be rejected");
    }
}

#[test]
fn manifest_contract_rejects_deviations() {
    let closure = Closure::new();
    assert!(validate_manifest(&closure.lock, "linux-x86_64", &closure.manifest).is_ok());
    // Bytes that differ from the locked manifest digest or size.
    let mut padded = closure.manifest.clone();
    padded.push(b' ');
    assert!(validate_manifest(&closure.lock, "linux-x86_64", &padded).is_err());
    assert!(validate_manifest(&closure.lock, "linux-x86_64", b"{}").is_err());
    assert!(validate_manifest(&closure.lock, "nowhere", &closure.manifest).is_err());
}
