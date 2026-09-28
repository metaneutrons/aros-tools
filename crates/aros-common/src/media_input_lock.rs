//! Reviewed, content-addressed references for external boot-media inputs.
//!
//! This contract records exact bytes and origin metadata. It does not fetch,
//! redistribute, license-check or authenticate upstream hosting. A caller must
//! bind the raw lock digest to a reviewed media profile before using any file.

use crate::media_profile::{valid_slug, ResolvedMediaProfile};
use crate::{open_regular_file_nofollow, sha256_bytes, sha256_reader, Sha256Digest};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;
use url::{Host, Url};

const FORMAT_VERSION: u32 = 1;
const KIND: &str = "aros-media-external-inputs";
const MAX_LOCK_BYTES: usize = 1024 * 1024;
const MAX_FILES: usize = 256;

/// One externally obtained file; neither its URL nor revision proves its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaLockedInput {
    pub id: String,
    pub origin_url: String,
    pub origin_revision: String,
    pub sha256: String,
    pub size_bytes: u64,
    /// Declared license identifier for review, not a legal determination.
    pub license_id: String,
}

/// Closed external-input lock, independent of a local board alias or device.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaInputLock {
    pub format_version: u32,
    pub kind: String,
    pub id: String,
    pub files: Vec<MediaLockedInput>,
}

/// Parsed lock with the digest of the exact input bytes, not reserialized TOML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMediaInputLock {
    pub lock: MediaInputLock,
    pub sha256: Sha256Digest,
}

/// Fail-closed parsing or file-measurement error.
#[derive(Debug, thiserror::Error)]
pub enum MediaInputLockError {
    #[error("invalid media input lock: {0}")]
    Invalid(String),
    #[error("cannot read a locked media input: {0}")]
    Io(#[from] std::io::Error),
}

/// Parse a bounded, closed TOML lock and retain its exact-byte SHA-256.
///
/// # Errors
///
/// Returns an error for unknown fields, unsupported version, duplicate IDs,
/// malformed digest, non-HTTPS origin, missing size or incomplete metadata.
pub fn parse_media_input_lock(bytes: &[u8]) -> Result<ResolvedMediaInputLock, MediaInputLockError> {
    if bytes.len() > MAX_LOCK_BYTES {
        return Err(invalid("lock exceeds its one-megabyte limit"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("lock is not UTF-8"))?;
    let lock: MediaInputLock =
        toml::from_str(text).map_err(|error| invalid(&format!("TOML schema error: {error}")))?;
    validate_lock(&lock)?;
    Ok(ResolvedMediaInputLock {
        lock,
        sha256: sha256_bytes(bytes),
    })
}

/// Bind an exact raw lock to its reviewed identity and SHA-256.
///
/// A profile registry must supply the expected ID and digest. This function
/// does not infer, download or replace either value.
///
/// # Errors
///
/// Returns an error when the lock is invalid or differs from either pin.
pub fn bind_media_input_lock(
    expected_id: &str,
    expected_sha256: &str,
    bytes: &[u8],
) -> Result<ResolvedMediaInputLock, MediaInputLockError> {
    let expected = Sha256Digest::parse(expected_sha256)
        .map_err(|_| invalid("expected lock SHA-256 is malformed"))?;
    let resolved = parse_media_input_lock(bytes)?;
    if resolved.lock.id != expected_id || resolved.sha256 != expected {
        return Err(invalid(
            "lock identity or exact-byte SHA-256 differs from the pin",
        ));
    }
    Ok(resolved)
}

/// Resolve the complete lock set required by one reviewed profile.
///
/// This checks raw lock digests and every external role's file identity. It
/// does not fetch or qualify those files; callers must separately remeasure
/// each locked regular file before composition.
///
/// # Errors
///
/// Returns an error for missing, extra, duplicate, changed or incomplete locks.
pub fn bind_profile_media_input_locks(
    profile: &ResolvedMediaProfile,
    supplied: &[(&str, &[u8])],
) -> Result<Vec<ResolvedMediaInputLock>, MediaInputLockError> {
    if supplied.len() != profile.profile.external_locks.len() {
        return Err(invalid("supplied lock set differs from the profile pins"));
    }
    let mut seen = BTreeSet::new();
    let mut resolved = Vec::with_capacity(supplied.len());
    for (id, bytes) in supplied {
        if !seen.insert(*id) {
            return Err(invalid("duplicate supplied lock ID"));
        }
        let pin = profile
            .profile
            .external_locks
            .iter()
            .find(|pin| pin.id == *id)
            .ok_or_else(|| invalid("supplied lock is not pinned by the profile"))?;
        resolved.push(bind_media_input_lock(&pin.id, &pin.sha256, bytes)?);
    }
    for file in &profile.profile.required_files {
        if let Some(external) = &file.external_input {
            let lock = resolved
                .iter()
                .find(|lock| lock.lock.id == external.lock_id)
                .ok_or_else(|| invalid("external role references a missing lock"))?;
            if !lock
                .lock
                .files
                .iter()
                .any(|input| input.id == external.file_id)
            {
                return Err(invalid("external role references a missing locked file"));
            }
        }
    }
    Ok(resolved)
}

/// Measure one local regular file against a selected, already-bound entry.
///
/// The caller still owns source and destination containment. This read-only
/// function rejects symlinked parents/leaves and never repairs a changed file.
///
/// # Errors
///
/// Returns an error for a symlink, non-regular file, I/O failure or changed
/// length/SHA-256.
pub fn verify_media_locked_input(
    path: &Path,
    input: &MediaLockedInput,
) -> Result<(), MediaInputLockError> {
    let expected =
        Sha256Digest::parse(&input.sha256).map_err(|_| invalid("file SHA-256 is malformed"))?;
    let mut file = open_regular_file_nofollow(path)?;
    let actual = sha256_reader(&mut file)?;
    if actual.size != input.size_bytes || actual.digest != expected {
        return Err(invalid(&format!(
            "measured size or SHA-256 differs for locked input '{}'",
            input.id
        )));
    }
    Ok(())
}

fn validate_lock(lock: &MediaInputLock) -> Result<(), MediaInputLockError> {
    if lock.format_version != FORMAT_VERSION || lock.kind != KIND {
        return Err(invalid("unsupported format_version or kind"));
    }
    if !valid_slug(&lock.id) {
        return Err(invalid("lock ID must be a lowercase slug"));
    }
    if lock.files.is_empty() || lock.files.len() > MAX_FILES {
        return Err(invalid("files must contain between one and 256 entries"));
    }
    let mut ids = BTreeSet::new();
    for input in &lock.files {
        if !valid_slug(&input.id) || !ids.insert(input.id.as_str()) {
            return Err(invalid("file ID is invalid or duplicated"));
        }
        Sha256Digest::parse(&input.sha256).map_err(|_| invalid("file SHA-256 is malformed"))?;
        if input.size_bytes == 0 {
            return Err(invalid("file size must be positive"));
        }
        for (label, value) in [
            ("origin_revision", input.origin_revision.as_str()),
            ("license_id", input.license_id.as_str()),
        ] {
            if value.is_empty()
                || value.len() > 128
                || value.trim() != value
                || value.chars().any(char::is_control)
            {
                return Err(invalid(&format!("{label} is missing or unsafe")));
            }
        }
        validate_origin_url(&input.origin_url)?;
    }
    Ok(())
}

fn validate_origin_url(value: &str) -> Result<(), MediaInputLockError> {
    let url = Url::parse(value).map_err(|_| invalid("origin_url is not a valid URL"))?;
    let Some(Host::Domain(host)) = url.host() else {
        return Err(invalid("origin_url requires a DNS host"));
    };
    let domain_suffix = host.rsplit('.').next().unwrap_or(host);
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port().is_some_and(|port| port != 443)
        || !host.contains('.')
        || host.ends_with('.')
        || matches!(domain_suffix, "localhost" | "local" | "internal")
        || url.path() == "/"
    {
        return Err(invalid("origin_url is not an allowed HTTPS DNS URL"));
    }
    Ok(())
}

fn invalid(message: &str) -> MediaInputLockError {
    MediaInputLockError::Invalid(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        bind_media_input_lock, bind_profile_media_input_locks, parse_media_input_lock,
        verify_media_locked_input,
    };
    use crate::media_profile::parse_media_profile;
    use crate::sha256_bytes;
    use std::fs;

    fn fixture() -> Vec<u8> {
        format!(
            "format_version = 1\nkind = \"aros-media-external-inputs\"\nid = \"firmware-test\"\n\n[[files]]\nid = \"firmware-start\"\norigin_url = \"https://downloads.example.com/firmware/start4.elf\"\norigin_revision = \"test-revision\"\nsha256 = \"{}\"\nsize_bytes = 13\nlicense_id = \"LicenseRef-Test\"\n",
            sha256_bytes(b"firmware data")
        )
        .into_bytes()
    }

    #[test]
    fn binds_exact_raw_lock_and_measures_regular_file() {
        let bytes = fixture();
        let expected = sha256_bytes(&bytes).to_string();
        let lock = bind_media_input_lock("firmware-test", &expected, &bytes).expect("bound lock");
        let root = tempfile::tempdir().expect("root");
        let file = root.path().join("start4.elf");
        fs::write(&file, b"firmware data").expect("fixture file");
        verify_media_locked_input(&file, &lock.lock.files[0]).expect("verified bytes");
        fs::write(&file, b"changed bytes!").expect("tamper");
        assert!(verify_media_locked_input(&file, &lock.lock.files[0]).is_err());
    }

    #[test]
    fn binds_a_profile_to_the_complete_exact_external_file_set() {
        let lock = fixture();
        let profile = format!(
            r#"format_version = 1
id = "test-native-sd"
target_preset = "rpi-aarch64"
model = "rpi5"
transport = "native-sd"
medium = "mbr-fat32"
boot_protocol = "pi-firmware"
label = "Test only"

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
            sha256_bytes(&lock)
        );
        let resolved = parse_media_profile("test", &profile).expect("profile");
        let bound = bind_profile_media_input_locks(&resolved, &[("firmware-test", &lock)])
            .expect("complete exact lock");
        assert_eq!(bound.len(), 1);
        assert!(bind_profile_media_input_locks(&resolved, &[]).is_err());
        assert!(bind_profile_media_input_locks(&resolved, &[("wrong", &lock)]).is_err());
        assert!(
            bind_profile_media_input_locks(&resolved, &[("firmware-test", b"changed")]).is_err()
        );
        assert!(bind_profile_media_input_locks(
            &resolved,
            &[("firmware-test", &lock), ("firmware-test", &lock)]
        )
        .is_err());

        let bad_file = profile.replace("file_id = \"firmware-start\"", "file_id = \"missing\"");
        let parsed = parse_media_profile("test", &bad_file).expect("syntactically valid profile");
        assert!(bind_profile_media_input_locks(&parsed, &[("firmware-test", &lock)]).is_err());
        let orphan = profile.replace(
            "external_input = { lock_id = \"firmware-test\", file_id = \"firmware-start\" }",
            "",
        );
        assert!(parse_media_profile("test", &orphan).is_err());
    }

    #[test]
    fn rejects_changed_lock_unknown_fields_duplicate_ids_and_unsafe_origins() {
        let bytes = fixture();
        let text = String::from_utf8(bytes.clone()).expect("UTF-8");
        let expected = sha256_bytes(&bytes).to_string();
        assert!(bind_media_input_lock("wrong", &expected, &bytes).is_err());
        assert!(bind_media_input_lock("firmware-test", &"0".repeat(64), &bytes).is_err());
        assert!(bind_media_input_lock(
            "firmware-test",
            &expected,
            text.replace("test-revision", "changed-revision").as_bytes(),
        )
        .is_err());
        assert!(parse_media_input_lock(
            text.replace("size_bytes = 13", "size_bytes = 0").as_bytes()
        )
        .is_err());
        assert!(parse_media_input_lock(
            text.replace("license_id =", "command = \"sh\"\nlicense_id =")
                .as_bytes()
        )
        .is_err());
        assert!(parse_media_input_lock(
            text.replace("firmware/start4.elf", "firmware/start4.elf?token=secret")
                .as_bytes()
        )
        .is_err());
        assert!(parse_media_input_lock(
            text.replace("https://downloads.example.com", "http://localhost")
                .as_bytes()
        )
        .is_err());
        for host in ["localhost", "localhost.", "firmware.local", "intranet"] {
            assert!(
                parse_media_input_lock(text.replace("downloads.example.com", host).as_bytes())
                    .is_err()
            );
        }
        let entry = text.split("[[files]]").nth(1).expect("file entry");
        let duplicate = format!("{text}\n[[files]]{entry}");
        assert!(parse_media_input_lock(duplicate.as_bytes()).is_err());
        assert!(parse_media_input_lock(&vec![b'a'; 1024 * 1024 + 1]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_locked_file() {
        use std::os::unix::fs::symlink;

        let lock = parse_media_input_lock(&fixture()).expect("valid lock");
        let root = tempfile::tempdir().expect("root");
        fs::write(root.path().join("actual"), b"firmware data").expect("actual file");
        symlink("actual", root.path().join("linked")).expect("symlink");
        assert!(
            verify_media_locked_input(&root.path().join("linked"), &lock.lock.files[0]).is_err()
        );
    }
}
