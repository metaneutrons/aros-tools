//! Source references may use bounded punctuation absent from Make's complete
//! startup namespace, while legacy projections keep their narrower proof.

use super::*;
use aros_common::make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation};
use std::{
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
};

fn namespace(extra: &[(&str, MakeVariableOrigin)]) -> ValidatedMakeNamespace {
    let mut entries = vec![
        MakeNamespaceEntry {
            name: ".VARIABLES".into(),
            origin: MakeVariableOrigin::Default,
        },
        MakeNamespaceEntry {
            name: "MAKE_VERSION".into(),
            origin: MakeVariableOrigin::Default,
        },
    ];
    entries.extend(extra.iter().map(|(name, origin)| MakeNamespaceEntry {
        name: (*name).into(),
        origin: *origin,
    }));
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries,
    })
    .unwrap()
}

fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/selected/source"),
        relative_dir: Path::new(""),
        top_directory: Path::new("/selected/build"),
        working_directory: Path::new("/selected/build"),
    }
}

/// Run only fixed synthetic Make source through stdin. No values are imported
/// from the caller's environment or passed as command-line variable bindings.
fn native(source: &str, expression: &str) -> String {
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let mut child = Command::new(make)
        .args(["-rR", "--no-print-directory", "-s", "-f", "-", "probe"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input = format!("{source}\n$(info AROS_SOURCE_REFERENCE_RESULT={expression})\nprobe:\n");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "synthetic GNU Make query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let rows = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("AROS_SOURCE_REFERENCE_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1, "expected one synthetic Make observation");
    rows[0].to_owned()
}

#[test]
fn source_reference_absence_admits_bounded_transport_safe_punctuation() {
    let names = namespace(&[]);
    for name in ["/", ".", "!", "=", ":", "ORDINARY_NAME"] {
        assert!(
            names.proves_absent_source_reference(name),
            "expected {name:?} to be a supported absent source reference"
        );
    }
    assert!(!names.proves_absent("/"));
    assert!(names.proves_absent("ORDINARY_NAME"));

    let present = namespace(&[("/", MakeVariableOrigin::Environment)]);
    assert!(present.contains("/"));
    assert!(!present.proves_absent_source_reference("/"));
}

#[test]
fn source_reference_absence_rejects_automatic_engine_and_invalid_names() {
    let names = namespace(&[]);
    for name in [
        "",
        ".MISSING_ENGINE_NAME",
        ".SHELLSTATUS",
        ".VARIABLES",
        "A B",
        "A;B",
        "A|B",
        "é",
        "A\nB",
        "A=B",
        "A%",
        "query/name",
        "+ordinary",
        "-*-eval-flags-*-",
    ] {
        assert!(
            !names.proves_absent_source_reference(name),
            "must not infer absence for {name:?}"
        );
    }
    for automatic in ["@", "%", "<", "?", "^", "+", "*", "|"] {
        for suffix in ["", "D", "F"] {
            let name = format!("{automatic}{suffix}");
            assert!(
                !names.proves_absent_source_reference(&name),
                "must not infer absence for automatic variable {name:?}"
            );
        }
    }
    assert!(!names.proves_absent_source_reference(&"x".repeat(129)));
}

#[test]
fn source_program_matches_gnu_for_one_character_punctuation_references() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let names = namespace(&[]);
    let expression = concat!(
        "$(origin /)|$(flavor /)|$(value /)|",
        "$(origin !)|$(flavor !)|$(value !)|",
        "$(origin =)|$(flavor =)|$(value =)|",
        "$(origin .)|$(flavor .)|$(value .)|",
        "$(origin MISSING_2)|$(flavor MISSING_2)|$(value MISSING_2)|",
        "$(REM)",
    );
    let expected = concat!(
        "undefined|undefined||",
        "undefined|undefined||",
        "undefined|undefined||",
        "undefined|undefined||",
        "undefined|undefined||",
        "synthetic/path",
    );

    for first in ["/", ".", "!", "="] {
        let source = format!("TMP := {first}synthetic/path\n$(eval REM := $$$(TMP))\n");
        assert_eq!(native(&source, expression), expected);

        let mut program = MakeVariableProgram::new_source_program(&paths, &names, &[]).unwrap();
        program
            .read_file("root.mk", &source, &paths, &|_, _| Ok(None))
            .unwrap();
        assert_eq!(program.evaluate(expression, &paths).unwrap(), expected);
    }
}

#[test]
fn unmeasured_present_punctuation_refuses_and_rolls_back_source_eval() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    for name in ["/", "."] {
        let names = namespace(&[(name, MakeVariableOrigin::File)]);
        let mut program = MakeVariableProgram::new_source_program(&paths, &names, &[]).unwrap();
        let source = format!("TMP := {name}synthetic/path\n$(eval REM := $$$(TMP))\n");
        assert_eq!(
            native(&format!("{name} := prefix\n{source}"), "$(REM)"),
            "prefixsynthetic/path"
        );

        assert!(program
            .read_file("root.mk", &source, &paths, &|_, _| Ok(None))
            .is_err());
        assert!(
            program.events().is_empty(),
            "failed source eval must roll back"
        );
        assert_eq!(program.evaluate("$(TMP)|$(REM)", &paths).unwrap(), "|");
        assert!(program.evaluate(&format!("$({name})"), &paths).is_err());
        assert!(program
            .evaluate(&format!("$(value {name})"), &paths)
            .is_err());
    }
}

#[test]
fn source_only_absence_does_not_loosen_legacy_or_unknown_reference_guards() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let names = namespace(&[]);
    let mut source = MakeVariableProgram::new_source_program(&paths, &names, &[]).unwrap();
    assert_eq!(
        source.evaluate("$(/)|$(ORDINARY_MISSING)", &paths).unwrap(),
        "|"
    );
    for expression in [
        "$(.UNKNOWN_ENGINE_NAME)",
        "$(origin .UNKNOWN_ENGINE_NAME)",
        "$( @)",
        "$(origin @)",
        "$(flavor @D)",
    ] {
        assert!(
            source.evaluate(expression, &paths).is_err(),
            "unknown engine or automatic reference {expression:?} must remain unresolved"
        );
    }

    let mut legacy =
        MakeVariableProgram::new_with_namespace(&TargetContext::default(), &[], &paths, &names)
            .unwrap();
    assert!(legacy.evaluate("$(/)", &paths).is_err());
    assert!(legacy.evaluate("$(origin /)", &paths).is_err());
    assert!(legacy.evaluate("$(.)", &paths).is_err());
    assert!(legacy.evaluate("$(origin .)", &paths).is_err());
}
