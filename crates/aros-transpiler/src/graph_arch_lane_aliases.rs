//! Apply opaque, independently verified architecture lane aliases as CMake
//! source-selection declarations. This creates no MetaMake endpoint/provider.

use super::{ArchSourceDecl, DependencyGraph};
use crate::arch_endpoint_effects::{ArchEndpointEffect, ArchEndpointEffectData};
use crate::native_owner_projection::VerifiedArchLaneAlias;
use std::collections::BTreeSet;

const MAX_ALIASES: usize = 128;

impl DependencyGraph {
    /// Retag exact original architecture source groups through verified
    /// handwritten MetaMake edges. Original effects and source declarations
    /// remain intact; only the cloned CMake source tag changes.
    ///
    /// The graph must already retain each original effect selected by the
    /// native binder. The opaque proofs are also checked against this graph's
    /// exact MetaMake route and source group before any mutation occurs.
    ///
    /// # Errors
    /// Refuses stale/missing graph routes, mismatched source groups, physical
    /// owners at the selector endpoint, duplicate aliases, and file collisions.
    pub fn apply_arch_lane_aliases(
        &mut self,
        aliases: &[VerifiedArchLaneAlias],
    ) -> Result<Vec<String>, String> {
        if aliases.len() > MAX_ALIASES {
            return Err("architecture lane alias application limit exceeded".into());
        }
        let mut plans = Vec::with_capacity(aliases.len());
        let mut seen = BTreeSet::new();
        let mut planned_destinations = BTreeSet::new();

        for alias in aliases {
            let (mainmmake, source_tag, selected_tag, consumer, dependency) = alias.route();
            if source_tag == selected_tag
                || !self.arch_endpoint_effects.contains(alias.original_effect())
                || !self
                    .meta_targets
                    .get(consumer)
                    .is_some_and(|dependencies| dependencies.contains(dependency))
            {
                return Err(format!(
                    "verified architecture lane alias no longer matches the graph route: {consumer} -> {dependency}"
                ));
            }
            let ArchEndpointEffectData::ArchModuleObjects {
                mainmmake: effect_main,
                tag: effect_tag,
                module_sources,
                linklib_sources: _,
                directory,
            } = &alias.original_effect().data
            else {
                return Err(
                    "verified lane alias no longer names an architecture object effect".into(),
                );
            };
            if effect_main.as_str() != mainmmake
                || effect_tag.as_str() != source_tag
                || alias.original_effect().endpoint.as_str() != dependency
                || dependency != format!("{mainmmake}-{source_tag}")
                || consumer != format!("{mainmmake}-{selected_tag}")
            {
                return Err("verified architecture lane alias effect identity changed".into());
            }
            // A virtual selector must not become a second physical owner
            // through a different graph declaration.
            if self.make_meta_providers.contains(consumer) {
                return Err(format!(
                    "architecture lane alias selector has a physical provider: {consumer}"
                ));
            }

            let source_groups = self
                .arch_sources
                .get(mainmmake)
                .ok_or("verified architecture lane alias has no source group")?;
            let matching = source_groups
                .iter()
                .filter(|source| source_group_matches_effect(source, alias.original_effect()))
                .collect::<Vec<_>>();
            let [source] = matching.as_slice() else {
                return Err(format!(
                    "verified architecture lane alias has no unique exact source group: {mainmmake} {source_tag}"
                ));
            };
            let mut files = source.files.clone();
            files.sort();
            let identity = (
                mainmmake.to_owned(),
                source_tag.to_owned(),
                selected_tag.to_owned(),
                files.clone(),
            );
            let destination_identity =
                (mainmmake.to_owned(), selected_tag.to_owned(), files.clone());
            if !seen.insert(identity) || !planned_destinations.insert(destination_identity) {
                return Err("duplicate architecture lane alias source group".into());
            }
            if source_groups.iter().any(|other| {
                other.tag == selected_tag
                    && other.files.iter().any(|file| source.files.contains(file))
            }) {
                return Err(format!(
                    "architecture lane alias collides with an existing {selected_tag} source group"
                ));
            }
            plans.push((
                mainmmake.to_owned(),
                (**source).clone(),
                selected_tag.to_owned(),
                source.tag.clone(),
                directory.clone(),
                module_sources.clone(),
            ));
        }

        let mut notes = Vec::new();
        for (mainmmake, source, selected_tag, source_tag, directory, module_sources) in plans {
            let clone = ArchSourceDecl {
                tag: selected_tag.clone(),
                // The source edge aliases module objects only. The separate
                // client archive lane retains its original architecture tag.
                linklib_files: Vec::new(),
                ..source
            };
            // The plan is constructed only after exact source/effect matching;
            // these checks document the object-only cloning invariant.
            debug_assert_eq!(clone.mainmmake, mainmmake);
            debug_assert_eq!(clone.dir, directory);
            debug_assert!(module_sources.iter().all(|file| clone.files.contains(file)));
            debug_assert_eq!(clone.linklib_files.len(), 0);
            self.arch_sources
                .entry(mainmmake.clone())
                .or_default()
                .push(clone);
            notes.push(format!(
                "{mainmmake}: source lane {source_tag} also applies to {selected_tag}"
            ));
        }
        Ok(notes)
    }
}

fn source_group_matches_effect(source: &ArchSourceDecl, effect: &ArchEndpointEffect) -> bool {
    let ArchEndpointEffectData::ArchModuleObjects {
        mainmmake,
        tag,
        module_sources,
        linklib_sources,
        directory,
    } = &effect.data
    else {
        return false;
    };
    let expected: BTreeSet<_> = module_sources.iter().collect();
    let actual: BTreeSet<_> = source.files.iter().collect();
    expected.len() == module_sources.len()
        && actual.len() == source.files.len()
        && source.mainmmake.as_str() == mainmmake.as_str()
        && source.tag.as_str() == tag.as_str()
        && source.dir.as_str() == directory.as_str()
        && source.linklib_files.as_slice() == linklib_sources.as_slice()
        && actual == expected
}

#[cfg(test)]
mod tests {
    use super::source_group_matches_effect;
    use crate::arch_endpoint_effects::{ArchEndpointEffect, ArchEndpointEffectData};
    use crate::arch_sources::ArchSourceDecl;

    fn effect() -> ArchEndpointEffect {
        ArchEndpointEffect {
            recipe: "arch/i386-all/debug/mmakefile.src".into(),
            line: 1,
            endpoint: "kernel-debug-i386".into(),
            dependencies: vec!["kernel-debug-i386-includes".into()],
            data: ArchEndpointEffectData::ArchModuleObjects {
                mainmmake: "kernel-debug".into(),
                tag: "i386".into(),
                module_sources: vec!["debug_platform".into(), "disassemble".into()],
                linklib_sources: Vec::new(),
                directory: "arch/i386-all/debug".into(),
            },
        }
    }

    fn source_decl() -> ArchSourceDecl {
        ArchSourceDecl {
            mainmmake: "kernel-debug".into(),
            maindir: Some("rom/debug".into()),
            modname: Some("debug".into()),
            tag: "i386".into(),
            dir: "arch/i386-all/debug".into(),
            files: vec!["debug_platform".into(), "disassemble".into()],
            linklib_files: Vec::new(),
            extra_quote_include: None,
            include_dirs: Vec::new(),
            defines: Vec::new(),
            compile_options: Vec::new(),
            line: 0,
        }
    }

    #[test]
    fn exact_original_source_group_matches_its_effect() {
        assert!(source_group_matches_effect(&source_decl(), &effect()));
    }

    #[test]
    fn source_group_binding_refuses_wrong_lane_directory_files_or_linklib() {
        let effect = effect();
        let mut decl = source_decl();
        decl.tag = "x86_64".into();
        assert!(!source_group_matches_effect(&decl, &effect));
        decl = source_decl();
        decl.dir = "arch/x86_64-all/debug".into();
        assert!(!source_group_matches_effect(&decl, &effect));
        decl = source_decl();
        decl.files.pop();
        assert!(!source_group_matches_effect(&decl, &effect));
        decl = source_decl();
        decl.linklib_files.push("client_init".into());
        assert!(!source_group_matches_effect(&decl, &effect));
    }
}
