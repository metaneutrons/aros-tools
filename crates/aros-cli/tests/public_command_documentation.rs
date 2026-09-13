//! Keep the visible `aros` command tree and its public reference in lockstep.

use std::collections::BTreeSet;
use std::process::Command;

const CLI_REFERENCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs-site/src/content/docs/reference/cli.md"
));
const DIAGNOSTICS_REFERENCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs-site/src/content/docs/reference/diagnostics.md"
));

const fn aros() -> &'static str {
    env!("CARGO_BIN_EXE_aros")
}

fn help(arguments: &[String]) -> String {
    let output = Command::new(aros())
        .args(arguments)
        .arg("--help")
        .output()
        .expect("public command help must execute");
    assert!(
        output.status.success(),
        "help for `aros {}` failed: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("public command help must be UTF-8")
}

fn subcommands(help: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut in_commands = false;
    for line in help.lines() {
        if line == "Commands:" {
            in_commands = true;
            continue;
        }
        if in_commands && line.is_empty() {
            break;
        }
        if !in_commands {
            continue;
        }
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        if name != "help" {
            commands.push(name.to_owned());
        }
    }
    commands
}

fn collect_leaves(arguments: &[String], leaves: &mut BTreeSet<String>) {
    let children = subcommands(&help(arguments));
    if children.is_empty() {
        assert!(
            !arguments.is_empty(),
            "the root command unexpectedly has no public subcommands"
        );
        leaves.insert(arguments.join(" "));
        return;
    }
    for child in children {
        let mut nested = arguments.to_owned();
        nested.push(child);
        collect_leaves(&nested, leaves);
    }
}

#[test]
fn public_cli_reference_covers_every_visible_leaf_command() {
    let mut leaves = BTreeSet::new();
    collect_leaves(&[], &mut leaves);

    assert!(
        !leaves.is_empty(),
        "the root command must expose at least one visible public leaf command"
    );
    assert!(
        CLI_REFERENCE.contains("/aros-tools/reference/cli-contract/"),
        "the public command reference must link to the source-derived CLI contract"
    );

    let missing = leaves
        .iter()
        .filter(|command| !CLI_REFERENCE.contains(&format!("`aros {command}`")))
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "visible public commands absent from docs-site/src/content/docs/reference/cli.md: {missing:?}"
    );
}

#[test]
fn public_reference_preserves_the_current_board_native_lifecycle_and_installation_boundaries() {
    assert!(
        CLI_REFERENCE.contains("typed model-specific profile template"),
        "board init must describe the selected model rather than a legacy fixed template"
    );
    assert!(
        !CLI_REFERENCE.contains("Pi-4 USB-ECM profile template"),
        "board init must not promise a fixed Pi-4 USB-ECM template"
    );
    assert!(
        CLI_REFERENCE.contains("`--resume-from compiler`"),
        "the documented native lifecycle must expose its compiler-only resume boundary"
    );
    assert!(
        CLI_REFERENCE.contains("checks their inventory, modes, sizes, and snapshotted bytes"),
        "the installer reference must state its actual verification boundary"
    );
    assert!(
        DIAGNOSTICS_REFERENCE.contains("`aros toolchain build` also exposes `AX0801`"),
        "the diagnostics reference must connect AX0801 to the public native build command"
    );
}
