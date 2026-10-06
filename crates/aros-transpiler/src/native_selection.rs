//! Native profile selection and architecture endpoint binding for the command boundary.

use crate::cli_args::Args;
use crate::error_mapping::diagnostics_error;
use aros_common::{
    ArosError, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage, Result,
    SourceLocation,
};
use aros_transpiler::{DependencyGraph, TargetContext};
use std::path::Path;

pub fn bind_architecture_effects(
    projection: &aros_transpiler::native_owner_projection::NativeOwnerProjection,
    root: &Path,
    context: &TargetContext,
    graph: &mut DependencyGraph,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    use aros_transpiler::arch_endpoint_effects::ArchEndpointEffectData;
    let candidates = std::mem::take(&mut graph.arch_endpoint_effects);
    for effect in candidates
        .iter()
        .filter(|effect| effect.applies_to(context))
    {
        let mut proof = vec![effect.clone()];
        if let ArchEndpointEffectData::ArchModuleObjects { mainmmake, tag, .. } = &effect.data {
            proof.extend(candidates.iter().filter(|candidate| {
                candidate.recipe == effect.recipe && candidate.line == effect.line
                    && matches!(&candidate.data, ArchEndpointEffectData::EmptyLinklibAggregate {
                        mainmmake: owner, tag: candidate_tag, ..
                    } if owner == mainmmake && candidate_tag == tag)
            }).cloned());
        }
        let verified = projection.verify_arch_endpoint_effects(root, &proof).and_then(|()| {
            if let ArchEndpointEffectData::ArchModuleObjects { mainmmake, tag, directory, module_sources } = &effect.data {
                let base = graph.targets.contains_key(mainmmake)
                    || graph.inventory_targets.iter().any(|target| &target.mmake_name == mainmmake);
                let expected: std::collections::BTreeSet<_> = module_sources.iter().collect();
                let paired = graph.arch_sources.get(mainmmake).is_some_and(|sources| sources.iter().any(|source| {
                    &source.tag == tag && &source.dir == directory
                        && source.files.iter().collect::<std::collections::BTreeSet<_>>() == expected
                }));
                if !base || !paired {
                    return Err("architecture objects lack their exact module/source compilation binding".into());
                }
            }
            Ok(())
        });
        match verified {
            Ok(()) => graph.arch_endpoint_effects.push(effect.clone()),
            Err(message) => diagnostics.push(
                Diagnostic::error(
                    DiagnosticCode::CapabilityDrift,
                    DiagnosticStage::CapabilityValidation,
                    format!(
                        "architecture endpoint {} is outside its closed capability: {message}",
                        effect.endpoint
                    ),
                )
                .with_context(DiagnosticContext {
                    target: Some(effect.endpoint.clone()),
                    ..Default::default()
                })
                .with_location(SourceLocation {
                    path: effect.recipe.clone(),
                    line: Some(effect.line),
                    column: None,
                })
                .with_hint("review this source/template declaration and update the proven transpiler capability; no architecture producer or optionality is inferred"),
            ),
        }
    }
    projection.verify(root).map_err(|message| {
        native_selection_input_error(ArosError::Configuration {
            file: "architecture endpoint inputs".into(),
            message,
        })
    })
}

/// A rejected caller/source contract is an expected input failure, not an
/// internal invariant failure. Keep unrelated configuration errors unchanged.
pub fn native_selection_input_error(error: ArosError) -> ArosError {
    match error {
        ArosError::Configuration { file, message } => diagnostics_error(vec![
            Diagnostic::error(DiagnosticCode::GraphValidation,
                DiagnosticStage::GraphValidation, message)
                .with_location(SourceLocation::new(file))
                .with_hint("select an existing source profile with matching selectors and unchanged contract inputs"),
        ]),
        other => other,
    }
}

pub fn reverify_native_owner_projection(
    projection: Option<&aros_transpiler::native_owner_projection::NativeOwnerProjection>,
    source: &Path,
) -> Result<()> {
    if let Some(projection) = projection {
        projection.verify(source).map_err(|message| {
            native_selection_input_error(ArosError::Configuration {
                file: "native MetaMake projection".into(),
                message,
            })
        })?;
    }
    Ok(())
}

pub fn append_unscoped_diagnostics(
    errors: &mut Vec<Diagnostic>,
    unscoped: &mut std::collections::BTreeSet<Diagnostic>,
    added: Vec<Diagnostic>,
) {
    unscoped.extend(added.iter().cloned());
    errors.extend(added);
}

pub fn load_native_selection(
    args: &Args,
    context: Option<&TargetContext>,
) -> Result<Option<aros_common::native_build_contract::LoadedNativeBuildContract>> {
    let Some(name) = &args.native_profile else {
        return Ok(None);
    };
    let invalid = |message: &str| ArosError::Configuration {
        file: "native profile selection".into(),
        message: message.into(),
    };
    let profiles =
        aros_common::TargetProfile::load_from_file(&args.source_dir.join("aros-targets.toml"))?;
    let profile = profiles
        .iter()
        .find(|profile| &profile.name == name)
        .ok_or_else(|| invalid("native profile is not declared by the selected source"))?;
    let relative = profile
        .native_build_contract
        .as_ref()
        .ok_or_else(|| invalid("selected source profile has no native build contract"))?;
    let loaded = aros_common::native_build_contract::load_bound_native_build_contract(
        &args.source_dir,
        Path::new(relative),
        profile,
    )?;
    if args
        .native_contract_sha256
        .as_ref()
        .is_some_and(|digest| digest != loaded.sha256.as_str())
    {
        return Err(invalid(
            "native build contract digest differs from the caller binding",
        ));
    }
    let context = context.ok_or_else(|| invalid("native selection requires explicit selectors"))?;
    let selectors = profile
        .transpiler
        .as_ref()
        .ok_or_else(|| invalid("native source profile requires explicit transpiler selectors"))?;
    if context.cpu.as_deref() != Some(loaded.contract.abi.source_cpu.as_str())
        || context.platform.as_deref() != Some(profile.platform.as_str())
        || context.family.as_deref() != Some(selectors.family.as_str())
        || context.variant.as_deref() != Some(selectors.variant.as_str())
        || context.toolchain.as_deref() != Some(selectors.toolchain.as_str())
        || context.cpu32.as_deref() != Some(selectors.cpu32.as_str())
        || context.use_mmu.as_deref() != Some(if selectors.use_mmu { "1" } else { "0" })
        || context.float_abi != profile.float_abi
        || selectors
            .mesa_version
            .as_ref()
            .is_some_and(|version| context.mesa_version.as_ref() != Some(version))
    {
        return Err(invalid(
            "native profile and supplied MetaMake selectors disagree",
        ));
    }
    Ok(Some(loaded))
}
