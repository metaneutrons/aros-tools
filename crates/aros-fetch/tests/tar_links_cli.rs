#![cfg(unix)]

use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Command;

use aros_common::sha256_file;
use flate2::{write::GzEncoder, Compression};

fn probe(entries: &[(&str, tar::EntryType, Option<&str>)], success: bool) {
    probe_with_metadata(entries, success, 0o644, 1);
}

fn probe_with_metadata(
    entries: &[(&str, tar::EntryType, Option<&str>)],
    success: bool,
    mode: u32,
    mtime: u64,
) {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("cache");
    fs::create_dir(&cache).unwrap();
    let path = cache.join("fixture.tar.gz");
    let mut writer = tar::Builder::new(GzEncoder::new(
        File::create(&path).unwrap(),
        Compression::default(),
    ));
    for (path, kind, target) in entries {
        let payload: &[u8] = if kind.is_file() {
            b"regular payload\n"
        } else {
            b""
        };
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(*kind);
        header.set_size(payload.len() as u64);
        header.set_mode(mode);
        header.set_mtime(mtime);
        if let Some(target) = target {
            header.set_link_name(target).unwrap();
        }
        header.set_cksum();
        writer.append_data(&mut header, path, payload).unwrap();
    }
    writer.into_inner().unwrap().finish().unwrap();
    let digest = sha256_file(&path).unwrap().digest;
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("sentinel"), b"unchanged").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_aros-fetch"))
        .args([
            "--archive",
            "fixture",
            "--suffixes",
            "tar.gz",
            "--offline",
            "--require-checksums",
            "--checksums",
            &format!("fixture.tar.gz=sha256:{digest}"),
            "--location",
        ])
        .arg(cache)
        .arg("--destination")
        .arg(&destination)
        .arg("--base")
        .arg(root.path().join("base"))
        .output()
        .unwrap();
    assert_eq!(
        result.status.success(),
        success,
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read(destination.join("sentinel")).unwrap(),
        b"unchanged"
    );
    if success {
        let original = destination.join("source/value");
        let alias = destination.join("source/alias");
        let original_metadata = fs::metadata(&original).unwrap();
        let alias_metadata = fs::metadata(&alias).unwrap();
        if mode & 0o400 != 0 {
            assert_eq!(fs::read(&alias).unwrap(), fs::read(&original).unwrap());
        }
        assert_ne!(alias_metadata.ino(), original_metadata.ino());
        assert_eq!(
            alias_metadata.permissions().mode() & 0o777,
            original_metadata.permissions().mode() & 0o777
        );
        assert_eq!(alias_metadata.mtime(), original_metadata.mtime());
        assert_eq!(alias_metadata.mtime(), i64::try_from(mtime).unwrap());
    } else {
        assert!(String::from_utf8_lossy(&result.stderr).contains("AF0501"));
        assert!(!destination.join("source").exists());
    }
}

#[test]
fn tar_contained_hardlinks_become_independent_copies_in_either_order() {
    for entries in [
        vec![
            ("source/value", tar::EntryType::Regular, None),
            ("source/alias", tar::EntryType::Link, Some("source/value")),
        ],
        vec![
            ("source/alias", tar::EntryType::Link, Some("source/value")),
            ("source/value", tar::EntryType::Regular, None),
        ],
    ] {
        probe(&entries, true);
    }
}

#[test]
fn tar_hardlink_copy_preserves_permissions_and_target_mtime() {
    probe_with_metadata(
        &[
            ("source/value", tar::EntryType::Regular, None),
            ("source/alias", tar::EntryType::Link, Some("source/value")),
        ],
        true,
        0o644,
        1_600_000_000,
    );
}

#[test]
fn tar_hardlinks_reject_escape_missing_chains_and_symlink_targets() {
    for entries in [
        vec![("source/alias", tar::EntryType::Link, Some("../escape"))],
        vec![("source/alias", tar::EntryType::Link, Some("/etc/passwd"))],
        vec![("source/alias", tar::EntryType::Link, Some("missing"))],
        vec![("source/alias", tar::EntryType::Link, Some("source/alias"))],
        vec![
            ("source/value", tar::EntryType::Regular, None),
            ("source/second", tar::EntryType::Link, Some("source/value")),
            ("source/alias", tar::EntryType::Link, Some("source/second")),
        ],
        vec![
            ("source/value", tar::EntryType::Regular, None),
            ("source/link", tar::EntryType::Symlink, Some("value")),
            ("source/alias", tar::EntryType::Link, Some("source/link")),
        ],
        vec![
            ("source/real/value", tar::EntryType::Regular, None),
            ("source/dir", tar::EntryType::Symlink, Some("real")),
            (
                "source/alias",
                tar::EntryType::Link,
                Some("source/dir/value"),
            ),
        ],
    ] {
        probe(&entries, false);
    }
}

#[test]
fn tar_hardlink_output_may_not_have_a_symlink_parent_or_child_entry() {
    for entries in [
        vec![
            ("source/real/value", tar::EntryType::Regular, None),
            ("source/dir", tar::EntryType::Symlink, Some("real")),
            (
                "source/dir/alias",
                tar::EntryType::Link,
                Some("source/real/value"),
            ),
        ],
        vec![
            ("source/value", tar::EntryType::Regular, None),
            ("source/alias", tar::EntryType::Link, Some("source/value")),
            ("source/alias/child", tar::EntryType::Regular, None),
        ],
    ] {
        probe(&entries, false);
    }
}
