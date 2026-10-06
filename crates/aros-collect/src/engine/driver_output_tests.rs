//! Regression cases for explicit output and GNU driver default normalization.

use std::ffi::OsString;
use std::path::Path;

use super::{parse_configured, replace_output};

fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[test]
fn configured_gnu_default_is_explicitly_staged_before_the_operand_boundary() {
    let request = parse_configured(
        "collect-aros".into(),
        "ld".into(),
        "strip".into(),
        None,
        None,
        Some(Path::new("a.out")),
        strings(&["-T", "-o-script.ld", "--", "-o-input.o"]),
    )
    .unwrap();
    assert_eq!(request.output, Path::new("a.out"));
    assert_eq!(
        request.args,
        strings(&["-T", "-o-script.ld", "-o", "a.out", "--", "-o-input.o"])
    );
    assert_eq!(
        replace_output(&request.args, Path::new("a.out.collect-pre")).unwrap(),
        strings(&[
            "-T",
            "-o-script.ld",
            "-o",
            "a.out.collect-pre",
            "--",
            "-o-input.o"
        ])
    );
}

#[test]
fn gnu_default_does_not_hide_malformed_or_explicit_output_arguments() {
    for arguments in [
        strings(&["-o"]),
        strings(&["-o", ""]),
        strings(&["--output="]),
        strings(&["-L"]),
        strings(&["-m"]),
    ] {
        assert!(parse_configured(
            "collect-aros".into(),
            "ld".into(),
            "strip".into(),
            None,
            None,
            Some(Path::new("a.out")),
            arguments
        )
        .is_err());
    }
    let explicit = parse_configured(
        "collect-aros".into(),
        "ld".into(),
        "strip".into(),
        None,
        None,
        Some(Path::new("a.out")),
        strings(&["--output=selected.o", "input.o"]),
    )
    .unwrap();
    assert_eq!(explicit.output, Path::new("selected.o"));
    assert_eq!(explicit.args, strings(&["--output=selected.o", "input.o"]));
    assert!(parse_configured(
        "collect-aros".into(),
        "ld.lld".into(),
        "llvm-strip".into(),
        None,
        None,
        None,
        strings(&["input.o"])
    )
    .is_err());
}
