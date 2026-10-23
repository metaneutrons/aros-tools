use std::collections::BTreeSet;

use super::{LiteralMakeStartupArguments, MAX_ARGUMENTS, MAX_BYTES};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn base() -> Vec<String> {
    strings(&["-f", "synthetic-input.mk", "synthetic-goal"])
}

fn assert_rejected(arguments: &[String]) {
    assert!(LiteralMakeStartupArguments::admit(arguments).is_err());
}

#[test]
fn preserves_literal_argument_order_duplicates_and_input_identities() {
    let arguments = strings(&[
        "-r",
        "--file=first-input.mk",
        "SYNTHETIC_KEY=first-value",
        "-R",
        "-f",
        "second-input.mk",
        "synthetic-goal-one",
        "-o",
        "first-old.mk",
        "SYNTHETIC_KEY=second-value",
        "--old-file=second-old.mk",
        "-n",
        "-s",
        "--no-print-directory",
        "another-goal",
    ]);

    let admitted = LiteralMakeStartupArguments::admit(&arguments).unwrap();

    assert_eq!(admitted.arguments(), arguments);
    assert_eq!(
        admitted.command_line_names(),
        &BTreeSet::from(["SYNTHETIC_KEY".to_owned()])
    );
    assert_eq!(
        admitted.input_files(),
        &["first-input.mk".to_owned(), "second-input.mk".to_owned()]
    );
    assert_eq!(
        admitted.old_files(),
        &["first-old.mk".to_owned(), "second-old.mk".to_owned()]
    );
    assert_eq!(
        admitted.goals(),
        &["synthetic-goal-one".to_owned(), "another-goal".to_owned()]
    );
}

#[test]
fn admits_arbitrary_synthetic_assignment_names_without_building_a_value_map() {
    let arguments = strings(&[
        "-f",
        "input.mk",
        "goal",
        "1synthetic=first",
        "custom-key=spaces ; %|,(paren) and = bang!",
        "1synthetic=third",
    ]);
    let admitted = LiteralMakeStartupArguments::admit(&arguments).unwrap();

    assert_eq!(admitted.arguments(), arguments);
    assert_eq!(
        admitted.command_line_names(),
        &BTreeSet::from(["1synthetic".to_owned(), "custom-key".to_owned()])
    );
}

#[test]
fn debug_and_errors_do_not_retain_caller_values() {
    let secret = "synthetic-private-startup-value-5d1a";
    let arguments = strings(&["-f", "input.mk", "goal", &format!("SYNTHETIC_KEY={secret}")]);
    let admitted = LiteralMakeStartupArguments::admit(&arguments).unwrap();
    let debug = format!("{admitted:?}");
    assert!(!debug.contains(secret));

    let rejected = strings(&["-f", "input.mk", "goal", &format!("--eval={secret}")]);
    let error = LiteralMakeStartupArguments::admit(&rejected).unwrap_err();
    assert!(!format!("{error}").contains(secret));
    assert!(!format!("{error:?}").contains(secret));
}

#[test]
fn rejects_unmodeled_startup_effects_options_bundles_stdin_and_separator() {
    let unsupported: &[&[&str]] = &[
        &["--eval=synthetic-assignment=value"],
        &["--eval", "synthetic-assignment=value"],
        &["--load-average=2"],
        &["-L", "2"],
        &["--load=synthetic-module.so"],
        &["-l", "synthetic-module.so"],
        &["-C", "synthetic-directory"],
        &["-I", "synthetic-include-directory"],
        &["-j"],
        &["-j8"],
        &["--jobs=8"],
        &["--output-sync=target"],
        &["-rn"],
        &["-sR"],
        &["-W", "future-input.mk"],
        &["--warn-undefined-variables"],
        &["--"],
        &["-f", "-"],
        &["--file=-"],
        &["-o", "-"],
        &["--old-file=-"],
        &["-f"],
        &["-o"],
        &["--file="],
        &["--old-file="],
        &["-f", "~"],
        &["-f", "~/synthetic-input.mk"],
        &["--file=~/synthetic-input.mk"],
        &["-o", "~/synthetic-old.mk"],
    ];

    for extra in unsupported {
        let mut arguments = base();
        arguments.extend(extra.iter().map(|value| (*value).to_owned()));
        assert_rejected(&arguments);
    }
}

#[test]
fn rejects_assignment_expansion_operators_unsafe_names_and_control_bytes() {
    let unsupported = [
        "SYNTHETIC_KEY=$(shell touch should-not-run)",
        "SYNTHETIC_KEY=$$HOME",
        "SYNTHETIC_KEY!=synthetic-command",
        "SYNTHETIC_KEY:=literal",
        "SYNTHETIC_KEY+=literal",
        "SYNTHETIC_KEY?=literal",
        "SYNTHETIC_KEY::=literal",
        "SYNTHETIC_KEY:::=literal",
        "=empty-name",
        ".synthetic=value",
        "synthetic.name=value",
        "synthetic name=value",
        "synthetic/name=value",
        "SYNTHETIC_KEY=before\nafter",
        "SYNTHETIC_KEY=before\tafter",
        "SYNTHETIC_KEY=before\0after",
        "SYNTHETIC_KEY=before\u{7f}after",
    ];

    for assignment in unsupported {
        let mut arguments = base();
        arguments.push(assignment.to_owned());
        assert_rejected(&arguments);
    }
}

#[test]
fn rejects_each_nonempty_startup_control_even_if_an_empty_duplicate_follows() {
    let controls = ["MAKEFILES", "MAKEFLAGS", "GNUMAKEFLAGS", "MFLAGS"];
    let sentinel = "synthetic-control-private-value-1e2b";

    for control in controls {
        let mut arguments = base();
        arguments.push(format!("{control}={sentinel}"));
        arguments.push(format!("{control}="));
        let error = LiteralMakeStartupArguments::admit(&arguments).unwrap_err();
        assert!(!format!("{error}").contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
    }
}

#[test]
fn allows_empty_startup_controls_as_literal_assignments() {
    let arguments = strings(&[
        "-f",
        "input.mk",
        "goal",
        "MAKEFILES=",
        "MAKEFLAGS=",
        "GNUMAKEFLAGS=",
        "MFLAGS=",
    ]);
    let admitted = LiteralMakeStartupArguments::admit(&arguments).unwrap();

    assert_eq!(
        admitted.command_line_names(),
        &BTreeSet::from([
            "MAKEFILES".to_owned(),
            "MAKEFLAGS".to_owned(),
            "GNUMAKEFLAGS".to_owned(),
            "MFLAGS".to_owned(),
        ])
    );
    assert_eq!(admitted.arguments(), arguments);
}

#[test]
fn enforces_argument_count_and_total_byte_bounds() {
    let mut at_argument_limit = base();
    let filler_count = MAX_ARGUMENTS - at_argument_limit.len();
    at_argument_limit.extend(std::iter::repeat_n("-n".to_owned(), filler_count));
    assert_eq!(at_argument_limit.len(), MAX_ARGUMENTS);
    assert!(LiteralMakeStartupArguments::admit(&at_argument_limit).is_ok());
    at_argument_limit.push("-n".to_owned());
    assert_rejected(&at_argument_limit);

    let fixed = strings(&["-f", "in", "goal"]);
    let fixed_bytes: usize = fixed.iter().map(String::len).sum();
    let prefix = "SYNTH=";
    let value = "x".repeat(MAX_BYTES - fixed_bytes - prefix.len());
    let mut at_byte_limit = fixed.clone();
    at_byte_limit.push(format!("{prefix}{value}"));
    assert_eq!(
        at_byte_limit.iter().map(String::len).sum::<usize>(),
        MAX_BYTES
    );
    assert!(LiteralMakeStartupArguments::admit(&at_byte_limit).is_ok());

    let mut over_byte_limit = fixed;
    over_byte_limit.push(format!("{prefix}{value}x"));
    assert_rejected(&over_byte_limit);
}

#[test]
fn requires_an_explicit_input_and_goal() {
    assert_rejected(&strings(&["synthetic-goal"]));
    assert_rejected(&strings(&["-f", "input.mk"]));
}
