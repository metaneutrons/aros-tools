//! Bind source-local header prerequisites without creating a global `setup`.

use super::DependencyGraph;
use crate::ast::MetaTargetRule;
use crate::copy_includes::{AdhocHeaderRule, HeaderTransformDecl};
use crate::directory_setup::DirectorySetupDecl;
use crate::source_header_pipeline::{SourceHeaderPipelineDecl, SourceHeaderPipelineStep};
use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage, SourceLocation};
use std::collections::{BTreeMap, BTreeSet};

/// Stable internal endpoint shared by graph binding and provenance indexing.
pub fn private_source_directory_owner(file: &str, prerequisite: &str) -> String {
    format!(
        "aros-source-dirs-{}",
        aros_common::sha256_bytes(format!("{file}\n{prerequisite}").as_bytes())
    )
}

/// Normalise only the root aliases emitted by the typed producers below.
/// This is deliberately not a Make/CMake path evaluator.
fn canonical_path(path: &str) -> String {
    path.replace("$(AROS_INCLUDES)/", "${AROS_SDK_INCLUDE_DIR}/")
        .replace(
            "${CMAKE_BINARY_DIR}/SDK/include/",
            "${AROS_SDK_INCLUDE_DIR}/",
        )
        .replace("$(GENINCDIR)/", "${AROS_GENINC_DIR}/")
        .replace("${CMAKE_BINARY_DIR}/GENINCDIR/", "${AROS_GENINC_DIR}/")
        .replace("$(GENDIR)/", "${AROS_BUILD_DIR}/gen/")
        .replace("${CMAKE_BINARY_DIR}/gen/", "${AROS_BUILD_DIR}/gen/")
}

/// Conservative collision key, not proof that two paths denote the same file.
fn output_key(path: &str) -> String {
    canonical_path(path).to_ascii_lowercase()
}

/// Concrete typed owners share a generated target namespace. Ordinary
/// `meta_targets` and `make_meta_providers` are intentionally excluded: they
/// are source dependency aliases, not independent file producers.
fn concrete_owner_conflict(graph: &DependencyGraph, owner: &str) -> bool {
    graph.targets.contains_key(owner)
        || graph
            .source_archives
            .iter()
            .any(|archive| archive.declaration.owner == owner || archive.provider_target() == owner)
        || graph
            .directory_setups
            .iter()
            .any(|item| item.owner == owner)
        || graph
            .host_header_aggregates
            .iter()
            .any(|item| item.owner == owner)
        || graph
            .genmodule_header_rules
            .iter()
            .any(|item| item.owner == owner)
        || graph
            .genmodule_writefiles_rules
            .iter()
            .any(|item| item.owner == owner)
        || graph
            .host_file_generators
            .iter()
            .any(|item| item.owner == owner)
        || graph
            .host_header_rules
            .iter()
            .any(|item| item.owner == owner || item.setup_owner == owner)
        || graph.sdk_text_rules.iter().any(|item| item.owner == owner)
        || graph
            .sfd_header_rules
            .iter()
            .any(|item| item.owner == owner)
        || graph
            .source_text_rules
            .iter()
            .any(|item| item.owner == owner)
        || graph
            .source_value_rules
            .iter()
            .any(|item| item.owner == owner)
        || graph.sdk_file_copies.iter().any(|item| item.owner == owner)
        || graph.sdk_asset_rules.iter().any(|item| item.owner == owner)
        || graph
            .sdk_program_outputs
            .iter()
            .any(|item| item.owner == owner)
        || graph
            .sdk_object_groups
            .iter()
            .any(|item| item.owner == owner)
        || graph
            .literal_object_groups
            .iter()
            .any(|item| item.owner == owner)
        || graph.bison_outputs.iter().any(|item| item.owner == owner)
        || graph.flexcat_headers.iter().any(|item| item.owner == owner)
        || graph.flexcat_sources.iter().any(|item| item.owner == owner)
        || graph.ilbm_sources.iter().any(|item| item.owner == owner)
        || graph.define_headers.iter().any(|item| item.owner == owner)
        || graph.copy_directories.iter().any(|item| item.name == owner)
        || graph.fetches.iter().any(|item| item.name == owner)
        || graph.script_outputs.iter().any(|item| item.owner == owner)
        || graph.python_outputs.iter().any(|item| item.owner == owner)
        || graph.icon_targets.contains_key(owner)
}

/// Return exact file outputs exposed by already-typed producer families.
/// Pattern-based declarations contribute only literal filenames; this helper
/// never expands globs or predicts source-tree contents.
#[derive(Debug, Clone)]
enum OutputClaimSource {
    Typed,
    Adhoc {
        index: usize,
        file: String,
        line: usize,
    },
}

#[derive(Debug, Clone)]
struct ConcreteOutputClaim {
    output: String,
    source: OutputClaimSource,
}

fn concrete_output_claims(graph: &DependencyGraph) -> Vec<ConcreteOutputClaim> {
    let mut claims = Vec::new();
    {
        let mut add = |_owner: &str, output: String| {
            claims.push(ConcreteOutputClaim {
                output,
                source: OutputClaimSource::Typed,
            });
        };

        for archive in &graph.source_archives {
            add(
                &archive.declaration.owner,
                archive.declaration.output.clone(),
            );
        }

        for declaration in &graph.host_header_aggregates {
            for header in &declaration.headers {
                add(
                    &declaration.owner,
                    format!("${{AROS_SDK_INCLUDE_DIR}}/{}", header.header),
                );
                if header.generated_mirror {
                    add(
                        &declaration.owner,
                        format!("${{AROS_GENINC_DIR}}/{}", header.header),
                    );
                }
            }
        }
        for declaration in &graph.sdk_file_copies {
            for file in &declaration.files {
                add(
                    &declaration.owner,
                    format!("{}/{}", declaration.destination.trim_end_matches('/'), file),
                );
            }
        }
        for declaration in &graph.sfd_header_rules {
            for job in &declaration.jobs {
                add(
                    &declaration.owner,
                    format!("${{AROS_SDK_INCLUDE_DIR}}/{}", job.sdk_output),
                );
            }
        }
        for declaration in &graph.host_header_rules {
            add(&declaration.owner, declaration.primary_output.clone());
            add(&declaration.owner, declaration.sdk_output.clone());
        }
        for declaration in &graph.sdk_text_rules {
            add(&declaration.owner, declaration.output.clone());
        }
        for declaration in &graph.source_text_rules {
            for output in &declaration.outputs {
                add(&declaration.owner, output.output.clone());
            }
        }
        for declaration in &graph.source_value_rules {
            add(&declaration.owner, declaration.output.clone());
        }
        for declaration in &graph.sdk_asset_rules {
            for operation in &declaration.operations {
                match &operation.operation {
                    crate::sdk_asset_rules::SdkAssetOperation::Copy { output, .. }
                    | crate::sdk_asset_rules::SdkAssetOperation::WriteText { output, .. } => {
                        add(&declaration.owner, output.clone());
                    }
                }
            }
        }
        for declaration in &graph.sdk_program_outputs {
            add(&declaration.owner, declaration.output.clone());
        }
        for declaration in &graph.genmodule_header_rules {
            for output in &declaration.outputs {
                let root = match output.destination {
                    crate::genmodule_header_rules::GenmoduleHeaderDestination::Private => {
                        format!(
                            "${{AROS_BUILD_DIR}}/gen/{}/include",
                            declaration.declaring_dir
                        )
                    }
                    crate::genmodule_header_rules::GenmoduleHeaderDestination::Geninc => {
                        "${AROS_GENINC_DIR}".to_owned()
                    }
                    crate::genmodule_header_rules::GenmoduleHeaderDestination::Sdk => {
                        "${AROS_SDK_INCLUDE_DIR}".to_owned()
                    }
                };
                add(
                    &declaration.owner,
                    format!("{root}/{}", output.relative_path),
                );
            }
        }
        for declaration in &graph.host_file_generators {
            add(
                &declaration.owner,
                format!("${{AROS_BUILD_DIR}}/{}", declaration.output),
            );
        }
        for declaration in &graph.host_generated_headers {
            add(
                &format!("aros-host-header-{}", declaration.tool),
                format!("${{AROS_SDK_INCLUDE_DIR}}/{}", declaration.header),
            );
            add(
                &format!("aros-host-header-{}", declaration.tool),
                format!("${{AROS_GENINC_DIR}}/{}", declaration.header),
            );
        }
        for declaration in &graph.script_outputs {
            for output in &declaration.outputs {
                add(&declaration.owner, output.clone());
            }
        }
        for declaration in &graph.python_outputs {
            for job in &declaration.jobs {
                add(
                    &declaration.owner,
                    format!(
                        "{}/{}",
                        declaration.build_root.trim_end_matches('/'),
                        job.output
                    ),
                );
            }
        }
        for declaration in &graph.bison_outputs {
            add(&declaration.owner, declaration.output.clone());
        }
        for declaration in &graph.flexcat_headers {
            add(
                &declaration.owner,
                format!(
                    "${{AROS_BUILD_DIR}}/gen/{}/{}",
                    declaration.declaring_dir, declaration.header
                ),
            );
        }
        for declaration in &graph.flexcat_sources {
            add(
                &declaration.owner,
                format!(
                    "${{AROS_BUILD_DIR}}/gen/{}/{}",
                    declaration.declaring_dir, declaration.header
                ),
            );
        }
        for declaration in &graph.define_headers {
            add(&declaration.owner, declaration.output.clone());
        }
        for declaration in &graph.source_layered_headers {
            for copy in &declaration.copies {
                add(&declaration.owner, copy.output_alias.clone());
            }
        }
        for declaration in &graph.copy_includes {
            for pattern in &declaration.patterns {
                // A literal source filename has one exact staged SDK path. Do not
                // interpret wildcards, Make expressions, or pattern syntax here.
                if pattern.is_empty() || pattern.contains(['*', '?', '%', '$', '[', ']']) {
                    continue;
                }
                let relative = if declaration.flatten {
                    std::path::Path::new(pattern)
                        .file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .unwrap_or(pattern)
                } else {
                    pattern.as_str()
                };
                let destination = declaration.dest.trim_matches('/');
                let output = if destination.is_empty() {
                    format!("${{AROS_SDK_INCLUDE_DIR}}/{relative}")
                } else {
                    format!("${{AROS_SDK_INCLUDE_DIR}}/{destination}/{relative}")
                };
                add(&declaration.name, output);
            }
        }
    }
    for (index, declaration) in graph.adhoc_header_rules.iter().enumerate() {
        let root = match declaration.root.as_str() {
            "$(AROS_INCLUDES)/" | "${AROS_SDK_INCLUDE_DIR}/" => "${AROS_SDK_INCLUDE_DIR}/",
            "$(GENINCDIR)/" | "${AROS_GENINC_DIR}/" | "${CMAKE_BINARY_DIR}/GENINCDIR/" => {
                "${AROS_GENINC_DIR}/"
            }
            "$(GENDIR)/" | "${CMAKE_BINARY_DIR}/gen/" => "${AROS_BUILD_DIR}/gen/",
            _ => continue,
        };
        claims.push(ConcreteOutputClaim {
            output: format!("{root}{}", declaration.dest),
            source: OutputClaimSource::Adhoc {
                index,
                file: declaration.file.clone(),
                line: declaration.line,
            },
        });
    }
    claims
}

/// Preserve source spelling and case: this is an identity check, not a
/// collision key. The parser supplies source-root-relative paths, so aliases
/// or parent traversal are not accepted as the same physical source file.
fn source_path_identity(path: &str) -> Option<String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return None;
    }
    Some(path.to_owned())
}

/// Return the exact literal generated-include output a legacy Adhoc rule
/// claims. This intentionally does not evaluate Make expressions or accept
/// pattern destinations.
fn literal_adhoc_generated_output(rule: &AdhocHeaderRule) -> Option<String> {
    let root = match rule.root.as_str() {
        "$(GENINCDIR)/" | "${AROS_GENINC_DIR}/" | "${CMAKE_BINARY_DIR}/GENINCDIR/" => {
            "${AROS_GENINC_DIR}/"
        }
        _ => return None,
    };
    literal_include_output(root, &rule.dest)
}

fn literal_include_output(root: &str, destination: &str) -> Option<String> {
    if destination.is_empty()
        || destination.trim() != destination
        || destination.contains(['$', '*', '?', '%', '[', ']', ';', '\\'])
        || destination.chars().any(char::is_whitespace)
        || destination
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return None;
    }
    Some(format!("{root}{destination}"))
}

/// Reconcile an SDK Adhoc projection only with the already proved copy in
/// this pipeline. The projection itself is never evidence of a producer.
fn exact_adhoc_sdk_claim_index(
    pipeline: &SourceHeaderPipelineDecl,
    claims: &[ConcreteOutputClaim],
    graph: &DependencyGraph,
) -> Option<usize> {
    let location = pipeline.sdk_rule_location.as_ref()?;
    let physical_line = location.line.filter(|line| *line > 0)?;
    let source_file = source_path_identity(&pipeline.file)?;
    if source_path_identity(&location.path).as_deref() != Some(source_file.as_str()) {
        return None;
    }
    let sdk_output = canonical_path(&pipeline.sdk_output);
    let generated_output = canonical_path(&pipeline.generated_output);
    let matching = claims
        .iter()
        .filter_map(|claim| {
            let OutputClaimSource::Adhoc { index, file, line } = &claim.source else {
                return None;
            };
            let rule = graph.adhoc_header_rules.get(*index)?;
            if !matches!(
                rule.root.as_str(),
                "$(AROS_INCLUDES)/" | "${AROS_SDK_INCLUDE_DIR}/"
            ) {
                return None;
            }
            let exact_output = literal_include_output("${AROS_SDK_INCLUDE_DIR}/", &rule.dest)?;
            let mut prerequisites = rule.prereqs.split_whitespace();
            let input = prerequisites.next()?;
            (prerequisites.next().is_none()
                && canonical_path(input) == generated_output
                && source_path_identity(file).as_deref() == Some(source_file.as_str())
                && *line == physical_line
                && canonical_path(&claim.output) == sdk_output
                && exact_output == sdk_output)
                .then_some(*index)
        })
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [index] => Some(*index),
        _ => None,
    }
}

/// The generated-rule diagnostic location is reconstructed from physical
/// source anchors by the parser. It is the only current provenance field that
/// can be compared with `AdhocHeaderRule.line` (which is already physical).
fn exact_adhoc_claim_index(
    pipeline: &SourceHeaderPipelineDecl,
    claims: &[ConcreteOutputClaim],
    graph: &DependencyGraph,
) -> Option<usize> {
    let location = pipeline.diagnostic_location.as_ref()?;
    let physical_line = location.line?;
    if physical_line == 0 {
        return None;
    }
    let source_file = source_path_identity(&pipeline.file)?;
    if source_path_identity(&location.path).as_deref() != Some(source_file.as_str()) {
        return None;
    }
    let generated_output = canonical_path(&pipeline.generated_output);
    let matching = claims
        .iter()
        .filter_map(|claim| {
            let OutputClaimSource::Adhoc { index, file, line } = &claim.source else {
                return None;
            };
            let rule = graph.adhoc_header_rules.get(*index)?;
            let exact_output = literal_adhoc_generated_output(rule)?;
            (source_path_identity(file).as_deref() == Some(source_file.as_str())
                && *line == physical_line
                && canonical_path(&claim.output) == generated_output
                && exact_output == generated_output)
                .then_some(*index)
        })
        .collect::<Vec<_>>();
    if matching.len() == 1 {
        Some(matching[0])
    } else {
        None
    }
}

/// Match one legacy SDK mirror only when its file, owner, exact endpoints,
/// and mapped physical rule line all identify the same source rule. Multiple
/// identical records remain ambiguous and are not reconciled.
fn exact_legacy_mirror_indices(
    pipelines: &[SourceHeaderPipelineDecl],
    transforms: &[HeaderTransformDecl],
) -> Option<BTreeSet<usize>> {
    let mut matching_indices = BTreeSet::new();
    for pipeline in pipelines {
        let location = pipeline.sdk_rule_location.as_ref()?;
        let physical_line = location.line?;
        if physical_line == 0 {
            return None;
        }
        let source_file = source_path_identity(&pipeline.file)?;
        if source_path_identity(&location.path).as_deref() != Some(source_file.as_str()) {
            return None;
        }
        let matches = transforms
            .iter()
            .enumerate()
            .filter(|(_, prior)| {
                prior.copy_only
                    && prior.name == pipeline.owner
                    && source_path_identity(&prior.file).as_deref() == Some(source_file.as_str())
                    && prior.line == physical_line
                    && canonical_path(&prior.input) == canonical_path(&pipeline.generated_output)
                    && canonical_path(&prior.output) == canonical_path(&pipeline.sdk_output)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => {}
            [index] => {
                matching_indices.insert(*index);
            }
            _ => return None,
        }
    }
    Some(matching_indices)
}

fn pipeline_declared_output_keys(pipelines: &[SourceHeaderPipelineDecl]) -> BTreeSet<String> {
    pipelines
        .iter()
        .flat_map(|pipeline| [&pipeline.generated_output, &pipeline.sdk_output])
        .map(|output| output_key(output))
        .collect()
}

fn copy(
    owner: &str,
    file: &str,
    line: usize,
    input: String,
    output: String,
    generated_owner: Option<String>,
) -> HeaderTransformDecl {
    HeaderTransformDecl {
        name: owner.into(),
        file: file.into(),
        line,
        input,
        output,
        match_text: String::new(),
        replacement: String::new(),
        copy_only: true,
        replace_whole_line_containing: false,
        substitutions: Vec::new(),
        dependencies: Vec::new(),
        consumers: Vec::new(),
        generated_input_owner: generated_owner,
    }
}

/// Lower the source's in-place operations to distinct private intermediates.
/// The public generated header is emitted only after the last replacement.
fn lower_pipeline(pipeline: &SourceHeaderPipelineDecl) -> Result<Vec<HeaderTransformDecl>, String> {
    let Some(SourceHeaderPipelineStep::Copy { input, output, .. }) = pipeline.steps.first() else {
        return Err("header pipeline must begin with the exact source copy".into());
    };
    if input != &pipeline.source_prerequisite || output != &pipeline.generated_output {
        return Err("header pipeline source copy identity differs from its declaration".into());
    }
    let Some(SourceHeaderPipelineStep::Copy { input, output, .. }) = pipeline.steps.last() else {
        return Err("header pipeline must end with the exact SDK mirror".into());
    };
    if pipeline.steps.len() < 2
        || input != &pipeline.generated_output
        || output != &pipeline.sdk_output
    {
        return Err("header pipeline SDK mirror identity differs from its declaration".into());
    }
    let identity = aros_common::sha256_bytes(
        format!(
            "{}\n{}\n{}",
            pipeline.file, pipeline.owner, pipeline.generated_output
        )
        .as_bytes(),
    );
    let last_generated_step = pipeline.steps.len() - 2;
    let mut prior_output = pipeline.source_prerequisite.clone();
    let mut transforms = Vec::new();
    for (index, step) in pipeline.steps.iter().enumerate() {
        let step_output = if index == pipeline.steps.len() - 1 {
            pipeline.sdk_output.clone()
        } else if index == last_generated_step {
            pipeline.generated_output.clone()
        } else {
            format!("${{AROS_BUILD_DIR}}/gen/source-headers/{identity}/{index}.h")
        };
        let generated_owner = (index != 0).then(|| pipeline.owner.clone());
        let mut transform = match step {
            SourceHeaderPipelineStep::Copy { line, .. }
                if index == 0 || index == pipeline.steps.len() - 1 =>
            {
                copy(
                    &pipeline.owner,
                    &pipeline.file,
                    *line,
                    prior_output.clone(),
                    step_output.clone(),
                    generated_owner,
                )
            }
            SourceHeaderPipelineStep::ReplaceWholeLineInPlace {
                path,
                token,
                replacement,
                line,
            } if index != 0
                && index < pipeline.steps.len() - 1
                && path == &pipeline.generated_output
                && !token.is_empty() =>
            {
                let mut transform = copy(
                    &pipeline.owner,
                    &pipeline.file,
                    *line,
                    prior_output.clone(),
                    step_output.clone(),
                    generated_owner,
                );
                transform.copy_only = false;
                transform.replace_whole_line_containing = true;
                transform.match_text.clone_from(token);
                transform.replacement.clone_from(replacement);
                transform
            }
            _ => {
                return Err(
                    "header pipeline contains an unsupported or misplaced operation".into(),
                );
            }
        };
        // The generator sorts transforms by source line. A declaration whose
        // ordering would be changed by that sort is not admissible.
        if transforms
            .last()
            .is_some_and(|prior: &HeaderTransformDecl| prior.line > transform.line)
        {
            return Err("header pipeline operation lines are not in source order".into());
        }
        transform.consumers.clear();
        transforms.push(transform);
        prior_output = step_output;
    }
    Ok(transforms)
}

fn refusal(owner: &str, file: &str, location: Option<&SourceLocation>, reason: &str) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::CapabilityDrift,
        DiagnosticStage::CapabilityValidation,
        format!("Source header closure is unproven: {reason}"),
    )
    .with_location(
        location
            .cloned()
            .unwrap_or_else(|| SourceLocation::new(file)),
    )
    .with_context(DiagnosticContext {
        target: Some(owner.into()),
        ..Default::default()
    })
}

impl DependencyGraph {
    /// Bind each complete source-owned header group transactionally.
    ///
    /// # Panics
    /// Panics if an internally staged group loses all its pipeline declarations.
    pub fn bind_source_headers(&mut self) -> Vec<Diagnostic> {
        #[derive(Debug)]
        struct StagedPipelineGroup {
            identity: (String, String),
            pipelines: Vec<SourceHeaderPipelineDecl>,
            transforms: Vec<HeaderTransformDecl>,
            adhoc_claim_indices: BTreeSet<usize>,
            legacy_mirror_indices: BTreeSet<usize>,
            failure: Option<String>,
        }

        let mut diagnostics = Vec::new();
        let mut bound_pipelines = BTreeSet::new();

        let mut grouped = BTreeMap::<(String, String), Vec<SourceHeaderPipelineDecl>>::new();
        for pipeline in self.source_header_pipelines.clone() {
            grouped
                .entry((pipeline.file.clone(), pipeline.owner.clone()))
                .or_default()
                .push(pipeline);
        }
        let existing_claims = concrete_output_claims(self);
        let mut staged = Vec::with_capacity(grouped.len());
        for (identity, pipelines) in grouped {
            let owner = &identity.1;
            let mut failure = None;
            let sdk_outputs = pipelines
                .iter()
                .map(|pipeline| canonical_path(&pipeline.sdk_output))
                .collect::<BTreeSet<_>>();
            if sdk_outputs.len() != pipelines.len() {
                failure = Some("pipeline group declares a duplicate SDK output".to_owned());
            } else if pipelines.iter().any(|pipeline| {
                pipeline
                    .owner_prerequisites
                    .iter()
                    .any(|input| !sdk_outputs.contains(&canonical_path(input)))
            }) {
                failure = Some(
                    "pipeline owner has a prerequisite without an exact header producer".into(),
                );
            }

            let mut transforms = Vec::new();
            if failure.is_none() {
                for pipeline in &pipelines {
                    if let Ok(lowered) = lower_pipeline(pipeline) {
                        transforms.extend(lowered);
                    } else {
                        failure = Some("ordered pipeline operations are inconsistent".into());
                        break;
                    }
                }
            }
            let transform_outputs = transforms
                .iter()
                .map(|transform| output_key(&transform.output))
                .collect::<Vec<_>>();
            let unique_outputs = transform_outputs.iter().collect::<BTreeSet<_>>();
            if failure.is_none() && unique_outputs.len() != transform_outputs.len() {
                failure = Some("pipeline group has duplicate output paths".into());
            }

            // The legacy scan may have retained the generated rule as an
            // Adhoc claim and the final mirror as Adhoc or a copy transform. Reconcile
            // only unique claims with independently mapped physical lines.
            let adhoc_claim_indices = pipelines
                .iter()
                .flat_map(|pipeline| {
                    [
                        exact_adhoc_claim_index(pipeline, &existing_claims, self),
                        exact_adhoc_sdk_claim_index(pipeline, &existing_claims, self),
                    ]
                    .into_iter()
                    .flatten()
                })
                .collect::<BTreeSet<_>>();
            let legacy_mirror_indices =
                exact_legacy_mirror_indices(&pipelines, &self.header_transforms)
                    .unwrap_or_default();
            if failure.is_none() && concrete_owner_conflict(self, owner) {
                failure = Some("pipeline owner conflicts with another concrete producer".into());
            }
            if failure.is_none()
                && (transform_outputs.iter().any(|output| {
                    existing_claims.iter().any(|claim| {
                        output_key(&claim.output) == *output
                            && !matches!(
                                &claim.source,
                                OutputClaimSource::Adhoc { index, .. }
                                    if adhoc_claim_indices.contains(index)
                            )
                    })
                }) || self
                    .header_transforms
                    .iter()
                    .enumerate()
                    .any(|(index, prior)| {
                        !legacy_mirror_indices.contains(&index)
                            && transform_outputs
                                .iter()
                                .any(|output| output_key(&prior.output) == *output)
                    }))
            {
                failure = Some("pipeline output conflicts with another concrete producer".into());
            }
            staged.push(StagedPipelineGroup {
                identity,
                pipelines,
                transforms,
                adhoc_claim_indices,
                legacy_mirror_indices,
                failure,
            });
        }

        // Cross-family comparisons use the declared, exact public paths even
        // when another member of the group failed lowering. Neither ambiguous
        // owner nor output claims are resolved by iteration order.
        for left in 0..staged.len() {
            for right in left + 1..staged.len() {
                let left_group = &staged[left];
                let right_group = &staged[right];
                let mut left_outputs = pipeline_declared_output_keys(&left_group.pipelines);
                left_outputs.extend(
                    left_group
                        .transforms
                        .iter()
                        .map(|transform| output_key(&transform.output)),
                );
                let mut right_outputs = pipeline_declared_output_keys(&right_group.pipelines);
                right_outputs.extend(
                    right_group
                        .transforms
                        .iter()
                        .map(|transform| output_key(&transform.output)),
                );
                if left_outputs
                    .iter()
                    .any(|output| right_outputs.contains(output))
                {
                    if staged[left].failure.is_none() {
                        staged[left].failure =
                            Some("pipeline output is claimed by another source pipeline".into());
                    }
                    if staged[right].failure.is_none() {
                        staged[right].failure =
                            Some("pipeline output is claimed by another source pipeline".into());
                    }
                }
            }
        }

        // Pending layered copies are producer claims too. Refuse every
        // participant before committing, rather than let collection order
        // choose one of two conflicting producers.
        let mut aggregate_output_claims = BTreeMap::<String, BTreeSet<usize>>::new();
        for (index, aggregate) in self.layered_header_projections.iter().enumerate() {
            for header in &aggregate.copies {
                aggregate_output_claims
                    .entry(output_key(&header.output_alias))
                    .or_default()
                    .insert(index);
            }
        }
        let mut conflicting_aggregates = BTreeSet::<usize>::new();
        for claimants in aggregate_output_claims.values() {
            if claimants.len() > 1 {
                conflicting_aggregates.extend(claimants.iter().copied());
            }
        }
        for group in &mut staged {
            let mut outputs = pipeline_declared_output_keys(&group.pipelines);
            outputs.extend(group.transforms.iter().map(|item| output_key(&item.output)));
            for output in outputs {
                if let Some(claimants) = aggregate_output_claims.get(&output) {
                    conflicting_aggregates.extend(claimants.iter().copied());
                    if group.failure.is_none() {
                        group.failure =
                            Some("pipeline output is also claimed by a layered aggregate".into());
                    }
                }
            }
        }

        let mut committed_adhoc_claim_indices = BTreeSet::new();
        let mut committed_legacy_mirror_indices = BTreeSet::new();
        for group in staged {
            if let Some(reason) = group.failure {
                let pipeline = group
                    .pipelines
                    .first()
                    .expect("staged pipeline groups are nonempty");
                diagnostics.push(refusal(
                    &pipeline.owner,
                    &pipeline.file,
                    pipeline.diagnostic_location.as_ref(),
                    &reason,
                ));
                continue;
            }
            let (file, owner) = &group.identity;
            committed_adhoc_claim_indices.extend(group.adhoc_claim_indices);
            committed_legacy_mirror_indices.extend(group.legacy_mirror_indices);
            self.header_transforms.extend(group.transforms);
            bound_pipelines.insert((file.clone(), owner.clone()));
        }

        // Remove reconciled legacy records only after the whole source-owner
        // group passed every cross-family and sibling check. Indices are still
        // relative to the original vectors here because removals are batched.
        if !committed_legacy_mirror_indices.is_empty() {
            let mut index = 0usize;
            self.header_transforms.retain(|_| {
                let keep = !committed_legacy_mirror_indices.contains(&index);
                index += 1;
                keep
            });
        }
        if !committed_adhoc_claim_indices.is_empty() {
            let mut index = 0usize;
            self.adhoc_header_rules.retain(|_| {
                let keep = !committed_adhoc_claim_indices.contains(&index);
                index += 1;
                keep
            });
        }

        for (index, aggregate) in self
            .layered_header_projections
            .clone()
            .into_iter()
            .enumerate()
        {
            let mut directories = Vec::new();
            let mut dependencies = Vec::new();
            let mut staged_directory_owners = BTreeSet::new();
            let mut reason = conflicting_aggregates
                .contains(&index)
                .then(|| "aggregate output is claimed by another pending producer".to_owned());
            for prerequisite in &aggregate.unresolved_prerequisites {
                if let Some(group) = self
                    .source_directory_groups
                    .get(&(aggregate.file.clone(), prerequisite.clone()))
                {
                    let owner = private_source_directory_owner(&aggregate.file, prerequisite);
                    if staged_directory_owners.insert(owner.clone()) {
                        directories.push(DirectorySetupDecl {
                            owner: owner.clone(),
                            directories: group.directories.clone(),
                        });
                    }
                    if !dependencies.contains(&owner) {
                        dependencies.push(owner);
                    }
                } else if bound_pipelines.contains(&(aggregate.file.clone(), prerequisite.clone()))
                {
                    if !dependencies.contains(prerequisite) {
                        dependencies.push(prerequisite.clone());
                    }
                } else {
                    reason = Some(format!(
                        "local prerequisite {prerequisite} has no complete producer"
                    ));
                    break;
                }
            }
            let outputs = aggregate
                .copies
                .iter()
                .map(|copy| output_key(&copy.output_alias))
                .collect::<BTreeSet<_>>();
            let producer_claims = concrete_output_claims(self);
            let pipeline_claims = self
                .source_header_pipelines
                .iter()
                .flat_map(|pipeline| [&pipeline.generated_output, &pipeline.sdk_output])
                .map(|output| output_key(output))
                .collect::<BTreeSet<_>>();
            if reason.is_none()
                && (concrete_owner_conflict(self, &aggregate.owner)
                    || outputs.len() != aggregate.copies.len()
                    || outputs.iter().any(|output| {
                        producer_claims
                            .iter()
                            .any(|claim| output_key(&claim.output) == *output)
                            || self
                                .header_transforms
                                .iter()
                                .any(|prior| output_key(&prior.output) == *output)
                            || pipeline_claims.contains(output)
                    })
                    || directories.iter().any(|directory| {
                        directory.owner == aggregate.owner
                            || concrete_owner_conflict(self, &directory.owner)
                            || self
                                .directory_setups
                                .iter()
                                .any(|prior| prior.owner == directory.owner)
                    }))
            {
                reason = Some(
                    "aggregate outputs or directory owner conflict with another producer".into(),
                );
            }
            if let Some(reason) = reason {
                diagnostics.push(refusal(&aggregate.owner, &aggregate.file, None, &reason));
                continue;
            }
            for header in &aggregate.copies {
                self.header_transforms.push(copy(
                    &aggregate.owner,
                    &aggregate.file,
                    aggregate.line,
                    format!("${{AROS_SOURCE_DIR}}/{}", header.source_relative),
                    header.output_alias.clone(),
                    None,
                ));
            }
            self.directory_setups.extend(directories);
            self.add_meta_rule(MetaTargetRule {
                name: aggregate.owner.clone(),
                dependencies,
            });
            self.source_layered_headers.push(aggregate);
        }
        diagnostics
    }

    pub fn discharge_bound_header_provider_failures(&self, failures: &mut Vec<Diagnostic>) {
        failures.retain(|diagnostic| !(self.source_layered_headers.iter().any(|aggregate| {
            diagnostic.context.as_ref().and_then(|context| context.target.as_ref()) == Some(&aggregate.owner)
                && diagnostic.location.as_ref().is_some_and(|location| location.path == aggregate.file)
                && diagnostic.message == format!("nonvirtual Make provider {} has no concrete producer in its declaring mmakefile", aggregate.owner)
        }) || self.source_header_pipelines.iter().any(|pipeline| {
            diagnostic.context.as_ref().and_then(|context| context.target.as_ref()) == Some(&pipeline.owner)
                && diagnostic.location.as_ref().is_some_and(|location| location.path == pipeline.file)
                && diagnostic.message == format!("nonvirtual Make provider {} has no concrete producer in its declaring mmakefile", pipeline.owner)
                && [pipeline.generated_output.as_str(), pipeline.sdk_output.as_str()].iter().all(|output|
                    self.header_transforms.iter().any(|transform| (&transform.file, &transform.name) == (&pipeline.file, &pipeline.owner) && transform.output == *output))
        })));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipeline(replacements: usize) -> SourceHeaderPipelineDecl {
        let source = "${AROS_SOURCE_DIR}/compiler/include/exec/base.inc";
        let generated = "${AROS_GENINC_DIR}/exec/base.h";
        let sdk = "${AROS_SDK_INCLUDE_DIR}/exec/base.h";
        let mut steps = vec![SourceHeaderPipelineStep::Copy {
            input: source.into(),
            output: generated.into(),
            line: 11,
        }];
        for index in 0..replacements {
            steps.push(SourceHeaderPipelineStep::ReplaceWholeLineInPlace {
                path: generated.into(),
                token: format!("FIELD{index}"),
                replacement: format!("replacement{index}"),
                line: index + 12,
            });
        }
        steps.push(SourceHeaderPipelineStep::Copy {
            input: generated.into(),
            output: sdk.into(),
            line: 30,
        });
        SourceHeaderPipelineDecl {
            owner: "includes-base".into(),
            file: "compiler/include/mmakefile.src".into(),
            owner_line: 35,
            line: 10,
            sdk_rule_line: 30,
            diagnostic_location: Some(SourceLocation {
                path: "compiler/include/mmakefile.src".into(),
                line: Some(11),
                column: None,
            }),
            sdk_rule_location: Some(SourceLocation {
                path: "compiler/include/mmakefile.src".into(),
                line: Some(30),
                column: None,
            }),
            source_prerequisite: source.into(),
            owner_prerequisites: vec![sdk.into()],
            generated_output: generated.into(),
            sdk_output: sdk.into(),
            steps,
        }
    }

    fn renamed_pipeline(
        mut pipeline: SourceHeaderPipelineDecl,
        stem: &str,
        file: &str,
    ) -> SourceHeaderPipelineDecl {
        let file = file.to_owned();
        pipeline.file.clone_from(&file);
        if let Some(location) = &mut pipeline.diagnostic_location {
            location.path.clone_from(&file);
        }
        if let Some(location) = &mut pipeline.sdk_rule_location {
            location.path.clone_from(&file);
        }
        pipeline.source_prerequisite =
            format!("${{AROS_SOURCE_DIR}}/compiler/include/exec/{stem}.inc");
        pipeline.generated_output = format!("${{AROS_GENINC_DIR}}/exec/{stem}.h");
        pipeline.sdk_output = format!("${{AROS_SDK_INCLUDE_DIR}}/exec/{stem}.h");
        pipeline.owner_prerequisites = vec![pipeline.sdk_output.clone()];
        pipeline.steps[0] = SourceHeaderPipelineStep::Copy {
            input: pipeline.source_prerequisite.clone(),
            output: pipeline.generated_output.clone(),
            line: 11,
        };
        for step in &mut pipeline.steps {
            if let SourceHeaderPipelineStep::ReplaceWholeLineInPlace { path, .. } = step {
                *path = pipeline.generated_output.clone();
            }
        }
        *pipeline.steps.last_mut().unwrap() = SourceHeaderPipelineStep::Copy {
            input: pipeline.generated_output.clone(),
            output: pipeline.sdk_output.clone(),
            line: 30,
        };
        pipeline
    }

    fn adhoc_generated_rule(
        pipeline: &SourceHeaderPipelineDecl,
        line: usize,
        destination: &str,
    ) -> AdhocHeaderRule {
        AdhocHeaderRule {
            file: pipeline.file.clone(),
            line,
            root: "$(GENINCDIR)/".into(),
            dest: destination.into(),
            prereqs: "$(SRCDIR)/$(CURDIR)/exec/base.inc".into(),
        }
    }

    fn adhoc_sdk_rule(pipeline: &SourceHeaderPipelineDecl) -> AdhocHeaderRule {
        AdhocHeaderRule {
            file: pipeline.file.clone(),
            line: 30,
            root: "$(AROS_INCLUDES)/".into(),
            dest: "exec/base.h".into(),
            prereqs: "$(GENINCDIR)/exec/base.h".into(),
        }
    }

    #[test]
    fn exact_generated_and_sdk_adhoc_projections_are_consumed_together() {
        let declaration = pipeline(0);
        let mut graph = DependencyGraph::default();
        graph.adhoc_header_rules.extend([
            adhoc_generated_rule(&declaration, 11, "exec/base.h"),
            adhoc_sdk_rule(&declaration),
        ]);
        graph.source_header_pipelines.push(declaration);
        assert!(graph.bind_source_headers().is_empty());
        assert!(graph.adhoc_header_rules.is_empty());
        assert_eq!(graph.header_transforms.len(), 2);
    }

    #[test]
    fn sdk_adhoc_drift_never_consumes_the_generated_claim() {
        for kind in 0..11 {
            let mut declaration = pipeline(0);
            let mut sdk = adhoc_sdk_rule(&declaration);
            match kind {
                0 => sdk.line += 1,
                1 => sdk.file = "other/mmakefile.src".into(),
                2 => sdk.dest = "exec/Base.h".into(),
                3 => sdk.prereqs = "$(GENINCDIR)/exec/other.h".into(),
                4 => sdk.prereqs.push_str(" extra"),
                5 => declaration.sdk_rule_location = None,
                6 => declaration.sdk_rule_location.as_mut().unwrap().line = Some(0),
                7 => {
                    declaration.sdk_rule_location.as_mut().unwrap().path =
                        "other/mmakefile.src".into();
                }
                8 => declaration
                    .owner_prerequisites
                    .push("unproved-input".into()),
                9 => sdk.prereqs = "$(GENINCDIR)/exec/Base.h".into(),
                10 => sdk.file = "compiler\\include/mmakefile.src".into(),
                _ => unreachable!(),
            }
            let mut graph = DependencyGraph::default();
            graph
                .adhoc_header_rules
                .extend([adhoc_generated_rule(&declaration, 11, "exec/base.h"), sdk]);
            graph.source_header_pipelines.push(declaration);
            assert_eq!(graph.bind_source_headers().len(), 1, "drift case {kind}");
            assert_eq!(graph.adhoc_header_rules.len(), 2, "drift case {kind}");
            assert!(graph.header_transforms.is_empty());
        }
    }

    #[test]
    fn duplicate_sdk_adhoc_projections_remain_ambiguous() {
        let declaration = pipeline(0);
        let mut graph = DependencyGraph::default();
        graph.adhoc_header_rules.extend([
            adhoc_generated_rule(&declaration, 11, "exec/base.h"),
            adhoc_sdk_rule(&declaration),
            adhoc_sdk_rule(&declaration),
        ]);
        graph.source_header_pipelines.push(declaration);
        assert_eq!(graph.bind_source_headers().len(), 1);
        assert_eq!(graph.adhoc_header_rules.len(), 3);
        assert!(graph.header_transforms.is_empty());
    }

    #[test]
    fn refusal_never_labels_expanded_snapshot_lines_as_physical_source_lines() {
        for location in [
            None,
            Some(SourceLocation {
                path: "compiler/include/mmakefile.src".into(),
                line: Some(7),
                column: None,
            }),
        ] {
            let mut declaration = pipeline(0);
            declaration.diagnostic_location = location.clone();
            declaration
                .owner_prerequisites
                .push("unproved-input".into());
            let mut graph = DependencyGraph::default();
            graph.source_header_pipelines.push(declaration);
            let diagnostics = graph.bind_source_headers();
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(
                diagnostics[0].location.as_ref().unwrap().line,
                location.as_ref().and_then(|location| location.line),
            );
        }
    }

    #[test]
    fn ordered_replacements_have_distinct_inputs_and_final_public_output() {
        let pipeline = pipeline(5);
        let lowered = lower_pipeline(&pipeline).unwrap();
        assert_eq!(lowered.len(), 7);
        for (index, step) in lowered.iter().enumerate() {
            assert_ne!(step.input, step.output);
            assert_eq!(step.generated_input_owner.is_some(), index != 0);
            if index != 0 {
                assert_eq!(step.input, lowered[index - 1].output);
            }
        }
        assert_eq!(lowered[5].output, pipeline.generated_output);
        assert_eq!(lowered[6].output, pipeline.sdk_output);
        assert_eq!(lowered[1].match_text, "FIELD0");
        assert_eq!(lowered[5].match_text, "FIELD4");
    }

    #[test]
    fn non_smp_copy_uses_no_private_intermediate() {
        let pipeline = pipeline(0);
        let lowered = lower_pipeline(&pipeline).unwrap();
        assert_eq!(lowered.len(), 2);
        assert_eq!(lowered[0].output, pipeline.generated_output);
        assert!(lowered.iter().all(|step| step.copy_only));
    }

    #[test]
    fn extra_owner_prerequisite_cannot_disappear() {
        let mut pipeline = pipeline(0);
        pipeline
            .owner_prerequisites
            .push("another-operation".into());
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(pipeline);
        assert_eq!(graph.bind_source_headers().len(), 1);
        assert!(graph.header_transforms.is_empty());
    }

    #[test]
    fn pipeline_group_rejects_cross_family_output_and_owner_collisions() {
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(pipeline(0));
        graph
            .host_header_aggregates
            .push(crate::host_header_aggregates::HostHeaderAggregateDecl {
                owner: "other-header-owner".into(),
                tool: "make-header".into(),
                tool_source: "tools/make-header.c".into(),
                host_compile_flags: Vec::new(),
                headers: vec![crate::host_header_aggregates::HostHeaderAggregateOutput {
                    header: "exec/base.h".into(),
                    arguments: Vec::new(),
                    generated_mirror: true,
                }],
            });

        let diagnostics = graph.bind_source_headers();
        assert_eq!(diagnostics.len(), 1);
        assert!(graph.header_transforms.is_empty());

        let mut graph = DependencyGraph::default();
        let mut conflicting_owner = pipeline(0);
        conflicting_owner.sdk_output = "${AROS_SDK_INCLUDE_DIR}/exec/other.h".into();
        conflicting_owner.generated_output = "${AROS_GENINC_DIR}/exec/other.h".into();
        conflicting_owner.steps[0] = SourceHeaderPipelineStep::Copy {
            input: conflicting_owner.source_prerequisite.clone(),
            output: conflicting_owner.generated_output.clone(),
            line: 11,
        };
        conflicting_owner.owner_prerequisites = vec![conflicting_owner.sdk_output.clone()];
        *conflicting_owner.steps.last_mut().unwrap() = SourceHeaderPipelineStep::Copy {
            input: conflicting_owner.generated_output.clone(),
            output: conflicting_owner.sdk_output.clone(),
            line: 30,
        };
        graph.source_header_pipelines.push(conflicting_owner);
        graph
            .host_header_aggregates
            .push(crate::host_header_aggregates::HostHeaderAggregateDecl {
                owner: "includes-base".into(),
                tool: "make-header".into(),
                tool_source: "tools/make-header.c".into(),
                host_compile_flags: Vec::new(),
                headers: vec![crate::host_header_aggregates::HostHeaderAggregateOutput {
                    header: "exec/unrelated.h".into(),
                    arguments: Vec::new(),
                    generated_mirror: false,
                }],
            });

        let diagnostics = graph.bind_source_headers();
        assert_eq!(diagnostics.len(), 1);
        assert!(graph.header_transforms.is_empty());
    }

    #[test]
    fn exact_legacy_mirror_accepts_proven_include_role_aliases() {
        let declaration = pipeline(0);
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration.clone());
        graph.header_transforms.push(copy(
            &declaration.owner,
            &declaration.file,
            30,
            "${CMAKE_BINARY_DIR}/GENINCDIR/exec/base.h".into(),
            declaration.sdk_output.clone(),
            None,
        ));

        assert!(graph.bind_source_headers().is_empty());
        assert_eq!(graph.header_transforms.len(), 2);
        assert!(graph.header_transforms.iter().any(|transform| {
            transform.input == declaration.generated_output
                && transform.output == declaration.sdk_output
        }));
        assert!(graph
            .header_transforms
            .iter()
            .all(|transform| { !transform.input.contains("${CMAKE_BINARY_DIR}/GENINCDIR") }));
    }

    #[test]
    fn exact_physical_adhoc_generated_rule_is_reconciled_after_commit() {
        let declaration = pipeline(0);
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration.clone());
        graph
            .adhoc_header_rules
            .push(adhoc_generated_rule(&declaration, 11, "exec/base.h"));
        graph.header_transforms.push(copy(
            &declaration.owner,
            &declaration.file,
            30,
            declaration.generated_output.clone(),
            declaration.sdk_output.clone(),
            None,
        ));

        assert!(graph.bind_source_headers().is_empty());
        assert!(graph.adhoc_header_rules.is_empty());
        assert_eq!(graph.header_transforms.len(), 2);
        assert!(graph.header_transforms.iter().any(|transform| {
            transform.input == declaration.source_prerequisite
                && transform.output == declaration.generated_output
        }));
    }

    #[test]
    fn same_file_and_output_at_another_physical_line_still_conflicts() {
        let declaration = pipeline(0);
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration.clone());
        graph
            .adhoc_header_rules
            .push(adhoc_generated_rule(&declaration, 12, "exec/base.h"));
        let mirror = copy(
            &declaration.owner,
            &declaration.file,
            30,
            declaration.generated_output.clone(),
            declaration.sdk_output.clone(),
            None,
        );
        graph.header_transforms.push(mirror);

        assert_eq!(graph.bind_source_headers().len(), 1);
        assert_eq!(graph.adhoc_header_rules.len(), 1);
        assert_eq!(graph.header_transforms.len(), 1);
        assert_eq!(
            graph.header_transforms[0].input,
            declaration.generated_output
        );
    }

    #[test]
    fn same_physical_line_with_different_output_is_not_removed() {
        let declaration = pipeline(0);
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration.clone());
        graph
            .adhoc_header_rules
            .push(adhoc_generated_rule(&declaration, 11, "exec/other.h"));
        graph.header_transforms.push(copy(
            &declaration.owner,
            &declaration.file,
            30,
            declaration.generated_output.clone(),
            declaration.sdk_output.clone(),
            None,
        ));

        assert!(graph.bind_source_headers().is_empty());
        assert_eq!(graph.adhoc_header_rules.len(), 1);
        assert_eq!(graph.adhoc_header_rules[0].dest, "exec/other.h");
        assert_eq!(graph.header_transforms.len(), 2);
    }

    #[test]
    fn unavailable_physical_anchor_cannot_reconcile_adhoc_claim() {
        let mut declaration = pipeline(0);
        declaration.diagnostic_location = None;
        let mut graph = DependencyGraph::default();
        graph
            .adhoc_header_rules
            .push(adhoc_generated_rule(&declaration, 11, "exec/base.h"));
        graph.source_header_pipelines.push(declaration.clone());
        graph.header_transforms.push(copy(
            &declaration.owner,
            &declaration.file,
            30,
            declaration.generated_output.clone(),
            declaration.sdk_output.clone(),
            None,
        ));

        assert_eq!(graph.bind_source_headers().len(), 1);
        assert_eq!(graph.adhoc_header_rules.len(), 1);
        assert_eq!(graph.header_transforms.len(), 1);
    }

    #[test]
    fn zero_physical_line_is_not_source_provenance() {
        let mut declaration = pipeline(0);
        declaration.diagnostic_location.as_mut().unwrap().line = Some(0);
        let mut graph = DependencyGraph::default();
        graph
            .adhoc_header_rules
            .push(adhoc_generated_rule(&declaration, 0, "exec/base.h"));
        graph.source_header_pipelines.push(declaration.clone());
        assert_eq!(graph.bind_source_headers().len(), 1);
        assert_eq!(graph.adhoc_header_rules.len(), 1);
        assert!(graph.header_transforms.is_empty());

        let mut declaration = pipeline(0);
        declaration.sdk_rule_location.as_mut().unwrap().line = Some(0);
        let mut graph = DependencyGraph::default();
        graph.header_transforms.push(copy(
            &declaration.owner,
            &declaration.file,
            0,
            declaration.generated_output.clone(),
            declaration.sdk_output.clone(),
            None,
        ));
        graph.source_header_pipelines.push(declaration);
        assert_eq!(graph.bind_source_headers().len(), 1);
        assert_eq!(graph.header_transforms.len(), 1);
    }

    #[test]
    fn duplicate_exact_adhoc_claims_remain_ambiguous() {
        let declaration = pipeline(0);
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration.clone());
        graph.adhoc_header_rules.extend([
            adhoc_generated_rule(&declaration, 11, "exec/base.h"),
            adhoc_generated_rule(&declaration, 11, "exec/base.h"),
        ]);
        graph.header_transforms.push(copy(
            &declaration.owner,
            &declaration.file,
            30,
            declaration.generated_output.clone(),
            declaration.sdk_output.clone(),
            None,
        ));

        assert_eq!(graph.bind_source_headers().len(), 1);
        assert_eq!(graph.adhoc_header_rules.len(), 2);
        assert_eq!(graph.header_transforms.len(), 1);
    }

    #[test]
    fn unrelated_adhoc_claim_for_same_output_still_conflicts() {
        let declaration = pipeline(0);
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration.clone());
        graph.adhoc_header_rules.extend([
            adhoc_generated_rule(&declaration, 11, "exec/base.h"),
            adhoc_generated_rule(&declaration, 44, "exec/base.h"),
        ]);
        graph.header_transforms.push(copy(
            &declaration.owner,
            &declaration.file,
            30,
            declaration.generated_output.clone(),
            declaration.sdk_output.clone(),
            None,
        ));

        assert_eq!(graph.bind_source_headers().len(), 1);
        assert_eq!(graph.adhoc_header_rules.len(), 2);
        assert_eq!(graph.header_transforms.len(), 1);
    }

    #[test]
    fn malformed_pipeline_does_not_remove_same_rule_legacy_claims() {
        let mut declaration = pipeline(0);
        declaration
            .owner_prerequisites
            .push("unproved-input".into());
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration.clone());
        graph
            .adhoc_header_rules
            .push(adhoc_generated_rule(&declaration, 11, "exec/base.h"));
        graph.header_transforms.push(copy(
            &declaration.owner,
            &declaration.file,
            30,
            declaration.generated_output.clone(),
            declaration.sdk_output.clone(),
            None,
        ));

        assert_eq!(graph.bind_source_headers().len(), 1);
        assert_eq!(graph.adhoc_header_rules.len(), 1);
        assert_eq!(graph.header_transforms.len(), 1);
    }

    #[test]
    fn other_typed_output_claim_keeps_adhoc_and_mirror_on_pipeline_refusal() {
        let declaration = pipeline(0);
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration.clone());
        graph
            .adhoc_header_rules
            .push(adhoc_generated_rule(&declaration, 11, "exec/base.h"));
        graph.header_transforms.push(copy(
            &declaration.owner,
            &declaration.file,
            30,
            declaration.generated_output.clone(),
            declaration.sdk_output.clone(),
            None,
        ));
        graph
            .host_header_aggregates
            .push(crate::host_header_aggregates::HostHeaderAggregateDecl {
                owner: "unrelated-typed-owner".into(),
                tool: "make-header".into(),
                tool_source: "tools/make-header.c".into(),
                host_compile_flags: Vec::new(),
                headers: vec![crate::host_header_aggregates::HostHeaderAggregateOutput {
                    header: "exec/base.h".into(),
                    arguments: Vec::new(),
                    generated_mirror: true,
                }],
            });

        assert_eq!(graph.bind_source_headers().len(), 1);
        assert_eq!(graph.adhoc_header_rules.len(), 1);
        assert_eq!(graph.header_transforms.len(), 1);
    }

    #[test]
    fn duplicate_legacy_mirrors_are_not_reconciled() {
        let declaration = pipeline(0);
        let mirror = || {
            copy(
                &declaration.owner,
                &declaration.file,
                30,
                declaration.generated_output.clone(),
                declaration.sdk_output.clone(),
                None,
            )
        };
        let mirrors = [mirror(), mirror()];
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration);
        graph.header_transforms.extend(mirrors);

        assert_eq!(graph.bind_source_headers().len(), 1);
        assert_eq!(graph.header_transforms.len(), 2);
    }

    #[test]
    fn case_different_mirror_is_not_removed_as_an_exact_projection() {
        let declaration = pipeline(0);
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration.clone());
        let prior = copy(
            &declaration.owner,
            &declaration.file,
            30,
            declaration.generated_output.replace("base.h", "Base.h"),
            declaration.sdk_output.clone(),
            None,
        );
        graph.header_transforms.push(prior.clone());

        assert_eq!(graph.bind_source_headers().len(), 1);
        assert_eq!(graph.header_transforms.len(), 1);
        assert_eq!(graph.header_transforms[0].input, prior.input);
    }

    #[test]
    fn same_file_owner_and_endpoints_at_another_mirror_line_still_conflict() {
        let declaration = pipeline(0);
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration.clone());
        let prior = copy(
            &declaration.owner,
            &declaration.file,
            31,
            declaration.generated_output.clone(),
            declaration.sdk_output.clone(),
            None,
        );
        graph.header_transforms.push(prior.clone());

        assert_eq!(graph.bind_source_headers().len(), 1);
        assert_eq!(graph.header_transforms.len(), 1);
        assert_eq!(graph.header_transforms[0].line, prior.line);
    }

    #[test]
    fn case_different_owner_prerequisite_has_no_exact_producer() {
        let mut declaration = pipeline(0);
        declaration.owner_prerequisites = vec![declaration.sdk_output.replace("base.h", "Base.h")];
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration);

        assert_eq!(graph.bind_source_headers().len(), 1);
        assert!(graph.header_transforms.is_empty());
    }

    #[test]
    fn failed_sibling_pipeline_preserves_prior_mirror_and_commits_no_group_steps() {
        let mut first = pipeline(0);
        let mut second = renamed_pipeline(pipeline(1), "task", &first.file);
        first.owner_prerequisites = vec![first.sdk_output.clone(), second.sdk_output.clone()];
        second.owner_prerequisites = first.owner_prerequisites.clone();
        let mut graph = DependencyGraph {
            source_header_pipelines: vec![first.clone(), second.clone()],
            ..DependencyGraph::default()
        };
        graph
            .adhoc_header_rules
            .push(adhoc_generated_rule(&first, 11, "exec/base.h"));
        let legacy_mirror = copy(
            &first.owner,
            &first.file,
            30,
            first.generated_output.clone(),
            first.sdk_output.clone(),
            None,
        );
        graph.header_transforms.push(legacy_mirror);
        graph.header_transforms.push(copy(
            "unrelated-transform",
            "other/mmakefile.src",
            2,
            "input.h".into(),
            second.sdk_output,
            None,
        ));

        let diagnostics = graph.bind_source_headers();
        assert_eq!(diagnostics.len(), 1);
        assert!(graph.header_transforms.iter().any(|transform| {
            (&transform.file, &transform.name) == (&first.file, &first.owner)
                && transform.input == first.generated_output
                && transform.output == first.sdk_output
        }));
        assert_eq!(graph.adhoc_header_rules.len(), 1);
        assert_eq!(graph.header_transforms.len(), 2);
        assert!(graph
            .header_transforms
            .iter()
            .all(|transform| transform.name != first.owner || transform.copy_only));
    }

    #[test]
    fn disjoint_files_can_extend_a_shared_copy_owner() {
        let mut first = renamed_pipeline(pipeline(0), "first", "one/mmakefile.src");
        let mut second = renamed_pipeline(pipeline(0), "second", "two/mmakefile.src");
        first.owner = "includes-copy".into();
        second.owner = "includes-copy".into();
        let mut graph = DependencyGraph {
            source_header_pipelines: vec![first.clone(), second.clone()],
            ..DependencyGraph::default()
        };
        graph.header_transforms.push(copy(
            "includes-copy",
            "legacy/mmakefile.src",
            1,
            "legacy-input.h".into(),
            "${AROS_SDK_INCLUDE_DIR}/legacy.h".into(),
            None,
        ));

        assert!(graph.bind_source_headers().is_empty());
        assert_eq!(graph.header_transforms.len(), 5);
        assert!(graph.header_transforms.iter().any(|transform| {
            transform.name == "includes-copy"
                && transform.output == first.sdk_output
                && transform.file == first.file
        }));
        assert!(graph.header_transforms.iter().any(|transform| {
            transform.name == "includes-copy"
                && transform.output == second.sdk_output
                && transform.file == second.file
        }));
    }

    #[test]
    fn pending_aggregate_output_conflicts_refuse_every_claimant_in_either_order() {
        use crate::layered_header_copies::{LayeredHeaderCopy, LayeredHeaderCopyDecl};
        let declaration = |owner: &str| LayeredHeaderCopyDecl {
            owner: owner.into(),
            file: format!("{owner}/mmakefile.src"),
            line: 3,
            copies: vec![LayeredHeaderCopy {
                source_relative: format!("{owner}/example.h"),
                output_alias: "${AROS_SDK_INCLUDE_DIR}/example.h".into(),
            }],
            inputs: vec![format!("{owner}/example.h")],
            unresolved_prerequisites: vec![],
        };
        for owners in [["owner-a", "owner-b"], ["owner-b", "owner-a"]] {
            let mut graph = DependencyGraph {
                layered_header_projections: owners.into_iter().map(declaration).collect(),
                ..DependencyGraph::default()
            };
            assert_eq!(graph.bind_source_headers().len(), 2);
            assert!(graph.source_layered_headers.is_empty());
            assert!(graph.header_transforms.is_empty());
            assert!(graph.meta_targets.is_empty());
            assert!(graph.directory_setups.is_empty());
        }
    }

    #[test]
    fn pending_layered_copy_cannot_lose_to_pipeline_commit_order() {
        use crate::layered_header_copies::{LayeredHeaderCopy, LayeredHeaderCopyDecl};
        let declaration = pipeline(0);
        let mut graph = DependencyGraph::default();
        graph.source_header_pipelines.push(declaration.clone());
        graph
            .layered_header_projections
            .push(LayeredHeaderCopyDecl {
                owner: "competing-copy".into(),
                file: "other/mmakefile.src".into(),
                line: 3,
                copies: vec![LayeredHeaderCopy {
                    source_relative: "other/base.h".into(),
                    output_alias: declaration.sdk_output,
                }],
                inputs: vec!["other/base.h".into()],
                unresolved_prerequisites: vec![],
            });
        assert_eq!(graph.bind_source_headers().len(), 2);
        assert!(graph.source_layered_headers.is_empty());
        assert!(graph.header_transforms.is_empty());
        assert!(graph.meta_targets.is_empty());
    }

    #[test]
    fn rejected_layered_aggregate_does_not_publish_staged_directories() {
        let file = "compiler/include/mmakefile.src".to_owned();
        let aggregate = crate::layered_header_copies::LayeredHeaderCopyDecl {
            owner: "compiler-includes".into(),
            file: file.clone(),
            line: 20,
            copies: vec![crate::layered_header_copies::LayeredHeaderCopy {
                source_relative: "compiler/include/example.h".into(),
                output_alias: "${AROS_SDK_INCLUDE_DIR}/example.h".into(),
            }],
            inputs: vec!["example.h".into()],
            unresolved_prerequisites: vec!["setup".into(), "unknown-local".into()],
        };
        let mut graph = DependencyGraph::default();
        graph.layered_header_projections.push(aggregate);
        graph.source_directory_groups.insert(
            (file, "setup".into()),
            crate::source_directory_rules::SourceDirectoryGroupDecl {
                owner: "setup".into(),
                source_line: 5,
                directory_rule_line: 6,
                directories: vec!["${AROS_GENINC_DIR}/headers".into()],
            },
        );

        let diagnostics = graph.bind_source_headers();
        assert_eq!(diagnostics.len(), 1);
        assert!(graph.directory_setups.is_empty());
        assert!(graph.source_layered_headers.is_empty());
        assert!(graph.header_transforms.is_empty());
        assert!(!graph.meta_targets.contains_key("compiler-includes"));
    }
}
