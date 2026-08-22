#![cfg(target_os = "macos")]

use std::time::Duration;

use aros_macos_disk_claim::{ClaimError, WholeDiskClaim};

/// Exercises the public macOS entry point without ever resolving or claiming
/// a physical device. Validation must reject this slice before a Disk
/// Arbitration session or worker is created.
#[test]
fn invalid_slice_is_rejected_before_disk_arbitration() {
    let Err(error) = WholeDiskClaim::acquire("/dev/disk999999s1", Duration::from_millis(100))
    else {
        panic!("a slice must never reach Disk Arbitration");
    };
    assert!(matches!(error, ClaimError::InvalidDiskName { .. }));
}
