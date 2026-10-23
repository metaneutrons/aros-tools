use super::{
    parse_literal_recipe, LiteralRecipeCommand, LiteralRecipeStatement, RecipeRedirectionKind,
};

fn command(statement: &LiteralRecipeStatement) -> &LiteralRecipeCommand {
    match statement {
        LiteralRecipeStatement::Command(command) => command,
        _ => panic!("expected command statement"),
    }
}

fn assert_words(command: &LiteralRecipeCommand, expected: &[&str]) {
    assert_eq!(
        command
            .words()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn decodes_literal_quotes_escapes_and_empty_arguments() {
    let parsed = parse_literal_recipe(
        r#"tool plain 'single quoted' "double quoted" pre' mix '"post" '' "" a''b escaped\ space a\;b "a\$b" "a\qb" '$HOME'"#,
    )
    .unwrap();

    assert_eq!(parsed.len(), 1);
    assert_words(
        command(&parsed[0]),
        &[
            "tool",
            "plain",
            "single quoted",
            "double quoted",
            "pre mix post",
            "",
            "",
            "ab",
            "escaped space",
            "a;b",
            "a$b",
            "a\\qb",
            "$HOME",
        ],
    );
    assert!(command(&parsed[0]).redirections().is_empty());
}

#[test]
fn retains_list_order_and_separates_io_numbers_from_words() {
    let parsed = parse_literal_recipe("first; tool 2>err; tool 2 >out\nlast").unwrap();

    assert_eq!(parsed.len(), 4);
    assert_words(command(&parsed[0]), &["first"]);

    let adjacent = command(&parsed[1]);
    assert_words(adjacent, &["tool"]);
    assert_eq!(adjacent.redirections().len(), 1);
    assert_eq!(adjacent.redirections()[0].descriptor(), 2);
    assert_eq!(
        adjacent.redirections()[0].kind(),
        RecipeRedirectionKind::Output
    );
    assert_eq!(adjacent.redirections()[0].target(), "err");

    let separated = command(&parsed[2]);
    assert_words(separated, &["tool", "2"]);
    assert_eq!(separated.redirections().len(), 1);
    assert_eq!(separated.redirections()[0].descriptor(), 1);
    assert_eq!(separated.redirections()[0].target(), "out");

    let quoted_number = parse_literal_recipe("tool \"2\">out").unwrap();
    let quoted_number = command(&quoted_number[0]);
    assert_words(quoted_number, &["tool", "2"]);
    assert_eq!(quoted_number.redirections()[0].descriptor(), 1);
}

#[test]
fn io_numbers_join_unquoted_line_continuations_before_token_recognition() {
    for recipe in ["tool 2\\\n>out", "tool \\\n2>out", "tool 2\\\n1>out"] {
        let parsed = parse_literal_recipe(recipe).unwrap();
        let parsed = command(&parsed[0]);
        assert_words(parsed, &["tool"]);
        assert_eq!(
            parsed.redirections()[0].descriptor(),
            if recipe.contains("\n1") { 21 } else { 2 }
        );
    }
    for recipe in ["tool '2'\\\n>out", "tool 2\\\n >out", "tool 2\\\narg>out"] {
        let parsed = parse_literal_recipe(recipe).unwrap();
        let parsed = command(&parsed[0]);
        assert_eq!(parsed.words().len(), 2);
        assert_eq!(parsed.redirections()[0].descriptor(), 1);
    }
}

#[test]
fn preserves_redirection_kind_descriptor_target_and_source_order() {
    let parsed = parse_literal_recipe("tool <input >first 2>&1 >>append 0<&3 1>&- 2>last").unwrap();
    let parsed = command(&parsed[0]);
    assert_words(parsed, &["tool"]);

    let redirections = parsed.redirections();
    assert_eq!(redirections.len(), 7);
    let expected = [
        (0, RecipeRedirectionKind::Input, "input"),
        (1, RecipeRedirectionKind::Output, "first"),
        (2, RecipeRedirectionKind::DuplicateOutput, "1"),
        (1, RecipeRedirectionKind::Append, "append"),
        (0, RecipeRedirectionKind::DuplicateInput, "3"),
        (1, RecipeRedirectionKind::DuplicateOutput, "-"),
        (2, RecipeRedirectionKind::Output, "last"),
    ];
    for (redirection, (descriptor, kind, target)) in redirections.iter().zip(expected) {
        assert_eq!(redirection.descriptor(), descriptor);
        assert_eq!(redirection.kind(), kind);
        assert_eq!(redirection.target(), target);
    }
}

#[test]
fn retains_nested_conditions_and_both_diagnostic_branches() {
    let parsed = parse_literal_recipe(
        "before; if check; then build; if verify; then install; else report verify-failed; fi; else printf 'check failed' >&2; fi; after",
    )
    .unwrap();

    assert_eq!(parsed.len(), 3);
    assert_words(command(&parsed[0]), &["before"]);
    assert_words(command(&parsed[2]), &["after"]);

    let LiteralRecipeStatement::If {
        condition,
        then_body,
        else_body,
    } = &parsed[1]
    else {
        panic!("expected outer if statement");
    };
    assert_eq!(condition.len(), 1);
    assert_words(command(&condition[0]), &["check"]);
    assert_eq!(then_body.len(), 2);
    assert_words(command(&then_body[0]), &["build"]);

    let LiteralRecipeStatement::If {
        condition,
        then_body,
        else_body: inner_else,
    } = &then_body[1]
    else {
        panic!("expected nested if statement");
    };
    assert_words(command(&condition[0]), &["verify"]);
    assert_words(command(&then_body[0]), &["install"]);
    assert_words(command(&inner_else[0]), &["report", "verify-failed"]);

    assert_eq!(else_body.len(), 1);
    let diagnostic = command(&else_body[0]);
    assert_words(diagnostic, &["printf", "check failed"]);
    assert_eq!(diagnostic.redirections().len(), 1);
    assert_eq!(diagnostic.redirections()[0].descriptor(), 1);
    assert_eq!(
        diagnostic.redirections()[0].kind(),
        RecipeRedirectionKind::DuplicateOutput
    );
    assert_eq!(diagnostic.redirections()[0].target(), "2");
}

#[test]
fn rejects_dynamic_or_unsupported_syntax_even_in_inactive_branches() {
    for recipe in [
        "echo $HOME",
        "echo ${1}",
        "echo $(whoami)",
        "echo `whoami`",
        "NAME=value command",
        "NAME=\"quoted value\" command",
        "NAME='quoted value'",
        "for item in one; do echo item; done",
        "while true; do echo item; done",
        "cat <<EOF\nbody\nEOF",
        "echo *.c",
        "if true; then echo safe; else echo $HOME; fi",
        "if true; then echo safe; else echo $(touch marker); fi",
        "if true; then echo safe; else echo x | cat $HOME; fi",
        "if true; then echo safe; else NAME=value command; fi",
        "if true; then echo safe; else for item in x; do echo item; done; fi",
        "if true; then echo safe; else cat <<EOF\nbody\nEOF\nfi",
        "if true; then echo safe; else echo *.c; fi",
        "if true; then echo safe; else echo x; elif false; then echo y; fi",
        "if true; then echo safe; else fi",
        "echo value >\"$TARGET\"",
        "echo value 2>&not-a-descriptor",
        "echo value 2>&+1",
        ">out if check; then echo yes; fi",
    ] {
        assert!(
            parse_literal_recipe(recipe).is_err(),
            "accepted unsupported recipe: {recipe}"
        );
    }
}

#[test]
fn debug_and_errors_do_not_expose_recipe_values() {
    let parsed = parse_literal_recipe("private-command private-argument 2>private-target").unwrap();
    let debug = format!("{parsed:?}");
    for private_value in ["private-command", "private-argument", "private-target"] {
        assert!(
            !debug.contains(private_value),
            "debug exposed {private_value}"
        );
    }

    let error = parse_literal_recipe("private-command 'private payload' $(private-substitution)")
        .unwrap_err();
    for private_value in ["private-command", "private payload", "private-substitution"] {
        assert!(
            !error.contains(private_value),
            "error exposed {private_value}"
        );
    }
}

#[test]
fn enforces_statement_nesting_token_and_byte_budgets() {
    let at_statement_limit = (0..4096).map(|_| "x").collect::<Vec<_>>().join(";");
    assert_eq!(
        parse_literal_recipe(&at_statement_limit).unwrap().len(),
        4096
    );
    let over_statement_limit = (0..4097).map(|_| "x").collect::<Vec<_>>().join(";");
    assert!(parse_literal_recipe(&over_statement_limit).is_err());

    let nested = |depth: usize| {
        format!(
            "{}echo x{}",
            "if check; then ".repeat(depth),
            "; fi".repeat(depth)
        )
    };
    assert!(parse_literal_recipe(&nested(32)).is_ok());
    assert!(parse_literal_recipe(&nested(33)).is_err());

    let at_token_limit = (0..16_384).map(|_| "x").collect::<Vec<_>>().join(" ");
    assert_eq!(
        command(&parse_literal_recipe(&at_token_limit).unwrap()[0])
            .words()
            .len(),
        16_384
    );
    let over_token_limit = (0..16_385).map(|_| "x").collect::<Vec<_>>().join(" ");
    assert!(parse_literal_recipe(&over_token_limit).is_err());

    let oversized = "x".repeat(1024 * 1024 + 1);
    assert!(parse_literal_recipe(&oversized).is_err());
}
