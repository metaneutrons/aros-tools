use crate::{
    captured_programs::{CapturedProgramSet, ProgramRootSelection, ProgramSelection},
    measure_tree_content_cas, sha256_bytes, Sha256Digest,
};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
};

const DATA: &[u8] = b"opaque regular-file program data; never executed";

#[test]
fn portable_selection_is_strict_bounded_data_not_a_filesystem_capture() {
    use crate::captured_programs::ProgramSetSelection;
    let selected = ProgramSetSelection {
        roots: vec![ProgramRootSelection {
            root: PathBuf::from("/not-an-existing-selected-root"),
            content_sha256: sha256_bytes(DATA),
        }],
        programs: vec![program(
            "explicit-role",
            0,
            PathBuf::from("/not-an-existing-selected-root/invocation"),
            DATA,
        )],
    };
    selected.validate().unwrap();
    let bytes = serde_json::to_vec(&selected).unwrap();
    let decoded: ProgramSetSelection = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, selected);
    assert!(CapturedProgramSet::capture(&decoded.roots, &decoded.programs).is_err());
    for value in ["explicit-role", "not-an-existing-selected-root"] {
        assert!(!format!("{decoded:?}").contains(value));
    }
    let mut malformed = serde_json::to_value(&selected).unwrap();
    malformed["programs"][0]["unknown"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ProgramSetSelection>(malformed).is_err());
    let mut duplicate = selected.clone();
    duplicate.programs.push(duplicate.programs[0].clone());
    assert!(duplicate.validate().is_err());
    let mut invalid_root_index = selected;
    invalid_root_index.programs[0].root_index = 1;
    assert!(invalid_root_index.validate().is_err());
}

fn executable(path: &Path, bytes: &[u8], mode: u32) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn root_selection(root: &Path) -> ProgramRootSelection {
    let root = root.canonicalize().unwrap();
    let tree = measure_tree_content_cas(&root).unwrap();
    ProgramRootSelection {
        root,
        content_sha256: tree.payload_digest_excluding(None),
    }
}

fn canonical_root(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

fn program(
    role: &str,
    root_index: usize,
    invocation_path: PathBuf,
    bytes: &[u8],
) -> ProgramSelection {
    ProgramSelection {
        role: role.to_owned(),
        root_index,
        invocation_path,
        executable_sha256: sha256_bytes(bytes),
    }
}

#[test]
fn capture_transaction_rejects_program_drift_before_returning_callback_data() {
    let temporary = tempfile::tempdir().unwrap();
    let root = canonical_root(temporary.path());
    let invocation = root.join("bin/opaque-program");
    executable(&invocation, DATA, 0o755);
    let roots = [root_selection(&root)];
    let programs = [program("explicit-role", 0, invocation.clone(), DATA)];
    let (_, value) = CapturedProgramSet::capture_with(&roots, &programs, |_| Ok(42)).unwrap();
    assert_eq!(value, 42);
    assert!(CapturedProgramSet::capture_with(&roots, &programs, |_| {
        fs::write(&invocation, b"different program bytes").unwrap();
        Ok(42)
    })
    .is_err());
}

#[test]
fn captures_exact_invocation_and_root_selection() {
    let temporary = tempfile::tempdir().unwrap();
    let root = canonical_root(temporary.path());
    let invocation = root.join("bin/data-tool");
    executable(&invocation, DATA, 0o755);
    let selected_root = root_selection(&root);
    let selected_program = program("builder", 0, invocation.clone(), DATA);
    let set =
        CapturedProgramSet::capture(std::slice::from_ref(&selected_root), &[selected_program])
            .unwrap();

    let identity = set.identity_for_role("builder").unwrap();
    assert_eq!(identity.role(), "builder");
    assert_eq!(identity.root_index(), 0);
    assert_eq!(identity.invocation_path(), invocation.as_path());
    let resolved = invocation.canonicalize().unwrap();
    assert_eq!(identity.resolved_path(), resolved.as_path());
    assert_eq!(identity.executable_sha256(), &sha256_bytes(DATA));
    assert!(std::ptr::eq(
        identity,
        set.identity_for_word(invocation.to_str().unwrap()).unwrap()
    ));
    assert!(set.identity_for_word("data-tool").is_none());
    assert!(set
        .identity_for_word(&format!("{}/./bin/data-tool", root.display()))
        .is_none());
    assert!(set.identity_for_role("unknown").is_none());
    assert!(set.matches_root_selection(&selected_root.root, &selected_root.content_sha256));
    assert!(!set.matches_root_selection(&selected_root.root, &sha256_bytes(b"other root")));
    assert!(set.verify().is_ok());

    let debug = format!("{set:?} {identity:?} {selected_root:?}");
    assert!(!debug.contains(root.to_str().unwrap()));
    assert!(!debug.contains("builder"));
    assert!(!debug.contains(&sha256_bytes(DATA).to_string()));
    assert!(!debug.contains("opaque regular-file program data"));
}

#[test]
fn preserves_two_roles_for_the_same_bytes_through_distinct_aliases() {
    let temporary = tempfile::tempdir().unwrap();
    let root = canonical_root(temporary.path());
    let primary = root.join("bin/program");
    let alias = root.join("bin/program-alias");
    executable(&primary, DATA, 0o755);
    symlink("program", &alias).unwrap();
    let selected_root = root_selection(&root);
    let set = CapturedProgramSet::capture(
        &[selected_root],
        &[
            program("primary", 0, primary.clone(), DATA),
            program("alternate", 0, alias.clone(), DATA),
        ],
    )
    .unwrap();

    assert_eq!(
        set.identity_for_role("primary").unwrap().resolved_path(),
        set.identity_for_role("alternate").unwrap().resolved_path()
    );
    assert_eq!(
        set.identity_for_word(alias.to_str().unwrap())
            .unwrap()
            .invocation_path(),
        alias.as_path()
    );
    assert!(set.verify().is_ok());
    assert!(CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[
            program("primary", 0, primary, DATA),
            program("alternate", 0, alias, b"wrong alias bytes")
        ],
    )
    .is_err());
}

#[test]
fn refuses_external_alias_hop_even_when_final_target_reenters_root() {
    let selected = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let root = canonical_root(selected.path());
    let outside = canonical_root(external.path());
    let executable_path = root.join("program");
    executable(&executable_path, DATA, 0o755);
    symlink(&executable_path, outside.join("return-link")).unwrap();
    let alias = root.join("alias");
    symlink(outside.join("return-link"), &alias).unwrap();
    assert_eq!(alias.canonicalize().unwrap(), executable_path);
    assert!(CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[program("selected", 0, alias, DATA)]
    )
    .is_err());
}

#[test]
fn resolves_captured_directory_and_relative_parent_aliases_without_leaving_root() {
    let temporary = tempfile::tempdir().unwrap();
    let root = canonical_root(temporary.path());
    let primary = root.join("lib/program");
    executable(&primary, DATA, 0o755);
    fs::create_dir(root.join("bin")).unwrap();
    symlink("../lib/program", root.join("bin/relative")).unwrap();
    symlink("lib", root.join("directory-alias")).unwrap();
    let set = CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[
            program("relative", 0, root.join("bin/relative"), DATA),
            program("directory", 0, root.join("directory-alias/program"), DATA),
        ],
    )
    .unwrap();
    assert_eq!(
        set.identity_for_role("relative").unwrap().resolved_path(),
        primary
    );
    assert_eq!(
        set.identity_for_role("directory").unwrap().resolved_path(),
        primary
    );
    set.verify().unwrap();
}

#[test]
fn refuses_wrong_executable_or_root_digest() {
    let temporary = tempfile::tempdir().unwrap();
    let root = canonical_root(temporary.path());
    let invocation = root.join("program");
    executable(&invocation, DATA, 0o755);
    let selected_root = root_selection(&root);

    assert!(CapturedProgramSet::capture(
        std::slice::from_ref(&selected_root),
        &[program("builder", 0, invocation.clone(), b"wrong bytes")],
    )
    .is_err());

    let mut wrong_root = selected_root;
    wrong_root.content_sha256 = sha256_bytes(b"wrong root content");
    assert!(
        CapturedProgramSet::capture(&[wrong_root], &[program("builder", 0, invocation, DATA)],)
            .is_err()
    );
}

#[test]
fn refuses_wrong_root_index() {
    let temporary = tempfile::tempdir().unwrap();
    let root = canonical_root(temporary.path());
    let invocation = root.join("program");
    executable(&invocation, DATA, 0o755);
    assert!(CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[program("builder", 1, invocation, DATA)],
    )
    .is_err());
}

#[test]
fn refuses_overlapping_captured_roots() {
    let temporary = tempfile::tempdir().unwrap();
    let root = canonical_root(temporary.path());
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    let parent_program = root.join("program");
    let nested_program = nested.join("program");
    executable(&parent_program, DATA, 0o755);
    executable(&nested_program, DATA, 0o755);

    assert!(CapturedProgramSet::capture(
        &[root_selection(&root), root_selection(&nested)],
        &[
            program("parent", 0, parent_program, DATA),
            program("nested", 1, nested_program, DATA),
        ],
    )
    .is_err());
}

#[test]
fn refuses_oversized_aggregate_selection_input() {
    let long_path = PathBuf::from(format!("/{}", "x".repeat(8 * 1024 - 1)));
    let roots = [ProgramRootSelection {
        root: long_path.clone(),
        content_sha256: sha256_bytes(b"root"),
    }];
    let programs = (0..64)
        .map(|index| ProgramSelection {
            role: format!("role-{index}"),
            root_index: 0,
            invocation_path: long_path.clone(),
            executable_sha256: sha256_bytes(b"program"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        CapturedProgramSet::capture(&roots, &programs).unwrap_err(),
        "program selection input exceeds limit"
    );
}

#[test]
fn refuses_paths_outside_roots_and_symlink_escapes() {
    let root_temp = tempfile::tempdir().unwrap();
    let outside_temp = tempfile::tempdir().unwrap();
    let outside_root = canonical_root(outside_temp.path());
    let outside = outside_root.join("external");
    executable(&outside, DATA, 0o755);
    let root = canonical_root(root_temp.path());
    let alias = root.join("program-alias");
    symlink(&outside, &alias).unwrap();
    assert!(CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[program("builder", 0, alias, DATA)],
    )
    .is_err());

    let other_root = tempfile::tempdir().unwrap();
    let other_root_path = canonical_root(other_root.path());
    let foreign_invocation = other_root_path.join("program");
    executable(&foreign_invocation, DATA, 0o755);
    let selected = tempfile::tempdir().unwrap();
    let selected_root = canonical_root(selected.path());
    assert!(CapturedProgramSet::capture(
        &[root_selection(&selected_root)],
        &[program("builder", 0, foreign_invocation, DATA)],
    )
    .is_err());
}

#[test]
fn refuses_non_executable_regular_data() {
    let temporary = tempfile::tempdir().unwrap();
    let root = canonical_root(temporary.path());
    let invocation = root.join("program");
    executable(&invocation, DATA, 0o644);
    assert!(CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[program("builder", 0, invocation, DATA)],
    )
    .is_err());
}

#[test]
fn refuses_duplicate_roles_and_invocation_spellings() {
    let temporary = tempfile::tempdir().unwrap();
    let root = canonical_root(temporary.path());
    let first = root.join("first");
    let second = root.join("second");
    executable(&first, DATA, 0o755);
    executable(&second, DATA, 0o755);
    let selected_root = root_selection(&root);

    assert!(CapturedProgramSet::capture(
        std::slice::from_ref(&selected_root),
        &[
            program("same-role", 0, first.clone(), DATA),
            program("same-role", 0, second, DATA),
        ],
    )
    .is_err());
    assert!(CapturedProgramSet::capture(
        &[selected_root],
        &[
            program("first-role", 0, first.clone(), DATA),
            program("second-role", 0, first.clone(), DATA),
        ],
    )
    .is_err());

    let set = CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[program("first-role", 0, first, DATA)],
    )
    .unwrap();
    assert!(set.identity_for_word("first").is_none());
}

#[test]
fn verify_rejects_added_and_removed_tree_entries() {
    let added_tree = tempfile::tempdir().unwrap();
    let added_root = canonical_root(added_tree.path());
    let selected_program_path = added_root.join("program");
    executable(&selected_program_path, DATA, 0o755);
    let added_set = CapturedProgramSet::capture(
        &[root_selection(&added_root)],
        &[program("builder", 0, selected_program_path, DATA)],
    )
    .unwrap();
    fs::write(added_root.join("new-entry"), b"new").unwrap();
    assert!(added_set.verify().is_err());

    let removed_tree = tempfile::tempdir().unwrap();
    let removed_root = canonical_root(removed_tree.path());
    let selected_program_path = removed_root.join("program");
    let removable = removed_root.join("extra");
    executable(&selected_program_path, DATA, 0o755);
    fs::write(&removable, b"captured entry").unwrap();
    let removed_set = CapturedProgramSet::capture(
        &[root_selection(&removed_root)],
        &[program("builder", 0, selected_program_path, DATA)],
    )
    .unwrap();
    fs::remove_file(removable).unwrap();
    assert!(removed_set.verify().is_err());
}

#[test]
fn verify_rejects_an_alias_that_changes_its_target() {
    let temporary = tempfile::tempdir().unwrap();
    let root = canonical_root(temporary.path());
    let first = root.join("first");
    let second = root.join("second");
    let alias = root.join("alias");
    executable(&first, DATA, 0o755);
    executable(&second, b"different regular data", 0o755);
    symlink("first", &alias).unwrap();
    let set = CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[program("builder", 0, alias.clone(), DATA)],
    )
    .unwrap();

    fs::remove_file(&alias).unwrap();
    symlink("second", &alias).unwrap();
    assert!(set.verify().is_err());
}

#[test]
fn transaction_with_drift_does_not_return_its_value() {
    let temporary = tempfile::tempdir().unwrap();
    let root = canonical_root(temporary.path());
    let invocation = root.join("program");
    executable(&invocation, DATA, 0o755);
    let set = CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[program("builder", 0, invocation.clone(), DATA)],
    )
    .unwrap();

    let result = set.transaction(|_| {
        fs::write(&invocation, b"changed regular-file program data").unwrap();
        Ok("private result")
    });
    assert!(result.is_err());
}

#[test]
fn refuses_non_normalized_or_noncanonical_root_paths() {
    let temporary = tempfile::tempdir().unwrap();
    let canonical_root = canonical_root(temporary.path());
    let invocation = canonical_root.join("program");
    executable(&invocation, DATA, 0o755);
    let noncanonical_spelling = canonical_root.join(".");
    let tree = measure_tree_content_cas(&canonical_root).unwrap();
    let selected_root = ProgramRootSelection {
        root: noncanonical_spelling,
        content_sha256: tree.payload_digest_excluding(None),
    };
    assert!(CapturedProgramSet::capture(
        &[selected_root],
        &[program("builder", 0, invocation, DATA)],
    )
    .is_err());

    let selected_root = root_selection(&canonical_root);
    let non_normalized_invocation = canonical_root.join("nested/../program");
    assert!(CapturedProgramSet::capture(
        &[selected_root],
        &[program("builder", 0, non_normalized_invocation, DATA)],
    )
    .is_err());
}

#[test]
fn input_types_redact_structural_debug_values() {
    let digest = Sha256Digest::parse(&"a".repeat(64)).unwrap();
    let root = ProgramRootSelection {
        root: PathBuf::from("/private/root-marker"),
        content_sha256: digest.clone(),
    };
    let selection = ProgramSelection {
        role: "private-role-marker".into(),
        root_index: 3,
        invocation_path: PathBuf::from("/private/root-marker/program-marker"),
        executable_sha256: digest.clone(),
    };
    let debug = format!("{root:?} {selection:?}");
    assert!(!debug.contains("/private/root-marker"));
    assert!(!debug.contains("private-role-marker"));
    assert!(!debug.contains(digest.as_str()));
    assert!(debug.contains("root_index"));
}
