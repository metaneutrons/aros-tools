//! Exact additional output claims for selected literal and closed build rules.
//!
//! The existing graph source-header validator owns typed file producers. This
//! module supplies the remaining declared files, populated trees, and
//! mkdir-only roots without evaluating CMake/Make expressions or walking the
//! filesystem. Root aliases and physical path normalization belong to the
//! caller.

use crate::arch_endpoint_effects::ArchEndpointEffectData;
use crate::graph::DependencyGraph;
use std::collections::BTreeSet;

/// Additional outputs, paired with the concrete workflow that owns them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdditionalOutputs {
    /// `(physical workflow owner, exact output path)` claims.
    pub(crate) files: Vec<(String, String)>,
    /// `(physical workflow owner, populated tree root)` claims.
    pub(crate) trees: Vec<(String, String)>,
    /// `(physical workflow owner, directory created without file contents)`.
    pub(crate) directories: Vec<(String, String)>,
}

/// Collects claims from selected concrete workflow endpoints.
///
/// Provider aliases select their owning workflow, but are never themselves
/// recorded as output owners. Each declaration is visited once, even when
/// both its workflow and provider alias occur in `selected`.
#[must_use]
pub fn collect(graph: &DependencyGraph, selected: &BTreeSet<String>) -> AdditionalOutputs {
    let mut outputs = AdditionalOutputs::default();

    for package in &graph.packages {
        if package.resolved.is_empty() || !selected.contains(&package.mmake) {
            continue;
        }
        add_unique(&mut outputs.files, &package.mmake, &package.output);
    }

    for declaration in &graph.external_cmake {
        if !selected.contains(&declaration.mmake_name)
            && !selected.contains(&declaration.provider_target)
        {
            continue;
        }
        add_unique(
            &mut outputs.trees,
            &declaration.mmake_name,
            &declaration.binary_dir,
        );
        for product in declaration
            .library_products
            .iter()
            .chain(&declaration.header_products)
            .chain(&declaration.auxiliary_products)
        {
            add_unique(
                &mut outputs.files,
                &declaration.mmake_name,
                &rooted_path(product, &declaration.install_prefix),
            );
        }
    }

    for declaration in &graph.configure_builds {
        if !selected.contains(&declaration.mmake_name)
            && !declaration
                .provider_target
                .as_ref()
                .is_some_and(|provider| selected.contains(provider))
        {
            continue;
        }
        add_unique(
            &mut outputs.trees,
            &declaration.mmake_name,
            &declaration.binary_dir,
        );
        for product in declaration
            .private_products
            .iter()
            .chain(&declaration.install_products)
        {
            add_unique(
                &mut outputs.files,
                &declaration.mmake_name,
                &rooted_path(product, "${AROS_BUILD_DIR}"),
            );
        }
    }

    for fetch in &graph.fetches {
        if !selected.contains(&fetch.name) {
            continue;
        }
        let location = if fetch.location.is_empty() {
            &fetch.destination
        } else {
            &fetch.location
        };
        let candidates = archive_candidates(&fetch.archive, &fetch.suffixes);
        for candidate in &candidates {
            add_joined(&mut outputs.files, &fetch.name, location, candidate);
        }
        add_unique(&mut outputs.trees, &fetch.name, &fetch.destination);

        if fetch.normalization == "canonical-tar-gzip-v1" {
            const CANONICAL_CACHE: &str = ".aros-fetch-canonical-tar-gzip-v1";
            let cache_root = joined_path(location, CANONICAL_CACHE);
            add_unique(&mut outputs.trees, &fetch.name, &cache_root);
            for candidate in &candidates {
                add_joined(&mut outputs.files, &fetch.name, &cache_root, candidate);
            }
        }

        let base = if fetch.base.is_empty() {
            &fetch.destination
        } else {
            &fetch.base
        };
        if fetch.patches.split_whitespace().any(|patch| patch != "::") {
            // aros-fetch stores declaration-bound patch downloads and payloads
            // beneath this private namespace; claim the known root rather than
            // duplicating its internal cache-key algorithm here.
            add_joined(&mut outputs.trees, &fetch.name, base, ".aros-fetch");
        }
    }

    for declaration in &graph.directory_setups {
        if !selected.contains(&declaration.owner) {
            continue;
        }
        for directory in &declaration.directories {
            add_unique(&mut outputs.directories, &declaration.owner, directory);
        }
    }

    for effect in &graph.arch_endpoint_effects {
        if !selected.contains(&effect.endpoint) {
            continue;
        }
        if let ArchEndpointEffectData::SetArchIncludes {
            order_only_directory,
            ..
        } = &effect.data
        {
            add_unique(
                &mut outputs.directories,
                &effect.endpoint,
                order_only_directory,
            );
        }
    }

    for declaration in &graph.grub_builds {
        if !selected.contains(&declaration.mmake_name) {
            continue;
        }
        add_unique(
            &mut outputs.trees,
            &declaration.mmake_name,
            &declaration.binary_dir,
        );
        add_unique(
            &mut outputs.trees,
            &declaration.mmake_name,
            &declaration.install_prefix,
        );
    }

    for declaration in &graph.ahi_builds {
        if !selected.contains(&declaration.mmake_name) {
            continue;
        }
        add_unique(
            &mut outputs.trees,
            &declaration.mmake_name,
            &declaration.binary_dir,
        );
        add_unique(
            &mut outputs.trees,
            &declaration.mmake_name,
            &declaration.install_prefix,
        );
    }

    outputs
}

fn add_unique(claims: &mut Vec<(String, String)>, owner: &str, path: &str) {
    let claim = (owner.to_owned(), path.to_owned());
    if !claims.contains(&claim) {
        claims.push(claim);
    }
}

fn add_joined(claims: &mut Vec<(String, String)>, owner: &str, root: &str, child: &str) {
    add_unique(claims, owner, &joined_path(root, child));
}

fn joined_path(root: &str, child: &str) -> String {
    format!("{}/{child}", root.trim_end_matches('/'))
}

fn rooted_path(path: &str, root: &str) -> String {
    if path.starts_with('/') || path.starts_with("${") || path.starts_with("$(") {
        path.to_owned()
    } else {
        joined_path(root, path)
    }
}

fn archive_candidates(archive: &str, suffixes: &str) -> Vec<String> {
    let suffixes = suffixes.split_whitespace().collect::<Vec<_>>();
    let candidates = if suffixes.is_empty() {
        vec![archive.to_owned()]
    } else {
        suffixes
            .into_iter()
            .map(|suffix| format!("{archive}.{suffix}"))
            .collect()
    };
    let mut unique = Vec::new();
    for candidate in candidates {
        if !unique.contains(&candidate) {
            unique.push(candidate);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch_endpoint_effects::{ArchEndpointEffect, ArchEndpointEffectData};
    use crate::ast::ExternalCMakeDecl;
    use crate::directory_setup::DirectorySetupDecl;
    use crate::fetch::FetchDecl;
    use crate::packages::{PackageDecl, ResolvedPackageMember};
    use std::path::PathBuf;

    fn external() -> ExternalCMakeDecl {
        ExternalCMakeDecl {
            mmake_name: "external-workflow".into(),
            source_dir: "${AROS_BUILD_DIR}/ports/unit/src".into(),
            binary_dir: "${AROS_BUILD_DIR}/gen/external-cmake/unit".into(),
            install_prefix: "${AROS_BUILD_DIR}/gen/external-install/unit".into(),
            fetch_target: "unit-fetch".into(),
            local_patch_files: Vec::new(),
            provided_library: "unit".into(),
            provider_target: "external-provider-alias".into(),
            library_products: vec![
                "${AROS_BUILD_DIR}/gen/external-install/unit/lib/libunit.a".into()
            ],
            header_products: vec![
                "${AROS_BUILD_DIR}/gen/external-install/unit/include/unit.h".into()
            ],
            auxiliary_products: vec![
                "${AROS_BUILD_DIR}/gen/external-install/unit/lib/pkgconfig/unit.pc".into(),
            ],
            public_include_dirs: Vec::new(),
            options: Vec::new(),
            build_targets: Vec::new(),
            install_components: Vec::new(),
            library_group: false,
            host_tools: Vec::new(),
            compile_defines: Vec::new(),
            dir_path: PathBuf::from("workbench/unit"),
        }
    }

    #[test]
    fn claims_package_and_external_products_under_the_physical_workflow() {
        let mut graph = DependencyGraph::default();
        graph.packages.push(PackageDecl {
            file: "rom/package.mmakefile.src".into(),
            mmake: "unit-package".into(),
            output: "${AROS_ARCH_DIR}/unit.pkg".into(),
            members: Vec::new(),
            startup: None,
            uselibs: Vec::new(),
            is_kickstart: false,
            resolved: vec![ResolvedPackageMember {
                target: "unit-module".into(),
                runtime_name: "unit.library".into(),
            }],
            arch: String::new(),
        });
        graph.external_cmake.push(external());
        let selected = [
            "unit-package",
            "external-workflow",
            "external-provider-alias",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let outputs = collect(&graph, &selected);

        assert!(outputs
            .files
            .contains(&("unit-package".into(), "${AROS_ARCH_DIR}/unit.pkg".into())));
        assert!(outputs.files.contains(&(
            "external-workflow".into(),
            "${AROS_BUILD_DIR}/gen/external-install/unit/lib/libunit.a".into()
        )));
        assert_eq!(
            outputs
                .trees
                .iter()
                .filter(|(owner, path)| {
                    owner == "external-workflow"
                        && path == "${AROS_BUILD_DIR}/gen/external-cmake/unit"
                })
                .count(),
            1
        );
        assert_eq!(
            outputs
                .files
                .iter()
                .filter(|(owner, _)| owner == "external-workflow")
                .count(),
            3
        );
        assert!(outputs
            .files
            .iter()
            .all(|(owner, _)| owner != "external-provider-alias"));
    }

    #[test]
    fn collects_fetch_candidates_and_exact_directory_roots() {
        let mut graph = DependencyGraph::default();
        let fetch = FetchDecl {
            name: "unit-fetch".into(),
            archive: "unit".into(),
            suffixes: "tar.gz tgz".into(),
            origins: "https://example.invalid/unit".into(),
            checksums: String::new(),
            normalization: String::new(),
            normalized_size: String::new(),
            location: "${AROS_BUILD_DIR}/cache".into(),
            destination: "${AROS_BUILD_DIR}/ports/unit".into(),
            base: String::new(),
            patch_origins: "${AROS_SOURCE_DIR}/ports".into(),
            patches: "fix.diff:src:-p1,-f".into(),
            dir: "ports/unit".into(),
        };
        graph.fetches.push(fetch);
        graph.directory_setups.push(DirectorySetupDecl {
            owner: "directory-owner".into(),
            directories: vec!["${AROS_BUILD_DIR}/gen/setup/include".into()],
        });
        graph.arch_endpoint_effects.push(ArchEndpointEffect {
            recipe: "arch/x86_64-pc/unit/mmakefile.src".into(),
            line: 12,
            endpoint: "unit-set-archincludes".into(),
            dependencies: Vec::new(),
            data: ArchEndpointEffectData::SetArchIncludes {
                mainmmake: "unit".into(),
                tag: "x86_64".into(),
                modname: "unit".into(),
                maindir: "rom/unit".into(),
                priority: 1,
                priority_token: "1".into(),
                include_dirs: Vec::new(),
                definitions: Vec::new(),
                arguments: Vec::new(),
                generated_file: "gen/rom/unit/include/.unit.includeflag.1.x86_64".into(),
                order_only_directory: "gen/rom/unit/include".into(),
            },
        });

        let selected = ["unit-fetch", "directory-owner", "unit-set-archincludes"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let outputs = collect(&graph, &selected);

        assert!(outputs.files.contains(&(
            "unit-fetch".into(),
            "${AROS_BUILD_DIR}/cache/unit.tar.gz".into()
        )));
        assert!(outputs.files.contains(&(
            "unit-fetch".into(),
            "${AROS_BUILD_DIR}/cache/unit.tgz".into()
        )));
        assert!(outputs
            .trees
            .contains(&("unit-fetch".into(), "${AROS_BUILD_DIR}/ports/unit".into())));
        assert!(outputs.trees.contains(&(
            "unit-fetch".into(),
            "${AROS_BUILD_DIR}/ports/unit/.aros-fetch".into()
        )));
        assert!(outputs.directories.contains(&(
            "directory-owner".into(),
            "${AROS_BUILD_DIR}/gen/setup/include".into()
        )));
        assert!(outputs.directories.contains(&(
            "unit-set-archincludes".into(),
            "gen/rom/unit/include".into()
        )));
    }

    #[test]
    fn ignores_unselected_and_empty_package_endpoints() {
        let mut graph = DependencyGraph::default();
        graph.packages.push(PackageDecl {
            file: "rom/package.mmakefile.src".into(),
            mmake: "empty-package".into(),
            output: "${AROS_BUILD_DIR}/SYS/empty.pkg".into(),
            members: Vec::new(),
            startup: None,
            uselibs: Vec::new(),
            is_kickstart: false,
            resolved: Vec::new(),
            arch: String::new(),
        });

        let outputs = collect(&graph, &BTreeSet::from(["empty-package".into()]));

        assert!(outputs.files.is_empty());
        assert!(outputs.trees.is_empty());
        assert!(outputs.directories.is_empty());
    }
}
