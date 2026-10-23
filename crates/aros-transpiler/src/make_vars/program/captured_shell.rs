//! Source shell data over the same descriptor-bound input transaction.

use super::{
    charge_work, retained_cost, MakeProgramPaths, MakeVariableEvent, State, VariableFlavor,
    MAX_STATEMENTS, MAX_STORAGE,
};
use aros_common::{
    make_input_snapshot::MakeInputReader, make_namespace::MakeVariableOrigin, TreeContentEntryKind,
};
use std::cell::{Cell, RefCell};

pub(super) struct EffectSession<'a> {
    pub provider: &'a dyn crate::make_shell_effects::CapturedShellEffects,
    pub next: Cell<usize>,
    pub native_context: Option<&'a aros_common::make_native_observation::MakeObservationBinding>,
}

struct BoundControls {
    shell: String,
    flags: String,
    exports: std::collections::BTreeMap<String, String>,
}

pub(super) fn evaluate(
    command: &str,
    reader: &RefCell<&mut MakeInputReader<'_>>,
    state: &RefCell<State>,
    paths: &MakeProgramPaths<'_>,
    expressions: &crate::make_expr::MakeProgramBudget,
    budgets: (&Cell<usize>, &Cell<usize>),
    effects: Option<&EffectSession<'_>>,
) -> Result<String, String> {
    let (work, wildcard_work) = budgets;
    if state.borrow().coordinate_projection || state.borrow().initial_namespace.is_none() {
        return Err("captured shell data needs measured source state".into());
    }
    let pipeline = crate::make_text_pipeline::evaluate_source_with_programs(command);
    let external_programs = pipeline
        .as_ref()
        .map_or(&[] as &[&str], |(_, names)| *names);
    let controls = validate_bound_policy(
        reader,
        state,
        paths,
        expressions,
        work,
        wildcard_work,
        external_programs,
    )?;
    if let Some(effects) = effects {
        let controls = controls.ok_or("observed shell effects need a bound shell policy")?;
        let export_controls = {
            let data = state.borrow();
            let bytes = data.exports.keys().try_fold(0usize, |bytes, name| {
                bytes
                    .checked_add(name.len() + 1)
                    .ok_or("shell export-control size overflow")
            })?;
            charge_work(work, bytes).map_err(|error| error.to_string())?;
            data.exports
                .iter()
                .map(|(name, selection)| {
                    selection
                        .map(|selected| (name.clone(), selected))
                        .ok_or("observed shell export control is unresolved")
                })
                .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?
        };
        if command.contains('\0')
            || controls
                .exports
                .iter()
                .any(|(name, value)| name.contains(['\0', '=']) || value.contains('\0'))
        {
            return Err("observed shell effect has an OS argument delimiter".into());
        }
        let ordinal = effects.next.get();
        if ordinal >= MAX_STATEMENTS {
            return Err("observed shell effect count exceeds its limit".into());
        }
        let result = effects
            .provider
            .query(&crate::make_shell_effects::ShellEffectRequest {
                native_context: effects.native_context,
                ordinal,
                command,
                working_directory: paths.working_directory,
                shell: &controls.shell,
                shell_flags: &controls.flags,
                source_exports: &controls.exports,
                source_export_controls: &export_controls,
            })
            .map_err(|_| "bound shell effect provider refused the selected effect")?;
        let (result, output_bytes) = result.into_value()?;
        charge_work(work, command.len() + output_bytes).map_err(|error| error.to_string())?;
        record_status(state, result.status, work)?;
        effects.next.set(ordinal + 1);
        return Ok(result.value);
    }
    let (value, status) = if let Ok((value, _)) = pipeline {
        charge_work(work, command.len() + value.len()).map_err(|error| error.to_string())?;
        (value, 0)
    } else {
        let default_shell_controls = default_environment_controls(&state.borrow());
        let directory = |path: &str| {
            let kind = reader.borrow_mut().wildcard_entry_kind(path)?;
            match kind {
                Some(TreeContentEntryKind::Directory) => reader
                    .borrow_mut()
                    .realpath(path)?
                    .map(|path| {
                        path.into_os_string()
                            .into_string()
                            .map_err(|_| "shell directory is not UTF-8".to_owned())
                    })
                    .transpose(),
                Some(TreeContentEntryKind::SymbolicLink) => {
                    Err("shell directory traverses a symbolic link".into())
                }
                _ => Ok(None),
            }
        };
        let regular_file = |path: &str| match reader.borrow_mut().wildcard_entry_kind(path)? {
            Some(TreeContentEntryKind::SymbolicLink) => {
                Err("shell file test cannot follow a symbolic link".into())
            }
            value => Ok(value == Some(TreeContentEntryKind::RegularFile)),
        };
        let glob = |pattern: &str| {
            super::captured_wildcard::query(&mut reader.borrow_mut(), pattern, wildcard_work)
        };
        let charge = |amount| charge_work(work, amount).map_err(|error| error.to_string());
        let result = crate::make_shell_data::evaluate(
            command,
            &crate::make_shell_data::ShellDataContext {
                working_directory: paths.working_directory,
                directory: &directory,
                regular_file: &regular_file,
                glob: &glob,
                default_shell_controls,
                charge: &charge,
            },
        )?;
        (
            crate::make_shell_data::fold_stdout(&result.stdout),
            result.status,
        )
    };
    record_status(state, status, work)?;
    Ok(value)
}

fn validate_bound_policy(
    reader: &RefCell<&mut MakeInputReader<'_>>,
    state: &RefCell<State>,
    paths: &MakeProgramPaths<'_>,
    expressions: &crate::make_expr::MakeProgramBudget,
    work: &Cell<usize>,
    wildcard_work: &Cell<usize>,
    external_programs: &[&str],
) -> Result<Option<BoundControls>, String> {
    let policy = state.borrow().captured_shell_policy.clone();
    let Some(policy) = policy else {
        // Legacy data-only interpretation has no native shell-policy claim.
        // Live consumers independently require captured_shell_policy_bound().
        return Ok(None);
    };
    if !external_programs.is_empty() && policy.source_search.is_none() {
        return Err("captured external shell effect lacks a bound search capability".into());
    }

    let wildcard = |pattern: &str| {
        super::captured_wildcard::query(&mut reader.borrow_mut(), pattern, wildcard_work)
    };
    let realpath = |input: &str| {
        reader
            .borrow_mut()
            .realpath(input)?
            .map(|path| {
                path.into_os_string()
                    .into_string()
                    .map_err(|_| "captured realpath is not UTF-8".to_owned())
            })
            .transpose()
    };
    let refuse_nested_shell = |_command: &str| -> Result<String, String> {
        Err("nested shell effects in captured shell controls are unsupported".into())
    };
    let execution = super::Evaluation {
        paths,
        expressions,
        work,
        file: None,
        captured_wildcard: Some(&wildcard),
        captured_realpath: Some(&realpath),
        captured_shell: Some(&refuse_nested_shell),
        automatic_variables: None,
    };

    let shell = evaluate_policy_expression("$(SHELL)", state, &execution)?;
    if shell != policy.shell {
        return Err("captured shell policy rejects changed SHELL".into());
    }
    let flags = evaluate_policy_expression("$(.SHELLFLAGS)", state, &execution)?;
    if !policy.flags.iter().any(|allowed| allowed == &flags) {
        return Err("captured shell policy rejects changed .SHELLFLAGS".into());
    }
    if flags != "-c" {
        return Err("captured shell data does not model alternative execution flags".into());
    }

    let before = state.borrow().events.len();
    let exports = super::exports::values(state, &execution, true)
        .map_err(|_| "captured shell policy cannot establish exported environment".to_owned())?;
    if state.borrow().events.len() != before {
        return Err("captured shell policy export evaluation changed Make state".into());
    }
    let scan_cost =
        exports
            .iter()
            .try_fold(policy.inherited_scan_cost, |total, (name, value)| {
                total
                    .checked_add(name.len())
                    .and_then(|bytes| bytes.checked_add(value.len()))
                    .and_then(|bytes| bytes.checked_add(2))
                    .ok_or("captured shell policy environment scan size overflow")
            })?;
    charge_work(work, scan_cost).map_err(|error| error.to_string())?;
    // Native callers supply a sealed PATH and explicit locale. The data
    // evaluator cannot authorize another executable search or locale halfway
    // through Source parsing. Do not infer a PATH fallback when it is absent.
    if let Some(search) = &policy.source_search {
        if search.path().to_str() != policy.inherited.get("PATH").map(String::as_str) {
            return Err("captured shell inherited search path changed".into());
        }
        super::captured_shell_search::validate_captured_source_commands(
            search.as_ref(),
            exports
                .get("PATH")
                .map(String::as_str)
                .ok_or("captured shell search PATH is absent or exceeds its bound")?,
            external_programs,
            &mut reader.borrow_mut(),
        )?;
    } else if exports.get("PATH") != policy.inherited.get("PATH") {
        return Err("captured shell policy execution environment changed: PATH".into());
    }
    for name in [
        "LC_ALL",
        "LC_COLLATE",
        "LC_CTYPE",
        "LANG",
        "LANGUAGE",
        "GREP_OPTIONS",
        "AWKPATH",
        "AWKLIBPATH",
        "SED_OPTIONS",
    ] {
        if exports.get(name) != policy.inherited.get(name) {
            return Err(format!(
                "captured shell policy execution environment changed: {name}"
            ));
        }
    }
    for category in ["LC_COLLATE", "LC_CTYPE"] {
        let locale = ["LC_ALL", category, "LANG"]
            .into_iter()
            .filter_map(|name| exports.get(name))
            .find(|value| !value.is_empty());
        if locale.is_some_and(|value| !matches!(value.as_str(), "C" | "POSIX")) {
            return Err("captured shell policy needs a bytewise POSIX locale".into());
        }
    }
    aros_common::make_observer::validate_posix_shell_environment(&policy.inherited, &exports)
        .map_err(|_| "captured shell policy rejects exported environment".to_owned())?;
    Ok(Some(BoundControls {
        shell,
        flags,
        exports,
    }))
}

fn evaluate_policy_expression(
    expression: &str,
    state: &RefCell<State>,
    execution: &super::Evaluation<'_>,
) -> Result<String, String> {
    let before = state.borrow().events.len();
    let value = super::evaluate(expression, usize::MAX, state, execution)
        .map_err(|_| "captured shell policy control evaluation is unsupported".to_owned())?;
    if state.borrow().events.len() != before {
        return Err("captured shell policy control evaluation changed Make state".into());
    }
    Ok(value)
}

fn default_environment_controls(state: &State) -> bool {
    // These are shell language controls, not AROS variable guesses. Only the
    // complete measured initial export selection plus interpreted directives
    // can establish absence. Export-all/unknown exports already refuse.
    ["IFS", "CDPATH", "ENV", "BASH_ENV"].iter().all(|name| {
        matches!(
            state
                .exports
                .get(*name)
                .or_else(|| state.inherited_exports.get(*name)),
            None | Some(Some(false))
        )
    })
}

fn record_status(data: &RefCell<State>, status: u8, work: &Cell<usize>) -> Result<(), String> {
    let mut state = data.borrow_mut();
    let position = state.events.len();
    if position >= MAX_STATEMENTS {
        return Err("shell status effect exceeds statement limit".into());
    }
    let value = status.to_string();
    let cost = retained_cost(value.len() + 64).map_err(|error| error.to_string())?;
    charge_work(work, cost).map_err(|error| error.to_string())?;
    let storage = state
        .storage
        .checked_add(cost)
        .filter(|size| *size <= MAX_STORAGE)
        .ok_or("shell status effect exceeds storage limit")?;
    // GNU's intrinsic .SHELLSTATUS update is override-origin/simple; this is
    // not a source assignment or a relaxation of admitted source name syntax.
    let name = ".SHELLSTATUS";
    state.scope.local_names.insert(name.into());
    state
        .scope
        .raw
        .entry(name.into())
        .or_default()
        .push((position, value.clone()));
    state
        .scope
        .path_raw
        .entry(name.into())
        .or_default()
        .push((position, value.clone()));
    state
        .scope
        .assignments
        .entry(name.into())
        .or_default()
        .push((position, vec![value.clone()]));
    state
        .scope
        .path_replacements
        .entry(name.into())
        .or_default()
        .push(position);
    state.flavors.insert(name.into(), VariableFlavor::Simple);
    state
        .origins
        .insert(name.into(), MakeVariableOrigin::Override);
    state.names.insert(name.into());
    state.storage = storage;
    state.events.push(MakeVariableEvent {
        ordinal: position,
        source_line: 0,
        active: Some(true),
        text: format!(".SHELLSTATUS := {value}"),
        input_effect: true,
    });
    Ok(())
}
