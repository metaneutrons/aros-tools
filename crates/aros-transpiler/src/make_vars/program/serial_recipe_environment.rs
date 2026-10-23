//! Measured serial GNU recipe exports, not a general Make engine emulator.
//!
//! Source changes to engine option/level state require separate modeling. The
//! native startup gate already excludes parallel/jobserver options. Ordinary
//! Source exports and explicit export controls still come from the selected
//! target frame; no ambient process environment is consulted.

use super::{evaluate, Evaluation, State};
use aros_common::make_native_invocation::NativeMakeInvocation;
use aros_common::make_startup_arguments::LiteralMakeStartupArguments;
use std::{cell::RefCell, collections::BTreeMap};

const ENGINE_CONTROLS: [&str; 9] = [
    "MAKEFLAGS",
    "MFLAGS",
    "MAKELEVEL",
    "MAKEOVERRIDES",
    "GNUMAKEFLAGS",
    "MAKEFILES",
    "MAKE_TERMOUT",
    "MAKE_TERMERR",
    aros_common::make_initial_values::MAKE_CLI_PROPAGATION_VARIABLE,
];

#[derive(Clone, Eq, PartialEq)]
pub(super) struct SerialRecipeEnvironment {
    invocation_context_sha256: aros_common::Sha256Digest,
    flags: String,
    mflags: String,
    child_level: String,
    export_flags: bool,
    export_mflags: bool,
    shell_export: Option<bool>,
    shell_from_command_line: bool,
}

impl SerialRecipeEnvironment {
    pub(super) fn capture(
        data: &RefCell<State>,
        execution: &Evaluation<'_>,
    ) -> Result<Self, String> {
        {
            let state = data.borrow();
            if state.coordinate_projection || state.initial_namespace.is_none() {
                return Err("serial recipe environment needs measured native initial state".into());
            }
            // `names` contains Source assignments/undefinitions, not installed
            // initial values. Even restoring the initial bytes is not proof
            // that intervening engine option changes had no effect.
            if ENGINE_CONTROLS.iter().any(|name| {
                state.names.contains(*name) || state.target_raw_values.contains_key(*name)
            }) {
                return Err("Source changed Make engine state outside serial recipe policy".into());
            }
            let propagation = aros_common::make_initial_values::MAKE_CLI_PROPAGATION_VARIABLE;
            if state
                .initial_namespace
                .as_ref()
                .and_then(|namespace| namespace.origin(propagation))
                == Some(aros_common::make_namespace::MakeVariableOrigin::Automatic)
                && !state.scope.configuration.contains_key(propagation)
            {
                return Err("serial recipe environment lacks measured GNU CLI propagation".into());
            }
        }
        let value = |name: &str| -> Result<String, String> {
            let value = evaluate(&format!("$({name})"), usize::MAX, data, execution)
                .map_err(|_| "serial recipe engine value is not independently resolved")?;
            if value.len() > 64 * 1024 || value.chars().any(char::is_control) {
                return Err("serial recipe engine value exceeds its admitted domain".into());
            }
            Ok(value)
        };
        let flags = value("MAKEFLAGS")?;
        let mflags = value("MFLAGS")?;
        let level = value("MAKELEVEL")?;
        let next_level = level
            .parse::<u32>()
            .ok()
            .filter(|number| number.to_string() == level)
            .and_then(|number| number.checked_add(1))
            .ok_or("serial recipe MAKELEVEL is not an admitted measured integer")?;
        let state = data.borrow();
        let export_control = |name: &str| match state.exports.get(name) {
            Some(Some(selected)) => Ok(Some(*selected)),
            None => Ok(None),
            Some(None) => Err("serial recipe export control is unresolved".to_owned()),
        };
        Ok(Self {
            invocation_context_sha256: state
                .native_recipe_context_sha256
                .clone()
                .ok_or("serial recipe environment lacks its original invocation binding")?,
            flags,
            mflags,
            child_level: next_level.to_string(),
            export_flags: export_control("MAKEFLAGS")?.unwrap_or(true),
            export_mflags: export_control("MFLAGS")?.unwrap_or(true),
            shell_export: export_control("SHELL")?,
            shell_from_command_line: state.origins.get("SHELL")
                == Some(&aros_common::make_namespace::MakeVariableOrigin::CommandLine),
        })
    }

    pub(super) const fn retained_bytes(&self) -> usize {
        self.flags.len() + self.mflags.len() + self.child_level.len() + 64
    }

    pub(super) fn apply(
        &self,
        source_exports: &BTreeMap<String, String>,
        original: &NativeMakeInvocation,
    ) -> Result<BTreeMap<String, String>, String> {
        if aros_common::sha256_bytes(
            &original
                .to_bytes()
                .map_err(|_| "cannot bind serial recipe invocation")?,
        ) != self.invocation_context_sha256
        {
            return Err("serial recipe invocation differs from its native observation".into());
        }
        LiteralMakeStartupArguments::admit(original.arguments())
            .map_err(|_| "serial recipe startup is not independently admitted")?;
        aros_common::make_observer::validate_source_prelude_inputs(
            original.environment(),
            &BTreeMap::new(),
        )
        .map_err(|_| "serial recipe inherited engine options are unsupported")?;
        // Direct shell execution must not turn GNU's dry-run into a real write.
        // Canonical GNU flags have a compact short-option word before long
        // options/CLI propagation. The original startup gate admits only
        // r/R/n/s and the display-only long option; no jobserver is imported.
        let mut arguments = original.arguments().iter();
        let mut dry_run = false;
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "-f" | "-o" => {
                    arguments.next();
                }
                "-n" => dry_run = true,
                _ => {}
            }
        }
        if dry_run
            || has_dry_run(&self.flags)
            || ["MAKEFLAGS", "MFLAGS"].iter().any(|name| {
                original
                    .environment()
                    .get(*name)
                    .is_some_and(|value| has_dry_run(value))
            })
        {
            return Err("serial recipe refuses execution under native dry-run".into());
        }
        let mut result = source_exports.clone();
        for (name, value, exported) in [
            ("MAKEFLAGS", &self.flags, self.export_flags),
            ("MFLAGS", &self.mflags, self.export_mflags),
        ] {
            if exported {
                result.insert(name.to_owned(), value.clone());
            } else {
                result.remove(name);
            }
        }
        // GNU always supplies the next level to a recipe child, even after an
        // ordinary Source `unexport MAKELEVEL` directive.
        result.insert("MAKELEVEL".into(), self.child_level.clone());
        match self.shell_export {
            Some(true) => {
                if !result.contains_key("SHELL") {
                    return Err("serial recipe lacks its explicitly exported SHELL".into());
                }
            }
            Some(false) => {
                // GNU's saved incoming SHELL is independent of the ordinary
                // variable's export bit, including an explicit unexport.
                if let Some(inherited) = original.environment().get("SHELL") {
                    result.insert("SHELL".into(), inherited.clone());
                } else {
                    result.remove("SHELL");
                }
            }
            None => {
                // SHELL's executable value and its default inherited export
                // are different GNU domains. A file assignment selects the
                // interpreter but does not replace incoming SHELL by itself.
                if let Some(inherited) = original.environment().get("SHELL") {
                    result.insert("SHELL".into(), inherited.clone());
                } else if !self.shell_from_command_line {
                    result.remove("SHELL");
                } else if !result.contains_key("SHELL") {
                    return Err("serial recipe lacks its command-line SHELL export".into());
                }
            }
        }
        Ok(result)
    }
}

fn has_dry_run(flags: &str) -> bool {
    let word = flags
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('-');
    word.bytes().all(|byte| b"rRns".contains(&byte)) && word.contains('n')
}
