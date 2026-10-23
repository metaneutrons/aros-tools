//! Source-controlled exports from explicitly measured origins, never ambient data.

use super::{
    apply_scalar_fragment, charge_work, evaluate, guard, invalid, retained_cost,
    variable_assignment_impl, Evaluation, MakeVariableEvent, State, MAX_STATEMENTS, MAX_STORAGE,
    MAX_VALUE,
};
use crate::make_expr::MakeExprError;
use crate::make_vars::directive_tail;
use aros_common::make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace};
use std::cell::RefCell;
use std::collections::BTreeMap;

pub(super) fn is_directive(line: &str) -> bool {
    directive_tail(line, "export").is_some() || directive_tail(line, "unexport").is_some()
}

fn environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 128
        && bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(super) fn initial_selection(
    namespace: &ValidatedMakeNamespace,
) -> BTreeMap<String, Option<bool>> {
    namespace
        .to_observation()
        .entries
        .into_iter()
        .filter(|entry| {
            entry.origin == MakeVariableOrigin::Environment
                || entry.origin == MakeVariableOrigin::CommandLine
        })
        .map(|entry| {
            let exported = if entry.origin == MakeVariableOrigin::Environment
                || environment_name(&entry.name)
            {
                Some(true)
            } else {
                // A command-line override can retain a previously imported export
                // flag. Final origin alone cannot prove whether that happened.
                None
            };
            (entry.name, exported)
        })
        .collect()
}

pub(super) fn bind_initial_environment(
    state: &mut State,
    namespace: &ValidatedMakeNamespace,
    names: &std::collections::BTreeSet<String>,
) -> Result<(), String> {
    if names.iter().any(|name| !namespace.contains(name))
        || namespace.to_observation().entries.iter().any(|entry| {
            entry.origin == MakeVariableOrigin::Environment && !names.contains(&entry.name)
        })
    {
        return Err("initial environment membership differs from measured Make namespace".into());
    }
    for (name, exported) in &mut state.inherited_exports {
        if namespace.origin(name) == Some(MakeVariableOrigin::CommandLine)
            && !environment_name(name)
        {
            *exported = Some(names.contains(name));
        }
    }
    Ok(())
}

pub(super) fn apply(
    line: &str,
    source_line: usize,
    active: Option<bool>,
    data: &RefCell<State>,
    execution: &Evaluation<'_>,
) -> Result<bool, MakeExprError> {
    let (tail, exported) = if let Some(tail) = directive_tail(line, "export") {
        (tail, true)
    } else if let Some(tail) = directive_tail(line, "unexport") {
        (tail, false)
    } else {
        return Ok(false);
    };
    if tail.trim().is_empty() {
        return Err(invalid("implicit export-all/unexport-all is not modeled"));
    }
    let assignment = variable_assignment_impl(tail, true);
    if !exported && assignment.is_some() {
        return Err(invalid("unexport does not accept a variable assignment"));
    }
    let names = if let Some((name, _, _)) = assignment {
        if !environment_name(name) {
            return Err(invalid("unsupported explicit export name"));
        }
        apply_scalar_fragment(tail, source_line, active, data, execution)?;
        name.to_owned()
    } else if active == Some(true) {
        evaluate(tail, source_line, data, execution)?
    } else if tail.contains('$') {
        if active.is_none() {
            return Err(invalid(
                "dynamic export names require proved branch selection",
            ));
        }
        String::new()
    } else {
        tail.to_owned()
    };
    let names: Vec<_> = names.split_whitespace().collect();
    if names.is_empty() && active != Some(false) {
        return Err(invalid(
            "export names expanded to an unsupported empty selection",
        ));
    }
    if names.iter().any(|name| !environment_name(name)) {
        return Err(invalid("unsupported explicit export name"));
    }
    if active != Some(false) {
        for name in &names {
            initialize_missing(name, source_line, active, data, execution)?;
        }
    }
    let mut state = data.borrow_mut();
    if state.events.len() >= MAX_STATEMENTS {
        return Err(invalid("Make export statement count exceeds limit"));
    }
    let cost = retained_cost(
        line.len()
            .saturating_add(names.iter().map(|name| name.len()).sum::<usize>()),
    )?;
    charge_work(execution.work, cost)?;
    state.storage = state
        .storage
        .checked_add(cost)
        .filter(|size| *size <= MAX_STORAGE)
        .ok_or_else(|| invalid("Make export state exceeds storage budget"))?;
    for name in names {
        if active != Some(false) {
            state
                .exports
                .insert(name.to_owned(), active.map(|_| exported));
        }
    }
    let ordinal = state.events.len();
    state.events.push(MakeVariableEvent {
        ordinal,
        source_line,
        active,
        text: line.to_owned(),
        input_effect: false,
    });
    Ok(true)
}

fn initialize_missing(
    name: &str,
    source_line: usize,
    active: Option<bool>,
    data: &RefCell<State>,
    execution: &Evaluation<'_>,
) -> Result<(), MakeExprError> {
    let absent = {
        let state = data.borrow();
        if state
            .scope
            .conditionally_assigned_before(name, state.events.len())
        {
            return Err(invalid("Make export name has unresolved definition state"));
        }
        if state.scope.is_defined_before(name, state.events.len()) {
            return Ok(());
        }
        let absent = state.undefined.contains(name)
            || state.scope.undefinitions.contains_key(name)
            || state
                .initial_namespace
                .as_ref()
                .is_some_and(|namespace| namespace.proves_absent(name));
        if !absent
            && !state
                .initial_namespace
                .as_ref()
                .is_some_and(|namespace| namespace.contains(name))
        {
            return Err(invalid("Make export requires proved initial definedness"));
        }
        absent
    };
    if absent {
        // GNU export AND unexport create a missing variable as file-origin,
        // simply-expanded empty data. This affects later ?= and ifdef behavior.
        apply_scalar_fragment(&format!("{name} :="), source_line, active, data, execution)?;
    }
    Ok(())
}

pub(super) fn values(
    data: &RefCell<State>,
    execution: &Evaluation<'_>,
    inherit_initial: bool,
) -> Result<BTreeMap<String, String>, MakeExprError> {
    let selected = {
        let state = data.borrow();
        let mut selected = if inherit_initial {
            if state.coordinate_projection || state.initial_namespace.is_none() {
                return Err(invalid(
                    "source export inheritance needs a measured source-mode namespace",
                ));
            }
            if state.initial_namespace.as_ref().is_some_and(|namespace| {
                namespace
                    .to_observation()
                    .entries
                    .iter()
                    .any(|entry| entry.origin == MakeVariableOrigin::EnvironmentOverride)
            }) {
                return Err(invalid(
                    "environment-override export semantics are not modeled",
                ));
            }
            state.inherited_exports.clone()
        } else {
            BTreeMap::new()
        };
        selected.extend(state.exports.clone());
        selected
    };
    let mut result = BTreeMap::new();
    let mut total = 0usize;
    for (name, exported) in selected {
        match exported {
            Some(false) => continue,
            None => return Err(invalid("Make export selection is unresolved")),
            Some(true) => {}
        }
        let position = {
            let state = data.borrow();
            if guard(&state, &name).is_some() {
                return Err(invalid("exported Make variable has unmeasured state"));
            }
            let position = state.events.len();
            if !state.scope.is_defined_before(&name, position) {
                if !state.undefined.contains(&name)
                    && !state.scope.undefinitions.contains_key(&name)
                    && !state
                        .initial_namespace
                        .as_ref()
                        .is_some_and(|namespace| namespace.proves_absent(&name))
                {
                    return Err(invalid("exported Make variable has unproved definedness"));
                }
                continue;
            }
            position
        };
        let imported = {
            let state = data.borrow();
            if state.origins.get(&name) == Some(&MakeVariableOrigin::Environment) {
                let raw = super::lookup(&state, &name)
                    .ok_or_else(|| invalid("exported environment value was not measured"))?;
                // GNU passes untouched environment-origin data literally,
                // even when its Make-variable flavor is recursive. Simple
                // initial values carry our internal doubled-dollar encoding.
                Some(match state.flavors.get(&name) {
                    Some(super::VariableFlavor::Recursive) => raw,
                    Some(super::VariableFlavor::Simple) => raw.replace("$$", "$"),
                    _ => return Err(invalid("exported environment flavor was not measured")),
                })
            } else {
                None
            }
        };
        let value = if let Some(value) = imported {
            value
        } else {
            evaluate(&format!("$({name})"), usize::MAX, data, execution)?
        };
        if data.borrow().events.len() != position {
            return Err(invalid(
                "Make export value expansion has unsupported side effects",
            ));
        }
        total = total.saturating_add(name.len()).saturating_add(value.len());
        if value.chars().any(char::is_control) || total > MAX_VALUE {
            return Err(invalid(
                "Make export values exceed bounds or contain control data",
            ));
        }
        result.insert(name, value);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dirs::DirVars,
        make_vars::{MakeProgramPaths, MakeVariableProgram},
        parser::TargetContext,
    };
    use std::path::Path;

    fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
        MakeProgramPaths {
            dirs,
            source_root: Path::new("/source"),
            relative_dir: Path::new("unit"),
            top_directory: Path::new("/build"),
            working_directory: Path::new("/build"),
        }
    }

    #[test]
    fn explicit_exports_keep_flavor_and_create_initially_undefined_empty_values() {
        let dirs = DirVars::default();
        let paths = paths(&dirs);
        let mut program = MakeVariableProgram::new(&TargetContext::default(), &["ABSENT"]).unwrap();
        program.read_file("root.mk", concat!(
            "LATER := first\nexport SIMPLE := $(LATER)\nexport RECURSIVE = $(LATER)\n",
            "export EMPTY :=\nexport ABSENT\nexport REMOVED := discard\nunexport REMOVED\nLATER := second\n",
        ), &paths, &|_, _| Ok(None)).unwrap();
        assert_eq!(
            program.explicit_export_values(&paths).unwrap(),
            BTreeMap::from([
                ("SIMPLE".into(), "first".into()),
                ("RECURSIVE".into(), "second".into()),
                ("EMPTY".into(), String::new()),
                ("ABSENT".into(), String::new()),
            ])
        );
    }

    #[test]
    fn include_eval_conditionals_and_export_state_are_transactional() {
        let dirs = DirVars::default();
        let paths = paths(&dirs);
        let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
        program
            .read_file(
                "root.mk",
                "BASE := prior\nexport BASE\ninclude chosen.mk\nifeq (0,1)\nunexport BASE\nendif\n",
                &paths,
                &|name, _| {
                    Ok((name == "chosen.mk")
                        .then(|| "$(eval export FROM_INCLUDE := selected)\n".into()))
                },
            )
            .unwrap();
        let before = program.explicit_export_values(&paths).unwrap();
        assert_eq!(before["BASE"], "prior");
        assert_eq!(before["FROM_INCLUDE"], "selected");
        assert!(program
            .read_file(
                "bad.mk",
                "unexport BASE\nexport TRANSIENT := bad\nnot supported\n",
                &paths,
                &|_, _| Ok(None)
            )
            .is_err());
        assert_eq!(program.explicit_export_values(&paths).unwrap(), before);
        program.observe("unexport BASE", 20, None, &paths).unwrap();
        assert!(program.explicit_export_values(&paths).is_err());
    }

    #[test]
    fn names_expand_at_directive_time_but_values_at_invocation_time() {
        let dirs = DirVars::default();
        let paths = paths(&dirs);
        let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
        program.execute("SELECT := FIRST", 0, &paths).unwrap();
        program.execute("FIRST := one", 1, &paths).unwrap();
        program.execute("SECOND := two", 2, &paths).unwrap();
        program.execute("export $(SELECT)", 3, &paths).unwrap();
        program.execute("SELECT := SECOND", 4, &paths).unwrap();
        program.execute("FIRST := later", 5, &paths).unwrap();
        assert_eq!(
            program.explicit_export_values(&paths).unwrap(),
            BTreeMap::from([("FIRST".into(), "later".into())])
        );
        for refused in [
            "export",
            "unexport",
            "unexport FIRST := bad",
            "export invalid-name",
            "export $(unknown function)",
        ] {
            assert!(program.execute(refused, 6, &paths).is_err(), "{refused}");
        }
    }

    #[test]
    fn export_evaluation_refuses_unknown_values_and_rolls_back_effects() {
        let dirs = DirVars::default();
        let paths = paths(&dirs);
        let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
        program
            .execute("export UNSAFE = $(eval LEAK := bad)value", 0, &paths)
            .unwrap();
        let before = program.events().to_vec();
        assert!(program.explicit_export_values(&paths).is_err());
        assert_eq!(program.events(), before);
        assert!(program.evaluate("$(LEAK)", &paths).is_err());
        program.execute("unexport UNSAFE", 1, &paths).unwrap();
        assert!(program.execute("export UNKNOWN", 2, &paths).is_err());
    }

    #[test]
    fn explicit_exports_match_a_synthetic_print_only_gnu_child_environment() {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        let dirs = DirVars::default();
        let paths = paths(&dirs);
        let body = concat!(
            "LATER := first\nexport SIMPLE := $(LATER)\nexport RECURSIVE = $(LATER)\n",
            "export EMPTY :=\nexport ABSENT\nexport REMOVED := discard\nunexport REMOVED\nLATER := second\n",
            "unexport UNEXPORTED\nUNEXPORTED ?= $(error missing-empty-binding)\nCHECK := $(UNEXPORTED)\n",
        );
        let mut program =
            MakeVariableProgram::new(&TargetContext::default(), &["ABSENT", "UNEXPORTED"]).unwrap();
        program
            .read_file("synthetic.mk", body, &paths, &|_, _| Ok(None))
            .unwrap();
        let values = program.explicit_export_values(&paths).unwrap();
        let expected = format!(
            "{}|{}|{}|{}|{}",
            values["SIMPLE"],
            values["RECURSIVE"],
            if values.contains_key("EMPTY") {
                "x"
            } else {
                ""
            },
            if values.contains_key("ABSENT") {
                "x"
            } else {
                ""
            },
            if values.contains_key("REMOVED") {
                "x"
            } else {
                ""
            }
        );
        let make = if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        };
        let oracle = |source_body: &str| {
            // This custom synthetic recipe only prints five known test fields.
            // It reads no Source recipe or ambient environment and builds no
            // object. In particular, this is not an environment dump.
            let source = format!(
                "{source_body}.PHONY: probe\nprobe:\n\t@printf 'AROS_TEST_RESULT=%s|%s|%s|%s|%s\\n' \"$$SIMPLE\" \"$$RECURSIVE\" \"$${{EMPTY+x}}\" \"$${{ABSENT+x}}\" \"$${{REMOVED+x}}\"\n"
            );
            let mut child = Command::new(make)
                .args(["-rR", "--no-print-directory", "-s", "-f", "-", "probe"])
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(source.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "synthetic export oracle failed");
            let text = String::from_utf8(output.stdout).unwrap();
            let rows = text
                .lines()
                .filter_map(|line| line.strip_prefix("AROS_TEST_RESULT="))
                .collect::<Vec<_>>();
            assert_eq!(rows.len(), 1);
            rows[0].to_owned()
        };
        assert_eq!(expected, "first|second|x|x|");
        assert_eq!(oracle(body), expected);
        let changed_flavor = body.replace("export SIMPLE :=", "export SIMPLE =");
        assert_eq!(oracle(&changed_flavor), "second|second|x|x|");
        assert_ne!(oracle(&changed_flavor), expected);
        let removed_binding =
            body.replace("unexport REMOVED", "undefine REMOVED\nREMOVED := recreated");
        let mut recreated =
            MakeVariableProgram::new(&TargetContext::default(), &["ABSENT", "UNEXPORTED"]).unwrap();
        recreated
            .read_file("recreated.mk", &removed_binding, &paths, &|_, _| Ok(None))
            .unwrap();
        assert!(!recreated
            .explicit_export_values(&paths)
            .unwrap()
            .contains_key("REMOVED"));
        program
            .read_file(
                "remove.mk",
                "undefine REMOVED\nREMOVED := recreated\n",
                &paths,
                &|_, _| Ok(None),
            )
            .unwrap();
        assert!(!program
            .explicit_export_values(&paths)
            .unwrap()
            .contains_key("REMOVED"));
        assert_eq!(oracle(&removed_binding), expected);
    }

    #[test]
    fn source_exports_and_literal_env_prefix_feed_a_fresh_generic_child_context() {
        use crate::make_vars::{MakeInitialFlavor, MakeInitialValue};
        use aros_common::make_environment_prefix::LiteralMakeEnvironmentPrefix;
        use aros_common::make_namespace::{
            MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin,
            ValidatedMakeNamespace,
        };
        use std::io::Write as _;
        use std::process::{Command, Stdio};

        let dirs = DirVars::default();
        let paths = paths(&dirs);
        let root_body = concat!(
            "SEED := early\nexport SHARED := inherited\nexport LIVE = $(SEED)\n",
            "export OVERRIDE := base\nPREFIX := env PARAM=$(SEED) OVERRIDE=prefix\nSEED := late\n",
        );
        let child_body = concat!(
            "FROZEN := $(PARAM)/$(SELECTION)\nPARAM ?= wrong\nOVERRIDE += suffix\n",
            "SHARED := filevalue\nSELECTION := $(eval EFFECT := observed)ignored\n",
        );
        let expression = "$(FROZEN)|$(SHARED)|$(LIVE)|$(OVERRIDE)|$(SELECTION)|$(EFFECT)";
        let replay = |root_text: &str| {
            let mut root = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
            root.read_file("parent.mk", root_text, &paths, &|_, _| Ok(None))
                .unwrap();
            let mut environment = root.explicit_export_values(&paths).unwrap();
            let prefix =
                LiteralMakeEnvironmentPrefix::parse(&root.evaluate("$(PREFIX)", &paths).unwrap())
                    .unwrap();
            environment.extend(prefix.assignments().clone());
            let mut entries = vec![
                MakeNamespaceEntry {
                    name: ".VARIABLES".into(),
                    origin: MakeVariableOrigin::Default,
                },
                MakeNamespaceEntry {
                    name: "MAKE_VERSION".into(),
                    origin: MakeVariableOrigin::Default,
                },
                MakeNamespaceEntry {
                    name: "SELECTION".into(),
                    origin: MakeVariableOrigin::CommandLine,
                },
            ];
            entries.extend(environment.keys().map(|name| MakeNamespaceEntry {
                name: name.clone(),
                origin: MakeVariableOrigin::Environment,
            }));
            let namespace = ValidatedMakeNamespace::validate(MakeNamespaceObservation {
                version: 1,
                entries,
            })
            .unwrap();
            let mut bindings = environment
                .iter()
                .map(|(name, value)| MakeInitialValue {
                    name,
                    value,
                    flavor: MakeInitialFlavor::Recursive,
                    origin: MakeVariableOrigin::Environment,
                })
                .collect::<Vec<_>>();
            bindings.push(MakeInitialValue {
                name: "SELECTION",
                value: "owner",
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::CommandLine,
            });
            let mut child = MakeVariableProgram::new_with_initial_values(
                &TargetContext::default(),
                &[],
                &paths,
                &namespace,
                &bindings,
            )
            .unwrap();
            child
                .read_file("child.mk", child_body, &paths, &|_, _| Ok(None))
                .unwrap();
            child.evaluate(expression, &paths).unwrap()
        };
        let temporary = tempfile::tempdir().unwrap();
        let child_path = temporary.path().join("child.mk");
        std::fs::write(
            &child_path,
            format!("{child_body}$(info AROS_TEST_RESULT={expression})\n.PHONY: leaf\nleaf:\n"),
        )
        .unwrap();
        let quoted_child = format!(
            "'{}'",
            child_path
                .to_str()
                .unwrap()
                .replace('\'', "'\"'\"'")
                .replace('$', "$$")
        );
        let make = if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        };
        let oracle = |root_text: &str| {
            // Only synthetic parent/child data and one fixed GNU Make query
            // recipe execute. No actual Source recipe, target compiler or
            // inherited operator environment participates.
            let source = format!(
                "{root_text}.PHONY: probe\nprobe:\n\t@$(PREFIX) $(MAKE) --no-print-directory -s -rR -f {quoted_child} leaf SELECTION=owner\n"
            );
            let mut child = Command::new(make)
                .args(["-rR", "--no-print-directory", "-s", "-f", "-", "probe"])
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(source.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "synthetic parent/child oracle failed"
            );
            let text = String::from_utf8(output.stdout).unwrap();
            let rows = text
                .lines()
                .filter_map(|line| line.strip_prefix("AROS_TEST_RESULT="))
                .collect::<Vec<_>>();
            assert_eq!(rows.len(), 1);
            rows[0].to_owned()
        };
        let actual = replay(root_body);
        assert_eq!(
            actual,
            "early/owner|filevalue|late|prefix suffix|owner|observed"
        );
        assert_eq!(oracle(root_body), actual);
        let counter = root_body.replace("PREFIX :=", "PREFIX =");
        assert_eq!(oracle(&counter), replay(&counter));
        assert_ne!(oracle(&counter), actual);
    }
}
