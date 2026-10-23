//! Pure, bounded interpretation of one narrow Make text pipeline.
//!
//! Legacy projection recognizes unquoted ASCII `echo` data piped to one quoted
//! `sed` substitution. Source interpretation also admits literal single-quoted
//! echo words, a plain echo, and a bounded literal/range `tr` translation.
//! Neither entry point is a shell, sed, or tr implementation, and neither
//! executes commands or accesses the filesystem.

use regex::RegexBuilder;

mod tr;

const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MAX_SCRIPT_BYTES: usize = 4 * 1024;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_MATCHES: usize = 16_384;
const MAX_REGEX_SIZE: usize = 64 * 1024;

/// Interpret a bounded `echo DATA | sed 's/PATTERN/REPLACEMENT/[g]'` command.
///
/// Echo data is unquoted ASCII path-word text. Shell word splitting collapses
/// spaces and tabs to one space, and the command-substitution result omits the
/// final newline. The sed pattern accepts only literals, `.`, an optional
/// leading `^`, and up to nine balanced `\(...\)` captures. Replacements
/// accept literals, `&`, `\/`, and references to existing captures. This is
/// deliberately a small data model, not general shell or sed semantics.
///
/// # Errors
/// Rejects unsupported syntax, invalid captures and bounded resource exhaustion.
pub fn evaluate(command: &str) -> Result<String, String> {
    evaluate_with_echo_data(command, &|data| Ok(data.to_owned()))
}

// The expression layer can materialize its own trusted directory tokens in
// echo DATA only, never in command names, the pipe or the substitution script.
// The transformed data still passes the same closed path-word validation.
pub(crate) fn evaluate_with_echo_data(
    command: &str,
    transform: &impl Fn(&str) -> Result<String, String>,
) -> Result<String, String> {
    validate_command(command)?;
    let pipe = find_pipeline_pipe(command)?.ok_or("text pipeline must contain exactly one pipe")?;
    let input = parse_echo(&command[..pipe], transform)?;
    let script = parse_sed_command(&command[pipe + 1..])?;
    apply_substitution(&input, &script)
}

/// Interpret literal source echo data with an optional sed or tr pipeline.
///
/// Single-quoted words are inert data; expansions, double quotes,
/// escapes and option-dependent echo behavior are not evaluated.
/// This pure API defines a hermetic ASCII byte dialect, not the host locale
/// or native utility behavior. Native-equivalence callers must independently
/// bind the selected tools and a C/POSIX locale; the captured-shell adapter
/// performs those checks before publishing interpreted state.
///
/// # Errors
/// Rejects unsupported syntax and the same command/output bounds as the
/// legacy projection pipeline. Never invokes a shell or reads the environment.
pub fn evaluate_source(command: &str) -> Result<String, String> {
    evaluate_source_with_programs(command).map(|(output, _)| output)
}

// The closed parser, not a substring search, identifies the external program
// whose native lookup must remain bound. Echo is an admitted shell intrinsic.
pub(crate) fn evaluate_source_with_programs(
    command: &str,
) -> Result<(String, &'static [&'static str]), String> {
    validate_command(command)?;
    let pipe = find_pipeline_pipe(command)?;
    let input = parse_source_echo(&command[..pipe.unwrap_or(command.len())])?;
    if let Some(pipe) = pipe {
        let downstream = trim_horizontal(&command[pipe + 1..]);
        if tr::is_tr_command(downstream) {
            tr::evaluate(&input, downstream).map(|output| (output, &["tr"] as &[_]))
        } else {
            apply_substitution(&input, &parse_sed_command(downstream)?)
                .map(|output| (output, &["sed"] as &[_]))
        }
    } else {
        Ok((input, &[]))
    }
}

fn validate_command(command: &str) -> Result<(), String> {
    if command.len() > MAX_COMMAND_BYTES {
        return Err("text pipeline command exceeds the size limit".into());
    }
    if !command
        .bytes()
        .all(|byte| byte.is_ascii() && (byte == b'\t' || (0x20..=0x7e).contains(&byte)))
    {
        return Err("text pipeline accepts printable ASCII and tabs only".into());
    }

    Ok(())
}

fn find_pipeline_pipe(command: &str) -> Result<Option<usize>, String> {
    let mut quoted = false;
    let mut pipe = None;
    for (index, byte) in command.bytes().enumerate() {
        if byte == b'\'' {
            quoted = !quoted;
        } else if byte == b'|' && !quoted && pipe.replace(index).is_some() {
            return Err("text pipeline must contain exactly one pipe".into());
        }
    }
    if quoted {
        return Err("text pipeline contains an unterminated single quote".into());
    }
    Ok(pipe)
}

fn parse_source_echo(source: &str) -> Result<String, String> {
    let source = trim_horizontal(source);
    let mut rest = source
        .strip_prefix("echo")
        .ok_or("source text must start with the literal echo command")?;
    if !rest.is_empty() && !rest.as_bytes()[0].is_ascii_whitespace() {
        return Err("source echo command has extra command-name text".into());
    }
    let mut words = Vec::new();
    rest = rest.trim_start_matches([' ', '\t']);
    while !rest.is_empty() {
        let (word, tail) = if let Some(quoted) = rest.strip_prefix('\'') {
            let end = quoted
                .find('\'')
                .ok_or("source echo word has an unterminated quote")?;
            let word = &quoted[..end];
            if word.contains('\\') {
                return Err("source echo backslash interpretation is unsupported".into());
            }
            (word, &quoted[end + 1..])
        } else {
            let end = rest.find([' ', '\t']).unwrap_or(rest.len());
            let word = &rest[..end];
            if !word.bytes().all(is_echo_word_byte) {
                return Err("unquoted source echo contains unsupported shell syntax".into());
            }
            (word, &rest[end..])
        };
        if !tail.is_empty() && !tail.starts_with([' ', '\t']) {
            return Err("source echo word concatenation is unsupported".into());
        }
        if words.is_empty() && word.starts_with('-') {
            return Err("echo options are not supported".into());
        }
        words.push(word);
        rest = tail.trim_start_matches([' ', '\t']);
    }
    let result = words.join(" ");
    if result.len() > MAX_OUTPUT_BYTES {
        return Err("source echo output exceeds its size limit".into());
    }
    Ok(result)
}

#[cfg(test)]
mod source_echo_tests {
    use super::{evaluate, evaluate_source};

    #[test]
    fn parsed_external_programs_distinguish_literal_data_and_pipeline() {
        let (output, programs) = super::evaluate_source_with_programs("echo 'data | sed'").unwrap();
        assert_eq!(output, "data | sed");
        assert!(programs.is_empty());
        let (output, programs) =
            super::evaluate_source_with_programs("echo data | sed 's/data/result/'").unwrap();
        assert_eq!(output, "result");
        assert_eq!(programs, ["sed"]);
        assert!(super::evaluate_source_with_programs("echo data | unknown 's/a/b/'").is_err());
        let (output, programs) =
            super::evaluate_source_with_programs("echo 'AzBy' | tr A-Z a-z").unwrap();
        assert_eq!(output, "azby");
        assert_eq!(programs, ["tr"]);
        assert!(evaluate("echo abc | tr a-z A-Z").is_err());
    }

    #[test]
    fn single_quoted_echo_is_inert_data_and_keeps_empty_arguments() {
        assert_eq!(
            evaluate_source("echo '${ordinary}'").unwrap(),
            "${ordinary}"
        );
        assert_eq!(
            evaluate_source("echo '$(opaque);<>|&`data`'").unwrap(),
            "$(opaque);<>|&`data`"
        );
        assert_eq!(evaluate_source("echo '' '' tail").unwrap(), "  tail");
        assert_eq!(evaluate_source("echo 'a b' | sed 's/a/x/'").unwrap(), "x b");
        assert_eq!(evaluate_source("echo").unwrap(), "");
        assert!(
            evaluate("echo '${ordinary}'").is_err(),
            "legacy admission is unchanged"
        );
    }

    #[test]
    fn expansions_options_extra_commands_and_echo_escapes_refuse() {
        for command in [
            "echo ${ordinary}",
            "echo \"$ordinary\"",
            "echo `opaque`",
            "echo -n data",
            "echo '-e' data",
            "echo 'a\\nb'",
            "echo data > out",
            "echo 'data'; opaque",
            "echo 'a'b",
            "env echo data",
            "echo 'unterminated",
            "echo a | sed 's/a/b/' | opaque",
        ] {
            assert!(
                evaluate_source(command).is_err(),
                "unsupported literal echo accepted"
            );
        }
    }
}

fn parse_echo(
    source: &str,
    transform: &impl Fn(&str) -> Result<String, String>,
) -> Result<String, String> {
    let source = trim_horizontal(source);
    let Some(rest) = source.strip_prefix("echo") else {
        return Err("text pipeline must start with the literal echo command".into());
    };
    if !rest.is_empty() && !rest.as_bytes()[0].is_ascii_whitespace() {
        return Err("text pipeline echo command has extra command-name text".into());
    }

    let data = transform(rest.trim_matches([' ', '\t']))?;
    if data.len() > MAX_COMMAND_BYTES {
        return Err("text pipeline echo data exceeds the size limit".into());
    }
    let mut words = Vec::new();
    for word in data.split([' ', '\t']).filter(|word| !word.is_empty()) {
        if !word.bytes().all(is_echo_word_byte) {
            return Err("echo data contains a character outside the path-word subset".into());
        }
        words.push(word);
    }
    if words.first().is_some_and(|word| word.starts_with('-')) {
        return Err("echo options are not supported".into());
    }
    Ok(words.join(" "))
}

const fn is_echo_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'.' | b':' | b'+' | b'=')
}

fn parse_sed_command(source: &str) -> Result<String, String> {
    let source = trim_horizontal(source);
    let Some(rest) = source.strip_prefix("sed") else {
        return Err("text pipeline must use the literal sed command".into());
    };
    if rest.is_empty() || !rest.as_bytes()[0].is_ascii_whitespace() {
        return Err("sed must be followed by one quoted script argument".into());
    }
    let rest = trim_horizontal(rest);
    let Some(script_and_tail) = rest.strip_prefix('\'') else {
        return Err("sed script must be single-quoted".into());
    };
    let Some(script_end) = script_and_tail.find('\'') else {
        return Err("sed script has an unterminated single quote".into());
    };
    let script = &script_and_tail[..script_end];
    if !trim_horizontal(&script_and_tail[script_end + 1..]).is_empty() {
        return Err("sed accepts one script argument and no extra arguments".into());
    }
    if script.len() > MAX_SCRIPT_BYTES {
        return Err("sed script exceeds the size limit".into());
    }
    Ok(script.to_owned())
}

fn trim_horizontal(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}

struct Substitution {
    pattern: String,
    replacement: Vec<u8>,
    global: bool,
}

enum ReplacementPart {
    Literal(char),
    WholeMatch,
    Capture(usize),
}

fn apply_substitution(input: &str, script: &str) -> Result<String, String> {
    let substitution = parse_substitution(script)?;
    if substitution.pattern.is_empty() {
        return Err("sed pattern must not be empty".into());
    }
    let (pattern, capture_count) = translate_pattern(&substitution.pattern)?;
    let regex = RegexBuilder::new(&pattern)
        .size_limit(MAX_REGEX_SIZE)
        .dfa_size_limit(MAX_REGEX_SIZE)
        .build()
        .map_err(|_| "sed pattern exceeds the supported regex subset or size limit".to_string())?;
    let replacement = parse_replacement(&substitution.replacement, capture_count)?;

    let mut output = String::with_capacity(input.len().min(MAX_OUTPUT_BYTES));
    let mut copied_until = 0;
    let mut match_count = 0;
    for captures in regex.captures_iter(input) {
        match_count += 1;
        if match_count > MAX_MATCHES {
            return Err("sed substitution exceeds the match limit".into());
        }
        let whole = captures
            .get(0)
            .ok_or_else(|| "sed pattern produced an invalid match".to_string())?;
        append_bounded(&mut output, &input[copied_until..whole.start()])?;
        expand_replacement(&mut output, &replacement, &captures)?;
        copied_until = whole.end();
        if !substitution.global {
            break;
        }
    }
    append_bounded(&mut output, &input[copied_until..])?;
    Ok(output)
}

fn parse_substitution(script: &str) -> Result<Substitution, String> {
    if !script.starts_with("s/") {
        return Err("sed accepts only one s/// substitution".into());
    }
    let (pattern, after_pattern) = parse_delimited_field(&script.as_bytes()[2..])?;
    let (replacement, flags) = parse_delimited_field(after_pattern)?;
    let global = match flags {
        b"" => false,
        b"g" => true,
        _ => return Err("sed accepts only the optional g flag".into()),
    };
    Ok(Substitution {
        pattern: String::from_utf8(pattern).map_err(|_| "sed pattern is not ASCII".to_string())?,
        replacement: String::from_utf8(replacement)
            .map_err(|_| "sed replacement is not ASCII".to_string())?
            .into_bytes(),
        global,
    })
}

/// Read through the next unescaped slash, unescaping only `\/`.
fn parse_delimited_field(source: &[u8]) -> Result<(Vec<u8>, &[u8]), String> {
    let mut field = Vec::new();
    let mut index = 0;
    while index < source.len() {
        match source[index] {
            b'/' => return Ok((field, &source[index + 1..])),
            b'\\' if source.get(index + 1) == Some(&b'/') => {
                field.push(b'/');
                index += 2;
            }
            b'\\' if source.get(index + 1).is_some() => {
                // Keep a non-delimiter escape as one pair; otherwise the
                // second backslash in `\\` would incorrectly quote `/`.
                field.extend_from_slice(&source[index..index + 2]);
                index += 2;
            }
            byte => {
                field.push(byte);
                index += 1;
            }
        }
    }
    Err("sed substitution is missing a slash delimiter".into())
}

fn translate_pattern(pattern: &str) -> Result<(String, usize), String> {
    let bytes = pattern.as_bytes();
    let mut translated = String::with_capacity(pattern.len());
    let mut captures = 0;
    let mut depth = 0;
    let mut at_start = true;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                let Some(next) = bytes.get(index + 1).copied() else {
                    return Err("sed pattern ends with an unsupported escape".into());
                };
                match next {
                    b'(' => {
                        captures += 1;
                        if captures > 9 {
                            return Err("sed pattern supports at most nine captures".into());
                        }
                        depth += 1;
                        translated.push('(');
                    }
                    b')' => {
                        if depth == 0 {
                            return Err("sed pattern has an unmatched capture close".into());
                        }
                        depth -= 1;
                        translated.push(')');
                    }
                    b'.' | b'^' | b'$' | b'[' | b']' | b'*' | b'\\' => {
                        translated.push_str(&regex::escape(&(next as char).to_string()));
                    }
                    _ => return Err("sed pattern contains an unsupported escape".into()),
                }
                at_start = false;
                index += 2;
            }
            b'^' if at_start => {
                translated.push('^');
                at_start = false;
                index += 1;
            }
            b'.' => {
                translated.push('.');
                at_start = false;
                index += 1;
            }
            byte if is_pattern_literal_byte(byte) => {
                translated.push_str(&regex::escape(&(byte as char).to_string()));
                at_start = false;
                index += 1;
            }
            _ => return Err("sed pattern uses syntax outside the bounded BRE subset".into()),
        }
    }
    if depth != 0 {
        return Err("sed pattern has an unclosed capture".into());
    }
    Ok((translated, captures))
}

const fn is_pattern_literal_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte, b'/' | b'_' | b'-' | b':' | b'+' | b'=' | b' ' | b'\t')
}

fn parse_replacement(
    replacement: &[u8],
    capture_count: usize,
) -> Result<Vec<ReplacementPart>, String> {
    let mut parts = Vec::new();
    let mut index = 0;
    while index < replacement.len() {
        match replacement[index] {
            b'&' => {
                parts.push(ReplacementPart::WholeMatch);
                index += 1;
            }
            b'\\' => {
                let Some(next) = replacement.get(index + 1).copied() else {
                    return Err("sed replacement ends with an unsupported escape".into());
                };
                if (b'1'..=b'9').contains(&next) {
                    let capture = (next - b'0') as usize;
                    if capture > capture_count {
                        return Err("sed replacement references a missing capture".into());
                    }
                    parts.push(ReplacementPart::Capture(capture));
                    index += 2;
                } else if matches!(next, b'&' | b'\\') {
                    parts.push(ReplacementPart::Literal(next as char));
                    index += 2;
                } else {
                    return Err("sed replacement contains an unsupported escape".into());
                }
            }
            byte if byte == b'\t' || (0x20..=0x7e).contains(&byte) => {
                parts.push(ReplacementPart::Literal(byte as char));
                index += 1;
            }
            _ => return Err("sed replacement contains a control or non-ASCII character".into()),
        }
    }
    Ok(parts)
}

fn expand_replacement(
    output: &mut String,
    replacement: &[ReplacementPart],
    captures: &regex::Captures<'_>,
) -> Result<(), String> {
    for part in replacement {
        match part {
            ReplacementPart::Literal(character) => {
                append_char_bounded(output, *character)?;
            }
            ReplacementPart::WholeMatch => {
                let whole = captures
                    .get(0)
                    .ok_or_else(|| "sed pattern produced an invalid match".to_string())?;
                append_bounded(output, whole.as_str())?;
            }
            ReplacementPart::Capture(index) => {
                let capture = captures
                    .get(*index)
                    .ok_or_else(|| "sed capture did not participate in the match".to_string())?;
                append_bounded(output, capture.as_str())?;
            }
        }
    }
    Ok(())
}

fn append_char_bounded(output: &mut String, character: char) -> Result<(), String> {
    let new_len = output
        .len()
        .checked_add(character.len_utf8())
        .ok_or_else(|| "sed output exceeds the size limit".to_string())?;
    if new_len > MAX_OUTPUT_BYTES {
        return Err("sed output exceeds the size limit".into());
    }
    output.push(character);
    Ok(())
}

fn append_bounded(output: &mut String, value: &str) -> Result<(), String> {
    let new_len = output
        .len()
        .checked_add(value.len())
        .ok_or_else(|| "sed output exceeds the size limit".to_string())?;
    if new_len > MAX_OUTPUT_BYTES {
        return Err("sed output exceeds the size limit".into());
    }
    output.push_str(value);
    Ok(())
}

#[cfg(test)]
#[path = "make_text_pipeline_tests.rs"]
mod tests;
