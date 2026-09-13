//! Source-derived rendering for reviewed public CLI contract snapshots.

use std::fmt::Write;

use clap::{Arg, Command};

/// Escape one generated Markdown table cell.
pub fn table_cell(value: impl AsRef<str>) -> String {
    value.as_ref().replace('|', "\\|")
}

/// Return public root command families in stable display order.
#[must_use]
pub fn public_contract_commands(command: &Command) -> Vec<&Command> {
    let mut commands = command
        .get_subcommands()
        .filter(|child| is_public_contract_command(child))
        .collect::<Vec<_>>();
    commands.sort_by_key(|child| child.get_name());
    commands
}

/// Render the root CLI contract page from the Clap model.
#[must_use]
pub fn rendered_cli_contract_index(command: &Command) -> String {
    let mut sections = String::new();
    for child in public_contract_commands(command) {
        writeln!(
            sections,
            "- [\x60aros {}\x60](/aros-tools/reference/cli-contract/{}/)",
            child.get_name(),
            child.get_name(),
        )
        .expect("writing to a string cannot fail");
    }
    let mut document = String::from(
        "---\ntitle: Generated CLI contract\ndescription: Source-derived structural facts for the current public aros command model.\n---\n\nThis reference is generated from the \x60aros\x60 Clap command model and committed for review. It records visible commands and structural argument facts; task semantics, side effects, and recovery remain in the [command reference](/aros-tools/reference/cli/). Hidden lifecycle bridges are deliberately excluded.\n\nThe \x60position\x60 column is one-based for positional arguments and \x60—\x60 for options. \x60constraint\x60 records individual and group parser rules. Empty \x60default\x60, \x60values\x60, \x60environment\x60, and \x60conflicts\x60 cells mean that Clap declares none.\n\n## Global arguments\n\n| ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for argument in visible_arguments(command)
        .into_iter()
        .filter(|argument| argument.is_global_set())
    {
        writeln!(document, "| {} |", argument_contract(command, argument))
            .expect("writing to a string cannot fail");
    }
    document.push_str("\n## Command sections\n\n");
    document.push_str(&sections);
    document
}

/// Render one root command-family contract page from the Clap model.
#[must_use]
pub fn rendered_cli_contract_section(command: &Command) -> String {
    let mut document = format!(
        "---\ntitle: \"Generated CLI contract: {}\"\ndescription: Source-derived structural facts for the public aros {} command family.\n---\n\nThis page is generated from the \x60aros\x60 Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/). The \x60constraint\x60 column includes required exclusive groups.\n\n| Command | ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
        command.get_name(),
        command.get_name()
    );
    collect_command_contract(
        command,
        &["aros".to_owned(), command.get_name().to_owned()],
        &mut document,
    );
    document
}

fn is_public_contract_argument(argument: &Arg) -> bool {
    !argument.is_hide_set() && !matches!(argument.get_id().as_str(), "help" | "version")
}

fn is_public_contract_command(command: &Command) -> bool {
    !command.is_hide_set() && command.get_name() != "help"
}

fn visible_arguments(command: &Command) -> Vec<&Arg> {
    command
        .get_arguments()
        .filter(|argument| is_public_contract_argument(argument))
        .collect()
}

fn argument_contract(command: &Command, argument: &Arg) -> String {
    let spelling = match (argument.get_short(), argument.get_long()) {
        (Some(short), Some(long)) => format!("-{short}, --{long}"),
        (Some(short), None) => format!("-{short}"),
        (None, Some(long)) => format!("--{long}"),
        (None, None) => argument.get_id().to_string(),
    };
    let conflicts = command
        .get_arg_conflicts_with(argument)
        .into_iter()
        .filter(|candidate| is_public_contract_argument(candidate))
        .map(|candidate| candidate.get_id().to_string())
        .collect::<Vec<_>>();
    let values = argument
        .get_possible_values()
        .into_iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| value.get_name().to_owned())
        .collect::<Vec<_>>();

    [
        argument.get_id().to_string(),
        spelling,
        argument
            .get_index()
            .map_or_else(|| "—".to_owned(), |index| index.to_string()),
        argument_constraint(command, argument),
        argument
            .get_num_args()
            .map_or_else(|| "—".to_owned(), |range| range.to_string()),
        argument
            .get_default_values()
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(", "),
        values.join(", "),
        argument.get_env().map_or_else(
            || "—".to_owned(),
            |value| value.to_string_lossy().into_owned(),
        ),
        conflicts.join(", "),
    ]
    .into_iter()
    .map(table_cell)
    .collect::<Vec<_>>()
    .join(" | ")
}

fn argument_constraint(command: &Command, argument: &Arg) -> String {
    if argument.is_required_set() {
        return "required".to_owned();
    }
    let constraints = command
        .get_groups()
        .filter(|group| group.get_args().any(|id| id == argument.get_id()))
        .map(|group| {
            let mut group = group.clone();
            match (group.is_required_set(), group.is_multiple()) {
                (true, false) => format!("exactly one of {}", group.get_id()),
                (true, true) => format!("one or more of {}", group.get_id()),
                (false, false) => format!("at most one of {}", group.get_id()),
                (false, true) => "optional".to_owned(),
            }
        })
        .filter(|constraint| constraint != "optional")
        .collect::<Vec<_>>();
    if constraints.is_empty() {
        "optional".to_owned()
    } else {
        constraints.join("; ")
    }
}

fn collect_command_contract(command: &Command, path: &[String], document: &mut String) {
    if !is_public_contract_command(command) {
        return;
    }

    let mut children = command
        .get_subcommands()
        .filter(|child| is_public_contract_command(child))
        .collect::<Vec<_>>();
    children.sort_by_key(|child| child.get_name());

    if children.is_empty() {
        for argument in visible_arguments(command)
            .into_iter()
            .filter(|argument| !argument.is_global_set())
        {
            writeln!(
                document,
                "| \x60{}\x60 | {} |",
                path.join(" "),
                argument_contract(command, argument),
            )
            .expect("writing to a string cannot fail");
        }
    }

    for child in children {
        let mut child_path = path.to_vec();
        child_path.push(child.get_name().to_owned());
        collect_command_contract(child, &child_path, document);
    }
}
