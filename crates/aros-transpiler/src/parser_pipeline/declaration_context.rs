//! Shared state handed to the build-macro declaration passes.
//!
//! The passes were consecutive loops of one function. The read-only scope
//! they share is `DeclarationInputs`; the accumulators they append to are
//! `DeclarationOutputs`. Splitting them out changes no evaluation order.

use std::path::PathBuf;

use super::{
    Diagnostic, FetchDecl, Invocation, MetaTargetRule, Path, Regex, TargetContext, TargetDefinition,
};

pub(super) type KobjInputsCapture<'a> =
    dyn Fn(&Invocation, &str) -> Option<crate::kobj_scoped_inputs::KobjScopedInputs> + 'a;

#[derive(Clone, Copy)]
pub(super) struct DeclarationInputs<'a> {
    pub(super) root: &'a Path,
    pub(super) dirs: &'a crate::dirs::DirVars,
    pub(super) target: Option<&'a TargetContext>,
    pub(super) rel_dir: &'a PathBuf,
    pub(super) relative_path: &'a PathBuf,
    pub(super) parent_dir: &'a Path,
    pub(super) content: &'a str,
    pub(super) joined: &'a str,
    pub(super) scope: &'a crate::make_vars::VarScope,
    pub(super) flag_set: &'a crate::flags::FlagSet,
    pub(super) include_set: &'a crate::includes::IncludeSet,
    pub(super) invocations: &'a Vec<Invocation>,
    pub(super) fetches: &'a Vec<FetchDecl>,
    pub(super) re_libs: &'a Regex,
    pub(super) opts_arch_includes: &'a Vec<(String, String)>,
    pub(super) opts_include_dirs: &'a Vec<String>,
    pub(super) opts_link_options: &'a Vec<String>,
    pub(super) opts_spec_switches: &'a Vec<String>,
    pub(super) arch_defines: &'a Vec<(String, String)>,
    pub(super) arch_compile_options: &'a Vec<(String, String)>,
    pub(super) capture_kobj_inputs: &'a KobjInputsCapture<'a>,
}

pub(super) struct DeclarationOutputs<'a> {
    pub(super) meta_rules: &'a mut Vec<MetaTargetRule>,
    pub(super) targets: &'a mut Vec<TargetDefinition>,
    pub(super) capability_errors: &'a mut Vec<Diagnostic>,
    pub(super) skipped_programs: &'a mut Vec<String>,
    pub(super) skipped_client_archives: &'a mut Vec<String>,
    pub(super) unresolved_output_paths: &'a mut Vec<String>,
    pub(super) partial_source_lists: &'a mut Vec<String>,
    pub(super) source_inventory_patterns: &'a mut Vec<String>,
    pub(super) source_inventory_needs: &'a mut Vec<crate::ast::SourceInventoryNeed>,
    pub(super) source_inventory_targets: &'a mut Vec<crate::ast::InventoryTargetIdentity>,
}

impl DeclarationOutputs<'_> {
    pub(super) const fn reborrow(&mut self) -> DeclarationOutputs<'_> {
        DeclarationOutputs {
            meta_rules: &mut *self.meta_rules,
            targets: &mut *self.targets,
            capability_errors: &mut *self.capability_errors,
            skipped_programs: &mut *self.skipped_programs,
            skipped_client_archives: &mut *self.skipped_client_archives,
            unresolved_output_paths: &mut *self.unresolved_output_paths,
            partial_source_lists: &mut *self.partial_source_lists,
            source_inventory_patterns: &mut *self.source_inventory_patterns,
            source_inventory_needs: &mut *self.source_inventory_needs,
            source_inventory_targets: &mut *self.source_inventory_targets,
        }
    }
}
