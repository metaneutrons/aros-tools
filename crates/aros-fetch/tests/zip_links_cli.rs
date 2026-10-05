#![cfg(unix)]

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

use aros_common::sha256_file;
use zip::write::SimpleFileOptions;

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new(entries: &[(&str, Option<&str>)]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let mut writer = zip::ZipWriter::new(File::create(cache.join("fixture.zip")).unwrap());
        for (path, link) in entries {
            if let Some(target) = link {
                writer
                    .add_symlink(path, target, SimpleFileOptions::default())
                    .unwrap();
            } else {
                writer
                    .start_file(path, SimpleFileOptions::default().unix_permissions(0o755))
                    .unwrap();
                writer.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
            }
        }
        writer.finish().unwrap();
        let destination = root.path().join("destination");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("sentinel"), b"unchanged").unwrap();
        Self { root }
    }

    fn run(&self) -> Output {
        self.run_with_base(false)
    }

    fn run_with_base(&self, destination_base: bool) -> Output {
        let cache = self.root.path().join("cache");
        let digest = sha256_file(&cache.join("fixture.zip")).unwrap().digest;
        Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
            .args([
                "--archive",
                "fixture",
                "--suffixes",
                "zip",
                "--offline",
                "--require-checksums",
                "--checksums",
                &format!("fixture.zip=sha256:{digest}"),
                "--location",
            ])
            .arg(cache)
            .arg("--destination")
            .arg(self.root.path().join("destination"))
            .arg("--base")
            .arg(self.root.path().join(if destination_base {
                "destination"
            } else {
                "base"
            }))
            .output()
            .unwrap()
    }

    fn assert_rejected(&self) {
        let result = self.run();
        assert!(
            !result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("AF0501"),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let destination = self.root.path().join("destination");
        assert_eq!(
            fs::read(destination.join("sentinel")).unwrap(),
            b"unchanged"
        );
        assert_eq!(
            fs::read_dir(destination).unwrap().count(),
            1,
            "failed extraction published a partial tree"
        );
        assert!(!self.root.path().join("escaped").exists());
    }
}

#[test]
fn zip_marker_symlink_cannot_overwrite_an_extracted_file() {
    let fixture = Fixture::new(&[
        (".fixture.zip.unpacked", Some("source/run.sh")),
        ("source/run.sh", None),
    ]);
    let output = fixture.run_with_base(true);
    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostic.contains("marker") && diagnostic.contains("link"),
        "{diagnostic}"
    );
    let destination = fixture.root.path().join("destination");
    assert_eq!(
        fs::read(destination.join("sentinel")).unwrap(),
        b"unchanged"
    );
    let entries: Vec<_> = fs::read_dir(destination)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        entries
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        [".aros-fetch-patch-cache.lock", "sentinel"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect()
    );
    assert!(!fixture.root.path().join("destination/source").exists());
}

#[test]
fn zip_relative_links_and_forward_chains_publish_and_reuse_verified_tree() {
    let fixture = Fixture::new(&[
        ("source/sub/alias", Some("../chain")),
        ("source/chain", Some("run.sh")),
        ("source/run.sh", None),
    ]);
    for _ in 0..2 {
        let result = fixture.run();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let root = fixture.root.path().join("destination/source");
        assert_eq!(
            fs::read_link(root.join("sub/alias")).unwrap().to_str(),
            Some("../chain")
        );
        assert_eq!(
            fs::read(root.join("sub/alias")).unwrap(),
            fs::read(root.join("run.sh")).unwrap()
        );
        assert_eq!(
            fs::metadata(root.join("run.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
}

#[test]
fn zip_absolute_parent_escape_dangling_and_cyclic_links_fail_without_publication() {
    for entries in [
        vec![("source/alias", Some("/etc/passwd"))],
        vec![("source/alias", Some("../../escaped"))],
        vec![("source/alias", Some("missing"))],
        vec![("source/a", Some("b")), ("source/b", Some("a"))],
        vec![("source/a", Some("a"))],
        vec![("source/a", Some(""))],
        vec![("source/a", Some("bad\0target"))],
        vec![("x/alias", Some("..")), ("x/escape", Some("alias/../.."))],
    ] {
        Fixture::new(&entries).assert_rejected();
    }
}

#[test]
fn zip_entries_below_links_and_link_ancestors_fail_in_both_orders() {
    for entries in [
        vec![
            ("source/alias", Some("target")),
            ("source/alias/child", None),
            ("source/target/file", None),
        ],
        vec![
            ("source/alias/child", None),
            ("source/alias", Some("target")),
            ("source/target/file", None),
        ],
        vec![
            ("source/alias", Some("target")),
            ("source/alias/child", Some("../target")),
            ("source/target", None),
        ],
        vec![
            ("source/alias/child", Some("../target")),
            ("source/alias", Some("target")),
            ("source/target", None),
        ],
    ] {
        Fixture::new(&entries).assert_rejected();
    }
}

#[test]
fn zip_oversized_link_target_fails_without_publication() {
    let target = "a".repeat(4097);
    Fixture::new(&[("source/alias", Some(&target))]).assert_rejected();
}

#[test]
fn zip_normalized_duplicate_paths_are_rejected() {
    Fixture::new(&[("source/value", None), ("source/./value", None)]).assert_rejected();
}

#[test]
fn zip_mode_name_mismatch_and_special_entries_are_rejected() {
    for mode in [0o040_755_u32, 0o010_644] {
        let fixture = Fixture::new(&[("source/value", None)]);
        let path = fixture.root.path().join("cache/fixture.zip");
        let mut bytes = fs::read(&path).unwrap();
        let central = bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .unwrap();
        bytes[central + 38..central + 42].copy_from_slice(&(mode << 16).to_le_bytes());
        fs::write(path, bytes).unwrap();
        fixture.assert_rejected();
    }
    Fixture::new(&[("source/directory/", None)]).assert_rejected();
}
