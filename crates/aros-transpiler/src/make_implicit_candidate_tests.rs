use super::{collect, instantiate_implicit_candidate, RuleInstance, SourceRule};
use crate::make_definitions::MakeDefinitions;
use crate::make_rule_program::{expand, RuleProgram};
use std::fs;
use std::process::Command;

fn positioned(source: &str) -> RuleProgram {
    let states = vec![Some(true); source.lines().count()];
    expand(source, &states, &|_, _, _: &MakeDefinitions| {
        Err("unexpected expression callback".into())
    })
    .unwrap()
}

fn rule(source: &str) -> SourceRule {
    collect(&positioned(source))
        .unwrap()
        .rules
        .into_iter()
        .next()
        .expect("fixture has one rule")
}

fn candidate(rule: &SourceRule, target: &str) -> Result<Vec<RuleInstance>, String> {
    instantiate_implicit_candidate(rule, target)
}

#[test]
fn same_directory_candidates_substitute_each_match_and_retain_source_data() {
    let source = concat!(
        "# fixture\n",
        "foo-%.o bar-%.o: src/%.c src/%.c | ready\n",
        "\tprintf '%s|%s|%s\\n' '$@' '$*' '$<'\n",
    );
    let source_rule = rule(source);

    for (target, stem) in [("foo-alpha.o", "alpha"), ("bar-beta.o", "beta")] {
        let instances = candidate(&source_rule, target).unwrap();
        assert_eq!(instances.len(), 1);
        let instance = &instances[0];
        assert_eq!(instance.target, target);
        assert_eq!(instance.stem.as_deref(), Some(stem));
        assert_eq!(
            instance.prerequisites,
            vec![format!("src/{stem}.c"), format!("src/{stem}.c")]
        );
        assert_eq!(instance.order_only, ["ready"]);
        assert_eq!(
            instance.prerequisite_order,
            [
                format!("src/{stem}.c"),
                format!("src/{stem}.c"),
                "ready".to_owned(),
            ]
        );
        assert_eq!(instance.source_line, 1);
        assert_eq!(instance.output_line, 1);
        assert_eq!(instance.recipes.len(), 1);
        assert_eq!(instance.recipes[0].source_line, 2);
        assert_eq!(instance.recipes[0].output_line, 2);
        assert_eq!(
            instance.recipes[0].text,
            "printf '%s|%s|%s\\n' '$@' '$*' '$<'"
        );

        let automatic = instance.automatic_variables();
        assert_eq!(automatic["@"], target);
        assert_eq!(automatic["*"], stem);
        assert_eq!(automatic["<"], format!("src/{stem}.c"));
        assert_eq!(automatic["+"], format!("src/{stem}.c src/{stem}.c"));
    }
    assert!(candidate(&source_rule, "other-alpha.o").unwrap().is_empty());

    let overlapping = rule("foo-%.o foo-a%.o: src/%.c\n");
    let instances = candidate(&overlapping, "foo-ab.o").unwrap();
    assert_eq!(
        instances
            .iter()
            .map(|instance| instance.stem.as_deref())
            .collect::<Vec<_>>(),
        [Some("ab"), Some("b")]
    );
}

#[test]
fn refuses_empty_stems_malformed_mixed_static_double_colon_and_directory_cases() {
    let patterns = rule("foo-%.o bar-%.o: src/%.c\n");
    assert!(candidate(&patterns, "foo-.o").unwrap().is_empty());

    for (source, target, reason) in [
        (
            "foo-%%.o: src/%.c\n",
            "foo-a.o",
            "same-directory percent patterns",
        ),
        (
            "plain.o: src/%.c\n",
            "plain.o",
            "same-directory percent patterns",
        ),
        (
            "foo-%.o plain.o: src/%.c\n",
            "foo-a.o",
            "same-directory percent patterns",
        ),
        (
            "foo-a.o: foo-%.o: src/%.c\n",
            "foo-a.o",
            "static or double-colon syntax",
        ),
        (
            "foo-%.o:: src/%.c\n",
            "foo-a.o",
            "static or double-colon syntax",
        ),
        (
            "obj/foo-%.o: src/%.c\n",
            "foo-a.o",
            "same-directory percent patterns",
        ),
        (
            "foo-%.o: src/%.c\n",
            "obj/foo-a.o",
            "one same-directory literal target",
        ),
        (
            "foo-%.o: src/%.c\n",
            "foo-a.o bar-b.o",
            "one same-directory literal target",
        ),
        (
            "foo-%.o: src/%.c\n",
            "%foo.o",
            "one same-directory literal target",
        ),
        (
            "foo-%.o: src/%.c\n",
            "\tfoo-a.o\n",
            "one same-directory literal target",
        ),
    ] {
        let error = candidate(&rule(source), target).unwrap_err();
        assert!(
            error.contains(reason),
            "unexpected refusal for {source:?} and {target:?}: {error}"
        );
    }
}

#[test]
fn recipe_less_cancellation_candidates_remain_visible_without_search_claims() {
    let rule = rule("foo-%.o: src/%.c\n");
    let candidates = candidate(&rule, "foo-a.o").unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].recipes.is_empty());
    assert_eq!(candidates[0].prerequisites, ["src/a.c"]);
}

#[test]
fn ordinary_gnu_pattern_recipe_expansion_matches_candidate_automatic_values() {
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/a.c"), "").unwrap();
    fs::write(root.join("ready"), "").unwrap();

    let source = concat!(
        "foo-%.o bar-%.o: src/%.c src/%.c | ready\n",
        "\tprintf '%s|%s|%s\\n' '$@' '$*' '$<'\n",
    );
    fs::write(root.join("root.mk"), source).unwrap();
    let instance = candidate(&rule(source), "foo-a.o").unwrap().remove(0);
    let automatic = instance.automatic_variables();
    let expected = format!(
        "printf '%s|%s|%s\\n' '{}' '{}' '{}'",
        automatic["@"], automatic["*"], automatic["<"]
    );

    let output = Command::new(make)
        .current_dir(root)
        .env_clear()
        .env("PATH", std::env::var("PATH").expect("PATH is available"))
        .env("LC_ALL", "C")
        .args(["-r", "-R", "-n", "--no-print-directory", "-f", "root.mk"])
        .arg("foo-a.o")
        .output()
        .expect("GNU Make is available for the synthetic fixture");

    assert!(
        output.status.success(),
        "GNU Make fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed = String::from_utf8(output.stdout).unwrap();
    assert_eq!(printed.trim_end(), expected);
}
