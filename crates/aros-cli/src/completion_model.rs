//! Pure shell-completion generation from the public Clap command model.

use super::{observability, Cli};
use aros_common::{
    render_diagnostics, Diagnostic, DiagnosticCode, DiagnosticFormat, DiagnosticSet,
    DiagnosticStage,
};
use clap::{Command, CommandFactory, ValueEnum};
use miette::Result;
use std::fmt::Write as _;
use std::process::ExitCode;

/// Completion syntaxes intentionally supported by the public frontend.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CompletionShell {
    /// Bash completion syntax.
    Bash,
    /// Zsh completion syntax.
    Zsh,
    /// Fish completion syntax.
    Fish,
}

/// Write a completion script without resolving a checkout or opening a log.
pub fn write(shell: CompletionShell) -> Result<()> {
    let generated = render(shell, &Cli::command());
    aros_common::write_stdout(&generated)
        .map_err(|error| miette::miette!("could not write completion script: {error}"))
}

#[derive(Debug)]
struct CompletionNode {
    path: String,
    candidates: Vec<String>,
    children: Vec<(String, String)>,
}

fn render(shell: CompletionShell, command: &Command) -> String {
    let mut nodes = Vec::new();
    collect_public_nodes(command, "root", &mut nodes);
    match shell {
        CompletionShell::Bash => render_bash(&nodes),
        CompletionShell::Zsh => render_zsh(&nodes),
        CompletionShell::Fish => render_fish(&nodes),
    }
}

fn collect_public_nodes(command: &Command, path: &str, nodes: &mut Vec<CompletionNode>) {
    let children = command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
        .map(|child| (child.get_name().to_owned(), child))
        .collect::<Vec<_>>();
    let mut candidates = command
        .get_arguments()
        .filter(|argument| !argument.is_hide_set())
        .flat_map(|argument| {
            let mut names = Vec::new();
            if let Some(long) = argument.get_long() {
                names.push(format!("--{long}"));
            }
            if let Some(short) = argument.get_short() {
                names.push(format!("-{short}"));
            }
            if argument.is_positional() {
                names.extend(
                    argument
                        .get_possible_values()
                        .into_iter()
                        .filter(|value| !value.is_hide_set())
                        .map(|value| value.get_name().to_owned()),
                );
            }
            names
        })
        .collect::<Vec<_>>();
    candidates.extend(children.iter().map(|(name, _)| name.clone()));
    candidates.sort_unstable();
    candidates.dedup();

    let child_paths = children
        .iter()
        .map(|(name, _)| (name.clone(), format!("{path}/{name}")))
        .collect::<Vec<_>>();
    nodes.push(CompletionNode {
        path: path.to_owned(),
        candidates,
        children: child_paths,
    });
    for (name, child) in children {
        collect_public_nodes(child, &format!("{path}/{name}"), nodes);
    }
}

fn render_bash(nodes: &[CompletionNode]) -> String {
    let mut output = String::from(
        "# bash completion for aros; generated from the public Clap command model.\n\n_aros() {\n    local cur path word candidates\n    cur=\"${COMP_WORDS[COMP_CWORD]}\"\n    path=root\n    for word in \"${COMP_WORDS[@]:1:COMP_CWORD}\"; do\n        case \"${path}:${word}\" in\n",
    );
    for node in nodes {
        for (name, child_path) in &node.children {
            writeln!(
                output,
                "            {}:{}) path={} ;;",
                node.path, name, child_path
            )
            .expect("string write");
        }
    }
    output.push_str("        esac\n    done\n    case \"${path}\" in\n");
    for node in nodes {
        writeln!(
            output,
            "        {}) candidates=\"{}\" ;;",
            node.path,
            node.candidates.join(" ")
        )
        .expect("string write");
    }
    output.push_str("    esac\n    COMPREPLY=( $(compgen -W \"${candidates}\" -- \"${cur}\") )\n}\ncomplete -F _aros aros\n");
    output
}

fn render_zsh(nodes: &[CompletionNode]) -> String {
    let mut output = String::from(
        "#compdef aros\n# zsh completion for aros; generated from the public Clap command model.\n\n_aros() {\n    local path=root word\n    local -a candidates\n    integer index\n    for (( index = 2; index < CURRENT; index++ )); do\n        word=\"${words[index]}\"\n        case \"${path}:${word}\" in\n",
    );
    for node in nodes {
        for (name, child_path) in &node.children {
            writeln!(
                output,
                "            {}:{}) path={} ;;",
                node.path, name, child_path
            )
            .expect("string write");
        }
    }
    output.push_str("        esac\n    done\n    case \"${path}\" in\n");
    for node in nodes {
        writeln!(
            output,
            "        {}) candidates=({}) ;;",
            node.path,
            node.candidates.join(" ")
        )
        .expect("string write");
    }
    output.push_str("    esac\n    compadd -- $candidates\n}\n\n_aros \"$@\"\n");
    output
}

fn render_fish(nodes: &[CompletionNode]) -> String {
    let mut output = String::from(
        "# fish completion for aros; generated from the public Clap command model.\nfunction __aros_public_path\n    set -l path root\n    set -l words (commandline -opc)\n    for word in $words[2..-1]\n        switch \"$path:$word\"\n",
    );
    for node in nodes {
        for (name, child_path) in &node.children {
            writeln!(
                output,
                "            case {}:{}\n                set path {}",
                node.path, name, child_path
            )
            .expect("string write");
        }
    }
    output.push_str("        end\n    end\n    echo $path\nend\n\n");
    for node in nodes {
        writeln!(
            output,
            "complete -c aros -f -n 'test (__aros_public_path) = {}' -a '{}'",
            node.path,
            node.candidates.join(" ")
        )
        .expect("string write");
    }
    output
}

/// Emit one completion script and render a stable diagnostic only if stdout fails.
pub fn emit(shell: CompletionShell, format: DiagnosticFormat) -> ExitCode {
    match write(shell) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            render_diagnostics(
                &DiagnosticSet::single(
                    Diagnostic::error(
                        DiagnosticCode::CliObservability,
                        DiagnosticStage::Observability,
                        format!("could not write shell completion script: {error}"),
                    )
                    .with_hint("check the stdout destination and retry"),
                ),
                format,
                observability::POLICY,
            );
            ExitCode::FAILURE
        }
    }
}
