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
        "---\ntitle: \"Generated CLI contract: {}\"\ndescription: Source-derived structural facts for the public aros {} command family.\n---\n\nThis page is generated from the \x60aros\x60 Clap command model. Global arguments are listed on the [contract index](/aros-tools/reference/cli-contract/). The \x60constraint\x60 column includes required exclusive groups.\n\n| Command | ID | Spelling | Position | Constraint | Arity | Default | Values | Environment | Conflicts |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
        command.get_name(),
        command.get_name()
    );
    collect_command_contract(
        command,
        &["aros".to_owned(), command.get_name().to_owned()],
        &mut document,
    );
    let mut notes = String::new();
    collect_command_notes(
        command,
        &["aros".to_owned(), command.get_name().to_owned()],
        &mut notes,
    );
    if !notes.is_empty() {
        document.push_str("\n## Command notes\n\n");
        document.push_str(notes.trim_end());
        document.push('\n');
    }
    document
}

/// Preserve source-declared help for conditional parser rules that Clap does
/// not expose through its stable argument introspection API.
fn collect_command_notes(command: &Command, path: &[String], notes: &mut String) {
    if !is_public_contract_command(command) {
        return;
    }
    if let Some(help) = command.get_after_help() {
        writeln!(notes, "### `{}`\n\n{help}\n", path.join(" "))
            .expect("writing to a string cannot fail");
    }
    let mut children = command.get_subcommands().collect::<Vec<_>>();
    children.sort_by_key(|child| child.get_name());
    for child in children {
        let mut child_path = path.to_vec();
        child_path.push(child.get_name().to_owned());
        collect_command_notes(child, &child_path, notes);
    }
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
    let spelling = markdown_code_span(&spelling);
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

/// Render exact CLI spelling as a Markdown code span, even when it contains
/// backticks or leading and trailing spaces.
fn markdown_code_span(value: &str) -> String {
    let mut longest_backtick_run = 0;
    let mut backtick_run = 0;
    for character in value.chars() {
        if character == '`' {
            backtick_run += 1;
            longest_backtick_run = longest_backtick_run.max(backtick_run);
        } else {
            backtick_run = 0;
        }
    }

    let delimiter = "`".repeat(longest_backtick_run + 1);
    let needs_padding = value.starts_with('`')
        || value.ends_with('`')
        || (value.starts_with(' ')
            && value.ends_with(' ')
            && value.chars().any(|character| character != ' '));
    if needs_padding {
        format!("{delimiter} {value} {delimiter}")
    } else {
        format!("{delimiter}{value}{delimiter}")
    }
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

#[cfg(test)]
mod tests {
    use super::{markdown_code_span, rendered_cli_contract_index, rendered_cli_contract_section};
    use clap::{Arg, Command};

    fn table_cell_count(line: &str) -> usize {
        let mut separator_count = 0_usize;
        let mut escaped = false;
        for character in line.chars() {
            if character == '|' && !escaped {
                separator_count += 1;
            }
            if character == '\\' {
                escaped = !escaped;
            } else {
                escaped = false;
            }
        }
        separator_count.saturating_sub(1)
    }

    fn assert_table_dimensions(rendered: &str, columns: usize, data_rows: usize) {
        let table_lines = rendered
            .lines()
            .filter(|line| line.starts_with('|'))
            .collect::<Vec<_>>();
        assert_eq!(table_lines.len(), data_rows + 2);
        for line in table_lines {
            assert_eq!(table_cell_count(line), columns, "invalid table row: {line}");
        }
    }

    #[test]
    fn command_notes_preserve_declared_conditional_help_in_sorted_order() {
        let mut command = Command::new("toolchain")
            .subcommand(Command::new("z-last").after_help("Last note."))
            .subcommand(Command::new("a-first").after_help("Both identity halves are required."));
        command.build();
        let rendered = rendered_cli_contract_section(&command);
        assert!(rendered.contains("## Command notes\n\n"));
        assert!(rendered
            .contains("### `aros toolchain a-first`\n\nBoth identity halves are required.\n"));
        assert!(
            rendered.find("aros toolchain a-first").unwrap()
                < rendered.find("aros toolchain z-last").unwrap()
        );
    }

    #[test]
    fn command_notes_exclude_hidden_commands_and_their_descendants() {
        let mut command = Command::new("toolchain").subcommand(
            Command::new("internal")
                .hide(true)
                .after_help("Private parent note.")
                .subcommand(Command::new("child").after_help("Private descendant note.")),
        );
        command.build();
        let rendered = rendered_cli_contract_section(&command);
        assert!(!rendered.contains("Command notes"));
        assert!(!rendered.contains("Private"));
    }

    #[test]
    fn global_and_section_tables_have_valid_column_counts_and_exact_spellings() {
        let mut command = Command::new("toolchain")
            .arg(Arg::new("global-flag").long("flag").global(true))
            .subcommand(
                Command::new("inspect")
                    .arg(Arg::new("local-flag").long("local"))
                    .arg(Arg::new("path").index(1)),
            );
        command.build();

        let index = rendered_cli_contract_index(&command);
        assert_table_dimensions(&index, 9, 1);
        assert!(index.contains("| `--flag` |"));

        let section = rendered_cli_contract_section(&command);
        assert_table_dimensions(&section, 10, 2);
        assert!(section.contains("| `--local` |"));
        assert!(section.contains("| `path` |"));
    }

    #[test]
    fn code_spans_choose_a_delimiter_that_preserves_embedded_backticks() {
        assert_eq!(markdown_code_span("a`b"), "``a`b``");
        assert_eq!(markdown_code_span("a``b`c"), "```a``b`c```");
        assert_eq!(markdown_code_span("`value`"), "`` `value` ``");
        assert_eq!(markdown_code_span("value`"), "`` value` ``");
        assert_eq!(markdown_code_span("  value  "), "`   value   `");
    }
}
