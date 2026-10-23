//! Descriptor-bound input reads for a caller-selected Make data interpreter.
//!
//! Exact Make read spellings and physical content identities are distinct.
//! This module proves consistency of requested reads within complete captured
//! roots, not completeness of a source program, execution or source authority.

use crate::{
    directory_entry_names_nofollow_bounded, local_source::LocalSourceIdentity,
    measure_regular_file_bounded, measure_tree_content_cas_bounded, open_regular_file_nofollow,
    publication::filesystem_paths_overlap, sha256_bytes, Sha256Digest, TreeContentCas,
    TreeContentEntryKind, TreeTraversalLimits,
};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fmt,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

const MAX_ROOTS: usize = 8;
const MAX_ROOT_ENTRIES: usize = 100_000;
const MAX_ROOT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_READS: usize = 65_536;
const MAX_TOTAL_READ_BYTES: usize = 64 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 8192;
const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;
const MAX_INCLUDE_DIRECTORIES: usize = 64;
const MAX_QUERY_NAMES: usize = MAX_ROOT_ENTRIES;
const MAX_QUERY_BYTES: usize = MAX_RECORD_BYTES;
const MAX_QUERY_COUNT: usize = MAX_READS;

/// Independently selected complete root and its expected portable content.
#[derive(Clone, Debug)]
pub struct MakeInputRootSelection {
    pub root: PathBuf,
    pub content_sha256: Sha256Digest,
}

#[derive(Clone, Debug)]
struct Root {
    path: PathBuf,
    tree: TreeContentCas,
    measurement: RootMeasurement,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RootMeasurement {
    Complete,
    LocalSource(LocalSourceIdentity),
}

impl Root {
    fn measure(&self) -> Result<TreeContentCas, String> {
        match &self.measurement {
            RootMeasurement::Complete => measure(&self.path),
            RootMeasurement::LocalSource(identity) => {
                identity.verified_tree(&self.path, local_source_preset(identity)?)
            }
        }
    }
}

/// Immutable captured roots and an independently bound process cwd.
#[derive(Clone, Debug)]
pub struct MakeInputSnapshot {
    roots: Arc<[Root]>,
    working_directory: PathBuf,
    include_directories: Option<Vec<String>>,
}

/// Aggregate resource limits for one multi-child Make input transaction.
///
/// Each limit must be nonzero and no greater than the finite ceiling stated
/// here. Per-child limits on an individual [`MakeInputReader`] continue to
/// apply independently. `maximum_reads` counts file probes (including missing
/// include candidates), `maximum_read_bytes` counts measured file bytes,
/// `maximum_queries` counts reader queries and directory checks, and the
/// remaining limits count actual directory names, query bytes, and manifest
/// bytes across all children.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MakeInputBatchLimits {
    pub maximum_children: usize,
    pub maximum_read_bytes: usize,
    pub maximum_reads: usize,
    pub maximum_queries: usize,
    pub maximum_query_names: usize,
    pub maximum_query_bytes: usize,
    pub maximum_record_bytes: usize,
}

impl MakeInputBatchLimits {
    fn validate(self) -> Result<(), String> {
        if self.maximum_children == 0 || self.maximum_children > 4096 {
            return Err("Make input batch child limit is outside 1..=4096".into());
        }
        if self.maximum_read_bytes == 0 || self.maximum_read_bytes > 1024 * 1024 * 1024 {
            return Err("Make input batch byte limit is outside 1..=1 GiB".into());
        }
        if self.maximum_reads == 0 || self.maximum_reads > 1_000_000 {
            return Err("Make input batch read limit is outside 1..=1000000".into());
        }
        if self.maximum_queries == 0 || self.maximum_queries > 1_000_000 {
            return Err("Make input batch query limit is outside 1..=1000000".into());
        }
        if self.maximum_query_names == 0 || self.maximum_query_names > 1_000_000 {
            return Err("Make input batch query name limit is outside 1..=1000000".into());
        }
        if self.maximum_query_bytes == 0 || self.maximum_query_bytes > 64 * 1024 * 1024 {
            return Err("Make input batch query byte limit is outside 1..=64 MiB".into());
        }
        if self.maximum_record_bytes == 0 || self.maximum_record_bytes > 64 * 1024 * 1024 {
            return Err("Make input batch record byte limit is outside 1..=64 MiB".into());
        }
        Ok(())
    }
}

/// One requested spelling, its resolved regular path and observed content.
/// `None` records proved absence, never an I/O or symlink failure.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MakeInputReadKind {
    /// Exact path lookup, without Make include-directory search.
    Direct,
    /// Original Make include spelling and its complete bound search order.
    Include,
}

/// One verified file lookup, retaining its operation and resolution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakeInputRead {
    pub kind: MakeInputReadKind,
    pub requested: String,
    pub identity: String,
    pub path: PathBuf,
    pub content_sha256: Option<Sha256Digest>,
}

/// Read records verified by the enclosing transaction's successful return.
///
/// A batch callback may inspect a
/// child's provisional manifest, but must not publish it early. This is not
/// proof that an arbitrary caller requested every source input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedMakeInputReads {
    reads: Vec<MakeInputRead>,
}

impl VerifiedMakeInputReads {
    #[must_use]
    pub fn reads(&self) -> &[MakeInputRead] {
        &self.reads
    }

    /// Protect all exact spellings, including absent optional inputs.
    /// The option and its value occupy one argv element, even for `-` names.
    #[must_use]
    pub fn old_file_arguments(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        self.reads
            .iter()
            .filter(|read| seen.insert(read.identity.as_str()))
            .map(|read| format!("--old-file={}", read.identity))
            .collect()
    }
}

/// Temporary reader borrowed only during one verified transaction.
pub struct MakeInputReader<'a> {
    snapshot: &'a MakeInputSnapshot,
    reads: Vec<MakeInputRead>,
    bytes: usize,
    record_bytes: usize,
    probes: usize,
    queries: usize,
    query_names: usize,
    query_bytes: usize,
    failed: bool,
}

/// Bounded access to multiple child working directories over one root check.
///
/// A refused child or resource-limit failure poisons the whole batch, even if
/// the operation callback suppresses the returned error.
pub struct MakeInputBatch<'a> {
    snapshot: &'a MakeInputSnapshot,
    limits: MakeInputBatchLimits,
    check_control: &'a mut dyn FnMut() -> Result<(), String>,
    children: usize,
    reads: usize,
    read_bytes: usize,
    queries: usize,
    query_names: usize,
    query_bytes: usize,
    record_bytes: usize,
    failure: Option<String>,
}

/// An include's actual native read identity and captured text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakeInputText {
    pub identity: String,
    pub text: String,
}

/// One exact Make read identity and its captured bytes, without decoding.
///
/// Debug output intentionally omits both the byte payload and identity so a
/// caller cannot accidentally disclose source contents or paths in logs.
#[derive(Clone, Eq, PartialEq)]
pub struct MakeInputBytes {
    pub identity: String,
    pub bytes: Vec<u8>,
}

impl fmt::Debug for MakeInputBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MakeInputBytes")
            .field("identity", &"<redacted>")
            .field("bytes_len", &self.bytes.len())
            .finish()
    }
}

struct InputContent {
    path: PathBuf,
    digest: Option<Sha256Digest>,
    bytes: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Default)]
struct DirectoryEnumerationCost {
    names: usize,
    bytes: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct SetupCost {
    queries: usize,
    query_names: usize,
    query_bytes: usize,
}

impl SetupCost {
    fn directory(path: &Path, enumeration: DirectoryEnumerationCost) -> Result<Self, String> {
        Ok(Self {
            queries: 1,
            query_names: enumeration.names,
            query_bytes: path
                .as_os_str()
                .len()
                .checked_add(enumeration.bytes)
                .ok_or("Make input child setup query bytes overflowed")?,
        })
    }

    const fn spelling(bytes: usize) -> Self {
        Self {
            queries: 0,
            query_names: 0,
            query_bytes: bytes,
        }
    }
}

impl MakeInputSnapshot {
    /// Capture exact, disjoint roots without adopting their observed digests.
    ///
    /// # Errors
    /// Refuses invalid/overlapping paths, unsafe/changing roots, mismatched
    /// independently supplied content, or explicit resource limits.
    pub fn capture(
        working_directory: &Path,
        selected: &[MakeInputRootSelection],
    ) -> Result<Self, String> {
        validate_absolute(working_directory)?;
        require_canonical(working_directory)?;
        if selected.is_empty() || selected.len() > MAX_ROOTS {
            return Err("invalid Make input root count".into());
        }
        let mut roots: Vec<Root> = Vec::new();
        for selection in selected {
            validate_absolute(&selection.root)?;
            require_canonical(&selection.root)?;
            for root in &roots {
                if filesystem_paths_overlap(&root.path, &selection.root)
                    .map_err(|_| "cannot compare Make input root identities")?
                {
                    return Err("Make input roots overlap or repeat".into());
                }
            }
            let tree = measure(&selection.root)?;
            if tree.payload_digest_excluding(None) != selection.content_sha256 {
                return Err("Make input root differs from independently selected content".into());
            }
            roots.push(Root {
                path: selection.root.clone(),
                tree,
                measurement: RootMeasurement::Complete,
            });
            if roots
                .iter()
                .map(|root| root.tree.entry_count())
                .sum::<usize>()
                > MAX_ROOT_ENTRIES
            {
                return Err("combined Make input root entry count exceeds limit".into());
            }
        }
        let result = Self {
            roots: Arc::from(roots),
            working_directory: working_directory.to_owned(),
            include_directories: None,
        };
        result.require_directory(working_directory)?;
        result.verify()?;
        Ok(result)
    }

    /// Capture one original local-source selection and independently selected
    /// complete roots. The source identity remains the authority for the
    /// source root; a new digest is never adopted from the current filesystem.
    /// A complete root may overlap the source root only when it is equal to or
    /// below that identity's exact excluded generated subtree.
    ///
    /// # Errors
    /// Refuses malformed or changed local source identity, invalid/overlapping
    /// roots, mismatched complete-root content, unsafe paths, or resource excess.
    pub fn capture_with_local_source(
        working_directory: &Path,
        source_root: &Path,
        identity: &LocalSourceIdentity,
        selected: &[MakeInputRootSelection],
    ) -> Result<Self, String> {
        validate_absolute(working_directory)?;
        require_canonical(working_directory)?;
        validate_absolute(source_root)?;
        require_canonical(source_root)?;
        if selected
            .len()
            .checked_add(1)
            .is_none_or(|count| count > MAX_ROOTS)
        {
            return Err("invalid Make input root count".into());
        }

        let preset = local_source_preset(identity)?;
        let generated_root = source_root.join(&identity.generated_subtree);
        let source_tree = identity.verified_tree(source_root, preset)?;
        validate_root_limits(&source_tree)?;

        let mut roots = vec![Root {
            path: source_root.to_owned(),
            tree: source_tree,
            measurement: RootMeasurement::LocalSource(identity.clone()),
        }];
        for selection in selected {
            validate_absolute(&selection.root)?;
            require_canonical(&selection.root)?;
            for root in &roots {
                if filesystem_paths_overlap(&root.path, &selection.root)
                    .map_err(|_| "cannot compare Make input root identities")?
                {
                    let allowed_source_partition =
                        matches!(&root.measurement, RootMeasurement::LocalSource(_))
                            && selection.root.starts_with(&generated_root);
                    if !allowed_source_partition {
                        return Err("Make input roots overlap or repeat".into());
                    }
                }
            }
            let tree = measure(&selection.root)?;
            if tree.payload_digest_excluding(None) != selection.content_sha256 {
                return Err("Make input root differs from independently selected content".into());
            }
            roots.push(Root {
                path: selection.root.clone(),
                tree,
                measurement: RootMeasurement::Complete,
            });
            let entry_count = roots.iter().try_fold(0usize, |total, root| {
                total.checked_add(root.tree.entry_count())
            });
            if entry_count.is_none_or(|count| count > MAX_ROOT_ENTRIES) {
                return Err("combined Make input root entry count exceeds limit".into());
            }
        }

        let result = Self {
            roots: Arc::from(roots),
            working_directory: working_directory.to_owned(),
            include_directories: None,
        };
        result.require_directory(working_directory)?;
        result.verify()?;
        Ok(result)
    }

    /// Bind the independently selected native include-directory order.
    ///
    /// These exact directory spellings must come from the caller's native
    /// option/context proof; an empty list is an explicit claim, not a default.
    /// This data check does not authenticate GNU's `.INCLUDE_DIRS` observation.
    ///
    /// # Errors
    /// Refuses excessive data and directories outside captured real inputs.
    pub fn with_include_directories(mut self, directories: &[String]) -> Result<Self, String> {
        self.include_directories = Some(self.validated_include_directories(directories)?);
        self.verify()?;
        Ok(self)
    }

    fn validated_include_directories(&self, directories: &[String]) -> Result<Vec<String>, String> {
        Self::validate_include_directory_data(directories)?;
        for identity in directories {
            let path = self.resolve(identity)?;
            self.require_directory(&path)?;
        }
        Ok(directories.to_vec())
    }

    fn validate_include_directory_data(directories: &[String]) -> Result<(), String> {
        if directories.len() > MAX_INCLUDE_DIRECTORIES
            || directories
                .iter()
                .try_fold(0usize, |total, directory| {
                    total.checked_add(directory.len())
                })
                .is_none_or(|total| total > MAX_RECORD_BYTES)
        {
            return Err("Make include directory data exceeds limit".into());
        }
        Ok(())
    }

    #[must_use]
    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }

    /// Compare an exact captured root with independently selected content.
    ///
    /// A parent, child or alternate path spelling is not the selected root.
    /// This inspects retained selection data only, not the current filesystem;
    /// a subsequent transaction must still revalidate every captured root.
    #[must_use]
    pub fn matches_root_selection(&self, path: &Path, content_sha256: &Sha256Digest) -> bool {
        self.roots.iter().any(|root| {
            root.path.as_os_str() == path.as_os_str()
                && root.tree.payload_digest_excluding(None) == *content_sha256
        })
    }

    /// Compare held input-root bindings across independently scoped children.
    ///
    /// Requires exact root order, path, captured metadata/content and complete
    /// versus local-source measurement domain (including the local identity).
    /// Cwd and include search remain each child's separate observations; this
    /// does not transfer either scope. No filesystem revalidation is performed.
    #[must_use]
    pub fn same_root_bindings(&self, other: &Self) -> bool {
        self.roots.len() == other.roots.len()
            && self
                .roots
                .iter()
                .zip(other.roots.iter())
                .all(|(left, right)| {
                    left.path == right.path
                        && left.tree == right.tree
                        && match (&left.measurement, &right.measurement) {
                            (RootMeasurement::Complete, RootMeasurement::Complete) => true,
                            (
                                RootMeasurement::LocalSource(left),
                                RootMeasurement::LocalSource(right),
                            ) => left == right,
                            _ => false,
                        }
                })
    }

    /// Compare retained root and exact Make read-context bindings.
    ///
    /// Requires the same root bindings, byte-for-byte working-directory
    /// spelling, and exact ordered include-directory option (including the
    /// distinction between an unspecified list and an explicitly empty one).
    /// This compares retained data only; it does not revalidate the filesystem,
    /// authenticate the context, or prove a transaction.
    #[must_use]
    pub fn same_read_context(&self, other: &Self) -> bool {
        self.same_root_bindings(other)
            && self.working_directory.as_os_str() == other.working_directory.as_os_str()
            && self.include_directories == other.include_directories
    }

    /// Check a selected output delta without adopting changed original inputs.
    ///
    /// `outputs` contains `(mutable_regular_files, fresh_files, fresh_directories)`
    /// relative to the exact captured `output_root`. All other roots, measurement
    /// domains, cwd and include controls must remain identical. Selected mutable
    /// files must not be retained file inputs, even when their bytes are unchanged.
    /// Every supplied file-read group is replayed against the verified later
    /// snapshot, preserving content, resolution and optional-input absence.
    ///
    /// This is a data comparison, not output authority or producer proof. The
    /// owning runner must independently select its writable build root/outputs,
    /// supply its complete retained read groups and check root permissions and
    /// its original lifetime. Directory-query equivalence and subsequent Make
    /// expression evaluation are not established by file-read groups.
    /// Neither snapshot is mutated and the old one still refuses changed trees.
    ///
    /// # Errors
    /// Refuses changed root/context/domain bindings, unselected effects, changed
    /// retained file inputs, read bounds, or unsafe/current input drift.
    pub fn verify_output_transition(
        &self,
        after: &Self,
        output_root: &Path,
        outputs: (&[&str], &[&str], &[&str]),
        reads: &[&VerifiedMakeInputReads],
    ) -> Result<(), String> {
        self.verify_output_transition_with_replacements(after, output_root, outputs, &[], reads)
    }

    /// Check explicit replacements of owned regular outputs as well as other effects.
    ///
    /// Keeps the original context, root/domain and read-replay requirements of
    /// [`Self::verify_output_transition`]. `replaceable_regular_files` is separate
    /// from the in-place-only selection: it permits changed inode/content, not
    /// a changed mode/device, deletion, creation, symlink or directory. Retained
    /// file inputs cannot be replaced, even by identical bytes. Names remain
    /// independently selected effect data, not producer or write authority.
    /// # Errors
    /// Refuses unselected replacements/effects or any original input/read drift.
    pub fn verify_output_transition_with_replacements(
        &self,
        after: &Self,
        output_root: &Path,
        outputs: (&[&str], &[&str], &[&str]),
        replaceable_regular_files: &[&str],
        reads: &[&VerifiedMakeInputReads],
    ) -> Result<(), String> {
        if self.working_directory != after.working_directory
            || self.include_directories != after.include_directories
            || self.roots.len() != after.roots.len()
        {
            return Err("Make output transition changed input context".into());
        }
        let mut selected = false;
        for (before, current) in self.roots.iter().zip(after.roots.iter()) {
            let same_domain = match (&before.measurement, &current.measurement) {
                (RootMeasurement::Complete, RootMeasurement::Complete) => true,
                (RootMeasurement::LocalSource(a), RootMeasurement::LocalSource(b)) => a == b,
                _ => false,
            };
            if before.path != current.path || !same_domain {
                return Err("Make output transition changed root bindings".into());
            }
            if before.path.as_os_str() == output_root.as_os_str() {
                if selected
                    || !matches!(before.measurement, RootMeasurement::Complete)
                    || !before.tree.matches_output_transition_with_replacements(
                        &current.tree,
                        outputs.0,
                        replaceable_regular_files,
                        outputs.1,
                        outputs.2,
                    )
                {
                    return Err("Make output transition is outside selected effects".into());
                }
                selected = true;
            } else if before.tree != current.tree {
                return Err("Make output transition changed an immutable root".into());
            }
        }
        if !selected {
            return Err("Make output transition root is not captured".into());
        }
        for group in reads {
            for read in group.reads() {
                if outputs
                    .0
                    .iter()
                    .chain(replaceable_regular_files)
                    .any(|path| output_root.join(path) == read.path)
                {
                    return Err("Make output transition attempts to mutate a retained input".into());
                }
            }
        }
        after.verify_read_groups(reads)
    }

    /// Replay retained direct/include operations in this snapshot's context.
    ///
    /// Preserves original requested spelling, include search, resolved identity,
    /// physical path, content and proved absence. This does not prove that the
    /// caller supplied every input group or preserve directory-query results.
    /// # Errors
    /// Refuses inconsistent/over-limit records, changed search/resolution/content,
    /// unsafe inputs or snapshot drift. Does not modify any retained snapshot.
    pub fn verify_read_groups(&self, groups: &[&VerifiedMakeInputReads]) -> Result<(), String> {
        let mut expected = std::collections::BTreeMap::new();
        let mut count = 0usize;
        for group in groups {
            for read in group.reads() {
                count = count
                    .checked_add(1)
                    .filter(|count| *count <= MAX_READS)
                    .ok_or("Make retained file reads exceed limit")?;
                let key = (read.kind, read.requested.as_str());
                if expected
                    .insert(key, read)
                    .is_some_and(|prior| prior != read)
                {
                    return Err("Make retained file reads are inconsistent".into());
                }
            }
        }
        let ((), current) = self.transaction(|reader| {
            for (kind, requested) in expected.keys() {
                match kind {
                    MakeInputReadKind::Direct => {
                        reader.read_bytes(requested)?;
                    }
                    MakeInputReadKind::Include => {
                        reader.include_bytes(requested)?;
                    }
                }
            }
            Ok(())
        })?;
        let observed = current
            .reads()
            .iter()
            .map(|read| ((read.kind, read.requested.as_str()), read))
            .collect::<std::collections::BTreeMap<_, _>>();
        if observed != expected {
            return Err("Make output transition changed retained file reads".into());
        }
        Ok(())
    }

    /// Replay retained reads against transient expected trees for selected complete roots.
    ///
    /// Replacement trees are caller-supplied expected data. The caller must
    /// independently justify the actual output transition; this method grants
    /// no output authority and authenticates no producer. It preserves every
    /// untouched root baseline and measurement policy, along with this exact
    /// working directory and ordered include context. The transient snapshot
    /// is used only by [`Self::verify_read_groups`], whose full-root checks
    /// bracket retained read replay. This does not
    /// recapture or adopt any current source or compiler baseline.
    ///
    /// # Errors
    /// Refuses empty/excessive or duplicate replacements, unknown, noncanonical
    /// or local-source roots, unsafe/resource-excess trees, and any root drift
    /// or changed retained read.
    pub fn verify_read_groups_with_complete_root_transitions(
        &self,
        replacements: &[(&Path, &TreeContentCas)],
        groups: &[&VerifiedMakeInputReads],
    ) -> Result<(), String> {
        if replacements.is_empty() || replacements.len() > MAX_ROOTS {
            return Err("invalid Make input root replacement count".into());
        }

        validate_absolute(&self.working_directory)?;
        require_canonical(&self.working_directory)?;
        if self.roots.is_empty() || self.roots.len() > MAX_ROOTS {
            return Err("invalid Make input root count".into());
        }
        for root in self.roots.iter() {
            validate_absolute(&root.path)?;
            require_canonical(&root.path)?;
        }

        let mut roots = self.roots.to_vec();
        let mut replaced_roots = BTreeSet::new();
        for (path, tree) in replacements {
            validate_absolute(path)?;
            require_canonical(path)?;
            let index = self
                .roots
                .iter()
                .position(|root| root.path.as_os_str() == path.as_os_str())
                .ok_or("Make input root replacement is not captured")?;
            if !replaced_roots.insert(index) {
                return Err("Make input root replacement is duplicated".into());
            }
            if !matches!(self.roots[index].measurement, RootMeasurement::Complete) {
                return Err("Make input root replacement is not a complete root".into());
            }
            validate_root_limits(tree)?;
            roots[index].tree = (*tree).clone();
        }

        let mut total_entries = 0usize;
        for root in &roots {
            validate_root_limits(&root.tree)?;
            total_entries = total_entries
                .checked_add(root.tree.entry_count())
                .ok_or("combined Make input root entry count exceeds limit")?;
            if total_entries > MAX_ROOT_ENTRIES {
                return Err("combined Make input root entry count exceeds limit".into());
            }
        }

        let mut transient = Self {
            roots: roots.into(),
            working_directory: self.working_directory.clone(),
            include_directories: self.include_directories.clone(),
        };
        transient.require_directory(&transient.working_directory)?;
        if let Some(directories) = &self.include_directories {
            transient.include_directories =
                Some(transient.validated_include_directories(directories)?);
        }
        transient.verify_read_groups(groups)
    }

    /// Revalidate directory/file identities, bytes and metadata in all roots.
    ///
    /// # Errors
    /// Refuses any unsafe, missing or changed captured tree.
    pub fn verify(&self) -> Result<(), String> {
        self.verify_with_measurer(None, Root::measure)
    }

    /// Revalidate two retained snapshots in one synchronous measurement pass.
    ///
    /// Only identical canonical roots and measurement policies share a live
    /// measurement. Both retained baselines must agree; additional roots remain
    /// independently checked. No measurement or validation token is retained.
    /// # Errors
    /// Refuses conflicting baselines, unsafe roots or any changed input tree.
    pub fn verify_with(&self, other: &Self) -> Result<(), String> {
        self.verify_with_measurer(Some(other), Root::measure)
    }

    fn verify_with_measurer(
        &self,
        other: Option<&Self>,
        mut measure_root: impl FnMut(&Root) -> Result<TreeContentCas, String>,
    ) -> Result<(), String> {
        let roots = self
            .roots
            .iter()
            .chain(other.into_iter().flat_map(|snapshot| snapshot.roots.iter()))
            .collect::<Vec<_>>();
        let mut domains: Vec<&Root> = Vec::new();
        for root in &roots {
            if let Some(previous) = domains.iter().find(|previous| {
                previous.path == root.path && previous.measurement == root.measurement
            }) {
                if previous.tree != root.tree {
                    return Err("Make input snapshots disagree on retained root".into());
                }
            } else {
                domains.push(root);
            }
        }
        for domain in domains {
            let current = measure_root(domain)?;
            for root in roots
                .iter()
                .filter(|root| root.path == domain.path && root.measurement == domain.measurement)
            {
                if current != root.tree {
                    return Err("Make input root changed".into());
                }
            }
        }
        Ok(())
    }

    /// Observe the current tree of one already captured complete output root.
    ///
    /// The returned snapshot is untrusted observation data until compared with
    /// this original snapshot using an independently selected output policy.
    /// It preserves cwd, include policy, root order and measurement domains;
    /// it neither updates this snapshot nor adopts changed immutable roots.
    /// # Errors
    /// Refuses an unknown/local-source root or a failed bounded measurement.
    pub fn observe_output_root(&self, output_root: &Path) -> Result<Self, String> {
        let mut roots = self.roots.to_vec();
        let root = roots
            .iter_mut()
            .find(|root| root.path.as_os_str() == output_root.as_os_str())
            .filter(|root| matches!(root.measurement, RootMeasurement::Complete))
            .ok_or("Make output observation root is not a captured complete root")?;
        root.tree = measure(&root.path)?;
        Ok(Self {
            roots: roots.into(),
            working_directory: self.working_directory.clone(),
            include_directories: self.include_directories.clone(),
        })
    }

    /// Publish a result and read manifest only after a stable complete batch.
    ///
    /// # Errors
    /// Propagates the callback's failure and refuses input changes. A callback
    /// must not publish its own intermediate results outside this transaction.
    pub fn transaction<T>(
        &self,
        operation: impl FnOnce(&mut MakeInputReader<'_>) -> Result<T, String>,
    ) -> Result<(T, VerifiedMakeInputReads), String> {
        self.verify()?;
        let mut reader = MakeInputReader::new(self);
        let result = operation(&mut reader);
        self.verify()?;
        let result = result?;
        if reader.failed {
            return Err("Make input transaction contains a refused read".into());
        }
        Ok((
            result,
            VerifiedMakeInputReads {
                reads: reader.reads,
            },
        ))
    }

    /// Run multiple child readers against shared captured roots.
    ///
    /// The complete roots are verified before and after the callback, including
    /// when the callback fails. The control check runs at both transaction
    /// boundaries and around every child. Child manifests are usable only when
    /// this outer transaction succeeds. The callback must not publish
    /// intermediate results outside its returned value.
    ///
    /// # Errors
    /// Refuses invalid limits, cancellation, changed roots, failed child reads,
    /// a suppressed child failure, or aggregate resource exhaustion.
    pub fn batch_transaction<T>(
        &self,
        limits: MakeInputBatchLimits,
        mut check_control: impl FnMut() -> Result<(), String>,
        operation: impl FnOnce(&mut MakeInputBatch<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        limits.validate()?;
        self.verify()?;

        let (result, after, verification, failure) = {
            let mut batch = MakeInputBatch {
                snapshot: self,
                limits,
                check_control: &mut check_control,
                children: 0,
                reads: 0,
                read_bytes: 0,
                queries: 0,
                query_names: 0,
                query_bytes: 0,
                record_bytes: 0,
                failure: None,
            };
            let before = batch.check_control();
            let result = match before {
                Ok(()) => operation(&mut batch),
                Err(error) => Err(error),
            };
            if let Err(error) = &result {
                batch.remember_failure(error.clone());
            }
            let after = batch.check_control();
            let verification = self.verify();
            (result, after, verification, batch.failure.clone())
        };

        verification?;
        after?;
        let result = result?;
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(result)
    }

    fn child_scope(
        &self,
        working_directory: &Path,
        include_directories: Option<&[String]>,
        mut charge_setup: impl FnMut(SetupCost) -> Result<(), String>,
    ) -> Result<Self, String> {
        validate_absolute(working_directory)?;
        require_canonical(working_directory)?;
        let cwd_cost = self.require_directory_counted(working_directory)?;
        charge_setup(SetupCost::directory(working_directory, cwd_cost)?)?;
        let mut child = Self {
            roots: Arc::clone(&self.roots),
            working_directory: working_directory.to_owned(),
            include_directories: None,
        };
        let include_directories = include_directories
            .map(|directories| {
                child.validated_child_include_directories(directories, &mut charge_setup)
            })
            .transpose()?;
        child.include_directories = include_directories;
        Ok(child)
    }

    fn validated_child_include_directories(
        &self,
        directories: &[String],
        charge_setup: &mut impl FnMut(SetupCost) -> Result<(), String>,
    ) -> Result<Vec<String>, String> {
        Self::validate_include_directory_data(directories)?;
        for identity in directories {
            charge_setup(SetupCost::spelling(identity.len()))?;
            let path = self.resolve_with(identity, |directory| {
                let enumeration = self.require_directory_counted(directory)?;
                charge_setup(SetupCost::directory(directory, enumeration)?)
            })?;
            let enumeration = self.require_directory_counted(&path)?;
            charge_setup(SetupCost::directory(&path, enumeration)?)?;
        }
        Ok(directories.to_vec())
    }

    fn select(&self, path: &Path) -> Result<(&Root, String), String> {
        let (index, relative) = self.select_index(path)?;
        Ok((&self.roots[index], relative))
    }

    fn select_index(&self, path: &Path) -> Result<(usize, String), String> {
        let index = self
            .roots
            .iter()
            .enumerate()
            .filter(|(_, root)| path.starts_with(&root.path))
            .max_by_key(|(_, root)| root.path.components().count())
            .map(|(index, _)| index)
            .ok_or("Make input leaves captured roots")?;
        let root = &self.roots[index];
        let relative = path
            .strip_prefix(&root.path)
            .map_err(|_| "invalid Make input root binding")?
            .to_str()
            .ok_or("Make input path is not UTF-8")?
            .to_owned();
        if let RootMeasurement::LocalSource(identity) = &root.measurement {
            let relative_path = Path::new(&relative);
            if local_source_path_is_excluded(relative_path, identity) {
                return Err("Make input enters an excluded local source path".into());
            }
        }
        Ok((index, relative))
    }

    fn require_directory(&self, path: &Path) -> Result<(), String> {
        self.require_directory_counted(path).map(|_| ())
    }

    fn require_directory_counted(&self, path: &Path) -> Result<DirectoryEnumerationCost, String> {
        let (root, relative) = self.select(path)?;
        if relative.is_empty()
            || root.tree.entry_kind(&relative) == Some(TreeContentEntryKind::Directory)
        {
            let names = directory_entry_names_nofollow_bounded(path, MAX_ROOT_ENTRIES)
                .map_err(|_| "Make input traversal directory is no longer safe")?;
            let bytes = names.iter().try_fold(0usize, |total, name| {
                total.checked_add(name.as_os_str().as_encoded_bytes().len())
            });
            Ok(DirectoryEnumerationCost {
                names: names.len(),
                bytes: bytes.ok_or("Make input traversal directory byte count overflowed")?,
            })
        } else {
            Err("Make input traversal needs a captured real directory".into())
        }
    }

    fn resolve(&self, identity: &str) -> Result<PathBuf, String> {
        self.resolve_with(identity, |path| self.require_directory(path))
    }

    fn resolve_with(
        &self,
        identity: &str,
        mut require_directory: impl FnMut(&Path) -> Result<(), String>,
    ) -> Result<PathBuf, String> {
        if identity.is_empty()
            || identity.len() > MAX_PATH_BYTES
            || identity.chars().any(char::is_control)
        {
            return Err("invalid Make read identity".into());
        }
        let input = Path::new(identity);
        let mut path = if input.is_absolute() {
            PathBuf::new()
        } else {
            self.working_directory.clone()
        };
        for part in input.components() {
            match part {
                Component::RootDir => path.push(Path::new("/")),
                Component::CurDir => {}
                Component::Normal(part) => path.push(part),
                Component::ParentDir => {
                    // Do not lexically erase a symlink or nonexistent a/../.
                    require_directory(&path)?;
                    path.pop();
                    self.select(&path)?;
                }
                Component::Prefix(_) => return Err("unsupported Make input path prefix".into()),
            }
            if path.as_os_str().len() > MAX_PATH_BYTES {
                return Err("resolved Make input path exceeds limit".into());
            }
        }
        self.select(&path)?;
        Ok(path)
    }
}

impl<'a> MakeInputReader<'a> {
    const fn new(snapshot: &'a MakeInputSnapshot) -> Self {
        Self {
            snapshot,
            reads: Vec::new(),
            bytes: 0,
            record_bytes: 0,
            probes: 0,
            queries: 0,
            query_names: 0,
            query_bytes: 0,
            failed: false,
        }
    }

    /// The exact child snapshot bound to this reader.
    #[must_use]
    pub const fn snapshot(&self) -> &MakeInputSnapshot {
        self.snapshot
    }

    /// Mark the end of the reader's current file-read manifest.
    #[must_use]
    pub const fn read_checkpoint(&self) -> usize {
        self.reads.len()
    }

    /// Clone the provisional file-read suffix after a previously captured checkpoint.
    ///
    /// This returns only retained read records and performs no new reads or
    /// validation. The result remains provisional until the enclosing reader,
    /// batch, or root postchecks succeed; do not publish it early. Any consumer
    /// must preserve the enclosing transaction's failure and postcheck result.
    ///
    /// # Errors
    /// Refuses a checkpoint beyond the reader's current manifest.
    pub fn provisional_reads_since(
        &self,
        checkpoint: usize,
    ) -> Result<VerifiedMakeInputReads, String> {
        let reads = self
            .reads
            .get(checkpoint..)
            .ok_or("Make input read checkpoint is beyond the current manifest")?;
        Ok(VerifiedMakeInputReads {
            reads: reads.to_vec(),
        })
    }

    /// The process cwd bound to this reader's captured input snapshot.
    #[must_use]
    pub fn working_directory(&self) -> &Path {
        self.snapshot.working_directory()
    }

    /// Read one exact Make spelling relative to the bound process cwd.
    ///
    /// # Errors
    /// Refuses out-of-root paths, links, nonregular files, unknown aliases,
    /// changed content/identity, I/O failures and bounded resource exhaustion.
    pub fn read(&mut self, identity: &str) -> Result<Option<String>, String> {
        self.guarded(|reader| {
            let bytes = reader.read_bytes_inner(identity)?;
            bytes
                .map(|input| {
                    String::from_utf8(input.bytes).map_err(|_| "Make input is not UTF-8".to_owned())
                })
                .transpose()
        })
    }

    /// Read one exact Make spelling as captured bytes, without decoding.
    ///
    /// This uses the same descriptor-bound identity/content checks, read
    /// manifest and budgets as [`Self::read`]. It is intended for a parser that
    /// must apply its own encoding-aware syntax rules; it does not interpret
    /// or normalize the file contents.
    ///
    /// # Errors
    /// Refuses out-of-root paths, links, nonregular files, unknown aliases,
    /// changed content/identity, I/O failures and bounded resource exhaustion.
    pub fn read_bytes(&mut self, identity: &str) -> Result<Option<MakeInputBytes>, String> {
        self.guarded(|reader| reader.read_bytes_inner(identity))
    }

    /// Query the captured kind at one exact path spelling without reading file
    /// contents or following a final symbolic link.
    ///
    /// Missing final entries return `None` only after their real parent
    /// directory has been matched to the captured tree. A final symbolic link
    /// is returned as `SymbolicLink`; a symbolic link or non-directory in an
    /// ancestor position, including a missing intermediate component, is an
    /// error. This is a bounded membership query, not
    /// evidence that a Make input was read or that native execution is valid.
    /// Any error poisons the enclosing transaction.
    ///
    /// # Errors
    /// Returns an error for an unsafe or unbound path, an unsafe ancestor, a
    /// changed captured directory, or exhausted query budget.
    pub fn entry_kind(&mut self, identity: &str) -> Result<Option<TreeContentEntryKind>, String> {
        self.guarded(|reader| reader.query_entry_kind(identity))
    }

    /// Query a wildcard candidate with proved missing-ancestor semantics.
    ///
    /// Every existing ancestor must be a captured real directory. At the first
    /// missing component its exact parent listing and native no-follow absence
    /// are revalidated; only then may the remaining suffix be absent. This is
    /// not permission to ignore I/O failures, links, case aliases or paths beyond
    /// captured roots. Parent traversal through a missing component still refuses.
    /// Ordinary file/include reads retain their stricter traversal contract.
    ///
    /// # Errors
    /// Refuses unsafe/unbound/changing paths and exhausted shared query budgets.
    pub fn wildcard_entry_kind(
        &mut self,
        identity: &str,
    ) -> Result<Option<TreeContentEntryKind>, String> {
        self.guarded(|reader| {
            reader.begin_query(identity)?;
            let path = reader.resolve_query_path(identity)?;
            let (root_index, relative) = reader.snapshot.select_index(&path)?;
            let mut prefix = reader.snapshot.roots[root_index].path.clone();
            let components = Path::new(&relative).components().collect::<Vec<_>>();
            for component in components.iter().take(components.len().saturating_sub(1)) {
                prefix.push(component.as_os_str());
                let spelling = prefix.to_str().ok_or("Make wildcard path is not UTF-8")?;
                match reader.query_entry_kind(spelling)? {
                    None => return Ok(None),
                    Some(TreeContentEntryKind::Directory) => {}
                    Some(_) => {
                        return Err("Make wildcard ancestor is not a captured real directory".into())
                    }
                }
            }
            reader.query_entry_kind(identity)
        })
    }

    /// Resolve an existing captured regular file or real directory to its
    /// normalized absolute path from the bound working directory.
    ///
    /// The path is assembled from exact captured names after no-follow
    /// membership checks; aliases and symbolic links are not resolved. This
    /// query does not read file contents or add a Make input read claim. A
    /// missing safe final entry returns `None`. Missing intermediate
    /// components remain errors. GNU Make normalizes terminal `/` and `/.`
    /// before resolving the path, even for a regular file. This differs from
    /// actual file-read and directory-membership queries.
    /// Original tokens at or above the host `PATH_MAX` disappear, as in GNU
    /// Make; the independent query safety/resource bounds remain enforced.
    /// Any error poisons the enclosing transaction.
    ///
    /// # Errors
    /// Refuses an unbound or unsafe path, native aliases, final or ancestor
    /// symbolic links, changed entries, I/O failures, or exhausted query
    /// budgets.
    pub fn realpath(&mut self, identity: &str) -> Result<Option<PathBuf>, String> {
        self.guarded(|reader| reader.query_realpath(identity))
    }

    /// List the exact immediate UTF-8 basenames and captured kinds of one real
    /// directory, sorted by basename.
    ///
    /// A missing final directory returns `None` only after its real parent has
    /// been matched to the captured tree. A final file or symbolic link, and
    /// any symbolic link or non-directory in an ancestor position, is an
    /// error. Missing intermediate components also fail. This is a bounded
    /// directory membership query, not evidence that
    /// a Make input was read or that native execution is valid. Any error
    /// poisons the enclosing transaction.
    ///
    /// # Errors
    /// Returns an error for an unsafe or unbound path, an unsafe ancestor, a
    /// non-directory target, non-UTF-8 child names, a changed captured
    /// directory, or exhausted query budget.
    pub fn directory_entries(
        &mut self,
        identity: &str,
    ) -> Result<Option<Vec<(String, TreeContentEntryKind)>>, String> {
        self.guarded(|reader| reader.query_directory_entries(identity))
    }

    /// Resolve a GNU include using the independently bound search order.
    ///
    /// Cwd is checked first. A missing relative input cannot be called absent
    /// without an explicit include-directory binding, even for `-include`.
    /// Search selection preserves the native prefix spelling, not a canonical
    /// pathname substituted into MAKEFILE_LIST or old-file arguments.
    ///
    /// # Errors
    /// Refuses an unbound search, every read failure, and resource exhaustion.
    pub fn include(&mut self, requested: &str) -> Result<Option<MakeInputText>, String> {
        self.guarded(|reader| {
            reader
                .include_bytes_inner(requested)?
                .map(|input| {
                    String::from_utf8(input.bytes)
                        .map(|text| MakeInputText {
                            identity: input.identity,
                            text,
                        })
                        .map_err(|_| "Make input is not UTF-8".to_owned())
                })
                .transpose()
        })
    }

    /// Resolve and read an include as captured bytes, without decoding.
    ///
    /// Search order, absence semantics, exact identities and accounting are
    /// identical to [`Self::include`]. The caller must still bind include
    /// directories independently before a missing relative include can be
    /// called absent.
    ///
    /// # Errors
    /// Refuses an unbound search, unsafe or changed reads, and bounded resource
    /// exhaustion, as in [`Self::include`].
    pub fn include_bytes(&mut self, requested: &str) -> Result<Option<MakeInputBytes>, String> {
        self.guarded(|reader| reader.include_bytes_inner(requested))
    }

    fn guarded<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.failed {
            return Err("Make input reader was already refused".into());
        }
        let result = operation(self);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn read_bytes_inner(&mut self, identity: &str) -> Result<Option<MakeInputBytes>, String> {
        let input = self.probe(identity)?;
        self.record(MakeInputReadKind::Direct, identity, identity, &input)?;
        Ok(input.bytes.map(|bytes| MakeInputBytes {
            identity: identity.to_owned(),
            bytes,
        }))
    }

    fn include_bytes_inner(&mut self, requested: &str) -> Result<Option<MakeInputBytes>, String> {
        let direct = self.probe(requested)?;
        if direct.bytes.is_some() || Path::new(requested).is_absolute() {
            return self.selected_bytes(requested, requested, direct);
        }
        let snapshot = self.snapshot;
        let directories = snapshot
            .include_directories
            .as_ref()
            .ok_or("Make include search has not been independently bound")?;
        for directory in directories {
            if directory
                .len()
                .saturating_add(1)
                .saturating_add(requested.len())
                > MAX_PATH_BYTES
            {
                return Err("searched Make input path exceeds limit".into());
            }
            let identity = format!("{directory}/{requested}");
            let input = self.probe(&identity)?;
            if input.bytes.is_some() {
                return self.selected_bytes(requested, &identity, input);
            }
        }
        self.selected_bytes(requested, requested, direct)
    }

    fn selected_bytes(
        &mut self,
        requested: &str,
        identity: &str,
        input: InputContent,
    ) -> Result<Option<MakeInputBytes>, String> {
        self.record(MakeInputReadKind::Include, requested, identity, &input)?;
        Ok(input.bytes.map(|bytes| MakeInputBytes {
            identity: identity.to_owned(),
            bytes,
        }))
    }

    fn query_entry_kind(&mut self, identity: &str) -> Result<Option<TreeContentEntryKind>, String> {
        self.query_entry_kind_and_path(identity)
            .map(|(_, kind)| kind)
    }

    fn query_realpath(&mut self, identity: &str) -> Result<Option<PathBuf>, String> {
        if identity.len() > MAX_PATH_BYTES {
            return Err("Make realpath input path exceeds limit".into());
        }
        // GNU Make tests the original token against its host PATH_MAX before
        // normalization. Use the host binding, not a target/architecture table.
        if identity.len() >= native_make_path_limit()? {
            self.begin_query(identity)?;
            return Ok(None);
        }
        // Normalize only terminal separators/dots here. In particular, do not
        // erase an intermediate link or unknown component before `..`.
        let mut normalized = identity;
        loop {
            let trimmed = normalized.trim_end_matches('/');
            let trimmed = if trimmed.is_empty() && normalized.starts_with('/') {
                "/"
            } else {
                trimmed
            };
            if let Some(parent) = trimmed.strip_suffix("/.") {
                normalized = if parent.is_empty() { "/" } else { parent };
            } else {
                normalized = trimmed;
                break;
            }
        }
        self.query_bytes = self
            .query_bytes
            .checked_add(identity.len().saturating_sub(normalized.len()))
            .filter(|bytes| *bytes <= MAX_QUERY_BYTES)
            .ok_or("Make input query bytes exceed limit")?;
        let (path, kind) = self.query_entry_kind_and_path(normalized)?;
        match kind {
            None => Ok(None),
            Some(TreeContentEntryKind::RegularFile) => {
                open_regular_file_nofollow(&path)
                    .map_err(|_| "Make realpath target is not a safe captured regular file")?;
                Ok(Some(path))
            }
            Some(TreeContentEntryKind::Directory) => {
                let (_, relative) = self.snapshot.select_index(&path)?;
                // The root was already listed by the shared query helper;
                // avoid charging its query-name budget twice.
                if !relative.is_empty() {
                    captured_directory_entries(
                        self.snapshot,
                        &mut self.query_names,
                        &mut self.query_bytes,
                        &path,
                    )?;
                }
                Ok(Some(path))
            }
            Some(TreeContentEntryKind::SymbolicLink) => {
                Err("Make realpath refuses a final symbolic link".into())
            }
        }
    }

    fn query_entry_kind_and_path(
        &mut self,
        identity: &str,
    ) -> Result<(PathBuf, Option<TreeContentEntryKind>), String> {
        self.begin_query(identity)?;
        let path = self.resolve_query_path(identity)?;
        let snapshot = self.snapshot;
        let (root_index, relative) = snapshot.select_index(&path)?;
        if relative.is_empty() {
            captured_directory_entries(
                snapshot,
                &mut self.query_names,
                &mut self.query_bytes,
                &path,
            )?;
            return Ok((path, Some(TreeContentEntryKind::Directory)));
        }

        let relative_path = Path::new(&relative);
        let parent_path = path.parent().ok_or("Make query has no parent directory")?;
        let leaf = relative_path
            .file_name()
            .ok_or("Make query has no final path component")?
            .as_encoded_bytes();
        let parent_kind = captured_directory_child_kind(
            snapshot,
            &mut self.query_names,
            &mut self.query_bytes,
            parent_path,
            leaf,
        )?;
        let kind = snapshot.roots[root_index].tree.entry_kind(&relative);
        let kind = match (parent_kind, kind) {
            (None, None) => {
                verify_absent_exact_path(&path)?;
                None
            }
            (Some(parent_kind), Some(captured_kind)) if parent_kind == captured_kind => {
                Some(captured_kind)
            }
            (Some(_), None) => {
                return Err("Make query found an entry outside the captured tree".into());
            }
            (None, Some(_)) => return Err("captured Make query entry disappeared".into()),
            (Some(_), Some(_)) => {
                return Err("captured Make query entry kind changed".into());
            }
        };
        if has_directory_only_suffix(identity) {
            return match kind {
                Some(TreeContentEntryKind::Directory) => Ok((path, kind)),
                Some(TreeContentEntryKind::RegularFile) | None => Ok((path, None)),
                Some(TreeContentEntryKind::SymbolicLink) => {
                    Err("Make query directory suffix traverses a symbolic link".into())
                }
            };
        }
        Ok((path, kind))
    }

    fn query_directory_entries(
        &mut self,
        identity: &str,
    ) -> Result<Option<Vec<(String, TreeContentEntryKind)>>, String> {
        self.begin_query(identity)?;
        let path = self.resolve_query_path(identity)?;
        let snapshot = self.snapshot;
        let (root_index, relative) = snapshot.select_index(&path)?;
        if relative.is_empty() {
            let entries = captured_directory_entries(
                snapshot,
                &mut self.query_names,
                &mut self.query_bytes,
                &path,
            )?;
            return utf8_directory_entries(entries).map(Some);
        }

        let relative_path = Path::new(&relative);
        let parent_path = path.parent().ok_or("Make query has no parent directory")?;
        let leaf = relative_path
            .file_name()
            .ok_or("Make query has no final path component")?
            .as_encoded_bytes();
        let parent_kind = captured_directory_child_kind(
            snapshot,
            &mut self.query_names,
            &mut self.query_bytes,
            parent_path,
            leaf,
        )?;
        let kind = snapshot.roots[root_index].tree.entry_kind(&relative);
        let kind = match (parent_kind, kind) {
            (None, None) => {
                verify_absent_exact_path(&path)?;
                return Ok(None);
            }
            (Some(parent_kind), Some(captured_kind)) if parent_kind == captured_kind => {
                captured_kind
            }
            (Some(_), None) => {
                return Err("Make query found an entry outside the captured tree".into());
            }
            (None, Some(_)) => return Err("captured Make query entry disappeared".into()),
            (Some(_), Some(_)) => {
                return Err("captured Make query entry kind changed".into());
            }
        };
        if kind != TreeContentEntryKind::Directory {
            return Err("Make directory query target is not a captured directory".into());
        }

        let entries = captured_directory_entries(
            snapshot,
            &mut self.query_names,
            &mut self.query_bytes,
            &path,
        )?;
        utf8_directory_entries(entries).map(Some)
    }

    fn begin_query(&mut self, identity: &str) -> Result<(), String> {
        self.queries = self
            .queries
            .checked_add(1)
            .filter(|count| *count <= MAX_QUERY_COUNT)
            .ok_or("Make input query count exceeds limit")?;
        self.query_bytes = self
            .query_bytes
            .checked_add(identity.len())
            .filter(|bytes| *bytes <= MAX_QUERY_BYTES)
            .ok_or("Make input query bytes exceed limit")?;
        Ok(())
    }

    fn resolve_query_path(&mut self, identity: &str) -> Result<PathBuf, String> {
        let snapshot = self.snapshot;
        let query_names = &mut self.query_names;
        let query_bytes = &mut self.query_bytes;
        snapshot.resolve_with(identity, |directory| {
            captured_directory_entries(snapshot, query_names, query_bytes, directory).map(|_| ())
        })
    }

    fn record(
        &mut self,
        kind: MakeInputReadKind,
        requested: &str,
        identity: &str,
        input: &InputContent,
    ) -> Result<(), String> {
        let cost = requested
            .len()
            .saturating_add(identity.len())
            .saturating_add(input.path.as_os_str().len());
        self.record_bytes = self
            .record_bytes
            .checked_add(cost)
            .filter(|bytes| *bytes <= MAX_RECORD_BYTES)
            .ok_or("Make input manifest exceeds limit")?;
        if self.reads.len() >= MAX_READS {
            return Err("Make input read count exceeds limit".into());
        }
        self.reads.push(MakeInputRead {
            kind,
            requested: requested.to_owned(),
            identity: identity.to_owned(),
            path: input.path.clone(),
            content_sha256: input.digest.clone(),
        });
        Ok(())
    }

    fn probe(&mut self, identity: &str) -> Result<InputContent, String> {
        // Components discard these suffixes; do not convert a directory-only
        // native lookup into a successful regular-file read.
        if identity.ends_with('/') || identity.ends_with("/.") {
            return Err("Make file read identity has a directory-only suffix".into());
        }
        self.probes = self
            .probes
            .checked_add(1)
            .filter(|count| *count <= MAX_READS)
            .ok_or("Make input probe count exceeds limit")?;
        let path = self.snapshot.resolve(identity)?;
        let (root, relative) = self.snapshot.select(&path)?;
        let relative_path = Path::new(&relative);
        for ancestor in relative_path
            .ancestors()
            .skip(1)
            .filter(|path| !path.as_os_str().is_empty())
        {
            let name = ancestor
                .to_str()
                .ok_or("Make input ancestor is not UTF-8")?;
            if matches!(
                root.tree.entry_kind(name),
                Some(TreeContentEntryKind::RegularFile | TreeContentEntryKind::SymbolicLink)
            ) {
                return Err("Make input ancestor is not a captured directory".into());
            }
        }
        if matches!(
            root.tree.entry_kind(&relative),
            Some(TreeContentEntryKind::Directory | TreeContentEntryKind::SymbolicLink)
        ) || relative.is_empty()
        {
            return Err("Make input is not a captured regular file".into());
        }
        // A missing tree key alone is not absence on a case-folding filesystem.
        let measured = match measure_regular_file_bounded(&path, MAX_FILE_BYTES) {
            Ok(measured) => measured,
            Err(error)
                if error.kind() == ErrorKind::NotFound
                    && root.tree.entry_kind(&relative).is_none() =>
            {
                None
            }
            Err(_) => return Err("cannot safely read Make input".into()),
        };
        let (content, bytes) = if let Some((identity, bytes)) = measured {
            let digest = sha256_bytes(&bytes);
            if !root
                .tree
                .matches_regular_file_identity(&relative, identity, &digest)
            {
                return Err("Make input identity/content differs from captured selection".into());
            }
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|bytes| *bytes <= MAX_TOTAL_READ_BYTES)
                .ok_or("Make input read bytes exceed limit")?;
            (Some(digest), Some(bytes))
        } else {
            if root.tree.entry_kind(&relative).is_some() {
                return Err("captured Make input disappeared".into());
            }
            (None, None)
        };
        Ok(InputContent {
            path,
            digest: content,
            bytes,
        })
    }
}

impl MakeInputBatch<'_> {
    /// Read inputs for one child cwd, optionally binding its native include
    /// search order. Each child retains the standalone reader's per-child
    /// safety limits; this batch adds aggregate limits across all children.
    ///
    /// The callback's snapshot and reader refer to the same child scope. The
    /// roots are shared with the enclosing snapshot and are revalidated only
    /// at the outer batch boundaries.
    ///
    /// # Errors
    /// Refuses an invalid cwd or include binding, cancellation, callback or
    /// reader failure, or aggregate read/query limit exhaustion.
    pub fn read_child<T>(
        &mut self,
        working_directory: &Path,
        include_directories: Option<&[String]>,
        operation: impl FnOnce(&MakeInputSnapshot, &mut MakeInputReader<'_>) -> Result<T, String>,
    ) -> Result<(T, VerifiedMakeInputReads), String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        self.children = match self.children.checked_add(1) {
            Some(children) if children <= self.limits.maximum_children => children,
            _ => return self.refuse("Make input batch child count exceeds limit"),
        };

        let child_result: Result<(T, VerifiedMakeInputReads), String> = (|| {
            self.check_control()?;
            let snapshot = self.snapshot;
            let child_snapshot =
                snapshot.child_scope(working_directory, include_directories, |cost| {
                    self.account_setup_cost(cost)?;
                    self.check_control()
                })?;
            let mut reader = MakeInputReader::new(&child_snapshot);
            let callback_result = operation(&child_snapshot, &mut reader);
            let accounting_result = self.account_reader(&reader);
            let value = callback_result?;
            if reader.failed {
                return Err("Make input child contains a refused read".into());
            }
            accounting_result?;
            Ok((
                value,
                VerifiedMakeInputReads {
                    reads: reader.reads,
                },
            ))
        })();
        if let Err(error) = &child_result {
            self.remember_failure(error.clone());
        }
        let after = self.check_control();
        if let Err(error) = &after {
            self.remember_failure(error.clone());
        }

        match (child_result, after) {
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
            (Ok(result), Ok(())) => Ok(result),
        }
    }

    fn check_control(&mut self) -> Result<(), String> {
        let result = (self.check_control)();
        if let Err(error) = &result {
            self.remember_failure(error.clone());
        }
        result
    }

    fn account_reader(&mut self, reader: &MakeInputReader<'_>) -> Result<(), String> {
        let reads = self
            .reads
            .checked_add(reader.probes)
            .filter(|count| *count <= self.limits.maximum_reads)
            .ok_or("Make input batch read count exceeds limit")?;
        let read_bytes = self
            .read_bytes
            .checked_add(reader.bytes)
            .filter(|bytes| *bytes <= self.limits.maximum_read_bytes)
            .ok_or("Make input batch read bytes exceed limit")?;
        let queries = self
            .queries
            .checked_add(reader.queries)
            .filter(|count| *count <= self.limits.maximum_queries)
            .ok_or("Make input batch query count exceeds limit")?;
        let query_names = self
            .query_names
            .checked_add(reader.query_names)
            .filter(|count| *count <= self.limits.maximum_query_names)
            .ok_or("Make input batch query name count exceeds limit")?;
        let query_bytes = self
            .query_bytes
            .checked_add(reader.query_bytes)
            .filter(|bytes| *bytes <= self.limits.maximum_query_bytes)
            .ok_or("Make input batch query bytes exceed limit")?;
        let record_bytes = self
            .record_bytes
            .checked_add(reader.record_bytes)
            .filter(|bytes| *bytes <= self.limits.maximum_record_bytes)
            .ok_or("Make input batch manifest bytes exceed limit")?;
        self.reads = reads;
        self.read_bytes = read_bytes;
        self.queries = queries;
        self.query_names = query_names;
        self.query_bytes = query_bytes;
        self.record_bytes = record_bytes;
        Ok(())
    }

    fn account_setup_cost(&mut self, cost: SetupCost) -> Result<(), String> {
        let queries = self
            .queries
            .checked_add(cost.queries)
            .filter(|count| *count <= self.limits.maximum_queries)
            .ok_or("Make input batch query count exceeds limit")?;
        let query_names = self
            .query_names
            .checked_add(cost.query_names)
            .filter(|count| *count <= self.limits.maximum_query_names)
            .ok_or("Make input batch query name count exceeds limit")?;
        let query_bytes = self
            .query_bytes
            .checked_add(cost.query_bytes)
            .filter(|bytes| *bytes <= self.limits.maximum_query_bytes)
            .ok_or("Make input batch query bytes exceed limit")?;
        self.queries = queries;
        self.query_names = query_names;
        self.query_bytes = query_bytes;
        Ok(())
    }

    fn refuse<T>(&mut self, error: &str) -> Result<T, String> {
        self.remember_failure(error.to_owned());
        Err(error.to_owned())
    }

    fn remember_failure(&mut self, error: String) {
        if self.failure.is_none() {
            self.failure = Some(error);
        }
    }
}

fn captured_directory_entries(
    snapshot: &MakeInputSnapshot,
    query_names: &mut usize,
    query_bytes: &mut usize,
    path: &Path,
) -> Result<Vec<(Vec<u8>, TreeContentEntryKind)>, String> {
    let (root_index, relative) = snapshot.select_index(path)?;
    let expected = snapshot.roots[root_index]
        .tree
        .directory_entry_kinds(&relative)
        .ok_or("Make query ancestor is not a captured real directory")?;
    let actual = query_directory_names(query_names, query_bytes, path)?;
    if actual.len() != expected.len()
        || actual
            .iter()
            .zip(&expected)
            .any(|(name, (expected_name, _))| {
                name.as_os_str().as_encoded_bytes() != expected_name.as_slice()
            })
    {
        return Err("Make query directory differs from captured selection".into());
    }
    Ok(expected)
}

/// Validate one exact captured child without treating an excluded LocalSource
/// name as part of a complete parent-directory enumeration.
///
/// The nofollow physical listing is still fully counted against the shared
/// query budgets before exact excluded child names are omitted from this
/// membership comparison. This helper returns only the selected child's
/// captured kind; full directory enumeration continues to use
/// `captured_directory_entries` and remains strict.
fn captured_directory_child_kind(
    snapshot: &MakeInputSnapshot,
    query_names: &mut usize,
    query_bytes: &mut usize,
    parent_path: &Path,
    child_name: &[u8],
) -> Result<Option<TreeContentEntryKind>, String> {
    let (root_index, relative) = snapshot.select_index(parent_path)?;
    let root = &snapshot.roots[root_index];
    let expected = root
        .tree
        .directory_entry_kinds(&relative)
        .ok_or("Make query ancestor is not a captured real directory")?;
    let Some((_, child_kind)) = expected
        .iter()
        .find(|(name, _)| name.as_slice() == child_name)
    else {
        // Exclusion tolerance is only for traversing a known captured child.
        // A missing or unknown child still requires the original complete
        // parent-list proof before absence can be reported.
        captured_directory_entries(snapshot, query_names, query_bytes, parent_path)?;
        return Ok(None);
    };
    let actual = query_directory_names(query_names, query_bytes, parent_path)?;
    let mut expected_index = 0;
    for name in actual {
        let name = name.as_os_str().as_encoded_bytes();
        if local_source_excludes_child(&root.measurement, &relative, name) {
            continue;
        }
        let Some((expected_name, _)) = expected.get(expected_index) else {
            return Err("Make query directory differs from captured selection".into());
        };
        if name != expected_name.as_slice() {
            return Err("Make query directory differs from captured selection".into());
        }
        expected_index += 1;
    }
    if expected_index != expected.len() {
        return Err("Make query directory differs from captured selection".into());
    }

    Ok(Some(*child_kind))
}

fn local_source_excludes_child(
    measurement: &RootMeasurement,
    parent_relative: &str,
    child_name: &[u8],
) -> bool {
    let RootMeasurement::LocalSource(identity) = measurement else {
        return false;
    };
    if child_name == b".git" {
        return true;
    }
    let generated_subtree = Path::new(&identity.generated_subtree);
    generated_subtree
        .parent()
        .is_some_and(|parent| parent == Path::new(parent_relative))
        && generated_subtree
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes() == child_name)
}

// An absent exact directory-list key is not absence on a case-folding host.
// Use descriptor/no-follow traversal; never follow or adopt an unknown alias.
fn verify_absent_exact_path(path: &Path) -> Result<(), String> {
    match open_regular_file_nofollow(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        _ => Err("Make query missing key has an unsafe or aliased native lookup".into()),
    }
}

fn query_directory_names(
    query_names: &mut usize,
    query_bytes: &mut usize,
    path: &Path,
) -> Result<Vec<OsString>, String> {
    let remaining_names = MAX_QUERY_NAMES.saturating_sub(*query_names);
    let names = directory_entry_names_nofollow_bounded(path, remaining_names.max(1))
        .map_err(|_| "cannot safely list Make query directory")?;
    let name_bytes = names.iter().try_fold(0usize, |total, name| {
        total.checked_add(name.as_os_str().as_encoded_bytes().len())
    });
    *query_names = query_names
        .checked_add(names.len())
        .filter(|count| *count <= MAX_QUERY_NAMES)
        .ok_or("Make input query name count exceeds limit")?;
    *query_bytes = query_bytes
        .checked_add(name_bytes.ok_or("Make input query bytes exceed limit")?)
        .filter(|bytes| *bytes <= MAX_QUERY_BYTES)
        .ok_or("Make input query bytes exceed limit")?;
    Ok(names)
}

fn utf8_directory_entries(
    entries: Vec<(Vec<u8>, TreeContentEntryKind)>,
) -> Result<Vec<(String, TreeContentEntryKind)>, String> {
    let mut result = entries
        .into_iter()
        .map(|(name, kind)| {
            String::from_utf8(name)
                .map(|name| (name, kind))
                .map_err(|_| "Make query directory contains a non-UTF-8 basename")
        })
        .collect::<Result<Vec<_>, _>>()?;
    result.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(result)
}

fn has_directory_only_suffix(identity: &str) -> bool {
    identity.ends_with('/') || identity.ends_with("/.")
}

fn validate_absolute(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path.as_os_str().len() > MAX_PATH_BYTES
        || path
            .to_str()
            .is_none_or(|text| text.chars().any(char::is_control))
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("Make input root/cwd must be a normalized absolute path".into());
    }
    Ok(())
}

fn require_canonical(path: &Path) -> Result<(), String> {
    if path
        .canonicalize()
        .map_err(|_| "cannot canonicalize Make input root/cwd")?
        != path
    {
        return Err("Make input root/cwd is not canonical".into());
    }
    Ok(())
}

fn local_source_preset(identity: &LocalSourceIdentity) -> Result<&str, String> {
    let preset = identity
        .generated_subtree
        .strip_prefix("build/")
        .ok_or("local source generated subtree is not an exact build/preset path")?;
    if preset.is_empty()
        || preset.len() > 128
        || !preset
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("local source generated subtree is not an exact build/preset path".into());
    }
    Ok(preset)
}

fn local_source_path_is_excluded(relative: &Path, identity: &LocalSourceIdentity) -> bool {
    relative.components().any(|component| {
        matches!(component, Component::Normal(name) if name == std::ffi::OsStr::new(".git"))
    }) || relative.starts_with(Path::new(&identity.generated_subtree))
}

fn validate_root_limits(tree: &TreeContentCas) -> Result<(), String> {
    if tree.entry_count() > MAX_ROOT_ENTRIES
        || tree
            .regular_file_bytes()
            .is_none_or(|bytes| bytes > MAX_ROOT_BYTES)
    {
        return Err("Make input root exceeds resource limit".into());
    }
    Ok(())
}

fn measure(path: &Path) -> Result<TreeContentCas, String> {
    let limits = TreeTraversalLimits::new(MAX_ROOT_ENTRIES, MAX_ROOT_BYTES)
        .map_err(|_| "invalid Make input tree limits")?;
    measure_tree_content_cas_bounded(path, limits)
        .map_err(|_| "cannot capture complete Make input root".into())
}

fn native_make_path_limit() -> Result<usize, String> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        usize::try_from(libc::PATH_MAX).map_err(|_| "invalid host path limit".into())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err("captured GNU Make path limit is not bound on this host".into())
    }
}

#[cfg(test)]
#[path = "make_input_batch_tests.rs"]
mod batch_tests;

#[cfg(test)]
#[path = "make_input_joint_verification_tests.rs"]
mod joint_verification_tests;

#[cfg(test)]
#[path = "make_input_root_transition_tests.rs"]
mod root_transition_tests;
