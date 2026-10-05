//! Diagnostic traversal, never an admission or a compilation graph.

use aros_common::Diagnostic;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize)]
pub struct AuditEndpoint {
    pub name: String,
    pub path: Vec<String>,
    pub handwritten_parents: Vec<String>,
}

/// Source shapes which have been understood but are not admitted providers.
#[derive(Debug, Serialize)]
pub struct PartialSourceProjection {
    pub family: &'static str,
    pub owner: String,
    pub outputs: usize,
    pub unresolved_contracts: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct NativeGraphAudit {
    pub schema_version: u32,
    pub qualification: &'static str,
    pub roots: Vec<String>,
    pub reachable: BTreeSet<String>,
    pub missing_endpoints: Vec<AuditEndpoint>,
    pub unproven_make_providers: Vec<AuditEndpoint>,
    pub unresolved_selectors: BTreeSet<String>,
    pub selected_capability_failures: Vec<Diagnostic>,
    /// These have no usable target identity. They are uncertainty, not evidence
    /// that a foreign architecture recipe belongs to this selected profile.
    pub unowned_capability_failures: Vec<Diagnostic>,
    pub unrelated_capability_failure_count: usize,
    pub cold_compilation_identities: BTreeSet<String>,
    pub producer_families: BTreeMap<String, BTreeSet<String>>,
    pub partial_source_projections: Vec<PartialSourceProjection>,
    /// This preserves the ordinary admission check. Completing an audit does
    /// not make this check succeed or authorize graph publication.
    pub strict_validation_error: Option<String>,
}

impl NativeGraphAudit {
    pub(super) fn new(roots: &[String]) -> Self {
        Self {
            schema_version: 1,
            qualification: "diagnostic-only-not-build-proof",
            roots: roots.to_vec(),
            reachable: BTreeSet::new(),
            missing_endpoints: Vec::new(),
            unproven_make_providers: Vec::new(),
            unresolved_selectors: BTreeSet::new(),
            selected_capability_failures: Vec::new(),
            unowned_capability_failures: Vec::new(),
            unrelated_capability_failure_count: 0,
            cold_compilation_identities: BTreeSet::new(),
            producer_families: BTreeMap::new(),
            partial_source_projections: Vec::new(),
            strict_validation_error: None,
        }
    }
}

pub(super) fn audit_endpoint(
    name: &str,
    parents: &BTreeMap<String, String>,
    explicit_edges: &BTreeSet<(String, String)>,
) -> AuditEndpoint {
    let mut path = vec![name.to_owned()];
    let mut cursor = name;
    while let Some(parent) = parents.get(cursor) {
        if path.contains(parent) {
            break;
        }
        path.push(parent.clone());
        cursor = parent;
    }
    path.reverse();
    AuditEndpoint {
        name: name.to_owned(),
        path,
        handwritten_parents: explicit_edges
            .iter()
            .filter(|(_, dependency)| dependency == name)
            .map(|(owner, _)| owner.clone())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        ast::{CopyDirectoryDecl, MetaTargetRule},
        binary_objects::BinaryObjectDecl,
        icons::IconTarget,
        DependencyGraph, TargetContext,
    };
    use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage};

    #[test]
    fn partial_source_projections_never_admit_make_providers() {
        use crate::source_archive_rules::{ArchiveMembers, SourceArchiveDecl};
        let mut graph = DependencyGraph::new();
        for owner in ["archive-owner", "header-owner", "compile-owner"] {
            graph.make_meta_providers.insert(owner.into());
            graph.add_explicit_meta_rule(MetaTargetRule {
                name: owner.into(),
                dependencies: Vec::new(),
            });
        }
        graph.source_archive_projections.push(SourceArchiveDecl {
            owner: "archive-owner".into(),
            file: "fixture/mmakefile.src".into(),
            line: 10,
            owner_line: 8,
            output: "${AROS_DEVELOPER_LIB_DIR}/libexample.a".into(),
            members: ArchiveMembers::Exact(vec!["${AROS_BUILD_DIR}/gen/example.o".into()]),
        });
        graph.layered_header_projections.push(
            crate::layered_header_copies::LayeredHeaderCopyDecl {
                owner: "header-owner".into(),
                file: "fixture/mmakefile.src".into(),
                line: 12,
                inputs: vec!["fixture/example.h".into()],
                copies: vec![crate::layered_header_copies::LayeredHeaderCopy {
                    source_relative: "fixture/example.h".into(),
                    output_alias: "${AROS_GENINC_DIR}/example.h".into(),
                }],
                unresolved_prerequisites: vec![
                    "unbound generated Make include".into(),
                    "setup".into(),
                ],
            },
        );
        graph.source_compile_projections.push(
            crate::source_compile_rules::SourceCompileGroupDecl {
                owner: "compile-owner".into(),
                parent: None,
                file: "fixture/mmakefile.src".into(),
                line: 14,
                archive_output: Some("${AROS_DEVELOPER_LIB_DIR}/libcompile.a".into()),
                objects: Vec::new(),
            },
        );
        let report = graph.audit_native_dependency_graph(
            &[
                "archive-owner".into(),
                "header-owner".into(),
                "compile-owner".into(),
            ],
            &TargetContext::default(),
            &[],
        );
        assert_eq!(report.partial_source_projections.len(), 3);
        assert_eq!(report.unproven_make_providers.len(), 3);
        assert!(report.strict_validation_error.is_some());
        assert!(!report.producer_families.iter().any(|(family, owners)| {
            // Source-written Make providers are not typed producer evidence.
            family != "make-meta-provider"
                && (owners.contains("header-owner")
                    || owners.contains("archive-owner")
                    || owners.contains("compile-owner"))
        }));
    }

    #[test]
    fn audit_collects_all_siblings_without_admitting_a_missing_endpoint() {
        let mut graph = DependencyGraph::new();
        graph.add_explicit_meta_rule(MetaTargetRule {
            name: "root".into(),
            dependencies: vec!["branch-a".into(), "branch-b".into()],
        });
        for branch in ["a", "b"] {
            graph.add_explicit_meta_rule(MetaTargetRule {
                name: format!("branch-{branch}"),
                dependencies: vec![format!("missing-{branch}")],
            });
        }
        let roots = vec!["root".into()];
        let report = graph.audit_native_dependency_graph(&roots, &TargetContext::default(), &[]);
        assert_eq!(report.missing_endpoints.len(), 2);
        assert_eq!(report.missing_endpoints[0].name, "missing-a");
        assert_eq!(report.missing_endpoints[1].name, "missing-b");
        assert_eq!(
            report.missing_endpoints[0].path,
            ["root", "branch-a", "missing-a"]
        );
        assert_eq!(
            report.missing_endpoints[1].handwritten_parents,
            ["branch-b"]
        );
        assert!(report.strict_validation_error.is_some());
        assert_eq!(
            graph.meta_targets.len(),
            3,
            "audit must not synthesize providers"
        );
        assert!(graph
            .selected_dependency_closure(&roots, &TargetContext::default(), &[])
            .is_err());
    }

    #[test]
    fn audit_reports_rejected_owners_and_unknown_selectors_despite_a_cycle() {
        let mut graph = DependencyGraph::new();
        graph.add_meta_rule(MetaTargetRule {
            name: "root".into(),
            dependencies: vec!["root".into(), "rejected".into(), "${UNKNOWN}".into()],
        });
        graph.make_meta_providers.insert("rejected".into());
        let failure = Diagnostic::error(
            DiagnosticCode::SourceParse,
            DiagnosticStage::Parsing,
            "unsupported recipe",
        )
        .with_context(DiagnosticContext {
            target: Some("rejected".into()),
            ..Default::default()
        });
        let unrelated = failure.clone().with_context(DiagnosticContext {
            target: Some("unselected".into()),
            ..Default::default()
        });
        let report = graph.audit_native_dependency_graph(
            &["root".into()],
            &TargetContext::default(),
            &[failure, unrelated],
        );
        assert_eq!(report.unproven_make_providers.len(), 1);
        assert_eq!(report.unproven_make_providers[0].name, "rejected");
        assert_eq!(report.selected_capability_failures.len(), 1);
        assert_eq!(report.unrelated_capability_failure_count, 1);
        assert!(report.unresolved_selectors.contains("${UNKNOWN}"));
        assert!(report.strict_validation_error.is_some());
    }

    #[test]
    fn audit_declares_every_modeled_producer_family() {
        let mut graph = DependencyGraph::new();
        graph.add_meta_rule(MetaTargetRule {
            name: "root".into(),
            dependencies: vec![],
        });

        let report =
            graph.audit_native_dependency_graph(&["root".into()], &TargetContext::default(), &[]);
        let expected: std::collections::BTreeSet<_> = [
            "compilation",
            "make-meta-provider",
            "fetch",
            "host-c-file",
            "host-header",
            "host-header-aggregate",
            "directory",
            "genmodule-header",
            "genmodule-writefiles",
            "sdk-object",
            "literal-object",
            "source-archive",
            "source-layered-header",
            "sdk-file-copy",
            "sdk-asset-rule",
            "sdk-text",
            "source-text",
            "source-value",
            "sfd-header",
            "header-copy",
            "package",
            "genmodule-includes",
            "genmodule-linklib",
            "genmodule-linklib-relative",
            "program-group-object",
            "external-cmake",
            "external-cmake-provider",
            "configure-build",
            "configure-provider",
            "grub-build",
            "ahi-build",
            "python-output",
            "script-output",
            "define-header",
            "header-transform",
            "flexcat-source",
            "catalog",
            "copy-directory",
            "flexcat-header",
            "ilbm-source",
            "bison-output",
            "icon",
            "binary-object",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(
            report
                .producer_families
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            expected
        );
    }

    #[test]
    fn audit_uses_exact_declared_owners_for_copy_icon_and_binary_producers() {
        let mut graph = DependencyGraph::new();
        graph.add_explicit_meta_rule(MetaTargetRule {
            name: "root".into(),
            dependencies: vec![
                "copy-owner".into(),
                "icon-owner".into(),
                "binary-owner".into(),
            ],
        });
        graph.copy_directories.push(CopyDirectoryDecl {
            name: "copy-owner".into(),
            source: "source".into(),
            destination: "destination".into(),
            file: "unit/mmakefile.src".into(),
            line: 1,
            dependencies: vec![],
        });
        graph.icon_targets.insert(
            "icon-owner".into(),
            IconTarget {
                mmake: "icon-owner".into(),
                directory: "unit".into(),
            },
        );
        graph.binary_objects.push(BinaryObjectDecl {
            name: "binary-owner".into(),
            output: "image.o".into(),
            directory: "unit".into(),
            sources: vec![],
            start: "0".into(),
            ldflags: vec![],
            consumer: "root".into(),
            arch_tag: String::new(),
        });

        let report =
            graph.audit_native_dependency_graph(&["root".into()], &TargetContext::default(), &[]);
        let expected = |name: &str| std::iter::once(name.to_owned()).collect();
        assert_eq!(
            report.producer_families["copy-directory"],
            expected("copy-owner")
        );
        assert_eq!(report.producer_families["icon"], expected("icon-owner"));
        assert_eq!(
            report.producer_families["binary-object"],
            expected("binary-owner")
        );
    }
}
