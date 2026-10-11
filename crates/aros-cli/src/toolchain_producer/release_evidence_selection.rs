//! Closed input format for independently selected release evidence.
//!
//! These records describe selections only. Parsing them opens no files and
//! authenticates neither evidence origin nor execution.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use aros_common::publication::PortableOutputName;
use aros_common::Sha256Digest;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

const MAX_SELECTION_BYTES: usize = 16 * 1024 * 1024;
const SELECTION_SCHEMA: &str = "aros-toolchain-release-evidence-selection-v2";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    pub schema: String,
    pub lanes: BTreeMap<String, Lane>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Lane {
    pub build_environment: Map<String, Value>,
    pub required_paths: Vec<String>,
    pub builds: [Build; 2],
    pub comparison: SelectedFile,
    pub compatibility: Compatibility,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Build {
    pub package_dir: PathBuf,
    pub measurement: SelectedFile,
    pub executor: ExecutorSelection,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SelectedFile {
    pub path: PathBuf,
    pub sha256: Sha256Digest,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutorSelection {
    pub contract_sha256: Sha256Digest,
    pub binary_sha256: Sha256Digest,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Compatibility {
    pub directory: PathBuf,
    pub inputs: SelectedFile,
    pub manifest_sha256: Sha256Digest,
    /// Independently selected source-contract policy, not a report or inputs
    /// document marker. Mandatory presence prevents silent legacy downgrade.
    pub native_sdk_required: bool,
}

/// Parse bounded, closed selection declarations without opening selected paths.
pub(super) fn parse(bytes: &[u8]) -> miette::Result<Selection> {
    if bytes.is_empty() || bytes.len() > MAX_SELECTION_BYTES {
        return Err(miette::miette!(
            "release evidence selection is empty or exceeds the metadata limit"
        ));
    }

    // Parsing through Value alone collapses duplicate keys. Preserve every
    // object as it is read so duplicates are rejected at every nesting level.
    let unique = serde_json::from_slice::<UniqueJson>(bytes).map_err(|_| {
        miette::miette!("release evidence selection is malformed or contains duplicate keys")
    })?;
    let selection: Selection = serde_json::from_value(unique.0)
        .map_err(|_| miette::miette!("release evidence selection must match the closed schema"))?;
    if selection.schema != SELECTION_SCHEMA || selection.lanes.is_empty() {
        return Err(miette::miette!(
            "release evidence selection schema or lane set is invalid"
        ));
    }

    for (asset, lane) in &selection.lanes {
        PortableOutputName::new(asset).map_err(|_| {
            miette::miette!("release evidence lane key must be a portable basename")
        })?;
        for build in &lane.builds {
            validate_selected_path(&build.package_dir)?;
            validate_selected_path(&build.measurement.path)?;
        }
        validate_selected_path(&lane.comparison.path)?;
        validate_selected_path(&lane.compatibility.directory)?;
        validate_selected_path(&lane.compatibility.inputs.path)?;
    }

    Ok(selection)
}

fn validate_selected_path(path: &Path) -> miette::Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(miette::miette!(
            "release evidence selection paths must be absolute and normalized"
        ));
    }
    Ok(())
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
#[cfg(unix)]
mod tests {
    use serde_json::{json, Value};

    use super::{parse, MAX_SELECTION_BYTES, SELECTION_SCHEMA};

    fn valid_selection() -> Value {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let selected_file = |path: &str| json!({"path":path,"sha256":digest});
        let lane = json!({
            "build_environment": {"compiler-observation":"selected independently"},
            "required_paths": ["bin/clang"],
            "builds": [
                {
                    "package_dir":"/tmp/release-evidence/package-a",
                    "measurement":selected_file("/tmp/release-evidence/measurement-a.json"),
                    "executor":{"contract_sha256":digest,"binary_sha256":digest}
                },
                {
                    "package_dir":"/tmp/release-evidence/package-b",
                    "measurement":selected_file("/tmp/release-evidence/measurement-b.json"),
                    "executor":{"contract_sha256":digest,"binary_sha256":digest}
                }
            ],
            "comparison":selected_file("/tmp/release-evidence/comparison.json"),
            "compatibility":{
                "directory":"/tmp/release-evidence/compatibility",
                "inputs":selected_file("/tmp/release-evidence/compatibility-inputs.json"),
                "manifest_sha256":digest,
                "native_sdk_required":false
            }
        });
        json!({
            "schema":SELECTION_SCHEMA,
            "lanes":{"llvm-pc-x86_64-linux.tar.xz":lane}
        })
    }

    #[test]
    fn parses_closed_selection_with_variable_lane_count_without_opening_paths() {
        let mut value = valid_selection();
        let lane = value["lanes"]["llvm-pc-x86_64-linux.tar.xz"].clone();
        value["lanes"]["second-independent-lane.tar.xz"] = lane;
        let bytes = serde_json::to_vec(&value).unwrap();

        let selection = parse(&bytes).unwrap();

        assert_eq!(selection.schema, SELECTION_SCHEMA);
        assert_eq!(selection.lanes.len(), 2);
        assert_eq!(
            selection.lanes["llvm-pc-x86_64-linux.tar.xz"].builds.len(),
            2
        );
    }

    #[test]
    fn requires_explicit_independent_native_sdk_policy() {
        let mut value = valid_selection();
        value["lanes"]["llvm-pc-x86_64-linux.tar.xz"]["compatibility"]["native_sdk_required"] =
            json!(true);
        let selected = parse(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            selected.lanes["llvm-pc-x86_64-linux.tar.xz"]
                .compatibility
                .native_sdk_required
        );

        value["lanes"]["llvm-pc-x86_64-linux.tar.xz"]["compatibility"]
            .as_object_mut()
            .unwrap()
            .remove("native_sdk_required");
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());

        value["lanes"]["llvm-pc-x86_64-linux.tar.xz"]["compatibility"]["native_sdk_required"] =
            json!("false");
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn rejects_wrong_schema_unknown_fields_and_nonportable_lane_keys() {
        let mut value = valid_selection();
        value["schema"] = json!("future-selection-v2");
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut value = valid_selection();
        value["unexpected"] = json!(true);
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut value = valid_selection();
        value["lanes"]["llvm-pc-x86_64-linux.tar.xz"]["builds"][0]["unexpected"] = json!(true);
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut value = valid_selection();
        let lane = value["lanes"]["llvm-pc-x86_64-linux.tar.xz"].clone();
        value["lanes"] = json!({"../escape.tar.xz":lane});
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn rejects_missing_or_extra_build_side() {
        let mut value = valid_selection();
        value["lanes"]["llvm-pc-x86_64-linux.tar.xz"]["builds"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut value = valid_selection();
        let second = value["lanes"]["llvm-pc-x86_64-linux.tar.xz"]["builds"][1].clone();
        value["lanes"]["llvm-pc-x86_64-linux.tar.xz"]["builds"]
            .as_array_mut()
            .unwrap()
            .push(second);
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn rejects_empty_malformed_and_oversized_documents() {
        assert!(parse(b"").is_err());
        assert!(parse(br#"{"schema":"unfinished""#).is_err());
        assert!(parse(&vec![b' '; MAX_SELECTION_BYTES + 1]).is_err());
        assert!(
            parse(br#"{"schema":"aros-toolchain-release-evidence-selection-v2","lanes":{}}"#)
                .is_err()
        );
    }

    #[test]
    fn rejects_duplicate_keys_at_all_object_depths() {
        for document in [
            br#"{"schema":"aros-toolchain-release-evidence-selection-v2","schema":"aros-toolchain-release-evidence-selection-v2","lanes":{}}"#.as_slice(),
            br#"{"schema":"aros-toolchain-release-evidence-selection-v2","lanes":{"same.tar.xz":{},"same.tar.xz":{}}}"#.as_slice(),
            br#"{"schema":"aros-toolchain-release-evidence-selection-v2","lanes":{"asset.tar.xz":{"build_environment":{"key":1,"key":2}}}}"#.as_slice(),
            br#"{"schema":"aros-toolchain-release-evidence-selection-v2","lanes":{"asset.tar.xz":{"build_environment":{"nested":[{"key":1,"\u006bey":2}]}}}}"#.as_slice(),
            br#"{"schema":"aros-toolchain-release-evidence-selection-v2","lanes":{"asset.tar.xz":{"builds":[],"builds":[]}}}"#.as_slice(),
        ] {
            assert!(parse(document).is_err());
        }
    }

    #[test]
    fn rejects_relative_and_parent_traversal_paths_in_every_path_field() {
        let mut value = valid_selection();
        value["lanes"]["llvm-pc-x86_64-linux.tar.xz"]["builds"][0]["package_dir"] =
            json!("relative/package");
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut value = valid_selection();
        value["lanes"]["llvm-pc-x86_64-linux.tar.xz"]["builds"][0]["measurement"]["path"] =
            json!("/tmp/release-evidence/../measurement.json");
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut value = valid_selection();
        value["lanes"]["llvm-pc-x86_64-linux.tar.xz"]["comparison"]["path"] =
            json!("relative/comparison.json");
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut value = valid_selection();
        value["lanes"]["llvm-pc-x86_64-linux.tar.xz"]["compatibility"]["directory"] =
            json!("/tmp/release-evidence/compatibility/../other");
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut value = valid_selection();
        value["lanes"]["llvm-pc-x86_64-linux.tar.xz"]["compatibility"]["inputs"]["path"] =
            json!("relative/inputs.json");
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
