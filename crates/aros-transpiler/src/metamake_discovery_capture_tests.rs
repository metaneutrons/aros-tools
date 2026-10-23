use super::CapturedProjectDiscovery;
use crate::metamake_project::ConfiguredProject;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn project(extra: &str) -> ConfiguredProject {
    ConfiguredProject::parse(
        &format!("[probe]\ndefaultmakefilename mmakefile\n{extra}"),
        "probe",
    )
    .unwrap()
}

fn directory() -> TempDir {
    tempfile::tempdir().unwrap()
}

#[cfg(unix)]
fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
#[cfg(unix)]
fn directory_aliases_preserve_logical_names_and_physical_referents_for_both_inputs() {
    let temp = directory();
    let root = temp.path();
    fs::create_dir(root.join("real")).unwrap();
    fs::write(root.join("real/mmakefile.src"), b"source").unwrap();
    fs::write(root.join("real/mmakefile"), b"sibling").unwrap();
    symlink("real", root.join("alias"));

    let capture = CapturedProjectDiscovery::capture(root, &project(""), &BTreeSet::new()).unwrap();
    assert_eq!(
        capture.inputs(),
        &[
            ("alias/mmakefile".into(), "real/mmakefile".into()),
            ("alias/mmakefile.src".into(), "real/mmakefile.src".into()),
            ("real/mmakefile".into(), "real/mmakefile".into()),
            ("real/mmakefile.src".into(), "real/mmakefile.src".into()),
        ]
        .into_iter()
        .collect()
    );
}

#[test]
#[cfg(unix)]
fn ordinary_docs_directory_alias_without_make_owners_is_harmless() {
    let temp = directory();
    let root = temp.path();
    fs::create_dir(root.join("docs")).unwrap();
    fs::write(root.join("docs/guide.txt"), b"documentation").unwrap();
    symlink("docs", root.join("docs-alias"));

    let capture = CapturedProjectDiscovery::capture(root, &project(""), &BTreeSet::new()).unwrap();
    assert!(capture.inputs().is_empty());
}

#[test]
#[cfg(unix)]
fn selected_file_alias_resolves_to_its_physical_source() {
    let temp = directory();
    let root = temp.path();
    fs::write(root.join("physical.src"), b"source").unwrap();
    symlink("physical.src", root.join("mmakefile.src"));

    let capture = CapturedProjectDiscovery::capture(root, &project(""), &BTreeSet::new()).unwrap();
    assert_eq!(capture.inputs()["mmakefile.src"], "physical.src");
}

#[test]
#[cfg(unix)]
fn recapture_observes_a_changed_link_target() {
    let temp = directory();
    let root = temp.path();
    fs::create_dir(root.join("first")).unwrap();
    fs::create_dir(root.join("second")).unwrap();
    fs::write(root.join("first/mmakefile.src"), b"one").unwrap();
    fs::write(root.join("second/mmakefile.src"), b"two").unwrap();
    symlink("first/mmakefile.src", root.join("mmakefile.src"));
    let first = CapturedProjectDiscovery::capture(root, &project(""), &BTreeSet::new())
        .unwrap()
        .into_inputs();

    fs::remove_file(root.join("mmakefile.src")).unwrap();
    symlink("second/mmakefile.src", root.join("mmakefile.src"));
    let second = CapturedProjectDiscovery::capture(root, &project(""), &BTreeSet::new())
        .unwrap()
        .into_inputs();

    assert_ne!(first, second);
    assert_eq!(first["mmakefile.src"], "first/mmakefile.src");
    assert_eq!(second["mmakefile.src"], "second/mmakefile.src");
}

#[test]
#[cfg(unix)]
fn link_loops_and_external_targets_are_refused() {
    let loop_root = directory();
    symlink("second", loop_root.path().join("first"));
    symlink("first", loop_root.path().join("second"));
    let error = CapturedProjectDiscovery::capture(loop_root.path(), &project(""), &BTreeSet::new())
        .unwrap_err();
    assert!(error.contains("link cycle/hop limit"), "{error}");

    let external_root = directory();
    symlink(
        "/outside/physical.src",
        external_root.path().join("mmakefile.src"),
    );
    let error =
        CapturedProjectDiscovery::capture(external_root.path(), &project(""), &BTreeSet::new())
            .unwrap_err();
    assert!(
        error.contains("unsupported MetaMake link target"),
        "{error}"
    );
}

#[test]
#[cfg(unix)]
fn links_into_an_explicitly_excluded_namespace_are_unmeasured_and_refused() {
    let temp = directory();
    let root = temp.path();
    fs::create_dir(root.join("generated")).unwrap();
    fs::write(root.join("generated/mmakefile.src"), b"source").unwrap();
    symlink("generated/mmakefile.src", root.join("mmakefile.src"));
    let excluded = std::iter::once("generated".to_owned()).collect();

    let error = CapturedProjectDiscovery::capture(root, &project(""), &excluded).unwrap_err();
    assert!(error.contains("link target is unmeasured"), "{error}");
}

#[test]
#[cfg(unix)]
fn ignored_nonselected_link_to_an_external_namespace_is_skipped_by_the_resolver() {
    let temp = directory();
    symlink("/outside/docs", temp.path().join("docs"));

    let capture = CapturedProjectDiscovery::capture(
        temp.path(),
        &project("ignoredir docs\n"),
        &BTreeSet::new(),
    )
    .unwrap();
    assert!(capture.inputs().is_empty());
}

#[test]
#[cfg(unix)]
fn captured_slash_ignoredir_keeps_siblings_and_uses_logical_alias_path() {
    let temp = directory();
    let root = temp.path();
    for directory in [
        "build",
        "build/arm-raspi-copy",
        "build/other",
        "vendor/arm-raspi",
        "physical/arm-raspi",
    ] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    for path in [
        "build/arm-raspi-copy/mmakefile.src",
        "build/other/mmakefile.src",
        "vendor/arm-raspi/mmakefile.src",
        "physical/arm-raspi/mmakefile.src",
    ] {
        fs::write(root.join(path), b"source").unwrap();
    }
    symlink("/outside/ignored", root.join("build/arm-raspi"));
    symlink("physical", root.join("alias"));

    let capture = CapturedProjectDiscovery::capture(
        root,
        &project("ignoredir build/arm-raspi\n"),
        &BTreeSet::new(),
    )
    .unwrap();
    assert!(!capture.inputs().contains_key("build/arm-raspi"));
    for logical in [
        "build/arm-raspi-copy/mmakefile.src",
        "build/other/mmakefile.src",
        "vendor/arm-raspi/mmakefile.src",
        "alias/arm-raspi/mmakefile.src",
        "physical/arm-raspi/mmakefile.src",
    ] {
        assert!(capture.inputs().contains_key(logical), "{logical}");
    }
}

#[test]
#[cfg(unix)]
fn alias_to_ancestor_of_excluded_namespace_cannot_hide_omitted_children() {
    let temp = directory();
    fs::create_dir_all(temp.path().join("build/generated")).unwrap();
    fs::write(temp.path().join("build/generated/mmakefile.src"), b"hidden").unwrap();
    fs::write(temp.path().join("build/mmakefile.src"), b"visible").unwrap();
    let excluded = std::iter::once("build/generated".to_owned()).collect();
    let exact_ignored = project("ignoredir build/generated\n");
    let direct = CapturedProjectDiscovery::capture(temp.path(), &exact_ignored, &excluded).unwrap();
    assert!(direct.inputs().contains_key("build/mmakefile.src"));
    assert!(!direct
        .inputs()
        .contains_key("build/generated/mmakefile.src"));
    symlink("build", temp.path().join("alias"));
    let error =
        CapturedProjectDiscovery::capture(temp.path(), &exact_ignored, &excluded).unwrap_err();
    assert!(error.contains("unmeasured namespace"), "{error}");
}

#[test]
#[cfg(unix)]
fn lexical_root_equivalence_produces_the_same_capture() {
    let temp = directory();
    let root = temp.path();
    fs::create_dir(root.join("child")).unwrap();
    fs::write(root.join("child/mmakefile.src"), b"source").unwrap();
    let lexical = root.join("child/..");

    let canonical_capture =
        CapturedProjectDiscovery::capture(root, &project(""), &BTreeSet::new()).unwrap();
    let lexical_capture =
        CapturedProjectDiscovery::capture(&lexical, &project(""), &BTreeSet::new()).unwrap();
    assert_eq!(canonical_capture.inputs(), lexical_capture.inputs());
}

#[test]
fn excluded_names_must_be_safe_root_relative_namespaces() {
    let temp = directory();
    let excluded = std::iter::once("../outside".to_owned()).collect();

    let error =
        CapturedProjectDiscovery::capture(temp.path(), &project(""), &excluded).unwrap_err();
    assert!(
        error.contains("invalid MetaMake source inventory path"),
        "{error}"
    );
}

#[test]
#[cfg(unix)]
fn non_utf8_inventory_names_and_special_files_are_refused() {
    use std::os::unix::ffi::OsStringExt;

    let invalid_name = std::ffi::OsString::from_vec(vec![b'x', 0xff]);
    // APFS rejects this filename before a walk can observe it. Exercise the
    // exact production validator without requiring unsupported filesystem data.
    let error = super::relative_name(Path::new(&invalid_name)).unwrap_err();
    assert!(
        error.contains("non-UTF-8 MetaMake discovery path"),
        "{error}"
    );

    let socket_root = directory();
    let _socket =
        std::os::unix::net::UnixListener::bind(socket_root.path().join("socket")).unwrap();
    let error =
        CapturedProjectDiscovery::capture(socket_root.path(), &project(""), &BTreeSet::new())
            .unwrap_err();
    assert!(error.contains("unsupported special file"), "{error}");
}
