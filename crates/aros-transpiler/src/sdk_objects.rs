//! Closed collection of finite Make aggregates containing standalone objects.
//!
//! Objects are compiled from local C/C++ sources and copied into the configured
//! Developer library. The collector reads the already joined source snapshot;
//! it never executes Make or inspects ambient generated objects.

use crate::dirs::DirVars;
use crate::make_expr::{
    evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeVariableGuard, MakeVariableLookup,
};
use crate::make_vars::{ConditionalTruth, VarScope};
use crate::parser::Invocation;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

/// One exact source-derived compile and staging pair for a local SDK object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkObjectDecl {
    /// C or C++ source file below the configured CMake source root.
    pub source: String,
    /// Generated Make intermediate represented as a CMake path.
    pub intermediate: String,
    /// Object staged into the configured Developer library.
    pub output: String,
    /// `C` or `CXX`.
    pub language: String,
    /// Closed literal preprocessor definitions, without the `-D` prefix.
    pub defines: Vec<String>,
    /// Closed literal preprocessor undefinitions, without the `-U` prefix.
    pub undefines: Vec<String>,
    /// Closed compile options passed through to CMake.
    pub options: Vec<String>,
    /// Proven source-root or generated include directories.
    pub includes: Vec<String>,
    /// One-based source line of the compile macro.
    pub line: usize,
}

/// One finite ordinary Make target backed by source-derived SDK objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkObjectGroupDecl {
    /// Literal target name in the mmakefile.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// One-based target-rule line.
    pub line: usize,
    /// Unique compile/stage declarations required by this aggregate.
    pub objects: Vec<SdkObjectDecl>,
}

/// A relevant object aggregate or producer outside the closed model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkObjectRejection {
    /// Best available target name, or `<unknown>`.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// One-based line of the target or closest producer declaration.
    pub line: usize,
    /// Why the declaration was refused.
    pub reason: String,
}

#[derive(Debug, Clone)]
struct StagePattern {
    line: usize,
    directory: String,
    recipe_valid: bool,
    state: ConditionalTruth,
    control_valid: bool,
}

#[derive(Debug, Clone)]
struct CompilePattern {
    line: usize,
    language: String,
    directory: Option<String>,
    flags: Result<CompileFlags, String>,
    declaration_error: Option<String>,
    state: ConditionalTruth,
    control_valid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompileFlags {
    defines: Vec<String>,
    undefines: Vec<String>,
    options: Vec<String>,
    includes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MacroAction {
    Defined(String),
    Undefined,
}

#[derive(Debug, Clone)]
struct LaneCandidate {
    stage_line: usize,
    directory: String,
    language: String,
    compile_line: usize,
    intermediate: String,
    output: String,
    source: String,
    stage_error: Option<String>,
    compile_error: Option<String>,
    flags: Result<CompileFlags, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ObjectPath {
    Intermediate { path: String, basename: String },
    Output { path: String, basename: String },
}

#[derive(Debug, Clone)]
struct Aggregate {
    owner: String,
    line: usize,
    state: ConditionalTruth,
    control_valid: bool,
    object_hint: bool,
    prerequisites: Vec<String>,
    expansion_error: Option<String>,
}

/// Collects finite standalone-object groups from the same joined snapshot used
/// to produce `invocations`, `scope`, and conditional line states.
#[must_use]
pub(crate) fn collect_from_snapshot(
    invocations: &[Invocation],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    source_snapshot: &str,
) -> (Vec<SdkObjectGroupDecl>, Vec<SdkObjectRejection>) {
    let lines = source_snapshot.lines().collect::<Vec<_>>();
    let controls = scan_source_controls(&lines);
    let file = match source_relative_file(rel_dir) {
        Ok(file) => file,
        Err(reason) => {
            return (
                Vec::new(),
                vec![SdkObjectRejection {
                    owner: "<unknown>".into(),
                    file: rel_dir.to_string_lossy().replace('\\', "/"),
                    line: 1,
                    reason,
                }],
            );
        }
    };
    let Some(configured_lib) = dirs.expand("$(AROS_LIB)") else {
        return (
            Vec::new(),
            vec![SdkObjectRejection {
                owner: "<unknown>".into(),
                file,
                line: 1,
                reason: "configured Developer library path `AROS_LIB` is unresolved".into(),
            }],
        );
    };
    let Some(generated_root) = dirs.expand("$(GENDIR)") else {
        return (
            Vec::new(),
            vec![SdkObjectRejection {
                owner: "<unknown>".into(),
                file,
                line: 1,
                reason: "configured generated directory `GENDIR` is unresolved".into(),
            }],
        );
    };
    if validate_cmake_descendant(&configured_lib, "${AROS_BUILD_DIR}").is_err() {
        return (
            Vec::new(),
            vec![SdkObjectRejection {
                owner: "<unknown>".into(),
                file,
                line: 1,
                reason: "configured Developer library path is not a safe build-tree directory"
                    .into(),
            }],
        );
    }
    if generated_root != "${AROS_BUILD_DIR}/gen" {
        return (
            Vec::new(),
            vec![SdkObjectRejection {
                owner: "<unknown>".into(),
                file,
                line: 1,
                reason: "configured `GENDIR` is outside the supported CMake generated root".into(),
            }],
        );
    }
    let Some(local_generated) = join_cmake_path(&generated_root, &path_text(rel_dir)) else {
        return (
            Vec::new(),
            vec![SdkObjectRejection {
                owner: "<unknown>".into(),
                file,
                line: 1,
                reason: "mmakefile directory is not a safe relative path".into(),
            }],
        );
    };
    let mut rejections = Vec::new();
    let stages = collect_stage_patterns(
        &lines,
        scope,
        dirs,
        root,
        rel_dir,
        line_states,
        &controls,
        &file,
        &configured_lib,
        &local_generated,
        &mut rejections,
    );
    let compiles = collect_compile_patterns(
        invocations,
        &lines,
        scope,
        dirs,
        root,
        rel_dir,
        line_states,
        &controls,
    );
    let candidates = pair_candidates(&stages, &compiles, &configured_lib, &local_generated);
    let aggregates = collect_aggregates(&lines, scope, dirs, root, rel_dir, line_states, &controls);

    // GNU Make chooses the first applicable implicit rule whose prerequisite
    // ought to exist. Every active ordinary target participates, including
    // targets not requested by the selected native graph. Keep lane evidence
    // independently of whether the aggregate is otherwise admissible.
    let mut declared_intermediates = BTreeSet::new();
    let mut conditional_intermediates = BTreeMap::<String, (String, usize)>::new();
    let unresolved_membership = aggregates
        .iter()
        .find(|aggregate| {
            aggregate.state != ConditionalTruth::False && aggregate.expansion_error.is_some()
        })
        .map(|aggregate| (aggregate.owner.clone(), aggregate.line));
    for aggregate in &aggregates {
        if aggregate.state == ConditionalTruth::False || aggregate.expansion_error.is_some() {
            continue;
        }
        if aggregate.prerequisites.is_empty() {
            continue;
        }
        for prerequisite in &aggregate.prerequisites {
            if let Ok(ObjectPath::Intermediate { path, .. }) =
                parse_object_path(prerequisite, &configured_lib, &local_generated)
            {
                // Ought-to-exist selection depends on each declared path even
                // if a sibling prerequisite later makes this owner unsupported.
                if aggregate.state == ConditionalTruth::True && aggregate.control_valid {
                    declared_intermediates.insert(path);
                } else {
                    conditional_intermediates
                        .entry(path)
                        .or_insert_with(|| (aggregate.owner.clone(), aggregate.line));
                }
            }
        }
    }

    let mut groups_by_owner: BTreeMap<String, SdkObjectGroupDecl> = BTreeMap::new();
    let mut prior_owner_lines = BTreeMap::<String, usize>::new();
    let mut all_path_decls = BTreeMap::<String, (SdkObjectDecl, String, usize)>::new();
    let mut conflicted_paths = BTreeSet::new();
    for aggregate in aggregates {
        let has_object = aggregate.prerequisites.iter().any(|item| {
            Path::new(item)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("o"))
        });
        if !has_object && !aggregate.object_hint {
            continue;
        }
        if aggregate.state == ConditionalTruth::False {
            continue;
        }
        let reject = |reason: String| SdkObjectRejection {
            owner: aggregate.owner.clone(),
            file: file.clone(),
            line: aggregate.line,
            reason,
        };
        if let Some(previous_line) =
            prior_owner_lines.insert(aggregate.owner.clone(), aggregate.line)
        {
            rejections.push(reject(format!(
                "ordinary aggregate owner has duplicate declarations (first at line {previous_line})"
            )));
            groups_by_owner.remove(&aggregate.owner);
            continue;
        }
        if aggregate.state == ConditionalTruth::Unknown {
            rejections.push(reject(
                "aggregate is guarded by an unresolved Make conditional".into(),
            ));
            continue;
        }
        if !aggregate.control_valid {
            rejections.push(reject(
                "aggregate is inside malformed or unbalanced Make control flow".into(),
            ));
            continue;
        }
        if let Some(error) = aggregate.expansion_error {
            rejections.push(reject(format!(
                "cannot resolve aggregate prerequisites: {error}"
            )));
            continue;
        }
        if aggregate.prerequisites.is_empty() {
            continue;
        }
        let object_paths = match aggregate
            .prerequisites
            .iter()
            .map(|path| parse_object_path(path, &configured_lib, &local_generated))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(objects) => objects,
            Err(reason) => {
                rejections.push(reject(format!(
                    "aggregate has an unsupported prerequisite: {reason}"
                )));
                continue;
            }
        };
        let mut selected = Vec::<(SdkObjectDecl, String)>::new();
        let mut lane_for_basename = BTreeMap::<String, String>::new();
        let mut failure = None;
        for object in object_paths {
            let basename = match &object {
                ObjectPath::Intermediate { basename, .. } | ObjectPath::Output { basename, .. } => {
                    basename
                }
            };
            let choices = candidates
                .iter()
                .map(|candidate| candidate.for_basename(basename, rel_dir))
                .filter(|candidate| candidate.source_exists(root));
            let candidate = match &object {
                ObjectPath::Intermediate { path, .. } => {
                    let matching = choices
                        .filter(|candidate| candidate.intermediate == *path)
                        .collect::<Vec<_>>();
                    match matching.as_slice() {
                        [candidate] => Some(candidate.clone()),
                        [] => {
                            failure = Some(format!(
                                "intermediate prerequisite `{path}` has no unique source-declared compile/stage pair"
                            ));
                            None
                        }
                        _ => {
                            failure = Some(format!(
                                "intermediate prerequisite `{path}` has duplicate source-declared compile/stage pairs"
                            ));
                            None
                        }
                    }
                }
                ObjectPath::Output { .. } => {
                    let matching = choices.collect::<Vec<_>>();
                    if matching.is_empty() {
                        failure = Some(format!(
                            "SDK output `{}` has no local source-declared compile/stage pair",
                            object_path_text(&object)
                        ));
                        None
                    } else if matching.iter().enumerate().any(|(index, candidate)| {
                        matching[..index]
                            .iter()
                            .any(|prior| prior.intermediate == candidate.intermediate)
                    }) {
                        failure = Some(format!(
                            "SDK output `{}` has duplicate source-declared staging or compile patterns",
                            object_path_text(&object)
                        ));
                        None
                    } else {
                        let selected = matching
                            .iter()
                            .find(|candidate| {
                                declared_intermediates.contains(&candidate.intermediate)
                            })
                            .unwrap_or(&matching[0])
                            .clone();
                        if let Some((owner, line)) = &unresolved_membership {
                            failure = Some(format!(
                                "cannot prove implicit lane for `{}` because ordinary aggregate `{owner}` at line {line} has unresolved prerequisites",
                                object_path_text(&object)
                            ));
                            None
                        } else if let Some(possible_lane) = matching.iter().find(|candidate| {
                            candidate.intermediate != selected.intermediate
                                && conditional_intermediates.contains_key(&candidate.intermediate)
                        }) {
                            let (owner, line) =
                                &conditional_intermediates[&possible_lane.intermediate];
                            failure = Some(format!(
                                "cannot prove implicit lane for `{}` because `{}` from unresolved aggregate `{owner}` at line {line} may take precedence",
                                object_path_text(&object),
                                possible_lane.intermediate
                            ));
                            None
                        } else {
                            Some(selected)
                        }
                    }
                }
            };
            let Some(candidate) = candidate else {
                continue;
            };
            if let Some(previous) =
                lane_for_basename.insert(basename.clone(), candidate.intermediate.clone())
            {
                if previous != candidate.intermediate {
                    failure = Some(format!(
                        "aggregate names both `{previous}` and `{}` for `{basename}`; the implicit lane proof conflicts",
                        candidate.intermediate
                    ));
                    continue;
                }
            }
            if let Some(reason) = candidate.rejection_reason() {
                failure = Some(reason);
                continue;
            }
            if let Some(reason) = candidate.source_error(root) {
                failure = Some(reason);
                continue;
            }
            let flags = match &candidate.flags {
                Ok(flags) => flags.clone(),
                Err(reason) => {
                    failure = Some(reason.clone());
                    continue;
                }
            };
            let declaration = SdkObjectDecl {
                source: candidate.source.clone(),
                intermediate: candidate.intermediate.clone(),
                output: candidate.output.clone(),
                language: candidate.language.clone(),
                defines: flags.defines,
                undefines: flags.undefines,
                options: flags.options,
                includes: flags.includes,
                line: candidate.compile_line + 1,
            };
            let key = format!("{}\n{}", declaration.intermediate, declaration.output);
            if !selected
                .iter()
                .any(|(existing, _)| *existing == declaration)
            {
                selected.push((declaration, key));
            }
        }
        if let Some(reason) = failure {
            rejections.push(reject(reason));
            continue;
        }
        if selected.is_empty() {
            continue;
        }
        let group = SdkObjectGroupDecl {
            owner: aggregate.owner.clone(),
            file: file.clone(),
            line: aggregate.line,
            objects: selected
                .iter()
                .map(|(declaration, _)| declaration.clone())
                .collect(),
        };
        for declaration in &group.objects {
            let keys = [
                format!("intermediate:{}", declaration.intermediate),
                format!("output:{}", declaration.output),
            ];
            if keys.iter().any(|key| conflicted_paths.contains(key)) {
                rejections.push(reject(
                    "object output conflicts with another source declaration".into(),
                ));
                failure = Some("conflicting source declaration".into());
                continue;
            }
            let conflict = keys.iter().find_map(|key| {
                all_path_decls.get(key).and_then(|(prior, owner, line)| {
                    (prior != declaration).then(|| (key.clone(), owner.clone(), *line))
                })
            });
            if let Some((key, prior_owner, prior_line)) = conflict {
                conflicted_paths.insert(key);
                groups_by_owner.remove(&prior_owner);
                rejections.push(SdkObjectRejection {
                    owner: prior_owner,
                    file: file.clone(),
                    line: prior_line,
                    reason:
                        "same SDK object intermediate/output has conflicting compile declarations"
                            .into(),
                });
                rejections.push(reject(
                    "same SDK object intermediate/output has conflicting compile declarations"
                        .into(),
                ));
                failure = Some("conflicting source declaration".into());
            } else {
                for key in keys {
                    all_path_decls
                        .entry(key)
                        .or_insert_with(|| (declaration.clone(), group.owner.clone(), group.line));
                }
            }
        }
        if failure.is_none() {
            groups_by_owner.insert(group.owner.clone(), group);
        }
    }
    rejections.sort_by(|left, right| {
        (&left.file, left.line, &left.owner, &left.reason).cmp(&(
            &right.file,
            right.line,
            &right.owner,
            &right.reason,
        ))
    });
    rejections.dedup();
    (groups_by_owner.into_values().collect(), rejections)
}

impl LaneCandidate {
    fn source_exists(&self, root: &Path) -> bool {
        let relative = self
            .source
            .strip_prefix("${AROS_SOURCE_DIR}/")
            .unwrap_or_default();
        if relative.is_empty() {
            return false;
        }
        root.join(relative).symlink_metadata().is_ok()
    }

    fn source_error(&self, root: &Path) -> Option<String> {
        let relative = self.source.strip_prefix("${AROS_SOURCE_DIR}/")?;
        safe_source_path(root, Path::new(relative), false)
            .err()
            .map(|error| format!("unsafe SDK object source: {error}"))
    }

    fn rejection_reason(&self) -> Option<String> {
        self.stage_error
            .as_ref()
            .or(self.compile_error.as_ref())
            .cloned()
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_stage_patterns(
    lines: &[&str],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    controls: &SourceControls,
    _file: &str,
    configured_lib: &str,
    local_generated: &str,
    _rejections: &mut Vec<SdkObjectRejection>,
) -> Vec<StagePattern> {
    let mut stages = Vec::new();
    for (line_no, raw_line) in lines.iter().enumerate() {
        if controls.suppressed[line_no] || raw_line.starts_with('\t') {
            continue;
        }
        let plain = crate::make_vars::strip_make_comment(raw_line).trim();
        let Some((target, prerequisite)) = split_rule(plain) else {
            continue;
        };
        if target.trim() != "$(AROS_LIB)/%.o" {
            continue;
        }
        let state = effective_line_state(line_states, controls, line_no);
        let context = MakeExprContext::new(scope, dirs, line_no, root, rel_dir);
        let rendered_target = evaluate_make_expr(target.trim(), &context);
        let rendered_prerequisite = evaluate_make_expr(prerequisite.trim(), &context);
        let (Ok(rendered_target), Ok(rendered_prerequisite)) =
            (rendered_target, rendered_prerequisite)
        else {
            continue;
        };
        if rendered_target != format!("{configured_lib}/%.o") {
            continue;
        }
        let Some(directory) = rendered_prerequisite.strip_suffix("/%.o") else {
            continue;
        };
        if directory != local_generated
            && !directory
                .strip_prefix(&format!("{local_generated}/"))
                .is_some_and(safe_component)
        {
            continue;
        }
        let recipe_valid = lines
            .get(line_no + 1)
            .is_some_and(|line| *line == "\t@$(CP) $< $@")
            && !lines
                .get(line_no + 2)
                .is_some_and(|line| line.starts_with('\t'));
        let control_valid = controls.valid.get(line_no).copied().unwrap_or(false);
        stages.push(StagePattern {
            line: line_no,
            directory: directory.to_owned(),
            recipe_valid,
            state,
            control_valid,
        });
    }
    stages
}

#[allow(clippy::too_many_arguments)]
fn collect_compile_patterns(
    invocations: &[Invocation],
    lines: &[&str],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    controls: &SourceControls,
) -> Vec<CompilePattern> {
    let mut compiles = Vec::new();
    for invocation in invocations.iter().filter(|invocation| {
        invocation.name == "rule_compile" || invocation.name == "rule_compile_cxx"
    }) {
        let line_no = invocation.line;
        let Some(source_line) = lines.get(line_no) else {
            continue;
        };
        if controls.suppressed.get(line_no).copied().unwrap_or(true)
            || source_line.starts_with('\t')
            || !source_line
                .trim_start()
                .starts_with(if invocation.name == "rule_compile" {
                    "%rule_compile"
                } else {
                    "%rule_compile_cxx"
                })
        {
            continue;
        }
        let state = effective_line_state(line_states, controls, line_no);
        let control_valid = controls.valid.get(line_no).copied().unwrap_or(false);
        let parsed = parse_macro_arguments(&invocation.args);
        let (arguments, parse_error) = match parsed {
            Ok(arguments) => (arguments, None),
            Err(error) => (BTreeMap::new(), Some(error)),
        };
        let directory = arguments.get("targetdir").and_then(|raw| {
            let context = MakeExprContext::new(scope, dirs, line_no, root, rel_dir);
            evaluate_make_expr(raw, &context).ok()
        });
        let mut declaration_error = parse_error;
        let allowed = ["basename", "targetdir", "compiler"]
            .into_iter()
            .collect::<BTreeSet<_>>();
        if declaration_error.is_none() {
            if let Some(unsupported) = arguments
                .keys()
                .find(|name| !allowed.contains(name.as_str()))
            {
                declaration_error = Some(format!(
                    "compile macro has unsupported explicit argument `{unsupported}`"
                ));
            }
            if arguments.get("basename").map(String::as_str) != Some("%") {
                declaration_error.get_or_insert_with(|| {
                    "compile macro must declare the wildcard basename `basename=%`".into()
                });
            }
            if !arguments.contains_key("targetdir") || directory.is_none() {
                declaration_error.get_or_insert_with(|| {
                    "compile macro must declare a resolvable explicit `targetdir`".into()
                });
            }
            if arguments
                .get("compiler")
                .is_some_and(|compiler| compiler != "target")
            {
                declaration_error.get_or_insert_with(|| {
                    "compile macro compiler must be omitted or exactly `target`".into()
                });
            }
        }
        if let Some(name) = local_compile_default_assignment(lines, controls, line_states, line_no)
        {
            declaration_error.get_or_insert_with(|| {
                format!("source-local `{name}` assignment replaces the compile macro default and is unsupported")
            });
        }
        let flags = if declaration_error.is_none() {
            collect_user_flags(
                scope,
                dirs,
                root,
                rel_dir,
                line_no,
                invocation.name == "rule_compile_cxx",
            )
        } else {
            Err(declaration_error
                .clone()
                .unwrap_or_else(|| "invalid compile macro".into()))
        };
        compiles.push(CompilePattern {
            line: line_no,
            language: if invocation.name == "rule_compile_cxx" {
                "CXX".into()
            } else {
                "C".into()
            },
            directory,
            flags,
            declaration_error,
            state,
            control_valid,
        });
    }
    compiles
}

fn pair_candidates(
    stages: &[StagePattern],
    compiles: &[CompilePattern],
    configured_lib: &str,
    local_generated: &str,
) -> Vec<LaneCandidate> {
    let mut candidates = Vec::new();
    for stage in stages {
        if stage.state == ConditionalTruth::False {
            continue;
        }
        let Some(lane) = stage.directory.strip_prefix(local_generated) else {
            continue;
        };
        let lane = lane.strip_prefix('/').unwrap_or(lane);
        if !lane.is_empty() && !safe_component(lane) {
            continue;
        }
        for compile in compiles
            .iter()
            .filter(|compile| compile.directory.as_deref() == Some(stage.directory.as_str()))
        {
            let mut stage_error = None;
            if !stage.recipe_valid {
                stage_error = Some(
                    "SDK staging pattern does not have the sole exact `@$(CP) $< $@` recipe".into(),
                );
            } else if stage.state == ConditionalTruth::Unknown {
                stage_error =
                    Some("SDK staging pattern is guarded by an unresolved Make conditional".into());
            } else if stage.state == ConditionalTruth::False {
                continue;
            } else if !stage.control_valid {
                stage_error = Some(
                    "SDK staging pattern is inside malformed or unbalanced Make control flow"
                        .into(),
                );
            }
            let mut compile_error = compile.declaration_error.clone();
            if compile.state == ConditionalTruth::False {
                continue;
            }
            if compile.state == ConditionalTruth::Unknown {
                compile_error.get_or_insert_with(|| {
                    "compile macro is guarded by an unresolved Make conditional".into()
                });
            } else if !compile.control_valid {
                compile_error.get_or_insert_with(|| {
                    "compile macro is inside malformed or unbalanced Make control flow".into()
                });
            }
            let _ = lane;
            let intermediate = format!("{}/<object>.o", stage.directory);
            let output = format!("{configured_lib}/<object>.o");
            candidates.push(LaneCandidate {
                stage_line: stage.line,
                directory: stage.directory.clone(),
                language: compile.language.clone(),
                compile_line: compile.line,
                intermediate,
                output,
                source: format!(
                    "${{AROS_SOURCE_DIR}}/<object>.{}",
                    if compile.language == "C" { "c" } else { "cpp" }
                ),
                stage_error,
                compile_error,
                flags: compile.flags.clone(),
            });
        }
    }
    candidates.sort_by_key(|candidate| (candidate.stage_line, candidate.compile_line));
    candidates
}

impl LaneCandidate {
    fn for_basename(&self, basename: &str, rel_dir: &Path) -> Self {
        let stem = basename.strip_suffix(".o").unwrap_or(basename);
        let directory = path_text(rel_dir);
        let source = if directory.is_empty() || directory == "." {
            format!(
                "${{AROS_SOURCE_DIR}}/{stem}.{}",
                if self.language == "C" { "c" } else { "cpp" }
            )
        } else {
            format!(
                "${{AROS_SOURCE_DIR}}/{directory}/{stem}.{}",
                if self.language == "C" { "c" } else { "cpp" }
            )
        };
        Self {
            intermediate: format!("{}/{}", self.directory, basename),
            output: format!(
                "{}/{}",
                self.output.trim_end_matches("/<object>.o"),
                basename
            ),
            source,
            ..self.clone()
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_aggregates(
    lines: &[&str],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    controls: &SourceControls,
) -> Vec<Aggregate> {
    let mut aggregates = Vec::new();
    for (line_no, raw_line) in lines.iter().enumerate() {
        if controls.suppressed[line_no] || raw_line.starts_with('\t') {
            continue;
        }
        let plain = crate::make_vars::strip_make_comment(raw_line).trim();
        let Some((target, prerequisite)) = split_rule(plain) else {
            continue;
        };
        let target = target.trim();
        if !safe_target_name(target) || target.starts_with('.') {
            continue;
        }
        // A target with a tab-indented recipe is outside the aggregate model.
        if lines
            .get(line_no + 1)
            .is_some_and(|line| line.starts_with('\t'))
        {
            continue;
        }
        let context = MakeExprContext::new(scope, dirs, line_no, root, rel_dir);
        let state = effective_line_state(line_states, controls, line_no);
        if state == ConditionalTruth::False {
            continue;
        }
        match evaluate_make_list(prerequisite.trim(), &context) {
            Ok(prerequisites) => {
                if prerequisites.is_empty() {
                    continue;
                }
                let object_hint = prerequisites.iter().any(|item| {
                    Path::new(item)
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("o"))
                });
                aggregates.push(Aggregate {
                    owner: target.to_owned(),
                    line: line_no + 1,
                    state,
                    control_valid: controls.valid[line_no],
                    object_hint,
                    prerequisites,
                    expansion_error: None,
                });
            }
            Err(error) => {
                let uppercase = prerequisite.to_ascii_uppercase();
                let object_hint =
                    prerequisite.to_ascii_lowercase().contains(".o") || uppercase.contains("OBJ");
                aggregates.push(Aggregate {
                    owner: target.to_owned(),
                    line: line_no + 1,
                    state,
                    control_valid: controls.valid[line_no],
                    object_hint,
                    prerequisites: Vec::new(),
                    expansion_error: Some(error.to_string()),
                });
            }
        }
    }
    aggregates
}

fn collect_user_flags(
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
    cxx: bool,
) -> Result<CompileFlags, String> {
    let lookup: &MakeVariableLookup<'_> = &|name: &str| {
        (matches!(name, "USER_CPPFLAGS" | "USER_CFLAGS" | "USER_CXXFLAGS")
            && scope.raw_at(name, line).is_none())
        .then(String::new)
    };
    let guard: &MakeVariableGuard<'_> = &|name: &str| {
        scope
            .path_is_conditional_at(name, line)
            .then(|| {
                "value depends on an unresolved conditional, opaque definition, or frozen path"
                    .into()
            })
            .or_else(|| scope.flavor_uncertainty_reason_at(name, line))
    };
    let raw = if cxx {
        "$(USER_CPPFLAGS) $(USER_CXXFLAGS)"
    } else {
        "$(USER_CPPFLAGS) $(USER_CFLAGS)"
    };
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir)
        .with_lookup(lookup)
        .with_guard(guard);
    let words = evaluate_make_list(raw, &context)
        .map_err(|error| format!("cannot resolve positional user compile flags: {error}"))?;
    parse_compile_flags(&words, scope, dirs, root, rel_dir, line)
}

fn parse_compile_flags(
    words: &[String],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Result<CompileFlags, String> {
    let mut flags = CompileFlags {
        defines: Vec::new(),
        undefines: Vec::new(),
        options: Vec::new(),
        includes: Vec::new(),
    };
    let mut seen_defines = BTreeSet::new();
    let mut seen_undefines = BTreeSet::new();
    let mut seen_options = BTreeSet::new();
    let mut seen_includes = BTreeSet::new();
    let mut macro_actions = BTreeMap::<String, MacroAction>::new();
    let mut at = 0;
    while at < words.len() {
        let word = &words[at];
        if let Some(value) = word.strip_prefix("-D") {
            if value.starts_with('=') {
                return Err(format!("unsafe preprocessor definition `{word}`"));
            }
            if !safe_define(value) {
                return Err(format!("unsafe preprocessor definition `{word}`"));
            }
            let (name, macro_value) = value
                .split_once('=')
                .map_or((value, "1"), |(name, macro_value)| (name, macro_value));
            check_macro_action(
                &mut macro_actions,
                name,
                MacroAction::Defined(macro_value.to_owned()),
            )?;
            push_unique(&mut flags.defines, &mut seen_defines, value.to_owned());
        } else if let Some(value) = word.strip_prefix("-U") {
            if !safe_identifier(value) {
                return Err(format!("unsafe preprocessor undefinition `{word}`"));
            }
            check_macro_action(&mut macro_actions, value, MacroAction::Undefined)?;
            push_unique(&mut flags.undefines, &mut seen_undefines, value.to_owned());
        } else if word == "-I" {
            let Some(path) = words.get(at + 1) else {
                return Err("`-I` has no include directory".into());
            };
            let rendered = prove_include_path(path, root, rel_dir, dirs)?;
            push_unique(&mut flags.includes, &mut seen_includes, rendered);
            at += 1;
        } else if let Some(path) = word.strip_prefix("-I") {
            if path.is_empty() {
                return Err("`-I` has no include directory".into());
            }
            let rendered = prove_include_path(path, root, rel_dir, dirs)?;
            push_unique(&mut flags.includes, &mut seen_includes, rendered);
        } else if safe_compile_option(word) {
            push_unique(&mut flags.options, &mut seen_options, word.clone());
        } else {
            return Err(format!("unsupported or unsafe user compile flag `{word}`"));
        }
        at += 1;
    }
    let _ = (scope, dirs, root, rel_dir, line);
    Ok(flags)
}

fn prove_include_path(
    path: &str,
    root: &Path,
    rel_dir: &Path,
    dirs: &DirVars,
) -> Result<String, String> {
    let generated_root = dirs
        .expand("$(GENDIR)")
        .ok_or_else(|| "configured generated directory `GENDIR` is unresolved".to_owned())?;
    if path.starts_with(&format!("{generated_root}/")) {
        validate_cmake_descendant(path, &generated_root)?;
        return Ok(path.to_owned());
    }
    let source_root = "${AROS_SOURCE_DIR}";
    let source_path = if path.starts_with(&format!("{source_root}/")) {
        path.strip_prefix(&format!("{source_root}/"))
            .unwrap_or_default()
            .to_owned()
    } else if path.starts_with('/') || path.contains("${") {
        return Err(format!(
            "include path `{path}` is outside the selected source/generated roots"
        ));
    } else {
        format!("{}/{path}", path_text(rel_dir))
    };
    let relative = validate_relative_path(&source_path)?;
    safe_source_path(root, &relative, true)?;
    Ok(format!("${{AROS_SOURCE_DIR}}/{}", path_text(&relative)))
}

fn parse_object_path(
    path: &str,
    configured_lib: &str,
    local_generated: &str,
) -> Result<ObjectPath, String> {
    if let Some(basename) = path.strip_prefix(&format!("{configured_lib}/")) {
        if safe_object_basename(basename) {
            return Ok(ObjectPath::Output {
                path: path.to_owned(),
                basename: basename.to_owned(),
            });
        }
        return Err(format!(
            "Developer-library object path `{path}` is not one safe basename"
        ));
    }
    if let Some(rest) = path.strip_prefix(&format!("{local_generated}/")) {
        let (lane, basename) = match rest.split_once('/') {
            Some((lane, basename)) if safe_component(lane) => (Some(lane), basename),
            Some(_) => return Err(format!("generated object path `{path}` has an unsafe lane")),
            None => (None, rest),
        };
        if !safe_object_basename(basename) {
            return Err(format!(
                "generated object path `{path}` is not one safe basename"
            ));
        }
        let _ = lane;
        return Ok(ObjectPath::Intermediate {
            path: path.to_owned(),
            basename: basename.to_owned(),
        });
    }
    Err(format!("object prerequisite `{path}` is outside the local generated or configured Developer library roots"))
}

fn object_path_text(path: &ObjectPath) -> &str {
    match path {
        ObjectPath::Intermediate { path, .. } | ObjectPath::Output { path, .. } => path,
    }
}

#[derive(Debug)]
struct SourceControls {
    suppressed: Vec<bool>,
    valid: Vec<bool>,
    states_without_context: Vec<ConditionalTruth>,
}

fn valid_make_define_header(rest: &str) -> bool {
    let modifiers = ["override", "export", "private"];
    rest.split_whitespace()
        .find(|word| !modifiers.contains(word))
        .is_some_and(|name| !matches!(name, "=" | ":=" | "::=" | "+=" | "?=" | "!="))
}

fn scan_source_controls(lines: &[&str]) -> SourceControls {
    #[derive(Clone, Copy)]
    struct Conditional {
        start: usize,
        seen_else: bool,
    }
    #[derive(Clone, Copy)]
    enum DefinitionEnd {
        Endef,
        PercentEnd,
    }
    let mut suppressed = vec![false; lines.len()];
    let mut valid = vec![true; lines.len()];
    let mut states_without_context = vec![ConditionalTruth::True; lines.len()];
    let mut definitions: Vec<DefinitionEnd> = Vec::new();
    let mut conditionals: Vec<Conditional> = Vec::new();
    let mut control_bad = false;
    for (line_no, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim_start();
        if let Some(end) = definitions.last().copied() {
            suppressed[line_no] = true;
            if raw.starts_with('\t') {
                valid[line_no] = !control_bad;
                continue;
            }
            if let Some(rest) = make_define_header(trimmed) {
                if !valid_make_define_header(rest) {
                    valid[line_no] = false;
                    control_bad = true;
                }
                definitions.push(DefinitionEnd::Endef);
            } else if let Some(rest) = trimmed.strip_prefix("%define ") {
                if rest.trim().is_empty() {
                    valid[line_no] = false;
                    control_bad = true;
                }
                definitions.push(DefinitionEnd::PercentEnd);
            } else {
                match make_control_word(trimmed) {
                    Some(("endef", _)) if matches!(end, DefinitionEnd::Endef) => {
                        definitions.pop();
                    }
                    _ if trimmed == "%end" && matches!(end, DefinitionEnd::PercentEnd) => {
                        definitions.pop();
                    }
                    _ => {}
                }
            }
            continue;
        }
        if raw.starts_with('\t') {
            states_without_context[line_no] = if conditionals.is_empty() {
                ConditionalTruth::True
            } else {
                ConditionalTruth::Unknown
            };
            valid[line_no] = !control_bad;
            continue;
        }
        let directive = make_control_word(trimmed);
        if let Some(rest) = make_define_header(trimmed) {
            suppressed[line_no] = true;
            if !valid_make_define_header(rest) {
                valid[line_no] = false;
                control_bad = true;
            }
            definitions.push(DefinitionEnd::Endef);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("%define ") {
            suppressed[line_no] = true;
            if rest.trim().is_empty() {
                valid[line_no] = false;
                control_bad = true;
            }
            definitions.push(DefinitionEnd::PercentEnd);
            continue;
        }
        if matches!(directive, Some(("endef", _))) || trimmed == "%end" {
            suppressed[line_no] = true;
            valid[line_no] = false;
            control_bad = true;
            continue;
        }
        states_without_context[line_no] = if conditionals.is_empty() {
            ConditionalTruth::True
        } else {
            ConditionalTruth::Unknown
        };
        valid[line_no] = !control_bad;
        match directive {
            Some(("ifeq" | "ifneq" | "ifdef" | "ifndef", _)) => conditionals.push(Conditional {
                start: line_no,
                seen_else: false,
            }),
            Some(("else", _)) => {
                if let Some(frame) = conditionals.last_mut() {
                    if frame.seen_else {
                        valid[line_no] = false;
                        control_bad = true;
                    }
                    frame.seen_else = true;
                } else {
                    valid[line_no] = false;
                    control_bad = true;
                }
            }
            Some(("endif", _)) if conditionals.pop().is_none() => {
                valid[line_no] = false;
                control_bad = true;
            }
            _ => {}
        }
    }
    for frame in conditionals {
        for is_valid in valid.iter_mut().skip(frame.start) {
            *is_valid = false;
        }
    }
    if !definitions.is_empty() {
        if let Some(start) = lines.iter().position(|line| {
            let trimmed = line.trim_start();
            make_define_header(trimmed).is_some() || trimmed.starts_with("%define ")
        }) {
            for line in start..lines.len() {
                suppressed[line] = true;
                valid[line] = false;
            }
        }
    }
    SourceControls {
        suppressed,
        valid,
        states_without_context,
    }
}

fn effective_line_state(
    line_states: Option<&[ConditionalTruth]>,
    controls: &SourceControls,
    line: usize,
) -> ConditionalTruth {
    line_states
        .and_then(|states| states.get(line))
        .copied()
        .unwrap_or_else(|| {
            controls
                .states_without_context
                .get(line)
                .copied()
                .unwrap_or(ConditionalTruth::Unknown)
        })
}

fn make_control_word(line: &str) -> Option<(&str, &str)> {
    let line = line
        .split_once(" #")
        .map_or(line, |(before, _)| before)
        .trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with('%') {
        return None;
    }
    let mut parts = line.splitn(2, char::is_whitespace);
    Some((parts.next()?, parts.next().unwrap_or_default().trim()))
}

fn make_define_header(line: &str) -> Option<&str> {
    let mut remaining = line
        .split_once(" #")
        .map_or(line, |(before, _)| before)
        .trim();
    loop {
        let mut parts = remaining.splitn(2, char::is_whitespace);
        let word = parts.next()?;
        let rest = parts.next().unwrap_or_default().trim();
        match word {
            "define" => return Some(rest),
            "override" | "export" | "private" => remaining = rest,
            _ => return None,
        }
    }
}

fn split_rule(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut depth = 0usize;
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes[at] == b'$' && bytes.get(at + 1) == Some(&b'(') {
            depth += 1;
            at += 2;
            continue;
        }
        if bytes[at] == b')' && depth > 0 {
            depth -= 1;
            at += 1;
            continue;
        }
        if bytes[at] == b':' && depth == 0 {
            if bytes.get(at + 1) == Some(&b'=') || bytes.get(at + 1) == Some(&b':') {
                return None;
            }
            return Some((&line[..at], &line[at + 1..]));
        }
        at += 1;
    }
    None
}

fn parse_macro_arguments(raw: &str) -> Result<BTreeMap<String, String>, String> {
    let words = split_make_argument_words(raw)?;
    let mut arguments = BTreeMap::new();
    for word in words {
        let Some((name, value)) = word.split_once('=') else {
            return Err("compile macro contains an unnamed or malformed argument".into());
        };
        if !safe_identifier(name) {
            return Err("compile macro contains an unsafe argument name".into());
        }
        if arguments.contains_key(name) {
            return Err(format!("compile macro repeats argument `{name}`"));
        }
        let value = if value.starts_with('"') || value.starts_with('\'') {
            let quote = value.as_bytes()[0] as char;
            if value.len() < 2
                || !value.ends_with(quote)
                || value[1..value.len() - 1].contains(quote)
            {
                return Err("compile macro has an unsupported quoted argument".into());
            }
            value[1..value.len() - 1].to_owned()
        } else {
            if value.contains(['"', '\'']) {
                return Err("compile macro has an unsupported argument quote".into());
            }
            value.to_owned()
        };
        arguments.insert(name.to_owned(), value);
    }
    Ok(arguments)
}

fn split_make_argument_words(raw: &str) -> Result<Vec<String>, String> {
    let bytes = raw.as_bytes();
    let mut words = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut depth = 0usize;
    let mut at = 0usize;
    while at < bytes.len() {
        let byte = bytes[at];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
            at += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            start.get_or_insert(at);
            quote = Some(byte);
            at += 1;
            continue;
        }
        if byte == b'$' && bytes.get(at + 1) == Some(&b'(') {
            start.get_or_insert(at);
            depth += 1;
            at += 2;
            continue;
        }
        if byte == b')' && depth > 0 {
            depth -= 1;
            at += 1;
            continue;
        }
        if byte.is_ascii_whitespace() && depth == 0 {
            if let Some(begin) = start.take() {
                words.push(raw[begin..at].to_owned());
            }
            at += 1;
            continue;
        }
        start.get_or_insert(at);
        at += 1;
    }
    if quote.is_some() || depth != 0 {
        return Err("compile macro has unbalanced quote or Make expression".into());
    }
    if let Some(begin) = start {
        words.push(raw[begin..].to_owned());
    }
    Ok(words)
}

fn safe_target_name(value: &str) -> bool {
    !value.is_empty()
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn safe_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn safe_define(value: &str) -> bool {
    if let Some((name, rhs)) = value.split_once('=') {
        safe_identifier(name)
            && !rhs.is_empty()
            && rhs.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'_' | b'.' | b',' | b'+' | b'/' | b'-')
            })
    } else {
        safe_identifier(value)
    }
}

fn safe_compile_option(value: &str) -> bool {
    matches!(
        value,
        "-g" | "-g0"
            | "-g1"
            | "-g2"
            | "-g3"
            | "-O0"
            | "-O1"
            | "-O2"
            | "-O3"
            | "-Os"
            | "-Og"
            | "-Ofast"
            | "-pipe"
            | "-pthread"
            | "-fexceptions"
            | "-fno-exceptions"
            | "-frtti"
            | "-fno-rtti"
            | "-fstrict-aliasing"
            | "-fno-strict-aliasing"
            | "-fwrapv"
            | "-fno-common"
            | "-fcommon"
            | "-fshort-wchar"
            | "-fvisibility=hidden"
            | "-fvisibility=default"
            | "-fno-omit-frame-pointer"
            | "-fomit-frame-pointer"
            | "-fno-builtin"
            | "-Wall"
            | "-Wextra"
            | "-Werror"
            | "-Wpedantic"
            | "-Wformat"
            | "-Wshadow"
            | "-Wconversion"
            | "-Wsign-conversion"
            | "-Wno-unused-parameter"
    ) || safe_standard_option(value)
        || safe_warning_option(value)
}

fn safe_standard_option(value: &str) -> bool {
    let Some(version) = value.strip_prefix("-std=") else {
        return false;
    };
    ["c", "gnu", "c++", "gnu++"].iter().any(|prefix| {
        version.strip_prefix(prefix).is_some_and(|number| {
            matches!(
                number,
                "89" | "90" | "99" | "11" | "14" | "17" | "20" | "23"
            )
        })
    })
}

fn safe_warning_option(value: &str) -> bool {
    let Some(warning) = value.strip_prefix("-W") else {
        return false;
    };
    !warning.is_empty()
        && warning
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'=' | b'.'))
        && !warning.starts_with("l,")
        && !warning.starts_with("p,")
}

fn safe_object_basename(value: &str) -> bool {
    value.strip_suffix(".o").is_some_and(|stem| {
        !stem.is_empty()
            && stem.as_bytes()[0].is_ascii_alphanumeric()
            && stem.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'.')
            })
    })
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'.'))
}

fn validate_cmake_descendant(path: &str, root: &str) -> Result<(), String> {
    let Some(relative) = path.strip_prefix(&format!("{root}/")) else {
        return Err(format!("path `{path}` is not below `{root}`"));
    };
    if relative.split('/').all(safe_component) {
        Ok(())
    } else {
        Err(format!("path `{path}` contains unsafe components"))
    }
}

fn validate_relative_path(value: &str) -> Result<std::path::PathBuf, String> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path.components().next().is_none()
    {
        return Err(format!("unsafe relative source path `{value}`"));
    }
    if path
        .components()
        .any(|component| !safe_component(&component.as_os_str().to_string_lossy()))
    {
        return Err(format!("unsafe relative source path `{value}`"));
    }
    Ok(path.to_path_buf())
}

fn safe_source_path(root: &Path, relative: &Path, require_directory: bool) -> Result<(), String> {
    let relative_text = path_text(relative);
    let relative = validate_relative_path(&relative_text)?;
    let physical_root = root
        .canonicalize()
        .map_err(|error| format!("cannot resolve source root: {error}"))?;
    let mut path = physical_root;
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        path.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("source path `{}` is unavailable: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "source path `{}` crosses a symlink",
                path.display()
            ));
        }
        let final_component = index + 1 == components.len();
        if !final_component && !metadata.is_dir() {
            return Err(format!(
                "source component `{}` is not a directory",
                path.display()
            ));
        }
        if final_component
            && (require_directory && !metadata.is_dir()
                || !require_directory && !metadata.is_file())
        {
            return Err(format!(
                "source path `{}` is not a regular {}",
                path.display(),
                if require_directory {
                    "directory"
                } else {
                    "file"
                }
            ));
        }
    }
    Ok(())
}

fn source_relative_file(rel_dir: &Path) -> Result<String, String> {
    if rel_dir.as_os_str().is_empty() || rel_dir == Path::new(".") {
        return Ok("mmakefile.src".to_owned());
    }
    let path = validate_relative_path(&path_text(rel_dir))?;
    let value = format!("{}/mmakefile.src", path_text(&path));
    Ok(value)
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn join_cmake_path(root: &str, relative: &str) -> Option<String> {
    if relative.is_empty() || relative == "." {
        return Some(root.trim_end_matches('/').to_owned());
    }
    let relative = validate_relative_path(relative).ok()?;
    Some(format!(
        "{}/{}",
        root.trim_end_matches('/'),
        path_text(&relative)
    ))
}

fn local_compile_default_assignment(
    lines: &[&str],
    controls: &SourceControls,
    line_states: Option<&[ConditionalTruth]>,
    before_line: usize,
) -> Option<String> {
    lines
        .iter()
        .take(before_line)
        .enumerate()
        .filter(|(line_no, raw)| {
            !raw.starts_with('\t')
                && !controls.suppressed[*line_no]
                && (!controls.valid[*line_no]
                    || effective_line_state(line_states, controls, *line_no)
                        != ConditionalTruth::False)
        })
        .filter_map(|(_, raw)| {
            let plain = crate::make_vars::strip_make_comment(raw);
            crate::make_vars::variable_assignment(plain)
        })
        .find_map(|(name, _, _)| {
            matches!(name, "CPPFLAGS" | "CFLAGS" | "CXXFLAGS").then(|| name.to_owned())
        })
}

fn push_unique(output: &mut Vec<String>, seen: &mut BTreeSet<String>, value: String) {
    if seen.insert(value.clone()) {
        output.push(value);
    }
}

fn check_macro_action(
    actions: &mut BTreeMap<String, MacroAction>,
    name: &str,
    action: MacroAction,
) -> Result<(), String> {
    if let Some(previous) = actions.get(name) {
        if previous != &action {
            return Err(format!(
                "conflicting preprocessor actions for macro `{name}`"
            ));
        }
    } else {
        actions.insert(name.to_owned(), action);
    }
    Ok(())
}

#[cfg(test)]
#[path = "sdk_objects_tests.rs"]
mod tests;
