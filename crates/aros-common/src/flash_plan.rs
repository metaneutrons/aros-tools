//! Read-only planning of source-resolved flash slots and exact regular files.
//!
//! This layer knows sectors and half-open ranges, not boards, partition labels,
//! image formats or device commands. A source adapter must first resolve and
//! verify the layout, producer receipts and each format-specific artifact.
//! A returned plan is a point-in-time observation, not write authorization.

use crate::{
    casefold_path_key, open_regular_file_nofollow, sha256_bytes, sha256_reader, Sha256Digest,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_ROLES: usize = 64;

/// One half-open flash range; zero-length and overflowing ranges are invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FlashRange {
    pub start: u64,
    pub end: u64,
}

/// A required source-resolved role and its exclusively owned flash slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlashSlot {
    pub role: String,
    pub range: FlashRange,
    /// Native artifacts must share the selected native build-set binding.
    /// External inputs instead have their independently checked lock receipt.
    pub native_build: bool,
}

/// Source adapter output. No capacity, offset or board default is inferred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlashLayout {
    pub source_contract_sha256: Sha256Digest,
    pub flash_bytes: u64,
    pub erase_sector_bytes: u64,
    pub slots: Vec<FlashSlot>,
}

/// Exact file declaration from an already verified producer/lock receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlashArtifact {
    pub role: String,
    pub relative_path: String,
    pub sha256: Sha256Digest,
    pub size_bytes: u64,
    pub receipt_sha256: Sha256Digest,
    pub native_build_set_sha256: Option<Sha256Digest>,
}

/// Measured file placement and only the erase sectors needed for that file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlashPlacement {
    pub role: String,
    pub relative_path: String,
    pub sha256: Sha256Digest,
    pub size_bytes: u64,
    pub receipt_sha256: Sha256Digest,
    pub payload: FlashRange,
    pub erase: FlashRange,
}

/// A complete read-only plan for exactly the source adapter's required roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlashPlan {
    pub source_contract_sha256: Sha256Digest,
    /// Digest of the normalized layout returned by the source adapter, not
    /// a claim about the original source document's raw-byte digest.
    pub resolved_layout_sha256: Sha256Digest,
    pub native_build_set_sha256: Sha256Digest,
    pub flash_bytes: u64,
    pub erase_sector_bytes: u64,
    pub placements: Vec<FlashPlacement>,
}

/// Resolve exactly one file per required role without changing any file/device.
///
/// The caller must verify that `layout` actually derives from the selected
/// source contract, and that receipt/build-set digests bind the real source,
/// ABI, feature selection and compiler. Matching digest fields alone is not
/// provenance. This helper remeasures every file with a bounded no-follow read
/// and rejects mixed build sets, unsafe paths, extra/missing roles, overlaps and
/// payload or rounded erase spans outside their exclusive slots. It never
/// erases a whole partition merely because a payload occupies part of it.
/// Consumers must reverify/stage the same bytes before any later publication
/// or separately authorized device write.
///
/// # Errors
/// Rejects malformed layouts/paths, unsafe files, changed contents or identity,
/// incomplete inputs, overflow, non-sector-aligned slots or an invalid span.
pub fn plan_flash_artifacts(
    root: &Path,
    layout: &FlashLayout,
    native_build_set: &Sha256Digest,
    artifacts: &[FlashArtifact],
) -> Result<FlashPlan, String> {
    let slots = validate_layout(layout)?;
    if artifacts.len() != slots.len() {
        return Err("flash artifacts differ from the complete required role set".into());
    }
    let mut roles = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut placements = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let slot = slots
            .get(artifact.role.as_str())
            .ok_or("flash artifact has an unselected role")?;
        if !roles.insert(&artifact.role) {
            return Err("duplicate flash artifact role".into());
        }
        if artifact.relative_path.len() > 4096 {
            return Err("oversized flash artifact path".into());
        }
        let path = PathBuf::from(&artifact.relative_path);
        let key = casefold_path_key(&path).map_err(|_| "unsafe relative flash artifact path")?;
        if !paths.insert(key) {
            return Err("duplicate flash artifact path".into());
        }
        let expected_build_set = slot.native_build.then_some(native_build_set);
        if artifact.native_build_set_sha256.as_ref() != expected_build_set {
            return Err("flash artifact native/external origin or build set differs".into());
        }
        let payload_end = slot
            .range
            .start
            .checked_add(artifact.size_bytes)
            .filter(|end| artifact.size_bytes != 0 && *end <= slot.range.end)
            .ok_or("flash artifact is empty or exceeds its source-owned slot")?;
        let erase_end = payload_end
            .checked_add(layout.erase_sector_bytes - 1)
            .map(|end| end / layout.erase_sector_bytes * layout.erase_sector_bytes)
            .filter(|end| *end <= slot.range.end)
            .ok_or("flash artifact erase span exceeds its source-owned slot")?;
        verify_file(&root.join(path), artifact)?;
        placements.push(FlashPlacement {
            role: artifact.role.clone(),
            relative_path: artifact.relative_path.clone(),
            sha256: artifact.sha256.clone(),
            size_bytes: artifact.size_bytes,
            receipt_sha256: artifact.receipt_sha256.clone(),
            payload: FlashRange {
                start: slot.range.start,
                end: payload_end,
            },
            erase: FlashRange {
                start: slot.range.start,
                end: erase_end,
            },
        });
    }
    placements.sort_by_key(|placement| placement.payload.start);
    let mut normalized = layout.clone();
    normalized.slots.sort_by_key(|slot| slot.range.start);
    let layout_bytes =
        serde_json::to_vec(&normalized).map_err(|_| "cannot encode the resolved flash layout")?;
    Ok(FlashPlan {
        source_contract_sha256: layout.source_contract_sha256.clone(),
        resolved_layout_sha256: sha256_bytes(&layout_bytes),
        native_build_set_sha256: native_build_set.clone(),
        flash_bytes: layout.flash_bytes,
        erase_sector_bytes: layout.erase_sector_bytes,
        placements,
    })
}

pub(crate) fn validate_layout(layout: &FlashLayout) -> Result<BTreeMap<&str, &FlashSlot>, String> {
    if layout.flash_bytes == 0
        || layout.erase_sector_bytes == 0
        || !layout.erase_sector_bytes.is_power_of_two()
        || !layout.flash_bytes.is_multiple_of(layout.erase_sector_bytes)
        || layout.slots.is_empty()
        || layout.slots.len() > MAX_ROLES
        || !layout.slots.iter().any(|slot| slot.native_build)
    {
        return Err("invalid flash geometry or required role set".into());
    }
    let mut slots = BTreeMap::new();
    for (index, slot) in layout.slots.iter().enumerate() {
        if slot.role.is_empty()
            || slot.role.len() > 64
            || !slot.role.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
            || slots.insert(slot.role.as_str(), slot).is_some()
        {
            return Err("invalid or duplicate flash role".into());
        }
        if slot.range.start >= slot.range.end
            || slot.range.end > layout.flash_bytes
            || !slot.range.start.is_multiple_of(layout.erase_sector_bytes)
            || !slot.range.end.is_multiple_of(layout.erase_sector_bytes)
        {
            return Err("flash slot is empty, unaligned or outside flash capacity".into());
        }
        for previous in &layout.slots[..index] {
            if slot.range.start < previous.range.end && previous.range.start < slot.range.end {
                return Err("exclusive flash role slots overlap".into());
            }
        }
    }
    Ok(slots)
}

fn verify_file(path: &Path, expected: &FlashArtifact) -> Result<(), String> {
    let mut file = open_regular_file_nofollow(path).map_err(|_| "unsafe or missing flash input")?;
    let before = file.metadata().map_err(|_| "cannot inspect flash input")?;
    if before.len() != expected.size_bytes {
        return Err("flash input size differs from its receipt".into());
    }
    let limit = expected
        .size_bytes
        .checked_add(1)
        .ok_or("flash input read budget overflows")?;
    let measured =
        sha256_reader(&mut file.by_ref().take(limit)).map_err(|_| "cannot measure flash input")?;
    let after = file.metadata().map_err(|_| "cannot recheck flash input")?;
    if measured.size != expected.size_bytes
        || measured.digest != expected.sha256
        || after.len() != expected.size_bytes
        || before.modified().ok() != after.modified().ok()
    {
        return Err("flash input changed or its SHA-256 differs from the receipt".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (
        tempfile::TempDir,
        FlashLayout,
        Sha256Digest,
        Vec<FlashArtifact>,
    ) {
        let root = tempfile::tempdir().unwrap();
        let build_set = sha256_bytes(b"matched native source/ABI/features/compiler");
        let slots = [
            ("bootloader", 0x2000, 0x8000, false, 0x5801),
            ("partition-table", 0x8000, 0x9000, false, 0xc00),
            ("core", 0x0002_0000, 0x0082_0000, true, 0x0001_0003),
            ("bsp", 0x0082_0000, 0x00c0_0000, true, 0x0002_0001),
            ("developer", 0x00c0_0000, 0x0100_0000, true, 0x0040_0000),
        ];
        let mut artifacts = Vec::new();
        let mut layout = FlashLayout {
            source_contract_sha256: sha256_bytes(b"source export"),
            flash_bytes: 32 << 20,
            erase_sector_bytes: 0x1000,
            slots: Vec::new(),
        };
        for (role, start, end, native_build, size) in slots {
            layout.slots.push(FlashSlot {
                role: role.into(),
                range: FlashRange { start, end },
                native_build,
            });
            let relative_path = format!("{role}.bin");
            let bytes = vec![0x5a; size];
            fs::write(root.path().join(&relative_path), &bytes).unwrap();
            artifacts.push(FlashArtifact {
                role: role.into(),
                relative_path,
                sha256: sha256_bytes(&bytes),
                size_bytes: size as u64,
                receipt_sha256: sha256_bytes(role.as_bytes()),
                native_build_set_sha256: native_build.then(|| build_set.clone()),
            });
        }
        (root, layout, build_set, artifacts)
    }

    #[test]
    fn shared_partition_roles_keep_disjoint_minimal_erase_spans() {
        let (root, layout, build_set, artifacts) = fixture();
        let plan = plan_flash_artifacts(root.path(), &layout, &build_set, &artifacts).unwrap();
        assert_eq!(plan.placements[3].payload.end, 0x0084_0001);
        assert_eq!(plan.placements[3].erase.end, 0x0084_1000);
        assert_eq!(plan.placements[4].erase.start, 0x00c0_0000);
        assert_eq!(plan.placements[4].erase.end, 0x0100_0000);
        assert_eq!(plan.placements[0].erase.end, 0x8000);
        let mut shuffled = layout;
        shuffled.slots.reverse();
        let mut reversed = artifacts;
        reversed.reverse();
        assert_eq!(
            plan,
            plan_flash_artifacts(root.path(), &shuffled, &build_set, &reversed).unwrap()
        );
    }

    #[test]
    fn unsafe_geometry_and_incomplete_or_mixed_inputs_fail_closed() {
        let (root, layout, build_set, artifacts) = fixture();
        for (index, start, end) in [
            (3, 0x0082_0000, 0x00c0_1000),
            (3, 0x0082_0001, 0x00c0_0000),
            (4, 0x00c0_0000, 0x00c0_0000),
            (4, 0x00c0_0000, 0x0300_0000),
        ] {
            let mut altered = layout.clone();
            altered.slots[index].range = FlashRange { start, end };
            assert!(plan_flash_artifacts(root.path(), &altered, &build_set, &artifacts).is_err());
        }
        for index in 0..artifacts.len() {
            let mut altered = artifacts.clone();
            altered[index].native_build_set_sha256 = Some(sha256_bytes(b"other build"));
            assert!(plan_flash_artifacts(root.path(), &layout, &build_set, &altered).is_err());
            altered[index] = artifacts[index].clone();
            altered[index].size_bytes = layout.slots[index].range.end;
            assert!(plan_flash_artifacts(root.path(), &layout, &build_set, &altered).is_err());
        }
        let mut missing = artifacts.clone();
        missing.pop();
        assert!(plan_flash_artifacts(root.path(), &layout, &build_set, &missing).is_err());
        let mut unselected = artifacts.clone();
        unselected[2].role = "unselected".into();
        assert!(plan_flash_artifacts(root.path(), &layout, &build_set, &unselected).is_err());
        let mut duplicate = artifacts;
        duplicate[1] = duplicate[0].clone();
        assert!(plan_flash_artifacts(root.path(), &layout, &build_set, &duplicate).is_err());
    }

    #[test]
    fn changed_files_and_symlink_or_aliased_paths_are_refused() {
        let (root, layout, build_set, artifacts) = fixture();
        for bad in [
            "../core.bin",
            "/core.bin",
            "sub/../core.bin",
            "./core.bin",
            "core\\.bin",
        ] {
            let mut altered = artifacts.clone();
            altered[2].relative_path = bad.into();
            assert!(plan_flash_artifacts(root.path(), &layout, &build_set, &altered).is_err());
        }
        let mut alias = artifacts.clone();
        alias[3].relative_path = "CORE.bin".into();
        assert!(plan_flash_artifacts(root.path(), &layout, &build_set, &alias).is_err());
        alias[3].relative_path = "short/".repeat(700);
        assert!(plan_flash_artifacts(root.path(), &layout, &build_set, &alias).is_err());
        let original = fs::read(root.path().join("core.bin")).unwrap();
        let mut changed = original.clone();
        changed[0] ^= 1;
        fs::write(root.path().join("core.bin"), changed).unwrap();
        assert!(plan_flash_artifacts(root.path(), &layout, &build_set, &artifacts).is_err());
        fs::write(root.path().join("core.bin"), &original).unwrap();
        #[cfg(unix)]
        {
            fs::rename(root.path().join("core.bin"), root.path().join("hidden.bin")).unwrap();
            std::os::unix::fs::symlink("hidden.bin", root.path().join("core.bin")).unwrap();
            assert!(plan_flash_artifacts(root.path(), &layout, &build_set, &artifacts).is_err());
        }
    }
}
