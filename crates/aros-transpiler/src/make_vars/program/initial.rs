//! Data-only initial values; their source/execution proof belongs to the caller.

use super::{
    bound_coordinate, retained_cost, valid_observed_name, State, VariableFlavor,
    CLOSED_TARGET_MAKE_SELECTORS, MAX_INITIAL_BINDINGS, MAX_STORAGE, MAX_VALUE,
};
use aros_common::make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace};
use std::collections::HashSet;

/// Actual GNU Make expansion flavor, never inferred from a value's spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MakeInitialFlavor {
    /// An upstream-expanded value; embedded dollars are literal data.
    Simple,
    /// A raw expression whose references expand at each later use.
    Recursive,
}

/// One independently supplied initial value and its measured interpretation.
///
/// This type authenticates nothing. In particular, a source formula and an
/// already-expanded simple value are not interchangeable. Do not populate it
/// from an unrelated directory table or bulk ambient-environment dump.
#[derive(Clone, Copy)]
pub struct MakeInitialValue<'a> {
    pub name: &'a str,
    pub value: &'a str,
    pub flavor: MakeInitialFlavor,
    pub origin: MakeVariableOrigin,
}

impl std::fmt::Debug for MakeInitialValue<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MakeInitialValue")
            .field("name", &self.name)
            .field("flavor", &self.flavor)
            .field("origin", &self.origin)
            .field("value_bytes", &self.value.len())
            .finish_non_exhaustive()
    }
}

pub(super) fn install(
    state: &mut State,
    namespace: &ValidatedMakeNamespace,
    values: &[MakeInitialValue<'_>],
) -> Result<(), String> {
    install_selected(state, namespace, values, false)
}

pub(super) fn install_recipe_engine(
    state: &mut State,
    namespace: &ValidatedMakeNamespace,
    packet: &aros_common::make_initial_values::ValidatedMakeRecipeEngineValues,
) -> Result<(), String> {
    let values = packet
        .entries()
        .map(|entry| MakeInitialValue {
            name: &entry.name,
            value: &entry.value,
            flavor: MakeInitialFlavor::Simple,
            origin: entry.origin,
        })
        .collect::<Vec<_>>();
    install_selected(state, namespace, &values, true)
}

fn install_selected(
    state: &mut State,
    namespace: &ValidatedMakeNamespace,
    values: &[MakeInitialValue<'_>],
    recipe_engine: bool,
) -> Result<(), String> {
    if values
        .len()
        .saturating_add(state.scope.configuration.len())
        .saturating_add(state.undefined.len())
        > MAX_INITIAL_BINDINGS
    {
        return Err("too many initial Make value bindings".into());
    }
    let mut names = HashSet::new();
    for binding in values {
        if !valid_observed_name(binding.name)
            || (state.coordinate_projection
                && (bound_coordinate(binding.name)
                    || CLOSED_TARGET_MAKE_SELECTORS.contains(&binding.name)))
            || state.scope.configuration.contains_key(binding.name)
            || state.undefined.contains(binding.name)
            || !names.insert(binding.name)
        {
            return Err("invalid, duplicate or overlapping initial Make value binding".into());
        }
        if namespace.origin(binding.name) != Some(binding.origin) {
            return Err("initial Make value origin differs from the measured namespace".into());
        }
        // Environment loses to file assignments; command-line bindings ignore
        // ordinary assignment/undefine replacement. GNU immediate RHS effects
        // still happen before stronger-origin discard. -e/override
        // and automatic lifetime semantics are still independently unsupported.
        let immutable_engine = recipe_engine
            && binding.name == aros_common::make_initial_values::MAKE_CLI_PROPAGATION_VARIABLE
            && binding.origin == MakeVariableOrigin::Automatic
            && binding.flavor == MakeInitialFlavor::Simple;
        if !immutable_engine
            && !matches!(
                binding.origin,
                MakeVariableOrigin::File
                    | MakeVariableOrigin::Default
                    | MakeVariableOrigin::Environment
                    | MakeVariableOrigin::CommandLine
            )
        {
            return Err("initial Make value has unsupported origin semantics".into());
        }
        // Preserve measured flavor even for environment-origin bindings.
        // GNU itself can create a simple environment binding; the caller must
        // observe it, not infer recursive flavor from the origin label.
        if binding.value.len() > MAX_VALUE
            || binding
                .value
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        {
            return Err(
                "initial Make value exceeds its bounds or contains unsupported control data".into(),
            );
        }
        let length = binding
            .value
            .len()
            .checked_add(if binding.flavor == MakeInitialFlavor::Simple {
                binding.value.matches('$').count()
            } else {
                0
            })
            .filter(|length| *length <= MAX_VALUE)
            .ok_or("escaped initial Make value exceeds its byte bound")?;
        let cost = retained_cost(binding.name.len() + length).map_err(|error| error.to_string())?;
        state.storage = state
            .storage
            .checked_add(cost)
            .filter(|size| *size <= MAX_STORAGE)
            .ok_or("initial Make values exceed the state storage budget")?;
        let (value, flavor) = match binding.flavor {
            MakeInitialFlavor::Simple => (binding.value.replace('$', "$$"), VariableFlavor::Simple),
            MakeInitialFlavor::Recursive => (binding.value.to_owned(), VariableFlavor::Recursive),
        };
        state.scope.configuration.insert(binding.name.into(), value);
        state.flavors.insert(binding.name.into(), flavor);
        state.origins.insert(binding.name.into(), binding.origin);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::DirVars;
    use crate::make_vars::{MakeProgramPaths, MakeVariableProgram};
    use crate::parser::TargetContext;
    use aros_common::make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation};
    use std::path::Path;

    fn namespace(names: &[(&str, MakeVariableOrigin)]) -> ValidatedMakeNamespace {
        let mut entries = vec![
            MakeNamespaceEntry {
                name: ".VARIABLES".into(),
                origin: MakeVariableOrigin::Default,
            },
            MakeNamespaceEntry {
                name: "MAKE_VERSION".into(),
                origin: MakeVariableOrigin::Default,
            },
        ];
        entries.extend(names.iter().map(|(name, origin)| MakeNamespaceEntry {
            name: (*name).into(),
            origin: *origin,
        }));
        ValidatedMakeNamespace::validate(MakeNamespaceObservation {
            version: 1,
            entries,
        })
        .unwrap()
    }

    fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
        MakeProgramPaths {
            dirs,
            source_root: Path::new("/source"),
            relative_dir: Path::new("module"),
            top_directory: Path::new("/build"),
            working_directory: Path::new("/build"),
        }
    }

    #[test]
    fn initial_flavors_preserve_later_contributions_and_append_timing() {
        let namespace = namespace(&[
            ("SIMPLE", MakeVariableOrigin::File),
            ("RECURSIVE", MakeVariableOrigin::File),
            ("EMPTY", MakeVariableOrigin::File),
        ]);
        let dirs = DirVars::from_config_text("");
        let paths = paths(&dirs);
        let mut program = MakeVariableProgram::new_with_initial_values(
            &TargetContext::default(),
            &[],
            &paths,
            &namespace,
            &[
                MakeInitialValue {
                    name: "SIMPLE",
                    value: "initial",
                    flavor: MakeInitialFlavor::Simple,
                    origin: MakeVariableOrigin::File,
                },
                MakeInitialValue {
                    name: "RECURSIVE",
                    value: "$(LATER)",
                    flavor: MakeInitialFlavor::Recursive,
                    origin: MakeVariableOrigin::File,
                },
                MakeInitialValue {
                    name: "EMPTY",
                    value: "",
                    flavor: MakeInitialFlavor::Simple,
                    origin: MakeVariableOrigin::File,
                },
            ],
        )
        .unwrap();
        assert!(program.events().is_empty());
        assert_eq!(program.evaluate("$(RECURSIVE)", &paths).unwrap(), "");
        program.execute("COPY := $(SIMPLE)", 1, &paths).unwrap();
        program.execute("LATER := first", 1, &paths).unwrap();
        program.execute("SIMPLE += $(LATER)", 2, &paths).unwrap();
        program.execute("RECURSIVE += $(LATER)", 3, &paths).unwrap();
        program
            .execute("EMPTY ?= $(unsupported never-expanded)", 4, &paths)
            .unwrap();
        program.execute("LATER := second", 5, &paths).unwrap();
        assert_eq!(
            program.evaluate("$(SIMPLE)", &paths).unwrap(),
            "initial first"
        );
        assert_eq!(
            program.evaluate("$(RECURSIVE)", &paths).unwrap(),
            "second second"
        );
        assert_eq!(program.evaluate("$(EMPTY)", &paths).unwrap(), "");
        program.execute("undefine SIMPLE", 6, &paths).unwrap();
        program.execute("SIMPLE += $(LATER)", 7, &paths).unwrap();
        program.execute("LATER := third", 8, &paths).unwrap();
        assert_eq!(program.evaluate("$(SIMPLE)", &paths).unwrap(), "third");
        assert_eq!(program.evaluate("$(COPY)", &paths).unwrap(), "initial");
    }

    #[test]
    fn initial_simple_data_does_not_expand_or_execute_embedded_dollars() {
        let namespace = namespace(&[("LITERAL", MakeVariableOrigin::Default)]);
        let dirs = DirVars::from_config_text("");
        let paths = paths(&dirs);
        let value = "$(eval INJECTED := bad)\n\t$cash # literal";
        let mut program = MakeVariableProgram::new_with_initial_values(
            &TargetContext::default(),
            &[],
            &paths,
            &namespace,
            &[MakeInitialValue {
                name: "LITERAL",
                value,
                flavor: MakeInitialFlavor::Simple,
                origin: MakeVariableOrigin::Default,
            }],
        )
        .unwrap();
        assert_eq!(program.evaluate("$(LITERAL)", &paths).unwrap(), value);
        assert!(program.events().is_empty());
        assert_eq!(program.evaluate("$(INJECTED)", &paths).unwrap(), "");
    }

    #[test]
    fn initial_flavors_match_print_only_gnu_make_and_wrong_flavor_counter() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let namespace = namespace(&[
            ("SNAP", MakeVariableOrigin::File),
            ("DEFER", MakeVariableOrigin::File),
            ("EMPTY", MakeVariableOrigin::File),
        ]);
        let dirs = DirVars::from_config_text("");
        let paths = paths(&dirs);
        let body = "LATER := first\nSNAP += $(LATER)\nDEFER += $(LATER)\nEMPTY ?= $(error must-not-expand)\nFROZEN := $(DEFER)\nLATER := second\n";
        let expression = "$(SNAP)|$(DEFER)|$(FROZEN)|<$(EMPTY)>";
        let replay = |flavor, raw| {
            let mut program = MakeVariableProgram::new_with_initial_values(
                &TargetContext::default(),
                &[],
                &paths,
                &namespace,
                &[
                    MakeInitialValue {
                        name: "SNAP",
                        value: "before",
                        flavor: MakeInitialFlavor::Simple,
                        origin: MakeVariableOrigin::File,
                    },
                    MakeInitialValue {
                        name: "DEFER",
                        value: raw,
                        flavor,
                        origin: MakeVariableOrigin::File,
                    },
                    MakeInitialValue {
                        name: "EMPTY",
                        value: "",
                        flavor: MakeInitialFlavor::Simple,
                        origin: MakeVariableOrigin::File,
                    },
                ],
            )
            .unwrap();
            program
                .read_file("synthetic.mk", body, &paths, &|_, _| {
                    Err("no synthetic include supplied".into())
                })
                .unwrap();
            program.evaluate(expression, &paths).unwrap()
        };
        let make = if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        };
        let oracle = |operator: &str| {
            // Only this synthetic text is read. No database, environment values,
            // real Source recipes, compiler or filesystem discovery is involved.
            let source = format!(
                "SNAP := before\nDEFER {operator} $(LATER)\nEMPTY :=\n{body}$(info AROS_TEST_RESULT={expression})\n.PHONY: probe\nprobe:\n"
            );
            let mut child = Command::new(make)
                .args([
                    "-rR",
                    "--no-print-directory",
                    "-s",
                    "-n",
                    "-f",
                    "-",
                    "probe",
                ])
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
            assert!(output.status.success(), "synthetic Make oracle failed");
            let stdout = String::from_utf8(output.stdout).unwrap();
            let rows = stdout
                .lines()
                .filter_map(|line| line.strip_prefix("AROS_TEST_RESULT="))
                .collect::<Vec<_>>();
            assert_eq!(rows.len(), 1);
            rows[0].to_owned()
        };
        let recursive = replay(MakeInitialFlavor::Recursive, "$(LATER)");
        let simple = replay(MakeInitialFlavor::Simple, "");
        assert_eq!(recursive, "before first|second second|first first|<>");
        assert_eq!(recursive, oracle("="));
        assert_eq!(simple, oracle(":="));
        assert_ne!(
            recursive, simple,
            "a frozen recursive value must be detectable"
        );
    }

    #[test]
    fn failed_file_read_does_not_publish_changes_to_initial_values() {
        let namespace = namespace(&[("VALUE", MakeVariableOrigin::File)]);
        let dirs = DirVars::from_config_text("");
        let paths = paths(&dirs);
        let mut program = MakeVariableProgram::new_with_initial_values(
            &TargetContext::default(),
            &[],
            &paths,
            &namespace,
            &[MakeInitialValue {
                name: "VALUE",
                value: "initial",
                flavor: MakeInitialFlavor::Simple,
                origin: MakeVariableOrigin::File,
            }],
        )
        .unwrap();
        let includes = |requested: &str, _: &str| match requested {
            "child.mk" => Ok(Some("VALUE += $(LATER)\n".into())),
            _ => Err("unbound synthetic include".into()),
        };
        assert!(program
            .read_file(
                "failed.mk",
                "LATER := first\ninclude child.mk\ninclude missing.mk\n",
                &paths,
                &includes
            )
            .is_err());
        assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "initial");
        assert_eq!(program.evaluate("$(LATER)", &paths).unwrap(), "");
        assert!(program.events().is_empty());
        program
            .read_file(
                "valid.mk",
                "LATER := second\ninclude child.mk\nLATER := third\n",
                &paths,
                &includes,
            )
            .unwrap();
        assert_eq!(
            program.evaluate("$(VALUE)", &paths).unwrap(),
            "initial second"
        );
    }

    #[test]
    fn environment_and_command_line_priorities_match_print_only_gnu_make() {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        let observed = namespace(&[
            ("FROM_ENV", MakeVariableOrigin::Environment),
            ("EMPTY", MakeVariableOrigin::Environment),
            ("REPLACED", MakeVariableOrigin::Environment),
            ("LOCKED", MakeVariableOrigin::CommandLine),
        ]);
        let dirs = DirVars::default();
        let paths = paths(&dirs);
        let bindings = [
            MakeInitialValue {
                name: "FROM_ENV",
                value: "pre$(LATER)",
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::Environment,
            },
            MakeInitialValue {
                name: "EMPTY",
                value: "",
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::Environment,
            },
            MakeInitialValue {
                name: "REPLACED",
                value: "environment",
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::Environment,
            },
            MakeInitialValue {
                name: "LOCKED",
                value: "locked$(LATER)",
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::CommandLine,
            },
        ];
        let body = concat!(
            "LATER := one\nFROM_ENV ?= wrong\nFROM_ENV += tail$(LATER)\nEMPTY ?= $(error must-not-expand)\n",
            "REPLACED := file\nLOCKED := ignored$(LATER)\nLOCKED += $(error must-not-expand)\n",
            "undefine LOCKED\ndefine LOCKED\nwrong\nendef\nLATER := two\n",
        );
        let expression = "$(FROM_ENV)|<$(EMPTY)>|$(REPLACED)|$(LOCKED)";
        let mut program = MakeVariableProgram::new_with_initial_values(
            &TargetContext::default(),
            &[],
            &paths,
            &observed,
            &bindings,
        )
        .unwrap();
        program
            .read_file("synthetic.mk", body, &paths, &|_, _| Ok(None))
            .unwrap();
        let actual = program.evaluate(expression, &paths).unwrap();
        assert_eq!(actual, "pretwo tailtwo|<>|file|lockedtwo");
        assert_eq!(program.state.origins["FROM_ENV"], MakeVariableOrigin::File);
        assert_eq!(
            program.state.origins["EMPTY"],
            MakeVariableOrigin::Environment
        );
        assert_eq!(program.state.origins["REPLACED"], MakeVariableOrigin::File);
        assert_eq!(
            program.state.origins["LOCKED"],
            MakeVariableOrigin::CommandLine
        );
        let make = if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        };
        let source =
            format!("{body}$(info AROS_TEST_RESULT={expression})\n.PHONY: probe\nprobe:\n");
        let mut child = Command::new(make)
            .args([
                "-rR",
                "--no-print-directory",
                "-s",
                "-n",
                "-f",
                "-",
                "probe",
                "LOCKED=locked$(LATER)",
            ])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("FROM_ENV", "pre$(LATER)")
            .env("EMPTY", "")
            .env("REPLACED", "environment")
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
            "synthetic priority oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .filter_map(|line| line.strip_prefix("AROS_TEST_RESULT="))
                .collect::<Vec<_>>(),
            [actual.as_str()]
        );

        // Reclassifying a command-line binding as ordinary file data must be
        // detected: its forbidden immediate RHS would now be evaluated.
        let mut wrong_bindings = bindings;
        wrong_bindings[3].origin = MakeVariableOrigin::File;
        let wrong_namespace = namespace(&[
            ("FROM_ENV", MakeVariableOrigin::Environment),
            ("EMPTY", MakeVariableOrigin::Environment),
            ("REPLACED", MakeVariableOrigin::Environment),
            ("LOCKED", MakeVariableOrigin::File),
        ]);
        let mut wrong = MakeVariableProgram::new_with_initial_values(
            &TargetContext::default(),
            &[],
            &paths,
            &wrong_namespace,
            &wrong_bindings,
        )
        .unwrap();
        assert!(wrong
            .read_file("wrong.mk", body, &paths, &|_, _| Ok(None))
            .is_err());
        assert!(wrong.events().is_empty());
    }

    #[test]
    fn ignored_plain_and_export_assignments_keep_gnu_immediate_rhs_effects() {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        let observed = namespace(&[("LOCKED", MakeVariableOrigin::CommandLine)]);
        let dirs = DirVars::default();
        let paths = paths(&dirs);
        let make = if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        };
        for prefix in ["", "export "] {
            for operator in [":=", "+="] {
                for flavor in [MakeInitialFlavor::Simple, MakeInitialFlavor::Recursive] {
                    let mut program = MakeVariableProgram::new_with_initial_values(
                        &TargetContext::default(),
                        &[],
                        &paths,
                        &observed,
                        &[MakeInitialValue {
                            name: "LOCKED",
                            value: "command",
                            flavor,
                            origin: MakeVariableOrigin::CommandLine,
                        }],
                    )
                    .unwrap();
                    let body = format!("{prefix}LOCKED {operator} $(eval MARK := hit)ignored\n");
                    program
                        .read_file("synthetic.mk", &body, &paths, &|_, _| Ok(None))
                        .unwrap();
                    let result = program.evaluate("<$(MARK)>|$(LOCKED)", &paths).unwrap();
                    let expected = if operator == ":=" || flavor == MakeInitialFlavor::Simple {
                        "<hit>|command"
                    } else {
                        "<>|command"
                    };
                    assert_eq!(result, expected);
                    let cli = if flavor == MakeInitialFlavor::Simple {
                        "LOCKED:=command"
                    } else {
                        "LOCKED=command"
                    };
                    let source = format!(
                        "{body}$(info AROS_TEST_RESULT=<$(MARK)>|$(LOCKED))\n.PHONY: probe\nprobe:\n"
                    );
                    let mut child = Command::new(make)
                        .args([
                            "-rR",
                            "--no-print-directory",
                            "-s",
                            "-n",
                            "-f",
                            "-",
                            "probe",
                            cli,
                        ])
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
                        "synthetic ignored-assignment oracle failed"
                    );
                    assert_eq!(
                        String::from_utf8(output.stdout)
                            .unwrap()
                            .lines()
                            .filter_map(|line| line.strip_prefix("AROS_TEST_RESULT="))
                            .collect::<Vec<_>>(),
                        [expected]
                    );
                }
            }
        }
    }

    #[test]
    fn namespace_only_priority_cannot_be_replaced_with_fabricated_file_values() {
        let dirs = DirVars::default();
        let paths = paths(&dirs);
        let observed = namespace(&[("UNKNOWN_CLI", MakeVariableOrigin::CommandLine)]);
        let mut program = MakeVariableProgram::new_with_namespace(
            &TargetContext::default(),
            &[],
            &paths,
            &observed,
        )
        .unwrap();
        for statement in [
            "UNKNOWN_CLI := file",
            "UNKNOWN_CLI = file",
            "UNKNOWN_CLI += literal",
            "UNKNOWN_CLI ?= file",
            "undefine UNKNOWN_CLI",
            "export UNKNOWN_CLI := file",
        ] {
            program.execute(statement, 0, &paths).unwrap();
            assert!(
                program.evaluate("$(UNKNOWN_CLI)", &paths).is_err(),
                "{statement}"
            );
        }
        assert!(program
            .execute("UNKNOWN_CLI += $(eval MARK := unsafe)", 1, &paths)
            .is_err());
        program
            .execute(
                "UNKNOWN_CLI := $(eval MARK := measured-effect)unused",
                2,
                &paths,
            )
            .unwrap();
        assert_eq!(
            program.evaluate("$(MARK)", &paths).unwrap(),
            "measured-effect"
        );
        assert!(program.evaluate("$(UNKNOWN_CLI)", &paths).is_err());
        for origin in [
            MakeVariableOrigin::EnvironmentOverride,
            MakeVariableOrigin::Override,
            MakeVariableOrigin::Automatic,
        ] {
            let observed = namespace(&[("VALUE", origin)]);
            let mut unsupported = MakeVariableProgram::new_with_namespace(
                &TargetContext::default(),
                &[],
                &paths,
                &observed,
            )
            .unwrap();
            for statement in [
                "VALUE := file",
                "VALUE = file",
                "undefine VALUE",
                "export VALUE := file",
            ] {
                assert!(unsupported.execute(statement, 0, &paths).is_err());
                assert!(unsupported.evaluate("$(VALUE)", &paths).is_err());
            }
            assert!(unsupported.events().is_empty());
        }
    }

    #[test]
    fn initial_values_refuse_unobserved_conflicting_or_unmodeled_origins() {
        let dirs = DirVars::from_config_text("");
        let paths = paths(&dirs);
        let observed = namespace(&[
            ("VALUE", MakeVariableOrigin::File),
            ("TOP", MakeVariableOrigin::File),
        ]);
        let good = MakeInitialValue {
            name: "VALUE",
            value: "known",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::File,
        };
        for bad in [
            MakeInitialValue {
                name: "ABSENT",
                ..good
            },
            MakeInitialValue {
                name: "TOP",
                ..good
            },
            MakeInitialValue {
                origin: MakeVariableOrigin::Default,
                ..good
            },
            MakeInitialValue {
                value: "bad\0data",
                ..good
            },
        ] {
            assert!(MakeVariableProgram::new_with_initial_values(
                &TargetContext::default(),
                &[],
                &paths,
                &observed,
                &[bad]
            )
            .is_err());
        }
        assert!(MakeVariableProgram::new_with_initial_values(
            &TargetContext::default(),
            &[],
            &paths,
            &observed,
            &[good, good]
        )
        .is_err());
        let mut target = TargetContext::default();
        target
            .make_variables
            .insert("VALUE".into(), "explicit-overlay".into());
        assert!(MakeVariableProgram::new_with_initial_values(
            &target,
            &[],
            &paths,
            &observed,
            &[good]
        )
        .is_err());
        for origin in [
            MakeVariableOrigin::EnvironmentOverride,
            MakeVariableOrigin::Override,
            MakeVariableOrigin::Automatic,
        ] {
            let observed = namespace(&[("VALUE", origin)]);
            assert!(MakeVariableProgram::new_with_initial_values(
                &TargetContext::default(),
                &[],
                &paths,
                &observed,
                &[MakeInitialValue { origin, ..good }]
            )
            .err()
            .unwrap()
            .contains("unsupported origin"));
        }
        let selector = CLOSED_TARGET_MAKE_SELECTORS[0];
        let selector_namespace = namespace(&[(selector, MakeVariableOrigin::File)]);
        assert!(MakeVariableProgram::new_with_initial_values(
            &TargetContext::default(),
            &[],
            &paths,
            &selector_namespace,
            &[MakeInitialValue {
                name: selector,
                ..good
            }]
        )
        .is_err());
        let mut missing = MakeVariableProgram::new_with_namespace(
            &TargetContext::default(),
            &[],
            &paths,
            &observed,
        )
        .unwrap();
        assert!(missing.evaluate("$(VALUE)", &paths).is_err());
    }

    #[test]
    fn initial_value_allocation_and_dollar_escaping_are_bounded() {
        let dirs = DirVars::from_config_text("");
        let paths = paths(&dirs);
        let observed = namespace(&[("VALUE", MakeVariableOrigin::File)]);
        let too_many = "$".repeat(MAX_VALUE / 2 + 1);
        let value = MakeInitialValue {
            name: "VALUE",
            value: &too_many,
            flavor: MakeInitialFlavor::Simple,
            origin: MakeVariableOrigin::File,
        };
        assert!(MakeVariableProgram::new_with_initial_values(
            &TargetContext::default(),
            &[],
            &paths,
            &observed,
            &[value]
        )
        .is_err());
        assert!(MakeVariableProgram::new_with_initial_values(
            &TargetContext::default(),
            &[],
            &paths,
            &observed,
            &[MakeInitialValue {
                flavor: MakeInitialFlavor::Recursive,
                ..value
            }]
        )
        .is_ok());
        let names = (0..8)
            .map(|index| format!("VALUE_{index}"))
            .collect::<Vec<_>>();
        let observed = namespace(
            &names
                .iter()
                .map(|name| (name.as_str(), MakeVariableOrigin::File))
                .collect::<Vec<_>>(),
        );
        let large = "a".repeat(1024 * 1024);
        let values = names
            .iter()
            .map(|name| MakeInitialValue {
                name,
                value: &large,
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::File,
            })
            .collect::<Vec<_>>();
        assert!(MakeVariableProgram::new_with_initial_values(
            &TargetContext::default(),
            &[],
            &paths,
            &observed,
            &values
        )
        .is_err());
    }
}
