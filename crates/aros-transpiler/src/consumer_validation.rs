//! Read-only, pre-compiler validation boundary for the owned CMake engine.
//!
//! This verifies the closed source document, inputs and explicit selectors.
//! It does not walk recipes, prove graph closure, execute a compiler or publish
//! files. Both later transpiler passes must repeat the selected binding.

use crate::cli_args::Args;
use crate::native_selection::{load_native_selection, LoadedNativeSelection};
use aros_common::{native_consumer_contract::NativeConsumerContract, ArosError, Result};
use aros_transpiler::{dirs::DirVars, TargetContext};
use std::path::Path;

const MAKE_CONFIG_PATH: &str = "config/make.cfg.in";
const MAX_MAKE_CONFIG_BYTES: u64 = 16 * 1024 * 1024;

pub fn execute(args: &Args) -> Result<()> {
    let context = TargetContext {
        cpu: args.cpu.clone(),
        platform: args.platform.clone(),
        family: args.family.clone(),
        variant: args.variant.clone(),
        toolchain: args.toolchain.clone(),
        cpu32: args.cpu32.clone(),
        use_mmu: args.use_mmu.clone(),
        float_abi: args.float_abi.clone(),
        mesa_version: args.mesa_version.clone(),
        ..TargetContext::default()
    };
    let Some(LoadedNativeSelection::Consumer(loaded)) =
        load_native_selection(args, Some(&context))?
    else {
        return Err(invalid("an explicit consumer selection is required"));
    };
    // Resolve actual-host substitutions now, rather than accepting a document
    // which cannot represent the host used by the two graph passes.
    loaded
        .contract
        .make_variables_for_host(aros_common::target::native_host_key().unwrap_or(""))?;
    let mut exec_smp = None;
    for binding in loaded.contract.generated_make_templates.values() {
        let Some(value) = binding.substitutions.get("@ENABLE_EXECSMP@") else {
            continue;
        };
        let selected = match value.as_str() {
            "" => false,
            "#define __AROSEXEC_SMP__" => true,
            _ => return Err(invalid("invalid @ENABLE_EXECSMP@ substitution")),
        };
        if exec_smp.is_some_and(|previous| previous != selected) {
            return Err(invalid("templates disagree on @ENABLE_EXECSMP@"));
        }
        exec_smp = Some(selected);
    }
    let source = args
        .source_dir
        .canonicalize()
        .map_err(|_| invalid("cannot resolve the source root"))?;
    let source = source
        .to_str()
        .ok_or_else(|| invalid("source root is not UTF-8"))?;
    let source_path = Path::new(source);
    let (sdk_include_relative, sdk_root_dependencies) =
        sdk_include_relative(source_path, &loaded.contract)?;
    reject_sdk_root_overrides(&loaded.contract, &sdk_root_dependencies)?;
    let contract_path = loaded
        .path
        .to_str()
        .ok_or_else(|| invalid("contract path is not UTF-8"))?;
    let document = serde_json::json!({
        "schema": "aros-native-consumer-validation-v1",
        "qualification": "source-binding-not-graph-or-build-proof",
        "source_dir": source,
        "contract_path": contract_path,
        "contract_sha256": loaded.sha256,
        "profile": loaded.contract.profile,
        "abi": loaded.contract.abi,
        "sdk_include_relative": sdk_include_relative,
        "exec_smp": exec_smp.unwrap_or(false),
        "input_paths": loaded.contract.inputs.iter().map(|input| &input.path).collect::<Vec<_>>()
    });
    aros_common::outputln!("{document}");
    Ok(())
}

fn sdk_include_relative(
    source: &Path,
    contract: &NativeConsumerContract,
) -> Result<(String, std::collections::BTreeSet<String>)> {
    let mut sealed_inputs = contract
        .inputs
        .iter()
        .filter(|input| input.path == MAKE_CONFIG_PATH);
    let config_input = sealed_inputs
        .next()
        .ok_or_else(|| invalid("consumer contract must seal config/make.cfg.in"))?;
    if sealed_inputs.next().is_some() {
        return Err(invalid(
            "consumer contract must contain exactly one config/make.cfg.in input",
        ));
    }

    let config_path = source.join(MAKE_CONFIG_PATH);
    let (identity, bytes) =
        aros_common::measure_regular_file_bounded(&config_path, MAX_MAKE_CONFIG_BYTES)
            .map_err(|_| invalid("cannot safely measure source config/make.cfg.in"))?
            .ok_or_else(|| invalid("source config/make.cfg.in is absent"))?;
    if aros_common::sha256_bytes(&bytes) != config_input.sha256 {
        return Err(invalid(
            "source config/make.cfg.in differs from its sealed consumer input",
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| invalid("source config/make.cfg.in is not UTF-8"))?;
    let dirs = DirVars::from_config_text(text);
    let developer = dirs
        .expand_source_proven("$(AROS_DEVELOPER)")
        .ok_or_else(|| invalid("source AROS_DEVELOPER root cannot be resolved"))?;
    let includes = dirs
        .expand_source_proven("$(AROS_INCLUDES)")
        .ok_or_else(|| invalid("source AROS_INCLUDES root cannot be resolved"))?;
    let developer_relative = build_relative_path(&developer, "AROS_DEVELOPER")?;
    let includes_relative = build_relative_path(&includes, "AROS_INCLUDES")?;
    if includes_relative != format!("{developer_relative}/include") {
        return Err(invalid(
            "source AROS_INCLUDES must be exactly AROS_DEVELOPER/include",
        ));
    }
    let dependencies = dirs
        .source_dependency_closure(&["AROS_DEVELOPER", "AROS_INCLUDES"])
        .ok_or_else(|| invalid("source SDK root dependency closure exceeded its resource limit"))?;

    // Reopen through the same bounded no-follow reader after parsing. Require
    // both path identity and digest to remain stable before returning the
    // source-derived relative value to the CMake consumer.
    let (rechecked_identity, rechecked_bytes) =
        aros_common::measure_regular_file_bounded(&config_path, MAX_MAKE_CONFIG_BYTES)
            .map_err(|_| invalid("cannot recheck source config/make.cfg.in"))?
            .ok_or_else(|| invalid("source config/make.cfg.in disappeared during validation"))?;
    if rechecked_identity != identity
        || rechecked_bytes != bytes
        || aros_common::sha256_bytes(&rechecked_bytes) != config_input.sha256
    {
        return Err(invalid(
            "source config/make.cfg.in changed during consumer validation",
        ));
    }
    Ok((includes_relative, dependencies))
}

fn build_relative_path(value: &str, variable: &str) -> Result<String> {
    const BUILD_PREFIX: &str = "${AROS_BUILD_DIR}/";
    let relative = value
        .strip_prefix(BUILD_PREFIX)
        .ok_or_else(|| invalid(format!("source {variable} is not rooted in AROS_BUILD_DIR")))?;
    if relative.is_empty() || relative.len() > 4096 {
        return Err(invalid(format!(
            "source {variable} has an invalid relative path"
        )));
    }
    for component in relative.split('/') {
        if component.is_empty()
            || matches!(component, "." | "..")
            || !component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
        {
            return Err(invalid(format!(
                "source {variable} contains an unsafe or noncanonical path component"
            )));
        }
    }
    Ok(relative.to_owned())
}

fn reject_sdk_root_overrides(
    contract: &NativeConsumerContract,
    source_dependencies: &std::collections::BTreeSet<String>,
) -> Result<()> {
    let shared = contract.make_variables.keys();
    let hosts = contract
        .host_make_variables
        .values()
        .flat_map(|variables| variables.keys());
    if let Some(name) = shared
        .chain(hosts)
        .find(|name| source_dependencies.contains(name.as_str()))
    {
        return Err(invalid(format!(
            "native make configuration must not override source SDK root dependency variable {name}"
        )));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> ArosError {
    ArosError::Configuration {
        file: "native consumer validation".into(),
        message: message.into(),
    }
}
