//! Narrow correction for generated `uselibs` header prerequisites.

use super::{DependencyGraph, ModuleType};
use crate::ast::{InventoryTargetIdentity, ModuleMacroForm};
use std::collections::BTreeSet;

impl DependencyGraph {
    /// Removes only generated header aliases for a uniquely resolved plain
    /// `%build_linklib` provider.
    ///
    /// Full and ABI-only module macros emit `includes-<uselib>` prerequisites
    /// on their generated client-link archive even when the uselib provider is
    /// an ordinary archive with no generated include interface. Once typed
    /// link resolution proves that exact provider, this removes the phantom
    /// prerequisite while retaining every real header endpoint and every
    /// source-written edge.
    ///
    /// `concrete_endpoints` must include graph meta-target keys and Make
    /// providers as well as concrete producer endpoints. The caller supplies
    /// the context-resolved endpoint set used by native graph selection.
    ///
    /// Returns removed `owner -> dependency` pairs in deterministic order.
    pub fn omit_implicit_plain_linklib_headers(
        &mut self,
        concrete_endpoints: &BTreeSet<String>,
    ) -> Vec<String> {
        let declarations: Vec<InventoryTargetIdentity> = self
            .targets
            .values()
            .map(InventoryTargetIdentity::from)
            .chain(self.inventory_targets.iter().cloned())
            .collect();

        let mut removals = BTreeSet::new();
        for consumer in &declarations {
            if !matches!(
                consumer.module_macro,
                Some(ModuleMacroForm::Full | ModuleMacroForm::AbiOnly)
            ) {
                continue;
            }

            let owner = format!("{}-linklib", consumer.mmake_name);
            for use_lib in &consumer.use_libs {
                let dependency = format!("includes-{use_lib}");
                let matching_providers: Vec<_> = consumer
                    .link_libs
                    .iter()
                    .flat_map(|archive| {
                        declarations.iter().filter(move |provider| {
                            provider.mmake_name.as_str() == archive.as_str()
                                && provider.target_name.as_str() == use_lib.as_str()
                        })
                    })
                    .collect();
                let ordinary_archives = self
                    .source_archives
                    .iter()
                    .filter(|archive| {
                        consumer.link_libs.contains(&archive.provider_target())
                            && archive.archive_basename() == format!("lib{use_lib}.a")
                    })
                    .count();
                let plain_linklib = match matching_providers.as_slice() {
                    [provider] if ordinary_archives == 0 => {
                        provider.module_type == ModuleType::LinkLib
                            && provider.module_macro.is_none()
                            && !provider.genmodule_abi
                            && !provider.genmodule_only
                            && provider.genmodule_linklibs.is_none()
                    }
                    [] if ordinary_archives == 1 => true,
                    _ => false,
                };
                if !plain_linklib {
                    continue;
                }

                // A same-named alias may be the consumer's own genmodule
                // interface. It is real even if a malformed caller omitted it
                // from the supplied endpoint set.
                if dependency == format!("includes-{}", consumer.target_name) {
                    continue;
                }

                let known_endpoint = concrete_endpoints.contains(&dependency)
                    || self.meta_targets.contains_key(&dependency)
                    || self.make_meta_providers.contains(&dependency)
                    || self.targets.contains_key(&dependency)
                    || self
                        .inventory_targets
                        .iter()
                        .any(|target| target.mmake_name == dependency);
                if known_endpoint
                    // Cycle flattening may move a handwritten prerequisite
                    // to another owner. Its source provenance still forbids
                    // treating that endpoint as an implicit absent alias.
                    || self
                        .explicit_meta_edges
                        .iter()
                        .any(|(_, prerequisite)| prerequisite == &dependency)
                    || !self
                        .meta_targets
                        .get(&owner)
                        .is_some_and(|dependencies| dependencies.contains(&dependency))
                {
                    continue;
                }

                removals.insert((owner.clone(), dependency));
            }
        }

        let mut removed = Vec::new();
        for (owner, dependency) in removals {
            if self
                .meta_targets
                .get_mut(&owner)
                .is_some_and(|dependencies| dependencies.remove(&dependency))
            {
                removed.push(format!("{owner} -> {dependency}"));
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::MetaTargetRule;
    use crate::dirs::DirVars;
    use crate::testing::TempTree;
    use crate::{parse_mmakefile_with_dirs_and_context, TargetContext};
    use std::fmt::Write;
    use std::fs;

    fn fixture(source: &str, source_files: &[&str]) -> (TempTree, DependencyGraph) {
        let tree = TempTree::new();
        for stem in source_files {
            fs::write(tree.0.join(format!("{stem}.c")), "int fixture;\n").unwrap();
        }
        let file = tree.0.join("mmakefile.src");
        fs::write(&file, source).unwrap();
        let parsed = parse_mmakefile_with_dirs_and_context(
            &file,
            &tree.0,
            &DirVars::load(&tree.0),
            &TargetContext::default(),
        )
        .unwrap();
        assert!(
            parsed.skipped_programs.is_empty(),
            "{:#?}",
            parsed.skipped_programs
        );

        let mut graph = DependencyGraph::new();
        for target in parsed.targets {
            graph.add_target(target);
        }
        for rule in parsed.meta_rules {
            graph.add_meta_rule(rule);
        }
        for rule in parsed.explicit_meta_rules {
            graph.add_explicit_meta_rule(rule);
        }
        graph.make_meta_providers.extend(parsed.make_meta_providers);
        (tree, graph)
    }

    fn plain_linklib_source(consumers: &str) -> String {
        format!(
            "{consumers}\
             %build_linklib mmake=plain-archive libname=plain files=archive\n"
        )
    }

    #[test]
    fn full_and_abi_only_remove_only_the_plain_library_header_alias() {
        let source = plain_linklib_source(
            "%build_module mmake=full-id modname=full-module modtype=library files=full uselibs=plain\n\
             %build_module_abi mmake=abi-id modname=abi-module modtype=library uselibs=plain\n",
        );
        let (_tree, mut graph) = fixture(&source, &["full", "archive"]);
        assert!(graph.resolve_use_libs().is_empty());

        assert!(graph.meta_targets["full-id-linklib"].contains("includes-plain"));
        assert!(graph.meta_targets["abi-id-linklib"].contains("includes-plain"));
        assert_eq!(graph.targets["full-id"].link_libs, ["plain-archive"]);
        assert_eq!(graph.targets["abi-id"].link_libs, ["plain-archive"]);

        let removed = graph.omit_implicit_plain_linklib_headers(&BTreeSet::new());
        assert_eq!(
            removed,
            [
                "abi-id-linklib -> includes-plain",
                "full-id-linklib -> includes-plain"
            ]
        );
        for owner in ["full-id-linklib", "abi-id-linklib"] {
            assert!(!graph.meta_targets[owner].contains("includes-plain"));
        }
        assert!(graph.meta_targets["full-id-linklib"].contains("full-id-includes"));
        assert!(graph.meta_targets["abi-id-linklib"].contains("abi-id-includes"));

        // Archive and KOBJ/runtime link edges remain bound to the provider.
        for owner in ["full-id", "full-id-kobj", "abi-id"] {
            assert!(graph.meta_targets[owner].contains("plain-archive"));
            assert!(!graph.meta_targets[owner].contains("linklibs-plain"));
        }
    }

    #[test]
    fn inventory_identity_can_supply_the_resolved_consumer_and_provider() {
        let source = plain_linklib_source(
            "%build_module mmake=inventory-consumer modname=consumer modtype=library files=consumer uselibs=plain\n",
        );
        let (_tree, mut graph) = fixture(&source, &["consumer", "archive"]);
        let consumer = graph.targets.remove("inventory-consumer").unwrap();
        graph.inventory_targets.push((&consumer).into());

        assert!(graph
            .resolve_inventory_link_edges(&TargetContext::default())
            .unwrap()
            .is_empty());
        assert_eq!(graph.inventory_targets[0].link_libs, ["plain-archive"]);
        assert_eq!(
            graph.omit_implicit_plain_linklib_headers(&BTreeSet::new()),
            ["inventory-consumer-linklib -> includes-plain"]
        );
    }

    #[test]
    fn bound_ordinary_archive_omits_only_its_implicit_missing_header_alias() {
        let source = "%build_module mmake=consumer modname=consumer modtype=library files=consumer uselibs=plain\n";
        let (_tree, mut graph) = fixture(source, &["consumer"]);
        graph
            .source_archives
            .push(crate::source_archive_binding::BoundSourceArchive {
                declaration: crate::source_archive_rules::SourceArchiveDecl {
                    owner: "ordinary-plain".into(),
                    file: "compiler/plain/mmakefile.src".into(),
                    line: 9,
                    owner_line: 3,
                    output: "${AROS_BUILD_DIR}/SYS/Developer/lib/libplain.a".into(),
                    members: crate::source_archive_rules::ArchiveMembers::Exact(vec![
                        "${AROS_BUILD_DIR}/gen/plain.o".into(),
                    ]),
                },
                command: crate::source_archive_command::SourceArchiveCommand {
                    flags: vec!["cr".into()],
                },
                members: vec!["${AROS_BUILD_DIR}/gen/plain.o".into()],
                compile_groups: Vec::new(),
                producer_owners: BTreeSet::new(),
            });
        assert!(graph.resolve_use_libs().is_empty());
        assert_eq!(
            graph.targets["consumer"].link_libs,
            ["ordinary-plain-archive"]
        );
        assert_eq!(
            graph.omit_implicit_plain_linklib_headers(&BTreeSet::new()),
            ["consumer-linklib -> includes-plain"]
        );

        graph.add_explicit_meta_rule(MetaTargetRule {
            name: "consumer-linklib".into(),
            dependencies: vec!["includes-plain".into()],
        });
        assert!(graph
            .omit_implicit_plain_linklib_headers(&BTreeSet::new())
            .is_empty());
        assert!(graph.meta_targets["consumer-linklib"].contains("includes-plain"));
    }

    #[test]
    fn exact_punctuated_uselibs_are_matched_without_rewriting_names() {
        let names = [
            "romhack",
            "stdc.static",
            "fatbpb",
            "loadseg",
            "coolimagesstatic",
            "z-nogzip.static",
        ];
        let mut source = format!(
            "%build_module mmake=consumer modname=consumer modtype=library files=consumer uselibs=\"{}\"\n",
            names.join(" ")
        );
        let mut stems = vec!["consumer".to_owned()];
        for (index, name) in names.iter().enumerate() {
            let stem = format!("archive-{index}");
            stems.push(stem.clone());
            writeln!(
                source,
                "%build_linklib mmake=plain-provider-{index} libname={name} files={stem}"
            )
            .unwrap();
        }

        let source_files: Vec<_> = stems.iter().map(String::as_str).collect();
        let (_tree, mut graph) = fixture(&source, &source_files);
        assert!(graph.resolve_use_libs().is_empty());

        let removed = graph.omit_implicit_plain_linklib_headers(&BTreeSet::new());
        assert_eq!(removed.len(), names.len());
        for name in names {
            assert!(removed.contains(&format!("consumer-linklib -> includes-{name}")));
            assert!(!graph.meta_targets["consumer-linklib"].contains(&format!("includes-{name}")));
        }
        assert!(graph.meta_targets["consumer-linklib"].contains("consumer-includes"));
    }

    #[test]
    fn explicit_duplicate_and_known_header_endpoints_are_preserved() {
        for endpoint_mode in ["passed", "meta-key", "make-provider"] {
            let source = plain_linklib_source(
                "%build_module mmake=consumer modname=consumer modtype=library files=consumer uselibs=plain\n",
            );
            let (_tree, mut graph) = fixture(&source, &["consumer", "archive"]);
            assert!(graph.resolve_use_libs().is_empty());

            match endpoint_mode {
                "passed" => {}
                "meta-key" => graph.add_meta_rule(MetaTargetRule {
                    name: "includes-plain".to_owned(),
                    dependencies: Vec::new(),
                }),
                "make-provider" => {
                    graph
                        .make_meta_providers
                        .insert("includes-plain".to_owned());
                }
                _ => unreachable!(),
            }
            let endpoints = if endpoint_mode == "passed" {
                BTreeSet::from(["includes-plain".to_owned()])
            } else {
                BTreeSet::new()
            };
            assert!(graph
                .omit_implicit_plain_linklib_headers(&endpoints)
                .is_empty());
            assert!(graph.meta_targets["consumer-linklib"].contains("includes-plain"));
        }

        let source = plain_linklib_source(
            "%build_module mmake=consumer modname=consumer modtype=library files=consumer uselibs=plain\n",
        );
        let (_tree, mut graph) = fixture(&source, &["consumer", "archive"]);
        assert!(graph.resolve_use_libs().is_empty());
        graph.add_explicit_meta_rule(MetaTargetRule {
            name: "consumer-linklib".to_owned(),
            dependencies: vec!["includes-plain".to_owned()],
        });

        assert!(graph
            .omit_implicit_plain_linklib_headers(&BTreeSet::new())
            .is_empty());
        assert!(graph.meta_targets["consumer-linklib"].contains("includes-plain"));
    }

    #[test]
    fn handwritten_dependency_is_preserved_after_owner_projection() {
        let source = plain_linklib_source(
            "%build_module mmake=consumer modname=consumer modtype=library files=consumer uselibs=plain\n",
        );
        let (_tree, mut graph) = fixture(&source, &["consumer", "archive"]);
        assert!(graph.resolve_use_libs().is_empty());
        graph.add_explicit_meta_rule(MetaTargetRule {
            name: "source-owner".into(),
            dependencies: vec!["includes-plain".into()],
        });
        assert!(graph
            .omit_implicit_plain_linklib_headers(&BTreeSet::new())
            .is_empty());
        assert!(graph.meta_targets["consumer-linklib"].contains("includes-plain"));
    }

    #[test]
    fn unresolved_and_ambiguous_libraries_keep_the_header_edge() {
        let source = "%build_module mmake=consumer modname=consumer modtype=library files=consumer uselibs=plain\n";
        let (_tree, mut missing) = fixture(source, &["consumer"]);
        assert_eq!(missing.resolve_use_libs().len(), 1);
        assert!(missing
            .omit_implicit_plain_linklib_headers(&BTreeSet::new())
            .is_empty());
        assert!(missing.meta_targets["consumer-linklib"].contains("includes-plain"));

        let source = format!(
            "{source}\
             %build_linklib mmake=plain-one libname=plain files=one\n\
             %build_linklib mmake=plain-two libname=plain files=two\n"
        );
        let (_tree, mut ambiguous) = fixture(&source, &["consumer", "one", "two"]);
        assert_eq!(ambiguous.resolve_use_libs().len(), 1);
        assert!(ambiguous
            .omit_implicit_plain_linklib_headers(&BTreeSet::new())
            .is_empty());
        assert!(ambiguous.meta_targets["consumer-linklib"].contains("includes-plain"));
    }

    #[test]
    fn runtime_only_simple_and_nonplain_providers_are_out_of_scope() {
        let source = plain_linklib_source(
            "%build_module_library mmake=runtime modname=runtime modtype=library files=runtime uselibs=plain\n\
             %build_module_simple mmake=simple modname=simple modtype=library files=simple uselibs=plain\n\
             %build_module mmake=full modname=full modtype=library files=full uselibs=plain\n",
        );
        let (_tree, mut graph) = fixture(&source, &["runtime", "simple", "full", "archive"]);
        assert!(graph.resolve_use_libs().is_empty());
        assert_eq!(
            graph.targets["runtime"].module_macro,
            Some(ModuleMacroForm::RuntimeOnly)
        );
        assert_eq!(
            graph.targets["simple"].module_macro,
            Some(ModuleMacroForm::Simple)
        );

        // Exercise the macro-form guard even if a malformed or hand-authored
        // rule happens to use the same spelling as a generated linklib owner.
        graph.add_meta_rule(MetaTargetRule {
            name: "runtime-linklib".to_owned(),
            dependencies: vec!["includes-plain".to_owned()],
        });
        graph.add_meta_rule(MetaTargetRule {
            name: "simple-linklib".to_owned(),
            dependencies: vec!["includes-plain".to_owned()],
        });

        let removed = graph.omit_implicit_plain_linklib_headers(&BTreeSet::new());
        assert_eq!(removed, ["full-linklib -> includes-plain"]);
        assert!(graph.meta_targets["runtime-linklib"].contains("includes-plain"));
        assert!(graph.meta_targets["simple-linklib"].contains("includes-plain"));
        assert!(!graph.meta_targets["full-linklib"].contains("includes-plain"));
    }

    #[test]
    fn nonplain_or_nonunique_provider_does_not_remove_the_header_edge() {
        let source = plain_linklib_source(
            "%build_module mmake=consumer modname=consumer modtype=library files=consumer uselibs=plain\n",
        );
        for provider_mode in ["module", "genmodule", "interface", "duplicate"] {
            let (_tree, mut graph) = fixture(&source, &["consumer", "archive"]);
            assert!(graph.resolve_use_libs().is_empty());
            match provider_mode {
                "module" => {
                    graph.targets.get_mut("plain-archive").unwrap().module_type =
                        ModuleType::Library;
                }
                "genmodule" => {
                    graph
                        .targets
                        .get_mut("plain-archive")
                        .unwrap()
                        .genmodule_abi = true;
                }
                "interface" => {
                    graph
                        .targets
                        .get_mut("plain-archive")
                        .unwrap()
                        .genmodule_linklibs = Some(crate::ast::GenmoduleLinklibs::default());
                }
                "duplicate" => {
                    let mut duplicate = graph.targets["plain-archive"].clone();
                    duplicate.mmake_name = "second-plain-archive".to_owned();
                    graph.add_target(duplicate);
                    graph
                        .targets
                        .get_mut("consumer")
                        .unwrap()
                        .link_libs
                        .push("second-plain-archive".to_owned());
                }
                _ => unreachable!(),
            }

            assert!(graph
                .omit_implicit_plain_linklib_headers(&BTreeSet::new())
                .is_empty());
            assert!(graph.meta_targets["consumer-linklib"].contains("includes-plain"));
        }
    }

    #[test]
    fn same_name_consumer_interface_is_not_removed() {
        let source = plain_linklib_source(
            "%build_module mmake=consumer modname=plain modtype=library files=consumer uselibs=plain\n",
        );
        let (_tree, mut graph) = fixture(&source, &["consumer", "archive"]);
        assert!(graph.resolve_use_libs().is_empty());
        assert!(graph
            .omit_implicit_plain_linklib_headers(&BTreeSet::new())
            .is_empty());
        assert!(graph.meta_targets["consumer-linklib"].contains("includes-plain"));
    }
}
