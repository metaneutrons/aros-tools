//! `%make_package` and `%link_kickstart`.
//!
//! These two decide what a bootable system consists of. `%make_package` lists
//! the modules that go into a PKG container, `%link_kickstart` links the few
//! that have to be one relocatable ELF because the bootstrap takes its entry
//! point from the first executable section of the first module it loads
//! (`elfloader.c:662`).
//!
//! Both name their members by module name and category, not by mmake target:
//! `devs=ata ahci` means `$(AROS_DEVS)/ata.device` and `ahci.device`. The
//! mapping from a module name to the target that builds it needs every
//! mmakefile parsed, so it happens in the dependency graph rather than here.
//!
//! Until this was transpiled, cmake/Kickstart.cmake carried the lists by hand,
//! and they were incomplete: the base package was missing dos64, both
//! filesystem handlers, all five base hidds and debug.

use std::path::Path;

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_list, MakeExprContext};
use crate::make_vars::{collect_vars_impl, ConditionalTruth, VarScope};

use serde::{Deserialize, Serialize};

/// A package member after its build target has been identified.
///
/// Keeping the target and archive basename in one value prevents parallel
/// lists from drifting when a declaration contains a duplicate or one member
/// cannot be resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPackageMember {
    /// mmake id of the target that produces the module.
    pub target: String,
    /// Basename stored in the PKG container, matching `%make_package`.
    pub runtime_name: String,
}

/// One `%make_package` or `%link_kickstart` declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageDecl {
    /// The declaring mmakefile, for reporting.
    pub file: String,
    /// mmake target name.
    pub mmake: String,
    /// Output path, already rendered as a CMake expression.
    pub output: String,
    /// Members as `(category, module name)`, in declaration order.
    pub members: Vec<(String, String)>,
    /// `%link_kickstart` only: the object that must come first, since it
    /// supplies the entry point.
    pub startup: Option<String>,
    /// `%link_kickstart` only: static libraries to link against.
    pub uselibs: Vec<String>,
    /// True for `%link_kickstart`, false for `%make_package`.
    pub is_kickstart: bool,
    /// Target/runtime-name pairs filled in by the graph once every mmakefile
    /// has been parsed. Startup comes first where one is declared. Duplicate
    /// producer targets are removed as GNU Make removes duplicates from `$^`.
    #[serde(default)]
    pub resolved: Vec<ResolvedPackageMember>,
    /// The architecture this declaration belongs to, as `<cpu>-<platform>`,
    /// taken from its directory. Empty for a portable declaration.
    ///
    /// Needed because the output path is architecture-relative: three
    /// architectures declare `$(AROSARCHDIR)/aros-bsp.pkg`, which all render
    /// to the same file. Only the configured architecture may build it, and
    /// CMake decides that, so the transpiler stays target-agnostic.
    #[serde(default)]
    pub arch: String,
}

/// Categories a package declaration can name, with the module kind each maps
/// to. `arch_` variants install into the architecture-specific tree but name
/// modules the same way.
const CATEGORIES: [(&str, &str); 12] = [
    ("classes", "class"),
    ("devs", "device"),
    ("handlers", "handler"),
    ("hidds", "hidd"),
    ("libs", "library"),
    ("res", "resource"),
    ("arch_classes", "class"),
    ("arch_devs", "device"),
    ("arch_handlers", "handler"),
    ("arch_hidds", "hidd"),
    ("arch_libs", "library"),
    ("arch_res", "resource"),
];

/// The basename that the reference `%make_package` places in a PKG.
///
/// Handlers are the one historical exception to the ordinary `name.kind`
/// spelling (`ram-handler`, not `ram.handler`). Custom suffixes carried by the
/// sole `misc=` declaration use the ordinary branch (`serial.logger`).
pub(crate) fn runtime_name(kind: &str, name: &str) -> String {
    // Typed lists may retain their install subdirectory (`USB/hub.class` is
    // sourced below Classes/USB), while PKG's historical tool is invoked with
    // `--basename`. Only the last path component belongs in the container.
    let name = name.rsplit('/').next().unwrap_or(name);
    if kind == "handler" {
        format!("{name}-handler")
    } else {
        format!("{name}.{kind}")
    }
}

/// The `<cpu>-<platform>` an mmakefile belongs to, from its path.
fn declaring_arch(rel_dir: &Path) -> String {
    let s = rel_dir.to_string_lossy().replace('\\', "/");
    let Some(rest) = s.strip_prefix("arch/") else {
        return String::new();
    };
    rest.split('/').next().unwrap_or_default().to_owned()
}

/// Maps a Make variable an output path is built from.
///
/// Every variable the tree uses for this is listed, not just the ones the
/// currently configured architectures need: the aim is to build every
/// architecture through CMake, and a package whose path silently fails to map
/// is a package that never gets built.
fn map_output_var(name: &str) -> Option<&'static str> {
    match name {
        // config/make.cfg.in:17,97-99. This build names the system directory
        // SYS rather than the reference tree's AROS, but preserves the same
        // nesting: TARGETDIR/SYS/boot/<platform>.
        "TARGETDIR" => Some("${AROS_BUILD_DIR}"),
        "AROSDIR" => Some("${AROS_SYS_DIR}"),
        "AROS_BOOT" => Some("${AROS_BOOT_DIR}"),
        "AROSARCHDIR" => Some("${AROS_BOOT_ARCH_DIR}"),
        // Target parameters, so a rom image can be named after its CPU.
        "AROS_TARGET_CPU" | "CPU" => Some("${AROS_TARGET_CPU}"),
        "AROS_TARGET_ARCH" | "ARCH" => Some("${AROS_TARGET_PLATFORM}"),
        "AROS_TARGET_FAMILY" | "FAMILY" => Some("${AROS_TARGET_FAMILY}"),
        _ => None,
    }
}

/// Converts an already-suffixed `misc=` path into the same `(kind, name)`
/// representation used by the typed package arguments.
fn misc_member(path: &str) -> Option<(String, String)> {
    let basename = path.rsplit('/').next()?.trim_matches('"');
    if let Some(name) = basename.strip_suffix("-handler") {
        return (!name.is_empty()).then(|| ("handler".to_owned(), name.to_owned()));
    }
    let (name, kind) = basename.rsplit_once('.')?;
    (!name.is_empty() && !kind.is_empty()).then(|| (kind.to_owned(), name.to_owned()))
}

/// Whether a resolved `misc=` path has a concrete member basename.
///
/// AROS_DEVS resolves to `${AROS_BUILD_DIR}/SYS/Devs` in this build. That
/// trusted CMake directory may remain deferred because PKG records only the
/// basename. Other Make/CMake interpolation or shell/list syntax in the
/// directory or basename stays unsupported.
fn misc_path_is_concrete(path: &str) -> bool {
    let (directory, basename) = path.rsplit_once('/').unwrap_or(("", path));
    if basename.is_empty() || basename.contains(['$', ';', '(', ')']) {
        return false;
    }
    if directory.contains([';', '(', ')']) {
        return false;
    }
    !directory.contains('$') || directory == "${AROS_BUILD_DIR}/SYS/Devs"
}

/// Renders an output path as a CMake expression using the Make state at the
/// declaration. Local assignments take precedence over build-directory
/// mappings, and unresolved conditional values are rejected rather than
/// selected by their textual order.
fn render_output(raw: &str, scope: &VarScope, line: usize) -> Result<String, String> {
    render_output_inner(raw, scope, line, 8, &mut Vec::new())
}

fn render_output_inner(
    raw: &str,
    scope: &VarScope,
    line: usize,
    depth: usize,
    guard: &mut Vec<String>,
) -> Result<String, String> {
    if depth == 0 {
        return Err("output variable expansion exceeded its depth limit".to_owned());
    }
    let mut output = String::with_capacity(raw.len() + 32);
    let mut rest = raw;
    while let Some(start) = rest.find("$(") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            return Err("output path has an unterminated Make variable".to_owned());
        };
        let name = &after[..end];
        if name.is_empty() {
            return Err("output path has an empty Make variable".to_owned());
        }
        if guard.iter().any(|item| item == name) {
            return Err(format!("output path has a recursive Make variable {name}"));
        }
        if scope.path_is_conditional_at(name, line) {
            return Err(format!(
                "output path depends on {name}, assigned inside an unevaluated Make conditional"
            ));
        }

        // Path variables are tracked separately from word-list variables so
        // spaces and slash-bearing values retain Make's scalar path semantics.
        let replacement = if let Some(value) = scope.path_raw_at(name, line) {
            guard.push(name.to_owned());
            let expanded = render_output_inner(&value, scope, line, depth - 1, guard);
            guard.pop();
            expanded?
        } else if scope.is_known_local(name) {
            // A local introduced only by a proven-false branch is empty in
            // this target's Make environment.
            String::new()
        } else if let Some(mapped) = map_output_var(name) {
            mapped.to_owned()
        } else {
            return Err(format!("output path has unmapped variable {name}"));
        };
        output.push_str(&replacement);
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

/// Extracts the module name passed as the kickstart entry object.
fn startup_module(raw: &str) -> Option<String> {
    let object = raw.rsplit('/').next()?.strip_suffix(".o")?;
    let (module, object_kind) = object.rsplit_once('_')?;
    (!module.is_empty() && !object_kind.is_empty()).then(|| module.to_owned())
}

/// Reads every `%make_package` and `%link_kickstart` from one mmakefile.
///
/// Returns the declarations plus a list of the ones that could not be
/// resolved, so an unmapped output directory or an unresolved list surfaces
/// instead of silently producing a package with missing members.
#[must_use]
pub fn collect_packages(content: &str, rel_dir: &Path) -> (Vec<PackageDecl>, Vec<String>) {
    let joined = crate::parser::join_continuations(content);
    let (scope, line_states) = collect_vars_impl(&joined, None);
    let root = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let dirs = DirVars::load(&root);
    collect_packages_with_scope(&joined, rel_dir, &scope, &dirs, &root, &line_states)
}

/// Reads package macros with the parser's declaration-time Make scope.
///
/// `joined`, `scope`, and `line_states` must share the same continuation-joined
/// line coordinates. The parser pipeline supplies its selected target scope;
/// every other caller must provide states from `collect_vars_impl` so an
/// unknown conditional is diagnosed instead of treated as false.
#[must_use]
pub(crate) fn collect_packages_with_scope(
    joined: &str,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    line_states: &[ConditionalTruth],
) -> (Vec<PackageDecl>, Vec<String>) {
    let base = rel_dir.to_string_lossy().replace('\\', "/");
    let file = if base.is_empty() {
        "mmakefile.src".to_owned()
    } else {
        format!("{base}/mmakefile.src")
    };

    let mut decls = Vec::new();
    let mut skipped = Vec::new();

    for (line_number, line) in joined.lines().enumerate() {
        let trimmed = line.trim();
        let is_kickstart = trimmed.starts_with("%link_kickstart");
        if !is_kickstart && !trimmed.starts_with("%make_package") {
            continue;
        }

        let line_display = line_number + 1;
        match line_states
            .get(line_number)
            .copied()
            .unwrap_or(ConditionalTruth::Unknown)
        {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                let mmake = arg(trimmed, "mmake").unwrap_or_default();
                skipped.push(format!(
                    "{file}:{line_display}: {mmake} is guarded by an unresolved Make conditional"
                ));
                continue;
            }
            ConditionalTruth::True => {}
        }

        let Some(mmake) = arg(trimmed, "mmake") else {
            continue;
        };
        let Some(raw_file) = arg(trimmed, "file") else {
            skipped.push(format!("{file}: {mmake} has no file="));
            continue;
        };
        let output = match render_output(&raw_file, scope, line_number) {
            Ok(output) => output,
            Err(reason) => {
                skipped.push(format!(
                    "{file}:{line_display}: {mmake} output {raw_file}: {reason}"
                ));
                continue;
            }
        };

        // startup names the first executable member in the link-kickstart
        // object. A malformed supplied value must not degrade into no startup.
        let startup = if let Some(raw) = arg(trimmed, "startup") {
            if let Some(startup) = startup_module(&raw) {
                Some(startup)
            } else {
                skipped.push(format!(
                    "{file}:{line_display}: {mmake} startup={raw} is not a module object path"
                ));
                continue;
            }
        } else {
            None
        };

        let expression_context = MakeExprContext::new(scope, dirs, line_number, root, rel_dir);
        let mut members = Vec::new();
        let mut unresolved_members = false;
        for (key, kind) in CATEGORIES {
            let Some(raw) = arg(trimmed, key) else {
                continue;
            };
            let names = match evaluate_make_list(&raw, &expression_context) {
                Ok(names) => names,
                Err(reason) => {
                    skipped.push(format!(
                        "{file}:{line_display}: {mmake} {key}={raw}: {reason}"
                    ));
                    unresolved_members = true;
                    break;
                }
            };
            if let Some(name) = names
                .iter()
                .find(|name| name.contains(['$', ';', '(', ')']))
            {
                skipped.push(format!(
                    "{file}:{line_display}: {mmake} {key}={raw} resolved to a non-concrete member '{name}'"
                ));
                unresolved_members = true;
                break;
            }
            for name in names {
                members.push((kind.to_owned(), name));
            }
        }
        if unresolved_members {
            continue;
        }
        if let Some(raw) = arg(trimmed, "misc") {
            let paths = match evaluate_make_list(&raw, &expression_context) {
                Ok(paths) => paths,
                Err(reason) => {
                    skipped.push(format!(
                        "{file}:{line_display}: {mmake} misc={raw}: {reason}"
                    ));
                    continue;
                }
            };
            for path in paths {
                if !misc_path_is_concrete(&path) {
                    skipped.push(format!(
                        "{file}:{line_display}: {mmake} misc={raw} resolved to a non-concrete member '{path}'"
                    ));
                    unresolved_members = true;
                    break;
                }
                if let Some(member) = misc_member(&path) {
                    members.push(member);
                } else {
                    skipped.push(format!(
                        "{file}:{line_display}: {mmake} misc={path} has no canonical basename"
                    ));
                    unresolved_members = true;
                    break;
                }
            }
        }
        if unresolved_members {
            continue;
        }

        // A package with no members is a declaration we failed to read, not an
        // empty package: the tree has none.
        if members.is_empty() {
            skipped.push(format!("{file}: {mmake} resolved to no members"));
            continue;
        }

        let uselibs = if let Some(raw) = arg(trimmed, "uselibs") {
            match evaluate_make_list(&raw, &expression_context) {
                Ok(uselibs)
                    if uselibs
                        .iter()
                        .all(|name| !name.contains(['$', ';', '(', ')'])) =>
                {
                    uselibs
                }
                Ok(_) => {
                    skipped.push(format!(
                        "{file}:{line_display}: {mmake} uselibs={raw} resolved to a non-concrete library"
                    ));
                    continue;
                }
                Err(reason) => {
                    skipped.push(format!(
                        "{file}:{line_display}: {mmake} uselibs={raw}: {reason}"
                    ));
                    continue;
                }
            }
        } else {
            Vec::new()
        };

        decls.push(PackageDecl {
            file: file.clone(),
            mmake,
            output,
            members,
            startup,
            uselibs,
            is_kickstart,
            resolved: Vec::new(),
            arch: declaring_arch(rel_dir),
        });
    }

    (decls, skipped)
}

/// Reads `key=value` at a word boundary.
fn arg(line: &str, key: &str) -> Option<String> {
    let mut from = 0usize;
    loop {
        let hit = line[from..].find(key)? + from;
        let before_ok = hit == 0
            || line[..hit]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let rest = &line[hit + key.len()..];
        if before_ok {
            if let Some(v) = rest.strip_prefix("=\"") {
                let end = v.find('"')?;
                return Some(v[..end].to_owned());
            }
            if let Some(v) = rest.strip_prefix('=') {
                let end = v.find(char::is_whitespace).unwrap_or(v.len());
                let value = v[..end].trim();
                if !value.is_empty() {
                    return Some(value.to_owned());
                }
            }
        }
        from = hit + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::TargetContext;
    use std::path::{Path, PathBuf};

    fn collect_selected_packages(
        source: &str,
        rel_dir: &Path,
        root: &Path,
        target: &TargetContext,
    ) -> (Vec<PackageDecl>, Vec<String>) {
        let joined = crate::parser::join_continuations(source);
        let (scope, line_states) = collect_vars_impl(&joined, Some(target));
        let dirs = DirVars::load(root);
        collect_packages_with_scope(&joined, rel_dir, &scope, &dirs, root, &line_states)
    }

    #[test]
    fn reads_the_base_package() {
        // Condensed from rom/mmakefile.src:152.
        let src = "\
BASE_DEVICES  := console input gameport keyboard
BASE_HANDLERS := ram con
BASE_LIBS     := aros dos dos64
BASE_LIBS_ARCH := debug
BASE_RSRCS    := bootloader dosboot

%make_package mmake=kernel-package-base file=$(AROS_BOOT)/aros-base.pkg \\
\tdevs=$(BASE_DEVICES) handlers=$(BASE_HANDLERS) libs=$(BASE_LIBS) \\
\tarch_libs=$(BASE_LIBS_ARCH) res=$(BASE_RSRCS)
";
        let (decls, skipped) = collect_packages(src, &PathBuf::from("rom"));
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(decls.len(), 1);
        let d = &decls[0];
        assert_eq!(d.mmake, "kernel-package-base");
        assert_eq!(d.output, "${AROS_BOOT_DIR}/aros-base.pkg");
        assert!(!d.is_kickstart);

        let names: Vec<&str> = d.members.iter().map(|(_, n)| n.as_str()).collect();
        // dos64, both handlers and debug are exactly what the hand-written
        // list in Kickstart.cmake was missing.
        assert!(names.contains(&"dos64"));
        assert!(names.contains(&"ram"));
        assert!(names.contains(&"con"));
        assert!(names.contains(&"debug"));
        assert_eq!(d.members.len(), 12);

        let kinds: Vec<&str> = d
            .members
            .iter()
            .filter(|(_, n)| n == "ram")
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(kinds, vec!["handler"]);
    }

    #[test]
    fn reads_the_kickstart_link() {
        // arch/x86_64-pc/boot/mmakefile.src.
        let src = "%link_kickstart mmake=kernel-pc-x86_64-kernel file=$(AROSARCHDIR)/kernel \\\n\tstartup=$(KOBJSDIR)/kernel_resource.o libs=exec res=task\n";
        let (decls, skipped) = collect_packages(src, &PathBuf::from("arch/x86_64-pc/boot"));
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(decls.len(), 1);
        let d = &decls[0];
        assert!(d.is_kickstart);
        assert_eq!(d.output, "${AROS_BOOT_ARCH_DIR}/kernel");
        // The startup object names the module whose first executable section
        // the bootstrap jumps to.
        assert_eq!(d.startup.as_deref(), Some("kernel"));
        assert_eq!(
            d.members,
            vec![
                ("library".to_owned(), "exec".to_owned()),
                ("resource".to_owned(), "task".to_owned())
            ]
        );
    }

    #[test]
    fn malformed_supplied_startup_is_not_treated_as_absent() {
        let src =
            "%link_kickstart mmake=x file=$(AROS_BOOT)/kernel startup=not-an-object libs=exec\n";
        let (decls, skipped) = collect_packages(src, Path::new("arch/riscv-esp32p4/boot"));
        assert!(decls.is_empty());
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("startup=not-an-object"), "{skipped:#?}");
    }

    #[test]
    fn an_unmapped_output_directory_is_reported() {
        let src = "%make_package mmake=x file=$(SOMEWHERE)/x.pkg libs=a\n";
        let (decls, skipped) = collect_packages(src, &PathBuf::from("d"));
        assert!(decls.is_empty());
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("unmapped"));
    }

    #[test]
    fn output_roots_follow_make_cfg_layout() {
        let (scope, _) = collect_vars_impl("", None);
        assert_eq!(
            render_output("$(TARGETDIR)/root.bin", &scope, 0).unwrap(),
            "${AROS_BUILD_DIR}/root.bin"
        );
        assert_eq!(
            render_output("$(AROSDIR)/system.bin", &scope, 0).unwrap(),
            "${AROS_SYS_DIR}/system.bin"
        );
        assert_eq!(
            render_output("$(AROS_BOOT)/base.pkg", &scope, 0).unwrap(),
            "${AROS_BOOT_DIR}/base.pkg"
        );
        assert_eq!(
            render_output("$(AROSARCHDIR)/kernel", &scope, 0).unwrap(),
            "${AROS_BOOT_ARCH_DIR}/kernel"
        );
        // There is no AROS_BOOT_ARCH variable in make.cfg.in. Keeping an
        // invented alias would conceal a misspelling in a declaration.
        assert_eq!(map_output_var("AROS_BOOT_ARCH"), None);
    }

    #[test]
    fn output_variables_use_make_state_at_each_declaration() {
        let src = "\
PACKAGE_OUTPUT = $(AROS_BOOT)/before.pkg
%make_package mmake=before file=$(PACKAGE_OUTPUT) libs=exec
PACKAGE_OUTPUT = $(AROS_BOOT)/after.pkg
%make_package mmake=after file=$(PACKAGE_OUTPUT) libs=exec
";
        let (decls, skipped) = collect_packages(src, Path::new("rom"));
        assert!(skipped.is_empty(), "{skipped:#?}");
        assert_eq!(
            decls
                .iter()
                .map(|decl| decl.output.as_str())
                .collect::<Vec<_>>(),
            vec!["${AROS_BOOT_DIR}/before.pkg", "${AROS_BOOT_DIR}/after.pkg"]
        );
    }

    #[test]
    fn output_variables_follow_proven_false_and_true_branches() {
        let root = tempfile::tempdir().unwrap();
        let src = "\
ifeq ($(AROS_TARGET_CPU),riscv)
PACKAGE_OUTPUT := $(AROS_BOOT)/riscv.pkg
else
PACKAGE_OUTPUT := $(AROS_BOOT)/other.pkg
endif
%make_package mmake=selected file=$(PACKAGE_OUTPUT) libs=exec
";
        let target = TargetContext {
            cpu: Some("riscv".to_owned()),
            ..TargetContext::default()
        };
        let (decls, skipped) = collect_selected_packages(
            src,
            Path::new("arch/riscv-esp32p4/boot"),
            root.path(),
            &target,
        );
        assert!(skipped.is_empty(), "{skipped:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].output, "${AROS_BOOT_DIR}/riscv.pkg");
    }

    #[test]
    fn output_variables_in_unknown_branches_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let src = "\
ifeq ($(OPTIONAL_OUTPUT),1)
PACKAGE_OUTPUT := $(AROS_BOOT)/optional.pkg
endif
%make_package mmake=unknown-output file=$(PACKAGE_OUTPUT) libs=exec
";
        let target = TargetContext {
            cpu: Some("riscv".to_owned()),
            ..TargetContext::default()
        };
        let (decls, skipped) = collect_selected_packages(
            src,
            Path::new("arch/riscv-esp32p4/boot"),
            root.path(),
            &target,
        );
        assert!(decls.is_empty());
        assert_eq!(skipped.len(), 1);
        assert!(
            skipped[0].contains("PACKAGE_OUTPUT")
                && skipped[0].contains("unevaluated Make conditional"),
            "{skipped:#?}"
        );
    }

    #[test]
    fn target_parameter_aliases_follow_target_cfg() {
        for (reference, alias, expected) in [
            ("AROS_TARGET_CPU", "CPU", "${AROS_TARGET_CPU}"),
            ("AROS_TARGET_ARCH", "ARCH", "${AROS_TARGET_PLATFORM}"),
            ("AROS_TARGET_FAMILY", "FAMILY", "${AROS_TARGET_FAMILY}"),
        ] {
            assert_eq!(map_output_var(reference), Some(expected));
            assert_eq!(map_output_var(alias), Some(expected));
        }
    }

    #[test]
    fn local_output_name_expands_below_the_system_root() {
        let src = "\
ARM_BSP := aros-$(AROS_TARGET_CPU)-bsp.rom
%make_package mmake=kernel-package-raspi-arm file=$(AROSDIR)/$(ARM_BSP) libs=exec
";
        let (decls, skipped) = collect_packages(src, Path::new("arch/arm-raspi/boot"));
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(
            decls[0].output,
            "${AROS_SYS_DIR}/aros-${AROS_TARGET_CPU}-bsp.rom"
        );
    }

    #[test]
    fn a_declaration_resolving_to_nothing_is_reported() {
        // aros-acpi declares devs= and res= empty, so only hidds carry members;
        // with none of them resolvable the package would be silently empty.
        let src = "%make_package mmake=x file=$(AROS_BOOT)/x.pkg devs=$(UNKNOWN)\n";
        let (decls, skipped) = collect_packages(src, &PathBuf::from("d"));
        assert!(decls.is_empty());
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("UNKNOWN"), "{skipped:#?}");
    }

    #[test]
    fn runtime_basenames_match_make_package() {
        assert_eq!(runtime_name("handler", "ram"), "ram-handler");
        assert_eq!(runtime_name("library", "dos"), "dos.library");
        assert_eq!(
            runtime_name("class", "USB/bootkeyboard"),
            "bootkeyboard.class"
        );
        assert_eq!(
            runtime_name("device", "USBHardware/pciusb"),
            "pciusb.device"
        );
    }

    #[test]
    fn reads_the_x86_64_bsp_misc_logger() {
        let src = "\
LOG_RESOURCES := serial.logger
BSP_MISC := \\
        $(addprefix $(AROS_DEVS)/,$(LOG_RESOURCES))

%make_package mmake=kernel-bsp-pc-x86_64 file=$(AROSARCHDIR)/aros-bsp.pkg \\
    libs=exec misc=$(BSP_MISC)
";
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("config")).unwrap();
        std::fs::write(
            root.path().join("config/make.cfg.in"),
            "AROS_DEVS := /AROS/Devs\n",
        )
        .unwrap();
        let target = TargetContext::default();
        let (decls, skipped) =
            collect_selected_packages(src, Path::new("arch/x86_64-pc/boot"), root.path(), &target);
        assert!(skipped.is_empty(), "{skipped:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(
            decls[0].members,
            vec![
                ("library".to_owned(), "exec".to_owned()),
                ("logger".to_owned(), "serial".to_owned())
            ]
        );
        assert_eq!(runtime_name("logger", "serial"), "serial.logger");
    }

    #[test]
    fn deferred_build_directory_prefix_is_allowed_for_concrete_misc_member() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("config")).unwrap();
        std::fs::write(
            root.path().join("config/make.cfg.in"),
            "AROS_DEVS := $(TOP)/SYS/Devs\n",
        )
        .unwrap();
        let src = "\
LOG_RESOURCES := serial.logger
BSP_MISC := $(addprefix $(AROS_DEVS)/,$(LOG_RESOURCES))
%make_package mmake=kernel-bsp file=$(AROS_BOOT)/bsp.pkg libs=exec misc=$(BSP_MISC)
";
        let target = TargetContext {
            cpu: Some("x86_64".to_owned()),
            platform: Some("pc".to_owned()),
            ..TargetContext::default()
        };
        let (decls, skipped) =
            collect_selected_packages(src, Path::new("arch/x86_64-pc/boot"), root.path(), &target);
        assert!(skipped.is_empty(), "{skipped:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(
            decls[0].members,
            vec![
                ("library".to_owned(), "exec".to_owned()),
                ("logger".to_owned(), "serial".to_owned())
            ]
        );
    }

    #[test]
    fn misc_member_basename_rejects_variable_and_shell_syntax() {
        for path in [
            "${AROS_BUILD_DIR}/SYS/Devs/serial$(UNKNOWN).logger",
            "${AROS_BUILD_DIR}/SYS/Devs/serial$(shell echo bad).logger",
        ] {
            assert!(
                !misc_path_is_concrete(path),
                "unsafe misc basename accepted: {path}"
            );
        }
    }

    #[test]
    fn typed_members_may_retain_an_install_subdirectory() {
        let src = "\
USB_CLASSES := USB/bootkeyboard USB/hub
USB_DEVS := USBHardware/pciusb
%make_package mmake=usb file=$(AROS_BOOT)/usb.pkg \\
    classes=$(USB_CLASSES) devs=$(USB_DEVS)
";
        let (decls, skipped) = collect_packages(src, Path::new("rom/usb"));
        assert!(skipped.is_empty(), "{skipped:#?}");
        assert_eq!(
            decls[0].members,
            vec![
                ("class".to_owned(), "USB/bootkeyboard".to_owned()),
                ("class".to_owned(), "USB/hub".to_owned()),
                ("device".to_owned(), "USBHardware/pciusb".to_owned()),
            ]
        );
    }

    #[test]
    fn an_unresolved_misc_list_is_reported_without_hiding_other_members() {
        let src = "%make_package mmake=x file=$(AROS_BOOT)/x.pkg libs=exec misc=$(UNKNOWN)\n";
        let (decls, skipped) = collect_packages(src, Path::new("rom"));
        assert!(decls.is_empty());
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("misc=$(UNKNOWN)"), "{skipped:#?}");
    }

    #[test]
    fn package_lists_follow_make_assignment_operators_at_each_declaration() {
        let src = "\
PKG_LIBS := initial
PKG_DEVS ?= default-device
PKG_DEVS ?= ignored-device
%make_package mmake=package-before file=$(AROS_BOOT)/before.pkg libs=\"$(PKG_LIBS)\" devs=\"$(PKG_DEVS)\"
PKG_LIBS = replacement
%make_package mmake=package-after-replacement file=$(AROS_BOOT)/replacement.pkg libs=\"$(PKG_LIBS)\"
PKG_LIBS += appended
%make_package mmake=package-after-append file=$(AROS_BOOT)/append.pkg libs=\"$(PKG_LIBS)\"
PKG_LIBS ?= ignored
%make_package mmake=package-after-set-if-unset file=$(AROS_BOOT)/set-if-unset.pkg libs=\"$(PKG_LIBS)\"
";
        let (decls, skipped) = collect_packages(src, Path::new("arch/riscv-esp32p4/boot"));
        assert!(skipped.is_empty(), "{skipped:#?}");
        assert_eq!(decls.len(), 4);
        assert_eq!(
            decls[0].members[0],
            ("device".to_owned(), "default-device".to_owned())
        );
        assert_eq!(
            decls[0].members[1],
            ("library".to_owned(), "initial".to_owned())
        );
        assert_eq!(
            decls[1].members,
            vec![("library".to_owned(), "replacement".to_owned())]
        );
        assert_eq!(
            decls[2].members,
            vec![
                ("library".to_owned(), "replacement".to_owned()),
                ("library".to_owned(), "appended".to_owned()),
            ]
        );
        assert_eq!(decls[3].members, decls[2].members);
    }

    #[test]
    fn a_typed_list_expansion_error_skips_the_whole_package() {
        let src = "\
PKG_LIBS := exec $(UNRESOLVED_PACKAGE_LIB)
%make_package mmake=package file=$(AROS_BOOT)/package.pkg libs=\"$(PKG_LIBS)\" devs=console
";
        let (decls, skipped) = collect_packages(src, Path::new("rom"));
        assert!(decls.is_empty(), "a partial package must not be emitted");
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("PKG_LIBS"), "{skipped:#?}");
        assert!(
            skipped[0].contains("UNRESOLVED_PACKAGE_LIB"),
            "{skipped:#?}"
        );
    }

    #[test]
    fn target_condition_selects_only_the_true_package_branch() {
        let root = tempfile::tempdir().unwrap();
        let src = "\
PKG_LIBS := base
ifeq ($(AROS_TARGET_CPU),riscv)
PKG_LIBS += riscv-only
%make_package mmake=package-riscv file=$(AROS_BOOT)/riscv.pkg libs=\"$(PKG_LIBS)\"
else
PKG_LIBS := other-cpu-only
%make_package mmake=package-other file=$(AROS_BOOT)/other.pkg libs=\"$(PKG_LIBS)\"
endif
PKG_LIBS += after-conditional
%make_package mmake=package-after file=$(AROS_BOOT)/after.pkg libs=\"$(PKG_LIBS)\"
";
        let target = TargetContext {
            cpu: Some("riscv".to_owned()),
            ..TargetContext::default()
        };
        let (decls, skipped) = collect_selected_packages(
            src,
            Path::new("arch/riscv-esp32p4/boot"),
            root.path(),
            &target,
        );
        assert!(skipped.is_empty(), "{skipped:#?}");
        assert_eq!(decls.len(), 2);
        assert_eq!(decls[0].mmake, "package-riscv");
        assert_eq!(
            decls[0].members,
            vec![
                ("library".to_owned(), "base".to_owned()),
                ("library".to_owned(), "riscv-only".to_owned()),
            ]
        );
        assert_eq!(decls[1].mmake, "package-after");
        assert_eq!(
            decls[1].members,
            vec![
                ("library".to_owned(), "base".to_owned()),
                ("library".to_owned(), "riscv-only".to_owned()),
                ("library".to_owned(), "after-conditional".to_owned()),
            ]
        );
    }

    #[test]
    fn unknown_package_condition_is_not_selected_by_absence() {
        let root = tempfile::tempdir().unwrap();
        let src = "\
PKG_LIBS := base
ifeq ($(OPTIONAL_GRAPHICS),1)
PKG_LIBS += optional-ui
%make_package mmake=package-optional file=$(AROS_BOOT)/optional.pkg libs=\"$(PKG_LIBS)\"
endif
%make_package mmake=package-default file=$(AROS_BOOT)/default.pkg libs=\"$(PKG_LIBS)\"
";
        let target = TargetContext {
            cpu: Some("riscv".to_owned()),
            ..TargetContext::default()
        };
        let (decls, skipped) = collect_selected_packages(
            src,
            Path::new("arch/riscv-esp32p4/boot"),
            root.path(),
            &target,
        );
        assert!(
            decls.is_empty(),
            "unresolved list state must not emit a subset"
        );
        assert_eq!(skipped.len(), 2);
        assert!(skipped.iter().any(|message| {
            message.contains("package-optional") && message.contains("unresolved Make conditional")
        }));
        assert!(skipped.iter().any(|message| {
            message.contains("PKG_LIBS") && message.contains("unevaluated Make conditional")
        }));
    }

    #[test]
    fn actual_p4_package_conditionals_are_reported_when_selectors_are_unknown() {
        let Ok(source_root) = std::env::var("AROS_TEST_P4_SOURCE_ROOT") else {
            return;
        };
        let root = PathBuf::from(source_root);
        let rel_dir = Path::new("arch/riscv-esp32p4/boot");
        let path = root.join(rel_dir).join("mmakefile.src");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let target = TargetContext {
            cpu: Some("riscv".to_owned()),
            platform: Some("esp32p4".to_owned()),
            ..TargetContext::default()
        };
        let (decls, skipped) = collect_selected_packages(&source, rel_dir, &root, &target);
        assert!(
            decls
                .iter()
                .all(|declaration| { declaration.mmake != "kernel-package-esp32p4-riscv" }),
            "a package with unresolved C2/C3/C4 membership was emitted: {decls:#?}"
        );
        assert!(
            skipped.iter().any(|message| {
                message.contains("kernel-package-esp32p4-riscv")
                    && message.contains("PKG_DEVS")
                    && message.contains("unevaluated Make conditional")
            }),
            "P4 package switches were not diagnosed: {skipped:#?}"
        );
    }
}
