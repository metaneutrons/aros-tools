//! Bounded CLI read-back for one source-selected SDK generation closure.
//!
//! This command joins independently selected release inputs, compiler and SDK
//! package bytes, compatibility evidence, and one generation record. It never
//! executes a compiler or authenticates the job that produced these bytes.

#[cfg(unix)]
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use aros_common::Sha256Digest;
use clap::Args;
use miette::Result;
use serde::Deserialize;

const SELECTION_SCHEMA: &str = "aros-sdk-generation-readback-selection-v1";
const INPUT_COLLECTION_FILE: &str = "toolchain-release-inputs-v2.json";
const MAX_SELECTED_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Explicit inputs for SDK generation read-back. This operation is read-only.
#[derive(Args)]
pub struct VerifyGenerationArgs {
    /// Directory containing the independently selected release-input collection
    #[arg(long, value_name = "DIR")]
    inputs_dir: PathBuf,
    /// SHA-256 of the exact release-input collection bytes
    #[arg(long, value_name = "SHA256")]
    inputs_sha256: Sha256Digest,
    /// Exact SDK product ID from the selected release inputs
    #[arg(long, value_name = "ID")]
    sdk_id: String,
    /// Exact compiler group ID from the selected release inputs
    #[arg(long, value_name = "ID")]
    compiler_group: String,
    /// Exact compiler profile ID from the selected release inputs
    #[arg(long, value_name = "PROFILE")]
    compiler_profile: String,
    /// Active release host selected independently of package metadata
    #[arg(long, value_name = "HOST")]
    host: String,
    /// Immutable release identifier expected in the compiler package
    #[arg(long, value_name = "ID")]
    release_id: String,
    /// Closed, independently selected generation read-back JSON
    #[arg(long, value_name = "FILE")]
    selection: PathBuf,
    /// SHA-256 of the exact selection document bytes
    #[arg(long, value_name = "SHA256")]
    selection_sha256: Sha256Digest,
    /// Absolute build roots forbidden in SDK payloads; repeatable
    #[arg(long = "forbidden-prefix", value_name = "DIR", required = true)]
    forbidden_prefixes: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadbackSelection {
    schema: String,
    compiler: CompilerSelection,
    sdk: SdkSelection,
    generation: GenerationSelection,
    compatibility: CompatibilitySelection,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilerSelection {
    directory: PathBuf,
    archive_sha256: Sha256Digest,
    archive_size: u64,
    environment: SelectedDocument,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SdkSelection {
    directory: PathBuf,
    tree_sha256: Sha256Digest,
    archive_sha256: Sha256Digest,
    archive_size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationSelection {
    path: PathBuf,
    sha256: Sha256Digest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilitySelection {
    directory: PathBuf,
    manifest_sha256: Sha256Digest,
    inputs: SelectedDocument,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedDocument {
    path: PathBuf,
    sha256: Sha256Digest,
}

fn parse_selection(bytes: &[u8]) -> Result<ReadbackSelection> {
    let selection: ReadbackSelection = serde_json::from_slice(bytes)
        .map_err(|_| miette::miette!("SDK generation selection is not closed JSON"))?;
    if selection.schema != SELECTION_SCHEMA {
        miette::bail!("SDK generation selection has an unsupported schema");
    }
    validate_selection_values(&selection)?;
    Ok(selection)
}

fn validate_selection_values(selection: &ReadbackSelection) -> Result<()> {
    for (label, digest) in [
        ("compiler archive", &selection.compiler.archive_sha256),
        (
            "compiler environment",
            &selection.compiler.environment.sha256,
        ),
        ("SDK tree", &selection.sdk.tree_sha256),
        ("SDK archive", &selection.sdk.archive_sha256),
        ("generation record", &selection.generation.sha256),
        (
            "compatibility manifest",
            &selection.compatibility.manifest_sha256,
        ),
        (
            "compatibility inputs",
            &selection.compatibility.inputs.sha256,
        ),
    ] {
        require_nonzero_digest(label, digest)?;
    }
    if selection.compiler.archive_size == 0 || selection.sdk.archive_size == 0 {
        miette::bail!("selected compiler and SDK archive sizes must be positive");
    }
    Ok(())
}

fn require_nonzero_digest(label: &str, digest: &Sha256Digest) -> Result<()> {
    if digest.as_str().bytes().all(|byte| byte == b'0') {
        miette::bail!("selected {label} SHA-256 must be nonzero");
    }
    Ok(())
}

#[cfg(unix)]
pub fn run(args: &VerifyGenerationArgs) -> Result<()> {
    run_unix(args)
}

#[cfg(not(unix))]
pub fn run(_args: &VerifyGenerationArgs) -> Result<()> {
    miette::bail!("SDK generation read-back requires a supported Unix host")
}

#[cfg(unix)]
fn run_unix(args: &VerifyGenerationArgs) -> Result<()> {
    use aros_toolchain::compatibility::{
        NativeCompatibilityExpectedPackage, NativeCompatibilityInputClaims,
        PortableNativeCompatibilityRequest,
    };
    use aros_toolchain::package::PackageFormat;
    use aros_toolchain::package_verify::PackageVerificationRequest;
    use aros_toolchain::release_inputs::{ReleaseInputs, ACTIVE_HOSTS};
    use aros_toolchain::sdk_generation::SdkGenerationReadbackRequest;
    use aros_toolchain::sdk_package::SdkVerificationRequest;
    use aros_toolchain::source_lock::CompilerFamily;

    require_nonzero_digest("release-input collection", &args.inputs_sha256)?;
    require_nonzero_digest("selection", &args.selection_sha256)?;
    validate_normalized_absolute_path(&args.inputs_dir, "release-input directory")?;
    validate_normalized_absolute_path(&args.selection, "selection file")?;
    for prefix in &args.forbidden_prefixes {
        validate_normalized_absolute_path(prefix, "forbidden prefix")?;
    }

    let selection_file = read_selected_file(
        &args.selection,
        &args.selection_sha256,
        "SDK generation selection",
        MAX_SELECTED_FILE_BYTES,
    )?;
    let selection = parse_selection(&selection_file.bytes)?;
    validate_selection_paths(&selection)?;

    let input_root = inspect_real_directory(&args.inputs_dir, "release-input directory")?;
    let compiler_root =
        inspect_real_directory(&selection.compiler.directory, "compiler package directory")?;
    let sdk_root = inspect_real_directory(&selection.sdk.directory, "SDK package directory")?;
    let compatibility_root = inspect_real_directory(
        &selection.compatibility.directory,
        "compatibility evidence directory",
    )?;
    let roots = [&input_root, &compiler_root, &sdk_root, &compatibility_root];
    require_disjoint_roots(&roots)?;
    require_outside_roots(&args.selection, &roots, "selection file")?;
    require_outside_roots(&selection.generation.path, &roots, "generation record")?;
    for (path, label) in [
        (
            &selection.compiler.environment.path,
            "compiler environment document",
        ),
        (
            &selection.compatibility.inputs.path,
            "compatibility inputs document",
        ),
    ] {
        require_outside_roots(
            path,
            &[&compiler_root, &sdk_root, &compatibility_root],
            label,
        )?;
    }

    let inputs = ReleaseInputs::load(&args.inputs_dir).map_err(|error| native_error(&error))?;
    if inputs.collection_sha256() != &args.inputs_sha256 {
        miette::bail!("release-input collection differs from its selected SHA-256");
    }

    // Resolve every semantic selector before opening compiler archives or the
    // compatibility evidence inventory.
    if !ACTIVE_HOSTS.contains(&args.host.as_str()) || !inputs.hosts().contains(&args.host) {
        miette::bail!("selected host is not an active release-input host");
    }
    let sdk = inputs
        .sdks()
        .iter()
        .find(|sdk| sdk.id() == args.sdk_id)
        .ok_or_else(|| miette::miette!("SDK ID is absent from selected release inputs"))?;
    let group = inputs
        .groups()
        .iter()
        .find(|group| group.id() == args.compiler_group)
        .ok_or_else(|| miette::miette!("compiler group is absent from selected release inputs"))?;
    let profile = group
        .profiles()
        .select(&args.compiler_profile)
        .map_err(|error| native_error(&error))?;
    if !sdk.compiler_profiles().iter().any(|reference| {
        reference.group_id() == group.id() && reference.profile() == profile.name()
    }) || !inputs.expected_lanes().iter().any(|lane| {
        lane.group_id() == group.id()
            && lane.profile() == profile.name()
            && lane.host() == args.host.as_str()
    }) {
        miette::bail!("selected SDK has no compiler reference for this group, profile and host");
    }

    let input_files = selected_input_file_snapshots(&args.inputs_dir, &inputs, sdk, group)?;
    let generation_file = read_selected_file(
        &selection.generation.path,
        &selection.generation.sha256,
        "SDK generation record",
        aros_toolchain::canonical::MAX_DOCUMENT_BYTES as u64,
    )?;
    let environment_file = read_selected_file(
        &selection.compiler.environment.path,
        &selection.compiler.environment.sha256,
        "compiler build-environment document",
        MAX_SELECTED_FILE_BYTES,
    )?;
    let build_environment = parse_build_environment(&environment_file.bytes, &args.host)?;
    let compatibility_inputs_file = read_selected_file(
        &selection.compatibility.inputs.path,
        &selection.compatibility.inputs.sha256,
        "native compatibility inputs document",
        aros_toolchain::canonical::MAX_DOCUMENT_BYTES as u64,
    )?;
    let claims = NativeCompatibilityInputClaims::parse(
        &compatibility_inputs_file.bytes,
        &selection.compatibility.inputs.sha256,
    )
    .map_err(|error| native_error(&error))?;
    let mut file_snapshots = vec![
        &selection_file.snapshot,
        &generation_file.snapshot,
        &environment_file.snapshot,
        &compatibility_inputs_file.snapshot,
    ];
    file_snapshots.extend(input_files.iter());
    require_unique_file_identities(&file_snapshots)?;

    let verified_compiler = aros_toolchain::package_verify::verify_with_format(
        &PackageVerificationRequest {
            package_dir: selection.compiler.directory.clone(),
            release_id: args.release_id.clone(),
            host: args.host.clone(),
            recipe: group.recipe().clone(),
            source_lock: group.source_lock().clone(),
            profile: profile.clone(),
            build_environment,
            forbidden_prefixes: args.forbidden_prefixes.clone(),
        },
        PackageFormat::CompilerFamilyV2,
    )
    .map_err(|error| native_error(&error))?;
    if verified_compiler.archive_sha256 != selection.compiler.archive_sha256
        || verified_compiler.archive_size != selection.compiler.archive_size
    {
        miette::bail!("verified compiler package differs from its independent selection");
    }
    // The native family-v2 verifier has already rebound this manifest
    // identity to the selected recipe, source lock and profile.
    let compiler_identity = verified_compiler
        .manifest
        .compiler_identity()
        .map_err(|error| miette::miette!("verified compiler manifest is invalid: {error}"))?;

    let (source_commit, _) = group.recipe().source();
    let expected_package = NativeCompatibilityExpectedPackage {
        manifest: &verified_compiler.manifest,
        archive_sha256: &verified_compiler.archive_sha256,
        archive_size: verified_compiler.archive_size,
        compiler: &compiler_identity,
        source_commit,
        host: &args.host,
    };
    let expected = match group.source_lock().family() {
        CompilerFamily::Llvm => claims
            .expectations_with_llvm_sdk(expected_package, group.profiles(), profile, sdk.identity())
            .map_err(|error| native_error(&error))?,
        CompilerFamily::Gnu => claims
            .expectations(expected_package, group.profiles(), profile, true)
            .map_err(|error| native_error(&error))?,
    };
    let compatibility = PortableNativeCompatibilityRequest {
        directory: selection.compatibility.directory.clone(),
        manifest_sha256: selection.compatibility.manifest_sha256.clone(),
        expected,
    };
    let sdk_package = SdkVerificationRequest {
        package_dir: selection.sdk.directory.clone(),
        identity: sdk.identity().clone(),
        expected_tree_sha256: selection.sdk.tree_sha256.clone(),
        expected_archive_sha256: selection.sdk.archive_sha256.clone(),
        expected_archive_size: selection.sdk.archive_size,
        forbidden_prefixes: args.forbidden_prefixes.clone(),
    };
    let readback =
        aros_toolchain::sdk_generation::readback_generation(&SdkGenerationReadbackRequest {
            inputs: &inputs,
            sdk_id: &args.sdk_id,
            compiler_group_id: &args.compiler_group,
            record_bytes: &generation_file.bytes,
            record_sha256: &selection.generation.sha256,
            package: &sdk_package,
            compatibility: &compatibility,
        })
        .map_err(|error| native_error(&error))?;

    revalidate_file_snapshot(&selection_file.snapshot, MAX_SELECTED_FILE_BYTES)?;
    revalidate_file_snapshot(
        &generation_file.snapshot,
        aros_toolchain::canonical::MAX_DOCUMENT_BYTES as u64,
    )?;
    revalidate_file_snapshot(&environment_file.snapshot, MAX_SELECTED_FILE_BYTES)?;
    revalidate_file_snapshot(
        &compatibility_inputs_file.snapshot,
        aros_toolchain::canonical::MAX_DOCUMENT_BYTES as u64,
    )?;
    for snapshot in &input_files {
        revalidate_file_snapshot(
            snapshot,
            aros_toolchain::canonical::MAX_DOCUMENT_BYTES as u64,
        )?;
    }
    for root in roots {
        revalidate_directory_snapshot(root)?;
    }
    // These repeat observations detect ordinary replacement or mutation, but
    // do not make a filesystem snapshot. Callers must keep selected paths
    // quiescent while this read-only operation runs.
    let final_inputs =
        ReleaseInputs::load(&args.inputs_dir).map_err(|error| native_error(&error))?;
    if final_inputs.collection_sha256() != &args.inputs_sha256 {
        miette::bail!("release-input collection changed before read-back completed");
    }

    aros_common::outputln!(
        "{}",
        serde_json::json!({
            "schema": "aros-sdk-generation-readback-v1",
            "operation": "sdk.verify-generation",
            "assurance": "byte-consistency-not-authenticated-execution-or-release-admission",
            "sdk_id": args.sdk_id,
            "compiler_group": args.compiler_group,
            "compiler_profile": args.compiler_profile,
            "compiler_identity": compiler_identity,
            "host": args.host,
            "release_id": args.release_id,
            "inputs_sha256": inputs.collection_sha256(),
            "selection_sha256": args.selection_sha256,
            "generation_record_sha256": readback.record_sha256(),
            "compiler_archive_sha256": verified_compiler.archive_sha256,
            "compiler_archive_size": verified_compiler.archive_size,
            "sdk_archive_sha256": readback.package().archive_sha256,
            "sdk_archive_size": readback.package().archive_size,
            "sdk_tree_sha256": readback.package().manifest.tree_sha256,
            "compatibility_manifest_sha256": readback.compatibility_manifest_sha256(),
            "compatibility_receipt_sha256": readback.compatibility_receipt_sha256(),
            "sdk_link_receipt_sha256": readback.sdk_link_receipt_sha256(),
        })
    );
    Ok(())
}

fn validate_selection_paths(selection: &ReadbackSelection) -> Result<()> {
    for (path, label) in [
        (&selection.compiler.directory, "compiler package directory"),
        (
            &selection.compiler.environment.path,
            "compiler environment document",
        ),
        (&selection.sdk.directory, "SDK package directory"),
        (&selection.generation.path, "generation record"),
        (
            &selection.compatibility.directory,
            "compatibility evidence directory",
        ),
        (
            &selection.compatibility.inputs.path,
            "compatibility inputs document",
        ),
    ] {
        validate_normalized_absolute_path(path, label)?;
    }
    Ok(())
}

fn validate_normalized_absolute_path(path: &Path, label: &str) -> Result<()> {
    let text = path
        .to_str()
        .ok_or_else(|| miette::miette!("{label} path must be UTF-8"))?;
    if !path.is_absolute()
        || text.len() < 2
        || text.contains("//")
        || text.ends_with('/')
        || text.contains('\\')
        || text.chars().any(char::is_control)
        || text
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        miette::bail!("{label} path must be normalized, absolute UTF-8");
    }
    Ok(())
}

#[cfg(unix)]
fn parse_build_environment(
    bytes: &[u8],
    expected_host: &str,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let UniqueJsonValue(value) =
        serde_json::from_slice::<UniqueJsonValue>(bytes).map_err(|_| {
            miette::miette!("compiler build-environment document is not unique-key JSON")
        })?;
    let object = value
        .as_object()
        .ok_or_else(|| miette::miette!("compiler build-environment document must be an object"))?;
    if object.get("schema").and_then(serde_json::Value::as_str)
        != Some("aros-toolchain-build-environment-v1")
        || object.get("host").and_then(serde_json::Value::as_str) != Some(expected_host)
    {
        miette::bail!("compiler build-environment document differs from its selected host");
    }
    Ok(object.clone())
}

#[cfg(unix)]
#[derive(Debug)]
struct UniqueJsonValue(serde_json::Value);

#[cfg(unix)]
impl<'de> Deserialize<'de> for UniqueJsonValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueJsonVisitor).map(Self)
    }
}

#[cfg(unix)]
struct UniqueJsonVisitor;

#[cfg(unix)]
impl<'de> serde::de::Visitor<'de> for UniqueJsonVisitor {
    type Value = serde_json::Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::Null)
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| E::custom("invalid JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::String(value))
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(UniqueJsonValue(value)) = sequence.next_element::<UniqueJsonValue>()? {
            values.push(value);
        }
        Ok(serde_json::Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        let mut values = serde_json::Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(serde::de::Error::custom("duplicate JSON object key"));
            }
            let UniqueJsonValue(value) = object.next_value()?;
            values.insert(key, value);
        }
        Ok(serde_json::Value::Object(values))
    }
}

#[cfg(unix)]
#[derive(Debug, Clone)]
struct FileSnapshot {
    path: PathBuf,
    identity: aros_common::FileIdentity,
    digest: Sha256Digest,
    size: u64,
}

#[cfg(unix)]
struct ReadSnapshot {
    snapshot: FileSnapshot,
    bytes: Vec<u8>,
}

#[cfg(unix)]
fn read_selected_file(
    path: &Path,
    expected: &Sha256Digest,
    label: &str,
    max_bytes: u64,
) -> Result<ReadSnapshot> {
    use std::io::Read as _;
    use std::os::unix::fs::MetadataExt as _;

    use aros_common::{
        measure_regular_file_digest_bounded, open_regular_file_nofollow, sha256_bytes,
    };

    validate_normalized_absolute_path(path, label)?;
    require_nonzero_digest(label, expected)?;
    let (before_identity, before) = measure_regular_file_digest_bounded(path, max_bytes)
        .map_err(|_| miette::miette!("{label} is not a bounded regular file"))?;
    if before.size == 0 || before.digest != *expected {
        miette::bail!("{label} differs from its independently selected bytes");
    }
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| miette::miette!("{label} cannot be opened without following links"))?;
    let descriptor_before = file
        .metadata()
        .map_err(|_| miette::miette!("{label} cannot be inspected"))?;
    if !descriptor_before.is_file()
        || descriptor_before.nlink() != 1
        || descriptor_before.dev() != before_identity.device()
        || descriptor_before.ino() != before_identity.inode()
        || descriptor_before.len() != before.size
    {
        miette::bail!("{label} changed before its bounded read");
    }
    let capacity = usize::try_from(before.size)
        .map_err(|_| miette::miette!("{label} is too large to retain"))?;
    let mut bytes = Vec::with_capacity(capacity);
    file.by_ref()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| miette::miette!("{label} cannot be read"))?;
    let descriptor_after = file
        .metadata()
        .map_err(|_| miette::miette!("{label} cannot be re-inspected"))?;
    let (after_identity, after) = measure_regular_file_digest_bounded(path, max_bytes)
        .map_err(|_| miette::miette!("{label} changed during its bounded read"))?;
    if bytes.len() as u64 != before.size
        || bytes.len() as u64 > max_bytes
        || sha256_bytes(&bytes) != *expected
        || after_identity != before_identity
        || after.digest != *expected
        || after.size != before.size
        || descriptor_after.dev() != before_identity.device()
        || descriptor_after.ino() != before_identity.inode()
        || descriptor_after.len() != before.size
        || descriptor_after.nlink() != 1
    {
        miette::bail!("{label} changed during its bounded read");
    }
    Ok(ReadSnapshot {
        snapshot: FileSnapshot {
            path: path.to_path_buf(),
            identity: before_identity,
            digest: before.digest,
            size: before.size,
        },
        bytes,
    })
}

#[cfg(unix)]
fn measure_selected_file(
    path: &Path,
    expected: &Sha256Digest,
    label: &str,
    max_bytes: u64,
) -> Result<FileSnapshot> {
    use std::os::unix::fs::MetadataExt as _;

    use aros_common::measure_regular_file_digest_bounded;
    use aros_common::open_regular_file_nofollow;

    validate_normalized_absolute_path(path, label)?;
    let (identity, measured) = measure_regular_file_digest_bounded(path, max_bytes)
        .map_err(|_| miette::miette!("{label} is not a bounded regular file"))?;
    if measured.size == 0 || measured.digest != *expected {
        miette::bail!("{label} differs from its independently selected bytes");
    }
    let file = open_regular_file_nofollow(path)
        .map_err(|_| miette::miette!("{label} cannot be opened without following links"))?;
    let metadata = file
        .metadata()
        .map_err(|_| miette::miette!("{label} cannot be inspected"))?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.dev() != identity.device()
        || metadata.ino() != identity.inode()
        || metadata.len() != measured.size
    {
        miette::bail!("{label} is linked or changed during measurement");
    }
    Ok(FileSnapshot {
        path: path.to_path_buf(),
        identity,
        digest: measured.digest,
        size: measured.size,
    })
}

#[cfg(unix)]
fn revalidate_file_snapshot(snapshot: &FileSnapshot, max_bytes: u64) -> Result<()> {
    use std::os::unix::fs::MetadataExt as _;

    use aros_common::measure_regular_file_digest_bounded;
    use aros_common::open_regular_file_nofollow;

    let (identity, measured) = measure_regular_file_digest_bounded(&snapshot.path, max_bytes)
        .map_err(|_| miette::miette!("a selected document changed before read-back completed"))?;
    if identity != snapshot.identity
        || measured.digest != snapshot.digest
        || measured.size != snapshot.size
    {
        miette::bail!("a selected document changed before read-back completed");
    }
    let file = open_regular_file_nofollow(&snapshot.path)
        .map_err(|_| miette::miette!("a selected document changed before read-back completed"))?;
    let metadata = file
        .metadata()
        .map_err(|_| miette::miette!("a selected document changed before read-back completed"))?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.dev() != snapshot.identity.device()
        || metadata.ino() != snapshot.identity.inode()
        || metadata.len() != snapshot.size
    {
        miette::bail!("a selected document changed before read-back completed");
    }
    Ok(())
}

#[cfg(unix)]
#[derive(Debug)]
struct DirectorySnapshot {
    path: PathBuf,
    device: u64,
    inode: u64,
}

#[cfg(unix)]
fn inspect_real_directory(path: &Path, label: &str) -> Result<DirectorySnapshot> {
    use std::fs;
    use std::os::unix::fs::MetadataExt as _;

    validate_normalized_absolute_path(path, label)?;
    let mut current = PathBuf::from("/");
    let mut final_metadata = None;
    for component in path.components() {
        match component {
            Component::RootDir => continue,
            Component::Normal(name) => current.push(name),
            _ => return Err(miette::miette!("{label} path is not normalized")),
        }
        let metadata = fs::symlink_metadata(&current)
            .map_err(|_| miette::miette!("{label} is unavailable"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            miette::bail!("{label} and its parent components must be real directories");
        }
        final_metadata = Some(metadata);
    }
    let metadata = final_metadata.ok_or_else(|| miette::miette!("{label} is unavailable"))?;
    Ok(DirectorySnapshot {
        path: path.to_path_buf(),
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(unix)]
fn revalidate_directory_snapshot(snapshot: &DirectorySnapshot) -> Result<()> {
    let current = inspect_real_directory(&snapshot.path, "selected directory")?;
    if current.device != snapshot.device || current.inode != snapshot.inode {
        miette::bail!("a selected directory changed before read-back completed");
    }
    Ok(())
}

#[cfg(unix)]
fn require_disjoint_roots(roots: &[&DirectorySnapshot]) -> Result<()> {
    for left in 0..roots.len() {
        for right in (left + 1)..roots.len() {
            if aros_common::publication::filesystem_paths_overlap(
                &roots[left].path,
                &roots[right].path,
            )
            .map_err(|_| miette::miette!("cannot establish selected root ancestry"))?
                || (roots[left].device == roots[right].device
                    && roots[left].inode == roots[right].inode)
            {
                miette::bail!("selected package, evidence and input roots must be disjoint");
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn require_outside_roots(path: &Path, roots: &[&DirectorySnapshot], label: &str) -> Result<()> {
    for root in roots {
        if aros_common::publication::filesystem_paths_overlap(path, &root.path)
            .map_err(|_| miette::miette!("cannot establish {label} ancestry"))?
        {
            miette::bail!("{label} must be outside selected package, evidence and input roots");
        }
    }
    Ok(())
}

#[cfg(unix)]
fn require_unique_file_identities(snapshots: &[&FileSnapshot]) -> Result<()> {
    for left in 0..snapshots.len() {
        for right in (left + 1)..snapshots.len() {
            if snapshots[left].identity == snapshots[right].identity {
                miette::bail!("selected documents must not alias the same regular file");
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn selected_input_file_snapshots(
    directory: &Path,
    inputs: &aros_toolchain::release_inputs::ReleaseInputs,
    sdk: &aros_toolchain::release_inputs::BoundSdk,
    group: &aros_toolchain::release_inputs::BoundGroup,
) -> Result<Vec<FileSnapshot>> {
    let mut snapshots = Vec::with_capacity(5);
    let collection_path = directory.join(INPUT_COLLECTION_FILE);
    snapshots.push(measure_selected_file(
        &collection_path,
        inputs.collection_sha256(),
        "release-input collection",
        aros_toolchain::canonical::MAX_DOCUMENT_BYTES as u64,
    )?);
    for (reference, label) in [
        (group.recipe_reference(), "selected recipe"),
        (group.source_lock_reference(), "selected source lock"),
        (group.profiles_reference(), "selected profiles"),
        (sdk.identity_reference(), "selected SDK identity"),
    ] {
        snapshots.push(measure_selected_file(
            &directory.join(reference.file()),
            reference.sha256(),
            label,
            aros_toolchain::canonical::MAX_DOCUMENT_BYTES as u64,
        )?);
    }
    Ok(snapshots)
}

#[cfg(unix)]
fn native_error(error: &aros_toolchain::ContractError) -> miette::Report {
    crate::observability::native_diagnostic(error.diagnostics().diagnostics[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection_json() -> String {
        let digest = "a".repeat(64);
        serde_json::json!({
            "schema": SELECTION_SCHEMA,
            "compiler": {
                "directory": "/tmp/compiler",
                "archive_sha256": digest,
                "archive_size": 11,
                "environment": {"path": "/tmp/environment.json", "sha256": "b".repeat(64)}
            },
            "sdk": {
                "directory": "/tmp/sdk",
                "tree_sha256": "c".repeat(64),
                "archive_sha256": "d".repeat(64),
                "archive_size": 12
            },
            "generation": {"path": "/tmp/generation.json", "sha256": "e".repeat(64)},
            "compatibility": {
                "directory": "/tmp/evidence",
                "manifest_sha256": "f".repeat(64),
                "inputs": {"path": "/tmp/compatibility-inputs.json", "sha256": "1".repeat(64)}
            }
        })
        .to_string()
    }

    #[test]
    fn selection_schema_is_closed_and_accepts_valid_bound_values() {
        let selection = parse_selection(selection_json().as_bytes()).unwrap();
        assert_eq!(selection.schema, SELECTION_SCHEMA);
        assert_eq!(selection.compiler.archive_size, 11);
        assert_eq!(selection.sdk.archive_size, 12);
    }

    #[cfg(unix)]
    #[test]
    fn independent_documents_reject_filesystem_ancestry_aliases() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        let directory = parent.join("SelectedRoot");
        std::fs::create_dir(&directory).unwrap();
        let selected = inspect_real_directory(&directory, "selected root").unwrap();
        let alias = parent.join("root-alias");
        symlink(&directory, &alias).unwrap();
        assert!(
            require_outside_roots(&parent.join("independent.json"), &[&selected], "selection")
                .is_ok()
        );
        assert!(
            require_outside_roots(&alias.join("selection.json"), &[&selected], "selection")
                .is_err()
        );
        for spelling in ["selectedroot", "SELECTEDROOT"] {
            let native_alias = parent.join(spelling);
            if native_alias.is_dir() {
                assert!(require_outside_roots(
                    &native_alias.join("selection.json"),
                    &[&selected],
                    "selection"
                )
                .is_err());
            }
        }
        assert!(!parent.join("independent.json").exists());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
    }

    #[test]
    fn selection_rejects_duplicate_and_unknown_fields() {
        let valid = selection_json();
        parse_selection(valid.as_bytes()).unwrap();
        for field in [
            format!("\"schema\":\"{SELECTION_SCHEMA}\""),
            "\"archive_size\":11".to_owned(),
            format!("\"tree_sha256\":\"{}\"", "c".repeat(64)),
        ] {
            assert!(valid.contains(&field));
            let duplicate = valid.replacen(&field, &format!("{field},{field}"), 1);
            let cause = serde_json::from_slice::<ReadbackSelection>(duplicate.as_bytes())
                .unwrap_err()
                .to_string();
            assert!(cause.contains("duplicate field"), "{cause}");
            assert!(parse_selection(duplicate.as_bytes()).is_err());
        }

        let mut unknown: serde_json::Value = serde_json::from_str(&selection_json()).unwrap();
        unknown["unexpected"] = serde_json::Value::Bool(true);
        assert!(parse_selection(unknown.to_string().as_bytes()).is_err());
    }

    #[test]
    fn selection_rejects_zero_digests_and_sizes() {
        let mut zero_digest: serde_json::Value = serde_json::from_str(&selection_json()).unwrap();
        zero_digest["sdk"]["tree_sha256"] = serde_json::Value::String("0".repeat(64));
        assert!(parse_selection(zero_digest.to_string().as_bytes()).is_err());

        let mut zero_size: serde_json::Value = serde_json::from_str(&selection_json()).unwrap();
        zero_size["compiler"]["archive_size"] = serde_json::Value::from(0);
        assert!(parse_selection(zero_size.to_string().as_bytes()).is_err());
    }

    #[test]
    fn path_validation_rejects_unsafe_paths_and_nested_selection_files() {
        assert!(validate_normalized_absolute_path(Path::new("/tmp/a/../b"), "path").is_err());
        assert!(validate_normalized_absolute_path(Path::new("relative/file"), "path").is_err());

        #[cfg(unix)]
        {
            let root = DirectorySnapshot {
                path: PathBuf::from("/tmp/package"),
                device: 1,
                inode: 2,
            };
            assert!(require_outside_roots(
                Path::new("/tmp/package/selection.json"),
                &[&root],
                "selection file"
            )
            .is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn bounded_file_read_rejects_digest_drift_and_hardlink_aliases() {
        use std::fs;

        use aros_common::sha256_bytes;

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let original = root.join("selected.json");
        fs::write(&original, b"first").unwrap();
        let original_digest = sha256_bytes(b"first");
        let snapshot = read_selected_file(&original, &original_digest, "test input", 32).unwrap();
        assert_eq!(snapshot.bytes, b"first");
        fs::write(&original, b"drift").unwrap();
        assert!(revalidate_file_snapshot(&snapshot.snapshot, 32).is_err());

        let source = root.join("source.json");
        let alias = root.join("alias.json");
        fs::write(&source, b"same bytes").unwrap();
        fs::hard_link(&source, &alias).unwrap();
        let digest = sha256_bytes(b"same bytes");
        assert!(read_selected_file(&source, &digest, "source", 32).is_err());
        assert!(read_selected_file(&alias, &digest, "alias", 32).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn environment_reader_rejects_duplicate_keys() {
        let valid = br#"{"schema":"aros-toolchain-build-environment-v1","host":"macos-aarch64","nested":{"x":1}}"#;
        parse_build_environment(valid, "macos-aarch64").unwrap();
        let duplicate = br#"{"schema":"aros-toolchain-build-environment-v1","host":"macos-aarch64","nested":{"x":1,"x":2}}"#;
        assert!(parse_build_environment(duplicate, "macos-aarch64").is_err());
    }
}
