//! Independent host-shell oracle; only private recorder programs are launched.

use super::{parse_literal_recipe, LiteralRecipeStatement, RecipeRedirectionKind};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

fn quote(value: &std::path::Path) -> String {
    format!("'{}'", value.to_str().unwrap().replace('\'', "'\\''"))
}

#[test]
fn private_shell_oracle_confirms_equal_precedence_without_dropping_operands() {
    use super::RecipeAndOrOperator::{And, Or};
    use aros_common::{run_output_with_control, CancellationToken};
    use std::time::Duration;

    let temporary = tempfile::tempdir().unwrap();
    for (recipe, operators, expected) in [
        ("true || printf skipped && printf tail", [Or, And], "tail"),
        (
            "false && printf skipped || printf fallback",
            [And, Or],
            "fallback",
        ),
    ] {
        let parsed = parse_literal_recipe(recipe).unwrap();
        let [LiteralRecipeStatement::AndOr { first, rest }] = parsed.as_slice() else {
            panic!()
        };
        assert_eq!(
            rest.iter()
                .map(|(operator, _)| *operator)
                .collect::<Vec<_>>(),
            operators
        );
        // The AST retains both potentially skipped operands. Only this
        // independent host-shell oracle observes an execution outcome.
        assert!(std::iter::once(first.as_ref())
            .chain(rest.iter().map(|(_, arm)| arm))
            .all(|arm| matches!(arm, LiteralRecipeStatement::Command(_))));
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", recipe])
            .env_clear()
            .current_dir(temporary.path());
        let output = run_output_with_control(
            &mut command,
            4096,
            Duration::from_secs(5),
            &CancellationToken::default(),
        )
        .unwrap();
        assert!(output.status.success() && !output.timed_out && !output.cancelled);
        assert_eq!(output.stdout.rendered_lossy(), expected);
        assert!(output.stderr.rendered_lossy().is_empty());
    }
}

#[test]
fn private_shell_oracle_preserves_pipeline_argv_and_redirection_override() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let producer = root.join("private producer");
    let consumer = root.join("private consumer");
    for (path, script) in [
        (
            &producer,
            "#!/bin/sh\nprintf '%s\\000' \"$@\" > \"$FIRST_ARGS\"\nprintf 'pipe payload\\n'\n",
        ),
        (
            &consumer,
            "#!/bin/sh\nprintf '%s\\000' \"$@\" > \"$SECOND_ARGS\"\n/bin/cat > \"$INPUT_BYTES\"\n",
        ),
    ] {
        fs::write(path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let before = [fs::read(&producer).unwrap(), fs::read(&consumer).unwrap()];
    for redirected in [false, true] {
        let extra = if redirected {
            format!(" >{}", quote(&root.join("override output")))
        } else {
            String::new()
        };
        let recipe = format!(
            "{} 'ordered arg' ''{} | {} 'literal|pipe' 'quoted space'",
            quote(&producer),
            extra,
            quote(&consumer),
        );
        let parsed = parse_literal_recipe(&recipe).unwrap();
        let [LiteralRecipeStatement::Pipeline(stages)] = parsed.as_slice() else {
            panic!()
        };
        assert_eq!(stages.len(), 2);
        assert_eq!(&stages[0].words()[1..], ["ordered arg", ""]);
        assert_eq!(&stages[1].words()[1..], ["literal|pipe", "quoted space"]);
        if redirected {
            assert_eq!(stages[0].redirections().len(), 1);
            assert_eq!(stages[0].redirections()[0].descriptor(), 1);
            assert_eq!(
                stages[0].redirections()[0].kind(),
                RecipeRedirectionKind::Output
            );
            assert_eq!(
                stages[0].redirections()[0].target(),
                root.join("override output").to_str().unwrap()
            );
        } else {
            assert!(stages[0].redirections().is_empty());
        }
        assert!(stages[1].redirections().is_empty());
        let output = Command::new("/bin/sh")
            .args(["-c", &recipe])
            .current_dir(&root)
            .env_clear()
            .env("FIRST_ARGS", root.join("first.argv"))
            .env("SECOND_ARGS", root.join("second.argv"))
            .env("INPUT_BYTES", root.join("stdin.bytes"))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty() && output.stderr.is_empty());
        for (stage, file) in stages.iter().zip(["first.argv", "second.argv"]) {
            let bytes = fs::read(root.join(file)).unwrap();
            assert!(bytes.ends_with(&[0]));
            let argv: Vec<_> = bytes[..bytes.len() - 1]
                .split(|byte| *byte == 0)
                .map(|word| std::str::from_utf8(word).unwrap())
                .collect();
            assert_eq!(argv, stage.words()[1..]);
        }
        assert_eq!(
            fs::read(root.join("stdin.bytes")).unwrap(),
            if redirected {
                b"".as_slice()
            } else {
                b"pipe payload\n".as_slice()
            }
        );
        if redirected {
            assert_eq!(
                fs::read(root.join("override output")).unwrap(),
                b"pipe payload\n"
            );
        }
        assert_eq!(
            before,
            [fs::read(&producer).unwrap(), fs::read(&consumer).unwrap()]
        );
    }
}
