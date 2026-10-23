//! Consistent packet composition, not native invocation authentication.

use std::collections::BTreeSet;

use aros_common::{
    make_initial_values::{
        MakeInitialValueFlavor, ValidatedMakeCommandLineValues, ValidatedMakeEnvironmentValues,
        ValidatedMakeInitialValues, ValidatedMakeParserControls, ValidatedMakeRecipeControls,
    },
    make_input_snapshot::{MakeInputReader, MakeInputSnapshot, VerifiedMakeInputReads},
    make_namespace::ValidatedMakeNamespace,
    make_startup_arguments::LiteralMakeStartupArguments,
};

use super::{MakeInitialFlavor, MakeInitialValue, MakeProgramPaths, MakeVariableProgram};

impl MakeVariableProgram {
    /// Interpret the original native invocation's explicit source inputs.
    ///
    /// Initial origin/flavor/raw data comes from the bound native observation;
    /// every explicit input is selected from admitted original argv in order,
    /// never from an owner/CPU/path table. Includes, wildcard and realpath use
    /// the caller's captured roots. The observation phase, source-root identities
    /// and full native argv/env/owner/receipt still require independent caller
    /// binding. This consistency reader launches no Make and executes no recipes.
    ///
    /// # Errors
    /// Refuses inconsistent startup data, native control effects, unsafe/changed
    /// input roots, unknown source semantics and exhausted interpreter budgets.
    pub fn read_native_captured_inputs(
        paths: &MakeProgramPaths<'_>,
        stdout: &[u8],
        invocation: &aros_common::make_native_invocation::NativeMakeInvocation,
        expected_environment_names: &BTreeSet<String>,
        snapshot: &MakeInputSnapshot,
    ) -> Result<(Self, super::MakeFileProgram, VerifiedMakeInputReads), String> {
        let ((program, file, _file_retained), reads) = snapshot.transaction(|reader| {
            Self::read_native_captured_inputs_in_reader(
                paths,
                stdout,
                invocation,
                expected_environment_names,
                reader,
            )
        })?;
        Ok((program, file, reads))
    }

    /// Compose and parse native startup inputs through a caller-owned reader.
    ///
    /// This performs startup admission and initial packet validation exactly
    /// as the standalone entry point does, while delegating all file, include,
    /// wildcard and realpath reads to the enclosing transaction. The caller
    /// must verify the whole captured roots before and after its full operation
    /// and retain independent invocation/phase authentication.
    ///
    /// # Errors
    /// Refuses inconsistent startup data, native control effects, unsafe or
    /// changed reads, unknown source semantics and exhausted interpreter limits.
    pub(crate) fn read_native_captured_inputs_in_reader(
        paths: &MakeProgramPaths<'_>,
        stdout: &[u8],
        invocation: &aros_common::make_native_invocation::NativeMakeInvocation,
        expected_environment_names: &BTreeSet<String>,
        reader: &mut MakeInputReader<'_>,
    ) -> Result<(Self, super::MakeFileProgram, usize), String> {
        Self::read_native_captured_inputs_in_selected_reader(
            paths,
            stdout,
            invocation,
            expected_environment_names,
            reader,
            None,
        )
    }

    pub(crate) fn read_native_captured_inputs_in_reader_with_shell_effects(
        paths: &MakeProgramPaths<'_>,
        observation: &aros_common::make_native_observation::RetainedMakeObservation,
        reader: &mut MakeInputReader<'_>,
        effects: &crate::make_shell_effects::ShellEffectSelection<'_>,
    ) -> Result<(Self, super::MakeFileProgram, usize), String> {
        // Bind exact retained identities, never replace an invocation's byte
        // digest with a hash of its parsed/re-serialized representation.
        if effects.observation != observation.binding() {
            return Err("native shell effect selection differs from child inputs".into());
        }
        Self::read_native_captured_inputs_in_selected_reader(
            paths,
            observation.initial_stdout(),
            observation.invocation(),
            observation.environment_names(),
            reader,
            Some(effects),
        )
    }

    fn read_native_captured_inputs_in_selected_reader(
        paths: &MakeProgramPaths<'_>,
        stdout: &[u8],
        invocation: &aros_common::make_native_invocation::NativeMakeInvocation,
        expected_environment_names: &BTreeSet<String>,
        reader: &mut MakeInputReader<'_>,
        effects: Option<&crate::make_shell_effects::ShellEffectSelection<'_>>,
    ) -> Result<(Self, super::MakeFileProgram, usize), String> {
        let startup = LiteralMakeStartupArguments::admit(invocation.arguments())
            .map_err(|_| "unsupported native invocation startup arguments")?;
        let assignments = ValidatedMakeCommandLineValues::from_make_stdout(
            stdout,
            &ValidatedMakeNamespace::from_make_stdout(stdout)
                .map_err(|_| "invalid native initial namespace")?,
            startup.command_line_names(),
        )
        .map_err(|_| "invalid native command-line packet")?;
        let controls = assignments
            .entries()
            .map(|entry| (entry.name.clone(), entry.value.clone()))
            .collect();
        aros_common::make_observer::validate_source_prelude_inputs(
            invocation.environment(),
            &controls,
        )
        .map_err(|_| "unsupported native inherited startup controls")?;
        let mut program = Self::from_native_invocation_observation(
            paths,
            stdout,
            invocation,
            expected_environment_names,
        )?;
        let inputs = startup
            .input_files()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let (file, file_retained) = if let Some(effects) = effects {
            if effects.observation.phase
                != aros_common::make_native_observation::MakeObservationPhase::FirstInputPrelude
                || effects.observation.stdout_sha256 != aros_common::sha256_bytes(stdout)
            {
                return Err("native shell effect selection differs from child inputs".into());
            }
            let namespace = ValidatedMakeNamespace::from_make_stdout(stdout)
                .map_err(|_| "invalid native initial namespace")?;
            ValidatedMakeRecipeControls::from_make_stdout(stdout, &namespace)
                .map_err(|_| "native shell effects need measured recipe controls")?;
            program.require_captured_shell_policy(
                effects.shell,
                effects.flags,
                invocation.environment(),
            )?;
            if let Some(search) = &effects.search {
                program.bind_captured_shell_source_path_prefixes(std::sync::Arc::clone(search))?;
            }
            program
                .read_captured_files_in_reader_with_shell_effects(&inputs, paths, reader, effects)?
        } else {
            program.read_captured_files_in_reader(&inputs, paths, reader)?
        };
        Ok((program, file, file_retained))
    }

    /// Compose a source program using the literal native child's startup data.
    ///
    /// The original environment membership comes from that child snapshot,
    /// not a reconstructed root environment or the current process. Argument
    /// order is admitted unchanged. The independently supplied execution paths
    /// must retain the captured cwd; source/build roots are never inferred from
    /// command-line variables. The stdout phase and its complete native-created
    /// environment membership still require independent binding by the caller.
    /// Value/flavor/origin come from GNU's observed packets, not a guessed
    /// environment-import algorithm. The caller must bind the entire captured
    /// argv/environment bytes to that observation; membership checks alone do
    /// not detect a stale value packet or establish native execution.
    /// This is data composition, not authentication or permission to run Make.
    ///
    /// # Errors
    /// Refuses a different cwd, unsupported startup syntax and every invalid
    /// or incomplete native observation described by `from_native_source_observation`.
    pub fn from_native_invocation_observation(
        paths: &MakeProgramPaths<'_>,
        stdout: &[u8],
        invocation: &aros_common::make_native_invocation::NativeMakeInvocation,
        expected_environment_names: &BTreeSet<String>,
    ) -> Result<Self, String> {
        if invocation.working_directory() != paths.working_directory {
            return Err("native invocation cwd differs from source-program paths".into());
        }
        let startup = LiteralMakeStartupArguments::admit(invocation.arguments())
            .map_err(|_| "unsupported native invocation startup arguments")?;
        let original_environment_names = invocation.environment().keys().cloned().collect();
        let mut program = Self::from_native_source_observation(
            paths,
            stdout,
            &startup,
            &original_environment_names,
            expected_environment_names,
        )?;
        // An additional typed-context join; the original owner's raw packet
        // binding remains independently required and is never replaced by it.
        program.state.native_recipe_context_sha256 = Some(aros_common::sha256_bytes(
            &invocation
                .to_bytes()
                .map_err(|_| "cannot bind native invocation bytes")?,
        ));
        Ok(program)
    }

    /// Compose native initial values from one explicitly selected stdout packet.
    ///
    /// All namespace/default/CLI/environment records must belong to the same
    /// observation. CLI membership comes from admitted original argv; complete
    /// environment-origin membership comes from an independently bound native
    /// namespace observation at that exact phase/context. Original process env
    /// names are separately supplied to preserve export history hidden by CLI
    /// overrides. Native-created environment-origin names are retained as
    /// measured data, never supplied from a fixed engine-name/value table.
    ///
    /// This neither reads the ambient environment nor authenticates the producer,
    /// exact argv/env/cwd/files, observation phase or receipt. The caller must
    /// prove those bindings and keep raw stdout private. Parser controls use a
    /// separate packet; other dot values excluded by the legacy packet remain
    /// unresolved. No default control value is guessed.
    ///
    /// # Errors
    /// Refuses malformed/inconsistent/missing packets, membership/origin/flavor
    /// mismatches, unsupported names and invalid or over-budget initial state.
    pub fn from_native_source_observation(
        paths: &MakeProgramPaths<'_>,
        stdout: &[u8],
        startup: &LiteralMakeStartupArguments,
        original_environment_names: &BTreeSet<String>,
        expected_environment_names: &BTreeSet<String>,
    ) -> Result<Self, String> {
        let namespace = ValidatedMakeNamespace::from_make_stdout(stdout)
            .map_err(|_| "invalid native initial namespace")?;
        if original_environment_names.len() > namespace.to_observation().entries.len()
            || original_environment_names
                .iter()
                .any(|name| !namespace.contains(name))
        {
            return Err("original environment membership differs from native namespace".into());
        }
        let defaults = ValidatedMakeInitialValues::from_make_stdout(stdout, &namespace)
            .map_err(|_| "invalid native default-value packet")?;
        let controls = ValidatedMakeParserControls::from_make_stdout(stdout, &namespace)
            .map_err(|_| "invalid native parser-control packet")?;
        // Separate opt-in packet: historical default/parser packets do not
        // acquire guessed dot-variable defaults or new required membership.
        let recipe_prefix = format!(
            "{}=",
            aros_common::make_initial_values::MAKE_RECIPE_CONTROLS_METADATA_KEY
        );
        let recipe_controls = stdout
            .split(|byte| *byte == b'\n')
            .any(|line| line.starts_with(recipe_prefix.as_bytes()))
            .then(|| {
                aros_common::make_initial_values::ValidatedMakeRecipeControls::from_make_stdout(
                    stdout, &namespace,
                )
                .map_err(|_| "invalid native recipe-control packet")
            })
            .transpose()?;
        // A separate opt-in packet supplies only GNU's immutable automatic
        // CLI-propagation scalar. Historical packets leave it unresolved;
        // ordinary automatic variables still have no initial-value admission.
        let engine_prefix = format!(
            "{}=",
            aros_common::make_initial_values::MAKE_RECIPE_ENGINE_VALUES_METADATA_KEY
        );
        let engine_values = stdout
            .split(|byte| *byte == b'\n')
            .any(|line| line.starts_with(engine_prefix.as_bytes()))
            .then(|| {
                aros_common::make_initial_values::ValidatedMakeRecipeEngineValues::from_make_stdout(
                    stdout, &namespace,
                )
                .map_err(|_| "invalid native recipe-engine packet")
            })
            .transpose()?;
        let command = ValidatedMakeCommandLineValues::from_make_stdout(
            stdout,
            &namespace,
            startup.command_line_names(),
        )
        .map_err(|_| "invalid native command-line packet")?;
        let environment = ValidatedMakeEnvironmentValues::from_make_stdout(
            stdout,
            &namespace,
            expected_environment_names,
        )
        .map_err(|_| "invalid native environment-value packet")?;
        let values = defaults
            .entries()
            .chain(controls.entries())
            .chain(
                recipe_controls
                    .iter()
                    .flat_map(ValidatedMakeRecipeControls::entries),
            )
            .chain(command.entries())
            .chain(environment.entries())
            .map(|entry| MakeInitialValue {
                name: &entry.name,
                value: &entry.value,
                flavor: match entry.flavor {
                    MakeInitialValueFlavor::Simple => MakeInitialFlavor::Simple,
                    MakeInitialValueFlavor::Recursive => MakeInitialFlavor::Recursive,
                },
                origin: entry.origin,
            })
            .collect::<Vec<_>>();
        let environment_names = original_environment_names
            .union(expected_environment_names)
            .cloned()
            .collect();
        let mut program = Self::new_source_program_with_environment(
            paths,
            &namespace,
            &values,
            &environment_names,
        )?;
        if let Some(engine_values) = engine_values {
            super::initial::install_recipe_engine(&mut program.state, &namespace, &engine_values)?;
        }
        Ok(program)
    }
}

#[cfg(test)]
#[path = "native_capture_bridge_tests.rs"]
mod native_capture_bridge_tests;

#[cfg(test)]
#[path = "native_input_reader_tests.rs"]
mod native_input_reader_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use aros_common::make_namespace::MakeVariableOrigin;

    #[test]
    fn borrowed_native_initial_values_do_not_disclose_raw_data_in_debug() {
        let binding = MakeInitialValue {
            name: "SYNTHETIC",
            value: "synthetic-private-native-value-911d",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        };
        let diagnostic = format!("{binding:?}");
        assert!(!diagnostic.contains(binding.value));
        assert!(diagnostic.contains("value_bytes"));
    }
}
