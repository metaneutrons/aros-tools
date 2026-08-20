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

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

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
    /// mmake ids of the members, filled in by the graph once every mmakefile
    /// has been parsed. Startup comes first where one is declared.
    #[serde(default)]
    pub resolved: Vec<String>,
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
        // The boot directory, and its architecture-specific subdirectory.
        "AROS_BOOT" => Some("${AROS_BOOT_DIR}"),
        "AROSARCHDIR" | "AROS_BOOT_ARCH" => Some("${AROS_BOOT_ARCH_DIR}"),
        // The AROS tree root inside the build, which holds SYS/ and boot/.
        "AROSDIR" | "TARGETDIR" => Some("${AROS_BUILD_DIR}"),
        // Target parameters, so a rom image can be named after its CPU.
        "AROS_TARGET_CPU" | "CPU" => Some("${AROS_TARGET_CPU}"),
        "AROS_TARGET_ARCH" | "ARCH" => Some("${AROS_TARGET_PLATFORM}"),
        "AROS_TARGET_FAMILY" | "FAMILY" => Some("${AROS_TARGET_FAMILY}"),
        _ => None,
    }
}

/// Collects assignments verbatim, for variables that appear inside a path.
///
/// `collect_lists` keeps only plain word lists, which is right for module
/// membership but drops exactly what a path needs: arch/arm-raspi declares
/// `ARM_BSP := aros-$(AROS_TARGET_CPU)-bsp.rom`, and mingw32 declares
/// `EXEDIR := $(AROSARCHDIR)`.
fn collect_raw_values(content: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    for line in content.lines() {
        let payload = line.trim().trim_end_matches('\\').trim();
        let Some(idx) = payload.find(":=").or_else(|| payload.find('=')) else {
            continue;
        };
        let (lhs, rhs) = payload.split_at(idx);
        let name = lhs.trim().trim_end_matches(':').trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let value = rhs.trim_start_matches(':').trim_start_matches('=').trim();
        if !value.is_empty() && !value.contains(char::is_whitespace) {
            vars.insert(name.to_owned(), value.to_owned());
        }
    }
    vars
}

/// Collects the simple `NAME := a b c` assignments a declaration draws on.
///
/// Only plain word lists are kept. A value containing a path, a shell call or
/// another substitution is not a module list and would not resolve to targets
/// anyway.
fn collect_lists(content: &str) -> HashMap<String, Vec<String>> {
    let mut vars: HashMap<String, Vec<String>> = HashMap::new();
    let mut pending: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        let continues = trimmed.ends_with('\\');
        let payload = trimmed.trim_end_matches('\\').trim();

        let (name, value) = if let Some(name) = pending.take() {
            (name, payload)
        } else {
            let Some(idx) = payload.find(":=").or_else(|| payload.find('=')) else {
                continue;
            };
            let (lhs, rhs) = payload.split_at(idx);
            let name = lhs.trim().trim_end_matches(':').trim().to_owned();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            let rhs = rhs.trim_start_matches(':').trim_start_matches('=').trim();
            (name, rhs)
        };

        let words: Vec<String> = value
            .split_whitespace()
            .filter(|w| !w.contains('$') && !w.contains('/') && !w.contains('('))
            .map(str::to_owned)
            .collect();
        if !words.is_empty() {
            vars.entry(name.clone()).or_default().extend(words);
        }
        if continues {
            pending = Some(name);
        }
    }
    vars
}

/// Expands a category value into module names.
fn expand_names(raw: &str, vars: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut out = Vec::new();
    for token in raw.split_whitespace() {
        if let Some(inner) = token.strip_prefix("$(").and_then(|t| t.strip_suffix(')')) {
            if let Some(values) = vars.get(inner) {
                out.extend(values.iter().cloned());
            }
            continue;
        }
        if token.contains('$') {
            continue;
        }
        out.push(token.to_owned());
    }
    out
}

/// Renders an output path as a CMake expression.
///
/// Substitutes every `$(VAR)`, resolving a variable declared in the same
/// mmakefile first and falling back to the known build directories and target
/// parameters. Bounded so a self-referential assignment cannot loop.
fn render_output(raw: &str, raw_vars: &HashMap<String, String>) -> Option<String> {
    let mut current = raw.to_owned();
    for _ in 0..8 {
        let Some(start) = current.find("$(") else {
            return Some(current);
        };
        let after = &current[start + 2..];
        let end = after.find(')')?;
        let name = &after[..end];

        // A local assignment wins: EXEDIR := $(AROSARCHDIR) has to expand one
        // step further before the mapping applies.
        let replacement = raw_vars
            .get(name)
            .cloned()
            .or_else(|| map_output_var(name).map(str::to_owned))?;

        current = format!(
            "{}{replacement}{}",
            &current[..start],
            &after[end + 1..]
        );
    }
    None
}

/// Reads every `%make_package` and `%link_kickstart` from one mmakefile.
///
/// Returns the declarations plus a list of the ones that could not be
/// resolved, so an unmapped output directory or an unresolved list surfaces
/// instead of silently producing a package with missing members.
#[must_use]
pub fn collect_packages(content: &str, rel_dir: &Path) -> (Vec<PackageDecl>, Vec<String>) {
    let base = rel_dir.to_string_lossy().replace('\\', "/");
    let file = if base.is_empty() {
        "mmakefile.src".to_owned()
    } else {
        format!("{base}/mmakefile.src")
    };

    let vars = collect_lists(content);
    let raw_vars = collect_raw_values(content);
    let mut decls = Vec::new();
    let mut skipped = Vec::new();

    // Join continuations so a declaration spread over several lines is one
    // string, the same way the parser handles build macros.
    let joined = content.replace("\\\n", " ");
    for line in joined.lines() {
        let trimmed = line.trim();
        let is_kickstart = trimmed.starts_with("%link_kickstart");
        if !is_kickstart && !trimmed.starts_with("%make_package") {
            continue;
        }

        let Some(mmake) = arg(trimmed, "mmake") else {
            continue;
        };
        let Some(raw_file) = arg(trimmed, "file") else {
            skipped.push(format!("{file}: {mmake} has no file="));
            continue;
        };
        let Some(output) = render_output(&raw_file, &raw_vars) else {
            skipped.push(format!("{file}: {mmake} output {raw_file} is unmapped"));
            continue;
        };

        let mut members = Vec::new();
        for (key, kind) in CATEGORIES {
            let Some(raw) = arg(trimmed, key) else {
                continue;
            };
            for name in expand_names(&raw, &vars) {
                members.push((kind.to_owned(), name));
            }
        }

        // A package with no members is a declaration we failed to read, not an
        // empty package: the tree has none.
        if members.is_empty() {
            skipped.push(format!("{file}: {mmake} resolved to no members"));
            continue;
        }

        let startup = arg(trimmed, "startup").and_then(|s| {
            // startup=$(KOBJSDIR)/kernel_resource.o names an object by module
            // and kind; keep just the module name.
            let base = s.rsplit('/').next()?.strip_suffix(".o")?;
            base.rsplit_once('_').map(|(m, _)| m.to_owned())
        });

        let uselibs = arg(trimmed, "uselibs")
            .map(|u| expand_names(&u, &vars))
            .unwrap_or_default();

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
    use std::path::PathBuf;

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
    fn an_unmapped_output_directory_is_reported() {
        let src = "%make_package mmake=x file=$(SOMEWHERE)/x.pkg libs=a\n";
        let (decls, skipped) = collect_packages(src, &PathBuf::from("d"));
        assert!(decls.is_empty());
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("unmapped"));
    }

    #[test]
    fn a_declaration_resolving_to_nothing_is_reported() {
        // aros-acpi declares devs= and res= empty, so only hidds carry members;
        // with none of them resolvable the package would be silently empty.
        let src = "%make_package mmake=x file=$(AROS_BOOT)/x.pkg devs=$(UNKNOWN)\n";
        let (decls, skipped) = collect_packages(src, &PathBuf::from("d"));
        assert!(decls.is_empty());
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("no members"));
    }
}
