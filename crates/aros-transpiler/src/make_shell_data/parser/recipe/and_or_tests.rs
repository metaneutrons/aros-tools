use super::{
    parse_literal_recipe, LiteralRecipeCommand, LiteralRecipeStatement, RecipeAndOrOperator,
};

fn command(statement: &LiteralRecipeStatement) -> &LiteralRecipeCommand {
    match statement {
        LiteralRecipeStatement::Command(command) => command,
        _ => panic!("expected command statement"),
    }
}

fn pipeline(statement: &LiteralRecipeStatement) -> &[LiteralRecipeCommand] {
    match statement {
        LiteralRecipeStatement::Pipeline(commands) => commands,
        _ => panic!("expected pipeline statement"),
    }
}

fn and_or(
    statement: &LiteralRecipeStatement,
) -> (
    &LiteralRecipeStatement,
    &[(RecipeAndOrOperator, LiteralRecipeStatement)],
) {
    match statement {
        LiteralRecipeStatement::AndOr { first, rest } => (first, rest),
        _ => panic!("expected AND/OR statement"),
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
fn retains_mixed_and_or_order_with_pipelines_as_tighter_arms() {
    let parsed =
        parse_literal_recipe("produce|filter && merge || report|diagnose && finish").unwrap();
    let [statement] = parsed.as_slice() else {
        panic!("expected one statement");
    };
    let (first, rest) = and_or(statement);

    let first_stages = pipeline(first);
    assert_eq!(first_stages.len(), 2);
    assert_words(&first_stages[0], &["produce"]);
    assert_words(&first_stages[1], &["filter"]);

    assert_eq!(rest.len(), 3);
    assert_eq!(rest[0].0, RecipeAndOrOperator::And);
    assert_words(command(&rest[0].1), &["merge"]);
    assert_eq!(rest[1].0, RecipeAndOrOperator::Or);
    let report_stages = pipeline(&rest[1].1);
    assert_eq!(report_stages.len(), 2);
    assert_words(&report_stages[0], &["report"]);
    assert_words(&report_stages[1], &["diagnose"]);
    assert_eq!(rest[2].0, RecipeAndOrOperator::And);
    assert_words(command(&rest[2].1), &["finish"]);
}

#[test]
fn retains_if_inside_chain_and_chains_inside_its_condition_and_branches() {
    let parsed = parse_literal_recipe(
        "if inspect && verify|summarize; then build && report; else recover || explain; fi && finalize",
    )
    .unwrap();
    let [statement] = parsed.as_slice() else {
        panic!("expected one statement");
    };
    let (first, rest) = and_or(statement);
    let LiteralRecipeStatement::If {
        condition,
        then_body,
        else_body,
    } = first
    else {
        panic!("expected if as first AND/OR arm");
    };
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].0, RecipeAndOrOperator::And);
    assert_words(command(&rest[0].1), &["finalize"]);

    let (condition_first, condition_rest) = and_or(&condition[0]);
    assert_words(command(condition_first), &["inspect"]);
    assert_eq!(condition_rest.len(), 1);
    assert_eq!(condition_rest[0].0, RecipeAndOrOperator::And);
    let condition_stages = pipeline(&condition_rest[0].1);
    assert_eq!(condition_stages.len(), 2);
    assert_words(&condition_stages[0], &["verify"]);
    assert_words(&condition_stages[1], &["summarize"]);

    let (then_first, then_rest) = and_or(&then_body[0]);
    assert_words(command(then_first), &["build"]);
    assert_eq!(then_rest[0].0, RecipeAndOrOperator::And);
    assert_words(command(&then_rest[0].1), &["report"]);
    let (else_first, else_rest) = and_or(&else_body[0]);
    assert_words(command(else_first), &["recover"]);
    assert_eq!(else_rest[0].0, RecipeAndOrOperator::Or);
    assert_words(command(&else_rest[0].1), &["explain"]);
}

#[test]
fn accepts_newline_comments_and_continuation_after_and_or_operators() {
    for (recipe, operator) in [
        ("first &&\n second", RecipeAndOrOperator::And),
        ("first && # comment\n second", RecipeAndOrOperator::And),
        ("first ||# comment\n second", RecipeAndOrOperator::Or),
        ("first \\\n&& second", RecipeAndOrOperator::And),
    ] {
        let parsed = parse_literal_recipe(recipe).unwrap_or_else(|error| {
            panic!("rejected supported line break after operator in {recipe:?}: {error}")
        });
        let [statement] = parsed.as_slice() else {
            panic!("expected one statement for {recipe:?}");
        };
        let (first, rest) = and_or(statement);
        assert_words(command(first), &["first"]);
        assert_eq!(rest.len(), 1, "recipe: {recipe:?}");
        assert_eq!(rest[0].0, operator, "recipe: {recipe:?}");
        assert_words(command(&rest[0].1), &["second"]);
    }

    assert!(parse_literal_recipe("first\n&& second").is_err());
}

#[test]
fn keeps_quoted_and_escaped_operators_as_word_data() {
    let parsed =
        parse_literal_recipe(r#"tool '&&' "||" \&\& \|\| left'&&'right next"||"word"#).unwrap();
    let [statement] = parsed.as_slice() else {
        panic!("expected one command");
    };

    assert_words(
        command(statement),
        &["tool", "&&", "||", "&&", "||", "left&&right", "next||word"],
    );
}

#[test]
fn rejects_missing_operands_single_ampersands_and_triple_operators() {
    for recipe in [
        "&& first",
        "first &&",
        "first ||",
        "first && || second",
        "first & second",
        "first &&& second",
        "first ||| second",
    ] {
        assert!(
            parse_literal_recipe(recipe).is_err(),
            "accepted malformed AND/OR recipe: {recipe}"
        );
    }
}

#[test]
fn rejects_dynamic_words_and_loops_after_a_false_and_arm() {
    for recipe in [
        "false && command $HOME",
        "false && command $(substitution)",
        "false && for item in one; do command; done",
    ] {
        assert!(
            parse_literal_recipe(recipe).is_err(),
            "accepted unsupported AND/OR recipe: {recipe}"
        );
    }
}

#[test]
fn enforces_statement_and_nesting_limits_for_and_or_trees() {
    fn nested_ifs(count: usize) -> String {
        format!(
            "{}leaf{}",
            "if check; then ".repeat(count),
            "; fi".repeat(count)
        )
    }

    let at_statement_limit = vec!["x"; 4096].join(" && ");
    let parsed = parse_literal_recipe(&at_statement_limit).unwrap();
    let [statement] = parsed.as_slice() else {
        panic!("expected one AND/OR list");
    };
    assert_eq!(and_or(statement).1.len(), 4095);

    let over_statement_limit = vec!["x"; 4097].join(" && ");
    assert!(parse_literal_recipe(&over_statement_limit).is_err());

    assert!(parse_literal_recipe(&nested_ifs(32)).is_ok());
    assert!(parse_literal_recipe(&nested_ifs(33)).is_err());
}
