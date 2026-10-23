//! Bounded, data-only handoff for selected native Mesa Make observations.
//!
//! These records establish internal consistency between caller-selected
//! digests and retained bytes. They do not authenticate a producer, prove that
//! a program ran, or establish execution authority or source/build parity.

use crate::{
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    make_startup_arguments::LiteralMakeStartupArguments,
    mesa_configuration::{MesaConfigurationIdentity, MesaMakeCoordinates},
    mesa_genmodule_inventory::MesaGenmoduleProducer,
    sha256_bytes, ArosError, Result, Sha256Digest,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Component, Path, PathBuf},
};

/// Native Mesa context record name.
pub const NATIVE_CONTEXT_FILE: &str = "native-make-context.json";
/// Native Mesa control record name.
pub const NATIVE_CONTROL_FILE: &str = "native-make-control.json";
/// Maximum number of retained native Make calls in one binding.
pub const MAX_NATIVE_CALLS: usize = 256;

const MAX_CONTEXT_BYTES: usize = 64 * 1024;
const MAX_CONTROL_BYTES: usize = 64 * 1024;
const MAX_PACKET_BYTES: usize = 4 * 1024 * 1024;
const MAX_SOURCE_CALL_OWNER_BYTES: usize = 16 * 1024;
const CONTEXT_SCHEMA: &str = "aros-mesa-native-context-v1";

/// Native preparation stages, in their fixed protocol order.
pub const NATIVE_PREPARATION_STAGES: [&str; 3] = [
    "native-root-query",
    "native-host-metamake-build",
    "native-root-launch",
];

/// Return the fixed native Mesa preflight stage sequence for selected producers.
///
/// This names a protocol layout only; it does not claim that any stage ran.
/// Producer owners must be nonempty, safe, strictly sorted and unique.
///
/// # Errors
/// Returns a configuration error for no producers, unsafe owners, or owner
/// order/uniqueness violations.
pub fn native_preflight_stage_names(producers: &[MesaGenmoduleProducer]) -> Result<Vec<String>> {
    if producers.is_empty() {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native preflight requires at least one selected producer",
        ));
    }
    let mut names = vec![
        "configure".to_owned(),
        "feature-inputs".to_owned(),
        "features".to_owned(),
        "configured-values".to_owned(),
    ];
    names.extend(NATIVE_PREPARATION_STAGES.map(str::to_owned));
    names.push("genmodule-inventory".to_owned());
    names.push("genmodule-build".to_owned());

    let mut previous_owner: Option<&str> = None;
    for producer in producers {
        let owner = producer.owner.as_str();
        if previous_owner.is_some_and(|previous| previous >= owner) {
            return Err(native_error(
                NATIVE_CONTROL_FILE,
                "native preflight producer owners are not strictly sorted and unique",
            ));
        }
        crate::mesa_genmodule_collection::mesa_genmodule_output_relative_path(owner).map_err(
            |_| {
                native_error(
                    NATIVE_CONTROL_FILE,
                    "native preflight producer owner is unsafe",
                )
            },
        )?;
        previous_owner = Some(owner);
        names.push(format!("genmodule-make-{owner}"));
    }
    Ok(names)
}

/// A stable file name for one retained native Make call slot.
///
/// # Errors
/// Returns a configuration error for slots outside the bounded call table.
pub fn native_packet_file(slot: usize) -> Result<String> {
    if slot >= MAX_NATIVE_CALLS {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native Make call slot is outside the supported range",
        ));
    }
    Ok(format!("native-make-call-{slot:04}.json"))
}

/// Digest selection for one retained native Make packet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaNativeMakeCallBinding {
    pub document_sha256: Sha256Digest,
    pub binding: MakeObservationBinding,
}

/// Associates a selected genmodule producer with one retained native call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaNativeOwnerCall {
    pub owner: String,
    pub call_index: usize,
}

/// Associates one native Make call with its caller-declared generated source file.
///
/// This is a source-file coordinate only. It does not establish source-graph
/// membership, producer identity, or execution authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaNativeSourceCall {
    pub generated_owner: String,
    pub call_index: usize,
}

/// Caller-selected bindings for native Mesa Make context, control and packets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaNativeMakeBinding {
    pub version: u32,
    pub source_inputs_sha256: Sha256Digest,
    pub compiler_tree_sha256: Sha256Digest,
    pub configuration_stdout_sha256: Sha256Digest,
    pub prepared_tree_sha256: Sha256Digest,
    pub parent_program_sha256: Sha256Digest,
    pub parent_invocation_sha256: Sha256Digest,
    pub parent_environment_sha256: Sha256Digest,
    pub host_metamake_sha256: Sha256Digest,
    pub context_sha256: Sha256Digest,
    pub control_sha256: Sha256Digest,
    pub calls: Vec<MesaNativeMakeCallBinding>,
    pub owners: Vec<MesaNativeOwnerCall>,
    pub source_calls: Vec<MesaNativeSourceCall>,
}

/// The selected identity of one bounded native preparation stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesaNativePreparationStageIdentity {
    pub program: Sha256Digest,
    pub invocation: Sha256Digest,
    pub environment: Sha256Digest,
    pub exit_status: i32,
}

/// Check the fixed root-query, host-metamake-build and root-launch stage chain.
///
/// This compares selected data only. It does not authenticate any stage or
/// establish that a process ran.
///
/// # Errors
/// Returns a configuration error for any changed identity, stage count or
/// required status.
pub fn validate_native_preparation_stages(
    binding: &MesaNativeMakeBinding,
    configured_program: &Sha256Digest,
    configured_environment: &Sha256Digest,
    stages: &[MesaNativePreparationStageIdentity],
) -> Result<()> {
    if binding.parent_program_sha256 != *configured_program {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native parent program differs from configured program",
        ));
    }
    if stages.len() != 3 {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native preparation stage count is invalid",
        ));
    }
    if stages[0].program != *configured_program
        || stages[0].environment != *configured_environment
        || stages[0].exit_status != 2
        || stages[1].program != *configured_program
        || stages[1].environment != *configured_environment
        || stages[1].exit_status != 0
    {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native preparation query/build stage identity is invalid",
        ));
    }
    if stages[2].program != binding.parent_program_sha256
        || stages[2].invocation != binding.parent_invocation_sha256
        || stages[2].environment != binding.parent_environment_sha256
        || stages[2].exit_status != 0
    {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native preparation launch stage identity is invalid",
        ));
    }
    Ok(())
}

/// Encode the fixed, data-only context projection for this binding.
///
/// Control, prepared-tree, call, producer-owner and source-call fields are
/// deliberately excluded so the context does not depend on artifacts or
/// coordinates selected after the context exists.
///
/// # Errors
/// Refuses unsupported binding versions or serialization failure.
pub fn native_make_context_bytes(binding: &MesaNativeMakeBinding) -> Result<Vec<u8>> {
    #[derive(Serialize)]
    struct Context<'a> {
        schema: &'static str,
        source_inputs_sha256: &'a Sha256Digest,
        compiler_tree_sha256: &'a Sha256Digest,
        configuration_stdout_sha256: &'a Sha256Digest,
        parent_program_sha256: &'a Sha256Digest,
        parent_invocation_sha256: &'a Sha256Digest,
        parent_environment_sha256: &'a Sha256Digest,
        host_metamake_sha256: &'a Sha256Digest,
    }

    if binding.version != 1 {
        return Err(native_error(
            NATIVE_CONTEXT_FILE,
            "unsupported native Make binding version",
        ));
    }

    serde_json::to_vec(&Context {
        schema: CONTEXT_SCHEMA,
        source_inputs_sha256: &binding.source_inputs_sha256,
        compiler_tree_sha256: &binding.compiler_tree_sha256,
        configuration_stdout_sha256: &binding.configuration_stdout_sha256,
        parent_program_sha256: &binding.parent_program_sha256,
        parent_invocation_sha256: &binding.parent_invocation_sha256,
        parent_environment_sha256: &binding.parent_environment_sha256,
        host_metamake_sha256: &binding.host_metamake_sha256,
    })
    .map_err(|_| native_error(NATIVE_CONTEXT_FILE, "cannot encode native Make context"))
}

/// Bounded retained native Make packets selected by a caller binding.
///
/// Raw packet bytes are private and intentionally omitted from Debug output.
#[derive(Clone)]
pub struct LoadedMesaNativeMake {
    binding: MesaNativeMakeBinding,
    coordinates: MesaMakeCoordinates,
    packets: Vec<Vec<u8>>,
}

impl fmt::Debug for LoadedMesaNativeMake {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoadedMesaNativeMake")
            .field("binding", &self.binding)
            .field("packet_count", &self.packets.len())
            .finish_non_exhaustive()
    }
}

impl LoadedMesaNativeMake {
    /// Return the caller-supplied binding retained with the selected packets.
    #[must_use]
    pub const fn binding(&self) -> &MesaNativeMakeBinding {
        &self.binding
    }

    /// Return the validated Make coordinates retained with these packets.
    #[must_use]
    pub const fn coordinates(&self) -> &MesaMakeCoordinates {
        &self.coordinates
    }

    /// Revalidate and return a retained packet by call slot, if present.
    ///
    /// # Errors
    /// Returns a configuration error if retained bytes no longer match the
    /// immutable caller binding or fail packet validation.
    pub fn observation(&self, slot: usize) -> Result<Option<RetainedMakeObservation>> {
        let Some(call) = self.binding.calls.get(slot) else {
            return Ok(None);
        };
        let Some(bytes) = self.packets.get(slot) else {
            return Err(native_error(
                NATIVE_CONTROL_FILE,
                "native retained call is absent",
            ));
        };
        let observation =
            RetainedMakeObservation::load(bytes, &call.document_sha256, &call.binding).map_err(
                |_| native_error(NATIVE_CONTROL_FILE, "native retained packet is invalid"),
            )?;
        Ok(Some(observation))
    }

    /// Revalidate and return a selected owner's retained packet, if present.
    ///
    /// # Errors
    /// Returns a configuration error if retained bytes no longer match the
    /// immutable caller binding or fail packet validation.
    pub fn observation_for_owner(&self, owner: &str) -> Result<Option<RetainedMakeObservation>> {
        let Some(owner_binding) = self
            .binding
            .owners
            .iter()
            .find(|entry| entry.owner == owner)
        else {
            return Ok(None);
        };
        self.observation(owner_binding.call_index)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeControl {
    version: u32,
    phase: MakeObservationPhase,
    maximum_calls: usize,
    output_root: PathBuf,
    program: PathBuf,
    executable_sha256: Sha256Digest,
    context_sha256: Sha256Digest,
    argument_zero: String,
}

/// Read and validate the selected native context, control and call packets.
///
/// The callback supplies only bounded bytes. The result retains exact packet
/// bytes and permits later consistency revalidation. It does not authenticate
/// files or producers, prove process execution, or check the prepared tree's
/// actual contents; callers must compare that tree separately.
///
/// # Errors
/// Rejects identity mismatch, unsupported binding structure, malformed or
/// oversized documents, digest mismatch, invalid packet data, and
/// producer/source-call cwd disagreement.
pub fn load_native_make(
    binding: &MesaNativeMakeBinding,
    identity: &MesaConfigurationIdentity,
    configured_stdout: &Sha256Digest,
    coordinates: &MesaMakeCoordinates,
    producers: &[MesaGenmoduleProducer],
    mut read: impl FnMut(&str, u64) -> Result<Vec<u8>>,
) -> Result<LoadedMesaNativeMake> {
    if identity.source_inputs_sha256 != binding.source_inputs_sha256
        || identity.compiler_tree_sha256 != binding.compiler_tree_sha256
        || configured_stdout != &binding.configuration_stdout_sha256
    {
        return Err(native_error(
            NATIVE_CONTEXT_FILE,
            "native Make identity differs from configured selection",
        ));
    }
    validate_binding_shape(binding, producers)?;
    coordinates
        .validate()
        .map_err(|_| native_error(NATIVE_CONTROL_FILE, "native Make coordinates are invalid"))?;

    let context = native_make_context_bytes(binding)?;
    if context.len() > MAX_CONTEXT_BYTES {
        return Err(native_error(
            NATIVE_CONTEXT_FILE,
            "native Make context exceeds its byte limit",
        ));
    }
    let context_bytes = read_bounded(
        &mut read,
        NATIVE_CONTEXT_FILE,
        MAX_CONTEXT_BYTES,
        NATIVE_CONTEXT_FILE,
    )?;
    if sha256_bytes(&context_bytes) != binding.context_sha256 || context_bytes != context {
        return Err(native_error(
            NATIVE_CONTEXT_FILE,
            "native Make context differs from its selected bytes",
        ));
    }

    let control_bytes = read_bounded(
        &mut read,
        NATIVE_CONTROL_FILE,
        MAX_CONTROL_BYTES,
        NATIVE_CONTROL_FILE,
    )?;
    if sha256_bytes(&control_bytes) != binding.control_sha256 {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native Make control digest differs from selection",
        ));
    }
    let control: NativeControl = serde_json::from_slice(&control_bytes)
        .map_err(|_| native_error(NATIVE_CONTROL_FILE, "native Make control is malformed"))?;
    if control.version != 4
        || control.phase != MakeObservationPhase::FirstInputPrelude
        || control.maximum_calls != binding.calls.len()
        || control.context_sha256 != binding.context_sha256
        || control.executable_sha256 != binding.parent_program_sha256
        || control.argument_zero.is_empty()
        || control.argument_zero.contains('\0')
    {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native Make control differs from selected binding",
        ));
    }
    let expected_output_root = expected_output_root(coordinates)?;
    if exact_normalized_absolute(&control.output_root).as_deref() != expected_output_root.to_str() {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native Make output root differs from selected coordinates",
        ));
    }
    if exact_normalized_absolute(&control.program).is_none() {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native Make program path is not normalized UTF-8",
        ));
    }

    let producer_by_owner: BTreeMap<_, _> = producers
        .iter()
        .map(|producer| (producer.owner.as_str(), producer))
        .collect();
    let mut packets = Vec::with_capacity(binding.calls.len());
    for (slot, call) in binding.calls.iter().enumerate() {
        let file = native_packet_file(slot)?;
        let bytes = read_bounded(&mut read, &file, MAX_PACKET_BYTES, &file)?;
        let observation =
            RetainedMakeObservation::load(&bytes, &call.document_sha256, &call.binding)
                .map_err(|_| native_error(&file, "native retained packet is invalid"))?;
        if call.binding.phase != control.phase
            || call.binding.context_sha256 != control.context_sha256
            || observation.binding().phase != control.phase
            || observation.binding().context_sha256 != control.context_sha256
            || observation.invocation().argument_zero() != Some(control.argument_zero.as_str())
        {
            return Err(native_error(
                &file,
                "native retained packet differs from control selection",
            ));
        }
        let source_call = binding.source_calls.get(slot).ok_or_else(|| {
            native_error(
                NATIVE_CONTROL_FILE,
                "native source call roster is incomplete",
            )
        })?;
        validate_source_call(source_call, coordinates, observation.invocation())?;
        packets.push(bytes);
    }

    for owner in &binding.owners {
        let producer = producer_by_owner.get(owner.owner.as_str()).ok_or_else(|| {
            native_error(
                NATIVE_CONTROL_FILE,
                "native owner is absent from selected producers",
            )
        })?;
        validate_declaration_directory(&producer.declaration_directory)?;
        let expected_cwd = Path::new(&coordinates.build_root).join(&producer.declaration_directory);
        let actual_cwd = RetainedMakeObservation::load(
            &packets[owner.call_index],
            &binding.calls[owner.call_index].document_sha256,
            &binding.calls[owner.call_index].binding,
        )
        .map_err(|_| {
            native_error(
                NATIVE_CONTROL_FILE,
                "native retained packet is invalid during owner validation",
            )
        })?;
        if exact_normalized_absolute(actual_cwd.invocation().working_directory()).as_deref()
            != expected_cwd.to_str()
        {
            return Err(native_error(
                NATIVE_CONTROL_FILE,
                "native owner working directory differs from selected producer",
            ));
        }
    }

    Ok(LoadedMesaNativeMake {
        binding: binding.clone(),
        coordinates: coordinates.clone(),
        packets,
    })
}

fn validate_binding_shape(
    binding: &MesaNativeMakeBinding,
    producers: &[MesaGenmoduleProducer],
) -> Result<()> {
    if binding.version != 1 {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "unsupported native Make binding version",
        ));
    }
    if !(1..=MAX_NATIVE_CALLS).contains(&binding.calls.len()) {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native Make call count is outside the supported range",
        ));
    }
    let mut expected_owners = BTreeSet::new();
    for producer in producers {
        if !expected_owners.insert(producer.owner.as_str()) {
            return Err(native_error(
                NATIVE_CONTROL_FILE,
                "selected native Make producers repeat an owner",
            ));
        }
    }
    let mut previous_owner: Option<&str> = None;
    let mut actual_owners = BTreeSet::new();
    for owner in &binding.owners {
        if previous_owner.is_some_and(|previous| previous >= owner.owner.as_str()) {
            return Err(native_error(
                NATIVE_CONTROL_FILE,
                "native Make owners are not strictly sorted and unique",
            ));
        }
        previous_owner = Some(&owner.owner);
        if owner.call_index >= binding.calls.len() {
            return Err(native_error(
                NATIVE_CONTROL_FILE,
                "native Make owner references an absent call",
            ));
        }
        actual_owners.insert(owner.owner.as_str());
    }
    if actual_owners != expected_owners {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native Make owners do not cover selected producers exactly",
        ));
    }
    if binding.source_calls.len() != binding.calls.len() {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native Make source call roster does not cover every call",
        ));
    }
    let mut source_owner_bytes = 0usize;
    for (slot, source_call) in binding.source_calls.iter().enumerate() {
        if source_call.call_index != slot {
            return Err(native_error(
                NATIVE_CONTROL_FILE,
                "native Make source call indexes are not dense and ordered",
            ));
        }
        source_owner_bytes = source_owner_bytes
            .checked_add(source_call.generated_owner.len())
            .filter(|bytes| *bytes <= MAX_SOURCE_CALL_OWNER_BYTES)
            .ok_or_else(|| {
                native_error(
                    NATIVE_CONTROL_FILE,
                    "native Make source call owner paths exceed their byte limit",
                )
            })?;
        validate_generated_owner(&source_call.generated_owner)?;
    }
    Ok(())
}

fn validate_source_call(
    source_call: &MesaNativeSourceCall,
    coordinates: &MesaMakeCoordinates,
    invocation: &crate::make_native_invocation::NativeMakeInvocation,
) -> Result<()> {
    let generated_owner = Path::new(&source_call.generated_owner);
    let generated_path = Path::new(&coordinates.build_root).join(generated_owner);
    let generated_parent = generated_owner.parent().unwrap_or_else(|| Path::new(""));
    let expected_cwd = Path::new(&coordinates.build_root).join(generated_parent);
    if invocation.working_directory() != expected_cwd {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native source call working directory differs from its generated source owner",
        ));
    }

    let startup = LiteralMakeStartupArguments::admit(invocation.arguments()).map_err(|_| {
        native_error(
            NATIVE_CONTROL_FILE,
            "native source call startup arguments are unsupported",
        )
    })?;
    if startup.input_files().len() != 1 || startup.goals().is_empty() {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native source call requires one explicit input and at least one goal",
        ));
    }
    let input = startup.input_files()[0].as_str();
    if !is_lexically_normalized_input(input) {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native source call explicit input is not lexically normalized",
        ));
    }
    let input_path = Path::new(input);
    if invocation.working_directory().join(input_path) != generated_path {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native source call explicit input differs from its generated source owner",
        ));
    }
    Ok(())
}

fn validate_generated_owner(value: &str) -> Result<()> {
    if value.is_empty()
        || value.contains(['\\', '\0'])
        || value
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        || Path::new(value).is_absolute()
    {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "native source call generated owner is not a normalized relative path",
        ));
    }
    Ok(())
}

fn is_lexically_normalized_input(value: &str) -> bool {
    if value.is_empty() || value.contains(['\\', '\0']) {
        return false;
    }
    let path = Path::new(value);
    let body = if path.is_absolute() {
        value.strip_prefix('/').unwrap_or_default()
    } else {
        value
    };
    if body.is_empty()
        || body
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return false;
    }
    true
}

fn read_bounded(
    read: &mut impl FnMut(&str, u64) -> Result<Vec<u8>>,
    path: &str,
    max_bytes: usize,
    error_file: &str,
) -> Result<Vec<u8>> {
    let bytes = read(
        path,
        u64::try_from(max_bytes).expect("configured byte bound fits u64"),
    )
    .map_err(|_| native_error(error_file, "cannot read selected native Make data"))?;
    if bytes.len() > max_bytes {
        return Err(native_error(
            error_file,
            "selected native Make data exceeds its byte limit",
        ));
    }
    Ok(bytes)
}

fn expected_output_root(coordinates: &MesaMakeCoordinates) -> Result<PathBuf> {
    let build_root = Path::new(&coordinates.build_root);
    let parent = build_root.parent().ok_or_else(|| {
        native_error(
            NATIVE_CONTROL_FILE,
            "native Make build root has no parent directory",
        )
    })?;
    Ok(parent.join("native-make-calls"))
}

fn exact_normalized_absolute(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    if !path.is_absolute() || text.contains('\0') {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normalized = PathBuf::from(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    (normalized.to_str()? == text).then(|| text.to_owned())
}

fn validate_declaration_directory(value: &str) -> Result<()> {
    if value.is_empty()
        || value.split('/').any(|segment| {
            segment.is_empty()
                || segment == "."
                || segment == ".."
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+".contains(&byte))
        })
    {
        return Err(native_error(
            NATIVE_CONTROL_FILE,
            "selected producer declaration directory is not normalized",
        ));
    }
    Ok(())
}

fn native_error(file: &str, message: &'static str) -> ArosError {
    ArosError::Configuration {
        file: file.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
#[path = "mesa_native_make_tests.rs"]
pub(crate) mod tests;
