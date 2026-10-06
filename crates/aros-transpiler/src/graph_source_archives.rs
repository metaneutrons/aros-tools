//! Admission of complete source-archive and source-compile bindings.

use super::DependencyGraph;
use crate::ast::MetaTargetRule;
use crate::literal_objects::LiteralObjectGroupDecl;
use crate::source_archive_rules::ArchiveMembers;
use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage, SourceLocation};
use std::collections::{BTreeMap, BTreeSet};

/// Stable internal endpoint shared by graph binding and provenance indexing.
pub fn private_source_object_owner(
    group: &crate::source_compile_rules::SourceCompileGroupDecl,
) -> String {
    let identity = format!(
        "{}\n{}\n{}\n{}",
        group.file,
        group.owner,
        group.line,
        group
            .objects
            .iter()
            .map(|object| object.output.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    format!(
        "aros-source-objects-{}",
        aros_common::sha256_bytes(identity.as_bytes())
    )
}

impl DependencyGraph {
    fn source_archive_owner_conflicts(&self, name: &str) -> bool {
        self.writefiles_has_other_producer(name)
            || self
                .host_file_generators
                .iter()
                .any(|rule| rule.owner == name)
            || self.sdk_asset_rules.iter().any(|rule| rule.owner == name)
            || self
                .genmodule_writefiles_rules
                .iter()
                .any(|rule| rule.owner == name)
    }

    /// Join complete source projections before native endpoint selection.
    /// This never calls the legacy synthetic HIDD archive resolver.
    ///
    /// # Panics
    /// Panics if the internal binding pass returns a qualified HIDD group
    /// without its proven macro parent.
    pub fn bind_source_archives(&mut self) -> Vec<Diagnostic> {
        let mut reachable = BTreeMap::new();
        for archive in &self.source_archive_projections {
            if !matches!(archive.members, ArchiveMembers::ProducerGlob { .. }) {
                continue;
            }
            // A proved macro contributes its own parent -> child edge. The
            // archive either is that parent or must explicitly require it.
            // The source uses a distinct intermediate HIDD parent endpoint.
            let allowed = self
                .source_compile_projections
                .iter()
                .filter(|group| {
                    group.parent.as_ref().is_some_and(|parent| {
                        parent == &archive.owner
                            || self
                                .explicit_meta_edges
                                .contains(&(archive.owner.clone(), parent.clone()))
                    })
                })
                .map(|group| {
                    (
                        group.parent.clone().expect("filtered typed parent"),
                        group.owner.clone(),
                    )
                })
                .collect::<BTreeSet<_>>();
            reachable.insert((archive.file.clone(), archive.owner.clone()), allowed);
        }
        let binding = crate::source_archive_binding::bind_source_archives(
            &self.source_archive_projections,
            &self.source_compile_projections,
            &self.source_archive_commands,
            &reachable,
        );
        let mut diagnostics = binding
            .rejections
            .into_iter()
            .map(|rejection| {
                Diagnostic::error(
                    DiagnosticCode::CapabilityDrift,
                    DiagnosticStage::CapabilityValidation,
                    format!("Source archive binding is unproven: {}", rejection.reason),
                )
                .with_location(SourceLocation {
                    path: rejection.file,
                    line: Some(rejection.line),
                    column: None,
                })
                .with_context(DiagnosticContext {
                    target: Some(rejection.owner),
                    ..Default::default()
                })
            })
            .collect::<Vec<_>>();
        for group in binding.qualified_hidd_groups {
            if self.source_archive_owner_conflicts(&group.owner) {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::CapabilityDrift,
                        DiagnosticStage::CapabilityValidation,
                        format!(
                            "Source HIDD object owner {} conflicts with another producer",
                            group.owner
                        ),
                    )
                    .with_context(DiagnosticContext {
                        target: Some(group.owner),
                        ..Default::default()
                    }),
                );
                continue;
            }
            let parent = group
                .parent
                .as_ref()
                .expect("binder proved the macro parent");
            self.add_meta_rule(MetaTargetRule {
                name: parent.clone(),
                dependencies: vec![group.owner.clone()],
            });
            self.add_meta_rule(MetaTargetRule {
                name: group.owner.clone(),
                dependencies: vec!["includes".into(), "includes-copy".into()],
            });
            self.literal_object_groups.push(LiteralObjectGroupDecl {
                owner: group.owner,
                file: group.file,
                line: group.line,
                objects: group.objects,
            });
        }
        for archive in binding.archives {
            let owner = &archive.declaration.owner;
            let interface = format!("{owner}-archive");
            if self.source_archive_owner_conflicts(owner)
                || self.source_archive_owner_conflicts(&interface)
            {
                diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::CapabilityDrift,
                        DiagnosticStage::CapabilityValidation,
                        format!("Source archive owner {owner} conflicts with another producer"),
                    )
                    .with_context(DiagnosticContext {
                        target: Some(owner.clone()),
                        ..Default::default()
                    }),
                );
                continue;
            }
            let mut dependencies = archive.producer_owners.clone();
            for group in archive
                .compile_groups
                .iter()
                .filter(|group| group.parent.is_none())
            {
                let private_owner = private_source_object_owner(group);
                dependencies.insert(private_owner.clone());
                self.literal_object_groups.push(LiteralObjectGroupDecl {
                    owner: private_owner,
                    file: group.file.clone(),
                    line: group.line,
                    objects: group.objects.clone(),
                });
            }
            self.add_meta_rule(MetaTargetRule {
                name: owner.clone(),
                dependencies: dependencies.into_iter().collect(),
            });
            self.add_meta_rule(MetaTargetRule {
                name: interface,
                dependencies: vec![owner.clone()],
            });
            self.source_archives.push(archive);
        }
        diagnostics
    }

    /// Discharge only the exact missing-recipe diagnostic proved by the
    /// complete binder. Other diagnostics for that owner remain fatal.
    pub fn discharge_bound_archive_provider_failures(&self, failures: &mut Vec<Diagnostic>) {
        failures.retain(|diagnostic| {
            !self.source_archives.iter().any(|archive| {
                diagnostic.context.as_ref().and_then(|context| context.target.as_ref())
                    == Some(&archive.declaration.owner)
                    && diagnostic.location.as_ref().is_some_and(|location| location.path == archive.declaration.file)
                    && diagnostic.message == format!(
                        "nonvirtual Make provider {} has no concrete producer in its declaring mmakefile",
                        archive.declaration.owner
                    )
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::literal_objects::LiteralObjectDecl;
    use crate::source_archive_command::SourceArchiveCommand;
    use crate::source_archive_rules::SourceArchiveDecl;
    use crate::source_compile_rules::SourceCompileGroupDecl;

    fn fixture(glob: bool) -> DependencyGraph {
        let file = "compiler/libexample/mmakefile.src";
        let owner = "linklibs-example";
        let output = "${AROS_BUILD_DIR}/SYS/Developer/lib/libexample.a";
        let objects = ["a", "b"].map(|name| LiteralObjectDecl {
            source: format!("${{AROS_SOURCE_DIR}}/compiler/libexample/{name}.c"),
            output: format!("${{AROS_BUILD_DIR}}/gen/lib/hidd/{name}.o"),
            language: "C".into(),
            arguments: Vec::new(),
            line: 15,
        });
        let mut graph = DependencyGraph::default();
        graph.source_archive_projections.push(SourceArchiveDecl {
            owner: owner.into(),
            file: file.into(),
            line: 20,
            owner_line: 10,
            output: output.into(),
            members: if glob {
                ArchiveMembers::ProducerGlob {
                    root: "${AROS_BUILD_DIR}/gen/lib/hidd".into(),
                    pattern: "*.o".into(),
                }
            } else {
                ArchiveMembers::Exact(objects.iter().map(|object| object.output.clone()).collect())
            },
        });
        graph.source_archive_commands.insert(
            (file.into(), owner.into()),
            SourceArchiveCommand {
                flags: vec!["cr".into()],
            },
        );
        for object in objects {
            let group_owner = if glob {
                format!(
                    "hidd-{}-stubs",
                    object
                        .output
                        .rsplit('/')
                        .next()
                        .unwrap()
                        .trim_end_matches(".o")
                )
            } else {
                owner.into()
            };
            if glob {
                graph
                    .explicit_meta_edges
                    .insert((owner.into(), "source-hidd-members".into()));
            }
            graph
                .source_compile_projections
                .push(SourceCompileGroupDecl {
                    owner: group_owner,
                    parent: glob.then(|| "source-hidd-members".into()),
                    file: file.into(),
                    line: 15,
                    archive_output: (!glob).then(|| output.into()),
                    objects: vec![object],
                });
        }
        graph
    }

    #[test]
    fn hidd_glob_requires_reachable_source_parent_and_typed_children() {
        let mut graph = fixture(true);
        assert!(graph.bind_source_archives().is_empty());
        assert_eq!(graph.source_archives.len(), 1);
        assert_eq!(graph.source_archives[0].members.len(), 2);
        let mut missing = fixture(true);
        missing.explicit_meta_edges.clear();
        assert!(!missing.bind_source_archives().is_empty());
        assert!(missing.source_archives.is_empty());
        let mut self_edge = fixture(true);
        self_edge.explicit_meta_edges.clear();
        self_edge
            .explicit_meta_edges
            .insert(("linklibs-example".into(), "linklibs-example".into()));
        assert!(!self_edge.bind_source_archives().is_empty());
        assert!(self_edge.source_archives.is_empty());
    }

    #[test]
    fn exact_archive_groups_have_distinct_private_identities() {
        let mut graph = fixture(false);
        assert!(graph.bind_source_archives().is_empty());
        assert_eq!(graph.literal_object_groups.len(), 2);
        assert_ne!(
            graph.literal_object_groups[0].owner,
            graph.literal_object_groups[1].owner
        );
        assert_eq!(graph.meta_targets["linklibs-example"].len(), 2);
    }

    #[test]
    fn default_links_bind_ordinary_archives_and_refuse_competing_modules() {
        use crate::default_link_set::{DefaultLinkItem, DefaultLinkSet};
        let mut graph = fixture(false);
        assert!(graph.bind_source_archives().is_empty());
        let set = DefaultLinkSet {
            items: vec![DefaultLinkItem {
                name: "example".into(),
                require_absent: vec!["noexample".into()],
                require_present: Vec::new(),
            }],
        };
        assert!(graph.resolve_default_link_set(&set).is_empty());
        assert_eq!(
            graph.default_link_set[0].archive,
            "linklibs-example-archive"
        );
        assert_eq!(graph.default_link_set[0].require_absent, ["noexample"]);

        let tree = crate::testing::TempTree::new();
        let file = tree.0.join("compiler/competing/mmakefile.src");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file.parent().unwrap().join("example.c"), "int example;\n").unwrap();
        std::fs::write(
            &file,
            "%build_linklib mmake=competing-example libname=example files=example\n",
        )
        .unwrap();
        let dirs = crate::dirs::DirVars::load(&tree.0);
        let parsed = crate::parse_mmakefile_with_dirs(&file, &tree.0, &dirs).unwrap();
        assert_eq!(parsed.targets.len(), 1);
        graph.add_target(parsed.targets.into_iter().next().unwrap());
        let failures = graph.resolve_default_link_set(&set);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("ambiguous"), "{failures:?}");
        assert!(graph.default_link_set.is_empty());
    }

    #[test]
    fn only_bound_missing_provider_diagnostic_is_discharged() {
        let mut graph = fixture(false);
        assert!(graph.bind_source_archives().is_empty());
        let diagnostic = Diagnostic::error(DiagnosticCode::CapabilityDrift, DiagnosticStage::CapabilityValidation,
            "nonvirtual Make provider linklibs-example has no concrete producer in its declaring mmakefile")
            .with_location(SourceLocation { path: "compiler/libexample/mmakefile.src".into(), line: Some(10), column: None })
            .with_context(DiagnosticContext { target: Some("linklibs-example".into()), ..Default::default() });
        let mut foreign = diagnostic.clone();
        foreign.location.as_mut().unwrap().path = "compiler/other/mmakefile.src".into();
        let mut different = diagnostic.clone();
        different.message = "source archive has unsafe inputs".into();
        let mut failures = vec![diagnostic, foreign, different];
        graph.discharge_bound_archive_provider_failures(&mut failures);
        assert_eq!(failures.len(), 2);
    }

    #[test]
    fn source_archive_owner_cannot_replace_an_existing_header_producer() {
        let mut graph = fixture(false);
        graph
            .header_transforms
            .push(crate::copy_includes::HeaderTransformDecl {
                name: "linklibs-example-archive".into(),
                file: "compiler/foreign/mmakefile.src".into(),
                line: 1,
                input: "${AROS_SOURCE_DIR}/seed.h".into(),
                output: "${AROS_SDK_INCLUDE_DIR}/seed.h".into(),
                match_text: String::new(),
                replacement: String::new(),
                copy_only: true,
                replace_whole_line_containing: false,
                substitutions: Vec::new(),
                dependencies: Vec::new(),
                consumers: Vec::new(),
                generated_input_owner: None,
            });
        let failures = graph.bind_source_archives();
        assert_eq!(failures.len(), 1);
        assert!(graph.source_archives.is_empty());
        assert!(graph.literal_object_groups.is_empty());
    }
}
