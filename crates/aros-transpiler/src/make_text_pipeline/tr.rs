//! Bounded ASCII transliteration for the source text-pipeline language.
//!
//! Only literal bytes and ascending printable-ASCII ranges are admitted. The
//! set lengths must match and the source set must be unique, avoiding tr cases
//! whose result can vary between utility implementations.

const MAX_OPERAND_BYTES: usize = 4 * 1024;
const MAX_EXPANDED_BYTES: usize = 128;

pub(super) fn is_tr_command(source: &str) -> bool {
    let source = trim_horizontal(source);
    source
        .strip_prefix("tr")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
}

pub(super) fn evaluate(input: &str, command: &str) -> Result<String, String> {
    let (source_operand, target_operand) = parse_command(command)?;
    translate(input, source_operand, target_operand)
}

fn parse_command(command: &str) -> Result<(&str, &str), String> {
    let command = trim_horizontal(command);
    let rest = command
        .strip_prefix("tr")
        .ok_or("text pipeline must use the literal tr command")?;
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return Err("tr command has extra command-name text".into());
    }

    let mut rest = rest.trim_start_matches([' ', '\t']);
    let source = take_operand(&mut rest)?;
    rest = rest.trim_start_matches([' ', '\t']);
    let target = take_operand(&mut rest)?;
    if !trim_horizontal(rest).is_empty() {
        return Err("tr accepts exactly two operands and no extra arguments".into());
    }
    if source.is_empty() || target.is_empty() {
        return Err("tr operands must be nonempty".into());
    }
    if source.starts_with('-') || target.starts_with('-') {
        return Err("tr options are not supported".into());
    }
    Ok((source, target))
}

fn take_operand<'a>(source: &mut &'a str) -> Result<&'a str, String> {
    let rest = *source;
    if let Some(quoted) = rest.strip_prefix('\'') {
        let end = quoted
            .find('\'')
            .ok_or("tr operand has an unterminated single quote")?;
        let operand = &quoted[..end];
        let tail = &quoted[end + 1..];
        if !tail.is_empty() && !tail.starts_with([' ', '\t']) {
            return Err("tr operand concatenation is unsupported".into());
        }
        if operand.contains('\\') {
            return Err("tr escapes are not supported".into());
        }
        *source = tail;
        return Ok(operand);
    }

    let end = rest.find([' ', '\t']).unwrap_or(rest.len());
    let operand = &rest[..end];
    if operand.is_empty() {
        return Err("tr requires exactly two operands".into());
    }
    if !operand.bytes().all(is_safe_unquoted_byte) {
        return Err("unquoted tr operand contains unsupported shell syntax".into());
    }
    *source = &rest[end..];
    Ok(operand)
}

const fn is_safe_unquoted_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte, b'_' | b'-' | b'.' | b'/' | b':' | b'+' | b'=' | b',')
}

fn translate(input: &str, source_operand: &str, target_operand: &str) -> Result<String, String> {
    if !input.is_ascii() {
        return Err("tr data model requires ASCII input".into());
    }
    let source = expand_set(source_operand)?;
    let target = expand_set(target_operand)?;
    if source.len() != target.len() {
        return Err("tr operands must expand to sets of equal length".into());
    }

    let mut mapping = [None; 128];
    for (source, target) in source.into_iter().zip(target) {
        let slot = mapping
            .get_mut(usize::from(source))
            .ok_or_else(|| "tr source operand is not ASCII".to_string())?;
        if slot.is_some() {
            return Err("tr source set must not contain duplicate bytes".into());
        }
        *slot = Some(target);
    }

    let mut output = String::with_capacity(input.len());
    for byte in input.bytes() {
        let translated = mapping
            .get(usize::from(byte))
            .copied()
            .flatten()
            .unwrap_or(byte);
        output.push(char::from(translated));
    }
    Ok(output)
}

fn expand_set(operand: &str) -> Result<Vec<u8>, String> {
    if operand.len() > MAX_OPERAND_BYTES {
        return Err("tr operand exceeds the size limit".into());
    }
    let bytes = operand.as_bytes();
    if !bytes
        .iter()
        .all(|byte| *byte == b'\t' || (0x20..=0x7e).contains(byte))
    {
        return Err("tr operands accept printable ASCII and tabs only".into());
    }
    if bytes.iter().any(|byte| matches!(byte, b'[' | b']')) {
        return Err("tr character classes and repeats are not supported".into());
    }
    if bytes.contains(&b'\\') {
        return Err("tr escapes are not supported".into());
    }

    let mut expanded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let first = bytes[index];
        if first == b'-' {
            if index != 0 && index + 1 != bytes.len() {
                return Err("tr operand contains an ambiguous range".into());
            }
            if expanded.len() == MAX_EXPANDED_BYTES {
                return Err("tr expanded operand exceeds the ASCII set bound".into());
            }
            expanded.push(first);
            index += 1;
            continue;
        }

        if bytes.get(index + 1) == Some(&b'-') {
            if let Some(&last) = bytes.get(index + 2) {
                if last == b'-' {
                    return Err("tr operand contains an ambiguous range".into());
                }
                if !(0x20..=0x7e).contains(&first) || !(0x20..=0x7e).contains(&last) {
                    return Err("tr ranges require printable ASCII endpoints".into());
                }
                if first > last {
                    return Err("tr ranges must be ascending ASCII".into());
                }
                let range_length = usize::from(last - first) + 1;
                if expanded.len() + range_length > MAX_EXPANDED_BYTES {
                    return Err("tr expanded operand exceeds the ASCII set bound".into());
                }
                expanded.extend(first..=last);
                index += 3;
                continue;
            }
        }

        if expanded.len() == MAX_EXPANDED_BYTES {
            return Err("tr expanded operand exceeds the ASCII set bound".into());
        }
        expanded.push(first);
        index += 1;
    }
    Ok(expanded)
}

fn trim_horizontal(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}

#[cfg(test)]
mod tests {
    use super::translate;
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[test]
    fn literal_and_range_operands_translate_only_their_source_bytes() {
        assert_eq!(
            super::super::evaluate_source("echo 'abcxyz' | tr a-c X-Z").unwrap(),
            "XYZxyz"
        );
        assert_eq!(
            super::super::evaluate_source("echo 'cab?' | tr 'abc?' 'XYZ!'").unwrap(),
            "ZXY!"
        );
        assert_eq!(
            super::super::evaluate_source("echo 'A-Z' | tr A-Z a-z").unwrap(),
            "a-z"
        );
        assert_eq!(
            super::super::evaluate_source("echo MiXeD-123 | tr A-Z a-z").unwrap(),
            "mixed-123"
        );
    }

    #[test]
    fn implementation_sensitive_set_lengths_and_duplicates_are_refused() {
        for command in [
            "echo abc | tr a-c XY",
            "echo abc | tr a-b XYZW",
            "echo abc | tr abca WXYZ",
            "echo abc | tr a-ca WXYZ",
        ] {
            assert!(
                super::super::evaluate_source(command).is_err(),
                "implementation-sensitive tr set was accepted: {command}"
            );
        }
    }

    #[test]
    fn expansion_work_is_bounded_and_non_ascii_data_is_not_reencoded() {
        assert!(super::expand_set(&"a-z".repeat(6))
            .unwrap_err()
            .contains("ASCII set bound"));
        assert!(translate("é", "a", "b")
            .unwrap_err()
            .contains("ASCII input"));
    }

    #[test]
    fn options_classes_escapes_extra_arguments_and_shell_syntax_are_refused() {
        for command in [
            "echo data | tr '' x",
            "echo data | tr x ''",
            "echo data | tr -s a b",
            "echo data | tr -d a b",
            "echo data | tr -c a b",
            "echo data | tr -s a",
            "echo data | tr -d a",
            "echo data | tr -c a",
            "echo data | tr '[[:lower:]]' x",
            "echo data | tr '[a*3]' xxx",
            "echo data | tr 'a\\n' 'x?'",
            "echo data | tr a b c",
            "echo data | tr a b; opaque",
            "echo data | tr a b > output",
            "echo data | unknown a b",
            "echo data | tr z-a Z-A",
            "echo data | tr a b | unknown",
        ] {
            assert!(
                super::super::evaluate_source(command).is_err(),
                "unsupported tr syntax was accepted: {command}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "requires explicit AROS_TEST_NATIVE_TR=1 and /usr/bin/tr; no compiler build"]
    fn native_tr_c_locale_comparison_is_opt_in() {
        assert_eq!(std::env::var("AROS_TEST_NATIVE_TR").as_deref(), Ok("1"));

        for (input, source, target) in [
            ("abcxyz\n", "a-c", "X-Z"),
            ("cab?\n", "abc?", "XYZ!"),
            ("A-Z\n", "A-Z", "a-z"),
        ] {
            let expected = translate(input, source, target).unwrap();
            let mut child = Command::new("/usr/bin/tr")
                .env_clear()
                .env("LC_ALL", "C")
                .arg(source)
                .arg(target)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("/usr/bin/tr should be available for the opt-in comparison");
            child
                .stdin
                .as_mut()
                .expect("stdin was piped")
                .write_all(input.as_bytes())
                .expect("native tr should read the fixture");
            let output = child
                .wait_with_output()
                .expect("native tr should finish the fixture");
            assert!(output.status.success());
            assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        }
    }
}
