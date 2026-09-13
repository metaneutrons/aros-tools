//! Keep the visible `aros` command tree and its public reference in lockstep.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CLI_REFERENCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs-site/src/content/docs/reference/cli.md"
));
const DIAGNOSTICS_REFERENCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs-site/src/content/docs/reference/diagnostics.md"
));

/// Every public Astro page that currently contains an executable `aros` shell
/// example and the focused fixture suite that owns its semantics. The test
/// below discovers examples from the pages themselves, so adding a new page
/// with an `aros` command requires an intentional semantic-owner entry here.
const DOCUMENTED_AROS_EXAMPLE_PAGES: &[(&str, &str)] = &[
    (
        "getting-started/installation.md",
        "discoverability_cli.rs::completions_are_deterministic_public_and_side_effect_free",
    ),
    (
        "getting-started/quick-start.md",
        "source_cli.rs, public_cli_semantics.rs, discoverability_cli.rs",
    ),
    (
        "reference/cli.md",
        "public_cli_semantics.rs, toolchain_plan_cli.rs, observability_cli.rs",
    ),
    (
        "reference/diagnostics.md",
        "observability_cli.rs::invalid_invocation_is_one_versioned_json_diagnostic",
    ),
    (
        "reference/troubleshooting.md",
        "observability_cli.rs::command_failure_and_local_jsonl_log_are_structured_and_separate",
    ),
    (
        "workflows/aros-nx.md",
        "source_cli.rs, public_cli_semantics.rs, observability_cli.rs",
    ),
    (
        "workflows/boards.md",
        "public_cli_semantics.rs::public_board_init_semantic_cases_cover_models_defaults_and_environment",
    ),
    (
        "workflows/cache.md",
        "discoverability_cli.rs::cache_status_is_passive_versioned_and_never_starts_a_backend",
    ),
    (
        "workflows/cross-development.md",
        "public_cli_semantics.rs::relative_engine_override_stays_at_the_invocation_directory",
    ),
    (
        "workflows/source.md",
        "source_cli.rs::source_commands_have_one_canonical_surface_without_aliases",
    ),
    (
        "workflows/toolchain-producer.md",
        "toolchain_plan_cli.rs::native_plan_binds_its_declared_contract_without_mutation",
    ),
    (
        "workflows/toolchains.md",
        "toolchain_management_cli.rs, toolchain_selection_cli.rs, discoverability_cli.rs",
    ),
    (
        "workflows/upstream-aros.md",
        "source_cli.rs, toolchain_management_cli.rs",
    ),
];

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

fn documentation_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs-site/src/content/docs")
}

fn collect_markdown_pages(root: &Path, directory: &Path, pages: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory).unwrap_or_else(|error| {
        panic!(
            "could not read documentation directory {}: {error}",
            directory.display()
        )
    });
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!(
                "could not read a documentation directory entry below {}: {error}",
                directory.display()
            )
        });
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_pages(root, &path, pages);
            continue;
        }
        let extension = path.extension().and_then(|value| value.to_str());
        if matches!(extension, Some("md" | "mdx")) {
            assert!(
                path.strip_prefix(root).is_ok(),
                "documentation page {} escaped its declared root {}",
                path.display(),
                root.display()
            );
            pages.push(path);
        }
    }
}

fn shell_words(input: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;

    for character in input.chars() {
        if escaped {
            word.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            } else {
                word.push(character);
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if character.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(character);
        }
    }

    if escaped {
        return Err("unfinished shell escape".to_owned());
    }
    if quote.is_some() {
        return Err("unfinished shell quote".to_owned());
    }
    if !word.is_empty() {
        words.push(word);
    }
    Ok(words)
}

fn documented_aros_invocations(document: &str) -> Result<Vec<Vec<String>>, String> {
    let lines = document.lines().collect::<Vec<_>>();
    let mut invocations = Vec::new();
    let mut in_fenced_block = false;
    let mut index = 0;

    while let Some(line) = lines.get(index) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fenced_block = !in_fenced_block;
            index += 1;
            continue;
        }
        if !in_fenced_block {
            index += 1;
            continue;
        }

        let is_aros = trimmed.starts_with("aros ") || trimmed == "aros";
        let is_variable_aros = trimmed.starts_with("\"$AROS\" ") || trimmed == "\"$AROS\"";
        if !is_aros && !is_variable_aros {
            index += 1;
            continue;
        }

        let mut command = trimmed.to_owned();
        while command.trim_end().ends_with('\\') {
            command = command
                .trim_end()
                .trim_end_matches('\\')
                .trim_end()
                .to_owned();
            index += 1;
            let continuation = lines
                .get(index)
                .ok_or_else(|| "unfinished continued shell command".to_owned())?;
            command.push(' ');
            command.push_str(continuation.trim());
        }

        let mut arguments = shell_words(&command)?;
        if arguments.first().is_some_and(|program| program == "$AROS") {
            "aros".clone_into(&mut arguments[0]);
        }
        if arguments.first().is_none_or(|program| program != "aros") {
            return Err(format!(
                "did not recognize documented AROS invocation `{command}`"
            ));
        }
        if let Some(redirection) = arguments.iter().position(|argument| {
            argument == ">"
                || argument.starts_with('>')
                || argument.starts_with("1>")
                || argument.starts_with("2>")
        }) {
            arguments.truncate(redirection);
        }
        invocations.push(arguments);
        index += 1;
    }

    if in_fenced_block {
        return Err("unfinished Markdown code fence".to_owned());
    }
    Ok(invocations)
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
    assert!(
        CLI_REFERENCE.contains("older unreleased revisions accepted bare `aros clean`"),
        "the cleanup reference must retain its explicit migration boundary"
    );
}

#[test]
fn published_aros_examples_parse_and_have_a_semantic_fixture_owner() {
    let root = documentation_root();
    let declared_pages = DOCUMENTED_AROS_EXAMPLE_PAGES
        .iter()
        .map(|(page, owner)| ((*page).to_owned(), (*owner).to_owned()))
        .collect::<BTreeMap<_, _>>();
    let mut pages = Vec::new();
    collect_markdown_pages(&root, &root, &mut pages);
    pages.sort();

    let mut observed_pages = BTreeSet::new();
    for page in pages {
        let relative = page
            .strip_prefix(&root)
            .expect("documentation page is below the declared root")
            .to_string_lossy()
            .replace('\\', "/");
        let document = fs::read_to_string(&page).unwrap_or_else(|error| {
            panic!(
                "could not read documentation page {}: {error}",
                page.display()
            )
        });
        let invocations = documented_aros_invocations(&document).unwrap_or_else(|error| {
            panic!("could not parse documented command in {relative}: {error}")
        });
        if invocations.is_empty() {
            continue;
        }
        let owner = declared_pages.get(&relative).unwrap_or_else(|| {
            panic!(
                "public AROS examples in {relative} have no declared semantic fixture owner; add one to DOCUMENTED_AROS_EXAMPLE_PAGES"
            )
        });
        assert!(
            !owner.is_empty(),
            "public AROS examples in {relative} must name their semantic fixture owner"
        );
        observed_pages.insert(relative.clone());

        for invocation in invocations {
            let output = if invocation.len() == 2 && invocation[1] == "--version" {
                Command::new(aros()).arg("--version").output()
            } else {
                Command::new(aros())
                    .args(&invocation[1..])
                    .arg("--help")
                    .output()
            }
            .unwrap_or_else(|error| {
                panic!(
                    "documented command `{} {}` in {relative} could not execute: {error}",
                    invocation[0],
                    invocation[1..].join(" ")
                )
            });
            assert!(
                output.status.success(),
                "documented command `{} {}` in {relative} is not accepted by the built CLI: {}",
                invocation[0],
                invocation[1..].join(" "),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    let declared = declared_pages.keys().cloned().collect::<BTreeSet<_>>();
    assert_eq!(
        observed_pages, declared,
        "the semantic-owner inventory must contain exactly the public pages with executable AROS examples"
    );
}
