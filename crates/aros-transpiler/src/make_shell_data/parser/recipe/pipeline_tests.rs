use super::{
    parse_literal_recipe, LiteralRecipeCommand, LiteralRecipeStatement, RecipeRedirectionKind,
};

fn pipeline(statement: &LiteralRecipeStatement) -> &[LiteralRecipeCommand] {
    match statement {
        LiteralRecipeStatement::Pipeline(commands) => commands,
        _ => panic!("expected pipeline statement"),
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
fn parses_three_stage_pipeline_without_spaces_around_pipes() {
    let parsed = parse_literal_recipe("produce input|filter pattern|consume output").unwrap();
    assert_eq!(parsed.len(), 1);

    let stages = pipeline(&parsed[0]);
    assert_eq!(stages.len(), 3);
    assert_words(&stages[0], &["produce", "input"]);
    assert_words(&stages[1], &["filter", "pattern"]);
    assert_words(&stages[2], &["consume", "output"]);
    assert!(stages.iter().all(|stage| stage.redirections().is_empty()));
}

#[test]
fn keeps_quoted_and_escaped_pipes_inside_literal_words() {
    let parsed = parse_literal_recipe("tool 'left|right' escaped\\|pipe | next").unwrap();
    let stages = pipeline(&parsed[0]);

    assert_eq!(stages.len(), 2);
    assert_words(&stages[0], &["tool", "left|right", "escaped|pipe"]);
    assert_words(&stages[1], &["next"]);
}

#[test]
fn retains_pipelines_in_conditions_and_both_branches() {
    let parsed = parse_literal_recipe(
        "if check|condition-filter; then build|compress|package; else report|diagnose; fi",
    )
    .unwrap();
    let [LiteralRecipeStatement::If {
        condition,
        then_body,
        else_body,
    }] = parsed.as_slice()
    else {
        panic!("expected one if statement");
    };

    let condition_stages = pipeline(&condition[0]);
    assert_eq!(condition_stages.len(), 2);
    assert_words(&condition_stages[0], &["check"]);
    assert_words(&condition_stages[1], &["condition-filter"]);

    let then_stages = pipeline(&then_body[0]);
    assert_eq!(then_stages.len(), 3);
    assert_words(&then_stages[0], &["build"]);
    assert_words(&then_stages[1], &["compress"]);
    assert_words(&then_stages[2], &["package"]);

    let else_stages = pipeline(&else_body[0]);
    assert_eq!(else_stages.len(), 2);
    assert_words(&else_stages[0], &["report"]);
    assert_words(&else_stages[1], &["diagnose"]);
}

#[test]
fn accepts_newline_comment_and_line_continuation_after_a_pipe() {
    for recipe in [
        "first |\n second",
        "first | # stage comment\n second",
        "first|#stage comment\nsecond",
        "first | \\\n second",
    ] {
        let parsed = parse_literal_recipe(recipe).unwrap_or_else(|error| {
            panic!("rejected supported line break after pipe in {recipe:?}: {error}")
        });
        let stages = pipeline(&parsed[0]);
        assert_eq!(stages.len(), 2, "recipe: {recipe:?}");
        assert_words(&stages[0], &["first"]);
        assert_words(&stages[1], &["second"]);
    }
}

#[test]
fn preserves_each_stages_redirections_in_source_order() {
    let parsed =
        parse_literal_recipe("first 2>before >after | second <input 3>>append 1>&2 | third 0<&3")
            .unwrap();
    let stages = pipeline(&parsed[0]);
    assert_eq!(stages.len(), 3);

    let first = stages[0].redirections();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].descriptor(), 2);
    assert_eq!(first[0].kind(), RecipeRedirectionKind::Output);
    assert_eq!(first[0].target(), "before");
    assert_eq!(first[1].descriptor(), 1);
    assert_eq!(first[1].kind(), RecipeRedirectionKind::Output);
    assert_eq!(first[1].target(), "after");

    let second = stages[1].redirections();
    assert_eq!(second.len(), 3);
    assert_eq!(second[0].descriptor(), 0);
    assert_eq!(second[0].kind(), RecipeRedirectionKind::Input);
    assert_eq!(second[0].target(), "input");
    assert_eq!(second[1].descriptor(), 3);
    assert_eq!(second[1].kind(), RecipeRedirectionKind::Append);
    assert_eq!(second[1].target(), "append");
    assert_eq!(second[2].descriptor(), 1);
    assert_eq!(second[2].kind(), RecipeRedirectionKind::DuplicateOutput);
    assert_eq!(second[2].target(), "2");

    let third = stages[2].redirections();
    assert_eq!(third.len(), 1);
    assert_eq!(third[0].descriptor(), 0);
    assert_eq!(third[0].kind(), RecipeRedirectionKind::DuplicateInput);
    assert_eq!(third[0].target(), "3");
}

#[test]
fn rejects_incomplete_unsupported_and_dynamic_pipeline_stages_everywhere() {
    for recipe in [
        "| first",
        "first |",
        "first |; second",
        "first | || second",
        "first |& second",
        "! first | second",
        "first | ! second",
        "first | if true; then second; fi",
        "first | second $HOME",
        "first | second *.c",
        "first | NAME=value command",
        "first | NAME=value",
        "if check; then safe; else first | second $HOME; fi",
        "if check; then safe | NAME=value command; else ok; fi",
        "if check; then ok; else first | second *.c; fi",
    ] {
        assert!(
            parse_literal_recipe(recipe).is_err(),
            "accepted unsupported pipeline recipe: {recipe}"
        );
    }
}

#[test]
fn counts_every_pipeline_stage_against_the_command_budget() {
    let at_limit = (0..4096).map(|_| "x").collect::<Vec<_>>().join("|");
    let parsed = parse_literal_recipe(&at_limit).unwrap();
    assert_eq!(pipeline(&parsed[0]).len(), 4096);

    let over_limit = (0..4097).map(|_| "x").collect::<Vec<_>>().join("|");
    assert!(parse_literal_recipe(&over_limit).is_err());
}

#[test]
fn pipeline_debug_and_errors_do_not_expose_recipe_values() {
    let parsed =
        parse_literal_recipe("private-producer private-argument|private-filter 2>private-target")
            .unwrap();
    let debug = format!("{parsed:?}");
    for private_value in [
        "private-producer",
        "private-argument",
        "private-filter",
        "private-target",
    ] {
        assert!(
            !debug.contains(private_value),
            "debug exposed {private_value}"
        );
    }

    let error = parse_literal_recipe(
        "private-producer|private-filter 'private payload' $(private-substitution)",
    )
    .unwrap_err();
    for private_value in [
        "private-producer",
        "private-filter",
        "private payload",
        "private-substitution",
    ] {
        assert!(
            !error.contains(private_value),
            "error exposed {private_value}"
        );
    }
}

#[test]
fn existing_shell_data_parser_continues_to_reject_pipes() {
    assert!(super::super::parse("first | second").is_err());
}
