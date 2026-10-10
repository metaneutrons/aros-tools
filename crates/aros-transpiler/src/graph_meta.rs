//! Include, meta-target, and cycle resolution.

use super::{
    AdhocHeaderRule, ArchIncludeDecl, ArosError, BTreeSet, CopyDirectoryDecl, CopyIncludesDecl,
    DependencyGraph, HashMap, HashSet, MetaTargetRule, Result,
};

impl DependencyGraph {
    pub fn add_adhoc_header_rules(&mut self, rules: Vec<AdhocHeaderRule>) {
        self.adhoc_header_rules.extend(rules);
    }

    pub fn add_copy_includes(&mut self, decls: Vec<CopyIncludesDecl>) {
        for decl in decls {
            let dup = self.copy_includes.iter().any(|d| {
                d.name == decl.name
                    && d.dest == decl.dest
                    && d.source_dir == decl.source_dir
                    && d.patterns == decl.patterns
                    && d.excludes == decl.excludes
                    && d.flatten == decl.flatten
            });
            if !dup {
                self.copy_includes.push(decl);
            }
        }
    }

    /// Adds copy targets while preserving their unique MetaMake owner.
    ///
    /// A target name can only map to one CMake custom target.  If two source
    /// declarations claim it with different paths, the conflict is reported
    /// to the caller and neither guessed copy is introduced.
    pub fn add_copy_directories(&mut self, decls: Vec<CopyDirectoryDecl>) -> Vec<String> {
        let mut skipped = Vec::new();
        for declaration in decls {
            let Some(existing) = self
                .copy_directories
                .iter()
                .find(|existing| existing.name == declaration.name)
            else {
                self.copy_directories.push(declaration);
                continue;
            };
            if existing.source != declaration.source
                || existing.destination != declaration.destination
            {
                skipped.push(format!(
                    "{}:{}: %copy_dir_recursive {} conflicts with {}:{} ({} -> {} versus {} -> {})",
                    declaration.file,
                    declaration.line,
                    declaration.name,
                    existing.file,
                    existing.line,
                    existing.source,
                    existing.destination,
                    declaration.source,
                    declaration.destination
                ));
            }
        }
        skipped
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
                        if !decl.dir.is_empty() {
                            let entry = (decl.tag.clone(), decl.dir.clone());
                            if !resolved.contains(&entry) {
                                resolved.push(entry);
                            }
                        }
                        // Make carries these inside TARGET_<X>_INCLUDES, so a
                        // consumer that asks for the includes compiles with
                        // them, under the declaration's architecture tag.
                        for define in &decl.defines {
                            let entry = (decl.tag.clone(), define.clone());
                            if !target.arch_defines.contains(&entry) {
                                target.arch_defines.push(entry);
                            }
                        }
                        for option in &decl.compile_options {
                            let entry = (decl.tag.clone(), option.clone());
                            if !target.arch_compile_options.contains(&entry) {
                                target.arch_compile_options.push(entry);
                            }
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

    /// Retain handwritten prerequisite provenance before unioning aliases.
    pub fn add_explicit_meta_rule(&mut self, rule: MetaTargetRule) {
        self.explicit_meta_edges.extend(
            rule.dependencies
                .iter()
                .map(|dependency| (rule.name.clone(), dependency.clone())),
        );
        self.add_meta_rule(rule);
    }

    /// Flattens strongly connected groups of pure #MM targets.
    ///
    /// GNU Make tolerates a circular prerequisite by dropping an edge during
    /// traversal. CMake rejects the same utility-target cycle at generate
    /// time. A cycle of phony aggregators denotes a shared prerequisite
    /// closure, so every member receives the union of the group's external
    /// dependencies and all internal edges are removed. The result is
    /// deterministic and preserves what building either entry point prepares.
    ///
    /// A recursive copy is an action, not a phony alias. Lift its body into a
    /// private endpoint before collapsing the aliases, and include that body
    /// in every member's shared closure. This preserves both aggregate and
    /// direct source-owner invocations without introducing utility cycles.
    /// Other producer families retain their existing normalization semantics.
    ///
    /// Reports each flattened component and each preserved copy. Rejects private-name
    /// collisions and copy prerequisites inside the component: lifting such
    /// a prerequisite would reintroduce a cycle or silently drop ordering.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when a private execution endpoint collides
    /// with a declared name or a copy prerequisite would re-enter its component.
    pub fn flatten_meta_cycles(&mut self) -> Result<Vec<String>> {
        fn visit(
            node: &str,
            graph: &HashMap<String, HashSet<String>>,
            nodes: &HashSet<String>,
            seen: &mut HashSet<String>,
            order: &mut Vec<String>,
        ) {
            if !seen.insert(node.to_owned()) {
                return;
            }
            let mut next: Vec<&str> = graph
                .get(node)
                .into_iter()
                .flatten()
                .map(String::as_str)
                .filter(|dep| nodes.contains(*dep))
                .collect();
            next.sort_unstable();
            for dep in next {
                visit(dep, graph, nodes, seen, order);
            }
            order.push(node.to_owned());
        }

        fn visit_reverse(
            node: &str,
            reverse: &HashMap<String, Vec<String>>,
            seen: &mut HashSet<String>,
            component: &mut Vec<String>,
        ) {
            if !seen.insert(node.to_owned()) {
                return;
            }
            component.push(node.to_owned());
            if let Some(next) = reverse.get(node) {
                for dep in next {
                    visit_reverse(dep, reverse, seen, component);
                }
            }
        }

        let nodes: HashSet<String> = self.meta_targets.keys().cloned().collect();
        let mut names: Vec<String> = nodes.iter().cloned().collect();
        names.sort_unstable();
        let mut seen = HashSet::new();
        let mut order = Vec::with_capacity(names.len());
        for name in &names {
            visit(name, &self.meta_targets, &nodes, &mut seen, &mut order);
        }

        let mut reverse: HashMap<String, Vec<String>> = HashMap::new();
        for (from, deps) in &self.meta_targets {
            for to in deps.iter().filter(|dep| nodes.contains(*dep)) {
                reverse.entry(to.clone()).or_default().push(from.clone());
            }
        }
        for incoming in reverse.values_mut() {
            incoming.sort_unstable();
            incoming.dedup();
        }

        seen.clear();
        let mut components = Vec::new();
        while let Some(name) = order.pop() {
            if seen.contains(&name) {
                continue;
            }
            let mut component = Vec::new();
            visit_reverse(&name, &reverse, &mut seen, &mut component);
            component.sort_unstable();
            let cyclic = component.len() > 1
                || self
                    .meta_targets
                    .get(&component[0])
                    .is_some_and(|deps| deps.contains(&component[0]));
            if cyclic {
                components.push(component);
            }
        }

        components.sort();
        let mut occupied = crate::generator::concrete_endpoint_names(self);
        occupied.extend(self.meta_targets.keys().cloned());
        // Missing source prerequisites also reserve their spelling. Creating
        // a private action must not turn such a reference into a valid edge.
        occupied.extend(self.meta_targets.values().flatten().cloned());
        occupied.extend(
            self.explicit_meta_edges
                .iter()
                .flat_map(|(owner, dependency)| [owner.clone(), dependency.clone()]),
        );
        occupied.extend(self.make_meta_providers.iter().cloned());
        occupied.extend(
            self.targets
                .values()
                .flat_map(|target| target.dependencies.iter().cloned()),
        );
        occupied.extend(
            self.copy_directories
                .iter()
                .flat_map(|copy| copy.dependencies.iter().cloned()),
        );
        let mut reports = Vec::new();
        for component in components {
            let members: HashSet<&str> = component.iter().map(String::as_str).collect();
            let mut lifted = Vec::new();
            for (index, declaration) in self.copy_directories.iter().enumerate() {
                if !members.contains(declaration.name.as_str()) {
                    continue;
                }
                if let Some(dependency) = declaration
                    .dependencies
                    .iter()
                    .find(|dependency| members.contains(dependency.as_str()))
                {
                    return Err(ArosError::Configuration {
                        file: declaration.file.clone(),
                        message: format!(
                            "{}:{}: recursive copy {} has an action prerequisite {dependency} inside its MetaMake cycle; cannot preserve ordering by alias normalization",
                            declaration.file, declaration.line, declaration.name
                        ),
                    });
                }
                let action = super::private_meta_copy_action(&declaration.name);
                if !occupied.insert(action.clone()) {
                    return Err(ArosError::Configuration {
                        file: declaration.file.clone(),
                        message: format!(
                            "{}:{}: recursive copy {} cannot use occupied internal endpoint {action}",
                            declaration.file, declaration.line, declaration.name
                        ),
                    });
                }
                lifted.push((index, declaration.name.clone(), action));
            }
            let mut external = BTreeSet::new();
            for name in &component {
                if let Some(deps) = self.meta_targets.get(name) {
                    external.extend(
                        deps.iter()
                            .filter(|dep| !members.contains(dep.as_str()))
                            .cloned(),
                    );
                }
            }
            // Keep the declaration's source, destination and independently
            // resolved fetch prerequisites unchanged. Only its execution
            // identity is private; all original names remain public aliases.
            for (index, owner, action) in &lifted {
                self.copy_directories[*index].name.clone_from(action);
                self.lifted_copy_aliases
                    .insert(owner.clone(), action.clone());
                // Unlike siblings on a phony alias, these edges order the
                // action itself after the shared external prerequisites.
                self.meta_targets
                    .insert(action.clone(), external.iter().cloned().collect());
            }
            external.extend(lifted.iter().map(|(_, _, action)| action.clone()));
            for name in &component {
                if let Some(deps) = self.meta_targets.get_mut(name) {
                    deps.retain(|dep| !members.contains(dep.as_str()));
                    deps.extend(external.iter().cloned());
                }
            }
            let kind = if !lifted.is_empty()
                || component.iter().any(|name| {
                    self.targets.contains_key(name)
                        || self.icon_targets.contains_key(name)
                        || self
                            .external_cmake
                            .iter()
                            .any(|declaration| declaration.mmake_name == *name)
                        || self
                            .configure_builds
                            .iter()
                            .any(|declaration| declaration.mmake_name == *name)
                        || self
                            .grub_builds
                            .iter()
                            .any(|declaration| declaration.mmake_name == *name)
                }) {
                "build/meta"
            } else {
                "meta"
            };
            reports.push(format!(
                "flattened {kind} cycle [{}]; shared external dependencies [{}]",
                component.join(" -> "),
                external.into_iter().collect::<Vec<_>>().join(", ")
            ));
            for (_, owner, action) in lifted {
                reports.push(format!(
                    "preserved recursive copy action {owner} as {action}; public cycle aliases retain its prerequisites"
                ));
            }
        }
        Ok(reports)
    }

    /// Return only a normalization-created binding with its exact copy body
    /// and still-connected public alias. The private-name pattern is not proof.
    pub(super) fn lifted_copy_action(&self, owner: &str) -> Option<&str> {
        let action = self.lifted_copy_aliases.get(owner)?;
        (action == &super::private_meta_copy_action(owner)
            && self
                .copy_directories
                .iter()
                .filter(|declaration| declaration.name == *action)
                .count()
                == 1
            && self
                .meta_targets
                .get(owner)
                .is_some_and(|dependencies| dependencies.contains(action)))
        .then_some(action.as_str())
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
