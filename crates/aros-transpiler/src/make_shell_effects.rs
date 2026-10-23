//! Explicit caller-owned shell effects during staged Source interpretation.
//!
//! This is neither a launcher nor a portable packet authentication mechanism.
//! A live provider must belong to the original owning input/runtime transaction;
//! a data provider must independently bind its complete ordered observations.
//! Merely implementing this trait does not prove either condition. No provider
//! is inferred from Source text, executable names, hashes or ambient state.

use crate::make_shell_data::ShellDataOutput;
use std::{collections::BTreeMap, path::Path};

pub mod gnu_query;

/// Independently selected controls/provider for one native child read session.
///
/// Its phase-bearing binding is data, not owner/runtime authentication or
/// launch authority. The caller must independently select that expectation.
pub struct ShellEffectSelection<'a> {
    /// Independently selected expected child binding, not authority from data.
    pub observation: &'a aros_common::make_native_observation::MakeObservationBinding,
    pub shell: &'a str,
    pub flags: &'a [&'a str],
    pub provider: &'a dyn CapturedShellEffects,
    pub search: Option<std::sync::Arc<dyn crate::make_vars::CapturedShellSearch>>,
}

/// Preserve whether GNU has already normalized the selected process output.
/// A GNU query result must never be mislabeled as raw process stdout.
#[derive(Debug)]
pub enum ShellEffectOutput {
    Raw(ShellDataOutput),
    GnuValue(gnu_query::GnuShellQueryResult),
}

impl From<ShellDataOutput> for ShellEffectOutput {
    fn from(output: ShellDataOutput) -> Self {
        Self::Raw(output)
    }
}

impl ShellEffectOutput {
    pub(crate) fn into_value(self) -> Result<(gnu_query::GnuShellQueryResult, usize), String> {
        let (text, status, folded) = match self {
            Self::Raw(output) => (output.stdout, output.status, false),
            Self::GnuValue(output) => (output.value, output.status, true),
        };
        let bytes = text.len();
        if bytes > 1024 * 1024 || text.contains('\0') || folded && text.contains('\n') {
            return Err("observed shell output exceeds its admitted text domain".into());
        }
        let value = if folded {
            text
        } else {
            crate::make_shell_data::fold_stdout(&text)
        };
        Ok((gnu_query::GnuShellQueryResult { value, status }, bytes))
    }
}

/// Exact expanded effect and Source-derived controls, not a reconstructed argv.
///
/// The ordinal is zero-based within this complete selected file-read session.
/// The export map is NOT GNU Make's complete child process environment: a live
/// provider must independently establish engine-managed variables and process
/// context, or refuse. It must not launch directly from this map by assumption.
pub struct ShellEffectRequest<'a> {
    /// Present only for a native session whose child binding matched the
    /// selected expectation. Providers must compare their own expected context;
    /// this data does not authenticate the owning process or runtime.
    pub native_context: Option<&'a aros_common::make_native_observation::MakeObservationBinding>,
    pub ordinal: usize,
    pub command: &'a str,
    pub working_directory: &'a Path,
    pub shell: &'a str,
    pub shell_flags: &'a str,
    pub source_exports: &'a BTreeMap<String, String>,
    /// Explicit Source export/unexport directives at this read position.
    /// Absence is not an unexport: GNU may export an engine-owned variable
    /// even when it is absent from the ordinary Source export map.
    pub source_export_controls: &'a BTreeMap<String, bool>,
}

impl std::fmt::Debug for ShellEffectRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellEffectRequest")
            .field("ordinal", &self.ordinal)
            .field("native_context_present", &self.native_context.is_some())
            .field("command_bytes", &self.command.len())
            .field("source_export_entries", &self.source_exports.len())
            .field("source_export_controls", &self.source_export_controls.len())
            .finish_non_exhaustive()
    }
}

/// Independently selected provider for one complete staged file-read session.
///
/// The provider binds original owner/invocation/phase, Source/input roots and
/// actual script/interpreter/runtime identities, not only a shell path. Query
/// matching includes order, command, cwd, shell/flags, exact Source exports and
/// explicit export/unexport selection,
/// plus independently established full process context and environment.
/// A retained result may not substitute its own claimed context for independent
/// expected policy. A live provider needs explicit owning execution authority,
/// deadline/cancellation and bounded process/output handling. It must never
/// obtain permission merely because the Source expression was unsupported.
///
/// Provider state is session-local: refusal is not permission to resume a failed
/// original owner or silently reuse consumed observations. Implementations must
/// not publish partial results or expose raw command/output in errors. This
/// boundary is not an OS sandbox or evidence of GNU phase equivalence. The
/// session covers only effects expanded during the complete selected file
/// read, not shell calls deferred until later recipe or recursive expansion.
pub trait CapturedShellEffects {
    /// Recheck the provider's independent inputs, runtime and original owner.
    /// Called before and after interpretation, including a failed read.
    /// # Errors
    /// Refuses drift, stale ownership, cancellation or exhausted budget.
    fn revalidate(&self) -> Result<(), String>;

    /// Supply bounded raw stdout or explicitly GNU-folded value and status for
    /// this exact independently bound effect.
    /// No fallback or expression-specific output is synthesized by the reader.
    /// # Errors
    /// Refuses absent, duplicate, out-of-order or mismatched observations and
    /// unauthorized or failed execution. Raw values must not appear in errors.
    fn query(&self, request: &ShellEffectRequest<'_>) -> Result<ShellEffectOutput, String>;

    /// Verify complete session coverage before publishing interpreted state.
    /// # Errors
    /// Refuses missing/unused effects, incomplete execution or result retention.
    fn finish(&self, effects: usize) -> Result<(), String>;
}
