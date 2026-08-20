use crate::ast::{MetaTargetRule, TargetDefinition};
use crate::arch_sources::ArchSourceDecl;
use crate::copy_includes::{AdhocHeaderRule, CopyIncludesDecl};
use crate::fetch::FetchDecl;
use crate::includes::ArchIncludeDecl;
use aros_common::{ArosError, Result};
use std::collections::{HashMap, HashSet};

/// Dependency Graph for parallel target building and cycle detection.
#[derive(Debug, Default)]
pub struct DependencyGraph {
    pub targets: HashMap<String, TargetDefinition>,
    pub meta_targets: HashMap<String, HashSet<String>>,
    /// Every `%set_archincludes` declaration in the tree, keyed by `modname`.
    pub arch_decls: HashMap<String, Vec<ArchIncludeDecl>>,
    /// Every resolved `%copy_includes` declaration, deduplicated.
    pub copy_includes: Vec<CopyIncludesDecl>,
    /// Hand-written header staging rules found anywhere in the tree.
    pub adhoc_header_rules: Vec<AdhocHeaderRule>,
    /// `%build_archspecific` declarations, keyed by the target they extend.
    pub arch_sources: HashMap<String, Vec<ArchSourceDecl>>,
    /// `%fetch` declarations for third-party sources.
    pub fetches: Vec<FetchDecl>,
}

impl DependencyGraph {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_target(&mut self, target: TargetDefinition) {
        self.targets.insert(target.mmake_name.clone(), target);
    }

    pub fn add_fetches(&mut self, decls: Vec<FetchDecl>) {
        for d in decls {
            if !self.fetches.iter().any(|f| f.name == d.name) {
                self.fetches.push(d);
            }
        }
    }

    pub fn add_arch_sources(&mut self, decls: Vec<ArchSourceDecl>) {
        for d in decls {
            self.arch_sources
                .entry(d.mainmmake.clone())
                .or_default()
                .push(d);
        }
    }

    /// Attaches the architecture source overrides to their target.
    ///
    /// The join key is `mainmmake`, which the declaration states outright.
    pub fn resolve_arch_sources(&mut self) {
        for (name, decls) in &self.arch_sources {
            if let Some(target) = self.targets.get_mut(name) {
                for d in decls {
                    target
                        .arch_sources
                        .push((d.tag.clone(), d.dir.clone(), d.files.clone()));
                    // The declaring file's own include paths and flags belong to
                    // this target too, but only for its architecture.
                    for inc in &d.include_dirs {
                        let e = (d.tag.clone(), inc.clone());
                        if !target.arch_includes.contains(&e) {
                            target.arch_includes.push(e);
                        }
                    }
                    for def in &d.defines {
                        let e = (d.tag.clone(), def.clone());
                        if !target.arch_defines.contains(&e) {
                            target.arch_defines.push(e);
                        }
                    }
                    for opt in &d.compile_options {
                        let e = (d.tag.clone(), opt.clone());
                        if !target.arch_compile_options.contains(&e) {
                            target.arch_compile_options.push(e);
                        }
                    }
                }
            }
        }
    }

    pub fn add_adhoc_header_rules(&mut self, rules: Vec<AdhocHeaderRule>) {
        self.adhoc_header_rules.extend(rules);
    }

    pub fn add_copy_includes(&mut self, decls: Vec<CopyIncludesDecl>) {
        for decl in decls {
            let dup = self.copy_includes.iter().any(|d| {
                d.dest == decl.dest
                    && d.source_dir == decl.source_dir
                    && d.patterns == decl.patterns
                    && d.flatten == decl.flatten
            });
            if !dup {
                self.copy_includes.push(decl);
            }
        }
    }

    pub fn add_arch_decls(&mut self, decls: Vec<ArchIncludeDecl>) {
        for decl in decls {
            self.arch_decls
                .entry(decl.modname.clone())
                .or_default()
                .push(decl);
        }
    }

    /// Resolves each target's `%get_archincludes` requests against the
    /// declarations collected from the whole tree.
    ///
    /// The join key is `modname`, not `mainmmake`: `rom/exec` asks for both the
    /// `exec` and the `kernel` architecture includes, and those are declared
    /// under different `mainmmake` values. Results are ordered by `pri`, which
    /// is the order the Make build's wildcard glob produces.
    pub fn resolve_arch_includes(&mut self) {
        for decls in self.arch_decls.values_mut() {
            decls.sort_by(|a, b| a.pri.cmp(&b.pri).then_with(|| a.dir.cmp(&b.dir)));
        }

        for target in self.targets.values_mut() {
            let mut resolved: Vec<(String, String)> = Vec::new();
            for module in &target.arch_modules {
                if let Some(decls) = self.arch_decls.get(module) {
                    for decl in decls {
                        let entry = (decl.tag.clone(), decl.dir.clone());
                        if !resolved.contains(&entry) {
                            resolved.push(entry);
                        }
                    }
                }
            }
            // The target may already carry entries contributed by an
            // architecture make.opts; keep those, appended after the
            // %set_archincludes ones so the priority order is preserved.
            for entry in std::mem::take(&mut target.arch_includes) {
                if !resolved.contains(&entry) {
                    resolved.push(entry);
                }
            }
            target.arch_includes = resolved;
        }
    }

    pub fn add_meta_rule(&mut self, rule: MetaTargetRule) {
        self.meta_targets
            .entry(rule.name)
            .or_default()
            .extend(rule.dependencies);
    }

    /// Verifies that no cyclic dependencies exist in the graph.
    ///
    /// # Errors
    /// Returns an error if a dependency cycle is detected.
    pub fn validate_cycles(&self) -> Result<()> {
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();

        for target_name in self.targets.keys() {
            if !visited.contains(target_name) {
                self.check_cycle(target_name, &mut visited, &mut rec_stack)?;
            }
        }

        Ok(())
    }

    fn check_cycle(
        &self,
        node: &str,
        visited: &mut HashSet<String>,
        rec_stack: &mut HashSet<String>,
    ) -> Result<()> {
        visited.insert(node.to_string());
        rec_stack.insert(node.to_string());

        if let Some(target) = self.targets.get(node) {
            for dep in &target.dependencies {
                if visited.contains(dep) {
                    if rec_stack.contains(dep) {
                        return Err(ArosError::DependencyCycle {
                            target: format!("{node} -> {dep}"),
                        });
                    }
                } else if self.targets.contains_key(dep) {
                    self.check_cycle(dep, visited, rec_stack)?;
                }
            }
        }

        rec_stack.remove(node);
        Ok(())
    }
}
