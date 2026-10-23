use super::*;
use aros_transpiler::make_shell_effects::gnu_query::LiteralSourceQueryVariable;

fn render_source_bindings(
    directory: &PrivateNativeCaptureDirectory,
    command: &str,
    frozen: &[LiteralQueryVariable<'_>],
    source: &[LiteralSourceQueryVariable<'_>],
) -> GnuShellQuery {
    GnuShellQuery::render_with_source_bindings(directory.path(), command, frozen, source)
        .expect("render literal GNU query with selected Source bindings")
}

fn execute_query(
    fixture: &Fixture,
    directory: &PrivateNativeCaptureDirectory,
    bundle: &GnuShellQuery,
) -> GnuShellQueryResult {
    let budget = GnuShellQueryBudget::new(1, 4096).expect("valid query budget");
    budget
        .execute(
            &fixture.request(directory, bundle),
            &CancellationToken::default(),
            || Ok(Duration::from_secs(30)),
        )
        .expect("selected GNU query should complete")
}

#[test]
fn ordinary_source_assignment_preserves_command_line_precedence() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("source-cli-precedence");
    let source = [LiteralSourceQueryVariable {
        name: "QUERY_ARGUMENT",
        value: Some("source-value"),
        export: Some(true),
    }];
    let bundle =
        render_source_bindings(&directory, "printf '%s' \"$QUERY_ARGUMENT\"", &[], &source);

    let result = execute_query(&fixture, &directory, &bundle);
    assert_eq!(result.value, "selected-argument");
    assert_eq!(result.status, 0);
}

#[test]
fn ordinary_source_undefine_preserves_command_line_variable() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("source-cli-undefine");
    let source = [LiteralSourceQueryVariable {
        name: "QUERY_ARGUMENT",
        value: None,
        export: None,
    }];
    let bundle =
        render_source_bindings(&directory, "printf '%s' \"$QUERY_ARGUMENT\"", &[], &source);

    let result = execute_query(&fixture, &directory, &bundle);
    assert_eq!(result.value, "selected-argument");
    assert_eq!(result.status, 0);
}

#[test]
fn source_assignment_preserves_inherited_environment_export_selection() {
    let fixture = Fixture::new(&[("QUERY_SELECTION", "inherited-value")], None);
    let directory = fixture.query_directory("source-inherited-export");
    let source = [LiteralSourceQueryVariable {
        name: "QUERY_SELECTION",
        value: Some("source-value"),
        export: None,
    }];
    let bundle = render_source_bindings(
        &directory,
        "if test \"${QUERY_SELECTION+x}\" = x; then printf '%s' \"$QUERY_SELECTION\"; else printf absent; fi",
        &[],
        &source,
    );

    let result = execute_query(&fixture, &directory, &bundle);
    assert_eq!(result.value, "source-value");
    assert_eq!(result.status, 0);
}

#[test]
fn source_makeflags_assignment_keeps_gnu_normalized_original_flags() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("source-makeflags-normalized");
    let source = [LiteralSourceQueryVariable {
        name: "MAKEFLAGS",
        value: Some("s"),
        export: None,
    }];
    let bundle = render_source_bindings(&directory, "printf '%s' \"$MAKEFLAGS\"", &[], &source);

    let result = execute_query(&fixture, &directory, &bundle);
    assert_eq!(result.status, 0);
    let option_word = result
        .value
        .split_whitespace()
        .next()
        .expect("GNU normalized MAKEFLAGS contains its option word");
    assert!(option_word.contains('r'));
    assert!(option_word.contains('s'));
}

#[test]
fn frozen_override_still_supersedes_command_line_assignment() {
    let fixture = Fixture::new(&[], None);
    let directory = fixture.query_directory("frozen-cli-override");
    let frozen = [LiteralQueryVariable {
        name: "QUERY_ARGUMENT",
        value: Some("frozen-value"),
        export: Some(true),
    }];
    let bundle =
        render_source_bindings(&directory, "printf '%s' \"$QUERY_ARGUMENT\"", &frozen, &[]);

    let result = execute_query(&fixture, &directory, &bundle);
    assert_eq!(result.value, "frozen-value");
    assert_eq!(result.status, 0);
}

#[test]
fn source_undefine_unexports_environment_and_literal_value_keeps_dollar_and_newline() {
    let fixture = Fixture::new(&[("QUERY_SELECTION", "inherited-value")], None);
    let directory = fixture.query_directory("source-literals");
    let source = [
        LiteralSourceQueryVariable {
            name: "QUERY_SELECTION",
            value: None,
            export: Some(false),
        },
        LiteralSourceQueryVariable {
            name: "QUERY_LITERAL",
            value: Some("price=$5\nsecond line"),
            export: Some(true),
        },
    ];
    let bundle = render_source_bindings(
        &directory,
        "if test \"${QUERY_SELECTION+x}\" = x; then printf present; else printf absent; fi; printf '|%s' \"$QUERY_LITERAL\"",
        &[],
        &source,
    );

    let result = execute_query(&fixture, &directory, &bundle);
    assert_eq!(result.value, "absent|price=$5 second line");
    assert_eq!(result.status, 0);
}

#[test]
fn duplicate_names_within_or_across_source_and_frozen_bindings_are_refused() {
    let directory = std::path::Path::new("/tmp/gnu-source-binding-collisions");
    let duplicate_source = [
        LiteralSourceQueryVariable {
            name: "REPEATED",
            value: Some("first"),
            export: Some(true),
        },
        LiteralSourceQueryVariable {
            name: "REPEATED",
            value: Some("second"),
            export: Some(true),
        },
    ];
    assert!(
        GnuShellQuery::render_with_source_bindings(directory, ":", &[], &duplicate_source).is_err()
    );

    let frozen = [LiteralQueryVariable {
        name: "COLLISION",
        value: Some("frozen"),
        export: Some(true),
    }];
    let source = [LiteralSourceQueryVariable {
        name: "COLLISION",
        value: Some("source"),
        export: Some(true),
    }];
    assert!(GnuShellQuery::render_with_source_bindings(directory, ":", &frozen, &source).is_err());
}

#[test]
fn source_and_frozen_bindings_share_validation_and_size_limits() {
    let directory = std::path::Path::new("/tmp/gnu-source-binding-limits");
    let source = [LiteralSourceQueryVariable {
        name: "SOURCE_VALUE",
        value: Some("source"),
        export: Some(true),
    }];
    let frozen = [LiteralQueryVariable {
        name: "FROZEN_VALUE",
        value: Some("frozen"),
        export: Some(true),
    }];
    assert!(GnuShellQuery::render_with_source_bindings(directory, ":", &frozen, &source).is_ok());

    let invalid_name = [LiteralSourceQueryVariable {
        name: "INVALID NAME",
        value: Some("value"),
        export: Some(true),
    }];
    assert!(
        GnuShellQuery::render_with_source_bindings(directory, ":", &[], &invalid_name).is_err()
    );

    let nul_value = [LiteralSourceQueryVariable {
        name: "NUL_VALUE",
        value: Some("before\0after"),
        export: Some(true),
    }];
    assert!(GnuShellQuery::render_with_source_bindings(directory, ":", &[], &nul_value).is_err());

    let oversized_value = "x".repeat(1024 * 1024);
    let oversized_source = [LiteralSourceQueryVariable {
        name: "LARGE_VALUE",
        value: Some(&oversized_value),
        export: Some(true),
    }];
    assert!(
        GnuShellQuery::render_with_source_bindings(directory, ":", &[], &oversized_source).is_err()
    );

    let names = (0..4096)
        .map(|index| format!("SOURCE_{index}"))
        .collect::<Vec<_>>();
    let many_source = names
        .iter()
        .map(|name| LiteralSourceQueryVariable {
            name: name.as_str(),
            value: None,
            export: Some(false),
        })
        .collect::<Vec<_>>();
    assert!(
        GnuShellQuery::render_with_source_bindings(directory, ":", &frozen, &many_source).is_err()
    );
}
