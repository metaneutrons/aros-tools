//! Parser-only coverage for the explicit legacy-v1 and family-v2 repackage modes.

use clap::{error::ErrorKind, CommandFactory, Parser};

use crate::Cli;

const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn shared_arguments() -> Vec<&'static str> {
    vec![
        "aros",
        "toolchain",
        "producer",
        "repackage",
        "--recovery-request",
        "/evidence/recovery-request.json",
        "--source-release-id",
        "retained-release-1",
        "--first-extraction-dir",
        "/isolated/extract-a",
        "--second-extraction-dir",
        "/isolated/extract-b",
        "--first-output-dir",
        "/isolated/package-a",
        "--second-output-dir",
        "/isolated/package-b",
        "--comparison-output",
        "/evidence/comparison.json",
    ]
}

fn legacy_arguments() -> Vec<&'static str> {
    let mut arguments = shared_arguments();
    arguments.extend([
        "--source-package-dir",
        "/retained/package",
        "--recipe",
        "/inputs/recipe.json",
        "--source-lock",
        "/inputs/source-lock.json",
        "--profiles",
        "/inputs/profiles.json",
        "--preset",
        "pc-x86_64",
        "--host",
        "linux-x86_64",
        "--build-environment",
        "/evidence/build-environment.json",
    ]);
    arguments
}

fn family_v2_arguments() -> Vec<&'static str> {
    let mut arguments = shared_arguments();
    arguments.extend([
        "--release-format",
        "family-v2",
        "--release-dir",
        "/release",
        "--base-url",
        "https://example.invalid/releases/recovered",
        "--inputs-sha256",
        DIGEST,
        "--index-sha256",
        DIGEST,
        "--selection",
        "/evidence/selection.json",
        "--selection-sha256",
        DIGEST,
        "--subject-manifest",
        "/evidence/subjects.sha256",
        "--subject-manifest-sha256",
        DIGEST,
        "--qualification-evidence",
        "/evidence/qualification.json",
        "--qualification-sha256",
        DIGEST,
        "--policy",
        "/evidence/policy.json",
        "--policy-sha256",
        DIGEST,
        "--recovery-request-sha256",
        DIGEST,
        "--asset",
        "fixture-asset.tar.xz",
    ]);
    arguments
}

fn remove_option(arguments: &[&'static str], option: &str) -> Vec<&'static str> {
    let mut incomplete = arguments.to_vec();
    let position = incomplete
        .iter()
        .position(|argument| *argument == option)
        .unwrap_or_else(|| panic!("{option} is absent from fixture arguments"));
    incomplete.drain(position..position + 2);
    incomplete
}

#[test]
fn repackage_accepts_legacy_v1_with_implicit_or_explicit_format() {
    let mut implicit = legacy_arguments();
    implicit.extend(["--format", "human"]);
    assert!(Cli::try_parse_from(implicit).is_ok());

    let mut explicit = legacy_arguments();
    explicit.extend(["--release-format", "legacy-v1", "--format", "json"]);
    assert!(Cli::try_parse_from(explicit).is_ok());
}

#[test]
fn repackage_accepts_a_complete_explicit_family_v2_selection() {
    let mut arguments = family_v2_arguments();
    arguments.extend(["--forbidden-prefix", "/build/private", "--format", "json"]);
    assert!(Cli::try_parse_from(arguments).is_ok());
}

#[test]
fn repackage_family_v2_requires_every_explicit_selection_flag() {
    let arguments = family_v2_arguments();
    for option in [
        "--release-format",
        "--release-dir",
        "--base-url",
        "--inputs-sha256",
        "--index-sha256",
        "--selection",
        "--selection-sha256",
        "--subject-manifest",
        "--subject-manifest-sha256",
        "--qualification-evidence",
        "--qualification-sha256",
        "--policy",
        "--policy-sha256",
        "--recovery-request-sha256",
        "--asset",
    ] {
        let error = Cli::try_parse_from(remove_option(&arguments, option))
            .err()
            .unwrap_or_else(|| panic!("family-v2 unexpectedly accepted missing {option}"));
        assert_eq!(
            error.kind(),
            ErrorKind::MissingRequiredArgument,
            "{option}: {error}"
        );
    }
}

#[test]
fn repackage_legacy_v1_requires_every_source_selector() {
    for explicit in [false, true] {
        let mut arguments = legacy_arguments();
        if explicit {
            arguments.extend(["--release-format", "legacy-v1"]);
        }
        for option in [
            "--source-package-dir",
            "--recipe",
            "--source-lock",
            "--profiles",
            "--preset",
            "--host",
            "--build-environment",
        ] {
            let error = Cli::try_parse_from(remove_option(&arguments, option))
                .err()
                .unwrap_or_else(|| panic!("legacy-v1 unexpectedly accepted missing {option}"));
            assert_eq!(
                error.kind(),
                ErrorKind::MissingRequiredArgument,
                "explicit={explicit}, {option}: {error}"
            );
        }
    }
}

#[test]
fn repackage_rejects_legacy_selectors_in_family_v2_mode() {
    let family = family_v2_arguments();
    for (option, value) in [
        ("--source-package-dir", "/retained/package"),
        ("--recipe", "/inputs/recipe.json"),
        ("--source-lock", "/inputs/source-lock.json"),
        ("--profiles", "/inputs/profiles.json"),
        ("--preset", "pc-x86_64"),
        ("--host", "linux-x86_64"),
        ("--build-environment", "/evidence/build-environment.json"),
    ] {
        let mut mixed = family.clone();
        mixed.extend([option, value]);
        let error = Cli::try_parse_from(mixed)
            .err()
            .expect("mixed family-v2 and legacy arguments must fail");
        assert_eq!(
            error.kind(),
            ErrorKind::ArgumentConflict,
            "{option}: {error}"
        );
    }
}

#[test]
fn repackage_rejects_family_v2_metadata_in_legacy_v1_mode() {
    let mut mixed = legacy_arguments();
    mixed.extend([
        "--release-format",
        "legacy-v1",
        "--release-dir",
        "/release",
        "--base-url",
        "https://example.invalid/releases/recovered",
        "--inputs-sha256",
        DIGEST,
        "--index-sha256",
        DIGEST,
        "--selection",
        "/evidence/selection.json",
        "--selection-sha256",
        DIGEST,
        "--subject-manifest",
        "/evidence/subjects.sha256",
        "--subject-manifest-sha256",
        DIGEST,
        "--qualification-evidence",
        "/evidence/qualification.json",
        "--qualification-sha256",
        DIGEST,
        "--policy",
        "/evidence/policy.json",
        "--policy-sha256",
        DIGEST,
        "--recovery-request-sha256",
        DIGEST,
        "--asset",
        "fixture-asset.tar.xz",
    ]);
    let error = Cli::try_parse_from(mixed)
        .err()
        .expect("family-v2 metadata must conflict with legacy arguments");
    assert_eq!(error.kind(), ErrorKind::ArgumentConflict, "{error}");
}

#[test]
fn repackage_release_format_is_closed() {
    let mut arguments = family_v2_arguments();
    let position = arguments
        .iter()
        .position(|argument| *argument == "--release-format")
        .unwrap();
    arguments[position + 1] = "future-v3";
    let error = Cli::try_parse_from(arguments)
        .err()
        .expect("unknown repackage format must fail parsing");
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
}

#[test]
fn repackage_rejects_selectors_from_only_the_opposite_format() {
    let mut wrong_family = family_v2_arguments();
    let position = wrong_family
        .iter()
        .position(|argument| *argument == "--release-format")
        .unwrap();
    wrong_family[position + 1] = "legacy-v1";
    assert!(
        Cli::try_parse_from(wrong_family).is_err(),
        "explicit legacy-v1 must not accept only family-v2 selectors"
    );
    let mut wrong_legacy = legacy_arguments();
    wrong_legacy.extend(["--release-format", "family-v2"]);
    assert!(
        Cli::try_parse_from(wrong_legacy).is_err(),
        "explicit family-v2 must not accept only legacy-v1 selectors"
    );
}

#[test]
fn complete_cli_command_shape_passes_clap_validation() {
    Cli::command().debug_assert();
}
