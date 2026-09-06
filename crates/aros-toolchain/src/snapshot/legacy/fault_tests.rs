//! Lifecycle boundary models paired with the real publisher's syscall-fault tests.
//!
//! No real volume exhaustion or durability claim: partial metadata writes use
//! ENOSPC; post-rename tests model an uncertain publisher result after a real
//! successful rename. The common source-publisher tests independently exercise
//! its actual after-rename/before-parent-sync error-classification branch.

use super::{operations::PublicationFailure, Operations, SystemOperations};
use crate::{
    canonical,
    plan::{Backend, PlanRequest},
    snapshot::{LegacySourceView, SourceRole, SourceSnapshot},
    workspace::RunDirectories,
    ContractError, Recipe,
};
use aros_common::{publication::PublicationFailureClass, DiagnosticCode};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Write as _},
    os::unix::fs::{symlink, MetadataExt as _},
    path::{Path, PathBuf},
};

#[path = "../fixture_tests.rs"]
mod fixture;
use fixture::{git, Fixture, TIMEOUT};

#[derive(Clone, Copy)]
enum Fault {
    Write { ordinal: usize, prefix: usize },
    Publish { ordinal: usize, after_rename: bool },
}

struct FaultOperations {
    fault: Fault,
    writes: usize,
    publications: usize,
    fired: bool,
    partial: Option<(u64, Vec<u8>)>,
    current_root: Option<PathBuf>,
    retained_at_failure: Option<TreeInventory>,
}

impl FaultOperations {
    const fn new(fault: Fault) -> Self {
        Self {
            fault,
            writes: 0,
            publications: 0,
            fired: false,
            partial: None,
            current_root: None,
            retained_at_failure: None,
        }
    }
}

impl Operations for FaultOperations {
    fn write_metadata(&mut self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
        assert!(!self.fired, "mutation continued after failure");
        self.writes += 1;
        if let Fault::Write { ordinal, prefix } = self.fault {
            if self.writes == ordinal {
                let prefix = &bytes[..prefix];
                file.write_all(prefix)?;
                self.partial = Some((file.metadata()?.ino(), prefix.to_vec()));
                self.retained_at_failure = Some(inventory(self.current_root.as_ref().unwrap()));
                self.fired = true;
                return Err(rustix::io::Errno::NOSPC.into());
            }
        }
        SystemOperations.write_metadata(file, bytes)
    }

    fn publish(&mut self, staging: &Path, destination: &Path) -> Result<(), PublicationFailure> {
        assert!(!self.fired, "publication continued after failure");
        self.publications += 1;
        if let Fault::Publish {
            ordinal,
            after_rename,
        } = self.fault
        {
            if self.publications == ordinal {
                self.fired = true;
                if after_rename {
                    // Consumer-boundary model only. The real common fsync
                    // error path is separately exercised in common unit tests.
                    SystemOperations.publish(staging, destination)?;
                    self.retained_at_failure = Some(inventory(destination));
                    return Err(PublicationFailure {
                        class: PublicationFailureClass::CommitStateUncertain,
                        // The real typed uncertain error wraps its cause.
                        kind: io::ErrorKind::Other,
                    });
                }
                self.retained_at_failure = Some(inventory(staging));
                return Err(PublicationFailure {
                    class: PublicationFailureClass::Io,
                    kind: io::ErrorKind::StorageFull,
                });
            }
        }
        SystemOperations.publish(staging, destination)?;
        self.current_root = Some(destination.to_owned());
        Ok(())
    }
}

/// Independent byte/mode/link inventory, including Git metadata and empty dirs.
type TreeInventory = BTreeMap<PathBuf, (u32, Vec<u8>)>;

fn inventory(root: &Path) -> TreeInventory {
    fn visit(root: &Path, relative: &Path, result: &mut BTreeMap<PathBuf, (u32, Vec<u8>)>) {
        let current = root.join(relative);
        let metadata = fs::symlink_metadata(&current).unwrap();
        let bytes = if metadata.is_symlink() {
            fs::read_link(&current)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else if metadata.is_file() {
            fs::read(&current).unwrap()
        } else {
            assert!(metadata.is_dir());
            Vec::new()
        };
        result.insert(relative.to_owned(), (metadata.mode(), bytes));
        if metadata.is_dir() {
            for entry in fs::read_dir(current).unwrap() {
                visit(root, &relative.join(entry.unwrap().file_name()), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, Path::new(""), &mut result);
    result
}

fn assert_error(error: &ContractError, expected: &str, fixture: &Fixture) {
    let diagnostics = error.diagnostics();
    assert_eq!(diagnostics.diagnostics.len(), 1);
    let diagnostic = &diagnostics.diagnostics[0];
    assert_eq!(diagnostic.code, DiagnosticCode::ProducerState);
    assert!(diagnostic.message.contains(expected), "{error}");
    assert!(diagnostic.hint.as_ref().is_some_and(|hint| {
        hint.contains("retained") && hint.contains("never adopted, removed or authorized")
    }));
    let json = serde_json::to_string(diagnostics).unwrap();
    assert!(json.contains("AX0801"));
    assert!(!json.contains(fixture.work().parent().unwrap().to_str().unwrap()));
}

fn fixture_with_module(role: SourceRole) -> Fixture {
    let fixture = Fixture::new();
    let selected = match role {
        SourceRole::Source => &fixture.request.source_dir,
        SourceRole::Producer => &fixture.request.producer_dir,
        SourceRole::Tools => &fixture.request.tools_dir,
    };
    // A second store makes a later write failure preserve a completed earlier
    // store too, without fabricating repositories inside the material guard.
    let module_source = if matches!(role, SourceRole::Tools) {
        &fixture.request.producer_dir
    } else {
        &fixture.request.tools_dir
    };
    git(
        selected,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            module_source.to_str().unwrap(),
            "nested module",
        ],
    );
    symlink("dir/Größe file", selected.join("link")).unwrap();
    if matches!(role, SourceRole::Source) {
        fixture.commit_source();
    } else {
        git(selected, &["add", "."]);
        git(selected, &["commit", "-qm", "test: fault lifecycle"]);
    }
    let cache = fixture.request.cache_dir.as_ref().unwrap();
    fs::create_dir(cache).unwrap();
    fs::write(
        cache.join("unrelated cached payload"),
        b"preserve cache bytes",
    )
    .unwrap();
    fixture
}

fn exercise(fault: Fault, role: SourceRole) {
    let fixture = fixture_with_module(role);
    let leaf = role.leaf();
    let recipe = fixture.recipe();
    let run = fixture.reserve();
    let mut other_views = Vec::new();
    for other_role in [SourceRole::Source, SourceRole::Producer, SourceRole::Tools] {
        if other_role.leaf() != leaf {
            let raw = SourceSnapshot::prepare(&run, other_role, &recipe, TIMEOUT, &fixture.token)
                .unwrap();
            other_views.push(LegacySourceView::prepare(raw, TIMEOUT, &fixture.token).unwrap());
        }
    }
    let originals: Vec<_> = [
        &fixture.request.source_dir,
        &fixture.request.producer_dir,
        &fixture.request.tools_dir,
        fixture.request.output_dir.as_ref().unwrap(),
        fixture.request.cache_dir.as_ref().unwrap(),
    ]
    .into_iter()
    .chain(other_views.iter().map(|view| &view.material.root))
    .map(|root| (root.clone(), inventory(root)))
    .collect();
    let raw = SourceSnapshot::prepare(&run, role, &recipe, TIMEOUT, &fixture.token).unwrap();
    let raw_inventory = inventory(raw.root());
    let raw_inode = fs::metadata(raw.root()).unwrap().ino();
    let mut operations = FaultOperations::new(fault);
    let error = LegacySourceView::prepare_using(raw, TIMEOUT, &fixture.token, &mut operations)
        .err()
        .expect("failure must not yield a view guard");
    assert!(operations.fired, "fault boundary not exercised");
    assert_error(
        &error,
        match fault {
            Fault::Publish {
                after_rename: true, ..
            } => "legacy view publication failed (CommitStateUncertain, Other)",
            Fault::Publish {
                after_rename: false,
                ..
            } => "legacy view publication failed (Io, StorageFull)",
            Fault::Write { .. } => "isolated Git metadata I/O failed (StorageFull)",
        },
        &fixture,
    );

    let (retained, retained_inventory) =
        assert_retained(&fixture, leaf, &raw_inventory, raw_inode, &operations);
    run.revalidate(&fixture.token).unwrap();
    for view in &other_views {
        view.revalidate(TIMEOUT, &fixture.token).unwrap();
    }
    for (root, expected) in &originals {
        assert_eq!(&inventory(root), expected);
    }

    // Retained paths are evidence, not resumable receipts. A fresh raw copy is
    // allowed only when its own leaf is absent; the old view must never be adopted.
    match SourceSnapshot::prepare(&run, role, &recipe, TIMEOUT, &fixture.token) {
        Ok(raw) => {
            let error = LegacySourceView::prepare(raw, TIMEOUT, &fixture.token)
                .err()
                .unwrap();
            assert_error(&error, "refusing reuse", &fixture);
        }
        Err(error) => assert_error(&error, "refusing reuse", &fixture),
    }
    assert_eq!(inventory(&retained), retained_inventory);
    drop(other_views);
    run.release().unwrap();
    assert_eq!(inventory(&retained), retained_inventory);
    assert!(RunDirectories::reserve(
        &fixture.request,
        &aros_common::sha256_bytes(b"retry"),
        &fixture.token
    )
    .is_err());
    assert_eq!(inventory(&retained), retained_inventory);
}

fn assert_retained(
    fixture: &Fixture,
    leaf: &str,
    raw_inventory: &TreeInventory,
    raw_inode: u64,
    operations: &FaultOperations,
) -> (PathBuf, TreeInventory) {
    let retained_leaf = match operations.fault {
        Fault::Publish {
            ordinal: 1,
            after_rename: false,
        } => leaf.to_owned(),
        Fault::Publish {
            ordinal: 2,
            after_rename: true,
        } => format!("{leaf}-legacy"),
        _ => format!(".{leaf}-legacy-pending"),
    };
    let retained = fixture.work().join(&retained_leaf);
    for candidate in [
        leaf.to_owned(),
        format!(".{leaf}-legacy-pending"),
        format!("{leaf}-legacy"),
    ] {
        assert_eq!(
            fixture.work().join(&candidate).exists(),
            candidate == retained_leaf
        );
    }
    assert_eq!(fs::metadata(&retained).unwrap().ino(), raw_inode);
    let retained_inventory = inventory(&retained);
    assert_eq!(
        Some(&retained_inventory),
        operations.retained_at_failure.as_ref()
    );
    for (relative, entry) in raw_inventory {
        assert_eq!(
            retained_inventory.get(relative),
            Some(entry),
            "raw {relative:?}"
        );
    }
    if let Some((inode, bytes)) = &operations.partial {
        let paths: Vec<_> = retained_inventory
            .keys()
            .filter(|relative| {
                fs::symlink_metadata(retained.join(relative)).unwrap().ino() == *inode
            })
            .collect();
        assert_eq!(paths.len(), 1);
        assert_eq!(&fs::read(retained.join(paths[0])).unwrap(), bytes);
    }
    match operations.fault {
        Fault::Write { ordinal, .. } => {
            assert_eq!(operations.writes, ordinal);
            assert_eq!(operations.publications, 1);
        }
        Fault::Publish { ordinal, .. } => {
            assert_eq!(operations.publications, ordinal);
            assert_eq!(operations.writes, if ordinal == 1 { 0 } else { 6 });
        }
    }
    (retained, retained_inventory)
}

#[test]
fn metadata_enospc_retains_empty_and_partial_files_in_each_repository() {
    for ordinal in 1..=6 {
        for prefix in [0, 5] {
            exercise(Fault::Write { ordinal, prefix }, SourceRole::Source);
        }
    }
}

#[test]
fn enospc_before_each_relocation_preserves_raw_or_complete_staging() {
    for ordinal in [1, 2] {
        exercise(
            Fault::Publish {
                ordinal,
                after_rename: false,
            },
            SourceRole::Source,
        );
    }
}

#[test]
fn uncertain_result_after_each_rename_retains_tree_without_returning_a_guard() {
    for ordinal in [1, 2] {
        exercise(
            Fault::Publish {
                ordinal,
                after_rename: true,
            },
            SourceRole::Source,
        );
    }
}

#[test]
fn other_roles_obey_the_same_failure_and_retention_contract() {
    for role in [SourceRole::Producer, SourceRole::Tools] {
        exercise(
            Fault::Write {
                ordinal: 4,
                prefix: 5,
            },
            role,
        );
        exercise(
            Fault::Publish {
                ordinal: 2,
                after_rename: true,
            },
            role,
        );
    }
}
