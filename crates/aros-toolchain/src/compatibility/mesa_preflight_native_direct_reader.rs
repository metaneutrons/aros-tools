//! Read original direct children under one independently bound root batch.

use super::{
    failure, CancellationToken, CollectedMesaNativeDirect, ContractError,
    MesaNativeCollectionSelection, MesaNativeDirectOrigin, MesaPreflightRequest,
};
use aros_common::make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot};
use aros_transpiler::{
    dirs::DirVars,
    make_vars::MakeProgramPaths,
    native_owner_projection::{
        MatchedChildSource, MatchedChildSourceFold, RetainedChildSourceBatchLimits,
        RetainedChildSourceBatchRequest, RetainedChildSourceFoldLimits,
    },
};
use std::path::Path;

impl CollectedMesaNativeDirect {
    /// Project an original missing include using this owner's original outputs.
    ///
    /// `selection` indexes the original observed child and its active include
    /// event, not a replacement goal or a newly supplied recipe. Generated
    /// configuration programs are bound to per-file digests in the retained
    /// original collection inventory; Source program admission still comes
    /// from the independent Source policy. The callback is data-only and must
    /// not run recipes or publish its result before the enclosing postchecks.
    /// Failure consumes this owner. No producer completion, directory creation,
    /// include import or Mesa26 shell authority is granted by this projection.
    ///
    /// # Errors
    /// Refuses absent original coordinates, unbound/changed inputs, invalid
    /// include data, callback failure, cancellation or the original deadline.
    pub fn with_original_include_reader<T>(
        self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        selection: (usize, usize),
        limits: RetainedChildSourceBatchLimits,
        consume: impl FnOnce(
            &aros_transpiler::native_owner_projection::DeclaredIncludedRecipes,
            &mut aros_common::make_input_snapshot::MakeInputReader<'_>,
        ) -> Result<T, String>,
    ) -> Result<(Self, T), ContractError> {
        let (request, origin, selected, cancellation) = control;
        let value = self.with_source_child_batch(
            request,
            origin,
            selected,
            cancellation,
            |snapshot, children, check_control| {
                let child = children
                    .get(selection.0)
                    .ok_or("original include child coordinate is absent")?;
                self.prepared
                    .policy
                    .projection
                    .with_retained_child_include_reader(
                        &self.prepared.roots,
                        (
                            child,
                            selection.1,
                            Some((request.output_root, &self.output_tree)),
                        ),
                        snapshot,
                        limits,
                        check_control,
                        consume,
                    )
            },
        )?;
        Ok((self, value))
    }

    /// Interpret every original observed child's Source inputs, without recipes.
    ///
    /// Root selection comes from the original Source identity, post-collection
    /// build CAS and independent compiler inventory. Each child retains its own
    /// literal cwd, argv/env and read manifest; the complete vector stays private
    /// until root and original-owner postchecks pass. This does not prove native
    /// leaf execution, generated-file provenance, ABI projection or SDK success.
    ///
    /// # Errors
    /// Refuses original-owner drift, unsupported Source, changed coordinates,
    /// input/resource limits, cancellation or the original total deadline.
    pub fn read_children(
        &self,
        request: &MesaPreflightRequest<'_>,
        origin: &MesaNativeDirectOrigin<'_, '_>,
        selected: &MesaNativeCollectionSelection<'_>,
        limits: RetainedChildSourceBatchLimits,
        cancellation: &CancellationToken,
    ) -> Result<Vec<MatchedChildSource>, ContractError> {
        self.read_children_at_output(
            (request, origin, selected, cancellation),
            limits,
            &self.output_tree,
        )
    }

    // Only a separately checked owning transition supplies a later tree.
    pub(super) fn read_children_at_output(
        &self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        limits: RetainedChildSourceBatchLimits,
        output: &aros_common::TreeContentCas,
    ) -> Result<Vec<MatchedChildSource>, ContractError> {
        let (request, origin, selected, cancellation) = control;
        let children = self.with_source_child_batch_at_output(
            request,
            origin,
            selected,
            cancellation,
            output,
            |snapshot, requests, control| {
                self.prepared.policy.projection.read_retained_child_sources(
                    &self.prepared.roots,
                    requests,
                    snapshot,
                    limits,
                    control,
                )
            },
        )?;
        if children.len() != self.coordinates.len()
            || children
                .iter()
                .zip(&self.coordinates)
                .any(|(child, original)| {
                    child.coordinates().generated_owner() != original.generated_owner()
                        || child.coordinates().source_input() != original.source_input()
                        || child.coordinates().goals() != original.goals()
                })
        {
            return Err(failure(
                "direct Source interpreted child coordinates changed",
            ));
        }
        Ok(children)
    }

    /// Project required Source owners under the original complete call roster.
    ///
    /// The callback must stage and accurately account its owned result, never
    /// publish it early. Requirements are exact `(Source input, observed goal)`
    /// pairs, independently selected by the consumer. All captured children must
    /// still match complete existing-owner coordinate coverage. Unresolved
    /// dependency names stay in the separate Source metadata, not as producers.
    /// Only selected programs are interpreted; siblings do not supply their state.
    /// A child containing several required goals is interpreted once, in original
    /// captured order. Callback indices address original call slots.
    /// `fold` supplies the requirements, initial state and staging callback.
    ///
    /// Transient programs are consumed sequentially. The complete fold and
    /// original owner are revalidated before returning them together. Any failure
    /// consumes this owner and drops staged results, rather than leaving a failed
    /// owner available for another interpretation attempt. Generated-rule
    /// provenance, recipe execution and SDK success remain open.
    ///
    /// # Errors
    /// Refuses incomplete call coverage, missing/ambiguous requirements,
    /// owner/input drift, changed coordinates, working/projected limits,
    /// callback refusal, cancellation or the original total deadline.
    pub fn fold_children<T, F>(
        self,
        request: &MesaPreflightRequest<'_>,
        origin: &MesaNativeDirectOrigin<'_, '_>,
        selected: &MesaNativeCollectionSelection<'_>,
        limits: RetainedChildSourceFoldLimits,
        cancellation: &CancellationToken,
        fold: (&[(String, String)], T, F),
    ) -> Result<(Self, MatchedChildSourceFold<T>), ContractError>
    where
        F: FnMut(&mut T, usize, MatchedChildSource) -> Result<usize, String>,
    {
        let (required, state, mut project) = fold;
        if required.is_empty() {
            return Err(failure("direct Source fold has no required children"));
        }
        let folded = self.with_source_child_batch(
            request,
            origin,
            selected,
            cancellation,
            |snapshot, requests, control| {
                let children = requests
                    .iter()
                    .map(|child| (child.generated_owner, child.observation.invocation()))
                    .collect::<Vec<_>>();
                let selected_calls = self
                    .prepared
                    .policy
                    .projection
                    .select_required_child_coordinates(
                        request.source_root,
                        &self.prepared.roots,
                        self.prepared.invocation.working_directory(),
                        &children,
                        required,
                    )?;
                // A grouped child may cover several required goals. Retain
                // original call order, not caller requirement ordering, and
                // never replay that child's read-time effects twice.
                let slots = selected_calls
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>();
                let slots = slots.into_iter().collect::<Vec<_>>();
                let selected_requests = slots
                    .iter()
                    .map(|slot| {
                        let child = &requests[*slot];
                        RetainedChildSourceBatchRequest {
                            paths: child.paths,
                            generated_owner: child.generated_owner,
                            observation: child.observation,
                        }
                    })
                    .collect::<Vec<_>>();
                let folded = self
                    .prepared
                    .policy
                    .projection
                    .fold_retained_child_sources(
                        &self.prepared.roots,
                        &selected_requests,
                        snapshot,
                        limits,
                        control,
                        (state, |state, index, child| {
                            let slot =
                                *slots.get(index).ok_or("direct Source fold slot changed")?;
                            let original = self
                                .coordinates
                                .get(slot)
                                .ok_or("direct Source fold index changed")?;
                            if child.coordinates().generated_owner() != original.generated_owner()
                                || child.coordinates().source_input() != original.source_input()
                                || child.coordinates().goals() != original.goals()
                            {
                                return Err("direct Source folded child coordinates changed".into());
                            }
                            project(state, slot, child)
                        }),
                    )
                    .map_err(|detail| original_child_refusal(&detail, &slots))?;
                if folded.children() != slots.len() {
                    return Err("direct Source folded child count changed".into());
                }
                Ok(folded)
            },
        )?;
        Ok((self, folded))
    }

    pub(super) fn with_source_child_batch<T>(
        &self,
        request: &MesaPreflightRequest<'_>,
        origin: &MesaNativeDirectOrigin<'_, '_>,
        selected: &MesaNativeCollectionSelection<'_>,
        cancellation: &CancellationToken,
        operation: impl FnOnce(
            &MakeInputSnapshot,
            &[RetainedChildSourceBatchRequest<'_, '_>],
            &mut dyn FnMut() -> Result<(), String>,
        ) -> Result<T, String>,
    ) -> Result<T, ContractError> {
        self.with_source_child_batch_at_output(
            request,
            origin,
            selected,
            cancellation,
            &self.output_tree,
            operation,
        )
    }

    // Only an owning execution transition may supply another checked output.
    // The original collection tree is retained, never overwritten or rebased.
    pub(super) fn with_source_child_batch_at_output<T>(
        &self,
        request: &MesaPreflightRequest<'_>,
        origin: &MesaNativeDirectOrigin<'_, '_>,
        selected: &MesaNativeCollectionSelection<'_>,
        cancellation: &CancellationToken,
        current_output: &aros_common::TreeContentCas,
        operation: impl FnOnce(
            &MakeInputSnapshot,
            &[RetainedChildSourceBatchRequest<'_, '_>],
            &mut dyn FnMut() -> Result<(), String>,
        ) -> Result<T, String>,
    ) -> Result<T, ContractError> {
        self.revalidate_output(request, origin, selected, current_output, cancellation)?;
        let build = self.prepared.invocation.working_directory();
        let relative = build
            .strip_prefix(request.output_root)
            .ok()
            .and_then(Path::to_str)
            .ok_or_else(|| failure("direct Source build escaped original output root"))?;
        let build_selection = MakeInputRootSelection {
            root: build.to_owned(),
            content_sha256: current_output
                .subtree_payload_digest(relative)
                .ok_or_else(|| failure("direct Source build subtree is not captured"))?,
        };
        let compiler_selection = MakeInputRootSelection {
            root: request.drivers.compiler_root().to_owned(),
            content_sha256: request.drivers.compiler_tree_sha256().clone(),
        };
        let snapshot = request
            .local_source
            .map_or_else(
                || {
                    MakeInputSnapshot::capture(
                        build,
                        &[
                            MakeInputRootSelection {
                                root: request.source_root.to_owned(),
                                content_sha256: request.source_tree_sha256.clone(),
                            },
                            build_selection.clone(),
                            compiler_selection.clone(),
                        ],
                    )
                },
                |local| {
                    MakeInputSnapshot::capture_with_local_source(
                        build,
                        request.source_root,
                        local,
                        &[build_selection.clone(), compiler_selection.clone()],
                    )
                },
            )
            .map_err(|_| failure("cannot bind direct Source child input roots"))?;
        if self.coordinates.len() != self.calls.len() {
            return Err(failure("direct Source child coordinate count changed"));
        }
        let dirs = DirVars::default();
        let mut paths = Vec::with_capacity(self.calls.len());
        for (coordinate, call) in self.coordinates.iter().zip(&self.calls) {
            if call.binding().phase
                != aros_common::make_native_observation::MakeObservationPhase::FirstInputPrelude
            {
                return Err(failure("direct Source child observation phase changed"));
            }
            paths.push(MakeProgramPaths {
                dirs: &dirs,
                source_root: request.source_root,
                relative_dir: Path::new(coordinate.generated_owner())
                    .parent()
                    .ok_or_else(|| failure("direct Source owner has no directory"))?,
                top_directory: build,
                working_directory: call.invocation().working_directory(),
            });
        }
        let requests = paths
            .iter()
            .zip(self.coordinates.iter().zip(&self.calls))
            .map(
                |(paths, (coordinate, observation))| RetainedChildSourceBatchRequest {
                    paths,
                    generated_owner: coordinate.generated_owner(),
                    observation,
                },
            )
            .collect::<Vec<_>>();
        let mut check_control = || {
            if cancellation.is_cancelled() {
                return Err("direct Source child reading cancelled".into());
            }
            self.prepared
                .recorded
                .launch_inputs()
                .host
                .root
                .prepared
                .remaining()
                .map(|_| ())
                .map_err(|_| "direct Source original preparation deadline expired".into())
        };
        let result = operation(&snapshot, &requests, &mut check_control);
        // A parser failure must not skip the original owner's postcheck.
        self.revalidate_output(request, origin, selected, current_output, cancellation)?;
        result.map_err(|detail| {
            let retained = request.output_root.parent().is_some_and(|parent| {
                retain_child_batch_refusal(
                    parent,
                    &self.prepared.selection_sha256,
                    self.coordinates.len(),
                    &detail,
                )
                .is_ok()
            });
            failure(if retained {
                "cannot interpret original direct Source child batch; private diagnostic retained"
            } else {
                "cannot interpret original direct Source child batch; private diagnostic could not be retained"
            })
        })
    }
}

// The generic fold numbers its selected request slice. Retained diagnostics
// must instead identify the original captured call, without exposing values.
fn original_child_refusal(detail: &str, slots: &[usize]) -> String {
    for prefix in [
        "retained child source batch at index ",
        "retained child source fold at index ",
    ] {
        if let Some(indexed) = detail.strip_prefix(prefix) {
            if let Some((index, rest)) = indexed.split_once(':') {
                if !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()) {
                    if let Some(slot) = index
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| slots.get(index))
                    {
                        return format!("{prefix}{slot}:{rest}");
                    }
                }
            }
            // Keep opaque diagnostic content for hashing, but do not let an
            // unrecognized subset index be mislabeled as an original slot.
            return format!("unmapped selected child refusal: {detail}");
        }
    }
    detail.to_owned()
}

// This lives outside the immutable collection/output inventories. It records
// no Source expressions, argv, environment values or raw error text, and never
// turns a failed original owner into a reusable or successful owner.
fn retain_child_batch_refusal(
    parent: &Path,
    selection: &aros_common::Sha256Digest,
    calls: usize,
    detail: &str,
) -> Result<(), ContractError> {
    let index = [
        "retained child source batch at index ",
        "retained child source fold at index ",
    ]
    .into_iter()
    .find_map(|prefix| detail.strip_prefix(prefix))
    .and_then(|text| text.split_once(':'))
    .and_then(|(index, _)| index.parse::<usize>().ok())
    .filter(|index| *index < calls);
    let bytes = crate::canonical::bytes(&serde_json::json!({
        "schema": "aros-native-source-child-refusal-v1",
        "qualification": "failed-original-owner-diagnostic-not-execution-or-sdk",
        "selection": selection,
        "calls": calls,
        "child_index": index,
        "reason_sha256": aros_common::sha256_bytes(detail.as_bytes()),
        "reason_bytes": detail.len(),
    }))
    .map_err(|_| failure("cannot encode private Source refusal diagnostic"))?;
    let directory =
        aros_common::make_native_invocation::PrivateNativeCaptureDirectory::open(parent)
            .map_err(|_| failure("Source refusal diagnostic parent is not private"))?;
    directory
        .write_new(
            std::ffi::OsStr::new(&format!("direct-source-child-refusal-{selection}.json")),
            &bytes,
        )
        .map_err(|_| failure("cannot retain private Source refusal diagnostic"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn selected_fold_refusal_uses_original_call_slot() {
        for kind in ["batch", "fold"] {
            assert_eq!(
                original_child_refusal(
                    &format!("retained child source {kind} at index 1: private detail"),
                    &[2, 5],
                ),
                format!("retained child source {kind} at index 5: private detail"),
            );
        }
        assert_eq!(
            original_child_refusal("root input changed", &[2, 5]),
            "root input changed",
        );
    }

    #[test]
    fn malformed_selected_fold_index_never_claims_an_original_slot() {
        for index in ["", "-1", "+1", "2", "999999999999999999999999999"] {
            let detail = format!("retained child source fold at index {index}: private detail");
            let mapped = original_child_refusal(&detail, &[2, 5]);
            assert!(mapped.starts_with("unmapped selected child refusal:"));
            assert!(!mapped.starts_with("retained child source fold at index "));
        }
    }

    #[test]
    fn private_refusal_retains_only_bounded_index_and_opaque_reason_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let selection = aros_common::sha256_bytes(b"selected owner");
        let detail = "retained child source batch at index 26: private-source-value";
        retain_child_batch_refusal(&root, &selection, 550, detail).unwrap();
        let path = root.join(format!("direct-source-child-refusal-{selection}.json"));
        let bytes = std::fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("private-source-value"));
        let data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(data["child_index"], 26);
        assert_eq!(
            data["reason_sha256"],
            aros_common::sha256_bytes(detail.as_bytes()).to_string()
        );
        assert!(retain_child_batch_refusal(&root, &selection, 550, "replacement").is_err());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn private_refusal_never_adopts_unbounded_or_unstructured_indices() {
        for detail in [
            "retained child source batch at index 550: outside",
            "retained child source fold at index 550: outside",
            "retained child source fold at index -1: invalid",
            "retained child source batch at index -1: invalid",
            "retained child source batch at index 99999999999999999999999999: overflow",
            "not a child-indexed error",
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
            let selection = aros_common::sha256_bytes(b"selected owner");
            retain_child_batch_refusal(&root, &selection, 550, detail).unwrap();
            let bytes =
                std::fs::read(root.join(format!("direct-source-child-refusal-{selection}.json")))
                    .unwrap();
            let data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(data["child_index"].is_null());
        }
    }

    #[test]
    fn private_refusal_recognizes_fold_index_without_retaining_callback_text() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let selection = aros_common::sha256_bytes(b"fold owner");
        retain_child_batch_refusal(
            &root,
            &selection,
            550,
            "retained child source fold at index 136: private callback text",
        )
        .unwrap();
        let bytes =
            std::fs::read(root.join(format!("direct-source-child-refusal-{selection}.json")))
                .unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("private callback text"));
        let data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(data["child_index"], 136);
    }

    #[test]
    fn private_refusal_refuses_a_public_parent_without_writing() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(retain_child_batch_refusal(
            &root,
            &aros_common::sha256_bytes(b"selected owner"),
            1,
            "retained child source batch at index 0: failure",
        )
        .is_err());
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
    }
}
