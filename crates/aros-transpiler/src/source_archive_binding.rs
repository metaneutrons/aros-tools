//! Pure binding of source archive rules to typed object producers.
//!
//! This joins already-proved source declarations. It performs no filesystem
//! reads, does not expand generated globs against disk, and does not create
//! graph providers. A returned binding is complete enough for the graph and
//! CMake archive backend to consume without inferring compiler or AR roles.

use crate::literal_objects::LiteralObjectDecl;
use crate::source_archive_command::SourceArchiveCommand;
use crate::source_archive_rules::{ArchiveMembers, SourceArchiveDecl};
use crate::source_compile_rules::SourceCompileGroupDecl;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const BUILD_GEN_ROOT: &str = "${AROS_BUILD_DIR}/gen";
const MAX_MEMBERS: usize = 4096;

pub(crate) type SourceArchiveBindingKey = (String, String);

/// One ordinary archive with its source-selected command roles and complete
/// compile-producer coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundSourceArchive {
    /// Original source declaration, retained for source line and diagnostics.
    pub(crate) declaration: SourceArchiveDecl,
    /// Explicitly proved target archiver and ranlib command roles.
    pub(crate) command: SourceArchiveCommand,
    /// Finite object outputs in source `$^` order or GNU wildcard byte order.
    /// For a producer glob these came only from the supplied reachable set of
    /// typed HIDD groups, never from a configure-time directory listing.
    pub(crate) members: Vec<String>,
    /// Source compile groups that cover every member exactly once.
    pub(crate) compile_groups: Vec<SourceCompileGroupDecl>,
    /// HIDD owner endpoints selected for a `ProducerGlob`; empty for exact
    /// archive member lists.
    pub(crate) producer_owners: BTreeSet<String>,
}

impl BoundSourceArchive {
    /// The exact archive basename, e.g. `libhiddstubs.a`.
    ///
    /// # Panics
    /// Panics if the binding invariant of a validated archive path is violated.
    #[must_use]
    pub fn archive_basename(&self) -> &str {
        self.declaration
            .output
            .rsplit('/')
            .next()
            .expect("validated archive output has a basename")
    }

    /// Imported interface target used for the source archive output.
    #[must_use]
    pub fn provider_target(&self) -> String {
        format!("{}-archive", self.declaration.owner)
    }

    /// The library name without the `lib` prefix or `.a` suffix.
    ///
    /// # Panics
    /// Panics if the binding invariant of a `libNAME.a` basename is violated.
    #[must_use]
    pub fn library_name(&self) -> &str {
        self.archive_basename()
            .strip_prefix("lib")
            .and_then(|name| name.strip_suffix(".a"))
            .expect("validated archive basename has libNAME.a form")
    }
}

/// A source archive candidate refused by the binding pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceArchiveBindingRejection {
    pub(crate) owner: String,
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) reason: String,
}

/// Result of the pure archive/compile projection join.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SourceArchiveBindingResult {
    /// Fully bound ordinary archives.
    pub(crate) archives: Vec<BoundSourceArchive>,
    /// Every independently valid HIDD compile group, whether or not an
    /// archive's explicit reachable-owner set selects it.
    pub(crate) qualified_hidd_groups: Vec<SourceCompileGroupDecl>,
    /// Archive- and HIDD-owner-scoped refusals.
    pub(crate) rejections: Vec<SourceArchiveBindingRejection>,
}

/// Bind candidates to explicit command proofs and complete compile groups.
///
/// `commands` and `reachable_hidd_owners` are keyed by `(source file, archive
/// owner)`. The latter is required for every producer-glob archive and must
/// come from the caller's source graph reachability proof; it is not inferred
/// from matching names here. Its values are exact `(macro parent, child owner)`
/// pairs, preserving the source's intermediate parent endpoint.
#[must_use]
pub(crate) fn bind_source_archives(
    archives: &[SourceArchiveDecl],
    compile_groups: &[SourceCompileGroupDecl],
    commands: &BTreeMap<SourceArchiveBindingKey, SourceArchiveCommand>,
    reachable_hidd_owners: &BTreeMap<SourceArchiveBindingKey, BTreeSet<(String, String)>>,
) -> SourceArchiveBindingResult {
    let (qualified_hidd_groups, mut rejections) = qualify_hidd_groups(compile_groups);
    let mut keys = BTreeMap::<SourceArchiveBindingKey, usize>::new();
    let mut outputs = BTreeMap::<String, usize>::new();
    let mut providers = BTreeMap::<String, usize>::new();
    for archive in archives {
        *keys
            .entry((archive.file.clone(), archive.owner.clone()))
            .or_default() += 1;
        *outputs
            .entry(archive.output.to_ascii_lowercase())
            .or_default() += 1;
        *providers
            .entry(format!("{}-archive", archive.owner).to_ascii_lowercase())
            .or_default() += 1;
    }

    let mut bound = Vec::new();
    for archive in archives {
        let key = (archive.file.clone(), archive.owner.clone());
        let failure = if keys.get(&key).copied().unwrap_or_default() != 1 {
            Some("archive owner has duplicate source declarations".to_owned())
        } else if outputs
            .get(&archive.output.to_ascii_lowercase())
            .copied()
            .unwrap_or_default()
            != 1
        {
            Some(
                "archive output collides with another source declaration by case-folded path"
                    .into(),
            )
        } else if providers
            .get(&format!("{}-archive", archive.owner).to_ascii_lowercase())
            .copied()
            .unwrap_or_default()
            != 1
        {
            Some(
                "archive imported target collides with another source owner by case-folded name"
                    .into(),
            )
        } else if !valid_archive_declaration(archive) {
            Some("archive declaration has an unsafe owner, source path, output, or line".into())
        } else {
            None
        };
        if let Some(reason) = failure {
            rejections.push(rejection(archive, reason));
            continue;
        }

        let Some(command) = commands.get(&key) else {
            rejections.push(rejection(
                archive,
                "archive has no explicit source command-role proof",
            ));
            continue;
        };
        if command.flags.len() != 1 || command.flags[0] != "cr" {
            rejections.push(rejection(
                archive,
                "archive command flags differ from the admitted source `AR cr` role",
            ));
            continue;
        }

        let binding = match &archive.members {
            ArchiveMembers::Exact(members) => bind_exact_members(archive, members, compile_groups),
            ArchiveMembers::ProducerGlob { root, pattern } => {
                let Some(allowed) = reachable_hidd_owners.get(&key) else {
                    rejections.push(rejection(
                        archive,
                        "producer glob has no explicit reachable HIDD-owner set",
                    ));
                    continue;
                };
                bind_hidd_glob(archive, root, pattern, allowed, compile_groups)
            }
        };
        match binding {
            Ok((members, compile_groups, producer_owners)) => bound.push(BoundSourceArchive {
                declaration: archive.clone(),
                command: command.clone(),
                members,
                compile_groups,
                producer_owners,
            }),
            Err(reason) => rejections.push(rejection(archive, reason)),
        }
    }
    SourceArchiveBindingResult {
        archives: bound,
        qualified_hidd_groups,
        rejections,
    }
}

fn qualify_hidd_groups(
    groups: &[SourceCompileGroupDecl],
) -> (
    Vec<SourceCompileGroupDecl>,
    Vec<SourceArchiveBindingRejection>,
) {
    let hidd_indices = groups
        .iter()
        .enumerate()
        .filter_map(|(index, group)| group.parent.is_some().then_some(index))
        .collect::<Vec<_>>();
    let mut owners = BTreeMap::<String, Vec<usize>>::new();
    let mut outputs = BTreeMap::<String, Vec<usize>>::new();
    let mut unparented_rejections = Vec::new();
    for group in groups
        .iter()
        .filter(|group| group.parent.is_none() && group.archive_output.is_none())
    {
        unparented_rejections.push(group_rejection(
            group,
            "compile group has neither an archive association nor an explicit parent target",
        ));
    }
    for index in &hidd_indices {
        let group = &groups[*index];
        owners.entry(group.owner.clone()).or_default().push(*index);
    }
    for (index, group) in groups.iter().enumerate() {
        for object in &group.objects {
            outputs
                .entry(object.output.to_ascii_lowercase())
                .or_default()
                .push(index);
        }
    }

    let mut qualified = Vec::new();
    let mut rejections = unparented_rejections;
    for index in hidd_indices {
        let group = &groups[index];
        let reason = validate_hidd_group(group)
            .err()
            .or_else(|| {
                (owners.get(&group.owner).map_or(0, Vec::len) != 1)
                    .then(|| "HIDD owner has duplicate projected compile groups".into())
            })
            .or_else(|| {
                group.objects.first().and_then(|object| {
                    (outputs
                        .get(&object.output.to_ascii_lowercase())
                        .map_or(0, Vec::len)
                        != 1)
                        .then(|| {
                            "HIDD object output collides with another projected group by case-folded path".into()
                        })
                })
            });
        if let Some(reason) = reason {
            rejections.push(group_rejection(group, reason));
        } else {
            qualified.push(group.clone());
        }
    }
    (qualified, rejections)
}

type BoundArchiveMembers = (Vec<String>, Vec<SourceCompileGroupDecl>, BTreeSet<String>);

fn bind_exact_members(
    archive: &SourceArchiveDecl,
    members: &[String],
    groups: &[SourceCompileGroupDecl],
) -> Result<BoundArchiveMembers, String> {
    if members.is_empty() || members.len() > MAX_MEMBERS {
        return Err("exact archive member list is empty or exceeds the bounded limit".into());
    }

    let mut exact_members = BTreeSet::new();
    let mut folded_members = BTreeSet::new();
    let mut folded_basenames = BTreeSet::new();
    for member in members {
        if !safe_object_output(member) {
            return Err(format!(
                "archive member `{member}` is not a safe generated object path"
            ));
        }
        if !exact_members.insert(member.as_str())
            || !folded_members.insert(member.to_ascii_lowercase())
        {
            return Err(
                "archive members have duplicate or case-fold-colliding output paths".into(),
            );
        }
        let basename = object_basename(member).expect("safe object output has a basename");
        if !folded_basenames.insert(basename.to_ascii_lowercase()) {
            return Err("archive members have duplicate case-folded basenames".into());
        }
    }

    let mut candidate_indices = BTreeSet::new();
    for (index, group) in groups.iter().enumerate() {
        let Some(projected_archive) = group.archive_output.as_deref() else {
            continue;
        };
        if projected_archive.eq_ignore_ascii_case(&archive.output)
            && projected_archive != archive.output.as_str()
        {
            return Err("compile projection uses a case-fold alias of the archive output".into());
        }
        if projected_archive != archive.output.as_str() {
            continue;
        }
        if group.file != archive.file
            || group.owner != archive.owner
            || group.parent.is_some()
            || group.objects.is_empty()
        {
            return Err(format!(
                "compile group at {}:{} is foreign to this ordinary archive",
                group.file, group.line
            ));
        }
        candidate_indices.insert(index);
    }

    let expected_folded = members
        .iter()
        .map(|member| member.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let mut seen_outputs = BTreeSet::new();
    for index in &candidate_indices {
        let group = &groups[*index];
        for object in &group.objects {
            validate_compile_object(group, object)?;
            let folded = object.output.to_ascii_lowercase();
            if !expected_folded.contains(&folded) {
                return Err(format!(
                    "compile group at {}:{} produces foreign object `{}`",
                    group.file, group.line, object.output
                ));
            }
            if !seen_outputs.insert(folded) {
                return Err(format!(
                    "compile groups produce archive member `{}` more than once",
                    object.output
                ));
            }
        }
    }

    for member in members {
        let folded = member.to_ascii_lowercase();
        let occurrences = groups
            .iter()
            .enumerate()
            .flat_map(|(group_index, group)| {
                let folded = folded.as_str();
                group
                    .objects
                    .iter()
                    .filter(move |object| object.output.to_ascii_lowercase() == folded)
                    .map(move |object| (group_index, object))
            })
            .collect::<Vec<_>>();
        if occurrences.len() != 1 {
            return Err(format!(
                "archive member `{member}` has {} compile producers; exactly one is required",
                occurrences.len()
            ));
        }
        let (group_index, object) = occurrences[0];
        let group = &groups[group_index];
        if object.output.as_str() != member.as_str()
            || group.archive_output.as_deref() != Some(archive.output.as_str())
            || group.file != archive.file
            || group.owner != archive.owner
            || group.parent.is_some()
            || !candidate_indices.contains(&group_index)
        {
            return Err(format!(
                "archive member `{member}` is covered by a foreign or case-fold-alias compile group"
            ));
        }
        validate_compile_object(group, object)?;
    }
    if seen_outputs.len() != members.len() {
        return Err(
            "ordinary compile groups do not completely cover the exact archive members".into(),
        );
    }

    let selected_groups = candidate_indices
        .into_iter()
        .map(|index| groups[index].clone())
        .collect();
    Ok((members.to_vec(), selected_groups, BTreeSet::new()))
}

fn bind_hidd_glob(
    archive: &SourceArchiveDecl,
    root: &str,
    pattern: &str,
    allowed_edges: &BTreeSet<(String, String)>,
    groups: &[SourceCompileGroupDecl],
) -> Result<BoundArchiveMembers, String> {
    if !safe_generated_directory(root) || pattern != "*.o" {
        return Err("producer glob is not one safe generated-root `*.o` pattern".into());
    }
    if allowed_edges.is_empty() {
        return Err("producer glob has an empty reachable HIDD-owner set".into());
    }

    let parent = archive.owner.as_str();
    let allowed_owners = allowed_edges
        .iter()
        .map(|(_, owner)| owner.clone())
        .collect::<BTreeSet<_>>();
    let allowed_parents = allowed_edges
        .iter()
        .map(|(parent, _)| parent.clone())
        .chain([parent.to_owned()])
        .collect::<BTreeSet<_>>();
    let mut by_owner = BTreeMap::<String, Vec<usize>>::new();
    for (index, group) in groups.iter().enumerate() {
        if group
            .parent
            .as_ref()
            .is_some_and(|parent| allowed_parents.contains(parent))
            || allowed_owners.contains(&group.owner)
        {
            by_owner.entry(group.owner.clone()).or_default().push(index);
        }
    }

    // A source-declared child under this parent omitted by the graph reachability
    // evidence makes the known producer set incomplete, so do not shrink it.
    for group in groups.iter().filter(|group| {
        group
            .parent
            .as_ref()
            .is_some_and(|parent| allowed_parents.contains(parent))
    }) {
        if !allowed_owners.contains(&group.owner) {
            return Err(format!(
                "HIDD child `{}` names this archive parent but is absent from its reachable-owner set",
                group.owner
            ));
        }
    }

    let mut selected = BTreeMap::<String, (usize, String)>::new();
    for owner in &allowed_owners {
        if !safe_owner(owner) {
            return Err(format!("reachable HIDD owner `{owner}` is unsafe"));
        }
        let Some(indices) = by_owner.get(owner) else {
            return Err(format!(
                "reachable HIDD owner `{owner}` has no typed compile group"
            ));
        };
        if indices.len() != 1 {
            return Err(format!(
                "reachable HIDD owner `{owner}` has {} compile groups; exactly one is required",
                indices.len()
            ));
        }
        let index = indices[0];
        let group = &groups[index];
        if group.owner.as_str() != owner.as_str()
            || !group
                .parent
                .as_ref()
                .is_some_and(|parent| allowed_edges.contains(&(parent.clone(), owner.clone())))
            || group.archive_output.is_some()
            || !valid_source_file(&group.file)
            || group.line == 0
            || group.objects.len() != 1
        {
            return Err(format!(
                "HIDD group for `{owner}` does not have the exact required parent and singleton output"
            ));
        }
        let object = &group.objects[0];
        validate_compile_object(group, object)?;
        let Some(basename) = glob_basename(&object.output, root) else {
            return Err(format!(
                "reachable HIDD object `{}` is outside this producer glob",
                object.output
            ));
        };
        let Some(stem) = basename.strip_suffix(".o") else {
            return Err(format!(
                "reachable HIDD object `{}` does not match `*.o`",
                object.output
            ));
        };
        if !safe_component(stem) {
            return Err(format!("HIDD object basename `{basename}` is unsafe"));
        }
        let folded = object.output.to_ascii_lowercase();
        if selected
            .insert(folded, (index, object.output.clone()))
            .is_some()
        {
            return Err("reachable HIDD outputs collide by case-folded path".into());
        }
    }

    for (index, group) in groups.iter().enumerate() {
        for object in &group.objects {
            if !could_match_glob(&object.output, root) {
                continue;
            }
            let folded = object.output.to_ascii_lowercase();
            let Some((selected_index, selected_output)) = selected.get(&folded) else {
                return Err(format!(
                    "foreign typed object `{}` would be included by the archive producer glob",
                    object.output
                ));
            };
            if *selected_index != index || selected_output != &object.output {
                return Err(format!(
                    "case-fold-colliding typed object `{}` would be included by the archive producer glob",
                    object.output
                ));
            }
        }
    }

    let ordered = selected
        .values()
        .map(|(index, output)| (output.clone(), *index))
        .collect::<BTreeMap<_, _>>();
    if ordered.is_empty() {
        return Err("producer glob resolves to no typed HIDD object producers".into());
    }
    let members = ordered.keys().cloned().collect::<Vec<_>>();
    if members.len() > MAX_MEMBERS {
        return Err("producer glob resolves beyond the bounded HIDD producer limit".into());
    }
    let selected_groups = ordered
        .values()
        .map(|index| groups[*index].clone())
        .collect();
    Ok((members, selected_groups, allowed_owners))
}

fn validate_compile_object(
    group: &SourceCompileGroupDecl,
    object: &LiteralObjectDecl,
) -> Result<(), String> {
    if !valid_source_file(&group.file)
        || group.line == 0
        || object.line == 0
        || object.language != "C"
        || !safe_source_path(&object.source)
        || !safe_object_output(&object.output)
    {
        return Err(format!(
            "compile object in {}:{} has an unsafe source identity, line, language, or output",
            group.file, group.line
        ));
    }
    Ok(())
}

fn validate_hidd_group(group: &SourceCompileGroupDecl) -> Result<(), String> {
    let Some(parent) = group.parent.as_deref() else {
        return Err("HIDD compile projection has no explicit parent target".into());
    };
    if !safe_owner(&group.owner)
        || !safe_owner(parent)
        || !valid_source_file(&group.file)
        || group.line == 0
        || group.archive_output.is_some()
        || group.objects.len() != 1
    {
        return Err(format!(
            "HIDD group `{}` does not have one complete projected object and explicit safe parent",
            group.owner
        ));
    }
    validate_compile_object(group, &group.objects[0])
}

fn valid_archive_declaration(archive: &SourceArchiveDecl) -> bool {
    safe_owner(&archive.owner)
        && valid_source_file(&archive.file)
        && archive.line != 0
        && archive.owner_line != 0
        && safe_archive_output(&archive.output)
}

fn safe_archive_output(output: &str) -> bool {
    let Some(tail) = output.strip_prefix("${AROS_BUILD_DIR}/") else {
        return false;
    };
    let Some(filename) = tail.rsplit('/').next() else {
        return false;
    };
    let Some(name) = filename
        .strip_prefix("lib")
        .and_then(|name| name.strip_suffix(".a"))
    else {
        return false;
    };
    safe_component(name) && safe_path_tail(tail)
}

fn safe_object_output(output: &str) -> bool {
    let Some(tail) = output.strip_prefix(&format!("{BUILD_GEN_ROOT}/")) else {
        return false;
    };
    let Some(filename) = tail.rsplit('/').next() else {
        return false;
    };
    let Some(stem) = filename.strip_suffix(".o") else {
        return false;
    };
    safe_component(stem) && safe_path_tail(tail)
}

fn safe_source_path(source: &str) -> bool {
    let Some(tail) = source.strip_prefix("${AROS_SOURCE_DIR}/") else {
        return false;
    };
    safe_path_tail(tail)
}

fn safe_generated_directory(path: &str) -> bool {
    path == BUILD_GEN_ROOT
        || path
            .strip_prefix(&format!("{BUILD_GEN_ROOT}/"))
            .is_some_and(safe_path_tail)
}

fn safe_path_tail(tail: &str) -> bool {
    !tail.is_empty() && tail.split('/').all(safe_component)
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
}

fn safe_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= 160
        && owner.as_bytes()[0].is_ascii_alphanumeric()
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_source_file(file: &str) -> bool {
    !file.is_empty()
        && !file.starts_with('/')
        && !file.contains(['\\', ':', '\n', '\r', '\0'])
        && file
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn object_basename(output: &str) -> Option<&str> {
    output.rsplit('/').next()
}

fn glob_basename<'a>(output: &'a str, root: &str) -> Option<&'a str> {
    let tail = output.strip_prefix(root)?.strip_prefix('/')?;
    (!tail.contains('/')).then_some(tail)
}

fn could_match_glob(output: &str, root: &str) -> bool {
    glob_basename(output, root).is_some_and(|basename| {
        Path::new(basename)
            .extension()
            .is_some_and(|ext| ext == "o")
    })
}

fn rejection(
    archive: &SourceArchiveDecl,
    reason: impl Into<String>,
) -> SourceArchiveBindingRejection {
    SourceArchiveBindingRejection {
        owner: archive.owner.clone(),
        file: archive.file.clone(),
        line: archive.line,
        reason: reason.into(),
    }
}

fn group_rejection(
    group: &SourceCompileGroupDecl,
    reason: impl Into<String>,
) -> SourceArchiveBindingRejection {
    SourceArchiveBindingRejection {
        owner: group.owner.clone(),
        file: group.file.clone(),
        line: group.line,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "compiler/libexample/mmakefile.src";
    const OWNER: &str = "compiler-libexample";
    const OUTPUT: &str = "${AROS_BUILD_DIR}/SYS/Developer/lib/libexample.a";
    const OBJ_A: &str = "${AROS_BUILD_DIR}/gen/compiler/libexample/a.o";
    const OBJ_B: &str = "${AROS_BUILD_DIR}/gen/compiler/libexample/b.o";

    fn archive(members: ArchiveMembers) -> SourceArchiveDecl {
        SourceArchiveDecl {
            owner: OWNER.into(),
            file: FILE.into(),
            line: 20,
            owner_line: 10,
            output: OUTPUT.into(),
            members,
        }
    }

    fn command() -> SourceArchiveCommand {
        SourceArchiveCommand {
            flags: vec!["cr".into()],
        }
    }

    fn object(output: &str) -> LiteralObjectDecl {
        LiteralObjectDecl {
            source: "${AROS_SOURCE_DIR}/compiler/libexample/source.c".into(),
            output: output.into(),
            language: "C".into(),
            arguments: Vec::new(),
            line: 15,
        }
    }

    fn ordinary_group(outputs: &[&str]) -> SourceCompileGroupDecl {
        SourceCompileGroupDecl {
            owner: OWNER.into(),
            parent: None,
            file: FILE.into(),
            line: 15,
            archive_output: Some(OUTPUT.into()),
            objects: outputs.iter().map(|output| object(output)).collect(),
        }
    }

    fn command_map() -> BTreeMap<SourceArchiveBindingKey, SourceArchiveCommand> {
        BTreeMap::from([((FILE.into(), OWNER.into()), command())])
    }

    fn bind(
        archive: SourceArchiveDecl,
        groups: &[SourceCompileGroupDecl],
        reachable: &BTreeMap<SourceArchiveBindingKey, BTreeSet<(String, String)>>,
        commands: &BTreeMap<SourceArchiveBindingKey, SourceArchiveCommand>,
    ) -> (Vec<BoundSourceArchive>, Vec<SourceArchiveBindingRejection>) {
        let result = bind_source_archives(&[archive], groups, commands, reachable);
        (result.archives, result.rejections)
    }

    #[test]
    fn exact_archive_preserves_order_after_complete_unique_compile_coverage() {
        let archive = archive(ArchiveMembers::Exact(vec![OBJ_B.into(), OBJ_A.into()]));
        let groups = [ordinary_group(&[OBJ_A]), ordinary_group(&[OBJ_B])];
        let (bound, rejected) = bind(archive, &groups, &BTreeMap::new(), &command_map());
        assert!(rejected.is_empty());
        assert_eq!(bound.len(), 1);
        assert_eq!(bound[0].members, [OBJ_B, OBJ_A]);
        assert_eq!(bound[0].compile_groups.len(), 2);
        assert_eq!(bound[0].command.flags, ["cr"]);
        assert_eq!(bound[0].archive_basename(), "libexample.a");
        assert_eq!(bound[0].library_name(), "example");
        assert_eq!(bound[0].provider_target(), "compiler-libexample-archive");
    }

    #[test]
    fn partial_and_foreign_compile_groups_do_not_bind_exact_archives() {
        let archive = archive(ArchiveMembers::Exact(vec![OBJ_A.into(), OBJ_B.into()]));
        let (bound, rejected) = bind(
            archive.clone(),
            &[ordinary_group(&[OBJ_A])],
            &BTreeMap::new(),
            &command_map(),
        );
        assert!(bound.is_empty());
        assert!(rejected[0].reason.contains("exactly one"));

        let mut foreign = ordinary_group(&[OBJ_A, OBJ_B]);
        foreign.owner = "some-other-library".into();
        let (bound, rejected) = bind(archive, &[foreign], &BTreeMap::new(), &command_map());
        assert!(bound.is_empty());
        assert!(rejected[0].reason.contains("foreign"));
    }

    #[test]
    fn duplicate_member_basenames_and_casefold_outputs_are_refused() {
        let duplicate_basenames = archive(ArchiveMembers::Exact(vec![
            "${AROS_BUILD_DIR}/gen/one/shared.o".into(),
            "${AROS_BUILD_DIR}/gen/two/shared.o".into(),
        ]));
        let (bound, rejected) = bind(duplicate_basenames, &[], &BTreeMap::new(), &command_map());
        assert!(bound.is_empty());
        assert!(rejected[0].reason.contains("basenames"));

        let casefold_outputs = archive(ArchiveMembers::Exact(vec![
            "${AROS_BUILD_DIR}/gen/compiler/libexample/Alpha.o".into(),
            "${AROS_BUILD_DIR}/gen/compiler/libexample/alpha.o".into(),
        ]));
        let (bound, rejected) = bind(casefold_outputs, &[], &BTreeMap::new(), &command_map());
        assert!(bound.is_empty());
        assert!(rejected[0].reason.contains("case-fold"));

        let empty = archive(ArchiveMembers::Exact(Vec::new()));
        let (bound, rejected) = bind(empty, &[], &BTreeMap::new(), &command_map());
        assert!(bound.is_empty());
        assert!(rejected[0].reason.contains("empty"));
    }

    #[test]
    fn casefold_compile_output_collision_refuses_exact_archive_binding() {
        let archive = archive(ArchiveMembers::Exact(vec![OBJ_A.into()]));
        let aliased_output = OBJ_A.replace("a.o", "A.o");
        let mut foreign = ordinary_group(&[&aliased_output]);
        foreign.archive_output = Some("${AROS_BUILD_DIR}/SYS/Developer/lib/libother.a".into());
        foreign.owner = "compiler-libother".into();
        let (bound, rejected) = bind(
            archive,
            &[ordinary_group(&[OBJ_A]), foreign],
            &BTreeMap::new(),
            &command_map(),
        );
        assert!(bound.is_empty());
        assert!(rejected[0].reason.contains("exactly one"));
    }

    fn hidd_group(owner: &str, parent: &str, output: &str, file: &str) -> SourceCompileGroupDecl {
        SourceCompileGroupDecl {
            owner: owner.into(),
            parent: Some(parent.into()),
            file: file.into(),
            line: 4,
            archive_output: None,
            objects: vec![object(output)],
        }
    }

    fn hidd_archive() -> SourceArchiveDecl {
        SourceArchiveDecl {
            owner: "linklibs-hidd-stubs".into(),
            file: "compiler/libhiddstubs/mmakefile.src".into(),
            line: 40,
            owner_line: 35,
            output: "${AROS_BUILD_DIR}/SYS/Developer/lib/libhiddstubs.a".into(),
            members: ArchiveMembers::ProducerGlob {
                root: "${AROS_BUILD_DIR}/gen/lib/hidd".into(),
                pattern: "*.o".into(),
            },
        }
    }

    fn hidd_command_map() -> BTreeMap<SourceArchiveBindingKey, SourceArchiveCommand> {
        BTreeMap::from([(
            (
                "compiler/libhiddstubs/mmakefile.src".into(),
                "linklibs-hidd-stubs".into(),
            ),
            command(),
        )])
    }

    fn hidd_edges(owners: &[&str]) -> BTreeSet<(String, String)> {
        owners
            .iter()
            .map(|owner| ("linklibs-hidd-stubs".into(), (*owner).into()))
            .collect()
    }

    #[test]
    fn producer_glob_uses_only_reachable_typed_hidd_groups_in_make_order() {
        let archive = hidd_archive();
        let archive_key = (archive.file.clone(), archive.owner.clone());
        let groups = [
            hidd_group(
                "hidd-zeta-stubs",
                "linklibs-hidd-stubs",
                "${AROS_BUILD_DIR}/gen/lib/hidd/zeta.o",
                "compiler/hidd/zeta/mmakefile.src",
            ),
            hidd_group(
                "hidd-alpha-stubs",
                "linklibs-hidd-stubs",
                "${AROS_BUILD_DIR}/gen/lib/hidd/alpha.o",
                "compiler/hidd/alpha/mmakefile.src",
            ),
            hidd_group(
                "hidd-unrelated-stubs",
                "other-hidd-parent",
                "${AROS_BUILD_DIR}/gen/lib/other/unrelated.o",
                "compiler/hidd/unrelated/mmakefile.src",
            ),
        ];
        let reachable = BTreeMap::from([(
            archive_key,
            hidd_edges(&["hidd-alpha-stubs", "hidd-zeta-stubs"]),
        )]);
        let result = bind_source_archives(&[archive], &groups, &hidd_command_map(), &reachable);
        assert!(result.rejections.is_empty());
        assert_eq!(result.archives.len(), 1);
        assert_eq!(result.qualified_hidd_groups.len(), 3);
        let bound = &result.archives;
        assert_eq!(
            bound[0].members,
            [
                "${AROS_BUILD_DIR}/gen/lib/hidd/alpha.o",
                "${AROS_BUILD_DIR}/gen/lib/hidd/zeta.o"
            ]
        );
        assert_eq!(
            bound[0]
                .compile_groups
                .iter()
                .map(|group| group.owner.as_str())
                .collect::<Vec<_>>(),
            ["hidd-alpha-stubs", "hidd-zeta-stubs"]
        );
        assert_eq!(
            bound[0].producer_owners,
            BTreeSet::from(["hidd-alpha-stubs".into(), "hidd-zeta-stubs".into()])
        );
    }

    #[test]
    fn unrelated_valid_hidd_groups_are_qualified_without_being_archive_members() {
        let archive = hidd_archive();
        let archive_key = (archive.file.clone(), archive.owner.clone());
        let groups = [
            hidd_group(
                "hidd-alpha-stubs",
                "linklibs-hidd-stubs",
                "${AROS_BUILD_DIR}/gen/lib/hidd/alpha.o",
                "compiler/hidd/alpha/mmakefile.src",
            ),
            hidd_group(
                "hidd-other-stubs",
                "other-hidd-parent",
                "${AROS_BUILD_DIR}/gen/lib/other/other.o",
                "compiler/hidd/other/mmakefile.src",
            ),
        ];
        let reachable = BTreeMap::from([(archive_key, hidd_edges(&["hidd-alpha-stubs"]))]);
        let result = bind_source_archives(&[archive], &groups, &hidd_command_map(), &reachable);
        assert!(result.rejections.is_empty());
        assert_eq!(result.qualified_hidd_groups.len(), 2);
        assert_eq!(result.archives[0].members.len(), 1);
        assert_eq!(
            result.archives[0].producer_owners,
            BTreeSet::from(["hidd-alpha-stubs".into()])
        );
    }

    #[test]
    fn hidd_groups_are_returned_when_no_archive_selects_them() {
        let group = hidd_group(
            "hidd-alpha-stubs",
            "linklibs-hidd-stubs",
            "${AROS_BUILD_DIR}/gen/lib/hidd/alpha.o",
            "compiler/hidd/alpha/mmakefile.src",
        );
        let result = bind_source_archives(
            &[],
            std::slice::from_ref(&group),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(result.archives.is_empty());
        assert!(result.rejections.is_empty());
        assert_eq!(result.qualified_hidd_groups, [group]);
    }

    #[test]
    fn hidd_projection_requires_explicit_parent_and_singleton_inventory() {
        let mut malformed = hidd_group(
            "hidd-alpha-stubs",
            "linklibs-hidd-stubs",
            "${AROS_BUILD_DIR}/gen/lib/hidd/alpha.o",
            "compiler/hidd/alpha/mmakefile.src",
        );
        malformed
            .objects
            .push(object("${AROS_BUILD_DIR}/gen/lib/hidd/beta.o"));
        let result = bind_source_archives(&[], &[malformed], &BTreeMap::new(), &BTreeMap::new());
        assert!(result.qualified_hidd_groups.is_empty());
        assert!(result.rejections[0]
            .reason
            .contains("one complete projected object"));

        let mut missing_parent = hidd_group(
            "hidd-no-parent",
            "temporary-parent",
            "${AROS_BUILD_DIR}/gen/lib/hidd/no-parent.o",
            "compiler/hidd/no-parent/mmakefile.src",
        );
        missing_parent.parent = None;
        let result =
            bind_source_archives(&[], &[missing_parent], &BTreeMap::new(), &BTreeMap::new());
        assert!(result.qualified_hidd_groups.is_empty());
        assert!(result.rejections[0].reason.contains("explicit parent"));
    }

    #[test]
    fn producer_glob_refuses_missing_or_incomplete_parent_proof() {
        let archive = hidd_archive();
        let archive_key = (archive.file.clone(), archive.owner.clone());
        let group = hidd_group(
            "hidd-alpha-stubs",
            "linklibs-hidd-stubs",
            "${AROS_BUILD_DIR}/gen/lib/hidd/alpha.o",
            "compiler/hidd/alpha/mmakefile.src",
        );
        let result = bind_source_archives(
            std::slice::from_ref(&archive),
            std::slice::from_ref(&group),
            &hidd_command_map(),
            &BTreeMap::new(),
        );
        assert!(result.archives.is_empty());
        assert!(result.rejections[0]
            .reason
            .contains("reachable HIDD-owner set"));

        let reachable = BTreeMap::from([(
            archive_key.clone(),
            hidd_edges(&["hidd-alpha-stubs", "hidd-missing-stubs"]),
        )]);
        let result = bind_source_archives(&[archive], &[group], &hidd_command_map(), &reachable);
        assert!(result.archives.is_empty());
        assert!(result.rejections[0]
            .reason
            .contains("no typed compile group"));

        let empty_reachable = BTreeMap::from([(archive_key, BTreeSet::new())]);
        let result = bind_source_archives(
            &[hidd_archive()],
            &[hidd_group(
                "hidd-alpha-stubs",
                "linklibs-hidd-stubs",
                "${AROS_BUILD_DIR}/gen/lib/hidd/alpha.o",
                "compiler/hidd/alpha/mmakefile.src",
            )],
            &hidd_command_map(),
            &empty_reachable,
        );
        assert!(result.archives.is_empty());
        assert!(result.rejections[0].reason.contains("empty reachable"));
    }

    #[test]
    fn producer_glob_refuses_unsafe_root_or_pattern() {
        let mut archive = hidd_archive();
        archive.members = ArchiveMembers::ProducerGlob {
            root: "${AROS_BUILD_DIR}/gen/lib/hidd/../other".into(),
            pattern: "*.o".into(),
        };
        let archive_key = (archive.file.clone(), archive.owner.clone());
        let reachable = BTreeMap::from([(archive_key, hidd_edges(&["hidd-alpha-stubs"]))]);
        let groups = [hidd_group(
            "hidd-alpha-stubs",
            "linklibs-hidd-stubs",
            "${AROS_BUILD_DIR}/gen/lib/hidd/alpha.o",
            "compiler/hidd/alpha/mmakefile.src",
        )];
        let result = bind_source_archives(&[archive], &groups, &hidd_command_map(), &reachable);
        assert!(result.archives.is_empty());
        assert!(result.rejections[0].reason.contains("safe generated-root"));
    }

    #[test]
    fn producer_glob_rejects_unlisted_siblings_that_the_pattern_would_capture() {
        let archive = hidd_archive();
        let archive_key = (archive.file.clone(), archive.owner.clone());
        let groups = [
            hidd_group(
                "hidd-alpha-stubs",
                "linklibs-hidd-stubs",
                "${AROS_BUILD_DIR}/gen/lib/hidd/alpha.o",
                "compiler/hidd/alpha/mmakefile.src",
            ),
            hidd_group(
                "hidd-foreign-stubs",
                "other-hidd-parent",
                "${AROS_BUILD_DIR}/gen/lib/hidd/foreign.o",
                "compiler/hidd/foreign/mmakefile.src",
            ),
        ];
        let reachable = BTreeMap::from([(archive_key, hidd_edges(&["hidd-alpha-stubs"]))]);
        let (bound, rejected) = bind(archive, &groups, &reachable, &hidd_command_map());
        assert!(bound.is_empty());
        assert!(rejected[0].reason.contains("foreign typed object"));

        let omitted_child = hidd_group(
            "hidd-omitted-stubs",
            "linklibs-hidd-stubs",
            "${AROS_BUILD_DIR}/gen/lib/hidd/omitted.o",
            "compiler/hidd/omitted/mmakefile.src",
        );
        let groups = [groups[0].clone(), omitted_child];
        let (bound, rejected) = bind(hidd_archive(), &groups, &reachable, &hidd_command_map());
        assert!(bound.is_empty());
        assert!(rejected[0]
            .reason
            .contains("absent from its reachable-owner set"));
    }

    #[test]
    fn missing_command_role_proof_refuses_otherwise_complete_archive() {
        let archive = archive(ArchiveMembers::Exact(vec![OBJ_A.into()]));
        let groups = [ordinary_group(&[OBJ_A])];
        let (bound, rejected) = bind(archive, &groups, &BTreeMap::new(), &BTreeMap::new());
        assert!(bound.is_empty());
        assert!(rejected[0].reason.contains("command-role proof"));
    }
}
