use crate::arch_sources::ArchSourceDecl;
use crate::ast::{MetaTargetRule, ModuleType, TargetDefinition};
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
    /// `%make_package` and `%link_kickstart` declarations.
    pub packages: Vec<crate::packages::PackageDecl>,
}

/// Splits an `arch/<cpu>-<platform>/...` path into its two components.
fn arch_of(dir: &std::path::Path) -> Option<(String, String)> {
    let s = dir.to_string_lossy().replace('\\', "/");
    let rest = s.strip_prefix("arch/")?;
    let first = rest.split('/').next()?;
    let (cpu, platform) = first.split_once('-')?;
    Some((cpu.to_owned(), platform.to_owned()))
}

/// Whether a candidate's directory can serve a declaration made under `ctx`.
///
/// The rule matches AROS_ARCH_SOURCE_DIRS in cmake/AROS.cmake: "all" is a
/// wildcard in either position and "native" is a platform shared by every
/// non-hosted target. A candidate outside arch/ is architecture-neutral and
/// always eligible.
///
/// This is what separates the six targets named `serial` across four
/// architectures, so a package declared in arch/i386-pc/boot picks
/// kernel-pc-i386-serial and not the Amiga or Unix one.
fn arch_compatible(candidate: Option<&(String, String)>, ctx: Option<&(String, String)>) -> bool {
    let Some((cand_cpu, cand_plat)) = candidate else {
        return true;
    };
    let Some((ctx_cpu, ctx_plat)) = ctx else {
        // A declaration outside arch/ describes the portable base, so an
        // architecture-specific candidate cannot be meant.
        return false;
    };
    (cand_cpu == "all" || cand_cpu == ctx_cpu)
        && (cand_plat == "all" || cand_plat == "native" || cand_plat == ctx_plat)
}

/// The package category a module type installs under, where the two agree.
///
/// Handlers and classes come out of the parser as `Custom`, since the tree
/// declares them with modtypes the target model does not separate; for those
/// the name alone has to do.
fn module_kind_name(t: &ModuleType) -> Option<&'static str> {
    match t {
        ModuleType::Library => Some("library"),
        ModuleType::Device => Some("device"),
        ModuleType::Resource => Some("resource"),
        ModuleType::Hidd => Some("hidd"),
        ModuleType::Datatype => Some("datatype"),
        ModuleType::Gadget => Some("gadget"),
        ModuleType::Mcc => Some("mcc"),
        _ => None,
    }
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

    pub fn add_packages(&mut self, decls: Vec<crate::packages::PackageDecl>) {
        self.packages.extend(decls);
    }

    /// Turns each package's `(kind, module name)` members into the mmake ids
    /// that build them.
    ///
    /// A declaration names `devs=ata`, meaning the file `ata.device`, and the
    /// target that builds it is `kernel-ata`. Only the module name is stated,
    /// so the lookup needs every mmakefile parsed, which is why this runs on
    /// the finished graph.
    ///
    /// Returns the members that resolved to nothing. Those matter: a package
    /// missing a module still builds, and the failure only appears when the
    /// system does not boot.
    /// Turns each target's `uselibs` names into the link-library targets that
    /// build them.
    ///
    /// `uselibs="debug"` means link against libdebug.a, which
    /// `%build_linklib libname=debug` builds under the mmake id
    /// `linklibs-debug`. Nothing resolved those names, so every builder linked
    /// an always-empty ARG_LIBS and no module linked against anything at all.
    ///
    /// Resolving here rather than in CMake also sidesteps a name clash:
    /// aros_link_libraries discards `debug`, `optimized` and `general` because
    /// target_link_libraries reads them as build-type keywords, and `debug` is
    /// a real AROS link library used by 20 declarations.
    ///
    /// Returns the names that matched no link library.
    pub fn resolve_use_libs(&mut self) -> Vec<String> {
        let mut by_name: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for (mmake, target) in &self.targets {
            if matches!(target.module_type, ModuleType::LinkLib) {
                by_name
                    .entry(target.target_name.as_str())
                    .or_default()
                    .push(mmake.as_str());
            }
        }

        let mut unresolved = Vec::new();
        let mut resolved: Vec<(String, Vec<String>)> = Vec::new();
        for (mmake, target) in &self.targets {
            let mut ids = Vec::new();
            for name in &target.use_libs {
                match by_name.get(name.as_str()) {
                    Some(c) if c.len() == 1 => {
                        let id = (*c[0]).to_owned();
                        if !ids.contains(&id) {
                            ids.push(id);
                        }
                    }
                    Some(c) => unresolved.push(format!(
                        "{}: {mmake} uselibs={name} is ambiguous ({})",
                        target.dir_path.display(),
                        c.join(", ")
                    )),
                    // Not every uselib is built here: some name a host library
                    // or a port that is not fetched. Reported, not guessed at.
                    None => unresolved.push(format!(
                        "{}: {mmake} uselibs={name} has no link library",
                        target.dir_path.display()
                    )),
                }
            }
            if !ids.is_empty() {
                resolved.push((mmake.clone(), ids));
            }
        }

        for (mmake, ids) in resolved {
            if let Some(t) = self.targets.get_mut(&mmake) {
                t.link_libs = ids;
            }
        }
        unresolved
    }

    pub fn resolve_packages(&mut self) -> Vec<String> {
        // Indexed by name and by (name, kind). A module name alone is
        // ambiguous often enough to matter: `ahci` is both kernel-ahci and a
        // SysExplorer plugin, and `serial` matches six targets across four
        // architectures. The category the declaration states resolves most of
        // those, so it is tried first.
        let mut by_name: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        let mut by_name_kind: std::collections::HashMap<(&str, &'static str), Vec<&str>> =
            std::collections::HashMap::new();
        for (mmake, target) in &self.targets {
            by_name
                .entry(target.target_name.as_str())
                .or_default()
                .push(mmake.as_str());
            if let Some(kind) = module_kind_name(&target.module_type) {
                by_name_kind
                    .entry((target.target_name.as_str(), kind))
                    .or_default()
                    .push(mmake.as_str());
            }
        }

        let mut unresolved = Vec::new();
        let mut resolved_all = Vec::new();
        for decl in &self.packages {
            let mut ids: Vec<String> = Vec::new();
            let decl_arch = arch_of(std::path::Path::new(
                decl.file.strip_suffix("/mmakefile.src").unwrap_or(&decl.file),
            ));

            // The startup module has to be linked first: the bootstrap takes
            // its entry point from the first executable section of the first
            // module (elfloader.c:662).
            let ordered = decl
                .startup
                .iter()
                .map(|s| ("resource".to_owned(), s.clone()))
                .chain(decl.members.iter().cloned());

            for (kind, name) in ordered {
                // Prefer the category-qualified match; fall back to the name
                // alone for kinds the target model does not distinguish, such
                // as handlers.
                let pool = by_name_kind
                    .get(&(name.as_str(), kind.as_str()))
                    .or_else(|| by_name.get(name.as_str()));
                let Some(pool) = pool else {
                    unresolved.push(format!(
                        "{}: {} {kind}={name} has no target",
                        decl.file, decl.mmake
                    ));
                    continue;
                };

                // A package holds modules, never programs. rom/filesys/CDVDFS
                // builds both a cdrom handler and a test program called cdrom;
                // only the first belongs in aros-fs.pkg.
                let pool: Vec<&str> = {
                    let modules: Vec<&str> = pool
                        .iter()
                        .copied()
                        .filter(|id| {
                            !matches!(
                                self.targets.get(*id).map(|t| &t.module_type),
                                Some(ModuleType::Program | ModuleType::ProgramGroup)
                            )
                        })
                        .collect();
                    if modules.is_empty() {
                        pool.clone()
                    } else {
                        modules
                    }
                };
                let pool = &pool;

                // Narrow an ambiguous name in two steps.
                //
                // The declaration's own #MM dependencies are checked first,
                // because they state the intent directly. arch/x86_64-pc/boot
                // lists kernel-pc-i386-serial and kernel-pc-i386-parallel, so
                // the x86_64 BSP deliberately reuses the i386 drivers, which
                // no directory-based rule could infer. The same list separates
                // kernel-fs-cdvdfs-cdrom from kernel-fs-cdvdfs, which sit in
                // one directory under the same module name.
                let mut eligible: Vec<&str> = if pool.len() == 1 {
                    pool.clone()
                } else if let Some(deps) = self.meta_targets.get(&decl.mmake) {
                    let named: Vec<&str> =
                        pool.iter().copied().filter(|id| deps.contains(*id)).collect();
                    if named.is_empty() {
                        pool.clone()
                    } else {
                        named
                    }
                } else {
                    pool.clone()
                };

                // Then by architecture, which handles the drivers that exist
                // once per platform under one name.
                if eligible.len() > 1 {
                    let narrowed: Vec<&str> = eligible
                        .iter()
                        .copied()
                        .filter(|id| {
                            let cand = self
                                .targets
                                .get(*id)
                                .and_then(|t| arch_of(&t.dir_path));
                            arch_compatible(cand.as_ref(), decl_arch.as_ref())
                        })
                        .collect();
                    if !narrowed.is_empty() {
                        eligible = narrowed;
                    }
                }

                match eligible.len() {
                    1 => {
                        let id = eligible[0].to_owned();
                        if !ids.contains(&id) {
                            ids.push(id);
                        }
                    }
                    0 => unresolved.push(format!(
                        "{}: {} {kind}={name} has no target for this architecture (candidates: {})",
                        decl.file,
                        decl.mmake,
                        pool.join(", ")
                    )),
                    _ => unresolved.push(format!(
                        "{}: {} {kind}={name} is ambiguous ({})",
                        decl.file,
                        decl.mmake,
                        eligible.join(", ")
                    )),
                }
            }
            resolved_all.push(ids);
        }

        for (decl, ids) in self.packages.iter_mut().zip(resolved_all) {
            decl.resolved = ids;
        }
        unresolved
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
