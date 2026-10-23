use super::InventoryObservation;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

const INVENTORY_FILE_NAME: &str = "Makefile.sources";

fn inventory_path(ports: &Path) -> std::path::PathBuf {
    ports.join("mesa").join(INVENTORY_FILE_NAME)
}

fn canonical_root(directory: &tempfile::TempDir) -> PathBuf {
    fs::canonicalize(directory.path()).unwrap()
}

fn assert_rejected_with(result: Result<InventoryObservation, String>, reason: &str) {
    let error = result.unwrap_err();
    assert!(
        error.contains(reason),
        "expected rejection containing {reason:?}, got {error:?}"
    );
}

#[test]
fn present_and_absent_observations_verify_while_unchanged() {
    let directory = tempdir().unwrap();
    let root = canonical_root(&directory);
    let ports = root.join("ports");
    fs::create_dir_all(ports.join("mesa")).unwrap();
    let path = inventory_path(&ports);
    fs::write(&path, b"SOURCES = src/a.c\n").unwrap();

    let present = InventoryObservation::capture(&path, &ports).unwrap();
    assert!(present.present());
    present.verify().unwrap();

    fs::remove_file(&path).unwrap();
    let absent = InventoryObservation::capture(&path, &ports).unwrap();
    assert!(!absent.present());
    absent.verify().unwrap();
}

#[test]
fn verification_rejects_presence_changes_in_both_directions() {
    let directory = tempdir().unwrap();
    let root = canonical_root(&directory);
    let ports = root.join("ports");
    fs::create_dir_all(ports.join("mesa")).unwrap();
    let path = inventory_path(&ports);
    fs::write(&path, b"SOURCES = src/a.c\n").unwrap();

    let present = InventoryObservation::capture(&path, &ports).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(present
        .verify()
        .unwrap_err()
        .contains("observation changed"));

    let absent = InventoryObservation::capture(&path, &ports).unwrap();
    fs::write(&path, b"SOURCES = src/a.c\n").unwrap();
    assert!(absent.verify().unwrap_err().contains("observation changed"));
}

#[test]
fn verification_rejects_content_changes() {
    let directory = tempdir().unwrap();
    let root = canonical_root(&directory);
    let ports = root.join("ports");
    fs::create_dir_all(ports.join("mesa")).unwrap();
    let path = inventory_path(&ports);
    fs::write(&path, b"SOURCES = src/a.c\n").unwrap();

    let observation = InventoryObservation::capture(&path, &ports).unwrap();
    fs::write(&path, b"SOURCES = src/b.c\n").unwrap();
    assert!(observation
        .verify()
        .unwrap_err()
        .contains("observation changed"));
}

#[test]
fn capture_rejects_missing_parent_directory_inventory_directory_and_external_path() {
    let directory = tempdir().unwrap();
    let root = canonical_root(&directory);
    let ports = root.join("ports");
    fs::create_dir_all(&ports).unwrap();

    let missing_parent = ports.join("missing").join(INVENTORY_FILE_NAME);
    assert_rejected_with(
        InventoryObservation::capture(&missing_parent, &ports),
        "cannot resolve Mesa inventory parent",
    );

    let directory_inventory = inventory_path(&ports);
    fs::create_dir_all(&directory_inventory).unwrap();
    assert_rejected_with(
        InventoryObservation::capture(&directory_inventory, &ports),
        "not a regular file",
    );

    let outside = tempdir().unwrap();
    let external_root = canonical_root(&outside);
    let external_path = inventory_path(&external_root);
    fs::create_dir_all(external_path.parent().unwrap()).unwrap();
    fs::write(&external_path, b"SOURCES = src/a.c\n").unwrap();
    assert_rejected_with(
        InventoryObservation::capture(&external_path, &ports),
        "outside the selected ports directory",
    );
}

#[test]
fn capture_rejects_noncanonical_and_wrong_basename_paths() {
    let directory = tempdir().unwrap();
    let root = canonical_root(&directory);
    let ports = root.join("ports");
    fs::create_dir_all(ports.join("mesa")).unwrap();
    let path = inventory_path(&ports);

    assert_rejected_with(
        InventoryObservation::capture(Path::new("relative/Makefile.sources"), &ports),
        "must be absolute",
    );

    let wrong_name = ports.join("mesa").join("makefile.sources");
    assert_rejected_with(
        InventoryObservation::capture(&wrong_name, &ports),
        "must name exactly Makefile.sources",
    );

    let noncanonical = ports
        .join("mesa")
        .join("..")
        .join("mesa")
        .join(INVENTORY_FILE_NAME);
    assert_rejected_with(
        InventoryObservation::capture(&noncanonical, &ports),
        "only root and normal components",
    );

    // Keep the expected canonical path explicit: the helper does not rewrite paths.
    assert!(InventoryObservation::capture(&path, &ports).is_ok());
}

#[cfg(unix)]
#[test]
fn capture_rejects_a_symlink_inventory_file() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().unwrap();
    let root = canonical_root(&directory);
    let ports = root.join("ports");
    fs::create_dir_all(ports.join("mesa")).unwrap();
    let target = ports.join("mesa").join("actual.sources");
    fs::write(&target, b"SOURCES = src/a.c\n").unwrap();
    let path = inventory_path(&ports);
    symlink(&target, &path).unwrap();

    assert_rejected_with(
        InventoryObservation::capture(&path, &ports),
        "must not be a symlink",
    );
}

#[cfg(unix)]
#[test]
fn verification_rejects_retargeted_selected_ports_symlink() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().unwrap();
    let root = canonical_root(&directory);
    let ports_a = root.join("ports-a");
    let ports_b = root.join("ports-b");
    fs::create_dir_all(&ports_a).unwrap();
    fs::create_dir_all(&ports_b).unwrap();
    let path = inventory_path(&ports_a);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"SOURCES = src/a.c\n").unwrap();

    let selected_ports = root.join("selected-ports");
    symlink(&ports_a, &selected_ports).unwrap();
    let observation = InventoryObservation::capture(&path, &selected_ports).unwrap();

    fs::remove_file(&selected_ports).unwrap();
    symlink(&ports_b, &selected_ports).unwrap();
    assert!(observation
        .verify()
        .unwrap_err()
        .contains("outside the selected ports directory"));
}
