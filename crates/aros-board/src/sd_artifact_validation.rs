//! Typed SD artifact and board-identity validation.

use super::{
    normalized_sha256, safe_metadata, safe_relative_path, BoardImageExpectation, ImageManifest,
    Path, Result, UsbEcmArtifactIdentity, VerifiedImageArtifact, FORMAT_VERSION, KIND,
    UBOOT_USB_ECM_TRANSPORT,
};
use crate::sd_manifest::ManifestUsbEcmIdentity;

/// Validate schema discriminators, paths, digests, and payload inventory.
pub(super) fn validate_image_manifest(
    manifest: &ImageManifest,
    requested_image_relative_path: &Path,
    requested_image_path: &Path,
) -> Result<()> {
    if manifest.format_version != FORMAT_VERSION {
        miette::bail!(
            "SD image manifest has format_version {}, but this aros version supports image format {}.",
            manifest.format_version,
            FORMAT_VERSION
        );
    }
    if manifest.kind != KIND {
        miette::bail!(
            "SD image manifest kind '{}' is not '{}'. A staging manifest is not a raw image manifest.",
            manifest.kind,
            KIND
        );
    }
    let declared_relative_path = safe_relative_path(
        Path::new(&manifest.image.filename),
        "manifest image.filename",
    )?;
    if declared_relative_path != requested_image_relative_path {
        miette::bail!(
            "SD image manifest declares image '{}', but the explicitly selected image is '{}'.",
            declared_relative_path.display(),
            requested_image_relative_path.display()
        );
    }
    if !requested_image_path.is_file() {
        miette::bail!(
            "Selected SD image '{}' is not a regular file.",
            requested_image_path.display()
        );
    }
    normalized_sha256(&manifest.image.sha256, "SD image manifest.image.sha256")?;
    normalized_sha256(
        &manifest.partition.layout_sha256,
        "SD image manifest.partition.layout_sha256",
    )?;
    normalized_sha256(
        &manifest.source_manifest.sha256,
        "SD image manifest.source_manifest.sha256",
    )?;
    for (index, payload) in manifest.payload.iter().enumerate() {
        normalized_sha256(
            &payload.sha256,
            &format!("SD image manifest.payload[{index}].sha256"),
        )?;
    }
    Ok(())
}

/// Parse and normalize the board identity embedded in an image manifest.
pub(super) fn board_expectation_from_manifest(
    manifest: &ImageManifest,
) -> Result<BoardImageExpectation> {
    let usb_ecm_identity = manifest
        .usb_ecm
        .as_ref()
        .map(usb_ecm_identity_from_manifest);
    let expectation = BoardImageExpectation {
        name: manifest.board.name.clone(),
        model: manifest.board.model.clone(),
        transport: manifest.board.transport.clone(),
        usb_ecm_identity,
    };
    validate_board_expectation(&expectation, "SD image manifest")?;
    normalized_board_expectation(&expectation, "SD image manifest")
}

/// Convert the serialized USB identity into the verifier's immutable type.
pub(super) fn usb_ecm_identity_from_manifest(
    identity: &ManifestUsbEcmIdentity,
) -> UsbEcmArtifactIdentity {
    UsbEcmArtifactIdentity {
        vendor_id: identity.vendor_id,
        product_id: identity.product_id,
        serial: identity.serial.clone(),
        expected_target_mac: identity.expected_target_mac.clone(),
    }
}

/// Enforce the transport-dependent completeness of a board expectation.
pub(super) fn validate_board_expectation(
    expectation: &BoardImageExpectation,
    label: &str,
) -> Result<()> {
    for (field, value) in [
        ("name", expectation.name.as_str()),
        ("model", expectation.model.as_str()),
        ("transport", expectation.transport.as_str()),
    ] {
        if !safe_metadata(value) {
            miette::bail!(
                "{label}.{field} must be non-empty and contain no surrounding whitespace or control characters."
            );
        }
    }

    match (&expectation.transport[..], &expectation.usb_ecm_identity) {
        (UBOOT_USB_ECM_TRANSPORT, Some(identity)) => {
            normalized_usb_ecm_identity(identity, &format!("{label}.usb_ecm"))?;
        }
        (UBOOT_USB_ECM_TRANSPORT, None) => {
            miette::bail!(
                "{label} uses '{UBOOT_USB_ECM_TRANSPORT}' but lacks a complete USB-ECM identity."
            );
        }
        (_, Some(_)) => {
            miette::bail!(
                "{label} declares usb_ecm identity but transport is '{}', not '{UBOOT_USB_ECM_TRANSPORT}'.",
                expectation.transport
            );
        }
        (_, None) => {}
    }
    Ok(())
}

/// Return a validated expectation with canonical nested identity fields.
pub(super) fn normalized_board_expectation(
    expectation: &BoardImageExpectation,
    label: &str,
) -> Result<BoardImageExpectation> {
    validate_board_expectation(expectation, label)?;
    Ok(BoardImageExpectation {
        name: expectation.name.clone(),
        model: expectation.model.clone(),
        transport: expectation.transport.clone(),
        usb_ecm_identity: expectation
            .usb_ecm_identity
            .as_ref()
            .map(|identity| normalized_usb_ecm_identity(identity, &format!("{label}.usb_ecm")))
            .transpose()?,
    })
}

/// Validate IDs and serial data, then normalize the expected target MAC.
pub(super) fn normalized_usb_ecm_identity(
    identity: &UsbEcmArtifactIdentity,
    label: &str,
) -> Result<UsbEcmArtifactIdentity> {
    if identity.vendor_id == 0 || identity.product_id == 0 {
        miette::bail!("{label} must have non-zero USB vendor_id and product_id.");
    }
    if !safe_metadata(&identity.serial) {
        miette::bail!(
            "{label}.serial must be non-empty and contain no surrounding whitespace or control characters."
        );
    }
    Ok(UsbEcmArtifactIdentity {
        vendor_id: identity.vendor_id,
        product_id: identity.product_id,
        serial: identity.serial.clone(),
        expected_target_mac: normalize_unicast_mac(&identity.expected_target_mac, label)?,
    })
}

/// Parse a non-zero unicast MAC and return canonical lowercase notation.
pub(super) fn normalize_unicast_mac(value: &str, label: &str) -> Result<String> {
    let parts = value.split(':').collect::<Vec<_>>();
    if parts.len() != 6 || parts.iter().any(|part| part.len() != 2) {
        miette::bail!(
            "{label}.expected_target_mac '{}' must be a six-octet colon-separated MAC address.",
            value
        );
    }
    let mut bytes = [0_u8; 6];
    for (index, part) in parts.iter().enumerate() {
        bytes[index] = u8::from_str_radix(part, 16).map_err(|error| {
            miette::miette!(
                "{label}.expected_target_mac '{}' has invalid octet '{}': {error}",
                value,
                part
            )
        })?;
    }
    if bytes == [0; 6] || bytes[0] & 1 != 0 {
        miette::bail!(
            "{label}.expected_target_mac '{}' must be a non-zero unicast MAC address.",
            value
        );
    }
    let mut normalized = String::with_capacity(17);
    for (index, byte) in bytes.iter().enumerate() {
        if index != 0 {
            normalized.push(':');
        }
        append_hex_byte(&mut normalized, *byte);
    }
    Ok(normalized)
}

/// Append one byte as two lowercase hexadecimal digits.
pub(super) fn append_hex_byte(output: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.push(char::from(HEX[usize::from(byte >> 4)]));
    output.push(char::from(HEX[usize::from(byte & 0x0f)]));
}

/// Require every embedded board field to equal the selected local profile.
pub(super) fn validate_verified_board_match(
    artifact: &VerifiedImageArtifact,
    expectation: &BoardImageExpectation,
) -> Result<()> {
    let expected = normalized_board_expectation(expectation, "local board expectation")?;
    let actual = &artifact.board;
    if actual.name != expected.name {
        miette::bail!(
            "SD image manifest board.name '{}' does not match selected board '{}'; no disk was opened.",
            actual.name,
            expected.name
        );
    }
    if actual.model != expected.model {
        miette::bail!(
            "SD image manifest board.model '{}' does not match selected board model '{}'; no disk was opened.",
            actual.model,
            expected.model
        );
    }
    if actual.transport != expected.transport {
        miette::bail!(
            "SD image manifest board.transport '{}' does not match selected board transport '{}'; no disk was opened.",
            actual.transport,
            expected.transport
        );
    }
    if actual.usb_ecm_identity != expected.usb_ecm_identity {
        miette::bail!(
            "SD image manifest USB-ECM identity does not match selected board '{}'; no disk was opened.",
            expected.name
        );
    }
    Ok(())
}
