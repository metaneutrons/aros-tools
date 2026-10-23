//! Portable, digest-selected native Make observation data, not execution proof.
//!
//! A supervisor supplies the expected binding independently. Neither a document
//! nor its hashes prove the asserted phase, parent, program or execution. Input
//! root/content selections remain a separate caller obligation.

use crate::{
    make_initial_values::{
        ValidatedMakeCommandLineValues, ValidatedMakeEnvironmentValues, ValidatedMakeInitialValues,
        ValidatedMakeParserControls,
    },
    make_namespace::ValidatedMakeNamespace,
    make_native_invocation::NativeMakeInvocation,
    make_startup_arguments::LiteralMakeStartupArguments,
    sha256_bytes, Sha256Digest,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_INVOCATION_BYTES: usize = 2 * 1024 * 1024;
const MAX_STDOUT_BYTES: usize = 256 * 1024;
const MAX_ENVIRONMENT_NAMES: usize = 4096;

/// Distinct instrumented phases; neither denotes an observer-free invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MakeObservationPhase {
    /// The observer runs as `--eval` before the selected file inputs.
    /// That instrumentation can itself affect inherited Make administrative data.
    PreFirstFile,
    /// The observer is a first explicit input, before the selected source input.
    /// This does not assert that no implicit/preloaded input was read.
    FirstInputPrelude,
}

/// Independently selected expected data identities, not authenticated claims.
///
/// `context_sha256` identifies the supervisor's own context record. This type
/// does not interpret or prove that record. The invocation digest covers the
/// exact retained bytes, not a normalized re-encoding of their JSON.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakeObservationBinding {
    pub phase: MakeObservationPhase,
    pub context_sha256: Sha256Digest,
    pub invocation_sha256: Sha256Digest,
    pub stdout_sha256: Sha256Digest,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    binding: MakeObservationBinding,
    invocation: String,
    initial_stdout: String,
    environment_names: Vec<String>,
}

/// One bounded consistent retained packet, with raw values omitted from Debug.
///
/// This is a handoff container, not a successful producer or native-selection
/// capability. A caller must still bind executable/argv0, parent, phase, complete
/// process context, input closure and receipt before relying on native behavior.
pub struct RetainedMakeObservation {
    document: Document,
    invocation: NativeMakeInvocation,
    environment_names: BTreeSet<String>,
}

impl fmt::Debug for RetainedMakeObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RetainedMakeObservation")
            .field("phase", &self.document.binding.phase)
            .finish_non_exhaustive()
    }
}

impl RetainedMakeObservation {
    /// Join actual retained bytes using an independently supplied selection.
    ///
    /// Does not launch Make, read ambient state, or reconstruct missing data.
    ///
    /// # Errors
    /// Refuses changed/oversized data, unsupported startup controls and any
    /// incomplete namespace/default/control/CLI/environment packet.
    pub fn from_captured_bytes(
        invocation: &[u8],
        initial_stdout: &[u8],
        environment_names: &BTreeSet<String>,
        expected: &MakeObservationBinding,
    ) -> Result<Self, String> {
        if invocation.len() > MAX_INVOCATION_BYTES
            || initial_stdout.len() > MAX_STDOUT_BYTES
            || environment_names.len() > MAX_ENVIRONMENT_NAMES
        {
            return Err("native observation exceeds retained data bounds".into());
        }
        crate::make_initial_values::validate_environment_names(environment_names)
            .map_err(|_| "native observation environment membership is unsupported")?;
        let document = Document {
            version: 1,
            binding: expected.clone(),
            invocation: std::str::from_utf8(invocation)
                .map_err(|_| "native observation invocation is not UTF-8")?
                .to_owned(),
            initial_stdout: std::str::from_utf8(initial_stdout)
                .map_err(|_| "native observation stdout is not UTF-8")?
                .to_owned(),
            environment_names: environment_names.iter().cloned().collect(),
        };
        Self::validate(document, expected)
    }

    /// Load exact selected portable bytes, retaining their original inner bytes.
    ///
    /// The document cannot replace the caller's expected binding. Successful
    /// digest comparison establishes consistency only, never who wrote it.
    ///
    /// # Errors
    /// Refuses digest/phase/context mismatch, malformed or duplicate metadata,
    /// unsupported versions and all captured-data validation errors.
    pub fn load(
        bytes: &[u8],
        expected_document_sha256: &Sha256Digest,
        expected: &MakeObservationBinding,
    ) -> Result<Self, String> {
        if bytes.len() > MAX_DOCUMENT_BYTES || sha256_bytes(bytes) != *expected_document_sha256 {
            return Err("retained native observation document differs from selection".into());
        }
        let document = serde_json::from_slice(bytes)
            .map_err(|_| "retained native observation document is malformed")?;
        Self::validate(document, expected)
    }

    fn validate(document: Document, expected: &MakeObservationBinding) -> Result<Self, String> {
        if document.version != 1 || document.binding != *expected {
            return Err("retained native observation binding differs from selection".into());
        }
        if document.invocation.len() > MAX_INVOCATION_BYTES
            || document.initial_stdout.len() > MAX_STDOUT_BYTES
            || document.environment_names.len() > MAX_ENVIRONMENT_NAMES
        {
            return Err("native observation exceeds retained data bounds".into());
        }
        if sha256_bytes(document.invocation.as_bytes()) != expected.invocation_sha256
            || sha256_bytes(document.initial_stdout.as_bytes()) != expected.stdout_sha256
        {
            return Err("retained native observation payload differs from selection".into());
        }
        let environment_names: BTreeSet<_> = document.environment_names.iter().cloned().collect();
        if environment_names.len() != document.environment_names.len() {
            return Err("retained native observation repeats environment membership".into());
        }
        let invocation = NativeMakeInvocation::parse(document.invocation.as_bytes())
            .map_err(|_| "retained native observation invocation is invalid")?;
        validate_packets(
            &invocation,
            document.initial_stdout.as_bytes(),
            &environment_names,
        )?;
        let result = Self {
            document,
            invocation,
            environment_names,
        };
        // Encoding bounds apply to constructors as well as the portable loader.
        result.to_bytes()?;
        Ok(result)
    }

    /// Encode private portable data without dropping or normalizing inner bytes.
    ///
    /// # Errors
    /// Refuses serialization failure or an encoded document above its bound.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let bytes = serde_json::to_vec(&self.document)
            .map_err(|_| "cannot encode retained native observation")?;
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err("retained native observation encoded byte bound".into());
        }
        Ok(bytes)
    }

    #[must_use]
    pub const fn invocation(&self) -> &NativeMakeInvocation {
        &self.invocation
    }

    #[must_use]
    pub const fn initial_stdout(&self) -> &[u8] {
        self.document.initial_stdout.as_bytes()
    }

    #[must_use]
    pub const fn environment_names(&self) -> &BTreeSet<String> {
        &self.environment_names
    }

    #[must_use]
    pub const fn binding(&self) -> &MakeObservationBinding {
        &self.document.binding
    }
}

fn validate_packets(
    invocation: &NativeMakeInvocation,
    stdout: &[u8],
    environment_names: &BTreeSet<String>,
) -> Result<(), String> {
    let startup = LiteralMakeStartupArguments::admit(invocation.arguments())
        .map_err(|_| "retained native observation startup arguments are unsupported")?;
    let namespace = ValidatedMakeNamespace::from_make_stdout(stdout)
        .map_err(|_| "retained native observation namespace is invalid")?;
    if invocation
        .environment()
        .keys()
        .any(|name| !namespace.contains(name))
    {
        return Err("retained native observation original environment is incomplete".into());
    }
    ValidatedMakeInitialValues::from_make_stdout(stdout, &namespace)
        .map_err(|_| "retained native observation default packet is invalid")?;
    ValidatedMakeParserControls::from_make_stdout(stdout, &namespace)
        .map_err(|_| "retained native observation control packet is invalid")?;
    let command = ValidatedMakeCommandLineValues::from_make_stdout(
        stdout,
        &namespace,
        startup.command_line_names(),
    )
    .map_err(|_| "retained native observation CLI packet is invalid")?;
    ValidatedMakeEnvironmentValues::from_make_stdout(stdout, &namespace, environment_names)
        .map_err(|_| "retained native observation environment packet is invalid")?;
    let controls = command
        .entries()
        .map(|entry| (entry.name.clone(), entry.value.clone()))
        .collect();
    crate::make_observer::validate_source_prelude_inputs(invocation.environment(), &controls)
        .map_err(|_| "retained native observation inherited controls are unsupported")?;
    Ok(())
}
