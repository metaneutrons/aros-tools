//! Compiler argument parsing and validation for assembly-header recipes.

use super::{
    evaluate_make_expr, evaluate_make_list, is_build_path, BTreeMap, DirVars, MakeExprContext,
    Path, VarScope, BUILD_ALIAS, SOURCE_ALIAS,
};

pub(super) fn compiler_arguments(
    scope: &VarScope,
    arch_definitions: &[String],
    dirs: &DirVars,
    source_root: &Path,
    relative_dir: &Path,
) -> Result<Vec<String>, String> {
    for name in [
        "TARGET_CC",
        "TARGET_SYSROOT",
        "CFLAGS",
        "PRIV_EXEC_INCLUDES",
    ] {
        if let Some(reason) = scope.flavor_uncertainty_reason_at(name, usize::MAX) {
            return Err(format!("cannot safely resolve `{name}`: {reason}"));
        }
    }
    let lookup = |name: &str| {
        if scope
            .flavor_uncertainty_reason_at(name, usize::MAX)
            .is_some()
        {
            return None;
        }
        if matches!(
            name,
            "AROS_SOURCE_DIR" | "AROS_BUILD_DIR" | "AROS_PORTS_DIR" | "AROS_PORTS_SOURCE_DIR"
        ) && scope.raw_at(name, usize::MAX).is_none()
        {
            return Some(format!("$${{{name}}}"));
        }
        let value = scope.raw_at(name, usize::MAX)?;
        Some(escape_cmake_references_for_make(&value))
    };
    let guard = |name: &str| scope.flavor_uncertainty_reason_at(name, usize::MAX);
    let context = MakeExprContext::new(scope, dirs, usize::MAX, source_root, relative_dir)
        .with_lookup(&lookup)
        .with_guard(&guard)
        .without_filesystem();
    let admitted_compiler = dirs
        .expand("$(NATIVE_TARGET_CC)")
        .ok_or_else(|| "native target C compiler role was not explicitly admitted".to_owned())?;
    if admitted_compiler != "${CMAKE_C_COMPILER}" {
        return Err("native target compiler role is not `${CMAKE_C_COMPILER}`".into());
    }
    let target_compiler = scope
        .raw_at("TARGET_CC", usize::MAX)
        .ok_or_else(|| "cannot resolve TARGET_CC from the explicit Make context".to_owned())?;
    if target_compiler.trim() != "$(NATIVE_TARGET_CC)"
        || scope.raw_at("NATIVE_TARGET_CC", usize::MAX).is_some()
    {
        return Err("TARGET_CC does not resolve to the admitted native target compiler".into());
    }

    let sysroot = evaluate_make_list("$(strip $(TARGET_SYSROOT))", &context)
        .map_err(|error| format!("cannot resolve TARGET_SYSROOT: {error}"))?;
    let mut arguments = Vec::new();
    match sysroot.as_slice() {
        [] => {}
        [value] if value.starts_with("--sysroot=") => {
            let configured = evaluate_make_expr("$(strip $(AROS_DEVELOPER))", &context)
                .map_err(|error| format!("cannot resolve configured Developer root: {error}"))?;
            let expected = format!("--sysroot={configured}");
            if value != &expected || !is_build_path(&configured) {
                return Err(
                    "TARGET_SYSROOT is outside the configured build-tree Developer root".into(),
                );
            }
            arguments.push(value.clone());
        }
        _ => {
            return Err(
                "TARGET_SYSROOT must be empty or one configured `--sysroot` argument".into(),
            );
        }
    }

    let cflags = evaluate_make_list("$(strip $(CFLAGS))", &context)
        .map_err(|error| format!("cannot resolve CFLAGS: {error}"))?;
    let includes = evaluate_make_list("$(strip $(PRIV_EXEC_INCLUDES))", &context)
        .map_err(|error| format!("cannot resolve PRIV_EXEC_INCLUDES: {error}"))?;
    arguments.extend(parse_compiler_arguments(&cflags, false, &[])?);
    arguments.extend(parse_compiler_arguments(&includes, true, arch_definitions)?);
    Ok(arguments)
}

fn escape_cmake_references_for_make(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut escaped = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$'
            && bytes.get(cursor + 1) == Some(&b'{')
            && (cursor == 0 || bytes[cursor - 1] != b'$')
        {
            escaped.push_str("$$");
            escaped.push('{');
            cursor += 2;
        } else {
            let character = value[cursor..]
                .chars()
                .next()
                .expect("valid UTF-8 boundary");
            escaped.push(character);
            cursor += character.len_utf8();
        }
    }
    escaped
}

pub(super) fn parse_compiler_arguments(
    words: &[String],
    includes_only: bool,
    architecture_definitions: &[String],
) -> Result<Vec<String>, String> {
    if words.len() > 512 {
        return Err("compiler argument list exceeds the 512-word limit".into());
    }
    let mut remaining_architecture_definitions = BTreeMap::<String, usize>::new();
    for definition in architecture_definitions {
        *remaining_architecture_definitions
            .entry(definition.clone())
            .or_default() += 1;
    }
    let mut result = Vec::with_capacity(words.len());
    let mut index = 0;
    while index < words.len() {
        let word = &words[index];
        if let Some(kind) = include_option(word) {
            let (option, value, consumed) = if kind.attached {
                let value = word
                    .strip_prefix(kind.name)
                    .ok_or_else(|| "malformed include option".to_owned())?;
                (kind.name.to_owned(), value.to_owned(), 1)
            } else {
                let value = words
                    .get(index + 1)
                    .ok_or_else(|| format!("include option `{word}` has no path"))?
                    .clone();
                (word.clone(), value, 2)
            };
            validate_include_path(&value)?;
            result.push(option);
            result.push(value);
            index += consumed;
            continue;
        }
        if word == "-include" || word == "-imacros" {
            let value = words
                .get(index + 1)
                .ok_or_else(|| format!("include option `{word}` has no path"))?
                .clone();
            validate_include_path(&value)?;
            result.push(word.clone());
            result.push(value);
            index += 2;
            continue;
        }
        if word.starts_with("-include=") || word.starts_with("-imacros=") {
            let (_, value) = word.split_once('=').expect("prefix contains equals");
            validate_include_path(value)?;
            result.push(word.clone());
            index += 1;
            continue;
        }
        if word == "-D" {
            let value = words
                .get(index + 1)
                .ok_or_else(|| "macro definition option `-D` has no name".to_owned())?;
            let canonical = format!("-D{value}");
            validate_macro_definition(&canonical)?;
            if includes_only {
                consume_architecture_definition(
                    &canonical,
                    &mut remaining_architecture_definitions,
                )?;
            }
            result.push(word.clone());
            result.push(value.clone());
            index += 2;
            continue;
        }
        if let Some(name) = word.strip_prefix("-D").filter(|name| !name.is_empty()) {
            validate_macro_definition(&format!("-D{name}"))?;
            if includes_only {
                consume_architecture_definition(word, &mut remaining_architecture_definitions)?;
            }
            result.push(word.clone());
            index += 1;
            continue;
        }
        if includes_only {
            return Err(format!(
                "PRIV_EXEC_INCLUDES contains non-include argument `{word}`"
            ));
        }
        validate_compiler_flag(word)?;
        result.push(word.clone());
        index += 1;
    }
    Ok(result)
}

fn validate_macro_definition(argument: &str) -> Result<(), String> {
    let Some(definition) = argument.strip_prefix("-D") else {
        return Err(format!("compiler definition `{argument}` is malformed"));
    };
    let name = definition
        .split_once('=')
        .map_or(definition, |(name, _)| name);
    if name.is_empty()
        || !name.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic() || byte == b'_' || (index > 0 && byte.is_ascii_digit())
        })
        || !name
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        || !safe_argument_text(argument)
    {
        return Err(format!(
            "compiler definition `{argument}` is outside the safe macro vocabulary"
        ));
    }
    validate_compiler_flag(argument)
}

fn consume_architecture_definition(
    definition: &str,
    remaining: &mut BTreeMap<String, usize>,
) -> Result<(), String> {
    let Some(count) = remaining.get_mut(definition) else {
        return Err(format!(
            "PRIV_EXEC_INCLUDES contains non-include argument `{definition}` without a matching architecture metadata definition"
        ));
    };
    *count -= 1;
    if *count == 0 {
        remaining.remove(definition);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct IncludeOption {
    name: &'static str,
    attached: bool,
}

fn include_option(word: &str) -> Option<IncludeOption> {
    for name in ["-isystem", "-iquote", "-idirafter"] {
        if word == name {
            return Some(IncludeOption {
                name,
                attached: false,
            });
        }
        if word.starts_with(name) && word.len() > name.len() {
            return Some(IncludeOption {
                name,
                attached: true,
            });
        }
    }
    if word == "-I" {
        return Some(IncludeOption {
            name: "-I",
            attached: false,
        });
    }
    if word.starts_with("-I") && word.len() > 2 {
        return Some(IncludeOption {
            name: "-I",
            attached: true,
        });
    }
    None
}

fn validate_compiler_flag(word: &str) -> Result<(), String> {
    if !word.starts_with('-')
        || word == "-c"
        || word == "-S"
        || word.starts_with("-o")
        || word == "-E"
        || word == "--"
        || word == "-fsyntax-only"
        || word.starts_with("-specs")
        || word.starts_with("--specs")
        || word.starts_with("-fplugin")
        || word.starts_with("-plugin")
        || word.starts_with("-B")
        || word.starts_with("-X")
        || word.starts_with("-Wl,")
        || word.starts_with("-Wa,")
        || word.starts_with("-Wp,")
        || word.starts_with("-MF")
        || word.starts_with("-MT")
        || word.starts_with("-MQ")
        || word.starts_with("-MJ")
        || word == "-MD"
        || word == "-MMD"
        || word == "-MP"
        || word.starts_with("-dumpbase")
        || word.starts_with("-save-temps")
        || word.starts_with("--output")
    {
        return Err(format!(
            "compiler flag `{word}` is outside the guarded compile argv"
        ));
    }
    if !safe_argument_text(word) {
        return Err(format!(
            "compiler flag `{word}` contains unsafe shell or path syntax"
        ));
    }
    Ok(())
}

fn safe_argument_text(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'$' => {
                if bytes.get(cursor + 1) != Some(&b'{') {
                    return false;
                }
                let Some(end) = text[cursor + 2..].find('}') else {
                    return false;
                };
                let end = cursor + 2 + end;
                let name = &text[cursor + 2..end];
                if !matches!(
                    name,
                    "AROS_SOURCE_DIR"
                        | "AROS_BUILD_DIR"
                        | "AROS_PORTS_DIR"
                        | "AROS_PORTS_SOURCE_DIR"
                ) {
                    return false;
                }
                cursor = end + 1;
            }
            byte if byte.is_ascii_alphanumeric() || b"_-.+=,:/@%".contains(&byte) => cursor += 1,
            _ => return false,
        }
    }
    !text.is_empty()
}

fn validate_include_path(value: &str) -> Result<(), String> {
    if ![
        SOURCE_ALIAS,
        BUILD_ALIAS,
        "${AROS_PORTS_DIR}",
        "${AROS_PORTS_SOURCE_DIR}",
    ]
    .iter()
    .any(|root| value.strip_prefix(&format!("{root}/")).is_some())
    {
        return Err(format!(
            "include path `{value}` is not contained by a configured source or build root"
        ));
    }
    if !safe_argument_text(value) || has_parent_components(value) {
        return Err(format!(
            "include path `{value}` escapes or cannot be represented safely"
        ));
    }
    Ok(())
}

fn has_parent_components(value: &str) -> bool {
    value
        .split('/')
        .any(|component| component == ".." || component.is_empty())
}
