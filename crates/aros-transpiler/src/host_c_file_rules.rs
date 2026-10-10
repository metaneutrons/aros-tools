//! Closed proof for source-owned host-C generated build files.
//!
//! The declaration is deliberately only a description of inputs and an
//! invocation. This module admits it only when the source Makefiles contain
//! the matching, bounded producer chain and the host tool's own build rule.

use crate::genmodule_header_rules::{
    logical_lines, parse_rules, safe_relative_directory, safe_target_name, Rule,
};
use crate::make_vars::ConditionalTruth;
use aros_common::native_host_generator::NativeHostFileGenerator;
use std::fs;
use std::path::{Component, Path, PathBuf};

const STANDARD_C_HEADERS: &[&str] = &[
    "assert.h",
    "complex.h",
    "ctype.h",
    "errno.h",
    "fenv.h",
    "float.h",
    "inttypes.h",
    "iso646.h",
    "limits.h",
    "locale.h",
    "math.h",
    "setjmp.h",
    "signal.h",
    "stdalign.h",
    "stdarg.h",
    "stdatomic.h",
    "stdbool.h",
    "stddef.h",
    "stdint.h",
    "stdio.h",
    "stdlib.h",
    "stdnoreturn.h",
    "string.h",
    "tgmath.h",
    "threads.h",
    "time.h",
    "uchar.h",
    "wchar.h",
    "wctype.h",
];

/// Prove that one declaration describes the exact source Make producer chain.
///
/// `content` is the joined Makefile text used by the parser. The source-owned
/// `mmakefile.src` is also read from disk and checked, so an expanded or
/// synthetic caller buffer cannot stand in for the declared source file.
/// `line_states` uses zero-based physical lines in `content`.
pub(crate) fn validate_source_rule(
    content: &str,
    source_root: &Path,
    rel_dir: &Path,
    declaration: &NativeHostFileGenerator,
    line_states: Option<&[ConditionalTruth]>,
) -> Result<(), String> {
    let owner = &declaration.owner;
    if !safe_target_name(owner) {
        return Err(format!(
            "{owner}: host C generator owner is not a literal target"
        ));
    }

    let rel_dir_text =
        safe_relative_directory(rel_dir).map_err(|reason| format!("{owner}: {reason}"))?;
    let expected_recipe = if rel_dir_text.is_empty() {
        "mmakefile.src".to_owned()
    } else {
        format!("{rel_dir_text}/mmakefile.src")
    };
    if declaration.recipe != expected_recipe {
        return Err(format!(
            "{owner}: declared recipe {:?} does not match {expected_recipe:?}",
            declaration.recipe
        ));
    }

    let root = canonical_source_root(source_root).map_err(|reason| format!("{owner}: {reason}"))?;
    let recipe_text = read_source_file(&root, &declaration.recipe)
        .map_err(|reason| format!("{owner}: {reason}"))?;
    validate_declaration_paths(&rel_dir_text, declaration)
        .map_err(|reason| format!("{owner}: {reason}"))?;

    // Validate the supplied coordinate system first. This detects conditions
    // that the Make parser resolved as false/unknown and duplicate rules added
    // by a joined local fragment.
    let content_form = if crate::parser::join_continuations(content) == content {
        MakeTextForm::ParserJoined
    } else {
        // Unit callers may supply the source text directly. The actual parser
        // pipeline supplies join_continuations() output, and that form has its
        // own exact recipe spelling below.
        MakeTextForm::Raw
    };
    let joined_route = validate_make_chain(
        content,
        line_states,
        &rel_dir_text,
        declaration,
        content_form,
    )
    .map_err(|reason| format!("{owner}: {reason}"))?;
    // Then independently require the same literal rules in the source-owned
    // file. Source conditionals around these rules are intentionally rejected:
    // this module has no variable environment with which to evaluate them.
    let source_route = validate_make_chain(
        &recipe_text,
        None,
        &rel_dir_text,
        declaration,
        MakeTextForm::Raw,
    )
    .map_err(|reason| format!("{owner}: source mmakefile: {reason}"))?;
    if joined_route != source_route {
        return Err(format!(
            "{owner}: joined Make input and source mmakefile select different input routes"
        ));
    }

    validate_tool_chain(&root, declaration, source_route)
        .map_err(|reason| format!("{owner}: {reason}"))
}

fn validate_declaration_paths(
    rel_dir: &str,
    declaration: &NativeHostFileGenerator,
) -> Result<(), String> {
    derive_paths(rel_dir, declaration)?;
    if declaration.arguments.len() < 3
        || declaration.arguments[0] != "@INPUT_DIRECTORY@"
        || declaration.arguments[1] != "@OUTPUT_DIRECTORY@"
        || declaration
            .arguments
            .iter()
            .filter(|argument| argument.as_str() == "@INPUT_DIRECTORY@")
            .count()
            != 1
        || declaration
            .arguments
            .iter()
            .filter(|argument| argument.as_str() == "@OUTPUT_DIRECTORY@")
            .count()
            != 1
    {
        return Err("arguments must start with the input and output directory placeholders".into());
    }
    let mut input_names = std::collections::BTreeSet::new();
    for input in &declaration.inputs {
        if !safe_token(&input.filename) {
            return Err(format!(
                "declared input {:?} is not one safe basename",
                input.filename
            ));
        }
        if !input_names.insert(input.filename.to_ascii_lowercase()) {
            return Err(format!(
                "declared input {:?} is duplicated case-insensitively",
                input.filename
            ));
        }
    }
    validate_relative_file_path(&declaration.tool_source)
        .map_err(|reason| format!("tool source: {reason}"))?;
    validate_relative_file_path(&declaration.tool_recipe)
        .map_err(|reason| format!("tool recipe: {reason}"))?;
    let tool_source = Path::new(&declaration.tool_source);
    let tool_recipe = Path::new(&declaration.tool_recipe);
    if tool_source.extension().and_then(|value| value.to_str()) != Some("c")
        || tool_recipe.file_name().and_then(|value| value.to_str()) != Some("Makefile")
        || tool_source.parent() != tool_recipe.parent()
    {
        return Err("tool recipe must be Makefile beside one declared C source".into());
    }
    let source_stem = tool_source
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or("tool source has no UTF-8 basename")?;
    if !safe_token(source_stem) || declaration.tool_variable != source_stem.to_ascii_uppercase() {
        return Err("tool variable must be the uppercase C-source basename".into());
    }
    if !declaration
        .tool_variable
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err("tool variable is not one uppercase Make variable".into());
    }
    Ok(())
}

fn validate_make_chain(
    content: &str,
    line_states: Option<&[ConditionalTruth]>,
    rel_dir: &str,
    declaration: &NativeHostFileGenerator,
    text_form: MakeTextForm,
) -> Result<InputRoute, String> {
    let normalized = normalize_mmake_recipe_indentation(content);
    let rules = parse_rules(logical_lines(&normalized, line_states));
    let paths = derive_paths(rel_dir, declaration)?;
    let concrete_output = paths.concrete_output.as_str();
    let output_directory = paths.output_directory_expr.as_str();
    let tool_variable = format!("$({})", declaration.tool_variable);

    let owner = unique_rule(&rules, &declaration.owner)?;
    check_rule_state(owner)?;
    if owner.target.trim() != declaration.owner || owner.continued {
        return Err("owner must be one ordinary, uncontinued literal target".into());
    }
    let (normal, order_only) = prerequisites(&owner.prerequisites)?;
    if normal != [concrete_output] || !order_only.is_empty() || !owner.recipes.is_empty() {
        return Err("owner must depend on exactly the concrete output and have no recipe".into());
    }

    reject_direct_output_rule(&rules, concrete_output)?;

    let output_dir_rule = unique_rule(&rules, output_directory)?;
    check_rule_state(output_dir_rule)?;
    if output_dir_rule.target.trim() != output_directory
        || output_dir_rule.continued
        || !prerequisites(&output_dir_rule.prerequisites)?.0.is_empty()
        || !prerequisites(&output_dir_rule.prerequisites)?.1.is_empty()
    {
        return Err("output directory rule must have no prerequisites".into());
    }
    require_recipe_lines(output_dir_rule, &["%mkdirs_q $@"])?;

    let input_dir_rule = unique_rule(&rules, &paths.input_directory_expr)?;
    check_rule_state(input_dir_rule)?;
    if input_dir_rule.target.trim() != paths.input_directory_expr.as_str()
        || input_dir_rule.continued
        || !prerequisites(&input_dir_rule.prerequisites)?.0.is_empty()
        || !prerequisites(&input_dir_rule.prerequisites)?.1.is_empty()
    {
        return Err("input directory rule must have no prerequisites".into());
    }
    require_recipe_lines(input_dir_rule, &["%mkdirs_q $@"])?;

    let route = validate_input_route(&rules, declaration, &paths, &tool_variable, text_form)?;

    let generated = unique_rule(&rules, &paths.output_pattern)?;
    check_rule_state(generated)?;
    if generated.target.trim() != paths.output_pattern.as_str() || generated.continued {
        return Err(
            "generated file rule must match the declared output directory and suffix".into(),
        );
    }
    let (normal, order_only) = prerequisites(&generated.prerequisites)?;
    let mut expected_normal = vec![tool_variable.as_str()];
    let input_paths: Vec<String> = declaration
        .inputs
        .iter()
        .map(|input| format!("{}/{}", paths.input_directory_expr, input.filename))
        .collect();
    expected_normal.extend(input_paths.iter().map(String::as_str));
    if normal != expected_normal || order_only != [output_directory] {
        return Err(
            "generated C rule prerequisites differ from the declared tool and every sealed input"
                .into(),
        );
    }

    if declaration.arguments.get(2) != Some(&paths.output_stem) {
        return Err("declared literal stem differs from the concrete output basename".into());
    }
    let mut expected_invocation = vec![format!("@{tool_variable}")];
    expected_invocation.extend(declaration.arguments.iter().enumerate().map(
        |(index, argument)| match argument.as_str() {
            "@INPUT_DIRECTORY@" => paths.input_directory_expr.clone(),
            "@OUTPUT_DIRECTORY@" => paths.output_directory_expr.clone(),
            _ if index == 2 => "$*".to_owned(),
            _ => argument.clone(),
        },
    ));
    let expected_invocation = expected_invocation.join(" ");
    let recipes = normalized_recipe_lines(&generated.recipes)?;
    let expected_command = format!("{expected_invocation};");
    match recipes.as_slice() {
        [command] if command == &expected_command => {}
        [echo, command]
            if echo == &format!("@$(ECHO) \"Generating $*{}\";", paths.output_suffix)
                && command == &expected_command => {}
        _ => {
            return Err(
                "generated C rule must contain only the optional bounded echo and declared tool invocation"
                    .into(),
            )
        }
    }
    if paths.output_stem.is_empty()
        || !safe_token(&paths.output_stem)
        || !expected_invocation.contains(" $* ")
    {
        return Err("pattern stem does not match one safe concrete output stem".into());
    }

    // Reject source-local overrides of names that determine the modeled paths
    // and executable. The Makefile may use these variables, but this owner may
    // not silently redefine their meaning around the recipe.
    reject_local_variable_overrides(content, &declaration.tool_variable, route)?;
    Ok(route)
}

fn normalize_mmake_recipe_indentation(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            if line.starts_with(' ') {
                format!("\t{}", line.trim_start())
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputRoute {
    CopyPattern,
    DelegatedToolMake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MakeTextForm {
    Raw,
    ParserJoined,
}

fn validate_input_route(
    rules: &[Rule],
    declaration: &NativeHostFileGenerator,
    paths: &DerivedPaths,
    tool_variable: &str,
    text_form: MakeTextForm,
) -> Result<InputRoute, String> {
    let copy_rule_present = paths.input_copy_pattern.as_ref().is_some_and(|target| {
        rules.iter().any(|rule| {
            rule.state != ConditionalTruth::False
                && rule
                    .target
                    .split_whitespace()
                    .any(|candidate| candidate == target)
        })
    });
    let input_targets: Vec<_> = declaration
        .inputs
        .iter()
        .map(|input| format!("{}/{}", paths.input_directory_expr, input.filename))
        .collect();
    let delegated_rule_present = input_targets.iter().any(|target| {
        rules.iter().any(|rule| {
            rule.state != ConditionalTruth::False
                && rule
                    .target
                    .split_whitespace()
                    .any(|candidate| candidate == target)
        })
    });

    match (copy_rule_present, delegated_rule_present) {
        (true, true) => Err("declared inputs have both copy-pattern and delegated Make producers".into()),
        (true, false) => {
            let target = paths
                .input_copy_pattern
                .as_deref()
                .expect("copy pattern presence was established");
            let input_rule = unique_rule(rules, target)?;
            check_rule_state(input_rule)?;
            if input_rule.target.trim() != target || input_rule.continued {
                return Err("input copy rule must match the declared directory and suffix".into());
            }
            let (normal, order_only) = prerequisites(&input_rule.prerequisites)?;
            if normal != [paths.input_source_pattern.as_deref().unwrap_or_default()]
                || order_only != [paths.input_directory_expr.as_str()]
            {
                return Err("input copy rule has changed source or directory prerequisites".into());
            }
            require_recipe_lines(input_rule, &["@$(CP) $< $@"])?;
            Ok(InputRoute::CopyPattern)
        }
        (false, true) => {
            validate_delegated_source_input_chain(
                rules,
                declaration,
                paths,
                tool_variable,
                &input_targets,
                text_form,
            )?;
            Ok(InputRoute::DelegatedToolMake)
        }
        (false, false) => Err(
            "declared inputs have neither the bounded copy-pattern nor delegated Make producer chain"
                .into(),
        ),
    }
}

fn validate_delegated_source_input_chain(
    rules: &[Rule],
    declaration: &NativeHostFileGenerator,
    paths: &DerivedPaths,
    tool_variable: &str,
    input_targets: &[String],
    text_form: MakeTextForm,
) -> Result<(), String> {
    if input_targets.len() < 2 {
        return Err("delegated input chain requires at least two declared inputs".into());
    }
    let tool_directory = Path::new(&declaration.tool_recipe)
        .parent()
        .and_then(Path::to_str)
        .ok_or("tool recipe has no source directory")?;
    let submake =
        format!("$(MAKE) $(MKARGS) -C $(SRCDIR)/{tool_directory} SRCDIR=$(SRCDIR) TOP=$(TOP) all");
    let first = unique_rule(rules, &input_targets[0])?;
    check_rule_state(first)?;
    if first.target.trim() != input_targets[0] || first.continued {
        return Err("first delegated input rule must be one uncontinued literal target".into());
    }
    let (normal, order_only) = prerequisites(&first.prerequisites)?;
    if normal != [tool_variable] || order_only != [paths.input_directory_expr.as_str()] {
        return Err(
            "first delegated input must depend on the host tool and input directory".into(),
        );
    }
    require_recipe_lines(first, &[&format!("@{submake}"), "@test -s \"$@\""])?;

    for index in 1..input_targets.len() {
        let rule = unique_rule(rules, &input_targets[index])?;
        check_rule_state(rule)?;
        if rule.target.trim() != input_targets[index] || rule.continued {
            return Err(format!(
                "delegated input {} must be one uncontinued literal target",
                declaration.inputs[index].filename
            ));
        }
        let (normal, order_only) = prerequisites(&rule.prerequisites)?;
        if normal != [input_targets[index - 1].as_str()] || !order_only.is_empty() {
            return Err(format!(
                "delegated input {} must depend only on its preceding declared input",
                declaration.inputs[index].filename
            ));
        }
        let guard_recipe = match text_form {
            MakeTextForm::Raw => (
                format!("@if ! test -s \"$@\"; then \t    {submake}; fi"),
                true,
            ),
            MakeTextForm::ParserJoined => {
                let raw = format!("\t@if ! test -s \"$@\"; then \\\n\t    {submake}; \\\n\tfi");
                (
                    crate::parser::join_continuations(&raw).trim().to_owned(),
                    false,
                )
            }
        };
        require_recipe_lines_with_continuations(
            rule,
            &[guard_recipe, ("@test -s \"$@\"".to_owned(), false)],
        )?;
    }
    Ok(())
}

#[derive(Debug)]
struct DerivedPaths {
    input_directory_expr: String,
    input_copy_pattern: Option<String>,
    input_source_pattern: Option<String>,
    output_directory_expr: String,
    output_pattern: String,
    output_suffix: String,
    output_stem: String,
    concrete_output: String,
}

fn derive_paths(
    rel_dir: &str,
    declaration: &NativeHostFileGenerator,
) -> Result<DerivedPaths, String> {
    let input_suffix = declaration
        .input_directory
        .strip_prefix("gen/")
        .ok_or("input directory must be below the generated build root")?;
    validate_relative_suffix(input_suffix, "input directory")?;
    let input_directory_expr = format!("$(GENDIR)/{input_suffix}");
    let (input_copy_pattern, input_source_pattern) =
        uniform_extension(&declaration.inputs).map_or((None, None), |extension| {
            (
                Some(format!("{input_directory_expr}/%{extension}")),
                Some(format!("$(PORTSSOURCEDIR)/%{extension}")),
            )
        });

    let local_output_prefix = if rel_dir.is_empty() {
        "gen/".to_owned()
    } else {
        format!("gen/{rel_dir}/")
    };
    let output_suffix_path = declaration
        .output
        .strip_prefix(&local_output_prefix)
        .ok_or_else(|| {
            format!(
                "output {:?} must be below the declaring directory",
                declaration.output
            )
        })?;
    validate_relative_suffix(output_suffix_path, "output")?;
    let output_path = Path::new(output_suffix_path);
    let output_leaf = output_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("generated output has no UTF-8 filename")?;
    let output_stem = output_path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or("generated output has no UTF-8 stem")?;
    if !safe_token(output_leaf) || !safe_token(output_stem) {
        return Err("generated output has an unsafe filename".into());
    }
    let output_suffix = file_suffix(output_leaf);
    let output_parent = output_path
        .parent()
        .and_then(Path::to_str)
        .filter(|parent| !parent.is_empty())
        .unwrap_or("");
    if !output_parent.is_empty() {
        validate_relative_suffix(output_parent, "output directory")?;
    }
    let output_directory_expr = if output_parent.is_empty() {
        "$(GENDIR)/$(CURDIR)".to_owned()
    } else {
        format!("$(GENDIR)/$(CURDIR)/{output_parent}")
    };
    let output_pattern = format!("{output_directory_expr}/%{output_suffix}");
    let concrete_output = format!("{output_directory_expr}/{output_leaf}");

    Ok(DerivedPaths {
        input_directory_expr,
        input_copy_pattern,
        input_source_pattern,
        output_directory_expr,
        output_pattern,
        output_suffix,
        output_stem: output_stem.to_owned(),
        concrete_output,
    })
}

fn uniform_extension(
    inputs: &[aros_common::native_host_generator::NativeHostFileInput],
) -> Option<String> {
    let first = inputs.first()?;
    let suffix = file_suffix(&first.filename);
    if inputs
        .iter()
        .any(|input| file_suffix(&input.filename) != suffix)
    {
        return None;
    }
    Some(suffix)
}

fn file_suffix(filename: &str) -> String {
    filename
        .rfind('.')
        .filter(|index| *index > 0)
        .map_or_else(String::new, |index| filename[index..].to_owned())
}

fn validate_relative_suffix(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains(['\\', '$', ';', '\n', '\r', ':'])
        || Path::new(value)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || value.split('/').any(|component| !safe_token(component))
    {
        return Err(format!(
            "{label} path {value:?} is not a safe relative path"
        ));
    }
    Ok(())
}

fn unique_rule<'a>(rules: &'a [Rule], target: &str) -> Result<&'a Rule, String> {
    let matches: Vec<_> = rules
        .iter()
        .filter(|rule| {
            rule.state != ConditionalTruth::False
                && rule
                    .target
                    .split_whitespace()
                    .any(|candidate| candidate == target)
        })
        .collect();
    match matches.as_slice() {
        [rule] => Ok(rule),
        [] => Err(format!("missing required Make rule for {target:?}")),
        _ => Err(format!("duplicate Make rules for {target:?}")),
    }
}

fn reject_direct_output_rule(rules: &[Rule], concrete_output: &str) -> Result<(), String> {
    if rules.iter().any(|rule| {
        rule.state != ConditionalTruth::False
            && rule
                .target
                .split_whitespace()
                .any(|target| target == concrete_output)
    }) {
        return Err("concrete output also has a separate explicit producer rule".into());
    }
    Ok(())
}

fn check_rule_state(rule: &Rule) -> Result<(), String> {
    if rule.state != ConditionalTruth::True
        || rule.conditional_syntax
        || rule
            .recipes
            .iter()
            .any(|recipe| recipe.state != ConditionalTruth::True || recipe.conditional_syntax)
    {
        return Err(format!(
            "rule at source line {} is conditional or unresolved",
            rule.line + 1
        ));
    }
    Ok(())
}

fn prerequisites(value: &str) -> Result<(Vec<&str>, Vec<&str>), String> {
    let mut sections = value.split('|');
    let normal = sections
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let order_only = sections
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    if sections.next().is_some() {
        return Err("Make rule has more than one order-only separator".into());
    }
    Ok((normal, order_only))
}

fn require_recipe_lines(rule: &Rule, expected: &[&str]) -> Result<(), String> {
    let actual = normalized_recipe_lines(&rule.recipes)?;
    if actual.iter().map(String::as_str).collect::<Vec<_>>() != expected {
        return Err(format!(
            "rule for {:?} has commands outside the closed recipe",
            rule.target.trim()
        ));
    }
    Ok(())
}

fn require_recipe_lines_with_continuations(
    rule: &Rule,
    expected: &[(String, bool)],
) -> Result<(), String> {
    if rule.recipes.len() != expected.len()
        || rule
            .recipes
            .iter()
            .zip(expected)
            .any(|(actual, (text, continued))| {
                actual.text.trim() != text || actual.continued != *continued
            })
    {
        return Err(format!(
            "rule for {:?} has commands outside the exact continued recipe: actual {:?}, expected {:?}",
            rule.target.trim(),
            rule.recipes
                .iter()
                .map(|recipe| (&recipe.text, recipe.continued))
                .collect::<Vec<_>>(),
            expected
        ));
    }
    Ok(())
}

fn normalized_recipe_lines(
    recipes: &[crate::genmodule_header_rules::RecipeLine],
) -> Result<Vec<String>, String> {
    if recipes.is_empty() || recipes.iter().any(|recipe| recipe.continued) {
        return Err("required recipe is missing or continued".into());
    }
    Ok(recipes
        .iter()
        .map(|recipe| recipe.text.trim().to_owned())
        .collect())
}

fn reject_local_variable_overrides(
    content: &str,
    tool_variable: &str,
    input_route: InputRoute,
) -> Result<(), String> {
    let mut names = vec![
        "GENDIR",
        "CURDIR",
        "PORTSSOURCEDIR",
        "CP",
        "ECHO",
        tool_variable,
    ];
    if input_route == InputRoute::DelegatedToolMake {
        names.extend([
            "MAKE",
            "MKARGS",
            "SRCDIR",
            "TOP",
            "GENCTBL_UCD_VERSION",
            "GENCTBL_UCD_SHA256",
            "GENCTBL_UCD_READY",
            "FETCH",
        ]);
    }
    reject_dynamic_make_rebindings(content, &names, &[], "source mmakefile")?;
    Ok(())
}

const MAKE_ASSIGNMENT_OPERATORS: &[&str] = &[":::=", "::=", ":=", "?=", "+=", "!=", "="];
const MAKE_ASSIGNMENT_MODIFIERS: &[&str] = &["override", "export", "private", "unexport"];

struct MakeAssignment<'a> {
    name: &'a str,
    operator: &'a str,
    value: &'a str,
    modified: bool,
    target_specific: bool,
}

fn parse_make_assignment(line: &str) -> Option<MakeAssignment<'_>> {
    let line = line.trim_start();
    if line.starts_with('#') || line.starts_with('\t') {
        return None;
    }
    let (at, operator) = make_assignment_operator(line)?;
    let left = line[..at].trim();
    let (left, target_specific) = left
        .rsplit_once(':')
        .map_or((left, false), |(_, scoped)| (scoped.trim(), true));
    let mut words = left.split_whitespace();
    let mut modified = false;
    let mut name = words.next()?;
    while MAKE_ASSIGNMENT_MODIFIERS.contains(&name) {
        modified = true;
        name = words.next()?;
    }
    if words.next().is_some() {
        return None;
    }
    Some(MakeAssignment {
        name,
        operator,
        value: line[at + operator.len()..].trim(),
        modified,
        target_specific,
    })
}

fn reject_dynamic_make_rebindings(
    content: &str,
    protected: &[&str],
    canonical_assignments: &[&str],
    recipe_label: &str,
) -> Result<(), String> {
    reject_computed_make_assignment_names(content, recipe_label)?;
    // The closed rule parser recognizes only GNU make's ordinary tab recipe
    // prefix. A source-selected prefix would turn skipped recipe text into
    // Make syntax and invalidate every assignment/directive check below.
    let is_protected = |name: &str| name == ".RECIPEPREFIX" || protected.contains(&name);
    for (line_number, logical_line) in make_logical_nonrecipe_lines(content) {
        let line = make_text_before_comment(&logical_line).trim();
        if line.is_empty() {
            continue;
        }
        // GNU make expands eval text as new makefile syntax. Its assignment
        // target can be constructed dynamically, so this bounded proof rejects
        // eval rather than attempting to infer the resulting variable writes.
        if contains_eval_function(line) {
            return Err(format!(
                "{recipe_label} line {} contains an unbounded eval expansion",
                line_number + 1
            ));
        }
        if parse_make_assignment(line).is_some_and(|assignment| assignment.name.contains('$')) {
            return Err(format!(
                "{recipe_label} line {} uses a computed Make assignment name",
                line_number + 1
            ));
        }
        if let Some(assignment) = parse_make_assignment(line) {
            if is_protected(assignment.name) {
                if assignment.modified || assignment.target_specific {
                    return Err(format!(
                        "{recipe_label} line {} uses Make modifiers or target scope for protected variable {}",
                        line_number + 1,
                        assignment.name
                    ));
                }
                if !canonical_assignments.contains(&assignment.name) {
                    return Err(format!(
                        "{recipe_label} line {} locally assigns protected Make variable {}",
                        line_number + 1,
                        assignment.name
                    ));
                }
            }
        }
        if let Some((kind, name, dynamic)) = make_variable_directive(line) {
            if kind == "define" {
                return Err(format!(
                    "{recipe_label} line {} uses a define block outside the closed Make capability",
                    line_number + 1
                ));
            }
            if dynamic || is_protected(name) {
                return Err(format!(
                    "{recipe_label} line {} uses {kind} for a protected or dynamic Make variable",
                    line_number + 1
                ));
            }
        }
        if let Some((kind, names, dynamic)) = make_export_directive(line) {
            if dynamic || names.iter().any(|name| is_protected(name)) {
                return Err(format!(
                    "{recipe_label} line {} uses {kind} for a protected or dynamic Make variable",
                    line_number + 1
                ));
            }
        }
    }
    Ok(())
}

fn reject_computed_make_assignment_names(content: &str, recipe_label: &str) -> Result<(), String> {
    for (line_number, line) in make_logical_nonrecipe_lines(content) {
        let line = make_text_before_comment(&line).trim();
        let Some((operator_at, _)) = make_assignment_operator(line) else {
            continue;
        };
        let left = &line[..operator_at];
        let variable_part = last_top_level_colon(left).map_or(left, |colon| &left[colon + 1..]);
        let variable_part = strip_make_assignment_modifiers(variable_part);
        if variable_part.contains('$') {
            return Err(format!(
                "{recipe_label} line {} uses a computed Make assignment name",
                line_number + 1
            ));
        }
    }
    Ok(())
}

fn make_logical_nonrecipe_lines(content: &str) -> Vec<(usize, String)> {
    let physical: Vec<_> = content.lines().collect();
    let mut output = Vec::new();
    let mut index = 0usize;
    while index < physical.len() {
        let first_line = index;
        let is_recipe = physical[index].starts_with('\t');
        let mut logical = String::new();
        let mut continued = false;
        loop {
            let line = physical[index].trim_end_matches('\r');
            let trimmed = line.trim_end();
            let trailing_backslashes = trimmed
                .as_bytes()
                .iter()
                .rev()
                .take_while(|byte| **byte == b'\\')
                .count();
            if trailing_backslashes % 2 == 1 {
                let prefix = &trimmed[..trimmed.len() - 1];
                if continued {
                    logical.push_str(prefix.trim_start().trim_end());
                } else {
                    logical.push_str(prefix.trim_end());
                }
                logical.push(' ');
                continued = true;
                index += 1;
                if index == physical.len() {
                    break;
                }
                continue;
            }
            if continued {
                logical.push_str(line.trim());
            } else {
                logical.push_str(line);
            }
            index += 1;
            break;
        }
        if !is_recipe {
            output.push((first_line, logical));
        }
    }
    output
}

fn strip_make_assignment_modifiers(value: &str) -> &str {
    let mut remaining = value.trim_start();
    loop {
        let Some((first, rest)) = remaining.split_once(char::is_whitespace) else {
            return remaining;
        };
        if !MAKE_ASSIGNMENT_MODIFIERS.contains(&first) {
            return remaining;
        }
        remaining = rest.trim_start();
    }
}

fn make_assignment_operator(line: &str) -> Option<(usize, &'static str)> {
    let bytes = line.as_bytes();
    let mut nesting = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'$'
            && index + 1 < bytes.len()
            && matches!(bytes[index + 1], b'(' | b'{')
        {
            nesting.push(if bytes[index + 1] == b'(' { b')' } else { b'}' });
            index += 2;
            continue;
        }
        if let Some(close) = nesting.last().copied() {
            if bytes[index] == b'(' {
                nesting.push(b')');
            } else if bytes[index] == b'{' {
                nesting.push(b'}');
            } else if bytes[index] == close {
                nesting.pop();
            }
            index += 1;
            continue;
        }
        if let Some(operator) = MAKE_ASSIGNMENT_OPERATORS
            .iter()
            .find(|operator| line[index..].starts_with(**operator))
        {
            return Some((index, operator));
        }
        index += 1;
    }
    None
}

fn last_top_level_colon(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut nesting = Vec::new();
    let mut last = None;
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'$'
            && index + 1 < bytes.len()
            && matches!(bytes[index + 1], b'(' | b'{')
        {
            nesting.push(if bytes[index + 1] == b'(' { b')' } else { b'}' });
            index += 2;
            continue;
        }
        if let Some(close) = nesting.last().copied() {
            if bytes[index] == b'(' {
                nesting.push(b')');
            } else if bytes[index] == b'{' {
                nesting.push(b'}');
            } else if bytes[index] == close {
                nesting.pop();
            }
        } else if bytes[index] == b':' {
            last = Some(index);
        }
        index += 1;
    }
    last
}

fn make_text_before_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'#' {
            continue;
        }
        let mut slashes = 0;
        let mut previous = index;
        while previous > 0 && bytes[previous - 1] == b'\\' {
            slashes += 1;
            previous -= 1;
        }
        if slashes % 2 == 0 {
            return &line[..index];
        }
    }
    line
}

fn make_variable_directive(line: &str) -> Option<(&'static str, &str, bool)> {
    let mut words = line.split_whitespace();
    let mut directive = words.next()?;
    while MAKE_ASSIGNMENT_MODIFIERS.contains(&directive) {
        directive = words.next()?;
    }
    if directive != "define" && directive != "undefine" {
        return None;
    }
    let name = words.next()?;
    Some((
        if directive == "define" {
            "define"
        } else {
            "undefine"
        },
        name,
        name.contains('$'),
    ))
}

fn make_export_directive(line: &str) -> Option<(&'static str, Vec<&str>, bool)> {
    let mut words = line.split_whitespace();
    let directive = words.next()?;
    if directive != "export" && directive != "unexport" {
        return None;
    }
    // `export NAME = value` is an assignment and is parsed by
    // `parse_make_assignment`; here only the bare export directives matter.
    let names: Vec<_> = words.collect();
    if names
        .iter()
        .any(|word| MAKE_ASSIGNMENT_OPERATORS.contains(word))
    {
        return None;
    }
    Some((
        if directive == "export" {
            "export"
        } else {
            "unexport"
        },
        names.clone(),
        names.iter().any(|name| name.contains('$')),
    ))
}

fn contains_eval_function(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'$' && matches!(bytes[index + 1], b'(' | b'{') {
            let mut name_start = index + 2;
            while bytes.get(name_start).is_some_and(u8::is_ascii_whitespace) {
                name_start += 1;
            }
            if bytes.get(name_start..name_start + 4) == Some(b"eval")
                && bytes
                    .get(name_start + 4)
                    .is_none_or(|byte| byte.is_ascii_whitespace() || matches!(byte, b')' | b'}'))
            {
                return true;
            }
        }
        index += 1;
    }
    false
}

fn validate_tool_chain(
    source_root: &Path,
    declaration: &NativeHostFileGenerator,
    input_route: InputRoute,
) -> Result<(), String> {
    let tool_makefile = read_source_file(source_root, &declaration.tool_recipe)?;
    let protected = [
        declaration.tool_variable.as_str(),
        "USER_CFLAGS",
        "HOST_CFLAGS",
        "GENCTBL_UCD_VERSION",
        "GENCTBL_UCD_SHA256",
        "GENCTBL_UCD_READY",
        "FETCH",
        "MAKE",
        "MKARGS",
        "SRCDIR",
        "TOP",
        "PORTSSOURCEDIR",
        "GENINCDIR",
        "ECHO",
    ];
    let mut canonical_assignments = vec![
        declaration.tool_variable.as_str(),
        "USER_CFLAGS",
        "HOST_CFLAGS",
    ];
    if input_route == InputRoute::DelegatedToolMake {
        canonical_assignments.extend([
            "GENCTBL_UCD_VERSION",
            "GENCTBL_UCD_SHA256",
            "GENCTBL_UCD_READY",
            "FETCH",
        ]);
    }
    reject_dynamic_make_rebindings(
        &tool_makefile,
        &protected,
        &canonical_assignments,
        "host tool Makefile",
    )?;
    let tool_source = read_source_file(source_root, &declaration.tool_source)?;
    let standard_headers = standard_headers_in_c_source(&tool_source)?;
    reject_source_local_header_shadows(source_root, &declaration.tool_source, &standard_headers)?;

    let source_leaf = Path::new(&declaration.tool_source)
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("tool source filename is not UTF-8")?;
    let source_stem = Path::new(source_leaf)
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or("tool source stem is not UTF-8")?;
    let tool_default = unique_make_assignment(&tool_makefile, &declaration.tool_variable, "?=")?;
    require_unconditional_assignment(
        &tool_makefile,
        &declaration.tool_variable,
        "?=",
        &tool_default,
    )?;
    if tool_default != source_stem {
        return Err("host tool variable default must be its declared C-source basename".into());
    }
    let expected_target = format!("$({})", declaration.tool_variable);
    let tool_rules = parse_rules(logical_lines(&tool_makefile, None));
    let compile_rule = unique_rule(&tool_rules, &expected_target)?;
    check_rule_state(compile_rule)?;
    if compile_rule.target.trim() != expected_target || compile_rule.continued {
        return Err("host tool target must be one uncontinued tool-variable target".into());
    }
    let (normal, order_only) = prerequisites(&compile_rule.prerequisites)?;
    let tool_makefile_dependency = format!("$(SRCDIR)/{}", declaration.tool_recipe);
    let expected_compile_prerequisites = [
        source_leaf,
        tool_makefile_dependency.as_str(),
        "$(GENMODULE_DEPS)",
    ];
    let legacy_compile_prerequisites = [source_leaf, "$(GENMODULE_DEPS)"];
    let compile_prerequisites_match = match input_route {
        InputRoute::CopyPattern => {
            normal == expected_compile_prerequisites || normal == legacy_compile_prerequisites
        }
        InputRoute::DelegatedToolMake => normal == expected_compile_prerequisites,
    };
    if !compile_prerequisites_match || !order_only.is_empty() {
        return Err(
            "host tool prerequisites differ from its C source, declared Makefile, and Make dependencies"
                .into(),
        );
    }
    let compile_echo = "@$(ECHO) \"Compiling $(notdir $@)...\"";
    let compile_command = format!(
        "@$(HOST_CC) -g $(HOST_CFLAGS) -I$(GENINCDIR) -I$(TOP)/$(CURDIR) {source_stem}.c -o $@"
    );
    require_recipe_lines(compile_rule, &[compile_echo, &compile_command])?;

    let user_flags = unique_make_assignment(&tool_makefile, "USER_CFLAGS", ":=")?;
    let host_flags = unique_make_assignment(&tool_makefile, "HOST_CFLAGS", "?=")?;
    require_unconditional_assignment(&tool_makefile, "USER_CFLAGS", ":=", &user_flags)?;
    require_unconditional_assignment(&tool_makefile, "HOST_CFLAGS", "?=", &host_flags)?;
    if host_flags != "$(USER_CFLAGS)" {
        return Err("HOST_CFLAGS must default to USER_CFLAGS".into());
    }
    let mut expected_flags = vec!["-g".to_owned()];
    expected_flags.extend(user_flags.split_whitespace().map(str::to_owned));
    if declaration.compile_flags != expected_flags {
        return Err(format!(
            "declared compiler flags {:?} do not match the source recipe {:?}",
            declaration.compile_flags, expected_flags
        ));
    }

    if input_route == InputRoute::DelegatedToolMake {
        validate_delegated_tool_data_chain(&tool_makefile, declaration)?;
    }
    validate_top_level_tool_rule(source_root, declaration, input_route)?;
    validate_configure_tool_variable(source_root, declaration, source_stem)?;
    Ok(())
}

fn unique_make_assignment(
    content: &str,
    name: &str,
    expected_operator: &str,
) -> Result<String, String> {
    let assignments: Vec<_> = make_logical_nonrecipe_lines(content)
        .into_iter()
        .filter_map(|(_, logical_line)| {
            let line = make_text_before_comment(&logical_line).trim().to_owned();
            let assignment = parse_make_assignment(&line)?;
            (assignment.name == name).then(|| {
                (
                    assignment.operator.to_owned(),
                    assignment.value.to_owned(),
                    assignment.modified,
                    assignment.target_specific,
                )
            })
        })
        .collect();
    match assignments.as_slice() {
        [assignment]
            if !assignment.2 && !assignment.3 && assignment.0 == expected_operator =>
        {
            Ok(assignment.1.clone())
        }
        [assignment] if assignment.2 || assignment.3 => Err(format!(
            "{name} assignment uses Make modifiers or target scope that are outside the closed recipe"
        )),
        [assignment] => Err(format!(
            "{name} assignment uses {}, expected {expected_operator}",
            assignment.0
        )),
        [] => Err(format!("tool Makefile has no {name} assignment")),
        _ => Err(format!("tool Makefile has duplicate {name} assignments")),
    }
}

fn validate_delegated_tool_data_chain(
    tool_makefile: &str,
    declaration: &NativeHostFileGenerator,
) -> Result<(), String> {
    let (data_directory, version, archive) = archive_input_identity(declaration)?;
    let version_value = unique_make_assignment(tool_makefile, "GENCTBL_UCD_VERSION", ":=")?;
    if version_value != version {
        return Err(format!(
            "GENCTBL_UCD_VERSION {version_value:?} differs from the sealed input URL version {version:?}"
        ));
    }
    require_unconditional_assignment(tool_makefile, "GENCTBL_UCD_VERSION", ":=", &version)?;

    let archive_hash = unique_make_assignment(tool_makefile, "GENCTBL_UCD_SHA256", ":=")?;
    if !valid_sha256(&archive_hash) {
        return Err("GENCTBL_UCD_SHA256 is not one 64-character hexadecimal digest".into());
    }
    require_unconditional_assignment(tool_makefile, "GENCTBL_UCD_SHA256", ":=", &archive_hash)?;

    let ready_value = format!("$(GENDIR)/{data_directory}/.ucd-$(GENCTBL_UCD_VERSION)-ready");
    if unique_make_assignment(tool_makefile, "GENCTBL_UCD_READY", ":=")? != ready_value {
        return Err(
            "GENCTBL_UCD_READY differs from the declared input directory and version".into(),
        );
    }
    require_unconditional_assignment(tool_makefile, "GENCTBL_UCD_READY", ":=", &ready_value)?;
    if unique_make_assignment(tool_makefile, "FETCH", "?=")? != "$(SRCDIR)/scripts/fetch.sh" {
        return Err("FETCH must use the source tree's canonical fetch script".into());
    }
    require_unconditional_assignment(tool_makefile, "FETCH", "?=", "$(SRCDIR)/scripts/fetch.sh")?;

    let input_directory = format!("$(GENDIR)/{data_directory}");
    let ports_source_directory = "$(PORTSSOURCEDIR)";
    let tool_makefile_dependency = format!("$(SRCDIR)/{}", declaration.tool_recipe);
    let tool_rules = parse_rules(logical_lines(tool_makefile, None));
    let ready_target = "$(GENCTBL_UCD_READY)";
    let ready_rule = unique_rule(&tool_rules, ready_target)?;

    let all_rule = unique_rule(&tool_rules, "all")?;
    check_rule_state(all_rule)?;
    if all_rule.target.trim() != "all" || all_rule.continued || !all_rule.recipes.is_empty() {
        return Err("host tool all target must be one ordinary dependency-only rule".into());
    }
    let input_targets: Vec<String> = declaration
        .inputs
        .iter()
        .map(|input| format!("{input_directory}/{}", input.filename))
        .collect();
    let mut expected_all = vec![format!("$({})", declaration.tool_variable)];
    expected_all.extend(input_targets.iter().cloned());
    let (normal, order_only) = prerequisites(&all_rule.prerequisites)?;
    if normal != expected_all.iter().map(String::as_str).collect::<Vec<_>>()
        || !order_only.is_empty()
    {
        return Err(
            "host tool all target must depend on its executable and every declared input".into(),
        );
    }

    validate_tool_mkdir_rule(&tool_rules, &input_directory)?;
    validate_tool_mkdir_rule(&tool_rules, ports_source_directory)?;

    check_rule_state(ready_rule)?;
    if ready_rule.target.trim() != ready_target || ready_rule.continued {
        return Err("UCD archive ready target must be one literal tool variable".into());
    }
    let (normal, order_only) = prerequisites(&ready_rule.prerequisites)?;
    if normal != [tool_makefile_dependency.as_str()]
        || order_only != [input_directory.as_str(), ports_source_directory]
    {
        return Err("UCD archive ready rule has changed source or directory dependencies".into());
    }
    let archive_base =
        format!("https://www.unicode.org/Public/$(GENCTBL_UCD_VERSION)/{data_directory}");
    let archive_name = format!("{archive}.zip");
    let fetch_command = format!(
        "@$(FETCH) -ao \"{archive_base}\" \t    -a {archive} -s zip -l \"$(PORTSSOURCEDIR)\" -d \"{input_directory}\" -b \"{input_directory}\" -cs \"{archive_name}=sha256:$(GENCTBL_UCD_SHA256)\" -f"
    );
    let verify_command = format!(
        "@test -s \"{input_directory}/{}\" -a -s \"{input_directory}/{}\"",
        declaration.inputs[0].filename, declaration.inputs[1].filename
    );
    require_recipe_lines_with_continuations(
        ready_rule,
        &[
            (
                "@$(ECHO) \"Preparing verified Unicode $(GENCTBL_UCD_VERSION) data...\"".to_owned(),
                false,
            ),
            (fetch_command, true),
            (verify_command, false),
            ("@touch \"$@\"".to_owned(), false),
        ],
    )?;

    let output_target = input_targets.join(" ");
    let outputs_rule = unique_rule(&tool_rules, &input_targets[0])?;
    check_rule_state(outputs_rule)?;
    if outputs_rule.target.trim() != output_target || outputs_rule.continued {
        return Err("host tool must declare every sealed input as a ready-stamp output".into());
    }
    let (normal, order_only) = prerequisites(&outputs_rule.prerequisites)?;
    if normal != [ready_target] || !order_only.is_empty() {
        return Err(
            "host tool input outputs must depend only on the verified archive stamp".into(),
        );
    }
    require_recipe_lines(outputs_rule, &["@test -s \"$@\""])?;
    for target in input_targets.iter().skip(1) {
        if !std::ptr::eq(unique_rule(&tool_rules, target)?, outputs_rule) {
            return Err("host tool input outputs are not one shared ready-stamp rule".into());
        }
    }
    Ok(())
}

fn archive_input_identity(
    declaration: &NativeHostFileGenerator,
) -> Result<(String, String, String), String> {
    if declaration.inputs.len() != 2 {
        return Err("delegated archive route requires the two sealed raw inputs".into());
    }
    let input_directory = declaration
        .input_directory
        .strip_prefix("gen/")
        .ok_or("delegated input directory must be below gen/")?;
    validate_relative_suffix(input_directory, "delegated input directory")?;
    let mut expected_version: Option<&str> = None;
    let mut expected_data_directory: Option<&str> = None;
    for input in &declaration.inputs {
        let prefix = "https://www.unicode.org/Public/";
        let tail = input
            .url
            .strip_prefix(prefix)
            .ok_or("delegated raw inputs must use the canonical Unicode HTTPS URL")?;
        let parts: Vec<_> = tail.split('/').collect();
        if parts.len() != 3
            || parts[0].is_empty()
            || parts[0].eq_ignore_ascii_case("latest")
            || parts[1].is_empty()
            || parts[2] != input.filename
            || !safe_token(parts[0])
            || !safe_token(parts[1])
        {
            return Err(format!(
                "sealed input URL {:?} is not /Public/<version>/<data-dir>/<filename>",
                input.url
            ));
        }
        if expected_version.is_some_and(|version| version != parts[0])
            || expected_data_directory.is_some_and(|directory| directory != parts[1])
        {
            return Err(
                "sealed raw input URLs disagree on Unicode version or data directory".into(),
            );
        }
        expected_version = Some(parts[0]);
        expected_data_directory = Some(parts[1]);
    }
    let version = expected_version.ok_or("delegated route has no sealed inputs")?;
    let data_directory = expected_data_directory.ok_or("delegated route has no data directory")?;
    if input_directory != data_directory {
        return Err(
            "declared input directory differs from the sealed input URL data directory".into(),
        );
    }
    let archive = data_directory.to_ascii_uppercase();
    if !safe_token(&archive) {
        return Err("derived Unicode archive name is not one safe token".into());
    }
    Ok((data_directory.to_owned(), version.to_owned(), archive))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn require_unconditional_assignment(
    content: &str,
    name: &str,
    operator: &str,
    value: &str,
) -> Result<(), String> {
    let mut conditional_depth = 0usize;
    for (_, logical_line) in make_logical_nonrecipe_lines(content) {
        let text = make_text_before_comment(&logical_line).trim();
        let directive = text.split_whitespace().next().unwrap_or_default();
        if matches!(directive, "ifeq" | "ifneq" | "ifdef" | "ifndef") {
            conditional_depth += 1;
        }
        if let Some(assignment) = parse_make_assignment(text) {
            if assignment.name == name
                && (conditional_depth != 0
                    || assignment.modified
                    || assignment.target_specific
                    || assignment.operator != operator
                    || assignment.value != value)
            {
                return Err(format!("{name} assignment is conditional or changed"));
            }
        }
        if directive == "endif" {
            conditional_depth = conditional_depth.saturating_sub(1);
        }
    }
    Ok(())
}

fn validate_tool_mkdir_rule(rules: &[Rule], target: &str) -> Result<(), String> {
    let rule = unique_rule(rules, target)?;
    check_rule_state(rule)?;
    if rule.target.trim() != target
        || rule.continued
        || !prerequisites(&rule.prerequisites)?.0.is_empty()
        || !prerequisites(&rule.prerequisites)?.1.is_empty()
    {
        return Err(format!("host tool directory rule for {target:?} changed"));
    }
    require_recipe_lines(rule, &["@$(MKDIR) -p $@"])
}

fn validate_top_level_tool_rule(
    source_root: &Path,
    declaration: &NativeHostFileGenerator,
    input_route: InputRoute,
) -> Result<(), String> {
    let makefile = read_source_file(source_root, "Makefile.in")?;
    let protected = [
        declaration.tool_variable.as_str(),
        "MAKE",
        "MKARGS",
        "SRCDIR",
        "TOP",
        "CALL",
        "ECHO",
        "FETCH",
    ];
    reject_dynamic_make_rebindings(
        &makefile,
        &protected,
        &["TOP", "SRCDIR"],
        "top-level Makefile.in",
    )?;
    for (name, expected_value) in [("TOP", "@AROS_BUILDDIR@"), ("SRCDIR", "@SRCDIR@")] {
        let value = unique_make_assignment(&makefile, name, ":=")?;
        if value != expected_value {
            return Err(format!(
                "top-level {name} assignment must preserve its canonical configure placeholder"
            ));
        }
        require_unconditional_assignment(&makefile, name, ":=", expected_value)?;
    }
    let expected_target = format!("$({})", declaration.tool_variable);
    let expected_prerequisite = format!("$(SRCDIR)/{}", declaration.tool_source);
    let tool_makefile_prerequisite = format!("$(SRCDIR)/{}", declaration.tool_recipe);
    let source_directory = Path::new(&declaration.tool_source)
        .parent()
        .ok_or("tool source has no parent directory")?
        .to_string_lossy()
        .replace('\\', "/");
    let rules = parse_rules(logical_lines(&makefile, None));
    let rule = unique_rule(&rules, &expected_target)?;
    check_rule_state(rule)?;
    if rule.target.trim() != expected_target || rule.continued {
        return Err("top-level host tool target must be one literal variable".into());
    }
    let (normal, order_only) = prerequisites(&rule.prerequisites)?;
    let legacy_prerequisites = [expected_prerequisite.as_str()];
    let sealed_prerequisites = [
        expected_prerequisite.as_str(),
        tool_makefile_prerequisite.as_str(),
    ];
    let prerequisite_match = match input_route {
        InputRoute::CopyPattern => normal == legacy_prerequisites || normal == sealed_prerequisites,
        InputRoute::DelegatedToolMake => normal == sealed_prerequisites,
    };
    if !prerequisite_match || !order_only.is_empty() {
        return Err(
            "top-level host tool target must depend on its declared source C file and, for delegated inputs, its declared Makefile".into(),
        );
    }
    let expected_recipes = [
        "@$(ECHO) Building $(notdir $@)...".to_owned(),
        format!(
            "@$(CALL) $(MAKE) $(MKARGS) -C $(SRCDIR)/{source_directory} SRCDIR=$(SRCDIR) TOP=$(TOP)"
        ),
    ];
    let actual = normalized_recipe_lines(&rule.recipes)?;
    if actual != expected_recipes {
        return Err("top-level tool rule must call Make in the declared source directory".into());
    }
    Ok(())
}

fn validate_configure_tool_variable(
    source_root: &Path,
    declaration: &NativeHostFileGenerator,
    source_stem: &str,
) -> Result<(), String> {
    let configure = read_source_file(source_root, "configure.in")?;
    let marker = format!("\"\"{}", declaration.tool_variable);
    let occurrences: Vec<_> = configure.match_indices(&marker).collect();
    if occurrences.len() != 1 {
        return Err(format!(
            "configure.in must define {} exactly once through TOOLDIR",
            declaration.tool_variable
        ));
    }
    let start = occurrences[0].0;
    let tail = &configure[start + marker.len()..];
    let tail = tail.trim_start_matches([' ', '\t']);
    let expected = format!(":= $\"\"(TOOLDIR)/{source_stem}$\"\"(HOST_EXE_SUFFIX)$export_newline");
    if !tail.starts_with(&expected) {
        return Err(format!(
            "configure.in does not map {} to its TOOLDIR executable path",
            declaration.tool_variable
        ));
    }
    Ok(())
}

fn standard_headers_in_c_source(source: &str) -> Result<Vec<String>, String> {
    // The host tool is compiled with its source directory on the include path.
    // GENINCDIR is not imported into this source contract, so the closed list
    // below is assumed to resolve from the host's C implementation.
    // Reject trigraphs before comment stripping: C translation may turn them
    // into directive characters or backslashes before comments and splicing.
    if contains_c_trigraph(source.as_bytes()) {
        return Err("host C source contains a trigraph outside the closed scanner subset".into());
    }
    let uncommented = strip_c_comments(source);
    let mut headers = Vec::new();
    for directive in c_preprocessor_lines(&uncommented) {
        let line = directive.trim_start();
        let Some(rest) = line.strip_prefix('#').or_else(|| line.strip_prefix("%:")) else {
            continue;
        };
        let rest = rest.trim_start();
        let directive_name = rest
            .split(|character: char| character.is_ascii_whitespace())
            .next()
            .unwrap_or_default();
        if directive_name != "include" {
            if directive_name.starts_with("include") {
                return Err("host C source has an unsupported include directive".into());
            }
            continue;
        }
        let include = rest[directive_name.len()..].trim_start();
        if !include.starts_with('<') {
            return Err("host C source has a quoted or indirect include".into());
        }
        let Some(end) = include.find('>') else {
            return Err("host C source has a malformed angle include".into());
        };
        let header = &include[1..end];
        if !STANDARD_C_HEADERS.contains(&header) {
            return Err(format!(
                "host C source includes nonstandard or project header <{header}>"
            ));
        }
        if !include[end + 1..].trim().is_empty() {
            return Err("host C source has a nonliteral angle include".into());
        }
        headers.push(header.to_owned());
    }
    Ok(headers)
}

fn contains_c_trigraph(source: &[u8]) -> bool {
    source.windows(3).any(|trigraph| {
        trigraph[0] == b'?' && trigraph[1] == b'?' && b"=/'()!<>-".contains(&trigraph[2])
    })
}

fn reject_source_local_header_shadows(
    source_root: &Path,
    tool_source: &str,
    headers: &[String],
) -> Result<(), String> {
    let source_directory = Path::new(tool_source)
        .parent()
        .ok_or("tool source has no parent directory")?;
    for header in headers {
        let candidate = source_root.join(source_directory).join(header);
        match fs::symlink_metadata(&candidate) {
            Ok(_) => {
                return Err(format!(
                    "tool source directory contains a shadow for standard header <{header}>"
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cannot inspect possible local header shadow {}: {error}",
                    candidate.display()
                ));
            }
        }
    }
    Ok(())
}

fn c_preprocessor_lines(source: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut pending = String::new();
    for physical in source.lines() {
        let trimmed = physical.trim_end();
        if let Some(prefix) = trimmed.strip_suffix('\\') {
            pending.push_str(prefix);
        } else {
            pending.push_str(physical);
            output.push(std::mem::take(&mut pending));
        }
    }
    if !pending.is_empty() {
        output.push(pending);
    }
    output
}

fn strip_c_comments(source: &str) -> String {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        String,
        Character,
        LineComment,
        BlockComment,
    }

    let bytes = source.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut state = State::Normal;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        match state {
            State::Normal if byte == b'/' && next == Some(b'/') => {
                output.extend_from_slice(b"  ");
                state = State::LineComment;
                index += 2;
            }
            State::Normal if byte == b'/' && next == Some(b'*') => {
                output.extend_from_slice(b"  ");
                state = State::BlockComment;
                index += 2;
            }
            State::Normal if byte == b'"' => {
                output.push(byte);
                state = State::String;
                index += 1;
            }
            State::Normal if byte == b'\'' => {
                output.push(byte);
                state = State::Character;
                index += 1;
            }
            State::String | State::Character if byte == b'\\' => {
                output.push(byte);
                if let Some(next) = next {
                    output.push(next);
                    index += 2;
                } else {
                    index += 1;
                }
            }
            State::String if byte == b'"' => {
                output.push(byte);
                state = State::Normal;
                index += 1;
            }
            State::Character if byte == b'\'' => {
                output.push(byte);
                state = State::Normal;
                index += 1;
            }
            State::LineComment if byte == b'\n' => {
                output.push(byte);
                state = State::Normal;
                index += 1;
            }
            State::BlockComment if byte == b'*' && next == Some(b'/') => {
                output.extend_from_slice(b"  ");
                state = State::Normal;
                index += 2;
            }
            State::LineComment | State::BlockComment if byte != b'\n' => {
                output.push(b' ');
                index += 1;
            }
            _ => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn canonical_source_root(source_root: &Path) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(source_root)
        .map_err(|error| format!("source root cannot be read: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("source root must be a real directory, not a symlink".into());
    }
    source_root
        .canonicalize()
        .map_err(|error| format!("source root cannot be resolved: {error}"))
}

fn read_source_file(root: &Path, relative: &str) -> Result<String, String> {
    validate_relative_file_path(relative)?;
    let mut candidate = root.to_path_buf();
    let components: Vec<_> = Path::new(relative).components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err(format!("source path {relative:?} contains traversal"));
        };
        candidate.push(component);
        let metadata = fs::symlink_metadata(&candidate)
            .map_err(|error| format!("source path {relative:?} is unavailable: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("source path {relative:?} crosses a symlink"));
        }
        if index + 1 == components.len() {
            if !metadata.is_file() {
                return Err(format!("source path {relative:?} is not a regular file"));
            }
        } else if !metadata.is_dir() {
            return Err(format!(
                "source path parent in {relative:?} is not a directory"
            ));
        }
    }
    let bytes = fs::read(candidate)
        .map_err(|error| format!("source file {relative:?} cannot be read: {error}"))?;
    // Some legacy Makefiles contain a non-UTF-8 copyright byte. The bounded
    // rule syntax is ASCII; lossy decoding preserves those rules while any
    // non-ASCII byte in a matched target or recipe still fails exact matching.
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn validate_relative_file_path(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains(['\\', '$', ';', '\n', '\r', ':'])
        || Path::new(value)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || value.split('/').any(|component| !safe_token(component))
    {
        return Err(format!("source path {value:?} is not a safe relative file"));
    }
    Ok(())
}

fn safe_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+'))
}

#[cfg(test)]
#[path = "host_c_file_rules_tests.rs"]
mod tests;
