use crate::make_text_pipeline::evaluate;

const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MAX_SCRIPT_BYTES: usize = 4 * 1024;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_MATCHES: usize = 16_384;

fn assert_output(command: &str, expected: &str) {
    let output = evaluate(command)
        .unwrap_or_else(|error| panic!("expected pipeline to be accepted: {error}"));
    assert_eq!(output, expected, "command: {command}");
}

fn assert_rejected(command: &str) -> String {
    let error = evaluate(command)
        .expect_err("expected the closed text-pipeline grammar to reject this command");
    assert!(
        !error.trim().is_empty(),
        "rejection should explain its cause"
    );
    error
}

fn assert_rejected_for(command: &str, cause: &str) {
    let error = assert_rejected(command);
    assert!(
        error.contains(cause),
        "expected rejection cause {cause:?}, got {error:?}"
    );
}

#[test]
fn source_drive_prefix_scripts_convert_only_the_intended_words() {
    assert_output(
        r"echo C:/src/one.c D:/obj/two.o | sed 's/^\(.\):\//\/\1\//'",
        "/C/src/one.c D:/obj/two.o",
    );
    assert_output(
        r"echo C:/src/one.c D:/obj/two.o | sed 's/\(.\):\//\/\1\//g'",
        "/C/src/one.c /D/obj/two.o",
    );
    assert_output(
        r"echo /usr/local/include/item.h | sed 's/^\(.\):\//\/\1\//'",
        "/usr/local/include/item.h",
    );
    assert_output(
        r"echo /usr/local/include/item.h | sed 's/\(.\):\//\/\1\//g'",
        "/usr/local/include/item.h",
    );
}

#[test]
fn echo_data_materialization_cannot_rewrite_the_script_or_introduce_shell_syntax() {
    assert_eq!(
        super::evaluate_with_echo_data("echo TOKEN | sed 's/TOKEN/script/'", &|data| {
            Ok(data.replace("TOKEN", "selected"))
        })
        .unwrap(),
        "selected"
    );
    assert!(
        super::evaluate_with_echo_data("echo TOKEN | sed 's/a/b/'", &|_| {
            Ok("a; touch forbidden".into())
        })
        .unwrap_err()
        .contains("path-word subset")
    );
}

#[test]
fn substitutions_follow_bre_anchor_global_capture_and_replacement_rules() {
    // `+` is literal in the admitted BRE form, not a Rust-regex quantifier.
    assert_output(r"echo a+b | sed 's/a+b/X/'", "X");
    assert_output(r"echo aab | sed 's/a+b/X/'", "aab");
    assert_output(r"echo file.c | sed 's/file\.c/file.o/'", "file.o");
    assert_output(r"echo filexc | sed 's/file\.c/file.o/'", "filexc");
    assert_output(r"echo foo | sed 's/foo/\&/'", "&");
    assert_output(r"echo foo | sed 's/foo/\\/'", "\\");
    assert_output(r"echo aba aba | sed 's/^a/X/'", "Xba aba");
    assert_output(r"echo aba aba | sed 's/a/X/g'", "XbX XbX");
    assert_output(
        r"echo abcdefghi | sed 's/\(a\)\(b\)\(c\)\(d\)\(e\)\(f\)\(g\)\(h\)\(i\)/\9\1/'",
        "ia",
    );
    assert_output(r"echo foo foo | sed 's/foo/[&]/g'", "[foo] [foo]");
    assert_output(r"echo zulu alpha zulu | sed 's/q/Q/'", "zulu alpha zulu");
}

#[test]
fn rejects_shell_syntax_options_and_extra_commands() {
    for (command, cause) in [
        ("echo $(touch marker) | sed 's/a/b/'", "path-word subset"),
        ("echo `touch marker` | sed 's/a/b/'", "path-word subset"),
        ("echo safe; touch marker | sed 's/a/b/'", "path-word subset"),
        ("echo safe > output | sed 's/a/b/'", "path-word subset"),
        ("echo safe >> output | sed 's/a/b/'", "path-word subset"),
        ("echo safe < input | sed 's/a/b/'", "path-word subset"),
        ("echo safe | sed 's/a/b/' | cat", "exactly one pipe"),
        ("echo safe | sed -i 's/a/b/'", "single-quoted"),
        ("echo safe | sed -n 's/a/b/'", "single-quoted"),
        ("echo -n safe | sed 's/a/b/'", "echo options"),
        ("echo -e safe | sed 's/a/b/'", "echo options"),
        ("echo 'quoted data' | sed 's/a/b/'", "path-word subset"),
        ("echo safe | sed 's/a/b/;s/c/d/'", "optional g flag"),
        (
            "echo safe | sed 's/a/b/'; touch marker",
            "no extra arguments",
        ),
        ("echo safe | sed 's/a/b/' >> output", "no extra arguments"),
        (
            "echo safe | sed 's/a/b/'\n",
            "printable ASCII and tabs only",
        ),
    ] {
        assert_rejected_for(command, cause);
    }
}

#[test]
fn rejects_malformed_or_out_of_subset_bre_constructs() {
    for (command, cause) in [
        (r"echo a | sed 's/\(a/b/'", "unclosed capture"),
        (r"echo a | sed 's/a\)/b/'", "unmatched capture close"),
        (r"echo a | sed 's/\1/a/'", "unsupported escape"),
        (r"echo a | sed 's/a/\2/'", "missing capture"),
        (r"echo a | sed 's/a/\9/'", "missing capture"),
        (r"echo a | sed 's/a*/b/'", "outside the bounded BRE subset"),
        (
            r"echo a | sed 's/[ab]/x/'",
            "outside the bounded BRE subset",
        ),
    ] {
        assert_rejected_for(command, cause);
    }

    let ten_captures = format!(
        "{}{}",
        r"\(a\)", r"\(b\)\(c\)\(d\)\(e\)\(f\)\(g\)\(h\)\(i\)\(j\)"
    );
    let command = format!("echo abcdefghij | sed 's/{ten_captures}/x/'");
    assert_rejected_for(&command, "at most nine captures");
}

#[test]
fn command_and_script_byte_limits_have_valid_and_invalid_edges() {
    let suffix = " | sed 's/a/a/'";
    let word_bytes = MAX_COMMAND_BYTES - "echo ".len() - suffix.len();
    let at_limit = format!("echo {}{suffix}", "x".repeat(word_bytes));
    assert_eq!(at_limit.len(), MAX_COMMAND_BYTES);
    let output = evaluate(&at_limit).expect("a command at the byte limit is allowed");
    assert_eq!(output.len(), word_bytes);
    assert!(output.bytes().all(|byte| byte == b'x'));

    let over_limit = format!("echo {}{suffix}", "x".repeat(word_bytes + 1));
    assert_eq!(over_limit.len(), MAX_COMMAND_BYTES + 1);
    let error = assert_rejected(&over_limit);
    assert!(error.to_ascii_lowercase().contains("limit"), "{error}");

    let at_script_limit = format!("s/a/{}/", "z".repeat(MAX_SCRIPT_BYTES - 5));
    assert_eq!(at_script_limit.len(), MAX_SCRIPT_BYTES);
    let command = format!("echo a | sed '{at_script_limit}'");
    assert_eq!(
        evaluate(&command)
            .expect("a script at the byte limit is allowed")
            .len(),
        MAX_SCRIPT_BYTES - 5
    );

    let over_script_limit = format!("s/a/{}/", "z".repeat(MAX_SCRIPT_BYTES - 4));
    assert_eq!(over_script_limit.len(), MAX_SCRIPT_BYTES + 1);
    let command = format!("echo a | sed '{over_script_limit}'");
    let error = assert_rejected(&command);
    assert!(error.to_ascii_lowercase().contains("limit"), "{error}");
}

#[test]
fn output_and_match_limits_reject_only_the_over_limit_case() {
    let replacement = "z".repeat(MAX_SCRIPT_BYTES - 6);
    let script = format!("s/a/{replacement}/g");
    assert_eq!(script.len(), MAX_SCRIPT_BYTES);

    let within_output_limit = format!("echo {} | sed '{script}'", "a".repeat(256));
    let output = evaluate(&within_output_limit).expect("output below the byte limit is allowed");
    assert_eq!(output.len(), 256 * replacement.len());
    assert!(output.len() <= MAX_OUTPUT_BYTES);

    let over_output_limit = format!("echo {} | sed '{script}'", "a".repeat(257));
    let error = assert_rejected(&over_output_limit);
    assert!(error.to_ascii_lowercase().contains("limit"), "{error}");

    let within_match_limit = format!("echo {} | sed 's/a/b/g'", "a".repeat(MAX_MATCHES));
    let output = evaluate(&within_match_limit).expect("the match limit itself is allowed");
    assert_eq!(output.len(), MAX_MATCHES);

    let over_match_limit = format!("echo {} | sed 's/a/b/g'", "a".repeat(MAX_MATCHES + 1));
    let error = assert_rejected(&over_match_limit);
    assert!(error.to_ascii_lowercase().contains("limit"), "{error}");
}
