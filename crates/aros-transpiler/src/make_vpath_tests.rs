use super::{first_present, project, SearchPathEntry, SearchPaths};
use crate::make_definitions::MakeDefinitions;
use crate::make_expr::MakeRuleExpansion;
use crate::make_rule_events::{collect, SearchPathDeclaration};
use crate::make_rule_program::{expand, RuleProgram};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

fn positioned(source: &str, states: &[Option<bool>]) -> RuleProgram {
    expand(source, states, &|_, _, _: &MakeDefinitions| {
        Err("unexpected rule-generation expression".into())
    })
    .unwrap()
}

fn project_source(
    source: &str,
    states: &[Option<bool>],
    evaluate: &impl Fn(&str, usize) -> Result<String, String>,
) -> Result<SearchPaths, String> {
    let events = collect(&positioned(source, states))?;
    project(&events.search_paths, evaluate)
}

fn declaration(output_line: usize, source_line: usize, arguments: &str) -> SearchPathDeclaration {
    SearchPathDeclaration {
        output_line,
        source_line,
        arguments: arguments.to_owned(),
    }
}

#[test]
fn vpath_events_keep_source_positions_through_rule_program_expansion() {
    let source = "$(emit_rules)\nvpath %.c $(DIRS)\nvpath %.h $(DIRS)\n";
    let states = [Some(true); 3];
    let program = expand(source, &states, &|expression, source_line, definitions| {
        assert_eq!(expression, "$(emit_rules)");
        assert_eq!(source_line, 0);
        assert_eq!(definitions.line, source_line);
        Ok(MakeRuleExpansion {
            value: String::new(),
            rules: vec!["generated:\n\t@compile".to_owned()],
        })
    })
    .unwrap();
    let events = collect(&program).unwrap();

    assert_eq!(events.rules.len(), 1);
    assert_eq!(events.rules[0].output_line, 0);
    assert_eq!(events.rules[0].source_line, 0);
    assert_eq!(events.rules[0].recipes[0].output_line, 1);
    assert_eq!(events.rules[0].recipes[0].source_line, 0);
    assert_eq!(events.search_paths.len(), 2);
    assert_eq!(events.search_paths[0].output_line, 2);
    assert_eq!(events.search_paths[0].source_line, 1);
    assert_eq!(events.search_paths[0].arguments, "%.c $(DIRS)");
    assert_eq!(events.search_paths[1].output_line, 3);
    assert_eq!(events.search_paths[1].source_line, 2);

    let evaluated_at = RefCell::new(Vec::new());
    let projected = project(&events.search_paths, &|arguments, source_line| {
        evaluated_at
            .borrow_mut()
            .push((arguments.to_owned(), source_line));
        match source_line {
            1 => Ok("%.c first".to_owned()),
            2 => Ok("%.h second".to_owned()),
            _ => Err(format!("unexpected declaration line {source_line}")),
        }
    })
    .unwrap();

    assert_eq!(
        evaluated_at.into_inner(),
        vec![("%.c $(DIRS)".to_owned(), 1), ("%.h $(DIRS)".to_owned(), 2),]
    );
    assert_eq!(projected.entries()[0].output_line, 2);
    assert_eq!(projected.entries()[0].source_line, 1);
    assert_eq!(projected.entries()[0].pattern, "%.c");
    assert_eq!(projected.entries()[0].directories, ["first"]);
    assert_eq!(projected.entries()[1].output_line, 3);
    assert_eq!(projected.entries()[1].source_line, 2);
    assert_eq!(projected.entries()[1].pattern, "%.h");
    assert_eq!(projected.entries()[1].directories, ["second"]);
}

#[test]
fn ordinary_vpath_assignment_is_not_a_search_path_declaration() {
    let source = "vpath = ordinaryglobal\n";
    let events = collect(&positioned(source, &[Some(true)])).unwrap();

    assert!(events.search_paths.is_empty());
    assert_eq!(events.other_lines, [0]);
}

#[test]
fn inactive_and_define_body_vpath_lines_are_suppressed() {
    let source = concat!(
        "define stored_text\n",
        "vpath %.c in-definition\n",
        "endef\n",
        "vpath %.c inactive\n",
        "vpath %.c active\n",
    );
    let states = [Some(true), Some(true), Some(true), Some(false), Some(true)];
    let events = collect(&positioned(source, &states)).unwrap();

    assert_eq!(events.search_paths.len(), 1);
    assert_eq!(events.search_paths[0].output_line, 4);
    assert_eq!(events.search_paths[0].source_line, 4);
    assert_eq!(events.search_paths[0].arguments, "%.c active");
}

#[test]
fn unresolved_vpath_selection_is_refused_and_vpath_breaks_recipe_attachment() {
    let unknown = collect(&positioned("vpath %.c source\n", &[None])).unwrap_err();
    assert_eq!(unknown, "Make vpath event has unresolved selection");

    let source = "target:\n\t@attached\nvpath %.c source\n\t@orphan\n";
    let error = collect(&positioned(source, &[Some(true); 4])).unwrap_err();
    assert_eq!(error, "Make recipe has no active rule");
}

#[test]
fn repeated_patterns_append_in_declaration_order_and_directories_split_on_colon_or_space() {
    let source = concat!(
        "vpath %.c : first  second:third::\n",
        "vpath %.h headers\n",
        "vpath %.c fourth:fifth\n",
    );
    let paths = project_source(source, &[Some(true); 3], &|arguments, _| {
        Ok(arguments.to_owned())
    })
    .unwrap();

    assert_eq!(paths.entries().len(), 3);
    assert_eq!(paths.entries()[0].pattern, "%.c");
    assert_eq!(paths.entries()[0].directories, ["first", "second", "third"]);
    assert_eq!(paths.entries()[1].pattern, "%.h");
    assert_eq!(paths.entries()[1].directories, ["headers"]);
    assert_eq!(paths.entries()[2].pattern, "%.c");
    assert_eq!(paths.entries()[2].directories, ["fourth", "fifth"]);
    assert!(paths.entries()[0].output_line < paths.entries()[1].output_line);
    assert!(paths.entries()[1].output_line < paths.entries()[2].output_line);
}

#[test]
fn absent_directory_text_clears_exact_pattern_and_bare_vpath_clears_all() {
    let clear_one = "vpath %.c source\nvpath %.h headers\nvpath %.c\n";
    let paths = project_source(clear_one, &[Some(true); 3], &|arguments, _| {
        Ok(arguments.to_owned())
    })
    .unwrap();
    assert_eq!(paths.entries().len(), 1);
    assert_eq!(paths.entries()[0].pattern, "%.h");
    assert_eq!(paths.entries()[0].directories, ["headers"]);

    let clear_all = "vpath %.c source\nvpath %.h headers\nvpath\n";
    let paths = project_source(clear_all, &[Some(true); 3], &|arguments, _| {
        Ok(arguments.to_owned())
    })
    .unwrap();
    assert!(paths.entries().is_empty());
}

#[test]
fn separator_only_directory_text_does_not_clear_an_existing_pattern() {
    let source = "vpath lib.rs olddir\nvpath lib.rs :\nvpath lib.rs ::\n";
    let paths = project_source(source, &[Some(true); 3], &|arguments, _| {
        Ok(arguments.to_owned())
    })
    .unwrap();

    assert_eq!(paths.entries().len(), 1);
    assert_eq!(paths.entries()[0].pattern, "lib.rs");
    assert_eq!(paths.entries()[0].directories, ["olddir"]);
    assert_eq!(paths.entries()[0].output_line, 0);
}

#[test]
fn literal_and_single_percent_patterns_keep_cwd_selective_and_global_order() {
    let source = "vpath exact.c literal\nvpath src/%.h headers\n";
    let paths = project_source(source, &[Some(true); 2], &|arguments, _| {
        Ok(arguments.to_owned())
    })
    .unwrap();
    let cwd = Path::new("/work/project");
    let global_vpath = "fallback:generated";

    assert_eq!(
        paths.candidates("exact.c", cwd, global_vpath).unwrap(),
        vec![
            cwd.join("exact.c"),
            cwd.join("literal").join("exact.c"),
            cwd.join("fallback").join("exact.c"),
            cwd.join("generated").join("exact.c"),
        ]
    );
    assert_eq!(
        paths.candidates("src/unit.h", cwd, global_vpath).unwrap(),
        vec![
            cwd.join("src/unit.h"),
            cwd.join("headers").join("src/unit.h"),
            cwd.join("fallback").join("src/unit.h"),
            cwd.join("generated").join("src/unit.h"),
        ]
    );
    assert_eq!(
        paths.candidates("src/unit.c", cwd, global_vpath).unwrap(),
        vec![
            cwd.join("src/unit.c"),
            cwd.join("fallback").join("src/unit.c"),
            cwd.join("generated").join("src/unit.c"),
        ]
    );

    let source_path = project_source(
        "vpath %.rs crates/aros-transpiler\n",
        &[Some(true)],
        &|args, _| Ok(args.to_owned()),
    )
    .unwrap();
    assert_eq!(
        source_path.candidates("src/main.rs", cwd, "").unwrap(),
        vec![
            cwd.join("src/main.rs"),
            cwd.join("crates/aros-transpiler").join("src/main.rs"),
        ]
    );
}

#[test]
fn unsupported_expressions_patterns_and_quoted_words_are_rejected() {
    for (source, expected) in [
        (
            "vpath %.c $(DIRS)\n",
            "Make vpath has a nonliteral or unsupported path word",
        ),
        (
            "vpath %.c $@\n",
            "Make vpath has a nonliteral or unsupported path word",
        ),
        (
            "vpath foo%bar%baz src\n",
            "Make vpath pattern is outside the single-percent model",
        ),
        (
            "vpath %.c \"src dir\"\n",
            "Make vpath has a nonliteral or unsupported path word",
        ),
    ] {
        let error = project_source(source, &[Some(true)], &|arguments, _| {
            Ok(arguments.to_owned())
        })
        .unwrap_err();
        assert_eq!(error, expected, "{source}");
    }

    let source = "vpath %.h headers\nvpath %.c $(UNKNOWN)\n";
    let error = project_source(source, &[Some(true); 2], &|arguments, line| {
        if line == 0 {
            Ok(arguments.to_owned())
        } else {
            Err("unknown evaluator variable UNKNOWN".into())
        }
    })
    .unwrap_err();
    assert_eq!(error, "unknown evaluator variable UNKNOWN");
}

#[test]
fn candidates_refuse_absolute_files_and_relative_working_directories() {
    let paths = SearchPaths::default();
    let error = paths
        .candidates("/absolute.c", Path::new("/work"), "")
        .unwrap_err();
    assert_eq!(
        error,
        "Make vpath requires a relative file and absolute working directory"
    );
    let error = paths
        .candidates("relative.c", Path::new("work"), "")
        .unwrap_err();
    assert_eq!(
        error,
        "Make vpath requires a relative file and absolute working directory"
    );
}

#[test]
fn first_present_stops_at_the_first_hit_propagates_errors_and_returns_none_on_miss() {
    let candidates = [
        PathBuf::from("first"),
        PathBuf::from("second"),
        PathBuf::from("third"),
    ];
    let visited = RefCell::new(Vec::new());
    let selected = first_present(&candidates, &|path| {
        visited.borrow_mut().push(path.to_owned());
        Ok(path == Path::new("second"))
    })
    .unwrap();
    assert_eq!(selected, Some(PathBuf::from("second")));
    assert_eq!(
        visited.into_inner(),
        [PathBuf::from("first"), PathBuf::from("second")]
    );

    let visited = RefCell::new(Vec::new());
    let error = first_present(&candidates, &|path| {
        visited.borrow_mut().push(path.to_owned());
        if path == Path::new("first") {
            Ok(false)
        } else {
            Err("candidate cannot be proved safe".into())
        }
    })
    .unwrap_err();
    assert_eq!(error, "candidate cannot be proved safe");
    assert_eq!(visited.borrow().len(), 2);

    let visited = RefCell::new(Vec::new());
    let selected = first_present(&candidates, &|path| {
        visited.borrow_mut().push(path.to_owned());
        Ok(false)
    })
    .unwrap();
    assert_eq!(selected, None);
    assert_eq!(visited.borrow().len(), candidates.len());
}

#[cfg(unix)]
#[test]
fn unsafe_first_filesystem_hit_is_an_error_even_when_a_later_regular_file_exists() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let actual = root.join("actual.c");
    let link = root.join("link.c");
    std::fs::write(&actual, b"source").unwrap();
    aros_common::open_regular_file_nofollow(&actual).unwrap();
    assert_eq!(
        first_present(std::slice::from_ref(&actual), &|path| {
            aros_common::open_regular_file_nofollow(path)
                .map(|_| true)
                .map_err(|error| error.to_string())
        })
        .unwrap(),
        Some(actual.clone())
    );
    symlink(&actual, &link).unwrap();
    let candidates = [link, actual];
    let observed = Cell::new(0);

    let error = first_present(&candidates, &|path| {
        observed.set(observed.get() + 1);
        aros_common::open_regular_file_nofollow(path)
            .map(|_| true)
            .map_err(|error| format!("unsafe first candidate: {error}"))
    })
    .unwrap_err();

    assert!(error.starts_with("unsafe first candidate:"));
    assert_eq!(observed.get(), 1);
}

#[test]
fn project_rejects_nonunique_output_order_and_resource_limits_with_exact_reasons() {
    let unordered = [declaration(2, 0, "%.c src"), declaration(2, 1, "%.h inc")];
    assert_eq!(
        project(&unordered, &|arguments, _| Ok(arguments.to_owned())).unwrap_err(),
        "Make vpath declarations are not in unique output order"
    );

    let too_many_declarations = (0..=super::MAX_ENTRIES)
        .map(|line| declaration(line, line, ""))
        .collect::<Vec<_>>();
    let calls = Cell::new(0);
    assert_eq!(
        project(&too_many_declarations, &|_, _| {
            calls.set(calls.get() + 1);
            Ok(String::new())
        })
        .unwrap_err(),
        "Make vpath exceeds declaration limit"
    );
    assert_eq!(calls.get(), 0);

    let maximum_pattern = "a".repeat(super::MAX_VALUE_BYTES);
    let declarations = (0..=super::MAX_BYTES / super::MAX_VALUE_BYTES)
        .map(|line| declaration(line, line, "ignored"))
        .collect::<Vec<_>>();
    assert_eq!(
        project(&declarations, &|_, _| Ok(maximum_pattern.clone())).unwrap_err(),
        "Make vpath exceeds evaluated byte limit"
    );

    let oversized_value = "a".repeat(super::MAX_VALUE_BYTES + 1);
    assert_eq!(
        project(&[declaration(0, 0, "ignored")], &|_, _| {
            Ok(oversized_value.clone())
        })
        .unwrap_err(),
        "Make vpath declaration exceeds value byte limit"
    );

    let too_many_directories = format!("%.c {}", "d ".repeat(super::MAX_ENTRIES + 1));
    assert_eq!(
        project(&[declaration(0, 0, "ignored")], &|_, _| {
            Ok(too_many_directories.clone())
        })
        .unwrap_err(),
        "Make vpath directory list exceeds count limit"
    );

    let maximum_directories = format!("%.c {}", "d ".repeat(super::MAX_ENTRIES));
    let declarations = [declaration(0, 0, "first"), declaration(1, 1, "second")];
    assert_eq!(
        project(&declarations, &|arguments, _| {
            Ok(if arguments == "first" {
                maximum_directories.clone()
            } else {
                "%.h inc".to_owned()
            })
        })
        .unwrap_err(),
        "Make vpath exceeds directory count limit"
    );
}

#[test]
fn candidate_observation_limits_report_exact_reasons_without_invoking_observers() {
    let too_many = vec![PathBuf::from("candidate"); super::MAX_ENTRIES + 1];
    let calls = Cell::new(0);
    assert_eq!(
        first_present(&too_many, &|_| {
            calls.set(calls.get() + 1);
            Ok(false)
        })
        .unwrap_err(),
        "Make directory search exceeds candidate limit"
    );
    assert_eq!(calls.get(), 0);

    let over_byte_limit = PathBuf::from(format!("/{}", "x".repeat(super::MAX_BYTES)));
    assert_eq!(
        first_present(&[over_byte_limit], &|_| {
            calls.set(calls.get() + 1);
            Ok(false)
        })
        .unwrap_err(),
        "Make directory search exceeds candidate limit"
    );
    assert_eq!(calls.get(), 0);

    let too_many_directories = SearchPaths {
        entries: vec![SearchPathEntry {
            output_line: 0,
            source_line: 0,
            pattern: "%".to_owned(),
            directories: vec!["d".to_owned(); super::MAX_ENTRIES],
        }],
    };
    assert_eq!(
        too_many_directories
            .candidates("file.c", Path::new("/work"), "")
            .unwrap_err(),
        "Make vpath exceeds candidate count limit"
    );

    let overlong_global = "d".repeat(super::MAX_VALUE_BYTES + 1);
    assert_eq!(
        SearchPaths::default()
            .candidates("file.c", Path::new("/work"), &overlong_global)
            .unwrap_err(),
        "Make vpath directory list exceeds value byte limit"
    );
}

#[cfg(unix)]
#[test]
fn candidate_path_byte_limit_is_enforced() {
    let root = PathBuf::from(format!("/{}", "w".repeat(super::MAX_BYTES / 2 - 3)));
    let paths = SearchPaths {
        entries: vec![SearchPathEntry {
            output_line: 0,
            source_line: 0,
            pattern: "%".to_owned(),
            directories: vec!["dd".to_owned()],
        }],
    };

    assert_eq!(
        paths.candidates("x", &root, "").unwrap_err(),
        "Make vpath exceeds candidate byte limit"
    );
}
