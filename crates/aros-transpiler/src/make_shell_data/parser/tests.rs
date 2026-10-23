use super::{parse, Part, Statement, Word};

fn literal(text: &str, quoted: bool) -> Part {
    Part::Literal {
        text: text.into(),
        quoted,
    }
}

fn word(parts: Vec<Part>) -> Word {
    Word { parts }
}

fn simple(words: Vec<Word>) -> Statement {
    Statement::Simple(words)
}

#[test]
fn parses_control_flow_and_preserves_word_provenance() {
    let parsed = parse(
        r#"for item in one "two words" '$HOME' $HOME; do
if test "$item"; then echo "x"; else echo ''; fi; done"#,
    )
    .unwrap();

    assert_eq!(
        parsed,
        vec![Statement::For {
            name: "item".into(),
            words: vec![
                word(vec![literal("one", false)]),
                word(vec![literal("two words", true)]),
                word(vec![literal("$HOME", true)]),
                word(vec![Part::Variable {
                    name: "HOME".into(),
                    quoted: false,
                }]),
            ],
            body: vec![Statement::If {
                condition: vec![simple(vec![
                    word(vec![literal("test", false)]),
                    word(vec![Part::Variable {
                        name: "item".into(),
                        quoted: true,
                    }]),
                ])],
                then_body: vec![simple(vec![
                    word(vec![literal("echo", false)]),
                    word(vec![literal("x", true)]),
                ])],
                else_body: vec![simple(vec![
                    word(vec![literal("echo", false)]),
                    word(vec![literal("", true)]),
                ])],
            }],
        }]
    );
}

#[test]
fn applies_shell_quote_and_line_continuation_rules() {
    assert_eq!(
        parse(r#"echo "a\qb""#).unwrap(),
        vec![simple(vec![
            word(vec![literal("echo", false)]),
            word(vec![literal("a\\qb", true)]),
        ])]
    );
    assert_eq!(
        parse(r#"echo "a\$b""#).unwrap(),
        vec![simple(vec![
            word(vec![literal("echo", false)]),
            word(vec![literal("a$b", true)]),
        ])]
    );
    assert_eq!(
        parse("echo a\\\nb").unwrap(),
        vec![simple(vec![
            word(vec![literal("echo", false)]),
            word(vec![literal("ab", false)]),
        ])]
    );
    assert_eq!(
        parse(
            r#"echo "a\
b""#
        )
        .unwrap(),
        vec![simple(vec![
            word(vec![literal("echo", false)]),
            word(vec![literal("ab", true)]),
        ])]
    );
    assert!(parse("\\\n").unwrap().is_empty());
    assert!(parse("echo '$() | > ~ `'").is_ok());
    assert!(parse("echo foo~bar").is_ok());
}

#[test]
fn rejects_control_characters_globally() {
    for ch in [
        '\0', '\r', '\u{000b}', '\u{000c}', '\u{001b}', '\u{007f}', '\u{0085}',
    ] {
        assert!(parse(&format!("echo '{ch}'")).is_err());
        assert!(parse(&format!("# comment{ch}\necho ok")).is_err());
    }
}

#[test]
fn rejects_unsupported_syntax_and_empty_compound_bodies() {
    for source in [
        "echo $(touch x)",
        "echo `touch x`",
        "echo x | cat",
        "echo x && cat",
        "echo x >out",
        "echo (x)",
        "echo $@",
        "echo ${1}",
        "echo ~user",
        "for item 'in' x; do echo x; done",
        "for item in; do done",
        "if test; then fi",
        "if test; then echo x; else fi",
        "for item in x; do echo x",
        "echo 'unterminated",
        "echo x; done",
        "while true; do echo x; done",
        "[[ test ]]",
    ] {
        assert!(
            parse(source).is_err(),
            "accepted unsupported input: {source}"
        );
    }
    assert!(parse("for item in; do echo x; done").is_ok());
}

#[test]
fn rejects_empty_and_repeated_semicolon_separators_but_allows_newlines() {
    for source in [
        "; echo x",
        "echo;;x",
        "echo;\n; x",
        "echo\n; x",
        "for i in x;; do echo x; done",
        "for i in x\n; do echo x; done",
        "if test; then; echo x; fi",
    ] {
        assert!(
            parse(source).is_err(),
            "accepted invalid separator use: {source}"
        );
    }
    assert!(parse("echo x\n\n\necho y").is_ok());
    assert!(parse("echo x;\n\necho y;").is_ok());
}

#[test]
fn enforces_nesting_statement_token_and_size_limits() {
    let nested = format!("{}echo x{}", "if test; then ".repeat(33), "; fi".repeat(33));
    assert!(parse(&nested).is_err());

    let many_statements = (0..4097).map(|_| "x").collect::<Vec<_>>().join(";");
    assert!(parse(&many_statements).is_err());

    let many_tokens = (0..16_385).map(|_| "x").collect::<Vec<_>>().join(" ");
    assert!(parse(&many_tokens).is_err());

    let oversized = "x".repeat(1024 * 1024 + 1);
    assert!(parse(&oversized).is_err());
}
