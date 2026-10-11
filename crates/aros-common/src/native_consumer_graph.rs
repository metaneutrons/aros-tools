//! Shared closed source-graph validation for native and SDK consumers.
//!
//! ABI policy is deliberately outside this module: the legacy native consumer
//! applies its GNU/RISC-V ABI validator, while SDK consumers validate a generic
//! source-owned `SdkTarget`.

use crate::native_build_contract::{
    self, NativeBuildInput, NativeInvocationConfiguration, NativeOptionalMetaDependency,
};
use crate::native_consumer_contract::NativeSdkLinkProbes;
use crate::native_host_generator::NativeHostFileGenerator;
use crate::native_make_template::GeneratedMakeTemplateBinding;
use crate::{ArosError, Result, TargetProfile};
use std::collections::BTreeMap;
use std::path::Path;

/// Borrowed graph fields shared by source-owned native and SDK contracts.
#[derive(Clone, Copy)]
pub struct NativeConsumerGraphView<'a> {
    pub contract_profile: &'a str,
    pub source_baseline: &'a str,
    pub roots: &'a [String],
    pub inputs: &'a [NativeBuildInput],
    pub metamake_projection: &'a str,
    pub make_variables: &'a BTreeMap<String, String>,
    pub host_make_variables: &'a BTreeMap<String, BTreeMap<String, String>>,
    pub make_include_bindings: &'a BTreeMap<String, String>,
    pub generated_make_templates: &'a BTreeMap<String, GeneratedMakeTemplateBinding>,
    pub optional_meta_dependencies: &'a [NativeOptionalMetaDependency],
    pub host_file_generators: &'a [NativeHostFileGenerator],
    pub kernel_compiler_role: Option<&'a str>,
    pub native_sdk_link_probes: Option<&'a NativeSdkLinkProbes>,
}

/// Validate the common sealed graph, measured input closure and invocation policy.
///
/// `namespace` only selects diagnostic attribution; it does not change policy.
pub fn validate_native_consumer_graph(
    root: &Path,
    profile: &TargetProfile,
    graph: NativeConsumerGraphView<'_>,
    namespace: &str,
) -> Result<()> {
    let invalid = |field: &str, message: &str| ArosError::Configuration {
        file: format!("{namespace}/{field}"),
        message: message.into(),
    };

    let (_, profile_bytes) =
        crate::measure_regular_file_bounded(&root.join("aros-targets.toml"), MAX_BYTES)?
            .ok_or_else(|| invalid("profile", "source profile document is absent"))?;
    let profile_text = std::str::from_utf8(&profile_bytes)
        .map_err(|_| invalid("profile", "source profile document is not UTF-8"))?;
    let measured_profiles = TargetProfile::parse_config(profile_text, "aros-targets.toml")?;
    if !measured_profiles
        .targets
        .iter()
        .any(|selected| selected == profile)
    {
        return Err(invalid(
            "profile",
            "caller profile differs from the measured source declaration",
        ));
    }
    if graph.contract_profile != profile.name {
        return Err(invalid("identity", "selected source profile differs"));
    }
    if graph.source_baseline.len() != 40
        || !graph
            .source_baseline
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid(
            "source_baseline",
            "expected a 40-character Git object ID",
        ));
    }
    if graph.roots.is_empty() || graph.roots.len() > 16 {
        return Err(invalid(
            "roots",
            "requires one to 16 explicit source endpoints",
        ));
    }
    for (index, name) in graph.roots.iter().enumerate() {
        if name.is_empty()
            || name.len() > 256
            || matches!(name.as_str(), "." | "..")
            || name.starts_with('-')
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
            || index > 0 && graph.roots[index - 1] >= *name
        {
            return Err(invalid(
                "roots",
                "endpoints must be literal, sorted and unique",
            ));
        }
    }
    // The measured source profile is always part of the sealed input set.
    if !graph.inputs.iter().any(|input| {
        input.path == "aros-targets.toml" && input.sha256 == crate::sha256_bytes(&profile_bytes)
    }) {
        return Err(invalid(
            "inputs",
            "requires the selected source profile document",
        ));
    }
    validate_nofollow_inputs(root, graph.inputs, &invalid)?;
    if let Some(probes) = graph.native_sdk_link_probes {
        validate_native_sdk_link_probes(probes, graph.inputs, &invalid)?;
    }

    let reject = |message: String| invalid("configuration", &message);
    native_build_contract::validate_native_invocation_configuration(
        root,
        NativeInvocationConfiguration {
            inputs: graph.inputs,
            make_variables: graph.make_variables,
            host_make_variables: graph.host_make_variables,
            make_include_bindings: graph.make_include_bindings,
            generated_make_templates: graph.generated_make_templates,
            metamake_projection: Some(graph.metamake_projection),
            optional_meta_dependencies: graph.optional_meta_dependencies,
            host_file_generators: graph.host_file_generators,
            kernel_compiler_role: graph.kernel_compiler_role,
        },
        &reject,
    )
}

const MAX_BYTES: u64 = 1024 * 1024;
pub const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_NATIVE_SDK_PROBE_SOURCE_BYTES: usize = 1024;
const MAX_NATIVE_SDK_PROBE_LIBRARIES: usize = 16;
const MAX_NATIVE_SDK_PROBE_LIBRARY_BYTES: usize = 128;

fn validate_native_sdk_link_probes(
    probes: &NativeSdkLinkProbes,
    inputs: &[NativeBuildInput],
    invalid: &impl Fn(&str, &str) -> ArosError,
) -> Result<()> {
    validate_native_sdk_link_probe("c", &probes.c, "c", inputs, invalid)?;
    validate_native_sdk_link_probe("cxx", &probes.cxx, "cpp", inputs, invalid)
}

fn validate_native_sdk_link_probe(
    language: &str,
    probe: &crate::native_consumer_contract::NativeSdkLinkProbe,
    suffix: &str,
    inputs: &[NativeBuildInput],
    invalid: &impl Fn(&str, &str) -> ArosError,
) -> Result<()> {
    let field = "native_sdk_link_probes";
    if probe.source.len() > MAX_NATIVE_SDK_PROBE_SOURCE_BYTES {
        return Err(invalid(field, "probe source path exceeds its byte bound"));
    }
    let source = native_build_contract::validate_source_relative_path(&probe.source)
        .map_err(|reason| invalid(field, reason))?;
    if source.extension().and_then(|extension| extension.to_str()) != Some(suffix) {
        return Err(invalid(
            field,
            &format!("{language} probe source must use the .{suffix} suffix"),
        ));
    }
    if !inputs.iter().any(|input| input.path == probe.source) {
        return Err(invalid(
            field,
            &format!("{language} probe source must be a sealed input"),
        ));
    }
    if probe.libraries.len() > MAX_NATIVE_SDK_PROBE_LIBRARIES {
        return Err(invalid(field, "probe libraries exceed the 16-item bound"));
    }
    let mut libraries = std::collections::BTreeSet::new();
    for library in &probe.libraries {
        if library.is_empty()
            || library.len() > MAX_NATIVE_SDK_PROBE_LIBRARY_BYTES
            || !library
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            || !library
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
        {
            return Err(invalid(
                field,
                "libraries must be bounded bare literal names, not flags or paths",
            ));
        }
        if !libraries.insert(library) {
            return Err(invalid(field, "probe library names must be unique"));
        }
    }
    Ok(())
}

fn validate_nofollow_inputs(
    root: &Path,
    inputs: &[NativeBuildInput],
    invalid: &impl Fn(&str, &str) -> ArosError,
) -> Result<()> {
    if !(1..=128).contains(&inputs.len()) {
        return Err(invalid(
            "inputs",
            "requires one to 128 measured source files",
        ));
    }
    let mut identities = std::collections::BTreeSet::new();
    for input in inputs {
        let relative = native_build_contract::validate_source_relative_path(&input.path)
            .map_err(|reason| invalid("inputs", reason))?;
        let (identity, bytes) =
            crate::measure_regular_file_bounded(&root.join(relative), MAX_INPUT_BYTES)?
                .ok_or_else(|| invalid("inputs", "declared source input is absent"))?;
        if !identities.insert((identity.device(), identity.inode())) {
            return Err(invalid(
                "inputs",
                "source inputs alias the same regular file",
            ));
        }
        if crate::sha256_bytes(&bytes) != input.sha256 {
            return Err(invalid("inputs", "measured source input digest differs"));
        }
    }
    Ok(())
}
