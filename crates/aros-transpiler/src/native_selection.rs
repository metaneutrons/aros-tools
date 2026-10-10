//! Native profile selection and architecture endpoint binding for the command boundary.

use crate::cli_args::Args;
use crate::error_mapping::diagnostics_error;
use aros_common::{
    ArosError, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage, Result,
    SourceLocation,
};
use aros_transpiler::{DependencyGraph, TargetContext};
use std::collections::BTreeSet;
use std::path::Path;

#[cfg(test)]
#[path = "native_consumer_selection_tests.rs"]
mod native_consumer_selection_tests;

pub enum LoadedNativeSelection {
    Build(Box<aros_common::native_build_contract::LoadedNativeBuildContract>),
    Consumer(Box<aros_common::native_consumer_contract::LoadedNativeConsumerContract>),
}

/// Borrow shared invocation facts without manufacturing boot/package fields.
pub struct NativeInvocationRef<'a> {
    pub profile: &'a str,
    pub source_baseline: &'a str,
    pub inputs: &'a [aros_common::native_build_contract::NativeBuildInput],
    pub host_file_generators: &'a Vec<aros_common::native_host_generator::NativeHostFileGenerator>,
    pub make_include_bindings: &'a std::collections::BTreeMap<String, String>,
    pub generated_make_templates: &'a std::collections::BTreeMap<
        String,
        aros_common::native_make_template::GeneratedMakeTemplateBinding,
    >,
    pub kernel_compiler_role: Option<&'a str>,
    pub metamake_projection: Option<&'a str>,
    pub optional_meta_dependencies:
        &'a [aros_common::native_build_contract::NativeOptionalMetaDependency],
    pub abi: &'a aros_common::native_build_contract::NativeBuildAbi,
}

impl LoadedNativeSelection {
    pub fn invocation(&self) -> NativeInvocationRef<'_> {
        macro_rules! view {
            ($contract:expr, $projection:expr) => {{
                let contract = $contract;
                NativeInvocationRef {
                    profile: &contract.profile,
                    source_baseline: &contract.source_baseline,
                    inputs: &contract.inputs,
                    host_file_generators: &contract.host_file_generators,
                    make_include_bindings: &contract.make_include_bindings,
                    generated_make_templates: &contract.generated_make_templates,
                    kernel_compiler_role: contract.kernel_compiler_role.as_deref(),
                    metamake_projection: $projection,
                    optional_meta_dependencies: &contract.optional_meta_dependencies,
                    abi: &contract.abi,
                }
            }};
        }
        match self {
            Self::Build(loaded) => view!(
                &loaded.contract,
                loaded.contract.metamake_projection.as_deref()
            ),
            Self::Consumer(loaded) => view!(
                &loaded.contract,
                Some(loaded.contract.metamake_projection.as_str())
            ),
        }
    }

    pub const fn sha256(&self) -> &aros_common::Sha256Digest {
        match self {
            Self::Build(loaded) => &loaded.sha256,
            Self::Consumer(loaded) => &loaded.sha256,
        }
    }

    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Build(_) => "build",
            Self::Consumer(_) => "consumer",
        }
    }

    pub fn make_variables_for_host(
        &self,
        host: &str,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        match self {
            Self::Build(loaded) => loaded.contract.make_variables_for_host(host),
            Self::Consumer(loaded) => loaded.contract.make_variables_for_host(host),
        }
    }

    pub const fn build_contract(
        &self,
    ) -> Option<&aros_common::native_build_contract::NativeBuildContract> {
        match self {
            Self::Build(loaded) => Some(&loaded.contract),
            Self::Consumer(_) => None,
        }
    }

    pub fn roots(&self, graph: &DependencyGraph, context: &TargetContext) -> Result<Vec<String>> {
        match self {
            Self::Build(loaded) => graph.native_contract_roots(&loaded.contract, context),
            // The ordinary strict closure and source-owner proof below verify
            // these literal roots. No guessed substitute endpoint is allowed.
            Self::Consumer(loaded) => Ok(loaded.contract.roots.clone()),
        }
    }
}

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
        let verified = projection
            .verify_arch_endpoint_effects(root, &proof)
            .and_then(|()| {
                if let ArchEndpointEffectData::ArchModuleObjects {
                    mainmmake,
                    tag,
                    directory,
                    module_sources,
                } = &effect.data
                {
                    let base = graph.targets.contains_key(mainmmake)
                        || graph
                            .inventory_targets
                            .iter()
                            .any(|target| &target.mmake_name == mainmmake);
                    let expected: std::collections::BTreeSet<_> = module_sources.iter().collect();
                    let paired = graph.arch_sources.get(mainmmake).is_some_and(|sources| {
                        sources.iter().any(|source| {
                            &source.tag == tag
                                && &source.dir == directory
                                && source
                                    .files
                                    .iter()
                                    .collect::<std::collections::BTreeSet<_>>()
                                    == expected
                        })
                    });
                    if !base || !paired {
                        return Err(format!(
                        "architecture objects lack their exact module/source compilation binding \
                         (module={mainmmake}, module_exists={base}, source_group_matches={paired}, \
                         arch={tag}, directory={directory}, source_count={})",
                        module_sources.len()
                    ));
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
) -> Result<Option<LoadedNativeSelection>> {
    let Some(name) = args
        .native_profile
        .as_ref()
        .or(args.native_consumer_profile.as_ref())
    else {
        return Ok(None);
    };
    let invalid = |message: &str| ArosError::Configuration {
        file: "native profile selection".into(),
        message: message.into(),
    };
    let root = args
        .source_dir
        .canonicalize()
        .map_err(|_| invalid("cannot resolve the selected source root"))?;
    let (_, bytes) =
        aros_common::measure_regular_file_bounded(&root.join("aros-targets.toml"), 1024 * 1024)?
            .ok_or_else(|| invalid("source target declarations are absent"))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| invalid("source target declarations are not UTF-8"))?;
    let profiles = aros_common::TargetProfile::parse_config(text, "aros-targets.toml")?.targets;
    let profile = profiles
        .iter()
        .find(|profile| &profile.name == name)
        .ok_or_else(|| invalid("native profile is not declared by the selected source"))?;
    let (loaded, expected) = if args.native_consumer_profile.is_some() {
        let relative = profile
            .native_consumer_contract
            .as_ref()
            .ok_or_else(|| invalid("selected source profile has no native consumer contract"))?;
        (
            LoadedNativeSelection::Consumer(Box::new(
                aros_common::native_consumer_contract::load_bound_native_consumer_contract(
                    &args.source_dir,
                    Path::new(relative),
                    profile,
                )?,
            )),
            &args.native_consumer_contract_sha256,
        )
    } else {
        let relative = profile
            .native_build_contract
            .as_ref()
            .ok_or_else(|| invalid("selected source profile has no native build contract"))?;
        (
            LoadedNativeSelection::Build(Box::new(
                aros_common::native_build_contract::load_bound_native_build_contract(
                    &args.source_dir,
                    Path::new(relative),
                    profile,
                )?,
            )),
            &args.native_contract_sha256,
        )
    };
    if expected
        .as_ref()
        .is_some_and(|digest| digest != loaded.sha256().as_str())
    {
        return Err(invalid(
            "native contract digest differs from the caller binding",
        ));
    }
    let context = context.ok_or_else(|| invalid("native selection requires explicit selectors"))?;
    let selectors = profile
        .transpiler
        .as_ref()
        .ok_or_else(|| invalid("native source profile requires explicit transpiler selectors"))?;
    if context.cpu.as_deref() != Some(loaded.invocation().abi.source_cpu.as_str())
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

/// The source-relative directories of build trees, to be left out of a native
/// MetaMake walk: their generated files are outputs, not recipes. These are the
/// calling engine's own build directory and any sibling of it that holds a
/// CMake cache, which is what a second preset built in the same checkout
/// leaves behind. Empty when no build directory was given or it is not inside
/// the source tree. Never a basename, so a source directory called `build`
/// stays.
pub fn native_build_exclusion(args: &Args) -> Result<BTreeSet<String>> {
    let Some(build_dir) = &args.build_dir else {
        return Ok(BTreeSet::new());
    };
    let configuration_error = |message: String| ArosError::Configuration {
        file: build_dir.display().to_string(),
        message,
    };
    let source = args.source_dir.canonicalize().map_err(|error| {
        configuration_error(format!("cannot resolve the source directory: {error}"))
    })?;
    let build = build_dir.canonicalize().map_err(|error| {
        configuration_error(format!("cannot resolve the build directory: {error}"))
    })?;
    build_tree_exclusions(&source, &build).map_err(configuration_error)
}

fn build_tree_exclusions(
    source: &Path,
    build: &Path,
) -> std::result::Result<BTreeSet<String>, String> {
    let Ok(relative) = build.strip_prefix(source) else {
        return Ok(BTreeSet::new());
    };
    if relative.as_os_str().is_empty() {
        return Err("the build directory must not be the source tree itself".into());
    }
    let relative_text = |path: &Path| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| "the in-tree build directory path is not UTF-8".to_owned())
    };
    let mut excluded = BTreeSet::from([relative_text(relative)?]);
    let Some(parent) = build.parent() else {
        return Ok(excluded);
    };
    let entries = std::fs::read_dir(parent)
        .map_err(|error| format!("cannot list the build directory's parent: {error}"))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("cannot list the build directory's parent: {error}"))?
            .path();
        let configured = path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_dir())
            && path.join("CMakeCache.txt").is_file();
        if configured {
            if let Ok(sibling) = path.strip_prefix(source) {
                excluded.insert(relative_text(sibling)?);
            }
        }
    }
    Ok(excluded)
}

#[cfg(test)]
mod tests {
    use super::build_tree_exclusions;
    use std::collections::BTreeSet;

    #[test]
    fn a_configured_sibling_build_tree_is_left_out_with_the_own_one() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().canonicalize().unwrap();
        for (directory, cache) in [
            ("build/esp32p4-d1001", true),
            ("build/esp32p4-jc1060p470c-v2", true),
            ("build/unconfigured", false),
            ("build/kept-source", false),
        ] {
            std::fs::create_dir_all(source.join(directory)).unwrap();
            if cache {
                std::fs::write(source.join(directory).join("CMakeCache.txt"), "").unwrap();
            }
        }
        let own = source.join("build/esp32p4-jc1060p470c-v2");
        assert_eq!(
            build_tree_exclusions(&source, &own).unwrap(),
            BTreeSet::from([
                "build/esp32p4-d1001".to_owned(),
                "build/esp32p4-jc1060p470c-v2".to_owned(),
            ])
        );
    }

    #[test]
    fn a_build_directory_outside_the_source_excludes_nothing() {
        let source = tempfile::tempdir().unwrap();
        let build = tempfile::tempdir().unwrap();
        assert!(build_tree_exclusions(
            &source.path().canonicalize().unwrap(),
            &build.path().canonicalize().unwrap()
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn the_source_tree_itself_is_not_a_build_directory() {
        let source = tempfile::tempdir().unwrap();
        let root = source.path().canonicalize().unwrap();
        assert!(build_tree_exclusions(&root, &root).is_err());
    }
}
