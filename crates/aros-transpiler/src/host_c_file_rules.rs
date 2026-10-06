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
    validate_make_chain(content, line_states, &rel_dir_text, declaration)
        .map_err(|reason| format!("{owner}: {reason}"))?;
    // Then independently require the same literal rules in the source-owned
    // file. Source conditionals around these rules are intentionally rejected:
    // this module has no variable environment with which to evaluate them.
    validate_make_chain(&recipe_text, None, &rel_dir_text, declaration)
        .map_err(|reason| format!("{owner}: source mmakefile: {reason}"))?;

    validate_tool_chain(&root, declaration).map_err(|reason| format!("{owner}: {reason}"))
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
    for input in &declaration.inputs {
        if !safe_token(&input.filename) {
            return Err(format!(
                "declared input {:?} is not one safe basename",
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
) -> Result<(), String> {
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

    let input_rule = unique_rule(&rules, &paths.input_pattern)?;
    check_rule_state(input_rule)?;
    if input_rule.target.trim() != paths.input_pattern.as_str() || input_rule.continued {
        return Err("input copy rule must match the declared directory and suffix".into());
    }
    let (normal, order_only) = prerequisites(&input_rule.prerequisites)?;
    if normal != [paths.input_source_pattern.as_str()]
        || order_only != [paths.input_directory_expr.as_str()]
    {
        return Err("input copy rule has changed source or directory prerequisites".into());
    }
    require_recipe_lines(input_rule, &["@$(CP) $< $@"])?;

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
    reject_local_variable_overrides(content, &declaration.tool_variable)?;
    Ok(())
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

#[derive(Debug)]
struct DerivedPaths {
    input_directory_expr: String,
    input_pattern: String,
    input_source_pattern: String,
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
    let input_extension = uniform_extension(&declaration.inputs)?;
    let input_pattern = format!("{input_directory_expr}/%{input_extension}");
    let input_source_pattern = format!("$(PORTSSOURCEDIR)/%{input_extension}");

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
        input_pattern,
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
) -> Result<String, String> {
    let Some(first) = inputs.first() else {
        return Err("generator has no declared source inputs".into());
    };
    let suffix = file_suffix(&first.filename);
    if inputs
        .iter()
        .any(|input| file_suffix(&input.filename) != suffix)
    {
        return Err("declared inputs do not share one suffix for the copy pattern".into());
    }
    Ok(suffix)
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

fn reject_local_variable_overrides(content: &str, tool_variable: &str) -> Result<(), String> {
    let names = [
        "GENDIR",
        "CURDIR",
        "PORTSSOURCEDIR",
        "CP",
        "ECHO",
        tool_variable,
    ];
    reject_dynamic_make_rebindings(content, &names, "source mmakefile")?;
    for (line_number, line) in content.lines().enumerate() {
        let Some(assignment) = parse_make_assignment(line) else {
            continue;
        };
        if names.contains(&assignment.name) {
            return Err(format!(
                "source line {} locally assigns modeled Make variable {}",
                line_number + 1,
                assignment.name
            ));
        }
    }
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
    let (at, operator) = MAKE_ASSIGNMENT_OPERATORS
        .iter()
        .filter_map(|operator| line.find(operator).map(|at| (at, *operator)))
        .min_by_key(|(at, operator)| (*at, std::cmp::Reverse(operator.len())))?;
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
    recipe_label: &str,
) -> Result<(), String> {
    for (line_number, physical_line) in content.lines().enumerate() {
        if physical_line.starts_with('\t') {
            continue;
        }
        let line = make_text_before_comment(physical_line).trim();
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
        if let Some((kind, name, dynamic)) = make_variable_directive(line) {
            if dynamic || protected.contains(&name) {
                return Err(format!(
                    "{recipe_label} line {} uses {kind} for a protected or dynamic Make variable",
                    line_number + 1
                ));
            }
        }
        if let Some((kind, names, dynamic)) = make_export_directive(line) {
            if dynamic || names.iter().any(|name| protected.contains(name)) {
                return Err(format!(
                    "{recipe_label} line {} uses {kind} for a protected or dynamic Make variable",
                    line_number + 1
                ));
            }
        }
    }
    Ok(())
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
) -> Result<(), String> {
    let tool_makefile = read_source_file(source_root, &declaration.tool_recipe)?;
    let protected = [
        declaration.tool_variable.as_str(),
        "USER_CFLAGS",
        "HOST_CFLAGS",
    ];
    reject_dynamic_make_rebindings(&tool_makefile, &protected, "host tool Makefile")?;
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
    if normal != [source_leaf, "$(GENMODULE_DEPS)"] || !order_only.is_empty() {
        return Err(
            "host tool prerequisites differ from its C source and declared Make dependencies"
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

    validate_top_level_tool_rule(source_root, declaration)?;
    validate_configure_tool_variable(source_root, declaration, source_stem)?;
    Ok(())
}

fn unique_make_assignment<'a>(
    content: &'a str,
    name: &str,
    expected_operator: &str,
) -> Result<&'a str, String> {
    let assignments: Vec<_> = content
        .lines()
        .filter_map(parse_make_assignment)
        .filter(|assignment| assignment.name == name)
        .collect();
    match assignments.as_slice() {
        [assignment]
            if !assignment.modified
                && !assignment.target_specific
                && assignment.operator == expected_operator =>
        {
            Ok(assignment.value)
        }
        [assignment] if assignment.modified || assignment.target_specific => Err(format!(
            "{name} assignment uses Make modifiers or target scope that are outside the closed recipe"
        )),
        [assignment] => Err(format!(
            "{name} assignment uses {}, expected {expected_operator}",
            assignment.operator
        )),
        [] => Err(format!("tool Makefile has no {name} assignment")),
        _ => Err(format!("tool Makefile has duplicate {name} assignments")),
    }
}

fn validate_top_level_tool_rule(
    source_root: &Path,
    declaration: &NativeHostFileGenerator,
) -> Result<(), String> {
    let makefile = read_source_file(source_root, "Makefile.in")?;
    let expected_target = format!("$({})", declaration.tool_variable);
    let expected_prerequisite = format!("$(SRCDIR)/{}", declaration.tool_source);
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
    if normal != [expected_prerequisite.as_str()] || !order_only.is_empty() {
        return Err("top-level host tool target must depend on its declared source C file".into());
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
mod tests {
    use super::*;
    use aros_common::native_host_generator::NativeHostFileInput;
    use aros_common::Sha256Digest;
    use std::fs;
    use std::path::PathBuf;

    const REL_DIR: &str = "compiler/crt/stdc";

    fn declaration() -> NativeHostFileGenerator {
        NativeHostFileGenerator {
            owner: "compiler-stdc-genwcharsupport".into(),
            recipe: format!("{REL_DIR}/mmakefile.src"),
            tool_recipe: "tools/genctbl/Makefile".into(),
            tool_source: "tools/genctbl/genctbl.c".into(),
            tool_variable: "GENCTBL".into(),
            output: format!("gen/{REL_DIR}/defaults/en_GB_ISO8859-1.c"),
            input_directory: "gen/ucd".into(),
            compile_flags: ["-g", "-Wall", "-Werror", "-Wunused", "-O2"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            arguments: vec![
                "@INPUT_DIRECTORY@".into(),
                "@OUTPUT_DIRECTORY@".into(),
                "en_GB_ISO8859-1".into(),
                "--emit-c".into(),
            ],
            inputs: ["UnicodeData.txt", "SpecialCasing.txt"]
                .into_iter()
                .map(|filename| NativeHostFileInput {
                    filename: filename.into(),
                    url: format!("https://www.unicode.org/Public/17.0.0/ucd/{filename}"),
                    sha256: Sha256Digest::parse(
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    )
                    .unwrap(),
                    size: 1,
                })
                .collect(),
        }
    }

    const MAKEFILE: &str = r#"compiler-stdc-genwcharsupport : $(GENDIR)/$(CURDIR)/defaults/en_GB_ISO8859-1.c

$(GENDIR)/$(CURDIR)/defaults:
	%mkdirs_q $@

$(GENDIR)/ucd:
	%mkdirs_q $@

$(GENDIR)/ucd/%.txt: $(PORTSSOURCEDIR)/%.txt | $(GENDIR)/ucd
	@$(CP) $< $@

$(GENDIR)/$(CURDIR)/defaults/%.c: $(GENCTBL) $(GENDIR)/ucd/UnicodeData.txt $(GENDIR)/ucd/SpecialCasing.txt | $(GENDIR)/$(CURDIR)/defaults
	@$(ECHO) "Generating $*.c";
	@$(GENCTBL) $(GENDIR)/ucd $(GENDIR)/$(CURDIR)/defaults $* --emit-c;
"#;

    fn source_root() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let declaration = declaration();
        write(&root, &declaration.recipe, MAKEFILE);
        write(
            &root,
            &declaration.tool_recipe,
            r#"USER_CFLAGS := -Wall -Werror -Wunused -O2

-include $(TOP)/config/make.cfg
-include Makefile.deps

HOST_CC ?= gcc
HOST_CFLAGS ?= $(USER_CFLAGS)
GENCTBL ?= genctbl

$(GENCTBL) : genctbl.c $(GENMODULE_DEPS)
	@$(ECHO) "Compiling $(notdir $@)..."
	@$(HOST_CC) -g $(HOST_CFLAGS) -I$(GENINCDIR) -I$(TOP)/$(CURDIR) genctbl.c -o $@
"#,
        );
        write(
            &root,
            &declaration.tool_source,
            "#include <stdio.h>\nint main(void) { return 0; }\n",
        );
        write(
            &root,
            "Makefile.in",
            r"$(GENCTBL): $(SRCDIR)/tools/genctbl/genctbl.c
	@$(ECHO) Building $(notdir $@)...
	@$(CALL) $(MAKE) $(MKARGS) -C $(SRCDIR)/tools/genctbl SRCDIR=$(SRCDIR) TOP=$(TOP)
",
        );
        write(
            &root,
            "configure.in",
            "make_extra_commands=\"$make_extra_commands$export_newline\"\"GENCTBL\t:= $\"\"(TOOLDIR)/genctbl$\"\"(HOST_EXE_SUFFIX)$export_newline\"\n",
        );
        (temp, root)
    }

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn validate_fixture(
        content: &str,
        root: &Path,
        declaration: &NativeHostFileGenerator,
        states: Option<&[ConditionalTruth]>,
    ) -> Result<(), String> {
        validate_source_rule(content, root, Path::new(REL_DIR), declaration, states)
    }

    #[test]
    fn accepts_the_bounded_owner_generator_and_source_tool_chain() {
        let (_temp, root) = source_root();
        validate_fixture(MAKEFILE, &root, &declaration(), None).unwrap();
    }

    #[test]
    fn derives_non_ucd_directories_and_input_output_suffixes() {
        let (_temp, root) = source_root();
        let mut declaration = declaration();
        declaration.input_directory = "gen/reference/unicode".into();
        declaration.inputs = ["Primary.csv", "Secondary.csv"]
            .into_iter()
            .map(|filename| NativeHostFileInput {
                filename: filename.into(),
                url: format!("https://example.invalid/reference/{filename}"),
                sha256: Sha256Digest::parse(
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                )
                .unwrap(),
                size: 1,
            })
            .collect();
        declaration.output = format!("gen/{REL_DIR}/generated/locale_variant.h");
        declaration.arguments[2] = "locale_variant".into();

        let custom = MAKEFILE
            .replace(
                "$(GENDIR)/$(CURDIR)/defaults/en_GB_ISO8859-1.c",
                "$(GENDIR)/$(CURDIR)/generated/locale_variant.h",
            )
            .replace(
                "$(GENDIR)/$(CURDIR)/defaults/%.c",
                "$(GENDIR)/$(CURDIR)/generated/%.h",
            )
            .replace(
                "$(GENDIR)/$(CURDIR)/defaults",
                "$(GENDIR)/$(CURDIR)/generated",
            )
            .replace(
                "$(GENDIR)/ucd/UnicodeData.txt",
                "$(GENDIR)/reference/unicode/Primary.csv",
            )
            .replace(
                "$(GENDIR)/ucd/SpecialCasing.txt",
                "$(GENDIR)/reference/unicode/Secondary.csv",
            )
            .replace("$(GENDIR)/ucd/%.txt", "$(GENDIR)/reference/unicode/%.csv")
            .replace("$(PORTSSOURCEDIR)/%.txt", "$(PORTSSOURCEDIR)/%.csv")
            .replace("$(GENDIR)/ucd", "$(GENDIR)/reference/unicode")
            .replace("$*.c", "$*.h");
        write(&root, &declaration.recipe, &custom);
        validate_fixture(&custom, &root, &declaration, None).unwrap();
    }

    #[test]
    fn keeps_every_declared_input_as_a_normal_prerequisite() {
        let (_temp, root) = source_root();
        let altered = MAKEFILE.replace(" $(GENDIR)/ucd/SpecialCasing.txt |", " |");
        write(&root, &declaration().recipe, &altered);
        let error = validate_fixture(&altered, &root, &declaration(), None).unwrap_err();
        assert!(error.contains("every sealed input"), "{error}");
    }

    #[test]
    fn rejects_extra_shell_and_changed_generator_mode() {
        let (_temp, root) = source_root();
        let altered = MAKEFILE.replace(
            "\t@$(GENCTBL) $(GENDIR)/ucd $(GENDIR)/$(CURDIR)/defaults $* --emit-c;",
            "\t@$(GENCTBL) $(GENDIR)/ucd $(GENDIR)/$(CURDIR)/defaults $* --emit-c; touch bad",
        );
        write(&root, &declaration().recipe, &altered);
        assert!(validate_fixture(&altered, &root, &declaration(), None).is_err());

        let altered = MAKEFILE.replace("$* --emit-c;", "$* --emit-binary;");
        write(&root, &declaration().recipe, &altered);
        assert!(validate_fixture(&altered, &root, &declaration(), None).is_err());
    }

    #[test]
    fn rejects_unresolved_conditions_and_owner_additions() {
        let (_temp, root) = source_root();
        let mut states = vec![ConditionalTruth::True; MAKEFILE.lines().count()];
        let owner_line = MAKEFILE
            .lines()
            .position(|line| line.starts_with("compiler-stdc-genwcharsupport"))
            .unwrap();
        states[owner_line] = ConditionalTruth::Unknown;
        assert!(validate_fixture(MAKEFILE, &root, &declaration(), Some(&states)).is_err());

        let altered = MAKEFILE.replace(
            "compiler-stdc-genwcharsupport : $(GENDIR)/$(CURDIR)/defaults/en_GB_ISO8859-1.c",
            "compiler-stdc-genwcharsupport : $(GENDIR)/$(CURDIR)/defaults/en_GB_ISO8859-1.c extra",
        );
        write(&root, &declaration().recipe, &altered);
        assert!(validate_fixture(&altered, &root, &declaration(), None).is_err());
    }

    #[test]
    fn rejects_source_make_variable_rebinding_forms() {
        let (_temp, root) = source_root();
        let declarations = [
            "override GENCTBL := /tmp/evil",
            "export GENCTBL := /tmp/evil",
            "other-owner: private GENCTBL := /tmp/evil",
            "other-owner: override export private GENCTBL := /tmp/evil",
            "define GENCTBL =\n/tmp/evil\nendef",
            "undefine GENCTBL",
            "OTHER := $(eval GENCTBL := /tmp/evil)",
        ];
        for rebinding in declarations {
            let altered = format!("{rebinding}\n\n{MAKEFILE}");
            write(&root, &declaration().recipe, &altered);
            let error = validate_fixture(&altered, &root, &declaration(), None).unwrap_err();
            let expected = if rebinding.starts_with("define ") {
                "define"
            } else if rebinding.starts_with("undefine ") {
                "undefine"
            } else if rebinding.contains("$(eval") {
                "eval"
            } else {
                "GENCTBL"
            };
            assert!(error.contains(expected), "{rebinding:?}: {error}");
        }
    }

    #[test]
    fn rejects_tool_flag_assignments_with_make_modifiers() {
        let (_temp, root) = source_root();
        let tool_makefile_path = root.join(declaration().tool_recipe);
        let original = fs::read_to_string(&tool_makefile_path).unwrap();
        for assignment in [
            "HOST_CFLAGS ?= $(USER_CFLAGS)",
            "USER_CFLAGS := -Wall -Werror -Wunused -O2",
        ] {
            let changed = original.replace(assignment, &format!("override {assignment}"));
            assert_ne!(changed, original);
            write(&root, &declaration().tool_recipe, &changed);
            let error = validate_fixture(MAKEFILE, &root, &declaration(), None).unwrap_err();
            assert!(error.contains("Make modifiers"), "{error}");
        }
    }

    #[test]
    fn binds_host_flags_and_rejects_quoted_project_includes() {
        let (_temp, root) = source_root();
        let mut changed = declaration();
        changed.compile_flags.pop();
        assert!(validate_fixture(MAKEFILE, &root, &changed, None).is_err());

        write(
            &root,
            &changed.tool_source,
            "#include \"local.h\"\nint main(void) { return 0; }\n",
        );
        assert!(validate_fixture(MAKEFILE, &root, &declaration(), None).is_err());

        write(
            &root,
            &changed.tool_source,
            "#include <private_project_header.h>\nint main(void) { return 0; }\n",
        );
        assert!(validate_fixture(MAKEFILE, &root, &declaration(), None).is_err());
    }

    #[test]
    fn rejects_alternate_and_spliced_preprocessor_include_syntax() {
        let (_temp, root) = source_root();
        for source in [
            "%:include <private_project_header.h>\n",
            "#inc\\\nlude <private_project_header.h>\n",
            "#??=include <private_project_header.h>\n",
        ] {
            write(&root, &declaration().tool_source, source);
            let error = validate_fixture(MAKEFILE, &root, &declaration(), None).unwrap_err();
            assert!(
                error.contains("include") || error.contains("trigraph"),
                "{error}"
            );
        }
    }

    #[test]
    fn rejects_source_local_shadows_of_standard_headers() {
        let (_temp, root) = source_root();
        write(
            &root,
            &declaration().tool_source,
            "#include <stdio.h>\nint main(void) { return 0; }\n",
        );
        write(&root, "tools/genctbl/stdio.h", "/* local shadow */\n");
        let error = validate_fixture(MAKEFILE, &root, &declaration(), None).unwrap_err();
        assert!(
            error.contains("shadow for standard header <stdio.h>"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_tool_sources() {
        use std::os::unix::fs::symlink;

        let (_temp, root) = source_root();
        let real = root.join("tools/genctbl/real.c");
        fs::write(&real, "#include <stdio.h>\n").unwrap();
        let target = root.join("tools/genctbl/genctbl.c");
        fs::remove_file(&target).unwrap();
        symlink(real, target).unwrap();
        assert!(validate_fixture(MAKEFILE, &root, &declaration(), None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_source_local_header_shadows() {
        use std::os::unix::fs::symlink;

        let (_temp, root) = source_root();
        let real = root.join("tools/genctbl/real.h");
        fs::write(&real, "/* shadow */\n").unwrap();
        symlink(real, root.join("tools/genctbl/stdio.h")).unwrap();
        let error = validate_fixture(MAKEFILE, &root, &declaration(), None).unwrap_err();
        assert!(
            error.contains("shadow for standard header <stdio.h>"),
            "{error}"
        );
    }

    #[test]
    #[ignore = "requires AROS_TEST_P4_SOURCE"]
    fn actual_source_genctbl_recipe_is_admitted() {
        let root = PathBuf::from(
            std::env::var_os("AROS_TEST_P4_SOURCE")
                .expect("set AROS_TEST_P4_SOURCE to the selected AROS source tree"),
        );
        let root = root.canonicalize().expect("source tree must exist");
        let declaration = declaration();
        let content = fs::read_to_string(root.join(&declaration.recipe)).unwrap();
        validate_source_rule(&content, &root, Path::new(REL_DIR), &declaration, None).unwrap();
    }
}
