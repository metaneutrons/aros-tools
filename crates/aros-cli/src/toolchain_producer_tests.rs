//! Regression tests for the adjacent production module.

use std::fs;

use clap::{error::ErrorKind, Parser};

use super::{
    compare, compatibility_ports_source_closure, environment, CompareArgs, EnvironmentArgs,
    PackageArgs, ResultFormat,
};
use crate::Cli;

fn parse_package_format(command: &str, package_format: &str) -> Result<Cli, clap::Error> {
    let mut args = vec![
        "aros",
        "toolchain",
        "producer",
        command,
        "--recipe",
        "/producer/recipe.json",
        "--source-lock",
        "/producer/toolchains/lock.sources.json",
        "--profiles",
        "/producer/toolchains/profiles.json",
        "--preset",
        "pc-x86_64",
        "--release-id",
        "candidate-1",
        "--host",
        "linux-x86_64",
        "--build-environment",
        "/evidence/environment.json",
        "--input-dir",
        "/candidate/toolchain",
        "--package-format",
        package_format,
    ];
    if command == "package" {
        args.extend(["--output-dir", "/packages/candidate"]);
    }
    Cli::try_parse_from(args)
}

#[test]
fn package_format_option_is_closed_and_available_for_both_package_commands() {
    for command in ["package", "verify-package"] {
        for package_format in ["legacy-v1", "family-v2"] {
            assert!(
                parse_package_format(command, package_format).is_ok(),
                "{command} should accept {package_format}"
            );
        }

        let Err(error) = parse_package_format(command, "future-v3") else {
            panic!("{command} unexpectedly accepted an unknown package format");
        };
        assert_eq!(error.kind(), ErrorKind::InvalidValue, "{command}");
    }
}

#[derive(Parser)]
struct PackageParser {
    #[command(flatten)]
    args: PackageArgs,
}

fn guarded_package_arguments() -> Vec<&'static str> {
    vec![
        "package",
        "--recipe",
        "/recipe.json",
        "--source-lock",
        "/lock.json",
        "--profiles",
        "/profiles.json",
        "--preset",
        "pc-x86_64",
        "--release-id",
        "candidate-1",
        "--host",
        "linux-x86_64",
        "--build-environment",
        "/environment.json",
        "--input-dir",
        "/candidate/toolchain",
        "--output-dir",
        "/package",
        "--package-format",
        "family-v2",
        "--build-result",
        "/build.json",
        "--build-result-sha256",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "--build-work-dir",
        "/work",
    ]
}

#[test]
fn finished_package_options_require_the_complete_explicit_selection() {
    let arguments = guarded_package_arguments();
    assert!(PackageParser::try_parse_from(&arguments).is_ok());
    for option in [
        "--build-result-sha256",
        "--build-work-dir",
        "--package-format",
        "--build-result",
    ] {
        let mut incomplete = arguments.clone();
        let position = incomplete
            .iter()
            .position(|value| *value == option)
            .unwrap();
        incomplete.drain(position..position + 2);
        let error = PackageParser::try_parse_from(incomplete).err().unwrap();
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument, "{option}");
    }
}

#[test]
fn finished_package_digest_is_closed_and_legacy_format_is_rejected_before_io() {
    let arguments = guarded_package_arguments();
    for invalid in [
        "abc",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        let mut invalid_arguments = arguments.clone();
        let position = invalid_arguments
            .iter()
            .position(|value| *value == "--build-result-sha256")
            .unwrap();
        invalid_arguments[position + 1] = invalid;
        assert_eq!(
            PackageParser::try_parse_from(invalid_arguments)
                .err()
                .unwrap()
                .kind(),
            ErrorKind::ValueValidation,
        );
    }
    let mut legacy = arguments;
    let position = legacy
        .iter()
        .position(|value| *value == "--package-format")
        .unwrap();
    legacy[position + 1] = "legacy-v1";
    let parsed = PackageParser::try_parse_from(legacy).unwrap();
    let error = super::package(&parsed.args).unwrap_err();
    assert!(error
        .to_string()
        .contains("requires --package-format family-v2"));
}

#[test]
fn index_format_requires_only_its_own_stage_inputs() {
    let base = [
        "aros",
        "toolchain",
        "producer",
        "index",
        "--directory",
        "/release",
        "--release-id",
        "candidate-1",
        "--base-url",
        "https://example.invalid/candidate-1",
    ];
    let mut legacy = base.to_vec();
    legacy.extend([
        "--source-lock-filename",
        "lock.json",
        "--stage",
        "pre-attestation",
    ]);
    assert!(Cli::try_parse_from(&legacy).is_ok());
    let mut family = base.to_vec();
    family.extend([
        "--release-format",
        "family-v2",
        "--lane-inputs",
        "/lane-inputs.json",
        "--subject-manifest",
        "/subjects.sha256",
        "--stage",
        "pre-attestation",
    ]);
    assert!(Cli::try_parse_from(&family).is_ok());
    family.extend(["--source-lock-filename", "lock.json"]);
    assert!(Cli::try_parse_from(&family).is_err());

    let mut final_stage = base.to_vec();
    final_stage.extend([
        "--release-format",
        "family-v2",
        "--lane-inputs",
        "/lane-inputs.json",
        "--subject-manifest",
        "/subjects.sha256",
        "--stage",
        "final",
    ]);
    assert!(Cli::try_parse_from(&final_stage).is_err());
    let digest = "a".repeat(64);
    final_stage.extend(["--subject-manifest-sha256", &digest]);
    assert!(Cli::try_parse_from(&final_stage).is_ok());

    for incomplete in [
        vec![
            "--release-format",
            "family-v2",
            "--stage",
            "pre-attestation",
        ],
        vec![
            "--release-format",
            "future-v3",
            "--stage",
            "pre-attestation",
        ],
        vec!["--stage", "pre-attestation"],
    ] {
        let mut arguments = base.to_vec();
        arguments.extend(incomplete);
        assert!(Cli::try_parse_from(&arguments).is_err(), "{arguments:?}");
    }
}

#[test]
fn producer_stage_surface_excludes_the_migrated_source_cache_frontends() {
    let recipe = Cli::try_parse_from([
        "aros",
        "toolchain",
        "producer",
        "recipe",
        "--source-dir",
        "/source",
        "--producer-dir",
        "/producer",
        "--tools-dir",
        "/tools",
        "--source-lock",
        "/producer/toolchains/lock.sources.json",
        "--profiles",
        "/producer/toolchains/profiles.json",
        "--output",
        "/output/recipe.json",
    ]);
    assert!(recipe.is_ok());
    let removed_cache = Cli::try_parse_from([
        "aros",
        "toolchain",
        "producer",
        "cache",
        "--source-lock",
        "/producer/toolchains/lock.sources.json",
        "--cache-dir",
        "/cache",
        "--verify-only",
    ]);
    assert!(removed_cache.is_err());
    let profile = Cli::try_parse_from([
        "aros",
        "toolchain",
        "producer",
        "profile",
        "--recipe",
        "/producer/recipe.json",
        "--profiles",
        "/producer/toolchains/profiles.json",
        "--preset",
        "pc-x86_64",
        "--format",
        "json",
    ]);
    assert!(profile.is_ok());
    let package = Cli::try_parse_from([
        "aros",
        "toolchain",
        "producer",
        "package",
        "--recipe",
        "/producer/recipe.json",
        "--source-lock",
        "/producer/toolchains/lock.sources.json",
        "--profiles",
        "/producer/toolchains/profiles.json",
        "--preset",
        "pc-x86_64",
        "--release-id",
        "candidate-1",
        "--host",
        "linux-x86_64",
        "--build-environment",
        "/evidence/environment.json",
        "--input-dir",
        "/candidate/toolchain",
        "--output-dir",
        "/packages/first",
    ]);
    assert!(package.is_ok());
    let compare = Cli::try_parse_from([
        "aros",
        "toolchain",
        "producer",
        "compare",
        "--left",
        "/packages/left",
        "--right",
        "/packages/right",
        "--output",
        "/evidence/comparison.json",
    ]);
    assert!(compare.is_ok());
    let repackage = Cli::try_parse_from([
        "aros",
        "toolchain",
        "producer",
        "repackage",
        "--recovery-request",
        "/evidence/recovery.json",
        "--source-package-dir",
        "/packages/source",
        "--source-release-id",
        "toolchain-v1-source",
        "--recipe",
        "/producer/recipe.json",
        "--source-lock",
        "/producer/toolchains/lock.sources.json",
        "--profiles",
        "/producer/toolchains/profiles.json",
        "--preset",
        "pc-x86_64",
        "--host",
        "linux-x86_64",
        "--build-environment",
        "/evidence/environment.json",
        "--first-extraction-dir",
        "/work/extracted-a",
        "--second-extraction-dir",
        "/work/extracted-b",
        "--first-output-dir",
        "/packages/recovered-a",
        "--second-output-dir",
        "/packages/recovered-b",
        "--comparison-output",
        "/evidence/recovery-comparison.json",
    ]);
    assert!(repackage.is_ok());
    let validation = Cli::try_parse_from([
        "aros",
        "toolchain",
        "producer",
        "validate-recovery",
        "--recovery-request",
        "/evidence/recovery.json",
        "--release-dir",
        "/release/source",
        "--output",
        "/evidence/recovery-validation.json",
    ]);
    assert!(validation.is_ok());
    let qualification = Cli::try_parse_from([
        "aros",
        "toolchain",
        "producer",
        "record-qualification",
        "--release-dir",
        "/release/source",
        "--source-lock-filename",
        "llvm-11.0.0.sources.json",
        "--lifecycle-reports-dir",
        "/evidence/lifecycle",
        "--comparison-reports-dir",
        "/evidence/comparison",
        "--compatibility-reports-dir",
        "/evidence/compatibility",
        "--source-repository",
        "https://github.com/metaneutrons/aros-toolchains",
        "--source-workflow",
        ".github/workflows/toolchain-release.yml",
        "--source-run-id",
        "42",
        "--source-tag",
        "toolchain-v1-source",
        "--source-tag-object",
        "0123456789012345678901234567890123456789",
        "--source-tag-commit",
        "0123456789012345678901234567890123456789",
        "--attestation-repository",
        "https://github.com/metaneutrons/aros-toolchains",
        "--attestation-workflow",
        ".github/workflows/toolchain-release.yml",
        "--attestation-signer",
        "github-actions",
        "--created-at",
        "100",
        "--expires-at",
        "200",
        "--output",
        "/evidence/qualification.json",
    ]);
    assert!(qualification.is_ok());
    let recovery_request = Cli::try_parse_from([
        "aros",
        "toolchain",
        "producer",
        "prepare-recovery",
        "--qualification-evidence",
        "/evidence/qualification.json",
        "--release-dir",
        "/release/source",
        "--source-tag-object",
        "0123456789012345678901234567890123456789",
        "--source-tag-commit",
        "0123456789012345678901234567890123456789",
        "--recovery-release-id",
        "toolchain-v1-recovered",
        "--recovery-tag-object",
        "1234567890123456789012345678901234567890",
        "--recovery-tag-commit",
        "0123456789012345678901234567890123456789",
        "--source-repository",
        "https://github.com/metaneutrons/aros-toolchains",
        "--source-workflow",
        ".github/workflows/toolchain-release.yml",
        "--attestation-repository",
        "https://github.com/metaneutrons/aros-toolchains",
        "--attestation-workflow",
        ".github/workflows/toolchain-release.yml",
        "--attestation-signer",
        "github-actions",
        "--now",
        "150",
        "--output",
        "/evidence/recovery.json",
    ]);
    assert!(recovery_request.is_ok());
    let source = Cli::try_parse_from([
        "aros",
        "toolchain",
        "producer",
        "materialize-engine-free-source",
        "--source-dir",
        "/source",
        "--recipe",
        "/producer/recipe.json",
        "--output-dir",
        "/output/engine-free",
    ]);
    assert!(source.is_ok());
    let index = Cli::try_parse_from([
        "aros",
        "toolchain",
        "producer",
        "index",
        "--directory",
        "/release",
        "--release-id",
        "candidate-1",
        "--base-url",
        "https://aros-toolchains.metaneutrons.cc/releases/candidate-1",
        "--source-lock-filename",
        "lock.sources.json",
        "--stage",
        "pre-attestation",
    ]);
    assert!(index.is_ok());
    assert!(Cli::try_parse_from([
        "aros",
        "toolchain",
        "producer",
        "compatibility-host-tools",
        "--host",
        "linux-x86_64",
    ])
    .is_ok());
}

#[test]
fn engine_free_source_requires_one_complete_explicit_identity() {
    let base = [
        "aros",
        "toolchain",
        "producer",
        "materialize-engine-free-source",
        "--source-dir",
        "/source",
        "--output-dir",
        "/output/engine-free",
    ];
    let commit = "1111111111111111111111111111111111111111";
    let tree = "2222222222222222222222222222222222222222";
    let parse = |extra: &[&str]| Cli::try_parse_from(base.into_iter().chain(extra.iter().copied()));
    assert!(parse(&["--recipe", "/producer/recipe.json"]).is_ok());
    assert!(parse(&["--source-commit", commit, "--source-tree", tree]).is_ok());
    for extra in [
        vec![],
        vec!["--source-commit", commit],
        vec!["--source-tree", tree],
        vec![
            "--recipe",
            "/producer/recipe.json",
            "--source-commit",
            commit,
            "--source-tree",
            tree,
        ],
        vec!["--source-commit", "HEAD", "--source-tree", tree],
        vec!["--source-commit", commit, "--source-tree", "main"],
        vec![
            "--source-commit",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "--source-tree",
            tree,
        ],
    ] {
        assert!(parse(&extra).is_err(), "must reject {extra:?}");
    }
}

#[test]
fn recovery_help_does_not_hard_code_the_historical_inventory_size() {
    for command in [
        "validate-recovery",
        "record-qualification",
        "prepare-recovery",
    ] {
        let Err(error) = Cli::try_parse_from(["aros", "toolchain", "producer", command, "--help"])
        else {
            panic!("{command} help unexpectedly parsed as an invocation");
        };
        assert_eq!(error.kind(), ErrorKind::DisplayHelp, "{command}");
        let help = error.to_string();
        assert!(
            !help.contains("56-member"),
            "{command} help must not hard-code the historical release shape"
        );
        if command != "record-qualification" {
            assert!(
                help.contains("selected by its release index"),
                "{command} help must describe index-selected inventory shapes"
            );
        }
    }
}

#[test]
fn environment_receipt_is_closed_and_non_overwriting() {
    let temporary = tempfile::tempdir().unwrap();
    let output = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("environment.json");
    environment(&EnvironmentArgs {
        host: "linux-x86_64".into(),
        output: output.clone(),
        format: ResultFormat::Human,
    })
    .unwrap();
    let document: serde_json::Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    assert_eq!(
        document,
        serde_json::json!({
            "schema": "aros-toolchain-build-environment-v1",
            "host": "linux-x86_64",
        })
    );
    assert!(environment(&EnvironmentArgs {
        host: "linux-x86_64".into(),
        output,
        format: ResultFormat::Human,
    })
    .is_err());
}

#[test]
fn comparison_receipt_is_closed_and_non_overwriting() {
    let temporary = tempfile::tempdir().unwrap();
    let left = temporary.path().join("left");
    let right = temporary.path().join("right");
    fs::create_dir(&left).unwrap();
    fs::create_dir(&right).unwrap();
    for directory in [&left, &right] {
        fs::write(directory.join("archive.tar.xz"), b"archive").unwrap();
        fs::write(directory.join("archive.tar.xz.manifest.json"), b"manifest").unwrap();
        fs::write(directory.join("archive.tar.xz.sha256"), b"checksum").unwrap();
        fs::write(directory.join("archive.tar.xz.spdx.json"), b"sbom").unwrap();
    }
    let output = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("comparison.json");
    let args = CompareArgs {
        left,
        right,
        output: output.clone(),
        format: ResultFormat::Human,
    };
    compare(&args).unwrap();
    let document: serde_json::Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    assert_eq!(document["schema"], 1);
    assert_eq!(document["operation"], "compare");
    assert_eq!(document["members"].as_array().unwrap().len(), 4);
    assert!(compare(&args).is_err());
}

#[test]
fn compatibility_receipt_closure_accepts_profiled_sources_and_rejects_tampering() {
    let sources = vec![
        serde_json::json!({
            "id": "unicode-data",
            "cache_filename": "UnicodeData.txt",
            "relative_path": "UnicodeData.txt",
            "fetch_marker": "",
            "sha256": "a".repeat(64),
            "size": 1,
        }),
        serde_json::json!({
            "id": "mesa",
            "cache_filename": "mesa-20.0.8.tar.xz",
            "relative_path": "ports/mesa-20.0.8.tar.xz",
            "fetch_marker": "ports/.mesa-20.0.8-fetched",
            "sha256": "b".repeat(64),
            "size": 2,
        }),
    ];
    assert!(compatibility_ports_source_closure(&sources));

    let mut duplicate_path = sources.clone();
    duplicate_path[1]["relative_path"] = serde_json::json!("UnicodeData.txt");
    assert!(!compatibility_ports_source_closure(&duplicate_path));

    let mut unsafe_path = sources;
    unsafe_path[1]["relative_path"] = serde_json::json!("../mesa-20.0.8.tar.xz");
    assert!(!compatibility_ports_source_closure(&unsafe_path));

    let mut unsafe_marker = unsafe_path;
    unsafe_marker[1]["relative_path"] = serde_json::json!("ports/mesa-20.0.8.tar.xz");
    unsafe_marker[1]["fetch_marker"] = serde_json::json!("ports/../.mesa-20.0.8-fetched");
    assert!(!compatibility_ports_source_closure(&unsafe_marker));
}
