//! Explicit V2 local index stages. This module never authenticates provenance
//! or grants permission to publish; the protected workflow owns those gates.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path};

use aros_common::{open_regular_file_nofollow, sha256_bytes, Sha256Digest};
use aros_toolchain::release_attestation_manifest_v2::{
    verify_attestation_manifest_v2, write_attestation_manifest_v2, AttestationManifestStageV2,
};
use aros_toolchain::release_checksums_v2_writer::write_final_checksums_v2;
use aros_toolchain::release_index_v2::{NativeReleaseIndexV2, INDEX_NAME, PROVENANCE_NAME};
use aros_toolchain::release_index_v2_builder::MeasuredReleaseIndexRequestV2;
use aros_toolchain::release_index_v2_readback::IndexedPackageReadbackRequestV2;
use aros_toolchain::release_index_v2_writer::write_measured_index_v2;
use aros_toolchain::release_inputs::ReleaseInputs;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use super::{native_error, print_json, IndexArgs, IndexStageArg, ResultFormat};

const MAX_LANE_BYTES: u64 = 16 * 1024 * 1024;
const LANE_SCHEMA: &str = "aros-toolchain-index-lanes-v1";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneInputs {
    schema: String,
    lanes: BTreeMap<String, LaneInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneInput {
    build_environment: Map<String, Value>,
    required_paths: Vec<String>,
}

pub(super) fn run(args: IndexArgs) -> miette::Result<()> {
    // Reject irrelevant stage arguments before reading or writing any input.
    let expected_manifest = match args.stage {
        IndexStageArg::PreAttestation => {
            if args.subject_manifest_sha256.is_some() {
                return Err(miette::miette!(
                    "family-v2 pre-attestation does not accept --subject-manifest-sha256"
                ));
            }
            None
        }
        IndexStageArg::Final => {
            let candidate = args.subject_manifest_sha256.as_deref().ok_or_else(|| {
                miette::miette!("family-v2 final requires --subject-manifest-sha256")
            })?;
            let digest = Sha256Digest::parse(candidate).map_err(|_| {
                miette::miette!("subject manifest SHA-256 must be lowercase 64-hex")
            })?;
            if digest.as_str() != candidate {
                return Err(miette::miette!(
                    "subject manifest SHA-256 must be lowercase 64-hex"
                ));
            }
            Some(digest)
        }
    };
    let lane_path = args
        .lane_inputs
        .as_deref()
        .ok_or_else(|| miette::miette!("family-v2 index requires --lane-inputs"))?;
    let manifest_path = args
        .subject_manifest
        .as_deref()
        .ok_or_else(|| miette::miette!("family-v2 index requires --subject-manifest"))?;
    if lane_path.starts_with(&args.directory) {
        return Err(miette::miette!(
            "lane inputs must be outside the release directory"
        ));
    }
    if matches!(args.stage, IndexStageArg::PreAttestation) {
        preflight_manifest_output(&args.directory, manifest_path)?;
    }
    let lane_bytes = read_metadata(lane_path, "lane inputs")?;
    let lanes = parse_lanes(&lane_bytes)?;
    let mut build_environments = BTreeMap::new();
    let mut required_paths = BTreeMap::new();
    for (asset, lane) in lanes.lanes {
        build_environments.insert(asset.clone(), lane.build_environment);
        required_paths.insert(asset, lane.required_paths);
    }
    let inputs = ReleaseInputs::load(&args.directory).map_err(|error| native_error(&error))?;

    let (index, index_sha256, manifest, checksums) = match args.stage {
        IndexStageArg::PreAttestation => {
            let measured = MeasuredReleaseIndexRequestV2 {
                directory: args.directory.clone(),
                inputs,
                release_id: args.release_id.clone(),
                base_url: args.base_url.clone(),
                build_environments,
                required_paths,
                forbidden_prefixes: args.forbidden_prefixes,
            };
            let written =
                write_measured_index_v2(&measured).map_err(|error| native_error(&error))?;
            let request = IndexedPackageReadbackRequestV2 {
                directory: args.directory.clone(),
                inputs: measured.inputs,
                index: written.index().clone(),
                build_environments: measured.build_environments,
                forbidden_prefixes: measured.forbidden_prefixes,
            };
            // A later write failure retains the index for diagnosis; these are
            // two exclusive outputs, not an atomic multi-file transaction.
            let manifest = write_attestation_manifest_v2(&request, manifest_path)
                .map_err(|error| native_error(&error))?;
            (
                written.index().clone(),
                written.sha256().clone(),
                manifest,
                None,
            )
        }
        IndexStageArg::Final => {
            let bytes = read_metadata(&args.directory.join(INDEX_NAME), "release index")?;
            let index = NativeReleaseIndexV2::parse(&bytes, &inputs)
                .map_err(|error| native_error(&error))?;
            if index.release_id() != args.release_id || index.base_url() != args.base_url {
                return Err(miette::miette!(
                    "family-v2 index differs from selected release identity"
                ));
            }
            // Do not infer required paths from the index being verified. The
            // original independent lane document remains a final-stage input.
            if required_paths.len() != index.artifacts().len()
                || index.artifacts().iter().any(|artifact| {
                    required_paths.get(artifact.asset()).map(Vec::as_slice)
                        != Some(artifact.required_paths())
                })
            {
                return Err(miette::miette!(
                    "family-v2 required paths differ from independent lane inputs"
                ));
            }
            let request = IndexedPackageReadbackRequestV2 {
                directory: args.directory.clone(),
                inputs,
                index: index.clone(),
                build_environments,
                forbidden_prefixes: args.forbidden_prefixes,
            };
            let manifest = verify_attestation_manifest_v2(
                &request,
                manifest_path,
                AttestationManifestStageV2::PreChecksums,
            )
            .map_err(|error| native_error(&error))?;
            if Some(manifest.sha256()) != expected_manifest.as_ref() {
                return Err(miette::miette!(
                    "subject manifest differs from the pre-attestation digest"
                ));
            }
            let written =
                write_final_checksums_v2(&request).map_err(|error| native_error(&error))?;
            let final_manifest = verify_attestation_manifest_v2(
                &request,
                manifest_path,
                AttestationManifestStageV2::Final,
            )
            .map_err(|error| native_error(&error))?;
            if final_manifest.sha256() != manifest.sha256() {
                return Err(miette::miette!(
                    "subject manifest changed during finalization"
                ));
            }
            (index, sha256_bytes(&bytes), final_manifest, Some(written))
        }
    };
    // Re-read independent input bytes before reporting success. A failure
    // retains any exclusive outputs; a caller must provide a quiescent stage.
    if read_metadata(lane_path, "lane inputs")? != lane_bytes {
        return Err(miette::miette!(
            "independent lane inputs changed during index advancement"
        ));
    }
    let mut document = serde_json::json!({
        "schema": "aros-toolchain-producer-stage-v2",
        "operation": "index",
        "release_format": "family-v2",
        "stage": match args.stage { IndexStageArg::PreAttestation => "pre-attestation", IndexStageArg::Final => "final" },
        "release_id": index.release_id(),
        "index": args.directory.join(INDEX_NAME),
        "index_sha256": index_sha256,
        "subject_manifest": manifest_path,
        "subject_manifest_sha256": manifest.sha256(),
        "subject_manifest_size": manifest.size(),
        "subject_count": manifest.members().len(),
        "inventory_count": index.expected_inventory().len(),
        "lane_count": index.artifacts().len(),
    });
    if let Some(checksums) = checksums {
        let provenance = checksums
            .readback()
            .members()
            .iter()
            .find(|member| member.name() == PROVENANCE_NAME)
            .ok_or_else(|| miette::miette!("final checksum readback lacks provenance"))?;
        document["checksums"] = serde_json::json!(checksums.path());
        document["checksums_sha256"] = serde_json::json!(checksums.sha256());
        document["provenance_sha256"] = serde_json::json!(provenance.sha256());
        document["provenance_size"] = serde_json::json!(provenance.size());
    }
    match args.format {
        ResultFormat::Json => print_json(&document),
        ResultFormat::Human => {
            aros_common::outputln!(
                "Native family-v2 index: {}\nSubject manifest: {}\nSubject SHA-256: {}\nLanes: {}\nInventory: {}",
                args.directory.join(INDEX_NAME).display(), manifest_path.display(),
                manifest.sha256(), index.artifacts().len(), index.expected_inventory().len()
            );
            if let Some(checksums) = document.get("checksums_sha256").and_then(Value::as_str) {
                aros_common::outputln!("Final checksums SHA-256: {checksums}");
            }
            Ok(())
        }
    }
}

// Early deterministic output rejection avoids reserving an index when the
// manifest path is already unusable. The library rechecks held descriptors at
// actual creation; this preflight is not a lock or a race-free reservation.
fn preflight_manifest_output(directory: &Path, output: &Path) -> miette::Result<()> {
    if !directory.is_absolute()
        || !output.is_absolute()
        || output.starts_with(directory)
        || output
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(miette::miette!(
            "subject manifest must be absolute and outside the release directory"
        ));
    }
    let parent = output
        .parent()
        .ok_or_else(|| miette::miette!("subject manifest lacks a parent"))?;
    let mut ancestor = std::path::PathBuf::new();
    for part in parent.components() {
        ancestor.push(part);
        let metadata = fs::symlink_metadata(&ancestor)
            .map_err(|_| miette::miette!("subject manifest parent is unavailable"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(miette::miette!(
                "subject manifest parent must have real directory ancestors"
            ));
        }
    }
    let output_parent = fs::symlink_metadata(parent)
        .map_err(|_| miette::miette!("subject manifest parent is unavailable"))?;
    let release = fs::symlink_metadata(directory)
        .map_err(|_| miette::miette!("release directory is unavailable"))?;
    if (output_parent.dev(), output_parent.ino()) == (release.dev(), release.ino()) {
        return Err(miette::miette!(
            "subject manifest parent aliases the release directory"
        ));
    }
    match fs::symlink_metadata(output) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(miette::miette!(
            "subject manifest output already exists or is unavailable"
        )),
    }
}

fn read_metadata(path: &Path, label: &str) -> miette::Result<Vec<u8>> {
    if !path.is_absolute() {
        return Err(miette::miette!(
            "family-v2 {label} must be an absolute path"
        ));
    }
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| miette::miette!("family-v2 {label} must be a safely opened regular file"))?;
    let before = file
        .metadata()
        .map_err(|_| miette::miette!("cannot inspect family-v2 {label}"))?;
    if before.nlink() != 1 || before.len() == 0 || before.len() > MAX_LANE_BYTES {
        return Err(miette::miette!(
            "family-v2 {label} is linked, empty or exceeds the metadata limit"
        ));
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    std::io::Read::by_ref(&mut file)
        .take(MAX_LANE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| miette::miette!("cannot read family-v2 {label}"))?;
    let after = file
        .metadata()
        .map_err(|_| miette::miette!("cannot re-inspect family-v2 {label}"))?;
    if bytes.len() as u64 != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || after.nlink() != 1
    {
        return Err(miette::miette!("family-v2 {label} changed during readback"));
    }
    Ok(bytes)
}

fn parse_lanes(bytes: &[u8]) -> miette::Result<LaneInputs> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_LANE_BYTES {
        return Err(miette::miette!(
            "family-v2 lane inputs exceed the metadata limit"
        ));
    }
    // A Value/BTreeMap parser alone silently collapses duplicate dynamic keys.
    // Reject them at every nesting level before projection into closed structs.
    let unique = serde_json::from_slice::<UniqueJson>(bytes).map_err(|_| {
        miette::miette!("family-v2 lane inputs are malformed or contain duplicate keys")
    })?;
    let lanes: LaneInputs = serde_json::from_value(unique.0)
        .map_err(|_| miette::miette!("family-v2 lane inputs must match the closed lane schema"))?;
    if lanes.schema != LANE_SCHEMA || lanes.lanes.is_empty() {
        return Err(miette::miette!(
            "family-v2 lane input schema or lane set is invalid"
        ));
    }
    Ok(lanes)
}

struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueJson;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(value.into())))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(value.into())))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueJson(Value::Number(number)))
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value.to_owned())))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(value) = seq.next_element::<UniqueJson>()? {
                    items.push(value.0);
                }
                Ok(UniqueJson(Value::Array(items)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut items = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if items.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                    items.insert(key, map.next_value::<UniqueJson>()?.0);
                }
                Ok(UniqueJson(Value::Object(items)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_lanes;

    #[test]
    fn closed_lane_projection_preserves_independent_environment_values() {
        let parsed = parse_lanes(br#"{"schema":"aros-toolchain-index-lanes-v1","lanes":{"fixture.tar.xz":{"build_environment":{"bool":true,"null":null,"signed":-1,"unsigned":1,"fraction":1.5,"array":["text",{"inner":2}]},"required_paths":["fixture"]}}}"#).unwrap();
        let environment = &parsed.lanes["fixture.tar.xz"].build_environment;
        assert_eq!(environment["bool"], serde_json::json!(true));
        assert_eq!(environment["null"], serde_json::Value::Null);
        assert_eq!(environment["signed"], serde_json::json!(-1));
        assert_eq!(environment["unsigned"], serde_json::json!(1));
        assert_eq!(environment["fraction"], serde_json::json!(1.5));
        assert_eq!(environment["array"][1]["inner"], serde_json::json!(2));
    }

    #[test]
    fn lane_parser_rejects_duplicates_at_every_dynamic_object_level() {
        for document in [
            r#"{"schema":"aros-toolchain-index-lanes-v1","schema":"aros-toolchain-index-lanes-v1","lanes":{}}"#,
            r#"{"schema":"aros-toolchain-index-lanes-v1","lanes":{"same":{},"same":{}}}"#,
            r#"{"schema":"aros-toolchain-index-lanes-v1","lanes":{"fixture":{"build_environment":{"key":1,"key":2},"required_paths":["fixture"]}}}"#,
            r#"{"schema":"aros-toolchain-index-lanes-v1","lanes":{"fixture":{"build_environment":{"outer":[{"key":1,"\u006bey":2}]},"required_paths":["fixture"]}}}"#,
            r#"{"schema":"aros-toolchain-index-lanes-v1","lanes":{"fixture":{"build_environment":{},"required_paths":["fixture"],"required_paths":["other"]}}}"#,
        ] {
            assert!(parse_lanes(document.as_bytes()).is_err());
        }
    }

    #[test]
    fn lane_parser_rejects_unknown_fields_schema_and_unbounded_documents() {
        for document in [
            r#"{"schema":"future-v2","lanes":{}}"#,
            r#"{"schema":"aros-toolchain-index-lanes-v1","lanes":{},"extra":true}"#,
            r#"{"schema":"aros-toolchain-index-lanes-v1","lanes":{"fixture":{"build_environment":{},"required_paths":["fixture"],"extra":true}}}"#,
            r#"{"schema":"aros-toolchain-index-lanes-v1","lanes":{}}"#,
            r#"{"schema":"aros-toolchain-index-lanes-v1","lanes":[]} trailing"#,
        ] {
            assert!(parse_lanes(document.as_bytes()).is_err());
        }
        assert!(parse_lanes(&vec![b' '; super::MAX_LANE_BYTES as usize + 1]).is_err());
    }
}
