//! Frontend parser, routing and public contract regressions.

use super::{
    cli_contract::RepositoryRequirement,
    cli_contract_render::{
        public_contract_commands, rendered_cli_contract_index, rendered_cli_contract_section,
        table_cell,
    },
    cli_contract_sections::CLI_CONTRACT_SECTIONS,
    command_boundary, embedded_board_registry, BoardCommand, BoardInitTransport, Cli, Commands,
    Parser,
};
use clap::{error::ErrorKind, CommandFactory};

const CLI_CONTRACT_INDEX: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs-site/src/content/docs/reference/cli-contract.md"
));
#[test]
fn cli_contract_escapes_markdown_table_separators_once() {
    assert_eq!(table_cell("one|two"), "one\\|two");
}

#[test]
fn generated_public_cli_contract_matches_the_clap_model() {
    let mut command = Cli::command();
    command.build();
    assert_eq!(
        CLI_CONTRACT_INDEX,
        rendered_cli_contract_index(&command),
        "a public CLI-model change requires an intentional reviewed update to docs-site/src/content/docs/reference/cli-contract.md"
    );
    let expected_sections = CLI_CONTRACT_SECTIONS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>();
    let actual_sections = public_contract_commands(&command)
        .iter()
        .map(|child| child.get_name())
        .collect::<Vec<_>>();
    assert_eq!(
        expected_sections, actual_sections,
        "every visible top-level command needs exactly one generated contract section"
    );
    for (name, expected) in CLI_CONTRACT_SECTIONS {
        let section = command
            .find_subcommand(name)
            .expect("declared public contract section must resolve");
        assert_eq!(
            *expected,
            rendered_cli_contract_section(section),
            "a public CLI-model change requires an intentional reviewed update to docs-site/src/content/docs/reference/cli-contract/{name}.md"
        );
    }
}

#[test]
fn diagnostic_context_identifies_the_exact_command_leaf() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["aros", "host-compiler", "install"],
            "host-compiler.install",
        ),
        (&["aros", "build-tools", "check"], "build-tools.check"),
        (&["aros", "toolchain", "list"], "toolchain.list"),
        (
            &["aros", "toolchain", "path", "--preset", "pc-x86_64"],
            "toolchain.path",
        ),
        (
            &[
                "aros",
                "toolchain",
                "producer",
                "compatibility-host-tools",
                "--host",
                "linux-x86_64",
            ],
            "toolchain.producer.compatibility-host-tools",
        ),
        (&["aros", "board", "scan"], "board.scan"),
        (&["aros", "board", "sd", "scan"], "board.sd.scan"),
        (
            &["aros", "image", "verify", "--artifact", "/tmp/media"],
            "image.verify",
        ),
        (
            &[
                "aros",
                "image",
                "build",
                "--profile",
                "rpi4-uboot-usb-ecm",
                "--build-root",
                "/tmp/build",
                "--receipt",
                "/tmp/receipt.json",
                "--output",
                "/tmp/media",
            ],
            "image.build",
        ),
        (
            &["aros", "image", "inspect", "--artifact", "/tmp/media"],
            "image.inspect",
        ),
        (&["aros", "golden", "capture"], "golden.capture"),
        (&["aros", "source", "init", "/tmp/AROS"], "source.init"),
    ];
    for (arguments, expected_mode) in cases {
        let command = Cli::try_parse_from(*arguments)
            .expect("leaf diagnostic case must parse")
            .command;
        let (boundary, context) = command_boundary(&command);
        assert_eq!(context.mode.as_deref(), Some(*expected_mode));
        assert!(
            !boundary.hint.is_empty(),
            "every exact command leaf must retain actionable recovery guidance"
        );
    }
}

#[test]
#[ignore = "developer aid for regenerating the reviewed CLI contract snapshots"]
fn regenerate_public_cli_contract_snapshots() {
    let mut command = Cli::command();
    command.build();
    let documentation_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs-site/src/content/docs/reference");
    std::fs::write(
        documentation_root.join("cli-contract.md"),
        rendered_cli_contract_index(&command),
    )
    .expect("write generated contract index");
    for (name, _) in CLI_CONTRACT_SECTIONS {
        let section = command
            .find_subcommand(name)
            .expect("declared public contract section must resolve");
        std::fs::write(
            documentation_root
                .join("cli-contract")
                .join(format!("{name}.md")),
            rendered_cli_contract_section(section),
        )
        .expect("write generated contract section");
    }
}

fn requirement(arguments: &[&str]) -> RepositoryRequirement {
    Cli::try_parse_from(arguments)
        .expect("valid command line")
        .command
        .repository_requirement()
}

#[test]
fn repository_policy_is_explicit_for_each_command_class() {
    assert_eq!(
        requirement(&["aros", "source", "init", "AROS"]),
        RepositoryRequirement::Global
    );
    assert_eq!(
        requirement(&["aros", "board", "scan"]),
        RepositoryRequirement::Global
    );
    assert_eq!(
        requirement(&["aros", "image", "verify", "--artifact", "/tmp/media"]),
        RepositoryRequirement::Global
    );
    assert_eq!(
        requirement(&[
            "aros",
            "image",
            "build",
            "--profile",
            "rpi4-uboot-usb-ecm",
            "--build-root",
            "/tmp/build",
            "--receipt",
            "/tmp/receipt.json",
            "--output",
            "/tmp/media",
        ]),
        RepositoryRequirement::Global
    );
    assert_eq!(
        requirement(&["aros", "toolchain", "inventory"]),
        RepositoryRequirement::Global
    );
    assert_eq!(
        requirement(&["aros", "info"]),
        RepositoryRequirement::Optional
    );
    assert_eq!(
        requirement(&["aros", "clean", "--preset", "pc-x86_64"]),
        RepositoryRequirement::Required
    );
    assert_eq!(
        requirement(&["aros", "source", "sync"]),
        RepositoryRequirement::Required
    );
}

#[test]
fn sdk_local_stages_require_exact_selections_and_have_global_diagnostics() {
    for leaf in ["package", "verify", "extract"] {
        assert_eq!(
            parse_error(&["aros", "sdk", leaf]),
            ErrorKind::MissingRequiredArgument
        );
        let mut arguments = vec![
            "aros".to_owned(),
            "sdk".to_owned(),
            leaf.to_owned(),
            "--identity".to_owned(),
            "/tmp/identity.json".to_owned(),
            "--identity-sha256".to_owned(),
            "1".repeat(64),
            "--tree-sha256".to_owned(),
            "2".repeat(64),
            "--forbidden-prefix".to_owned(),
            "/tmp/build".to_owned(),
        ];
        if leaf == "package" {
            arguments.extend(
                [
                    "--candidate-root",
                    "/tmp/sdk",
                    "--output-dir",
                    "/tmp/package",
                ]
                .map(str::to_owned),
            );
        } else {
            arguments.extend([
                "--package-dir".to_owned(),
                "/tmp/package".to_owned(),
                "--archive-sha256".to_owned(),
                "3".repeat(64),
                "--archive-size".to_owned(),
                "1".to_owned(),
            ]);
            if leaf == "extract" {
                arguments.extend(["--output-root", "/tmp/extracted"].map(str::to_owned));
            }
        }
        let command = Cli::try_parse_from(&arguments).unwrap().command;
        assert_eq!(
            command.repository_requirement(),
            RepositoryRequirement::Global
        );
        let (_, context) = command_boundary(&command);
        assert_eq!(
            context.mode.as_deref(),
            Some(format!("sdk.{leaf}").as_str())
        );
    }
}

#[test]
fn image_build_requires_explicit_inputs_and_apply_is_mutually_exclusive_with_dry_run() {
    assert_eq!(
        parse_error(&["aros", "image", "build"]),
        ErrorKind::MissingRequiredArgument
    );
    assert_eq!(
        parse_error(&[
            "aros",
            "image",
            "build",
            "--profile",
            "rpi4-uboot-usb-ecm",
            "--build-root",
            "/tmp/build",
            "--receipt",
            "/tmp/receipt.json",
            "--output",
            "/tmp/media",
            "--apply",
            "--dry-run",
        ]),
        ErrorKind::ArgumentConflict
    );
    Cli::try_parse_from([
        "aros",
        "image",
        "build",
        "--profile",
        "rpi4-uboot-usb-ecm",
        "--build-root",
        "/tmp/build",
        "--receipt",
        "/tmp/receipt.json",
        "--output",
        "/tmp/media",
        "--lock",
        "firmware=/tmp/lock.toml",
        "--external",
        "firmware:blob=/tmp/firmware.bin",
    ])
    .expect("valid explicit image build");
}

fn parse_error(arguments: &[&str]) -> ErrorKind {
    match Cli::try_parse_from(arguments) {
        Ok(_) => panic!("command line unexpectedly parsed: {arguments:?}"),
        Err(error) => error.kind(),
    }
}

#[test]
fn setup_modes_are_mutually_exclusive_at_the_cli_boundary() {
    assert_eq!(
        parse_error(&["aros", "setup", "--all", "--preset", "pc-x86_64"]),
        ErrorKind::ArgumentConflict
    );
    assert_eq!(
        parse_error(&["aros", "setup", "--all", "--local", "/opt/aros"]),
        ErrorKind::ArgumentConflict
    );
}

#[test]
fn setup_local_override_requires_one_preset() {
    assert_eq!(
        parse_error(&["aros", "setup", "--local", "/opt/aros"]),
        ErrorKind::MissingRequiredArgument
    );
    assert!(Cli::try_parse_from([
        "aros",
        "setup",
        "--preset",
        "pc-x86_64",
        "--local",
        "/opt/aros",
    ])
    .is_ok());
}

#[test]
fn installation_transport_modes_fail_before_repository_discovery() {
    for arguments in [
        &[
            "aros",
            "setup",
            "--preset",
            "pc-x86_64",
            "--local",
            "/opt/aros",
            "--force",
        ][..],
        &[
            "aros",
            "setup",
            "--preset",
            "pc-x86_64",
            "--force",
            "--offline",
        ][..],
        &["aros", "host-compiler", "install", "--force", "--offline"][..],
        &[
            "aros",
            "toolchain",
            "install",
            "--preset",
            "pc-x86_64",
            "--local",
            "/opt/aros",
            "--force",
        ][..],
        &[
            "aros",
            "toolchain",
            "install",
            "--preset",
            "pc-x86_64",
            "--force",
            "--offline",
        ][..],
    ] {
        assert_eq!(
            parse_error(arguments),
            ErrorKind::ArgumentConflict,
            "{arguments:?}"
        );
    }

    assert!(Cli::try_parse_from([
        "aros",
        "setup",
        "--preset",
        "pc-x86_64",
        "--local",
        "/opt/aros",
        "--offline",
    ])
    .is_ok());
    assert!(Cli::try_parse_from(["aros", "host-compiler", "install", "--offline"]).is_ok());
    assert!(Cli::try_parse_from([
        "aros",
        "toolchain",
        "install",
        "--preset",
        "pc-x86_64",
        "--local",
        "/opt/aros",
        "--offline",
    ])
    .is_ok());
}

#[test]
fn source_sync_uses_branch_while_source_init_retains_ref() {
    assert!(Cli::try_parse_from(["aros", "source", "sync", "--branch", "main"]).is_ok());
    assert_eq!(
        parse_error(&["aros", "source", "sync", "--ref", "main"]),
        ErrorKind::UnknownArgument
    );
    assert!(
        Cli::try_parse_from(["aros", "source", "init", "AROS", "--ref", "refs/heads/main",])
            .is_ok()
    );
}

#[test]
fn clean_requires_one_explicit_scope_and_supports_preview() {
    assert_eq!(
        parse_error(&["aros", "clean"]),
        ErrorKind::MissingRequiredArgument
    );
    assert_eq!(
        parse_error(&["aros", "clean", "--preset", "pc-x86_64", "--all"]),
        ErrorKind::ArgumentConflict
    );
    assert!(Cli::try_parse_from(["aros", "clean", "--preset", "pc-x86_64", "--dry-run",]).is_ok());
    assert!(Cli::try_parse_from(["aros", "clean", "--all", "--dry-run"]).is_ok());
}

#[test]
fn build_job_limits_reject_zero_at_the_cli_boundary() {
    assert_eq!(
        parse_error(&["aros", "build", "--jobs", "0"]),
        ErrorKind::ValueValidation
    );
    assert_eq!(
        parse_error(&["aros", "board", "build", "--jobs", "0"]),
        ErrorKind::ValueValidation
    );
    assert!(Cli::try_parse_from(["aros", "build", "--jobs", "1"]).is_ok());
}

#[test]
fn resource_and_board_mutation_contracts_are_enforced_by_the_parser() {
    for arguments in [
        &["aros", "test", "--timeout", "0"][..],
        &["aros", "test", "--memory", "0"][..],
        &[
            "aros",
            "board",
            "console",
            "--profile",
            "rpi5",
            "--baud",
            "0",
        ][..],
    ] {
        assert_eq!(
            parse_error(arguments),
            ErrorKind::ValueValidation,
            "{arguments:?}"
        );
    }
    for arguments in [
        &[
            "aros",
            "board",
            "deploy",
            "--profile",
            "rpi5",
            "--apply",
            "--dry-run",
        ][..],
        &[
            "aros",
            "board",
            "sd",
            "image",
            "--profile",
            "rpi5",
            "--boot-bundle",
            "/bundle",
            "--output",
            "/output",
            "--apply",
            "--dry-run",
        ][..],
    ] {
        assert_eq!(
            parse_error(arguments),
            ErrorKind::ArgumentConflict,
            "{arguments:?}"
        );
    }
    assert!(Cli::try_parse_from(["aros", "test", "--timeout", "1", "--memory", "1",]).is_ok());
}

#[test]
fn iso_boot_flags_require_iso_and_conflict_with_multiboot_modules() {
    assert_eq!(
        parse_error(&["aros", "test", "--require-llvmpipe-jit"]),
        ErrorKind::MissingRequiredArgument
    );
    assert_eq!(
        parse_error(&["aros", "test", "--iso", "/tmp/boot.iso", "--packages"]),
        ErrorKind::ArgumentConflict
    );
    assert_eq!(
        parse_error(&[
            "aros",
            "test",
            "--iso",
            "/tmp/boot.iso",
            "--module",
            "/tmp/module",
        ]),
        ErrorKind::ArgumentConflict
    );
    assert!(Cli::try_parse_from([
        "aros",
        "test",
        "--iso",
        "/tmp/boot.iso",
        "--require-llvmpipe-jit",
    ])
    .is_ok());
    assert!(
        Cli::try_parse_from(["aros", "test", "--packages", "--module", "/tmp/extra.pkg",]).is_ok()
    );
}

#[test]
fn board_init_requires_a_typed_model_and_never_inferrs_one_from_the_profile_label() {
    assert_eq!(
        parse_error(&["aros", "board", "init", "--profile", "pi5-usb"]),
        ErrorKind::MissingRequiredArgument
    );
    assert_eq!(
        parse_error(&["aros", "board", "init", "--board", "pi5-usb", "--model", "rpi5",]),
        ErrorKind::UnknownArgument
    );
    assert_eq!(
        parse_error(&[
            "aros",
            "board",
            "init",
            "--profile",
            "future",
            "--model",
            "future-board",
        ]),
        ErrorKind::InvalidValue
    );

    let parsed = Cli::try_parse_from([
        "aros",
        "board",
        "init",
        "--profile",
        "pi5-usb",
        "--model",
        "rpi3",
        "--transport",
        "native-tftp",
    ])
    .expect("explicit model and transport parse");
    let Commands::Board {
        command:
            BoardCommand::Init {
                profile,
                model,
                transport,
                ..
            },
    } = parsed.command
    else {
        panic!("expected board init command");
    };
    assert_eq!(profile, "pi5-usb");
    assert_eq!(model.as_str(), "rpi3");
    assert_eq!(transport, Some(BoardInitTransport::NativeTftp));
    assert!(Cli::try_parse_from([
        "aros",
        "board",
        "init",
        "--profile",
        "titan",
        "--model",
        "milk-v-titan",
    ])
    .is_ok());
    assert!(Cli::try_parse_from(["aros", "board", "doctor", "--profile", "pi5-usb"]).is_ok());
}

#[test]
fn board_init_model_contract_covers_each_reviewed_default_transport() {
    let registry = embedded_board_registry().expect("embedded board catalog is valid");
    for contract in registry.boards() {
        let model_argument = contract.id().as_str();
        let parsed = Cli::try_parse_from([
            "aros",
            "board",
            "init",
            "--profile",
            "deliberately-unrelated-label",
            "--model",
            model_argument,
        ])
        .expect("reviewed model must parse independently of the profile label");
        let Commands::Board {
            command: BoardCommand::Init {
                model, transport, ..
            },
        } = parsed.command
        else {
            panic!("expected board init command");
        };
        assert_eq!(model.as_str(), model_argument);
        assert_eq!(transport, None);
        let template = crate::board::config::prepare_template(
            Some(std::path::Path::new("boards.toml")),
            "deliberately-unrelated-label",
            model,
            None,
        )
        .expect("catalog model has an implemented default transport");
        assert_eq!(
            template.transport().to_string(),
            contract.default_transport()
        );
    }
}

#[test]
fn board_model_possible_values_come_from_the_embedded_registry() {
    let mut command = Cli::command();
    command.build();
    let mut values = command
        .find_subcommand("board")
        .expect("board command")
        .find_subcommand("init")
        .expect("board init command")
        .get_arguments()
        .find(|argument| argument.get_id() == "model")
        .expect("model argument")
        .get_possible_values()
        .into_iter()
        .map(|value| value.get_name().to_owned())
        .collect::<Vec<_>>();
    let registry = embedded_board_registry().expect("embedded board catalog is valid");
    let mut catalog_values = registry
        .boards()
        .map(|contract| contract.id().as_str().to_owned())
        .collect::<Vec<_>>();
    values.sort_unstable();
    catalog_values.sort_unstable();
    assert_eq!(values, catalog_values);
}

#[test]
fn toolchain_inventory_budget_is_bounded_at_the_cli_boundary() {
    assert_eq!(
        parse_error(&["aros", "toolchain", "inventory", "--max-entries", "0"]),
        ErrorKind::ValueValidation
    );
    assert_eq!(
        parse_error(&["aros", "toolchain", "inventory", "--max-entries", "100001"]),
        ErrorKind::ValueValidation
    );
    assert!(Cli::try_parse_from(["aros", "toolchain", "inventory", "--max-entries", "1"]).is_ok());
}

#[test]
fn native_producer_surface_has_one_prepared_input_mode_and_structural_package_output() {
    let native_roots = [
        "--preset",
        "pc-x86_64",
        "--recipe",
        "/recipe.json",
        "--source-dir",
        "/source",
        "--producer-dir",
        "/producer",
        "--tools-dir",
        "/tools",
    ];
    for command in ["plan", "build"] {
        let mut arguments = vec!["aros", "toolchain", command];
        arguments.extend(native_roots);
        arguments.push("--offline");
        assert_eq!(parse_error(&arguments), ErrorKind::UnknownArgument);
    }

    let package_context = [
        "--recipe",
        "/recipe.json",
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
    ];
    let mut package = vec!["aros", "toolchain", "producer", "package"];
    package.extend(package_context);
    package.extend(["--input-dir", "/candidate/toolchain"]);
    assert_eq!(
        parse_error(&package),
        ErrorKind::MissingRequiredArgument,
        "package cannot silently choose an output root"
    );

    let mut verify = vec!["aros", "toolchain", "producer", "verify-package"];
    verify.extend(package_context);
    verify.extend([
        "--input-dir",
        "/candidate/toolchain",
        "--output-dir",
        "/packages/out",
    ]);
    assert_eq!(
        parse_error(&verify),
        ErrorKind::UnknownArgument,
        "verification must not accept a packaging output option"
    );
}
