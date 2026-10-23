//! Data binding for one original missing include's GENMODULE recipe.

use super::super::DeclaredIncludedRecipes;
use crate::{
    genmodule_writefiles_rules::validate_native_makefile_tuple,
    make_rule_events::RuleInstance,
    make_rule_graph::traversal::TargetVisit,
    make_shell_data::{
        artifact_binding::{RecipeArtifactDirection, RecipeArtifactRequest, RecipeArtifactSlot},
        LiteralRecipeCommand, LiteralRecipeStatement,
    },
    make_vars::{MakeFileEventKind, MakeRecipeProjection},
};
use aros_common::{
    captured_programs::CapturedProgramSet,
    make_input_snapshot::MakeInputReader as SnapshotMakeInputReader, Sha256Digest,
};
use std::path::{Component, Path, PathBuf};

/// Source-derived include producer data; it does not claim the recipes ran.
///
/// Recipe indices address the retained `DeclaredIncludedRecipes` result so a
/// runtime can derive each recipe's serial environment from its original
/// expression and its independently retained Make invocation.
pub struct NativeGenmoduleIncludePlan {
    include_path: PathBuf,
    directory: PathBuf,
    arguments: Vec<String>,
    configurations: Vec<(PathBuf, Sha256Digest)>,
    generator_recipe: usize,
    directory_recipe: usize,
}

struct SelectedInclude<'a> {
    visit: usize,
    target: &'a str,
    include_path: PathBuf,
    directory_target: &'a str,
    directory: PathBuf,
    rule: &'a RuleInstance,
}

impl NativeGenmoduleIncludePlan {
    /// The absolute path selected by the exact missing include event.
    #[must_use]
    pub fn include_path(&self) -> &Path {
        &self.include_path
    }

    /// The include target's absolute parent directory.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Literal argv for the bound `source-genmodule` command, including argv[0].
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// Base then optional override configuration, with their input digests.
    #[must_use]
    pub fn configurations(&self) -> &[(PathBuf, Sha256Digest)] {
        &self.configurations
    }

    /// Digest of the bound base Source configuration input.
    #[must_use]
    pub fn base_configuration_digest(&self) -> &Sha256Digest {
        &self.configurations[0].1
    }

    /// Digest of the optional bound Source override configuration input.
    #[must_use]
    pub fn override_configuration_digest(&self) -> Option<&Sha256Digest> {
        self.configurations.get(1).map(|(_, digest)| digest)
    }

    /// Index of the retained include-producing recipe expression.
    #[must_use]
    pub const fn generator_recipe(&self) -> usize {
        self.generator_recipe
    }

    /// Index of the retained parent-directory recipe expression.
    #[must_use]
    pub const fn directory_recipe(&self) -> usize {
        self.directory_recipe
    }
}

pub(super) fn prepare_genmodule_include_in_reader(
    included: &DeclaredIncludedRecipes,
    source_root: &Path,
    programs: &CapturedProgramSet,
    reader: &mut SnapshotMakeInputReader<'_>,
) -> Result<NativeGenmoduleIncludePlan, String> {
    if !included
        .declared
        .source
        .snapshot
        .same_read_context(reader.snapshot())
    {
        return Err("declared include input reader context differs from original match".into());
    }
    if source_root != included.source_root.as_path() {
        return Err("native Source GENMODULE root differs from original recipe context".into());
    }

    let selected_include = selected_include(included)?;
    if selected_include.rule.order_only.len() != 1
        || selected_include.rule.order_only[0].as_str() != selected_include.directory_target
    {
        return Err("native GENMODULE include must order only its parent directory".into());
    }

    let generator_recipe = unique_recipe_for_visit(included, selected_include.visit)?;
    let generator = included
        .recipes()
        .get(generator_recipe)
        .ok_or("native GENMODULE recipe is absent")?;
    let command = exact_command(generator.expression(), "native GENMODULE include recipe")?;
    let configuration_words =
        validate_native_makefile_tuple(&command, source_root, &selected_include.include_path)?;
    if !matches!(configuration_words.len(), 1 | 2) {
        return Err("native GENMODULE configuration field count is unsupported".into());
    }

    let requests = configuration_words
        .iter()
        .map(|index| {
            let value = command
                .words()
                .get(*index)
                .ok_or("native GENMODULE configuration word is absent")?;
            Ok(RecipeArtifactRequest {
                recipe: 0,
                command: 0,
                slot: RecipeArtifactSlot::Word {
                    index: *index,
                    range: 0..value.len(),
                },
                direction: RecipeArtifactDirection::Input,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let directory_recipe = validate_directory_recipe(
        included,
        selected_include.directory_target,
        &selected_include.directory,
    )?;
    let include_path_for_binding = selected_include.include_path.clone();
    let directory_for_binding = selected_include.directory.clone();
    let command_words = command.words().to_vec();
    included.with_recipe_input_bindings_in_reader(
        generator_recipe,
        programs,
        &requests,
        reader,
        |bindings, artifacts| {
            if bindings.commands().len() != 1 {
                return Err("native GENMODULE include has additional shell commands".into());
            }
            let selected = bindings.unique_role("source-genmodule")?;
            let program = selected
                .program()
                .ok_or("selected Source GENMODULE role is unresolved")?;
            if selected.command().words() != command_words.as_slice()
                || !selected.command().redirections().is_empty()
            {
                return Err(
                    "native GENMODULE binding differs from the selected literal command".into(),
                );
            }
            let genmodule_path = normalized_absolute_path(
                program.invocation_path(),
                "selected Source GENMODULE program",
            )?;
            let genmodule_text = path_text(&genmodule_path, "selected Source GENMODULE program")?;
            if artifacts.len() != configuration_words.len() {
                return Err("native GENMODULE configuration binding count differs".into());
            }

            let mut configurations = Vec::with_capacity(artifacts.len());
            let mut expected_prerequisites = Vec::<String>::with_capacity(artifacts.len() + 1);
            expected_prerequisites.push(genmodule_text.to_owned());
            for (artifact, request) in artifacts.iter().zip(&requests) {
                if artifact.request() != request {
                    return Err("native GENMODULE configuration binding order differs".into());
                }
                let path = normalized_absolute_path(
                    artifact.path(),
                    "bound Source GENMODULE configuration",
                )?;
                if !path.starts_with(source_root) {
                    return Err("bound Source GENMODULE configuration leaves Source root".into());
                }
                let digest = artifact
                    .input_sha256()
                    .cloned()
                    .ok_or("bound Source GENMODULE configuration has no input digest")?;
                expected_prerequisites
                    .push(path_text(&path, "bound Source GENMODULE configuration")?.to_owned());
                configurations.push((path, digest));
            }
            if selected_include.rule.prerequisites != expected_prerequisites {
                return Err(
                    "native GENMODULE include prerequisites differ from bound inputs".into(),
                );
            }

            reject_phony_selected_names(
                included,
                std::iter::once(selected_include.target)
                    .chain(std::iter::once(selected_include.directory_target))
                    .chain(expected_prerequisites.iter().map(String::as_str)),
            )?;

            Ok(NativeGenmoduleIncludePlan {
                include_path: include_path_for_binding,
                directory: directory_for_binding,
                arguments: selected.command().words().to_vec(),
                configurations,
                generator_recipe,
                directory_recipe,
            })
        },
    )
}

pub(super) fn selected_include_directory(
    included: &DeclaredIncludedRecipes,
) -> Result<PathBuf, String> {
    let selected = selected_include(included)?;
    if selected.rule.order_only.len() != 1
        || selected.rule.order_only[0].as_str() != selected.directory_target
    {
        return Err("native GENMODULE include must order only its parent directory".into());
    }
    validate_directory_recipe(included, selected.directory_target, &selected.directory)?;
    Ok(selected.directory)
}

fn selected_include(included: &DeclaredIncludedRecipes) -> Result<SelectedInclude<'_>, String> {
    let event = included.include_event();
    if !event.active {
        return Err("native GENMODULE include event is inactive".into());
    }
    let MakeFileEventKind::Include {
        requested,
        present: false,
        resolved_identity: None,
    } = &event.kind
    else {
        return Err("native GENMODULE event is not the exact missing include".into());
    };
    let requested_path = Path::new(requested);
    let directory_target = requested_path
        .parent()
        .and_then(Path::to_str)
        .filter(|value| !value.is_empty())
        .ok_or("native GENMODULE include path has no parent directory")?;
    let include_path = absolute_make_path(included, requested, "native GENMODULE include path")?;
    let directory = include_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or("native GENMODULE include path has no parent directory")?
        .to_path_buf();
    let relative = include_path
        .strip_prefix(&included.declared.source.build_root)
        .map_err(|_| "native GENMODULE include path is outside the original build root")?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("native GENMODULE include path is not a build-root child".into());
    }
    let goals = included.traversal().goals();
    if goals.len() != 1 {
        return Err("native GENMODULE include target selection is ambiguous".into());
    }
    let visit_index = goals[0];
    let visit = included
        .traversal()
        .visits()
        .get(visit_index)
        .ok_or("native GENMODULE include target visit is absent")?;
    if visit.parent().is_some() || visit.target() != requested {
        return Err("native GENMODULE include target differs from exact missing include".into());
    }
    if included
        .traversal()
        .visits()
        .iter()
        .filter(|candidate| candidate.target() == requested)
        .count()
        != 1
    {
        return Err("native GENMODULE include target is absent or ambiguous".into());
    }
    let rule = visit
        .instance()
        .ok_or("native GENMODULE include has no explicit rule")?;
    if rule.double_colon {
        return Err("native GENMODULE include uses a double-colon rule".into());
    }
    Ok(SelectedInclude {
        visit: visit_index,
        target: requested,
        include_path,
        directory_target,
        directory,
        rule,
    })
}

fn unique_recipe_for_visit(
    included: &DeclaredIncludedRecipes,
    visit: usize,
) -> Result<usize, String> {
    let mut matches = included
        .recipes()
        .iter()
        .enumerate()
        .filter(|(_, recipe)| recipe.visit() == visit);
    let (index, recipe) = matches
        .next()
        .ok_or("native GENMODULE target has no retained recipe")?;
    if matches.next().is_some() || recipe.recipe() != 0 {
        return Err("native GENMODULE target does not have exactly one recipe".into());
    }
    Ok(index)
}

fn unique_visit_for_target<'a>(
    included: &'a DeclaredIncludedRecipes,
    target: &str,
) -> Result<(usize, &'a TargetVisit, &'a RuleInstance), String> {
    let mut matches = included
        .traversal()
        .visits()
        .iter()
        .enumerate()
        .filter(|(_, visit)| visit.target() == target);
    let (index, visit) = matches
        .next()
        .ok_or("native GENMODULE directory target is absent")?;
    if matches.next().is_some() {
        return Err("native GENMODULE directory target is ambiguous".into());
    }
    let rule = visit
        .instance()
        .ok_or("native GENMODULE directory has no explicit rule")?;
    if rule.double_colon {
        return Err("native GENMODULE directory uses a double-colon rule".into());
    }
    Ok((index, visit, rule))
}

fn validate_directory_recipe(
    included: &DeclaredIncludedRecipes,
    directory_target: &str,
    directory: &Path,
) -> Result<usize, String> {
    let resolved_directory =
        absolute_make_path(included, directory_target, "native GENMODULE directory")?;
    if resolved_directory != directory {
        return Err("native GENMODULE directory target differs from include parent".into());
    }
    let (visit_index, _, rule) = unique_visit_for_target(included, directory_target)?;
    if !rule.prerequisites.is_empty() || !rule.order_only.is_empty() {
        return Err("native GENMODULE directory rule has prerequisites".into());
    }
    let recipe_index = unique_recipe_for_visit(included, visit_index)?;
    let recipe = included
        .recipes()
        .get(recipe_index)
        .ok_or("native GENMODULE directory recipe is absent")?;
    let command = exact_command(recipe.expression(), "native GENMODULE directory recipe")?;
    if !command
        .words()
        .iter()
        .map(String::as_str)
        .eq(["mkdir", "-p", directory_target])
    {
        return Err("native GENMODULE directory command differs from mkdir -p parent".into());
    }
    Ok(recipe_index)
}

fn exact_command(
    expression: &MakeRecipeProjection,
    what: &str,
) -> Result<LiteralRecipeCommand, String> {
    let shell = expression.parse_literal_shell()?;
    if shell.prefixes.ignore_errors || shell.prefixes.explicit_recursive {
        return Err(format!("{what} has unsupported Make recipe controls"));
    }
    let [LiteralRecipeStatement::Command(command)] = shell.statements.as_slice() else {
        return Err(format!("{what} is not exactly one simple command"));
    };
    if !command.redirections().is_empty() {
        return Err(format!("{what} has redirections"));
    }
    Ok(command.clone())
}

fn reject_phony_selected_names<'a>(
    included: &DeclaredIncludedRecipes,
    selected: impl Iterator<Item = &'a str>,
) -> Result<(), String> {
    let selected = selected
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    for event in &included.declared.file().events {
        if !event.active {
            continue;
        }
        if let MakeFileEventKind::PhonyTargets(names) = &event.kind {
            if names.iter().any(|name| selected.contains(name)) {
                return Err("native GENMODULE selected target or prerequisite is phony".into());
            }
        }
    }
    Ok(())
}

fn normalized_absolute_path(path: &Path, what: &str) -> Result<PathBuf, String> {
    path.to_str()
        .filter(|value| !value.contains('\0') && !value.chars().any(char::is_control))
        .ok_or_else(|| format!("{what} must be an absolute normalized UTF-8 path"))?;
    if !path.is_absolute() {
        return Err(format!("{what} must be an absolute normalized UTF-8 path"));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir | Component::Normal(_) => normalized.push(component.as_os_str()),
            Component::CurDir | Component::ParentDir => {
                return Err(format!("{what} must be an absolute normalized UTF-8 path"));
            }
        }
    }
    if normalized.as_os_str() != path.as_os_str() {
        return Err(format!("{what} must be an absolute normalized UTF-8 path"));
    }
    Ok(normalized)
}

pub(super) fn absolute_make_path(
    included: &DeclaredIncludedRecipes,
    value: &str,
    what: &str,
) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if path.is_absolute() {
        return normalized_absolute_path(path, what);
    }
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("{what} is not a normalized Make path"));
    }
    normalized_absolute_path(
        &included
            .declared
            .source
            .snapshot
            .working_directory()
            .join(path),
        what,
    )
}

fn path_text<'a>(path: &'a Path, what: &str) -> Result<&'a str, String> {
    path.to_str().ok_or_else(|| format!("{what} is not UTF-8"))
}
