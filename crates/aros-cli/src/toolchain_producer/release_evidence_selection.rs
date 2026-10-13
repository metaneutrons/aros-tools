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
    /// Independent package-byte expectations, not generation or origin proof.
    /// Omission preserves compiler-only selections; exact coverage is checked
    /// against the independently selected release inputs before acquisition.
    #[serde(default)]
    pub sdk_packages: BTreeMap<String, SdkPackageSelection>,
    /// Independently selected SDK generation evidence, not generation or
    /// origin proof. Omission preserves compiler-only selections.
    #[serde(default)]
    pub sdk_generations: Vec<SdkGenerationSelection>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SdkPackageSelection {
    pub tree_sha256: Sha256Digest,
    pub archive_sha256: Sha256Digest,
    pub archive_size: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SdkGenerationSelection {
    pub sdk_id: String,
    pub group_id: String,
    pub target_profile: String,
    pub host: String,
    pub package_dir: PathBuf,
    pub record: SelectedFile,
    pub compatibility: Compatibility,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Lane {
    /// Optional independently selected LLVM SDK product for this compiler lane.
    /// Omission preserves compiler-only compatibility selections.
    #[serde(default)]
    pub llvm_sdk_id: Option<String>,
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
        if lane
            .llvm_sdk_id
            .as_deref()
            .is_some_and(|id| PortableOutputName::new(id).is_err())
        {
            return Err(miette::miette!(
                "LLVM SDK selection must be a portable product ID"
            ));
        }
        for build in &lane.builds {
            validate_selected_path(&build.package_dir)?;
            validate_selected_path(&build.measurement.path)?;
        }
        validate_selected_path(&lane.comparison.path)?;
        validate_selected_path(&lane.compatibility.directory)?;
        validate_selected_path(&lane.compatibility.inputs.path)?;
    }
    for (id, package) in &selection.sdk_packages {
        PortableOutputName::new(id)
            .map_err(|_| miette::miette!("SDK package key must be a portable product ID"))?;
        if package.archive_size == 0
            || package
                .tree_sha256
                .as_str()
                .bytes()
                .all(|byte| byte == b'0')
            || package
                .archive_sha256
                .as_str()
                .bytes()
                .all(|byte| byte == b'0')
        {
            return Err(miette::miette!(
                "SDK package byte expectations must be nonzero"
            ));
        }
    }

    let mut sdk_generation_lanes = std::collections::BTreeSet::new();
    for generation in &selection.sdk_generations {
        validate_sdk_generation_selector(&generation.sdk_id, "SDK ID", safe_sdk_id)?;
        validate_sdk_generation_selector(&generation.group_id, "compiler group ID", safe_group_id)?;
        validate_sdk_generation_selector(
            &generation.target_profile,
            "target profile ID",
            safe_source_preset,
        )?;
        validate_sdk_generation_selector(&generation.host, "host ID", safe_source_preset)?;
        if !sdk_generation_lanes.insert((
            generation.sdk_id.as_str(),
            generation.group_id.as_str(),
            generation.target_profile.as_str(),
            generation.host.as_str(),
        )) {
            return Err(miette::miette!(
                "SDK generation selection contains a duplicate lane"
            ));
        }
        validate_selected_path(&generation.package_dir)?;
        validate_selected_path(&generation.record.path)?;
        validate_selected_path(&generation.compatibility.directory)?;
        validate_selected_path(&generation.compatibility.inputs.path)?;
    }

    Ok(selection)
}

fn validate_sdk_generation_selector(
    value: &str,
    description: &str,
    grammar: fn(&str) -> bool,
) -> miette::Result<()> {
    if !grammar(value) || PortableOutputName::new(value).is_err() {
        return Err(miette::miette!("SDK generation {description} is unsafe"));
    }
    Ok(())
}

fn safe_sdk_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_.+".contains(&byte)
        })
}

fn safe_group_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value
            .bytes()
            .next_back()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn safe_source_preset(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte))
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .next_back()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
}

/// Require exact SDK generation lane coverage from independently bound
/// release inputs. The order of selected metadata is not significant.
pub(super) fn require_sdk_generation_lanes(
    selection: &Selection,
    expected: &[aros_toolchain::sdk_generation_lanes::ExpectedSdkGenerationLane],
) -> miette::Result<()> {
    let actual = selection
        .sdk_generations
        .iter()
        .map(|generation| {
            (
                generation.sdk_id.as_str(),
                generation.group_id.as_str(),
                generation.target_profile.as_str(),
                generation.host.as_str(),
            )
        })
        .collect::<std::collections::BTreeSet<_>>();
    let required = expected
        .iter()
        .map(|lane| (lane.sdk_id(), lane.group_id(), lane.profile(), lane.host()))
        .collect::<std::collections::BTreeSet<_>>();

    if actual.len() != selection.sdk_generations.len()
        || required.len() != expected.len()
        || actual != required
    {
        return Err(crate::observability::native_diagnostic(
            aros_common::Diagnostic::error(
                aros_common::DiagnosticCode::ProducerRecovery,
                aros_common::DiagnosticStage::Configuration,
                "SDK generation selection must exactly cover the independently selected release lanes",
            )
            .with_hint(
                "do not repackage, retarget a tag, or alter a release handoff; restore complete immutable evidence and use a fresh qualification when the failed stage was not packaging alone",
            ),
        ));
    }
    Ok(())
}

/// Selection metadata cannot choose which SDK products belong to a release.
pub(super) fn require_sdk_package_ids<'a>(
    selection: &Selection,
    expected: impl IntoIterator<Item = &'a str>,
) -> miette::Result<()> {
    if !selection
        .sdk_packages
        .keys()
        .map(String::as_str)
        .eq(expected)
    {
        return Err(miette::miette!(
            "SDK package selection must exactly cover the independently selected SDK input set"
        ));
    }
    Ok(())
}

fn validate_selected_path(path: &Path) -> miette::Result<()> {
    let mut normalized = PathBuf::new();
    for part in path.components() {
        normalized.push(part.as_os_str());
    }
    if !path.is_absolute()
        || normalized.as_os_str() != path.as_os_str()
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
    use std::collections::BTreeMap;

    use aros_common::sha256_bytes;
    use serde_json::{json, Value};

    use super::{
        parse, require_sdk_generation_lanes, require_sdk_package_ids, MAX_SELECTION_BYTES,
        SELECTION_SCHEMA,
    };

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

    fn valid_sdk_generation() -> Value {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        json!({
            "sdk_id":"core-sdk",
            "group_id":"gnu-rv32",
            "target_profile":"rv32-aros",
            "host":"linux-x86_64",
            "package_dir":"/tmp/release-evidence/sdk-package-core-sdk",
            "record":{"path":"/tmp/release-evidence/sdk-generation-record.json","sha256":digest},
            "compatibility":{
                "directory":"/tmp/release-evidence/sdk-compatibility",
                "inputs":{"path":"/tmp/release-evidence/sdk-compatibility-inputs.json","sha256":digest},
                "manifest_sha256":digest,
                "native_sdk_required":true
            }
        })
    }

    fn sdk_generation_input_fixture() -> aros_toolchain::release_inputs::ReleaseInputs {
        const PRODUCER_COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        const TOOLS_COMMIT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let source_lock = serde_json::to_vec(&json!({
            "schema":"aros-toolchain-source-lock-v3",
            "family":"gnu",
            "version":"16.2.0",
            "sources":[
                {
                    "component":"gcc","version":"16.2.0","purpose":"toolchain-component",
                    "filename":"gcc.tar.xz","url":"https://example.invalid/gcc.tar.xz",
                    "sha256":"1".repeat(64),"size":1
                },
                {
                    "component":"binutils","version":"2.47","purpose":"toolchain-component",
                    "filename":"binutils.tar.xz","url":"https://example.invalid/binutils.tar.xz",
                    "sha256":"2".repeat(64),"size":1
                }
            ],
            "host_python_packages":[{
                "name":"mako","version":"1.3.10","filename":"mako.tar.gz",
                "url":"https://example.invalid/mako.tar.gz","sha256":"3".repeat(64),
                "size":1,"source_root":"mako","python_path":"."
            }]
        }))
        .unwrap();
        let profile = serde_json::to_vec(&json!({
            "schema":"aros-toolchain-profiles-v2",
            "family":"gnu",
            "upstream_commit":"1111111111111111111111111111111111111111",
            "profiles":[{
                "name":"rv32-aros","configure_target":"riscv-aros",
                "upstream_output_target":"riscv-aros","target_triple":"riscv-aros",
                "cpu":"riscv","platform":"fixture","float_abi":"ilp32f",
                "capabilities":["c","libgcc","standalone-collector"],
                "target":{
                    "schema":"aros-riscv-target-v1","isa":"rv32imafc","abi":"ilp32f",
                    "code_model":"medany","architecture":"rv32i2p1_m2p0_a2p1_f2p2_c2p0",
                    "unaligned_access":false,"atomic_abi":0,"x3_reg_usage":0
                }
            }]
        }))
        .unwrap();
        let mut recipe = json!({
            "schema":"aros-toolchain-recipe-v2",
            "source_commit":"1111111111111111111111111111111111111111",
            "source_tree":"7".repeat(40),
            "producer_commit":PRODUCER_COMMIT,
            "producer_tree":"e".repeat(40),
            "tools_commit":TOOLS_COMMIT,
            "tools_tree":"f".repeat(40),
            "source_date_epoch":0,
            "source_lock_sha256":sha256_bytes(&source_lock),
            "profiles_sha256":sha256_bytes(&profile),
            "patches":[]
        });
        recipe["recipe_sha256"] = json!(sha256_bytes(
            &aros_toolchain::canonical::bytes(&recipe).unwrap()
        ));
        let recipe = serde_json::to_vec(&recipe).unwrap();
        let identity = serde_json::to_vec(&json!({
            "sdk_id":"core-sdk",
            "version":"1.2.3",
            "target":{
                "target_triple":"i686-unknown-aros",
                "abi":{"target_triple":"i686-unknown-aros","data_model":"ilp32"}
            },
            "source":{
                "commit":"0000000000000000000000000000000000000003",
                "git_tree":"000000000000000000000000000000000000000d",
                "content_sha256":"0000000000000000000000000000000000000000000000000000000000000017",
                "contract_sha256":"0000000000000000000000000000000000000000000000000000000000000021"
            },
            "source_date_epoch":1234
        }))
        .unwrap();
        let reference =
            |file: &str, bytes: &[u8]| json!({"file":file,"sha256":sha256_bytes(bytes)});
        let documents = BTreeMap::from([
            ("gnu-rv32-recipe.json".to_owned(), recipe.clone()),
            ("gnu-rv32-source-lock.json".to_owned(), source_lock.clone()),
            ("gnu-rv32-profiles.json".to_owned(), profile.clone()),
            ("core-sdk-identity.json".to_owned(), identity.clone()),
        ]);
        let collection = serde_json::to_vec(&json!({
            "schema":"aros-toolchain-release-inputs-v2",
            "producer_commit":PRODUCER_COMMIT,
            "tools_commit":TOOLS_COMMIT,
            "hosts":aros_toolchain::release_inputs::ACTIVE_HOSTS,
            "groups":[{
                "id":"gnu-rv32",
                "recipe":reference("gnu-rv32-recipe.json", &recipe),
                "source_lock":reference("gnu-rv32-source-lock.json", &source_lock),
                "profiles":reference("gnu-rv32-profiles.json", &profile)
            }],
            "sdks":[{
                "id":"core-sdk",
                "identity":reference("core-sdk-identity.json", &identity),
                "source_preset":"aros-sdk-source-v1",
                "compiler_profiles":[{"group_id":"gnu-rv32","profile":"rv32-aros"}]
            }]
        }))
        .unwrap();
        aros_toolchain::release_inputs::ReleaseInputs::parse(&collection, &documents).unwrap()
    }

    fn sdk_generation_selection_for_expected_inputs() -> Value {
        let inputs = sdk_generation_input_fixture();
        let generations = inputs
            .expected_sdk_generation_lanes()
            .iter()
            .map(|lane| {
                let mut generation = valid_sdk_generation();
                generation["sdk_id"] = json!(lane.sdk_id());
                generation["group_id"] = json!(lane.group_id());
                generation["target_profile"] = json!(lane.profile());
                generation["host"] = json!(lane.host());
                generation
            })
            .collect::<Vec<_>>();
        let mut selection = valid_selection();
        selection["sdk_generations"] = json!(generations);
        selection
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
        assert!(selection.sdk_generations.is_empty());
    }

    #[test]
    fn parses_closed_sdk_generation_selection() {
        let mut value = valid_selection();
        value["sdk_generations"] = json!([valid_sdk_generation()]);
        let selection = parse(&serde_json::to_vec(&value).unwrap()).unwrap();

        assert_eq!(selection.sdk_generations.len(), 1);
        let generation = &selection.sdk_generations[0];
        assert_eq!(generation.sdk_id, "core-sdk");
        assert_eq!(generation.group_id, "gnu-rv32");
        assert_eq!(generation.target_profile, "rv32-aros");
        assert_eq!(generation.host, "linux-x86_64");
        assert!(generation.compatibility.native_sdk_required);
    }

    #[test]
    fn requires_order_independent_exact_sdk_generation_lane_coverage() {
        let inputs = sdk_generation_input_fixture();
        let expected = inputs.expected_sdk_generation_lanes();
        let mut value = sdk_generation_selection_for_expected_inputs();
        value["sdk_generations"].as_array_mut().unwrap().reverse();
        let selection = parse(&serde_json::to_vec(&value).unwrap()).unwrap();

        require_sdk_generation_lanes(&selection, &expected).unwrap();
    }

    #[test]
    fn rejects_missing_extra_and_changed_sdk_generation_lanes() {
        let inputs = sdk_generation_input_fixture();
        let expected = inputs.expected_sdk_generation_lanes();

        let mut missing = sdk_generation_selection_for_expected_inputs();
        missing["sdk_generations"].as_array_mut().unwrap().pop();
        let selection = parse(&serde_json::to_vec(&missing).unwrap()).unwrap();
        assert!(require_sdk_generation_lanes(&selection, &expected).is_err());

        let mut extra = sdk_generation_selection_for_expected_inputs();
        let mut generation = valid_sdk_generation();
        generation["host"] = json!("linux-extra");
        extra["sdk_generations"]
            .as_array_mut()
            .unwrap()
            .push(generation);
        let selection = parse(&serde_json::to_vec(&extra).unwrap()).unwrap();
        assert!(require_sdk_generation_lanes(&selection, &expected).is_err());

        let mut changed = sdk_generation_selection_for_expected_inputs();
        changed["sdk_generations"][0]["target_profile"] = json!("other-profile");
        let selection = parse(&serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(require_sdk_generation_lanes(&selection, &expected).is_err());
    }

    #[test]
    fn rejects_duplicate_sdk_generation_lane_tuples() {
        let mut value = sdk_generation_selection_for_expected_inputs();
        let duplicate = value["sdk_generations"][0].clone();
        value["sdk_generations"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn requires_explicit_sdk_generation_native_sdk_policy() {
        let mut value = valid_selection();
        value["sdk_generations"] = json!([valid_sdk_generation()]);
        assert!(
            parse(&serde_json::to_vec(&value).unwrap())
                .unwrap()
                .sdk_generations[0]
                .compatibility
                .native_sdk_required
        );

        value["sdk_generations"][0]["compatibility"]["native_sdk_required"] = json!(false);
        assert!(
            !parse(&serde_json::to_vec(&value).unwrap())
                .unwrap()
                .sdk_generations[0]
                .compatibility
                .native_sdk_required
        );

        value["sdk_generations"][0]["compatibility"]
            .as_object_mut()
            .unwrap()
            .remove("native_sdk_required");
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());

        value["sdk_generations"][0]["compatibility"]["native_sdk_required"] = json!("false");
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn rejects_unknown_fields_and_unsafe_sdk_generation_selectors_or_paths() {
        let mut value = valid_selection();
        value["sdk_generations"] = json!([valid_sdk_generation()]);
        value["sdk_generations"][0]["unexpected"] = json!(true);
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());

        for (field, unsafe_value) in [
            ("sdk_id", "../escape"),
            ("group_id", "Uppercase"),
            ("target_profile", "../profile"),
            ("host", "not/a/host"),
            ("package_dir", "relative/package"),
            ("record", "relative/record.json"),
            ("compatibility.directory", "/tmp/sdk/../other"),
            ("compatibility.inputs", "relative/inputs.json"),
        ] {
            let mut value = valid_selection();
            value["sdk_generations"] = json!([valid_sdk_generation()]);
            match field {
                "compatibility.directory" => {
                    value["sdk_generations"][0]["compatibility"]["directory"] = json!(unsafe_value);
                }
                "compatibility.inputs" => {
                    value["sdk_generations"][0]["compatibility"]["inputs"]["path"] =
                        json!(unsafe_value);
                }
                "record" => {
                    value["sdk_generations"][0]["record"]["path"] = json!(unsafe_value);
                }
                _ => value["sdk_generations"][0][field] = json!(unsafe_value),
            }
            assert!(
                parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{field}"
            );
        }
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
    fn lane_llvm_sdk_product_id_is_optional_and_portable() {
        let mut value = valid_selection();
        let asset = "llvm-pc-x86_64-linux.tar.xz";
        assert_eq!(
            parse(&serde_json::to_vec(&value).unwrap()).unwrap().lanes[asset].llvm_sdk_id,
            None
        );

        value["lanes"][asset]["llvm_sdk_id"] = json!("aros-sdk-v1");
        assert_eq!(
            parse(&serde_json::to_vec(&value).unwrap()).unwrap().lanes[asset]
                .llvm_sdk_id
                .as_deref(),
            Some("aros-sdk-v1")
        );

        for unsafe_id in ["", "../sdk", "sdk/name", "CON"] {
            value["lanes"][asset]["llvm_sdk_id"] = json!(unsafe_id);
            assert!(
                parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{unsafe_id:?}"
            );
        }
    }

    #[test]
    fn sdk_package_selection_is_optional_only_for_an_empty_independent_product_set() {
        let bytes = serde_json::to_vec(&valid_selection()).unwrap();
        let compiler_only = parse(&bytes).unwrap();
        assert!(compiler_only.sdk_packages.is_empty());
        require_sdk_package_ids(&compiler_only, []).unwrap();
        assert!(require_sdk_package_ids(&compiler_only, ["core-sdk"]).is_err());

        let mut value = valid_selection();
        let package = json!({
            "tree_sha256":"a".repeat(64),
            "archive_sha256":"b".repeat(64),
            "archive_size":123
        });
        value["sdk_packages"] = json!({"second-sdk":package,"core-sdk":package});
        let selected = parse(&serde_json::to_vec(&value).unwrap()).unwrap();
        require_sdk_package_ids(&selected, ["core-sdk", "second-sdk"]).unwrap();
        assert_eq!(selected.sdk_packages["core-sdk"].archive_size, 123);
        assert!(require_sdk_package_ids(&selected, []).is_err());
        assert!(require_sdk_package_ids(&selected, ["core-sdk"]).is_err());
        assert!(require_sdk_package_ids(&selected, ["core-sdk", "other-sdk"]).is_err());
    }

    #[test]
    fn rejects_incomplete_unknown_or_invalid_sdk_byte_expectations() {
        let package = json!({
            "tree_sha256":"a".repeat(64),
            "archive_sha256":"b".repeat(64),
            "archive_size":123
        });
        for field in ["tree_sha256", "archive_sha256", "archive_size"] {
            let mut value = valid_selection();
            value["sdk_packages"] = json!({"core-sdk":package});
            value["sdk_packages"]["core-sdk"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        for (field, bad) in [
            ("tree_sha256", json!("0".repeat(64))),
            ("archive_sha256", json!("0".repeat(64))),
            ("archive_sha256", json!("invalid")),
            ("archive_size", json!(0)),
            ("archive_size", json!("123")),
            ("identity", json!({})),
        ] {
            let mut value = valid_selection();
            value["sdk_packages"] = json!({"core-sdk":package});
            value["sdk_packages"]["core-sdk"][field] = bad;
            assert!(
                parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{field}"
            );
        }
        let mut value = valid_selection();
        value["sdk_packages"] = json!({"../escape":package});
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
        value["sdk_packages"] = Value::Null;
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn rejects_duplicate_sdk_products_and_measurement_keys() {
        let mut value = valid_selection();
        value["sdk_packages"] = json!({
            "core-sdk":{
                "tree_sha256":"a".repeat(64),
                "archive_sha256":"b".repeat(64),
                "archive_size":123
            }
        });
        let document = serde_json::to_string(&value).unwrap();
        let duplicated_size = document.replace(
            "\"archive_size\":123",
            "\"archive_size\":123,\"archive_size\":123",
        );
        assert!(parse(duplicated_size.as_bytes()).is_err());
        let package = serde_json::to_string(&value["sdk_packages"]["core-sdk"]).unwrap();
        let duplicated_id = document.replace(
            &format!("\"core-sdk\":{package}"),
            &format!("\"core-sdk\":{package},\"core-sdk\":{package}"),
        );
        assert_ne!(duplicated_id, document);
        assert!(parse(duplicated_id.as_bytes()).is_err());
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
