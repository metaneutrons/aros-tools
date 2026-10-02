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
    options: Vec<CompletionOption>,
}

#[derive(Debug)]
struct CompletionOption {
    names: Vec<String>,
    values: Vec<String>,
}

fn render(shell: CompletionShell, command: &Command) -> String {
    let mut command = command.clone();
    command.build();
    let mut nodes = Vec::new();
    collect_public_nodes(&command, "root", &mut nodes);
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
    let options = command
        .get_arguments()
        .filter(|argument| !argument.is_hide_set() && !argument.is_positional())
        .filter(|argument| argument.get_action().takes_values())
        .map(|argument| {
            let mut names = Vec::new();
            if let Some(long) = argument.get_long() {
                names.push(format!("--{long}"));
            }
            if let Some(short) = argument.get_short() {
                names.push(format!("-{short}"));
            }
            CompletionOption {
                names,
                values: argument
                    .get_possible_values()
                    .into_iter()
                    .filter(|value| !value.is_hide_set())
                    .map(|value| value.get_name().to_owned())
                    .collect(),
            }
        })
        .collect();
    nodes.push(CompletionNode {
        path: path.to_owned(),
        candidates,
        children: child_paths,
        options,
    });
    for (name, child) in children {
        collect_public_nodes(child, &format!("{path}/{name}"), nodes);
    }
}

fn render_bash(nodes: &[CompletionNode]) -> String {
    let mut output = String::from(
        "# bash completion for aros; generated from the public Clap command model.\n\n_aros() {\n    local cur path word candidates option= prefix= match\n    local index\n    cur=\"${COMP_WORDS[COMP_CWORD]}\"\n    path=root\n    for (( index = 1; index < COMP_CWORD; index++ )); do\n        word=\"${COMP_WORDS[index]}\"\n        if [[ -n \"$option\" ]]; then\n            if [[ \"$word\" == = ]]; then\n                continue\n            fi\n            option=\n            continue\n        fi\n        case \"${path}:${word}\" in\n",
    );
    render_option_transitions(&mut output, nodes);
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
    output.push_str("        esac\n    done\n    if [[ \"$cur\" == --*=* ]]; then\n        option=\"${cur%%=*}\"\n        prefix=\"${option}=\"\n        cur=\"${cur#*=}\"\n    elif [[ -n \"$option\" && \"$cur\" == = ]]; then\n        cur=\n    fi\n    if [[ -n \"$option\" ]]; then\n        case \"${path}:${option}\" in\n");
    render_option_values(&mut output, nodes, false);
    output.push_str("        esac\n    else\n        case \"${path}\" in\n");
    for node in nodes {
        writeln!(
            output,
            "        {}) candidates=\"{}\" ;;",
            node.path,
            node.candidates.join(" ")
        )
        .expect("string write");
    }
    output.push_str("        esac\n    fi\n    COMPREPLY=()\n    while IFS= read -r match; do\n        COMPREPLY+=(\"${prefix}${match}\")\n    done < <(compgen -W \"${candidates}\" -- \"${cur}\")\n}\ncomplete -F _aros aros\n");
    output
}

fn render_zsh(nodes: &[CompletionNode]) -> String {
    let mut output = String::from(
        "#compdef aros\n# zsh completion for aros; generated from the public Clap command model.\n\n_aros() {\n    local path=root word option= cur=\"${words[CURRENT]}\"\n    local -a candidates\n    integer index\n    for (( index = 2; index < CURRENT; index++ )); do\n        word=\"${words[index]}\"\n        if [[ -n \"$option\" ]]; then\n            option=\n            continue\n        fi\n        case \"${path}:${word}\" in\n",
    );
    render_option_transitions(&mut output, nodes);
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
    output.push_str("        esac\n    done\n    if [[ \"$cur\" == --*=* ]]; then\n        option=\"${cur%%=*}\"\n        compset -P '*='\n    fi\n    if [[ -n \"$option\" ]]; then\n        case \"${path}:${option}\" in\n");
    render_option_values(&mut output, nodes, true);
    output.push_str("        esac\n    else\n        case \"${path}\" in\n");
    for node in nodes {
        writeln!(
            output,
            "        {}) candidates=({}) ;;",
            node.path,
            node.candidates.join(" ")
        )
        .expect("string write");
    }
    output.push_str("        esac\n    fi\n    compadd -- $candidates\n}\n\n_aros \"$@\"\n");
    output
}

fn render_fish(nodes: &[CompletionNode]) -> String {
    let mut output = String::from(
        "# fish completion for aros; generated from the public Clap command model.\nfunction __aros_public_context\n    set -l path root\n    set -l option ''\n    set -l words (commandline -opc)\n    for word in $words[2..-1]\n        if test -n \"$option\"\n            set option ''\n            continue\n        end\n        switch \"$path:$word\"\n",
    );
    for node in nodes {
        for option in &node.options {
            for name in &option.names {
                writeln!(output, "            case '{}:{}'\n                set option '{}'\n            case '{}:{}=*'\n                continue", node.path, name, name, node.path, name)
                    .expect("string write");
            }
        }
    }
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
    output.push_str("        end\n    end\n    echo \"$path:$option\"\nend\n\nfunction __aros_public_path\n    set -l context (string split -m 1 ':' (__aros_public_context))\n    echo $context[1]\nend\n\nfunction __aros_expects_value\n    string match -q '*:-*' (__aros_public_context)\nend\n\n");
    for node in nodes {
        writeln!(
            output,
            "complete -c aros -f -n 'test (__aros_public_path) = {}; and not __aros_expects_value' -a '{}'",
            node.path,
            node.candidates.join(" ")
        )
        .expect("string write");
        for option in &node.options {
            for name in &option.names {
                let (kind, name) = name
                    .strip_prefix("--")
                    .map_or_else(|| ("s", &name[1..]), |long| ("l", long));
                writeln!(output, "complete -c aros -f -n 'test (__aros_public_path) = {}' -{kind} {name} -r -a '{}'", node.path, option.values.join(" "))
                    .expect("string write");
            }
        }
    }
    output
}

/// Option values are consumed before command traversal so an operator's local
/// alias cannot accidentally select a similarly named subcommand.
fn render_option_transitions(output: &mut String, nodes: &[CompletionNode]) {
    for node in nodes {
        for option in &node.options {
            for name in &option.names {
                writeln!(
                    output,
                    "            {}:{name}) option={name} ;;\n            {}:{name}=*) ;;",
                    node.path, node.path
                )
                .expect("string write");
            }
        }
    }
}

fn render_option_values(output: &mut String, nodes: &[CompletionNode], zsh: bool) {
    for node in nodes {
        for option in &node.options {
            let values = option.values.join(" ");
            for name in &option.names {
                let assignment = if zsh {
                    format!("({values})")
                } else {
                    format!("\"{values}\"")
                };
                writeln!(
                    output,
                    "            {}:{name}) candidates={assignment} ;;",
                    node.path
                )
                .expect("string write");
            }
        }
    }
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
