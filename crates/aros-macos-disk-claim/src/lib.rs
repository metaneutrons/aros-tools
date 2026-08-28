//! A fail-closed RAII owner for a macOS Disk Arbitration whole-disk claim.
//!
//! The public API is safe. Platform FFI is confined to the private `platform`
//! module, which documents every unsafe operation and owns all callback state
//! until its Disk Arbitration session has been unscheduled.
//!
//! This crate deliberately does not unmount disks. Callers must first prove
//! that the selected whole disk and all of its descendants are unmounted. The
//! guard adds exclusive Disk Arbitration ownership around the raw-device open,
//! write, sync, and readback portion of that already-validated workflow.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod platform;

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::time::Duration;

    use crate::{BsdDiskName, ClaimError};

    pub struct ClaimHandle;

    impl ClaimHandle {
        pub fn acquire(_name: BsdDiskName, _timeout: Duration) -> Result<Self, ClaimError> {
            Err(ClaimError::UnsupportedPlatform)
        }

        pub fn release(&mut self) -> Result<(), ClaimError> {
            Ok(())
        }
    }
}

/// Default maximum time to wait for Disk Arbitration to complete a claim.
pub const DEFAULT_CLAIM_TIMEOUT: Duration = Duration::from_secs(5);

/// An exact, canonical whole-disk BSD name such as `disk4`.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BsdDiskName(String);

impl BsdDiskName {
    /// Parses either `diskN` or `/dev/diskN`.
    ///
    /// Slice names (`disk4s1`), raw paths (`/dev/rdisk4`), relative paths,
    /// non-UTF-8 input, leading zeroes, and all other spellings are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`ClaimError::InvalidDiskName`] for any non-canonical whole-disk
    /// identifier.
    pub fn parse(value: impl AsRef<OsStr>) -> Result<Self, ClaimError> {
        let value = value.as_ref();
        let Some(text) = value.to_str() else {
            return Err(ClaimError::InvalidDiskName {
                value: value.to_string_lossy().into_owned(),
                reason: "the name is not valid UTF-8",
            });
        };

        let name = text.strip_prefix("/dev/").unwrap_or(text);
        if text.contains('/') && text.strip_prefix("/dev/").is_none() {
            return Err(ClaimError::InvalidDiskName {
                value: text.to_owned(),
                reason: "only a BSD name or an exact /dev/diskN path is accepted",
            });
        }

        let Some(unit) = name.strip_prefix("disk") else {
            return Err(ClaimError::InvalidDiskName {
                value: text.to_owned(),
                reason: "the name must start with disk",
            });
        };
        if unit.is_empty() || !unit.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ClaimError::InvalidDiskName {
                value: text.to_owned(),
                reason: "only a whole diskN name is accepted; slices are forbidden",
            });
        }
        if unit.len() > 1 && unit.starts_with('0') {
            return Err(ClaimError::InvalidDiskName {
                value: text.to_owned(),
                reason: "the numeric unit must use its canonical spelling",
            });
        }
        if unit.parse::<u32>().is_err() {
            return Err(ClaimError::InvalidDiskName {
                value: text.to_owned(),
                reason: "the numeric unit is out of range",
            });
        }

        Ok(Self(name.to_owned()))
    }

    /// Returns the canonical BSD name without a `/dev/` prefix.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn buffered_device_path(&self) -> PathBuf {
        PathBuf::from(format!("/dev/{}", self.0))
    }

    fn raw_device_path(&self) -> PathBuf {
        PathBuf::from(format!("/dev/r{}", self.0))
    }
}

impl fmt::Display for BsdDiskName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Owns an exclusive Disk Arbitration claim until explicitly released or
/// dropped.
///
/// The guard does not open the raw device. A containing owner must declare
/// its `File` field before this guard so Rust closes the file before this
/// guard's `Drop` implementation unclaims the disk.
#[must_use = "dropping the guard immediately releases the Disk Arbitration claim"]
pub struct WholeDiskClaim {
    name: BsdDiskName,
    device_path: PathBuf,
    raw_device_path: PathBuf,
    claim: platform::ClaimHandle,
}

impl WholeDiskClaim {
    /// Claims an already-unmounted whole BSD disk.
    ///
    /// `device` must be `diskN` or `/dev/diskN`. This method never unmounts a
    /// volume and never accepts a slice or raw-device path. The asynchronous
    /// Disk Arbitration callback is driven on an owned worker run loop, and a
    /// timeout never frees callback state that Disk Arbitration might still
    /// reference.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name or timeout, or when Disk
    /// Arbitration cannot establish the exclusive claim.
    pub fn acquire(device: impl AsRef<OsStr>, timeout: Duration) -> Result<Self, ClaimError> {
        if timeout.is_zero() {
            return Err(ClaimError::InvalidTimeout);
        }

        let name = BsdDiskName::parse(device)?;
        let device_path = name.buffered_device_path();
        let raw_device_path = name.raw_device_path();
        let claim = platform::ClaimHandle::acquire(name.clone(), timeout)?;

        Ok(Self {
            name,
            device_path,
            raw_device_path,
            claim,
        })
    }

    /// Returns the exact BSD name verified by Disk Arbitration.
    #[must_use]
    pub const fn bsd_name(&self) -> &BsdDiskName {
        &self.name
    }

    /// Returns the buffered device node (`/dev/diskN`) used for the claim.
    #[must_use]
    pub fn device_path(&self) -> &Path {
        &self.device_path
    }

    /// Returns the corresponding raw device node (`/dev/rdiskN`) that the
    /// caller may open only while this guard remains alive.
    #[must_use]
    pub fn raw_device_path(&self) -> &Path {
        &self.raw_device_path
    }

    /// Explicitly releases the claim and reports a bounded cleanup failure.
    ///
    /// Normal callers can rely on `Drop`; this method is useful when cleanup
    /// errors should be surfaced before returning from a destructive command.
    ///
    /// # Errors
    ///
    /// Returns an error when Disk Arbitration cannot release the claim within
    /// its bounded cleanup operation.
    pub fn release(mut self) -> Result<(), ClaimError> {
        self.claim.release()
    }
}

/// Failure to validate, acquire, or cleanly release a whole-disk claim.
#[derive(Debug, Error)]
pub enum ClaimError {
    /// Input was not an exact canonical whole BSD disk name.
    #[error("invalid whole-disk name {value:?}: {reason}")]
    InvalidDiskName {
        /// Rejected input rendered lossily for diagnostics.
        value: String,
        /// Static validation rule that failed.
        reason: &'static str,
    },

    /// A zero timeout would not provide a meaningful bounded claim attempt.
    #[error("the Disk Arbitration claim timeout must be greater than zero")]
    InvalidTimeout,

    /// The crate is intentionally macOS-only at runtime.
    #[error("Disk Arbitration whole-disk claims are supported only on macOS")]
    UnsupportedPlatform,

    /// The dedicated callback worker could not be created.
    #[error("failed to start the Disk Arbitration claim worker: {0}")]
    WorkerSpawn(#[source] std::io::Error),

    /// Core Foundation could not provide the objects needed for a session.
    #[error("Disk Arbitration could not create {object}")]
    ObjectUnavailable {
        /// Object that could not be created.
        object: &'static str,
    },

    /// Disk Arbitration did not resolve the requested BSD name.
    #[error("Disk Arbitration could not resolve whole disk {0}")]
    DiskNotFound(BsdDiskName),

    /// The resolved object was not the exact requested whole disk.
    #[error("Disk Arbitration resolved {requested} to unexpected whole disk {actual}")]
    NotExactWholeDisk {
        /// Requested BSD name.
        requested: BsdDiskName,
        /// Whole-disk BSD name returned by Disk Arbitration.
        actual: String,
    },

    /// The whole disk itself still has a mounted volume path.
    #[error("whole disk {0} is mounted; this crate never auto-unmounts")]
    DiskMounted(BsdDiskName),

    /// Disk Arbitration completed the claim with a dissenter.
    #[error("Disk Arbitration rejected the claim for {disk} with status {status}")]
    ClaimRejected {
        /// Requested whole disk.
        disk: BsdDiskName,
        /// `DADissenterGetStatus` result.
        status: i32,
    },

    /// The asynchronous claim did not complete before the supplied deadline.
    #[error("timed out claiming {disk} after {timeout:?}; callback ownership was retained for safe cleanup")]
    ClaimTimedOut {
        /// Requested whole disk.
        disk: BsdDiskName,
        /// Caller-supplied bound.
        timeout: Duration,
    },

    /// The callback worker exited before it reported a result.
    #[error("Disk Arbitration claim worker stopped unexpectedly for {0}")]
    WorkerStopped(BsdDiskName),

    /// Explicit cleanup did not finish inside its fixed safety bound.
    #[error("timed out releasing the Disk Arbitration claim for {0}; the owning worker retained all callback state")]
    ReleaseTimedOut(BsdDiskName),
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{BsdDiskName, ClaimError, WholeDiskClaim};

    #[test]
    fn accepts_only_canonical_whole_disk_names() {
        for input in ["disk0", "disk4", "/dev/disk27"] {
            let parsed = BsdDiskName::parse(input).expect("valid whole-disk name");
            assert!(parsed.as_str().starts_with("disk"));
        }

        for input in [
            "",
            "disk",
            "disk01",
            "disk4s1",
            "rdisk4",
            "/dev/rdisk4",
            "./disk4",
            "/tmp/disk4",
            " disk4",
        ] {
            assert!(matches!(
                BsdDiskName::parse(input),
                Err(ClaimError::InvalidDiskName { .. })
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_utf8_names() {
        use std::os::unix::ffi::OsStringExt;

        let input = OsString::from_vec(vec![b'd', b'i', b's', b'k', 0xff]);
        assert!(matches!(
            BsdDiskName::parse(input),
            Err(ClaimError::InvalidDiskName { .. })
        ));
    }

    #[test]
    fn exposes_buffered_and_raw_paths_without_claiming() {
        let name = BsdDiskName::parse("/dev/disk12").expect("valid name");
        assert_eq!(name.buffered_device_path(), PathBuf::from("/dev/disk12"));
        assert_eq!(name.raw_device_path(), PathBuf::from("/dev/rdisk12"));
    }

    #[test]
    fn rejects_zero_timeout_before_platform_access() {
        assert!(matches!(
            WholeDiskClaim::acquire("disk1", Duration::ZERO),
            Err(ClaimError::InvalidTimeout)
        ));
    }
}
