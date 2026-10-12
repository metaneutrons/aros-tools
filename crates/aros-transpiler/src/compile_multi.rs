//! Source-bound C compile-multi producers with independent Make depfiles.
//!
//! Template expansion and diagnostic ownership alone do not admit producers.
//! This collector independently binds an unconditional physical phony owner,
//! its complete object list, the selected include-deps call, and both argv
//! contexts to the same source scope. Unsupported forms remain diagnostics.

use crate::dirs::DirVars;
use crate::literal_objects::{
    LiteralDependencyDecl, LiteralObjectDecl, LiteralObjectGroupDecl, LiteralObjectRejection,
};
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::make_vars::{
    collect_vars_impl, strip_make_comment, variable_assignment, ConditionalTruth, VarScope,
};
use crate::parser::{
    architecture_scope_positions, join_continuations, join_continuations_with_origins,
    macro_invocations, TargetContext,
};
use crate::source_rule_ownership::CompileMultiTemplateContract;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

const SOURCE: &str = "${AROS_SOURCE_DIR}";
const BUILD: &str = "${AROS_BUILD_DIR}";
const GENERATED: &str = "${AROS_BUILD_DIR}/gen";
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_OBJECTS: usize = 4096;

/// Admit only source-written invocations and owners; inserted configuration
/// may supply values but may not manufacture a physical producer declaration.
pub fn collect(
    content: &str,
    recipe: &Path,
    root: &Path,
    dirs: &DirVars,
    target: Option<&TargetContext>,
) -> (Vec<LiteralObjectGroupDecl>, Vec<LiteralObjectRejection>) {
    let invocations = macro_invocations(&join_continuations(content));
    if !invocations
        .iter()
        .any(|call| call.name == "rule_compile_multi")
    {
        return (Vec::new(), Vec::new());
    }
    let configuration = crate::local_make_includes::inline_native_make_configuration_for_selection(
        content,
        root,
        recipe,
        crate::LocalMakeIncludeLimits::default(),
        target,
    );
    let effective = join_continuations(&configuration.expanded);
    let positions = architecture_scope_positions(root, recipe, content, &configuration, &effective);
    let (scope, states) = collect_vars_impl(&effective, target);
    let (physical, origins) = join_continuations_with_origins(content);
    let directory = recipe.parent().unwrap_or_else(|| Path::new(""));
    let lines: Vec<_> = physical.lines().collect();
    let effective_lines: Vec<_> = effective.lines().collect();
    let contract = CompileMultiTemplateContract::read(root);
    let mut groups = Vec::new();
    let mut rejections = Vec::new();
    for call in invocations
        .iter()
        .filter(|call| call.name == "rule_compile_multi")
    {
        let physical_line = origins.get(call.line).copied().unwrap_or(call.line);
        let position = positions
            .as_ref()
            .and_then(|positions| positions.get(physical_line))
            .copied()
            .flatten();
        if position.is_some_and(|line| states.get(line) == Some(&ConditionalTruth::False)) {
            continue;
        }
        let mut owner = "<unknown>".to_owned();
        let result = (|| {
            if content.len() > MAX_BYTES
                || effective.len() > MAX_BYTES
                || !configuration.issues.is_empty()
            {
                return Err(
                    "compile-multi configuration is incomplete or exceeds its byte bound".into(),
                );
            }
            let position =
                position.ok_or("compile-multi invocation has no unique physical source anchor")?;
            if states.get(position) != Some(&ConditionalTruth::True) {
                return Err("compile-multi invocation has unresolved Make control".into());
            }
            let contract = contract.as_ref().map_err(Clone::clone)?;
            let arguments = contract.arguments(&call.args)?;
            let namespace = expand(&arguments["mmake"], &scope, dirs, root, directory, position)?;
            if namespace.is_empty()
                || !namespace
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
            {
                return Err("compile-multi namespace is not a safe variable prefix".into());
            }
            check_scope(
                &effective_lines,
                &states,
                &scope,
                dirs,
                root,
                directory,
                position,
                &namespace,
            )?;
            for other in invocations
                .iter()
                .filter(|other| other.name == "rule_compile_multi" && other.line != call.line)
            {
                let other_position = origins
                    .get(other.line)
                    .and_then(|physical| {
                        positions
                            .as_ref()
                            .and_then(|positions| positions.get(*physical))
                    })
                    .copied()
                    .flatten()
                    .ok_or("competing compile-multi declaration lacks an anchor")?;
                if states.get(other_position) == Some(&ConditionalTruth::False) {
                    continue;
                }
                let other_arguments = contract.arguments(&other.args)?;
                if states.get(other_position) != Some(&ConditionalTruth::True)
                    || expand(
                        &other_arguments["mmake"],
                        &scope,
                        dirs,
                        root,
                        directory,
                        other_position,
                    )? == namespace
                {
                    return Err(
                        "compile-multi namespace has a competing or unresolved declaration".into(),
                    );
                }
            }
            let extra_name = format!("{namespace}_MC_EXTRA_CPPFLAGS");
            let extra = if scope.raw_at(&extra_name, position).is_some() {
                expand(
                    &format!("$({extra_name})"),
                    &scope,
                    dirs,
                    root,
                    directory,
                    position,
                )?
            } else if scope.is_known_local(&extra_name) {
                return Err(
                    "compile-multi extra preprocessor flags are not bound at invocation".into(),
                );
            } else {
                String::new()
            };
            let values = arguments
                .iter()
                .map(|(name, raw)| {
                    expand(raw, &scope, dirs, root, directory, position)
                        .map(|value| (name.clone(), value))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            if values["compiler"] != "target"
                || values["usetree"] != "no"
                || !values["srcdir"].is_empty()
            {
                return Err("compile-multi currently requires target compiler, usetree=no and implicit source directory".into());
            }
            let targetdir = rooted(&values["targetdir"], GENERATED)?;
            let basenames: Vec<_> = values["basenames"].split_whitespace().collect();
            if basenames.is_empty() || basenames.len() > MAX_OBJECTS {
                return Err("compile-multi basenames must be finite and nonempty".into());
            }
            let mut objects = Vec::new();
            let mut unique = BTreeSet::new();
            for basename in &basenames {
                let relative = source_basename(basename, directory)?;
                let stem = Path::new(&relative)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("compile-multi basename has no filename")?;
                if !unique.insert(format!("{targetdir}/{stem}.o").to_ascii_lowercase()) {
                    return Err("compile-multi flattened basenames collide".into());
                }
            }
            let search_directories = source_search_directories(
                &invocations,
                &origins,
                positions
                    .as_deref()
                    .ok_or("physical source lookup anchors are unavailable")?,
                &scope,
                &states,
                dirs,
                root,
                directory,
                contract,
            )?;
            for basename in basenames {
                let relative = source_basename(basename, directory)?;
                let source_relative = format!("{relative}.c");
                let file = root
                    .canonicalize()
                    .map_err(|error| error.to_string())?
                    .join(&source_relative);
                aros_common::open_regular_file_nofollow(&file).map_err(|error| {
                    format!(
                        "compile-multi source {source_relative} is unavailable or unsafe: {error}"
                    )
                })?;
                let stem = Path::new(&relative)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("compile-multi basename has no filename")?;
                prove_source_lookup(root, &search_directories, stem, &source_relative)?;
                let output = format!("{targetdir}/{stem}.o");
                let source = format!("{SOURCE}/{source_relative}");
                let cflags = &values["cflags"];
                let dflags = if values["dflags"].is_empty() {
                    cflags
                } else {
                    &values["dflags"]
                };
                let safety = expand("$(SAFETY_CFLAGS)", &scope, dirs, root, directory, position)?;
                let sysroot = expand("$(TARGET_SYSROOT)", &scope, dirs, root, directory, position)?;
                let mut compile = flags(&sysroot, dirs, directory)?;
                let source_parent = source
                    .rsplit_once('/')
                    .ok_or("compile-multi source has no parent")?
                    .0;
                let workdir = working_directory(directory);
                for path in [
                    source_parent.to_owned(),
                    include_path(&values["incextra"], directory)?,
                    source_directory(directory),
                    workdir.clone(),
                ] {
                    compile.extend(["-iquote".into(), path]);
                }
                compile.extend(flags(
                    &format!("{cflags} {safety} {} {extra}", values["cppflags"]),
                    dirs,
                    directory,
                )?);
                compile.push(format!("-D__SRCFILENAME__=\"{source_relative}\""));
                let mut dependency = flags(&sysroot, dirs, directory)?;
                dependency.extend(mkdep_flags(&flags(
                    &format!("{dflags} {} {extra}", values["cppflags"]),
                    dirs,
                    directory,
                )?)?);
                dependency.extend([
                    format!("-I{workdir}"),
                    format!("-I{}", source_directory(directory)),
                ]);
                objects.push(LiteralObjectDecl {
                    source,
                    output,
                    language: "C".into(),
                    arguments: compile,
                    dependency: Some(LiteralDependencyDecl {
                        output: format!("{targetdir}/{stem}.d"),
                        arguments: dependency,
                    }),
                    line: physical_line + 1,
                });
            }
            // `mmake` is a variable namespace, not an aggregate declaration.
            // Derive the owner only from the physical dependency selector and
            // independently prove its complete object prerequisite list.
            owner = prove_include(
                &objects,
                &invocations,
                &origins,
                positions
                    .as_deref()
                    .ok_or("physical include anchors are unavailable")?,
                &scope,
                &states,
                dirs,
                root,
                directory,
                contract,
            )?;
            let owner_line = prove_owner(
                &owner,
                &objects,
                &lines,
                &origins,
                positions
                    .as_deref()
                    .ok_or("physical owner anchors are unavailable")?,
                &scope,
                &states,
                dirs,
                root,
                directory,
            )?;
            Ok(LiteralObjectGroupDecl {
                owner: owner.clone(),
                file: recipe.to_string_lossy().into_owned(),
                line: owner_line,
                objects,
            })
        })();
        match result {
            Ok(group) => groups.push(group),
            Err(reason) => rejections.push(LiteralObjectRejection {
                owner,
                file: recipe.to_string_lossy().into_owned(),
                line: physical_line + 1,
                reason,
            }),
        }
    }
    // A failed competing declaration cannot leave the first claimant admitted.
    let refused: BTreeSet<_> = rejections
        .iter()
        .map(|rejection| rejection.owner.as_str())
        .collect();
    groups.retain(|group| !refused.contains(group.owner.as_str()));
    let mut claims = BTreeMap::<String, usize>::new();
    let mut conflicts = BTreeSet::new();
    for (index, group) in groups.iter().enumerate() {
        for object in &group.objects {
            for output in object.produced_outputs() {
                if let Some(previous) = claims.insert(output.to_ascii_lowercase(), index) {
                    conflicts.extend([previous, index]);
                }
            }
        }
    }
    groups = groups
        .into_iter()
        .enumerate()
        .filter_map(|(index, group)| {
            if conflicts.contains(&index) {
                rejections.push(LiteralObjectRejection {
                    owner: group.owner,
                    file: group.file,
                    line: group.line,
                    reason: "compile-multi outputs have competing declarations".into(),
                });
                None
            } else {
                Some(group)
            }
        })
        .collect();
    (groups, rejections)
}

fn expand(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    directory: &Path,
    line: usize,
) -> Result<String, String> {
    let context = MakeExprContext::new(scope, dirs, line, root, directory).without_filesystem();
    evaluate_make_expr(raw, &context)
        .map_err(|error| format!("compile-multi argument is unresolved: {error}"))
}

fn safe_relative(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value.contains('\\')
        || Path::new(value)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+/".contains(&byte))
    {
        return Err(format!(
            "compile-multi path is not closed and source-relative: {value}"
        ));
    }
    if value
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(format!("compile-multi path is not canonical: {value}"));
    }
    Ok(value.to_owned())
}

fn rooted(value: &str, prefix: &str) -> Result<String, String> {
    let tail = value
        .strip_prefix(&format!("{prefix}/"))
        .ok_or_else(|| format!("compile-multi path must be below {prefix}"))?;
    Ok(format!("{prefix}/{}", safe_relative(tail)?))
}

fn safe_owner(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "all" | "clean" | "setup" | "install" | "TMP")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn working_directory(directory: &Path) -> String {
    if directory.as_os_str().is_empty() {
        BUILD.into()
    } else {
        format!("{BUILD}/{}", directory.display())
    }
}

fn source_directory(directory: &Path) -> String {
    if directory.as_os_str().is_empty() {
        SOURCE.into()
    } else {
        format!("{SOURCE}/{}", directory.display())
    }
}

fn include_path(value: &str, directory: &Path) -> Result<String, String> {
    for root in [SOURCE, BUILD] {
        if value == root {
            return Ok(value.into());
        }
        if value.starts_with(&format!("{root}/")) {
            return rooted(value, root);
        }
    }
    if value == "." {
        return Ok(working_directory(directory));
    }
    Ok(format!(
        "{}/{}",
        working_directory(directory),
        safe_relative(value)?
    ))
}

fn flags(raw: &str, dirs: &DirVars, directory: &Path) -> Result<Vec<String>, String> {
    let words: Vec<_> = raw.split_whitespace().map(str::to_owned).collect();
    if words.len() > 512 {
        return Err("compile-multi flag vector exceeds its word bound".into());
    }
    let mut result = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let word = &words[index];
        if matches!(word.as_str(), "-I" | "-iquote") {
            let path = words
                .get(index + 1)
                .ok_or("compile-multi include flag has no path")?;
            result.extend([word.clone(), include_path(path, directory)?]);
            index += 2;
            continue;
        }
        if let Some(path) = word.strip_prefix("-I").filter(|path| !path.is_empty()) {
            result.push(format!("-I{}", include_path(path, directory)?));
            index += 1;
            continue;
        }
        crate::literal_objects::validate_flag_word(
            word,
            words.get(index + 1).map(String::as_str),
            dirs,
        )?;
        result.push(word.clone());
        if matches!(word.as_str(), "-D" | "-U") {
            let value = words
                .get(index + 1)
                .ok_or("compile-multi macro flag has no value")?;
            crate::literal_objects::validate_flag_value(word, value, dirs)?;
            result.push(value.clone());
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(result)
}

fn mkdep_flags(words: &[String]) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let word = &words[index];
        if matches!(word.as_str(), "-D" | "-I") {
            result.push(word.clone());
            result.push(
                words
                    .get(index + 1)
                    .ok_or("mkdep option has no paired argument")?
                    .clone(),
            );
            index += 2;
        } else {
            if word != "-g" && word != "-Wall" && !word.starts_with("-O") {
                result.push(word.clone());
            }
            index += 1;
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn check_scope(
    lines: &[&str],
    states: &[ConditionalTruth],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    directory: &Path,
    line: usize,
    namespace: &str,
) -> Result<(), String> {
    let mut bindings = BTreeMap::<&str, usize>::new();
    for (index, raw) in lines.iter().enumerate() {
        if states.get(index) == Some(&ConditionalTruth::False) || raw.starts_with('\t') {
            continue;
        }
        let text = strip_make_comment(raw).trim();
        if macro_invocations(text)
            .iter()
            .any(|call| !matches!(call.name.as_str(), "rule_compile_multi" | "include_deps"))
        {
            return Err("compile-multi scope contains an unproven adjacent GenMF macro".into());
        }
        if text.contains(".RECIPEPREFIX")
            || text.contains(".SECONDEXPANSION")
            || text.contains(".EXPORT_ALL_VARIABLES")
            || text.contains("$(eval")
            || text.contains("${eval")
            || text.starts_with("load ")
            || text.starts_with("-load ")
            || text.starts_with("%define ")
            || text.starts_with("%include ")
            || text.starts_with("define ")
            || text.starts_with("override define ")
            || text == "export"
            || text.starts_with("export ")
            || text == "unexport"
            || text.starts_with("unexport ")
            || text.starts_with("undefine ")
        {
            return Err(
                "compile-multi scope has opaque controls or template/compiler mutations".into(),
            );
        }
        if text
            .split_whitespace()
            .next()
            .is_some_and(|word| matches!(word, "include" | "-include" | "sinclude" | "vpath"))
        {
            return Err("compile-multi scope has an unresolved Make include".into());
        }
        if let Some((name, value, kind)) = variable_assignment(text) {
            if matches!(
                name,
                "CPATH"
                    | "C_INCLUDE_PATH"
                    | "CPLUS_INCLUDE_PATH"
                    | "OBJC_INCLUDE_PATH"
                    | "GCC_EXEC_PREFIX"
                    | "COMPILER_PATH"
                    | "LIBRARY_PATH"
                    | "PATH"
                    | "SDKROOT"
                    | "MACOSX_DEPLOYMENT_TARGET"
            ) {
                return Err(format!(
                    "compile-multi scope mutates compiler environment {name}"
                ));
            }
            if name == "CMD"
                || name.contains("_MC_") && !name.ends_with("_MC_EXTRA_CPPFLAGS")
                || ["CFLAGS", "DFLAGS", "IQUOTE", "IQUOTE_END"]
                    .iter()
                    .any(|suffix| name == format!("{namespace}_{suffix}"))
            {
                return Err(format!(
                    "compile-multi scope mutates template variable {name}"
                ));
            }
            let expected = match name {
                "TARGET_CC" => Some("$(NATIVE_TARGET_CC)"),
                "TARGET_SYSROOT" => Some("--sysroot=$(AROS_DEVELOPER)"),
                "CFLAGS_IQUOTE" => Some("-iquote"),
                "CFLAGS_IQUOTE_END" => Some(""),
                "MKDEPEND" => Some("$(SRCDIR)/scripts/mkdep"),
                "NATIVE_TARGET_CC" | "TOP" | "SRCDIR" | "CURDIR" | "GENDIR" => {
                    return Err(format!("compile-multi scope rebinds reserved role {name}"))
                }
                _ => None,
            };
            if let Some(expected) = expected {
                if states.get(index) != Some(&ConditionalTruth::True)
                    || value.trim() != expected
                    || !matches!(
                        kind,
                        crate::make_vars::AssignmentKind::RecursiveSet
                            | crate::make_vars::AssignmentKind::SimpleSet
                    )
                {
                    return Err(format!(
                        "compile-multi source role {name} is not explicitly bound"
                    ));
                }
                *bindings.entry(name).or_default() += 1;
            }
            continue;
        }
        if let Some((_, rhs)) = text.split_once(':') {
            if rhs.contains('=') {
                return Err("compile-multi scope has inherited target-specific assignments".into());
            }
        }
    }
    for name in [
        "TARGET_CC",
        "TARGET_SYSROOT",
        "CFLAGS_IQUOTE",
        "CFLAGS_IQUOTE_END",
        "MKDEPEND",
    ] {
        if bindings.get(name) != Some(&1) || scope.raw_at(name, line).is_none() {
            return Err(format!(
                "compile-multi needs one preceding source binding for {name}"
            ));
        }
    }
    // The compiler is an opaque admitted tool role, not a Make expression
    // whose deferred CMake variable may be evaluated as a file-local name.
    // Its single exact source alias and absence of rebindings were checked
    // above; do not invent an ambient compiler path.
    if dirs.expand("$(NATIVE_TARGET_CC)").as_deref() != Some("${CMAKE_C_COMPILER}") {
        return Err("compile-multi target compiler role was not admitted".into());
    }
    let expected = format!(
        "--sysroot={}",
        dirs.expand("$(AROS_DEVELOPER)")
            .ok_or("SDK sysroot role is unavailable")?
    );
    if expand("$(TARGET_SYSROOT)", scope, dirs, root, directory, line)? != expected {
        return Err("compile-multi source sysroot differs from the admitted SDK root".into());
    }
    Ok(())
}

fn source_basename(basename: &str, directory: &Path) -> Result<String, String> {
    if let Some(relative) = basename.strip_prefix(&format!("{SOURCE}/")) {
        safe_relative(relative)
    } else {
        Ok(directory
            .join(safe_relative(basename)?)
            .to_string_lossy()
            .into_owned())
    }
}

#[allow(clippy::too_many_arguments)]
fn source_search_directories(
    calls: &[crate::parser::Invocation],
    origins: &[usize],
    positions: &[Option<usize>],
    scope: &VarScope,
    states: &[ConditionalTruth],
    dirs: &DirVars,
    root: &Path,
    directory: &Path,
    contract: &CompileMultiTemplateContract,
) -> Result<Vec<String>, String> {
    let mut search = Vec::new();
    for call in calls
        .iter()
        .filter(|call| call.name == "rule_compile_multi")
    {
        let position = origins
            .get(call.line)
            .and_then(|physical| positions.get(*physical))
            .copied()
            .flatten()
            .ok_or("source lookup call lacks a physical scope anchor")?;
        if states.get(position) == Some(&ConditionalTruth::False) {
            continue;
        }
        if states.get(position) != Some(&ConditionalTruth::True) {
            return Err("source lookup has unresolved Make control".into());
        }
        let arguments = contract.arguments(&call.args)?;
        if expand(
            &arguments["usetree"],
            scope,
            dirs,
            root,
            directory,
            position,
        )? != "no"
            || !expand(&arguments["srcdir"], scope, dirs, root, directory, position)?.is_empty()
        {
            return Err("compile-multi source search contains a different pattern context".into());
        }
        rooted(
            &expand(
                &arguments["targetdir"],
                scope,
                dirs,
                root,
                directory,
                position,
            )?,
            GENERATED,
        )?;
        let basenames = expand(
            &arguments["basenames"],
            scope,
            dirs,
            root,
            directory,
            position,
        )?;
        // Each directive sorts its own basename directories; successive
        // directives with the same %.c pattern accumulate in source order.
        let mut sorted = BTreeMap::new();
        for basename in basenames.split_whitespace() {
            let relative = source_basename(basename, directory)?;
            let parent = Path::new(&relative)
                .parent()
                .ok_or("source basename has no parent")?
                .to_string_lossy()
                .into_owned();
            let make_parent = basename
                .rsplit_once('/')
                .map_or_else(|| "./".to_owned(), |(parent, _)| format!("{parent}/"));
            sorted.insert(make_parent, parent);
        }
        search.extend(sorted.into_values());
        if search.len() > MAX_OBJECTS {
            return Err("compile-multi source search exceeds its bound".into());
        }
    }
    Ok(search)
}

fn prove_source_lookup(
    root: &Path,
    directories: &[String],
    stem: &str,
    expected: &str,
) -> Result<(), String> {
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    for directory in directories {
        let relative = if directory.is_empty() {
            format!("{stem}.c")
        } else {
            format!("{directory}/{stem}.c")
        };
        let candidate = canonical_root.join(&relative);
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => {
                aros_common::open_regular_file_nofollow(&candidate)
                    .map_err(|error| format!("compile-multi vpath candidate is unsafe: {error}"))?;
                return if relative == expected {
                    Ok(())
                } else {
                    Err(format!(
                        "compile-multi vpath selects {relative} instead of declared {expected}"
                    ))
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => {
                return Err(format!(
                    "compile-multi vpath candidate cannot be inspected: {error}"
                ))
            }
        }
    }
    Err("compile-multi vpath does not resolve the declared source".into())
}

#[allow(clippy::too_many_arguments)]
fn prove_owner(
    owner: &str,
    objects: &[LiteralObjectDecl],
    lines: &[&str],
    origins: &[usize],
    positions: &[Option<usize>],
    scope: &VarScope,
    states: &[ConditionalTruth],
    dirs: &DirVars,
    root: &Path,
    directory: &Path,
) -> Result<usize, String> {
    let expected: Vec<_> = objects.iter().map(|object| object.output.clone()).collect();
    let mut definitions = Vec::new();
    let mut phony = false;
    for (line, raw) in lines.iter().enumerate() {
        if raw.starts_with('\t') || raw.trim_start().starts_with('#') {
            continue;
        }
        let text = strip_make_comment(raw).trim();
        let Some((lhs, rhs)) = text.split_once(':') else {
            continue;
        };
        let position = origins
            .get(line)
            .and_then(|physical| positions.get(*physical))
            .copied()
            .flatten()
            .ok_or("source rule lacks a physical scope anchor")?;
        if states.get(position) == Some(&ConditionalTruth::False) {
            continue;
        }
        let context =
            MakeExprContext::new(scope, dirs, position, root, directory).without_filesystem();
        if lhs.trim() == ".PHONY" {
            if states.get(position) == Some(&ConditionalTruth::True)
                && !rhs.contains([':', ';', '|', '='])
            {
                phony |= evaluate_make_list(rhs, &context)
                    .map_err(|error| error.to_string())?
                    .iter()
                    .any(|name| name == owner);
            }
            continue;
        }
        if lhs.contains('=') || rhs.starts_with('=') {
            continue;
        }
        let targets = evaluate_make_list(lhs, &context).map_err(|error| error.to_string())?;
        if targets.iter().any(|target| {
            expected.contains(target)
                || objects.iter().any(|object| {
                    object
                        .dependency
                        .as_ref()
                        .is_some_and(|dependency| dependency.output == *target)
                })
        }) || targets.iter().any(|target| target.contains('%'))
        {
            return Err("compile-multi outputs have an additional physical or pattern rule".into());
        }
        if !targets.iter().any(|target| target == owner) {
            continue;
        }
        if targets != [owner]
            || states.get(position) != Some(&ConditionalTruth::True)
            || rhs.contains([':', ';', '|', '='])
        {
            return Err(
                "compile-multi owner is conditional, multi-target or has unsupported rule effects"
                    .into(),
            );
        }
        let prerequisites = evaluate_make_list(rhs, &context).map_err(|error| error.to_string())?;
        if prerequisites != expected {
            return Err(
                "compile-multi owner does not name the complete ordered object list".into(),
            );
        }
        for next in &lines[line + 1..] {
            let next_text = next.trim();
            if next_text.is_empty() || next_text.starts_with('#') {
                continue;
            }
            if next.starts_with('\t') || next_text.starts_with('@') {
                return Err("compile-multi owner has a physical recipe".into());
            }
            break;
        }
        definitions.push(origins[line] + 1);
    }
    if definitions.len() != 1 || !phony {
        return Err(
            "compile-multi requires one explicit physical phony owner (fix upstream)".into(),
        );
    }
    Ok(definitions[0])
}

#[allow(clippy::too_many_arguments)]
fn prove_include(
    objects: &[LiteralObjectDecl],
    calls: &[crate::parser::Invocation],
    origins: &[usize],
    positions: &[Option<usize>],
    scope: &VarScope,
    states: &[ConditionalTruth],
    dirs: &DirVars,
    root: &Path,
    directory: &Path,
    contract: &CompileMultiTemplateContract,
) -> Result<String, String> {
    let expected: Vec<_> = objects
        .iter()
        .filter_map(|object| {
            object
                .dependency
                .as_ref()
                .map(|dependency| dependency.output.clone())
        })
        .collect();
    let mut owners = Vec::new();
    for call in calls.iter().filter(|call| call.name == "include_deps") {
        let position = origins
            .get(call.line)
            .and_then(|physical| positions.get(*physical))
            .copied()
            .flatten()
            .ok_or("include-deps has no physical scope anchor")?;
        if states.get(position) == Some(&ConditionalTruth::False) {
            continue;
        }
        let arguments = contract.include_arguments(&call.args)?;
        let targets = expand(
            &arguments["depstargets"],
            scope,
            dirs,
            root,
            directory,
            position,
        )?;
        let deps = expand(&arguments["deps"], scope, dirs, root, directory, position)?
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !deps.iter().any(|dep| expected.contains(dep)) {
            continue;
        }
        // The trusted template uses findstring, not token membership. Admit
        // only the exact selected owner, so partial-name activation is absent.
        if !safe_owner(&targets) {
            return Err("include-deps selector must name one exact compile-multi owner".into());
        }
        if states.get(position) != Some(&ConditionalTruth::True) {
            return Err("include-deps is in unresolved Make control".into());
        }
        if deps != expected {
            return Err("include-deps does not name the complete source depfile list".into());
        }
        owners.push(targets);
    }
    if owners.len() != 1 {
        return Err("compile-multi needs one exact source include-deps declaration".into());
    }
    Ok(owners.remove(0))
}

#[cfg(test)]
#[path = "compile_multi_tests.rs"]
mod tests;
