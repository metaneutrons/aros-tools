//! Source-owned ESP media geometry, without board defaults or device operations.
//!
//! The source export is an inventoried declaration. Its raw digest and the
//! complete native input set bind the observation; they are not Git provenance,
//! verified producer receipts, native build success or flash authorization.

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::esp_partition::{encode_bound_native_partition_table, NativePartitionArtifact};
use crate::flash_plan::{validate_layout, FlashLayout, FlashRange, FlashSlot};
use crate::native_build_contract::{load_bound_native_build_contract, NativeBuildContract};
use crate::{open_regular_file_nofollow, sha256_bytes, Sha256Digest, TargetProfile};

/// A closed, source-owned export of board and locked-vendor numeric facts.
/// No numbers are inferred from a board/chip name or host configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeEspMediaGeometry {
    pub schema_version: u32,
    pub format: String,
    pub profile: String,
    pub board: String,
    pub chip: String,
    pub chip_id: u16,
    pub revision_min: u16,
    pub revision_max: u16,
    pub idf_version: String,
    pub flash_bytes: u64,
    pub erase_sector_bytes: u64,
    pub bootloader_offset: u64,
    pub partition_table_offset: u32,
    pub core_partition_kind: u8,
    pub core_partition_subtype: u8,
    pub package_partition_kind: u8,
    pub package_partition_subtype: u8,
    pub package_limit_bytes: u64,
    pub development_volume_offset: u64,
    pub development_volume_size_bytes: u64,
}

/// Source consistency binding for the normalized geometry and generated table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeMediaBinding {
    pub schema: &'static str,
    pub qualification: &'static str,
    pub geometry_contract: String,
    pub geometry_contract_sha256: Sha256Digest,
    pub geometry: NativeEspMediaGeometry,
    pub layout: FlashLayout,
    pub partition_binding: crate::esp_partition::NativePartitionBinding,
}

/// In-memory media observations; no core/BSP/Developer artifacts are invented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundNativeMedia {
    pub partition: NativePartitionArtifact,
    pub binding: NativeMediaBinding,
}

/// Resolve geometry only from an inventoried source export, then encode its CSV.
///
/// Validates the full native contract before/after resolution and independently
/// measures the geometry through bounded no-follow reads. The selected CSV
/// determines named core/package partitions; the source export determines the
/// shared package/development split, silicon interval and vendor format facts.
/// This does not execute Make, source scripts, transport or a device operation.
///
/// # Errors
/// Rejects absent/mismatched source declarations, unsafe paths/files, modified
/// inputs, invalid format/geometry, ambiguous roles and overlapping erase slots.
pub fn resolve_bound_native_media(
    source_root: &Path,
    relative_contract: &Path,
    profile: &TargetProfile,
    expected_contract_sha256: &Sha256Digest,
) -> Result<BoundNativeMedia, String> {
    let loaded = load_bound_native_build_contract(source_root, relative_contract, profile)
        .map_err(|error| error.to_string())?;
    if &loaded.sha256 != expected_contract_sha256 {
        return Err("native media source contract SHA-256 differs".into());
    }
    let geometry_path = loaded
        .contract
        .media
        .geometry_contract
        .as_ref()
        .ok_or("selected source has no explicit media geometry contract")?;
    let input = loaded
        .contract
        .inputs
        .iter()
        .find(|input| &input.path == geometry_path)
        .ok_or("media geometry is not in the validated source inventory")?;
    let root = source_root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let mut file = open_regular_file_nofollow(&root.join(geometry_path))
        .map_err(|error| format!("cannot open media geometry: {error}"))?;
    if file.metadata().map_err(|error| error.to_string())?.len() > 64 * 1024 {
        return Err("media geometry exceeds its byte budget".into());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 64 * 1024 || sha256_bytes(&bytes) != input.sha256 {
        return Err("media geometry size or SHA-256 differs from the source inventory".into());
    }
    let geometry: NativeEspMediaGeometry = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid media geometry: {error}"))?;
    geometry.validate(&loaded.contract)?;
    let mut partition = encode_bound_native_partition_table(
        source_root,
        relative_contract,
        profile,
        expected_contract_sha256,
        geometry.partition_table_offset,
        geometry.flash_bytes,
    )?;
    let layout = geometry.resolve_layout(
        &loaded.contract,
        &loaded.sha256,
        &partition.artifact.source.partitions,
    )?;
    validate_layout(&layout)?;
    let revalidated = load_bound_native_build_contract(source_root, relative_contract, profile)
        .map_err(|error| error.to_string())?;
    if revalidated != loaded {
        return Err("native media source contract changed during resolution".into());
    }
    // The lower-level encoder deliberately accepts explicit caller geometry.
    // At this boundary it came from the exact inventoried source declaration.
    partition.binding.geometry_origin = "source-owned-media-geometry";
    Ok(BoundNativeMedia {
        binding: NativeMediaBinding {
            schema: "aros-native-media-binding-v1",
            qualification: "experimental-source-consistency-only",
            geometry_contract: geometry_path.clone(),
            geometry_contract_sha256: input.sha256.clone(),
            geometry,
            layout,
            partition_binding: partition.binding.clone(),
        },
        partition,
    })
}

impl NativeEspMediaGeometry {
    fn validate(&self, contract: &NativeBuildContract) -> Result<(), String> {
        if self.schema_version != 1 || self.format != "esp-unsigned-flash-v1" {
            return Err("unsupported native media geometry schema or format".into());
        }
        if self.profile != contract.profile
            || self.board != contract.board
            || self.chip != contract.media.chip
            || self.idf_version != contract.media.idf_version
        {
            return Err("media geometry source profile/board/chip/IDF identity differs".into());
        }
        // These are container capabilities, not board-specific defaults: ESP
        // v1 images encode power-of-two MiB capacities and this table format
        // requires 4 KiB sectors. All board values remain explicit inputs.
        if !(1 << 20..=1 << 27).contains(&self.flash_bytes)
            || !self.flash_bytes.is_power_of_two()
            || self.erase_sector_bytes != 0x1000
            || self.revision_min > self.revision_max
            || self.core_partition_kind != 0
            || !(0x40..=0xfe).contains(&self.package_partition_kind)
            || self.bootloader_offset >= u64::from(self.partition_table_offset)
            || !self
                .bootloader_offset
                .is_multiple_of(self.erase_sector_bytes)
            || !u64::from(self.partition_table_offset).is_multiple_of(self.erase_sector_bytes)
            || self.package_limit_bytes == 0
            || self.development_volume_size_bytes == 0
        {
            return Err("invalid native ESP format geometry or silicon interval".into());
        }
        Ok(())
    }

    fn resolve_layout(
        &self,
        contract: &NativeBuildContract,
        source_contract_sha256: &Sha256Digest,
        partitions: &[crate::esp_partition::EspPartition],
    ) -> Result<FlashLayout, String> {
        let selected = |name: &str| {
            let mut matching = partitions.iter().filter(|partition| partition.name == name);
            let result = matching
                .next()
                .ok_or("selected media partition is absent")?;
            if matching.next().is_some() || result.flags != 0 {
                return Err("selected media partition is ambiguous or requires unsupported flags");
            }
            Ok(result)
        };
        let core = selected(&contract.media.core_partition)?;
        let package = selected(&contract.media.package_partition)?;
        if core.kind != self.core_partition_kind
            || core.subtype != self.core_partition_subtype
            || package.kind != self.package_partition_kind
            || package.subtype != self.package_partition_subtype
        {
            return Err(
                "media core/package partition formats differ from the selected roles".into(),
            );
        }
        let package_start = u64::from(package.offset);
        let package_end = package_start + u64::from(package.size);
        let package_limit = package_start
            .checked_add(self.package_limit_bytes)
            .ok_or("package limit overflows")?;
        let development_end = self
            .development_volume_offset
            .checked_add(self.development_volume_size_bytes)
            .ok_or("development volume range overflows")?;
        if package_limit > self.development_volume_offset
            || self.development_volume_offset < package_start
            || development_end > package_end
        {
            return Err("package/development split exceeds its shared source partition".into());
        }
        let slots = [
            (
                "bootloader",
                self.bootloader_offset,
                u64::from(self.partition_table_offset),
                false,
            ),
            (
                "partition-table",
                u64::from(self.partition_table_offset),
                u64::from(self.partition_table_offset) + self.erase_sector_bytes,
                false,
            ),
            (
                "core",
                u64::from(core.offset),
                u64::from(core.offset) + u64::from(core.size),
                true,
            ),
            ("bsp", package_start, package_limit, true),
            (
                "developer",
                self.development_volume_offset,
                development_end,
                true,
            ),
        ];
        Ok(FlashLayout {
            source_contract_sha256: source_contract_sha256.clone(),
            flash_bytes: self.flash_bytes,
            erase_sector_bytes: self.erase_sector_bytes,
            slots: slots
                .into_iter()
                .map(|(role, start, end, native_build)| FlashSlot {
                    role: role.into(),
                    range: FlashRange { start, end },
                    native_build,
                })
                .collect(),
        })
    }
}
