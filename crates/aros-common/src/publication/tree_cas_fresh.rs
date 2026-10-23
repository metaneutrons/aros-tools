//! Bounded comparisons for outputs that are known to be fresh.

use super::{TreeContentCas, TreeContentEntry};
use std::collections::BTreeSet;

impl TreeContentCas {
    /// Compare an exact fresh directory and only the ancestors needed to reach it.
    ///
    /// Unlike a fresh directory subtree, this permits no new files, links,
    /// sibling directories or descendants below `directory`. Existing entries
    /// retain the same checks as [`Self::matches_fresh_outputs`]. This is a
    /// snapshot comparison, not proof of a `mkdir` producer or write authority;
    /// root permissions still require the owning transaction's separate check.
    #[must_use]
    pub fn matches_fresh_directory_chain(&self, after: &Self, directory: &str) -> bool {
        self.matches_fresh_outputs(after, &[], &[directory])
            && after.entries.iter().all(|(path, entry)| {
                self.entries.contains_key(path)
                    || (entry.snapshot.kind == 2
                        && (path.as_slice() == directory.as_bytes()
                            || directory
                                .as_bytes()
                                .strip_prefix(path.as_slice())
                                .is_some_and(|suffix| suffix.starts_with(b"/"))))
            })
    }

    /// Compare this measured tree with a later snapshot that may contain only
    /// the specified fresh files and directories.
    ///
    /// The combined selection must be non-empty. Every selected path must be
    /// an exact, normalized relative UTF-8 path, absent in this snapshot, and
    /// present with its declared kind in `after`. Selected paths cannot be
    /// duplicated or overlap by ancestry. Existing entries must remain exact,
    /// except that the size and timestamps of existing directory ancestors
    /// of selected paths may change as the outputs are added. New entries may
    /// only be selected files, directories and regular files below selected
    /// directories, or directory ancestors needed to reach a selected path.
    /// The measured root identity must remain the same.
    /// Root permission mode is not represented in `TreeContentCas`; a live
    /// owner that promises stable root permissions must retain and check that
    /// separate selection before and after the transition.
    ///
    /// This compares snapshot data only and performs no filesystem I/O. The
    /// `fresh_files` and `fresh_directories` names are caller-supplied scope
    /// data; they do not authenticate a producer, establish trust, or grant
    /// authority to write to those paths.
    #[must_use]
    pub fn matches_fresh_outputs(
        &self,
        after: &Self,
        fresh_files: &[&str],
        fresh_directories: &[&str],
    ) -> bool {
        self.matches_output_transition(after, &[], fresh_files, fresh_directories)
    }

    /// Compare this snapshot with a transition containing in-place mutations
    /// to selected existing files and the specified fresh outputs.
    ///
    /// The combined selection must be non-empty. `mutable_regular_files` must
    /// name regular files present in both snapshots with the same device,
    /// inode, kind and mode; only their content, size, and timestamps may
    /// change. These names cannot authorize creation, deletion, replacement,
    /// renaming, mode changes or symbolic links. Fresh files and directories
    /// follow the same rules as [`Self::matches_fresh_outputs`]. Every path
    /// across all three selections must be an exact normalized relative UTF-8
    /// path, unique, and disjoint from the others by ancestry. Directory size
    /// and timestamp changes are allowed only for existing directory ancestors
    /// of fresh outputs.
    ///
    /// This compares snapshot data only and performs no filesystem I/O. It
    /// preserves root identity, but root permission mode is not represented
    /// here and must be checked separately by a caller that relies on it. All
    /// names are caller-supplied selection data; they do not authenticate a
    /// producer, establish trust, or grant authority to write to those paths.
    #[must_use]
    pub fn matches_output_transition(
        &self,
        after: &Self,
        mutable_regular_files: &[&str],
        fresh_files: &[&str],
        fresh_directories: &[&str],
    ) -> bool {
        self.matches_output_transition_with_replacements(
            after,
            mutable_regular_files,
            &[],
            fresh_files,
            fresh_directories,
        )
    }

    /// Compare an explicit output policy that also permits regular-file replacement.
    ///
    /// `replaceable_regular_files` must exist as regular files in both snapshots,
    /// with unchanged mode and filesystem device. Their inode/content may change,
    /// as happens when an archive tool atomically replaces an owned output. This
    /// does not permit deletion, creation under a replacement selection, symbolic links,
    /// directory replacement or mode changes. In-place-only names keep their
    /// stricter identity check. Every selected name across all four lists must
    /// be normalized, unique and disjoint by ancestry.
    ///
    /// Parent directory size/timestamps may change only above selected fresh or
    /// replaceable outputs; directory identity/mode and all other entries remain
    /// exact. As with the other comparisons, this is snapshot data, not writable
    /// path authority, producer evidence, rollback or filesystem confinement.
    /// Root permissions still need the live owner's separate check.
    #[must_use]
    pub fn matches_output_transition_with_replacements(
        &self,
        after: &Self,
        mutable_regular_files: &[&str],
        replaceable_regular_files: &[&str],
        fresh_files: &[&str],
        fresh_directories: &[&str],
    ) -> bool {
        if self.root != after.root {
            return false;
        }

        let selected: Vec<&str> = mutable_regular_files
            .iter()
            .chain(replaceable_regular_files)
            .chain(fresh_files)
            .chain(fresh_directories)
            .copied()
            .collect();
        if selected.is_empty() {
            return false;
        }

        let mut unique = BTreeSet::new();
        for path in &selected {
            if !is_normalized_relative_utf8_path(path) || !unique.insert(*path) {
                return false;
            }
        }
        for path in &selected {
            for (separator, _) in path.match_indices('/') {
                if unique.contains(&path[..separator]) {
                    return false;
                }
            }
        }

        let mutable_paths: BTreeSet<Vec<u8>> = mutable_regular_files
            .iter()
            .map(|path| path.as_bytes().to_vec())
            .collect();
        let replaceable_paths: BTreeSet<Vec<u8>> = replaceable_regular_files
            .iter()
            .map(|path| path.as_bytes().to_vec())
            .collect();
        let fresh_file_paths: BTreeSet<Vec<u8>> = fresh_files
            .iter()
            .map(|path| path.as_bytes().to_vec())
            .collect();
        let fresh_directory_paths: BTreeSet<Vec<u8>> = fresh_directories
            .iter()
            .map(|path| path.as_bytes().to_vec())
            .collect();
        let fresh_paths: BTreeSet<Vec<u8>> = fresh_file_paths
            .iter()
            .chain(&fresh_directory_paths)
            .cloned()
            .collect();
        let namespace_effects: BTreeSet<Vec<u8>> = fresh_paths
            .iter()
            .chain(&replaceable_paths)
            .cloned()
            .collect();

        for path in mutable_regular_files {
            let (Some(before), Some(current)) = (
                self.entries.get(path.as_bytes()),
                after.entries.get(path.as_bytes()),
            ) else {
                return false;
            };
            if !same_regular_file_identity_and_mode(before, current) {
                return false;
            }
        }
        for path in replaceable_regular_files {
            let (Some(before), Some(current)) = (
                self.entries.get(path.as_bytes()),
                after.entries.get(path.as_bytes()),
            ) else {
                return false;
            };
            if !same_regular_file_mode_and_device(before, current) {
                return false;
            }
        }
        for path in fresh_files {
            if self.entries.contains_key(path.as_bytes())
                || after
                    .entries
                    .get(path.as_bytes())
                    .is_none_or(|entry| entry.snapshot.kind != 1)
            {
                return false;
            }
        }
        for path in fresh_directories {
            if self.entries.contains_key(path.as_bytes())
                || after
                    .entries
                    .get(path.as_bytes())
                    .is_none_or(|entry| entry.snapshot.kind != 2)
            {
                return false;
            }
        }

        // Every selected name must be reachable through measured directories.
        // This also rules out a selected path passing through a symbolic link.
        for path in &selected {
            let bytes = path.as_bytes();
            let mut separator = 0;
            while let Some(next_separator) =
                bytes[separator..].iter().position(|byte| *byte == b'/')
            {
                separator += next_separator;
                if after
                    .entries
                    .get(&bytes[..separator])
                    .is_none_or(|entry| entry.snapshot.kind != 2)
                {
                    return false;
                }
                separator += 1;
            }
        }

        for (path, before) in &self.entries {
            let Some(current) = after.entries.get(path) else {
                return false;
            };
            if before == current {
                continue;
            }
            if mutable_paths.contains(path.as_slice()) {
                if !same_regular_file_identity_and_mode(before, current) {
                    return false;
                }
                continue;
            }
            if replaceable_paths.contains(path.as_slice()) {
                if !same_regular_file_mode_and_device(before, current) {
                    return false;
                }
                continue;
            }
            let is_output_ancestor = has_strict_descendant(&namespace_effects, path);
            if !is_output_ancestor || !same_directory_except_mutable_metadata(before, current) {
                return false;
            }
        }

        for (path, entry) in &after.entries {
            if self.entries.contains_key(path) {
                continue;
            }

            let selected_file = fresh_file_paths.contains(path.as_slice());
            if selected_file {
                if entry.snapshot.kind != 1 {
                    return false;
                }
                continue;
            }

            let selected_directory_member =
                is_same_or_descendant_of_any(path, &fresh_directory_paths);
            if selected_directory_member {
                if entry.snapshot.kind != 1 && entry.snapshot.kind != 2 {
                    return false;
                }
                continue;
            }

            let required_directory_ancestor = has_strict_descendant(&fresh_paths, path);
            if required_directory_ancestor && entry.snapshot.kind == 2 {
                continue;
            }

            return false;
        }

        true
    }
}

fn is_normalized_relative_utf8_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.ends_with('/')
        && !path.as_bytes().contains(&0)
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn is_same_or_descendant_of_any(path: &[u8], directories: &BTreeSet<Vec<u8>>) -> bool {
    directories.contains(path)
        || path
            .iter()
            .enumerate()
            .any(|(separator, byte)| *byte == b'/' && directories.contains(&path[..separator]))
}

fn has_strict_descendant(paths: &BTreeSet<Vec<u8>>, ancestor: &[u8]) -> bool {
    let mut prefix = Vec::with_capacity(ancestor.len() + 1);
    prefix.extend_from_slice(ancestor);
    prefix.push(b'/');
    paths
        .range(prefix.clone()..)
        .next()
        .is_some_and(|path| path.starts_with(&prefix))
}

fn same_regular_file_identity_and_mode(
    before: &TreeContentEntry,
    after: &TreeContentEntry,
) -> bool {
    before.snapshot.kind == 1
        && after.snapshot.kind == 1
        && before.snapshot.identity == after.snapshot.identity
        && before.snapshot.mode == after.snapshot.mode
}

const fn same_regular_file_mode_and_device(
    before: &TreeContentEntry,
    after: &TreeContentEntry,
) -> bool {
    before.snapshot.kind == 1
        && after.snapshot.kind == 1
        && before.snapshot.identity.device() == after.snapshot.identity.device()
        && before.snapshot.mode == after.snapshot.mode
}

fn same_directory_except_mutable_metadata(
    before: &TreeContentEntry,
    after: &TreeContentEntry,
) -> bool {
    before.snapshot.kind == 2
        && after.snapshot.kind == 2
        && before.snapshot.identity == after.snapshot.identity
        && before.snapshot.mode == after.snapshot.mode
        && before.content == after.content
}

#[cfg(all(test, unix))]
#[path = "tree_cas_replacement_tests.rs"]
mod replacement_tests;

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::publication::measure_tree_content_cas;
    use std::{
        collections::BTreeMap,
        os::unix::fs::{symlink, PermissionsExt},
    };

    fn measure(path: &std::path::Path) -> TreeContentCas {
        measure_tree_content_cas(path).unwrap()
    }

    #[test]
    fn accepts_fresh_files_and_directory_subtrees_with_existing_ancestor_changes() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join("existing")).unwrap();
        std::fs::write(root.join("existing/kept"), b"same bytes").unwrap();
        let before = measure(root);

        std::fs::write(root.join("existing/fresh.txt"), b"new file").unwrap();
        std::fs::create_dir_all(root.join("generated/nested")).unwrap();
        std::fs::write(root.join("generated/one.c"), b"one").unwrap();
        std::fs::write(root.join("generated/nested/two.c"), b"two").unwrap();
        let after = measure(root);

        assert!(before.matches_fresh_outputs(&after, &["existing/fresh.txt"], &["generated"]));
    }

    #[test]
    fn accepts_fresh_file_below_multiple_new_directory_ancestors() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let before = measure(root);

        std::fs::create_dir_all(root.join("new/inner")).unwrap();
        std::fs::write(root.join("new/inner/output"), b"fresh").unwrap();
        let after = measure(root);

        assert!(before.matches_fresh_outputs(&after, &["new/inner/output"], &[]));
    }

    #[test]
    fn rejects_unselected_writes_and_changes_to_existing_files() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("old"), b"old bytes").unwrap();
        let before = measure(root);

        std::fs::write(root.join("allowed"), b"fresh").unwrap();
        std::fs::write(root.join("outside"), b"unselected").unwrap();
        let after = measure(root);
        assert!(!before.matches_fresh_outputs(&after, &["allowed"], &[]));

        std::fs::remove_file(root.join("outside")).unwrap();
        std::fs::write(root.join("old"), b"changed bytes").unwrap();
        let after = measure(root);
        assert!(!before.matches_fresh_outputs(&after, &["allowed"], &[]));
    }

    #[test]
    fn rejects_chmod_to_an_existing_file() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("old"), b"stable bytes").unwrap();
        std::fs::set_permissions(root.join("old"), std::fs::Permissions::from_mode(0o600)).unwrap();
        let before = measure(root);

        std::fs::write(root.join("fresh"), b"new").unwrap();
        std::fs::set_permissions(root.join("old"), std::fs::Permissions::from_mode(0o644)).unwrap();
        let after = measure(root);
        assert!(!before.matches_fresh_outputs(&after, &["fresh"], &[]));
    }

    #[test]
    fn rejects_replacement_of_an_existing_file() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("old"), b"stable bytes").unwrap();
        let before = measure(root);

        std::fs::write(root.join("fresh"), b"new").unwrap();
        std::fs::write(root.join("replacement"), b"stable bytes").unwrap();
        std::fs::rename(root.join("replacement"), root.join("old")).unwrap();
        let after = measure(root);
        assert!(!before.matches_fresh_outputs(&after, &["fresh"], &[]));
    }

    #[test]
    fn rejects_deletion_of_an_existing_file() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("old"), b"stable bytes").unwrap();
        let before = measure(root);

        std::fs::write(root.join("fresh"), b"new").unwrap();
        std::fs::remove_file(root.join("old")).unwrap();
        let after = measure(root);
        assert!(!before.matches_fresh_outputs(&after, &["fresh"], &[]));
    }

    #[test]
    fn rejects_new_symbolic_links_as_files_or_subtree_members() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let target = temporary.path().join("target");
        std::fs::write(&target, b"target").unwrap();
        let before = measure(&root);

        symlink(&target, root.join("linked-file")).unwrap();
        let after = measure(&root);
        assert!(!before.matches_fresh_outputs(&after, &["linked-file"], &[]));

        std::fs::remove_file(root.join("linked-file")).unwrap();
        std::fs::create_dir(root.join("generated")).unwrap();
        symlink(&target, root.join("generated/link")).unwrap();
        let after = measure(&root);
        assert!(!before.matches_fresh_outputs(&after, &[], &["generated"]));
    }

    #[test]
    fn rejects_empty_duplicate_overlapping_and_malformed_selections() {
        let temporary = tempfile::tempdir().unwrap();
        let before = measure(temporary.path());

        assert!(!before.matches_fresh_outputs(&before, &[], &[]));
        assert!(!before.matches_fresh_outputs(&before, &["same", "same"], &[]));
        assert!(!before.matches_fresh_outputs(&before, &[], &["dir", "dir"]));
        assert!(!before.matches_fresh_outputs(&before, &["dir/file"], &["dir"]));
        assert!(!before.matches_fresh_outputs(&before, &["dir"], &["dir/child"]));

        for malformed in ["", "/absolute", ".", "..", "a/.", "a/..", "a/", "a//b"] {
            assert!(
                !before.matches_fresh_outputs(&before, &[malformed], &[]),
                "accepted malformed selection {malformed:?}"
            );
        }
    }

    #[test]
    fn rejects_a_replaced_measured_root() {
        let temporary = tempfile::tempdir().unwrap();
        let before_root = temporary.path().join("before");
        let after_root = temporary.path().join("replacement");
        std::fs::create_dir(&before_root).unwrap();
        std::fs::create_dir(&after_root).unwrap();
        let before = measure(&before_root);
        std::fs::write(after_root.join("fresh"), b"fresh").unwrap();
        let after = measure(&after_root);

        assert!(!before.matches_fresh_outputs(&after, &["fresh"], &[]));
    }

    #[test]
    fn rejects_previously_present_outputs_and_wrong_kinds() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("existing"), b"old").unwrap();
        let before = measure(root);
        std::fs::write(root.join("new"), b"new").unwrap();
        let after = measure(root);
        assert!(!before.matches_fresh_outputs(&after, &["existing"], &[]));

        let empty = tempfile::tempdir().unwrap();
        let before = measure(empty.path());
        std::fs::create_dir(empty.path().join("directory")).unwrap();
        std::fs::write(empty.path().join("file"), b"file").unwrap();
        let after = measure(empty.path());
        assert!(!before.matches_fresh_outputs(&after, &["directory"], &[]));
        assert!(!before.matches_fresh_outputs(&after, &[], &["file"]));
    }

    #[test]
    fn rejects_chmod_to_an_existing_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("existing")).unwrap();
        std::fs::set_permissions(
            root.join("existing"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let before = measure(&root);

        std::fs::set_permissions(
            root.join("existing"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::write(root.join("fresh"), b"fresh").unwrap();
        let after = measure(&root);
        assert!(!before.matches_fresh_outputs(&after, &["fresh"], &[]));
    }

    #[test]
    fn rejects_replacement_of_an_existing_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("existing")).unwrap();
        let before = measure(&root);

        std::fs::write(root.join("fresh"), b"fresh").unwrap();
        let saved = temporary.path().join("saved-directory");
        std::fs::rename(root.join("existing"), saved).unwrap();
        std::fs::create_dir(root.join("existing")).unwrap();
        let after = measure(&root);
        assert!(!before.matches_fresh_outputs(&after, &["fresh"], &[]));
    }

    #[test]
    fn accepts_in_place_content_change_with_fresh_outputs() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join("artifacts")).unwrap();
        std::fs::write(root.join("artifacts/state"), b"before").unwrap();
        let before = measure(root);

        std::fs::write(
            root.join("artifacts/state"),
            b"updated state with a different size",
        )
        .unwrap();
        std::fs::write(root.join("artifacts/new-output"), b"fresh file").unwrap();
        std::fs::create_dir_all(root.join("generated/nested")).unwrap();
        std::fs::write(root.join("generated/nested/output"), b"fresh subtree").unwrap();
        let after = measure(root);

        assert!(before.matches_output_transition(
            &after,
            &["artifacts/state"],
            &["artifacts/new-output"],
            &["generated"]
        ));
    }

    #[test]
    fn rejects_mode_change_to_mutable_regular_file() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("mutable"), b"before").unwrap();
        std::fs::set_permissions(root.join("mutable"), std::fs::Permissions::from_mode(0o600))
            .unwrap();
        let before = measure(root);

        std::fs::set_permissions(root.join("mutable"), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        let after = measure(root);

        assert!(!before.matches_output_transition(&after, &["mutable"], &[], &[]));
    }

    #[test]
    fn rejects_replacement_deletion_and_symbolic_link_for_mutable_files() {
        let replacement_temporary = tempfile::tempdir().unwrap();
        let replacement_root = replacement_temporary.path();
        std::fs::write(replacement_root.join("mutable"), b"same bytes").unwrap();
        let before = measure(replacement_root);
        std::fs::write(replacement_root.join("replacement"), b"same bytes").unwrap();
        std::fs::rename(
            replacement_root.join("replacement"),
            replacement_root.join("mutable"),
        )
        .unwrap();
        let after = measure(replacement_root);
        assert!(!before.matches_output_transition(&after, &["mutable"], &[], &[]));

        let deletion_temporary = tempfile::tempdir().unwrap();
        let deletion_root = deletion_temporary.path();
        std::fs::write(deletion_root.join("mutable"), b"before").unwrap();
        let before = measure(deletion_root);
        std::fs::remove_file(deletion_root.join("mutable")).unwrap();
        let after = measure(deletion_root);
        assert!(!before.matches_output_transition(&after, &["mutable"], &[], &[]));

        let link_temporary = tempfile::tempdir().unwrap();
        let link_root = link_temporary.path();
        std::fs::write(link_root.join("mutable"), b"before").unwrap();
        let before = measure(link_root);
        let target = link_temporary.path().join("target");
        std::fs::write(&target, b"target").unwrap();
        std::fs::remove_file(link_root.join("mutable")).unwrap();
        symlink(&target, link_root.join("mutable")).unwrap();
        let after = measure(link_root);
        assert!(!before.matches_output_transition(&after, &["mutable"], &[], &[]));
    }

    #[test]
    fn rejects_empty_or_overlapping_transition_policy() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join("directory")).unwrap();
        std::fs::write(root.join("directory/mutable"), b"before").unwrap();
        let snapshot = measure(root);

        assert!(!snapshot.matches_output_transition(&snapshot, &[], &[], &[]));
        assert!(!snapshot.matches_output_transition(
            &snapshot,
            &["directory/mutable"],
            &["directory/mutable"],
            &[]
        ));
        assert!(!snapshot.matches_output_transition(
            &snapshot,
            &["directory/mutable"],
            &[],
            &["directory"]
        ));
        assert!(!snapshot.matches_output_transition(
            &snapshot,
            &["directory/mutable"],
            &["directory"],
            &[]
        ));

        let empty = tempfile::tempdir().unwrap();
        let before = measure(empty.path());
        std::fs::write(empty.path().join("new"), b"created").unwrap();
        let after = measure(empty.path());
        assert!(!before.matches_output_transition(&after, &["new"], &[], &[]));
    }

    #[test]
    fn rejects_unselected_writes_during_mutable_transition() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("mutable"), b"before").unwrap();
        let before = measure(root);

        std::fs::write(root.join("mutable"), b"changed in place").unwrap();
        std::fs::write(root.join("outside"), b"unselected write").unwrap();
        let after = measure(root);

        assert!(!before.matches_output_transition(&after, &["mutable"], &[], &[]));
    }

    #[test]
    fn fresh_directory_chain_accepts_only_exact_new_ancestor_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join("existing")).unwrap();
        std::fs::write(root.join("keep"), b"unchanged").unwrap();
        let before = measure(root);
        std::fs::create_dir_all(root.join("existing/one/two")).unwrap();
        let after = measure(root);
        assert!(before.matches_fresh_directory_chain(&after, "existing/one/two"));
        assert!(!before.matches_fresh_directory_chain(&after, "existing/one"));
        assert!(!before.matches_fresh_directory_chain(&after, "existing"));
        for invalid in [
            "",
            "/existing/one/two",
            "existing/../one",
            "existing/one/two/",
        ] {
            assert!(!before.matches_fresh_directory_chain(&after, invalid));
        }
    }

    #[test]
    fn fresh_directory_chain_refuses_extra_files_directories_and_old_mutation() {
        for effect in ["file", "descendant", "sibling", "mutate", "delete"] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path();
            std::fs::write(root.join("keep"), b"unchanged").unwrap();
            let before = measure(root);
            std::fs::create_dir_all(root.join("one/two")).unwrap();
            match effect {
                "file" => std::fs::write(root.join("one/two/extra"), b"unexpected").unwrap(),
                "descendant" => std::fs::create_dir(root.join("one/two/extra")).unwrap(),
                "sibling" => std::fs::create_dir(root.join("one/two-adjacent")).unwrap(),
                "mutate" => std::fs::write(root.join("keep"), b"changed").unwrap(),
                "delete" => std::fs::remove_file(root.join("keep")).unwrap(),
                _ => unreachable!(),
            }
            let after = measure(root);
            if effect == "file" || effect == "descendant" {
                // The ordinary subtree policy deliberately accepts these;
                // the directory producer must enforce the stronger condition.
                assert!(before.matches_fresh_outputs(&after, &[], &["one/two"]));
            }
            assert!(
                !before.matches_fresh_directory_chain(&after, "one/two"),
                "{effect}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn fresh_directory_chain_refuses_symbolic_links() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().unwrap();
        let before = measure(temporary.path());
        symlink("elsewhere", temporary.path().join("one")).unwrap();
        let after = measure(temporary.path());
        assert!(!before.matches_fresh_directory_chain(&after, "one"));
    }

    #[test]
    fn accepts_large_fresh_file_policy() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("seed"), b"representative output").unwrap();
        let measured = measure(temporary.path());
        let seed = measured.entries.get(b"seed".as_slice()).unwrap().clone();
        let before = TreeContentCas {
            root: measured.root,
            entries: BTreeMap::new(),
        };
        let mut after = before.clone();

        // Reuse measured regular-file data so the policy-size case does not
        // need to create and hash twenty thousand physical files.
        let names: Vec<String> = (0..20_000)
            .map(|index| format!("generated-{index:05}.c"))
            .collect();
        for name in &names {
            after.entries.insert(name.as_bytes().to_vec(), seed.clone());
        }
        let policy: Vec<&str> = names.iter().map(String::as_str).collect();

        assert!(before.matches_fresh_outputs(&after, &policy, &[]));
    }

    #[test]
    fn matches_directory_subtrees_on_raw_bytes_and_respects_adjacent_prefixes() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("a")).unwrap();
        std::fs::create_dir(root.join("a-")).unwrap();
        std::fs::write(root.join("seed"), b"representative file").unwrap();
        let before = measure(&root);

        // Synthetic raw-byte key: APFS rejects this filesystem name, so this
        // verifies byte-prefix matching only, not a physical filesystem write.
        let directory = before.entries.get(b"a".as_slice()).unwrap().clone();
        let file = before.entries.get(b"seed".as_slice()).unwrap().clone();
        let mut before = before;
        before.entries.remove(b"a".as_slice());
        let mut after = before.clone();
        after.entries.insert(b"a/child-\xff.c".to_vec(), file);
        after.entries.insert(b"a".to_vec(), directory);
        assert!(before.matches_fresh_outputs(&after, &[], &["a"]));

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("a")).unwrap();
        let before = measure(&root);

        std::fs::write(root.join("a/unselected"), b"outside selected subtree").unwrap();
        std::fs::create_dir(root.join("a-")).unwrap();
        std::fs::write(root.join("a-/selected"), b"fresh").unwrap();
        std::fs::create_dir(root.join("a.")).unwrap();
        std::fs::write(root.join("a./selected"), b"fresh").unwrap();
        let after = measure(&root);
        assert!(!before.matches_fresh_outputs(&after, &[], &["a-", "a."]));

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let before = measure(&root);

        std::fs::create_dir(root.join("a")).unwrap();
        std::fs::write(root.join("a/selected"), b"fresh").unwrap();
        std::fs::create_dir(root.join("a-")).unwrap();
        std::fs::write(root.join("a-/outside"), b"not under a").unwrap();
        std::fs::create_dir(root.join("a.")).unwrap();
        std::fs::write(root.join("a./outside"), b"also not under a").unwrap();
        let after = measure(&root);
        assert!(!before.matches_fresh_outputs(&after, &[], &["a"]));
    }
}
