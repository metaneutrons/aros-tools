//! Closed contract for source-dependent media inputs.
//!
//! A CMake target can report built files; a verified legacy v1 boot bundle can
//! only report its measured inputs. These origins must not be conflated. A
//! bound v2 CMake receipt records source and release-toolchain identities for
//! independent remeasurement. A v1 receipt remains an explicitly weaker input,
//! not proof that legacy bytes came from a specific build. Neither version
//! proves that a medium boots or authenticates its producing repository.

use crate::media_profile::{valid_target_preset, MediaProfile};
use crate::{
    casefold_path_key, open_regular_file_nofollow, sha256_reader, toolchain_tree_inventory,
    ArosToolchainManifest, Sha256Digest, AROS_TOOLCHAIN_MANIFEST_FILE,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const FORMAT_VERSION: u32 = 1;
const BOUND_FORMAT_VERSION: u32 = 2;
const TREE_FORMAT_VERSION: u32 = 3;
const KIND: &str = "aros-media-inputs";
const MAX_RECEIPT_BYTES: usize = 1024 * 1024;
const MAX_FILES: usize = 1024;

/// One source-dependent file claimed by the declared receipt origin.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MediaBuildFile {
    pub role: String,
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Digest and cardinality of an entire built directory, including empty dirs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MediaBuildTree {
    pub role: String,
    pub path: String,
    pub sha256: Sha256Digest,
    pub file_count: usize,
    pub directory_count: usize,
}

/// Origin of measured files; legacy inputs do not assert CMake provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum MediaReceiptOrigin {
    #[serde(rename = "cmake")]
    Cmake,
    #[serde(rename = "legacy-v1")]
    LegacyV1,
}

/// Measured source and installed release-toolchain identity. This binds
/// content to a checkout and toolchain, but is not a cryptographic attestation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MediaBuildIdentity {
    pub source_commit: String,
    pub source_tree: String,
    pub toolchain_release_id: String,
    pub toolchain_tree_sha256: Sha256Digest,
    pub toolchain_manifest_sha256: Sha256Digest,
}

/// Versioned media input inventory, intentionally excluding final media identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MediaBuildReceipt {
    pub format_version: u32,
    pub kind: String,
    pub origin: MediaReceiptOrigin,
    /// For legacy inputs this is the selected profile's expected target, not
    /// independent evidence of the build that produced those inputs.
    pub target_preset: String,
    pub model: String,
    pub transport: String,
    /// Absent only in the explicitly weaker historical v1 receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_identity: Option<MediaBuildIdentity>,
    pub files: Vec<MediaBuildFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trees: Vec<MediaBuildTree>,
}

impl MediaBuildReceipt {
    /// Construct a checked v1 receipt from an existing verified producer.
    ///
    /// # Errors
    ///
    /// Returns an error when identity, file paths, roles or digests violate
    /// the closed receipt contract.
    pub fn new(
        origin: MediaReceiptOrigin,
        target_preset: String,
        model: String,
        transport: String,
        files: Vec<MediaBuildFile>,
    ) -> Result<Self, MediaReceiptError> {
        let receipt = Self {
            format_version: FORMAT_VERSION,
            kind: KIND.to_string(),
            origin,
            target_preset,
            model,
            transport,
            build_identity: None,
            files,
            trees: Vec::new(),
        };
        validate_receipt(&receipt)?;
        Ok(receipt)
    }

    /// Construct a source/toolchain-bound CMake receipt.
    ///
    /// # Errors
    ///
    /// Rejects an incomplete identity or invalid inventory.
    pub fn new_bound_cmake(
        target_preset: String,
        model: String,
        transport: String,
        identity: MediaBuildIdentity,
        files: Vec<MediaBuildFile>,
    ) -> Result<Self, MediaReceiptError> {
        let receipt = Self {
            format_version: BOUND_FORMAT_VERSION,
            kind: KIND.to_string(),
            origin: MediaReceiptOrigin::Cmake,
            target_preset,
            model,
            transport,
            build_identity: Some(identity),
            files,
            trees: Vec::new(),
        };
        validate_receipt(&receipt)?;
        Ok(receipt)
    }

    /// Construct a bound CMake receipt with complete built-directory inputs.
    ///
    /// # Errors
    /// Rejects malformed roles, paths, digests, counts or origin identity.
    pub fn new_bound_cmake_with_trees(
        target_preset: String,
        model: String,
        transport: String,
        identity: MediaBuildIdentity,
        files: Vec<MediaBuildFile>,
        trees: Vec<MediaBuildTree>,
    ) -> Result<Self, MediaReceiptError> {
        let receipt = Self {
            format_version: TREE_FORMAT_VERSION,
            kind: KIND.to_string(),
            origin: MediaReceiptOrigin::Cmake,
            target_preset,
            model,
            transport,
            build_identity: Some(identity),
            files,
            trees,
        };
        validate_receipt(&receipt)?;
        Ok(receipt)
    }
}

/// Fail-closed receipt parsing and measurement error.
#[derive(Debug, thiserror::Error)]
pub enum MediaReceiptError {
    #[error("invalid media build receipt: {0}")]
    Invalid(String),
    #[error("cannot read a declared media build file: {0}")]
    Io(#[from] std::io::Error),
}

/// Parse a bounded, closed JSON receipt without touching the build tree.
///
/// # Errors
///
/// Returns an error for an oversized document, unknown field, unsupported
/// version, unsafe path, duplicate role/path or malformed digest.
pub fn parse_media_build_receipt(bytes: &[u8]) -> Result<MediaBuildReceipt, MediaReceiptError> {
    if bytes.len() > MAX_RECEIPT_BYTES {
        return Err(invalid("receipt exceeds its one-megabyte limit"));
    }
    let receipt: MediaBuildReceipt = serde_json::from_slice(bytes)
        .map_err(|error| invalid(&format!("JSON schema error: {error}")))?;
    validate_receipt(&receipt)?;
    Ok(receipt)
}

/// Require that a typed receipt is compatible with the selected reviewed
/// profile, then remeasure every declared regular build file through a
/// no-follow descriptor.
///
/// # Errors
///
/// Returns an error for an incompatible profile, missing required role,
/// unsafe file, changed size or changed SHA-256. A successful result is a
/// point-in-time observation; composition must revalidate before publication.
pub fn verify_media_build_receipt(
    root: &Path,
    receipt: &MediaBuildReceipt,
    profile: &MediaProfile,
) -> Result<(), MediaReceiptError> {
    validate_receipt(receipt)?;
    if receipt.target_preset != profile.target_preset
        || receipt.model != profile.model
        || receipt.transport != profile.transport
    {
        return Err(invalid("receipt does not match the selected media profile"));
    }
    let actual_roles: BTreeSet<&str> = receipt
        .files
        .iter()
        .map(|file| file.role.as_str())
        .collect();
    for required in &profile.required_files {
        if receipt.origin == MediaReceiptOrigin::Cmake && required.external_input.is_some() {
            if actual_roles.contains(required.role.as_str()) {
                return Err(invalid(&format!(
                    "CMake receipt must not claim external role '{}'",
                    required.role
                )));
            }
            continue;
        }
        if !actual_roles.contains(required.role.as_str()) {
            return Err(invalid(&format!(
                "missing required build role '{}'",
                required.role
            )));
        }
    }
    let expected_tree_roles: BTreeSet<_> = profile
        .required_trees
        .iter()
        .map(|tree| tree.role.as_str())
        .collect();
    let actual_tree_roles: BTreeSet<_> = receipt
        .trees
        .iter()
        .map(|tree| tree.role.as_str())
        .collect();
    if expected_tree_roles != actual_tree_roles {
        return Err(invalid(
            "build receipt tree roles differ from the selected profile",
        ));
    }

    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err(invalid("media build root is not a directory"));
    }
    for declared in &receipt.files {
        let path = root.join(&declared.path);
        let mut file = open_regular_file_nofollow(&path)?;
        let actual = sha256_reader(&mut file)?;
        let expected = Sha256Digest::parse(&declared.sha256)
            .map_err(|_| invalid("file.sha256 is not a SHA-256 digest"))?;
        if actual.size != declared.size_bytes || actual.digest != expected {
            return Err(invalid(&format!(
                "measured size or SHA-256 differs for role '{}'",
                declared.role
            )));
        }
    }
    for declared in &receipt.trees {
        let measured = crate::media_tree::measure_media_tree(&root.join(&declared.path))?;
        if measured.sha256 != declared.sha256
            || measured.files.len() != declared.file_count
            || measured.directories.len() != declared.directory_count
        {
            return Err(invalid(&format!(
                "measured tree differs for role '{}'",
                declared.role
            )));
        }
    }
    Ok(())
}

/// Re-derive a bound receipt's origin.
///
/// Check the selected clean Git source and installed release toolchain.
/// Historical v1 receipts cannot pass and must be labelled unverified.
///
/// # Errors
///
/// Rejects a dirty or different checkout, a changed manifest, or a changed
/// toolchain payload. This does not authenticate the repository or its owner.
pub fn verify_media_build_identity(
    receipt: &MediaBuildReceipt,
    source_root: &Path,
    toolchain_root: &Path,
) -> Result<(), MediaReceiptError> {
    validate_receipt(receipt)?;
    let expected = receipt
        .build_identity
        .as_ref()
        .ok_or_else(|| invalid("historical v1 receipt has no source/toolchain binding"))?;
    let measured = measure_media_build_identity(source_root, toolchain_root)?;
    if &measured != expected {
        return Err(invalid(
            "source or toolchain identity differs from the media receipt",
        ));
    }
    Ok(())
}

/// Measure the clean source checkout and the complete installed toolchain.
///
/// Generated files below the CLI-owned `build/` directory are excluded from
/// source dirtiness only when no tracked source file exists there.
///
/// # Errors
/// Rejects any other source change or an altered toolchain installation.
pub fn measure_media_build_identity(
    source_root: &Path,
    toolchain_root: &Path,
) -> Result<MediaBuildIdentity, MediaReceiptError> {
    let selected_root = source_root.canonicalize()?;
    let git_root = git_value(source_root, &["rev-parse", "--show-toplevel"])?;
    if Path::new(&git_root).canonicalize()? != selected_root {
        return Err(invalid("source root is not the Git checkout root"));
    }
    let source_commit = git_value(source_root, &["rev-parse", "--verify", "HEAD"])?;
    let source_tree = git_value(source_root, &["rev-parse", "--verify", "HEAD^{tree}"])?;
    if !git_value(source_root, &["ls-files", "--", "build"])?.is_empty() {
        return Err(invalid(
            "source checkout tracks files in the generated build directory",
        ));
    }
    let changes = git_value(
        source_root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=normal",
            "--",
            ".",
            ":(exclude)build",
        ],
    )?;
    if !changes.is_empty() {
        return Err(invalid("source checkout is dirty"));
    }
    let manifest_path = toolchain_root.join(AROS_TOOLCHAIN_MANIFEST_FILE);
    let file = open_regular_file_nofollow(&manifest_path)?;
    let mut manifest_bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut manifest_bytes)?;
    if manifest_bytes.len() > 4 * 1024 * 1024 {
        return Err(invalid("toolchain manifest exceeds four megabytes"));
    }
    let manifest_sha256 = crate::sha256_bytes(&manifest_bytes);
    let manifest: ArosToolchainManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| invalid(&format!("invalid installed toolchain manifest: {error}")))?;
    manifest
        .validate()
        .map_err(|error| invalid(&format!("invalid installed toolchain manifest: {error}")))?;
    let (tree_sha256, inventory) = toolchain_tree_inventory(toolchain_root)
        .map_err(|error| invalid(&format!("cannot measure installed toolchain: {error}")))?;
    if tree_sha256 != manifest.tree_sha256 || inventory != manifest.files {
        return Err(invalid(
            "installed toolchain payload differs from its manifest",
        ));
    }
    let identity = MediaBuildIdentity {
        source_commit,
        source_tree,
        toolchain_release_id: manifest.release_id,
        toolchain_tree_sha256: Sha256Digest::parse(&tree_sha256)
            .map_err(|_| invalid("invalid installed toolchain tree digest"))?,
        toolchain_manifest_sha256: manifest_sha256,
    };
    validate_media_build_identity(&identity)?;
    Ok(identity)
}

fn git_value(root: &Path, args: &[&str]) -> Result<String, MediaReceiptError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(invalid("cannot verify source Git checkout"));
    }
    let value = std::str::from_utf8(&output.stdout)
        .map_err(|_| invalid("source Git output is not UTF-8"))?;
    Ok(value.trim().to_string())
}

fn validate_receipt(receipt: &MediaBuildReceipt) -> Result<(), MediaReceiptError> {
    let encoded_size = serde_json::to_vec(receipt)
        .map_err(|_| invalid("receipt cannot be serialized"))?
        .len();
    if encoded_size > MAX_RECEIPT_BYTES {
        return Err(invalid("receipt exceeds its one-megabyte limit"));
    }
    if !matches!(
        receipt.format_version,
        FORMAT_VERSION | BOUND_FORMAT_VERSION | TREE_FORMAT_VERSION
    ) || receipt.kind != KIND
    {
        return Err(invalid("unsupported format_version or kind"));
    }
    match (
        receipt.format_version,
        receipt.origin,
        &receipt.build_identity,
    ) {
        (FORMAT_VERSION, _, None) if receipt.trees.is_empty() => {}
        (BOUND_FORMAT_VERSION, MediaReceiptOrigin::Cmake, Some(identity))
            if receipt.trees.is_empty() =>
        {
            validate_media_build_identity(identity)?;
        }
        (TREE_FORMAT_VERSION, MediaReceiptOrigin::Cmake, Some(identity))
            if !receipt.trees.is_empty() =>
        {
            validate_media_build_identity(identity)?;
        }
        _ => {
            return Err(invalid(
                "receipt version, origin and build identity disagree",
            ))
        }
    }
    if !valid_target_preset(&receipt.target_preset) {
        return Err(invalid("target_preset must be a portable target name"));
    }
    for (label, value) in [
        ("model", receipt.model.as_str()),
        ("transport", receipt.transport.as_str()),
    ] {
        if !valid_slug(value) {
            return Err(invalid(&format!("{label} must be a lowercase slug")));
        }
    }
    if receipt.files.is_empty() || receipt.files.len() > MAX_FILES {
        return Err(invalid("files must contain between one and 1024 entries"));
    }
    let mut roles = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for file in &receipt.files {
        if !valid_slug(&file.role) || !roles.insert(&file.role) {
            return Err(invalid("file role is invalid or duplicated"));
        }
        if file.path.is_empty()
            || file.path.contains('\\')
            || file
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(invalid("file path is not a safe portable relative path"));
        }
        let path = PathBuf::from(&file.path);
        let folded = casefold_path_key(&path)
            .map_err(|_| invalid("file path is not portable across supported hosts"))?;
        if !paths.insert(folded) {
            return Err(invalid("file path is duplicated after case folding"));
        }
        Sha256Digest::parse(&file.sha256)
            .map_err(|_| invalid("file.sha256 is not a SHA-256 digest"))?;
    }
    let mut tree_roles = BTreeSet::new();
    let mut tree_paths = BTreeSet::new();
    for tree in &receipt.trees {
        if !valid_slug(&tree.role) || !tree_roles.insert(&tree.role) || roles.contains(&tree.role) {
            return Err(invalid("tree role is invalid or duplicated"));
        }
        if !crate::media_profile::valid_destination(&tree.path) || !tree_paths.insert(&tree.path) {
            return Err(invalid("tree path is unsafe or duplicated"));
        }
        if tree.file_count == 0 || tree.file_count > 20_000 || tree.directory_count > 20_000 {
            return Err(invalid("tree inventory has invalid cardinality"));
        }
    }
    Ok(())
}

/// Validate the syntax of one bound identity without claiming independent
/// origin authentication. Use [`verify_media_build_identity`] for remeasurement.
///
/// # Errors
///
/// Rejects malformed Git IDs or release identity.
pub fn validate_media_build_identity(
    identity: &MediaBuildIdentity,
) -> Result<(), MediaReceiptError> {
    for (label, value) in [
        ("source_commit", identity.source_commit.as_str()),
        ("source_tree", identity.source_tree.as_str()),
    ] {
        if value.len() != 40
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid(&format!(
                "{label} must be a lowercase Git object ID"
            )));
        }
    }
    if identity.toolchain_release_id.is_empty()
        || identity.toolchain_release_id.len() > 128
        || identity.toolchain_release_id.trim() != identity.toolchain_release_id
        || identity.toolchain_release_id.chars().any(char::is_control)
    {
        return Err(invalid("toolchain_release_id is invalid"));
    }
    Ok(())
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.starts_with(|character: char| {
            character.is_ascii_lowercase() || character.is_ascii_digit()
        })
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn invalid(message: &str) -> MediaReceiptError {
    MediaReceiptError::Invalid(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        parse_media_build_receipt, verify_media_build_identity, verify_media_build_receipt,
        MediaBuildFile, MediaBuildIdentity, MediaBuildReceipt, MediaReceiptOrigin, KIND,
    };
    use crate::media_profile::{built_in_media_profiles, parse_media_profile};
    use crate::{sha256_bytes, toolchain_tree_inventory, ArosToolchainManifest, Sha256Digest};
    use std::fs;
    use std::process::Command;

    fn fixture() -> (tempfile::TempDir, Vec<u8>) {
        let root = tempfile::tempdir().expect("build root");
        fs::write(root.path().join("loader.efi"), b"loader bytes").expect("build file");
        let digest = sha256_bytes(b"loader bytes");
        let receipt = format!(
            r#"{{"format_version":1,"kind":"{KIND}","origin":"cmake","target_preset":"opensbi-riscv64","model":"milk-v-titan","transport":"uefi-esp","files":[{{"role":"uefi-loader","path":"loader.efi","sha256":"{digest}","size_bytes":12}}]}}"#
        );
        (root, receipt.into_bytes())
    }

    #[allow(clippy::literal_string_with_formatting_args)] // Git's ^{tree} syntax is literal.
    #[test]
    fn bound_receipt_rechecks_clean_source_and_complete_toolchain_tree() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let toolchain = temp.path().join("toolchain");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&toolchain).unwrap();
        let git = |args: &[&str]| {
            let result = Command::new("git")
                .arg("-C")
                .arg(&source)
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            String::from_utf8(result.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q"]);
        fs::write(source.join("tracked.txt"), b"source").unwrap();
        git(&["add", "tracked.txt"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "source",
        ]);
        let commit = git(&["rev-parse", "HEAD"]);
        let tree_ref = "HEAD^{tree}";
        let tree = git(&["rev-parse", tree_ref]);

        fs::write(toolchain.join("clang"), b"compiler bytes").unwrap();
        let (tree_sha256, inventory) = toolchain_tree_inventory(&toolchain).unwrap();
        let manifest = ArosToolchainManifest {
            schema: 1,
            release_id: "test-1".into(),
            host: "linux-x86_64".into(),
            target_profile: "pc-x86_64".into(),
            target_triple: "x86_64-unknown-aros".into(),
            tree_sha256: tree_sha256.clone(),
            llvm_version: Some("1.2.3".into()),
            recipe_sha256: "1".repeat(64),
            source_lock_sha256: "2".repeat(64),
            profiles_sha256: "3".repeat(64),
            source_commit: commit.clone(),
            producer_commit: commit.clone(),
            tools_commit: commit.clone(),
            source_date_epoch: 1,
            capabilities: vec!["compiler".into()],
            build_environment: serde_json::Map::new(),
            files: inventory,
        };
        manifest.validate().unwrap();
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        fs::write(toolchain.join("toolchain-manifest.json"), &manifest_bytes).unwrap();
        let identity = MediaBuildIdentity {
            source_commit: commit,
            source_tree: tree,
            toolchain_release_id: manifest.release_id,
            toolchain_tree_sha256: Sha256Digest::parse(&tree_sha256).unwrap(),
            toolchain_manifest_sha256: sha256_bytes(&manifest_bytes),
        };
        let receipt = MediaBuildReceipt::new_bound_cmake(
            "pc-x86_64".into(),
            "pc".into(),
            "bios-iso".into(),
            identity,
            vec![MediaBuildFile {
                role: "bootstrap".into(),
                path: "tracked.txt".into(),
                sha256: sha256_bytes(b"source").to_string(),
                size_bytes: 6,
            }],
        )
        .unwrap();
        let bytes = serde_json::to_vec(&receipt).unwrap();
        let decoded = parse_media_build_receipt(&bytes).unwrap();
        verify_media_build_identity(&decoded, &source, &toolchain).unwrap();
        fs::create_dir(source.join("build")).unwrap();
        fs::write(source.join("build/generated.bin"), b"generated").unwrap();
        verify_media_build_identity(&decoded, &source, &toolchain).unwrap();
        fs::write(source.join("unexpected.txt"), b"untracked source").unwrap();
        assert!(verify_media_build_identity(&decoded, &source, &toolchain).is_err());
        fs::remove_file(source.join("unexpected.txt")).unwrap();

        let mut altered = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
        altered["build_identity"]["source_commit"] = serde_json::json!("0".repeat(40));
        let wrong = parse_media_build_receipt(&serde_json::to_vec(&altered).unwrap()).unwrap();
        assert!(verify_media_build_identity(&wrong, &source, &toolchain).is_err());
        fs::write(source.join("tracked.txt"), b"changed").unwrap();
        assert!(verify_media_build_identity(&decoded, &source, &toolchain).is_err());
        fs::write(source.join("tracked.txt"), b"source").unwrap();
        fs::write(toolchain.join("clang"), b"changed compiler").unwrap();
        assert!(verify_media_build_identity(&decoded, &source, &toolchain).is_err());
    }

    #[test]
    fn parser_rejects_unknown_fields_unsafe_paths_and_duplicate_roles() {
        let (_, bytes) = fixture();
        let text = String::from_utf8(bytes).expect("fixture UTF-8");
        assert!(parse_media_build_receipt(
            text.replace("\"files\":", "\"command\":\"sh\",\"files\":")
                .as_bytes()
        )
        .is_err());
        assert!(parse_media_build_receipt(
            text.replace("\"origin\":\"cmake\"", "\"origin\":\"unknown\"")
                .as_bytes()
        )
        .is_err());
        assert!(
            parse_media_build_receipt(text.replace("opensbi-riscv64", "pc-x86_64").as_bytes())
                .is_ok()
        );
        assert!(parse_media_build_receipt(
            text.replace("opensbi-riscv64", "-pc-x86_64").as_bytes()
        )
        .is_err());
        assert!(
            parse_media_build_receipt(text.replace("loader.efi", "../loader.efi").as_bytes())
                .is_err()
        );
        assert!(parse_media_build_receipt(
            text.replace("\"role\":\"uefi-loader\"", "\"role\":\"bad/role\"")
                .as_bytes()
        )
        .is_err());
        assert!(parse_media_build_receipt(
            text.replace("\"role\":\"uefi-loader\"", "\"role\":\"-bad\"")
                .as_bytes()
        )
        .is_err());
        let mut duplicate: serde_json::Value = serde_json::from_str(&text).expect("fixture JSON");
        let first = duplicate["files"][0].clone();
        duplicate["files"]
            .as_array_mut()
            .expect("files array")
            .push(first);
        assert!(parse_media_build_receipt(&serde_json::to_vec(&duplicate).expect("JSON")).is_err());

        duplicate["files"][1]["role"] = serde_json::json!("second-role");
        duplicate["files"][1]["path"] = serde_json::json!("LOADER.EFI");
        assert!(parse_media_build_receipt(&serde_json::to_vec(&duplicate).expect("JSON")).is_err());
        assert!(parse_media_build_receipt(&vec![b'a'; 1024 * 1024 + 1]).is_err());
        assert!(MediaBuildReceipt::new(
            MediaReceiptOrigin::Cmake,
            "opensbi-riscv64".to_string(),
            "milk-v-titan".to_string(),
            "uefi-esp".to_string(),
            vec![MediaBuildFile {
                role: "uefi-loader".to_string(),
                path: "a".repeat(1024 * 1024),
                sha256: sha256_bytes(b"loader bytes").to_string(),
                size_bytes: 12,
            }],
        )
        .is_err());
    }

    #[test]
    fn verification_rejects_missing_required_role_and_modified_file() {
        let (root, bytes) = fixture();
        let receipt = parse_media_build_receipt(&bytes).expect("valid receipt");
        let profile = built_in_media_profiles()
            .expect("profiles")
            .into_iter()
            .find(|entry| entry.profile.model == "milk-v-titan")
            .expect("Titan profile");
        assert!(verify_media_build_receipt(root.path(), &receipt, &profile.profile).is_err());

        let mut one_role = profile.profile;
        one_role
            .required_files
            .retain(|file| file.role == "uefi-loader");
        verify_media_build_receipt(root.path(), &receipt, &one_role).expect("measured file");
        fs::write(root.path().join("loader.efi"), b"changed bytes").expect("tamper");
        assert!(verify_media_build_receipt(root.path(), &receipt, &one_role).is_err());
    }

    #[test]
    fn cmake_receipt_only_claims_built_roles_and_legacy_inventory_stays_compatible() {
        let profile = format!(
            r#"format_version = 1
id = "test-native-sd"
target_preset = "rpi-aarch64"
model = "rpi5"
transport = "native-sd"
medium = "mbr-fat32"
boot_protocol = "pi-firmware"
label = "Test only"

[layout]
kind = "mbr-fat32"
start_lba = 2048
size_bytes = 67108864
label = "AROSBOOT"

[[external_locks]]
id = "firmware-test"
sha256 = "{}"

[[required_files]]
role = "kernel-image"
destination = "kernel8.img"

[[required_files]]
role = "firmware-start"
destination = "start4.elf"
external_input = {{ lock_id = "firmware-test", file_id = "firmware-start" }}
"#,
            "0".repeat(64)
        );
        let profile = parse_media_profile("test", &profile).expect("profile");
        let root = tempfile::tempdir().expect("root");
        fs::write(root.path().join("kernel8.img"), b"kernel").expect("kernel");
        fs::write(root.path().join("start4.elf"), b"firmware").expect("firmware");
        let built = MediaBuildFile {
            role: "kernel-image".to_string(),
            path: "kernel8.img".to_string(),
            sha256: sha256_bytes(b"kernel").to_string(),
            size_bytes: 6,
        };
        let external = MediaBuildFile {
            role: "firmware-start".to_string(),
            path: "start4.elf".to_string(),
            sha256: sha256_bytes(b"firmware").to_string(),
            size_bytes: 8,
        };
        let cmake = MediaBuildReceipt::new(
            MediaReceiptOrigin::Cmake,
            "rpi-aarch64".to_string(),
            "rpi5".to_string(),
            "native-sd".to_string(),
            vec![built.clone()],
        )
        .expect("CMake receipt");
        verify_media_build_receipt(root.path(), &cmake, &profile.profile)
            .expect("CMake only claims built files");

        let mut false_claim = cmake;
        false_claim.files.push(external.clone());
        assert!(verify_media_build_receipt(root.path(), &false_claim, &profile.profile).is_err());

        let legacy = MediaBuildReceipt::new(
            MediaReceiptOrigin::LegacyV1,
            "rpi-aarch64".to_string(),
            "rpi5".to_string(),
            "native-sd".to_string(),
            vec![built, external],
        )
        .expect("legacy inventory");
        verify_media_build_receipt(root.path(), &legacy, &profile.profile)
            .expect("legacy input measurement remains supported");
    }

    #[cfg(unix)]
    #[test]
    fn verification_rejects_a_symlinked_file_or_parent() {
        use std::os::unix::fs::symlink;

        let (root, bytes) = fixture();
        let receipt = parse_media_build_receipt(&bytes).expect("valid receipt");
        let mut profile = built_in_media_profiles()
            .expect("profiles")
            .into_iter()
            .find(|entry| entry.profile.model == "milk-v-titan")
            .expect("Titan profile")
            .profile;
        profile
            .required_files
            .retain(|file| file.role == "uefi-loader");

        fs::rename(root.path().join("loader.efi"), root.path().join("real.efi"))
            .expect("move real file");
        symlink("real.efi", root.path().join("loader.efi")).expect("create symlink");
        assert!(verify_media_build_receipt(root.path(), &receipt, &profile).is_err());

        let parent = root.path().join("artifact");
        symlink(root.path(), &parent).expect("create parent symlink");
        let mut parent_receipt = receipt;
        parent_receipt.files[0].path = "artifact/real.efi".to_string();
        assert!(verify_media_build_receipt(root.path(), &parent_receipt, &profile).is_err());
    }
}
