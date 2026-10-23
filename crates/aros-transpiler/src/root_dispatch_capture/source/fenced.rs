//! Second-pass Source binding for the exact prospective recorder invocation.
use super::{
    sha256_bytes, LiteralMakeStartupArguments, MakeInputSnapshot, MakeProgramPaths,
    MakeRecipeProjection, MakeVariableProgram, RootDispatchCapturePlan,
    RootDispatchCaptureSelection, Sha256Digest, SourceCheckedRootDispatchPlan,
    VerifiedMakeInputReads,
};
use aros_common::make_input_snapshot::MakeInputRead;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

const FALLBACK_FENCE: &[u8] =
    b".DEFAULT:\n.DEFAULT:\n\t$(error AROS_ROOT_DISPATCH_UNEXPECTED_DEFAULT)\n";

/// Exact trailing input spellings, selected independently in private evidence.
#[derive(Clone, Copy)]
pub struct RootDispatchFencedFiles<'a> {
    pub override_input: &'a str,
    pub fence_input: &'a str,
}

/// Independently observed full startup and unchanged candidate selection.
#[derive(Clone, Copy)]
pub struct FencedRootDispatchRequest<'a> {
    pub root_input: &'a str,
    pub selected: RootDispatchCaptureSelection<'a>,
    pub files: RootDispatchFencedFiles<'a>,
    pub startup: &'a LiteralMakeStartupArguments,
    pub candidate_recorder_reads: &'a VerifiedMakeInputReads,
}

/// Data-only second-pass result; not executable/shell/owner launch authority.
/// The live caller binds the new native packet, original owner and deadline.
pub struct FencedRootDispatchPlan {
    source: SourceCheckedRootDispatchPlan,
    projection: MakeRecipeProjection,
    projection_reads: VerifiedMakeInputReads,
    generated_reads: VerifiedMakeInputReads,
    override_reads: VerifiedMakeInputReads,
    fence_reads: VerifiedMakeInputReads,
    pre_suffix_control_reads: VerifiedMakeInputReads,
    original_recipe_reads: Option<VerifiedMakeInputReads>,
}

impl FencedRootDispatchPlan {
    #[must_use]
    pub const fn source(&self) -> &SourceCheckedRootDispatchPlan {
        &self.source
    }
    #[must_use]
    pub const fn projection(&self) -> &MakeRecipeProjection {
        &self.projection
    }
    #[must_use]
    pub const fn projection_reads(&self) -> &VerifiedMakeInputReads {
        &self.projection_reads
    }

    /// Recheck the same complete final snapshot; does not authenticate a process.
    /// # Errors
    /// Refuses changed roots, including retained generated instrumentation.
    pub fn revalidate(&self) -> Result<(), String> {
        self.source.revalidate()
    }

    /// Recheck every retained input/absence against an independently bound
    /// extended snapshot. The caller must separately justify root transitions;
    /// this does not refresh the original snapshot or authorize fresh outputs.
    /// # Errors
    /// Refuses changed resolution/content/absence, unsafe roots or read limits.
    pub fn verify_read_closure(&self, snapshot: &MakeInputSnapshot) -> Result<(), String> {
        snapshot.verify_read_groups(&self.read_groups())
    }

    /// Exact verified groups retained for closure and remake-fence auditing.
    #[must_use]
    pub fn read_groups(&self) -> Vec<&VerifiedMakeInputReads> {
        let mut reads = vec![
            self.source.source_reads(),
            self.source.role_reads(),
            &self.generated_reads,
            &self.override_reads,
            &self.fence_reads,
            &self.pre_suffix_control_reads,
            &self.projection_reads,
        ];
        if let Some(controls) = self.source.recipe_controls() {
            reads.push(controls.reads());
        }
        if let Some(original) = &self.original_recipe_reads {
            reads.push(original);
        }
        reads
    }
}

impl RootDispatchCapturePlan {
    /// Fatal fallback recipe only; earlier explicit `.DEFAULT` prerequisites
    /// are not asserted to be erased. A sole prerequisite-free goal is required.
    #[must_use]
    pub const fn fallback_fence_bytes() -> &'static [u8] {
        FALLBACK_FENCE
    }
}

type ReadClosure = BTreeMap<String, (PathBuf, Option<Sha256Digest>)>;

fn closure(
    groups: impl IntoIterator<Item = impl AsRef<[MakeInputRead]>>,
) -> Result<ReadClosure, String> {
    let mut result = BTreeMap::new();
    let mut count = 0usize;
    for group in groups {
        for read in group.as_ref() {
            count = count
                .checked_add(1)
                .filter(|count| *count <= 131_072)
                .ok_or("Root dispatch read closure exceeds count bound")?;
            let value = (read.path.clone(), read.content_sha256.clone());
            if result
                .insert(read.identity.clone(), value.clone())
                .is_some_and(|old| old != value)
            {
                return Err("Root dispatch read identity has inconsistent content".into());
            }
        }
    }
    Ok(result)
}

impl SourceCheckedRootDispatchPlan {
    fn candidate_read_groups<'a>(
        &'a self,
        recorder: &'a VerifiedMakeInputReads,
    ) -> Result<Vec<&'a VerifiedMakeInputReads>, String> {
        let controls = self
            .recipe_controls()
            .ok_or("Root dispatch shell controls were not measured")?;
        let mut groups = vec![
            self.source_reads(),
            self.role_reads(),
            controls.reads(),
            recorder,
        ];
        if let Some(original) = self.original_recipe_reads() {
            groups.push(original);
        }
        Ok(groups)
    }

    /// Construct prospective full argv, preserving the original ordered argv.
    /// This MUST be sampled separately; it cannot reuse the candidate's packet.
    /// # Errors
    /// Refuses unmeasured controls, overlapping input spellings and startup limits.
    pub fn prospective_fenced_startup(
        &self,
        files: RootDispatchFencedFiles<'_>,
        recorder_reads: &VerifiedMakeInputReads,
    ) -> Result<LiteralMakeStartupArguments, String> {
        let groups = self.candidate_read_groups(recorder_reads)?;
        let reads = closure(groups.iter().map(|group| group.reads()))?;
        for input in [files.override_input, files.fence_input] {
            super::super::literal_path(Path::new(input))?;
            if reads.values().any(|(path, _)| path == Path::new(input)) {
                return Err("Root dispatch generated input overlaps an original input".into());
            }
        }
        if files.override_input == files.fence_input {
            return Err("Root dispatch generated inputs overlap".into());
        }
        let mut arguments = self.startup.arguments().to_vec();
        arguments.extend([
            "-f".into(),
            files.override_input.into(),
            "-f".into(),
            files.fence_input.into(),
        ]);
        let mut seen = BTreeSet::new();
        for argument in groups
            .into_iter()
            .flat_map(VerifiedMakeInputReads::old_file_arguments)
            .chain([
                format!("--old-file={}", files.override_input),
                format!("--old-file={}", files.fence_input),
            ])
        {
            if seen.insert(argument.clone()) {
                arguments.push(argument);
            }
        }
        LiteralMakeStartupArguments::admit(&arguments)
            .map_err(|_| "Root dispatch fenced startup is unsupported".into())
    }

    /// Reinterpret Root from a NEW observation of the exact prospective argv,
    /// then apply verified generated-file effects in their actual input order.
    /// The supplied program must come from that packet, not the first pass.
    /// No packet authentication, process launch or implicit fixed point occurs.
    /// # Errors
    /// Refuses changed argv/recipe/roles/closure, generated bytes, Source effects,
    /// unresolved controls, input drift or shared interpreter/snapshot bounds.
    pub fn rebind_fenced(
        &self,
        request: FencedRootDispatchRequest<'_>,
        program: &mut MakeVariableProgram,
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
    ) -> Result<FencedRootDispatchPlan, String> {
        self.revalidate()?;
        if request.startup
            != &self.prospective_fenced_startup(request.files, request.candidate_recorder_reads)?
            || self.startup.input_files() != [request.root_input]
        {
            return Err("Root dispatch final startup differs from its prospective binding".into());
        }
        let ((), generated_reads) = snapshot.transaction(|reader| {
            for (input, expected) in [
                (request.files.override_input, self.plan.override_bytes()),
                (request.files.fence_input, FALLBACK_FENCE),
            ] {
                let read = reader
                    .read_bytes(input)?
                    .ok_or("Root dispatch generated input is absent")?;
                if read.bytes != expected {
                    return Err("Root dispatch generated input bytes differ".into());
                }
            }
            Ok(())
        })?;
        let source = RootDispatchCapturePlan::derive_captured_source(
            request.root_input,
            self.plan.original_sha256(),
            request.selected,
            program,
            paths,
            snapshot,
            request.startup,
        )?
        .with_captured_recipe_controls(program, paths)?;
        if source.plan.override_bytes() != self.plan.override_bytes()
            || source.selected_recipe != self.selected_recipe
            || source.selected_stem != self.selected_stem
            || source.source_reads != self.source_reads
            || source.role_reads != self.role_reads
        {
            return Err("Root dispatch second-pass Source selection differs".into());
        }
        let original_controls = self
            .recipe_controls()
            .ok_or("candidate recipe controls are absent")?;
        let controls = source
            .recipe_controls()
            .ok_or("final recipe controls are absent")?;
        if controls.shell != original_controls.shell
            || controls.flags != original_controls.flags
            || controls.reads != original_controls.reads
        {
            return Err("Root dispatch second-pass pre-suffix controls differ".into());
        }
        let pre_suffix_control_reads = controls.reads.clone();
        let override_reads = program.record_captured_input_list_entry(
            request.files.override_input,
            &sha256_bytes(self.plan.override_bytes()),
            paths,
            snapshot,
        )?;
        let (_, fence_reads) =
            program.read_captured_file(request.files.fence_input, paths, snapshot)?;
        let source = source.with_captured_recipe_controls(program, paths)?;
        let (projection, projection_reads) = source.project_instrumented_recipe(program, paths)?;
        let result = FencedRootDispatchPlan {
            source,
            projection,
            projection_reads,
            generated_reads,
            override_reads,
            fence_reads,
            pre_suffix_control_reads,
            original_recipe_reads: self.original_recipe_reads().cloned(),
        };
        let mut expected_groups = self.candidate_read_groups(request.candidate_recorder_reads)?;
        expected_groups.push(&result.generated_reads);
        let expected = closure(expected_groups.iter().map(|group| group.reads()))?;
        let actual_groups = result.read_groups();
        if closure(actual_groups.iter().map(|group| group.reads()))? != expected {
            return Err(
                "Root dispatch final read closure differs; another fixed point is not admitted"
                    .into(),
            );
        }
        result.revalidate()?;
        Ok(result)
    }
}
