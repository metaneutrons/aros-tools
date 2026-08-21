//! Architecture option files pulled in with `-include .../make.opts`.
//!
//! A module can be tuned per architecture without touching its mmakefile:
//!
//! ```text
//! -include $(SRCDIR)/arch/all-$(ARCH)/timer/make.opts
//! -include $(SRCDIR)/arch/$(CPU)-$(ARCH)/timer/make.opts
//! ```
//!
//! `arch/all-pc/timer/make.opts` is the reason `struct TimerBase` has its
//! `tb_vblank_timerequest` field on PC: the file sets `-DUSE_VBLANK_EMU`, and
//! `rom/timer/timer_intern.h` declares the field under that guard. Skipping
//! these files makes the declaration vanish and `arch/all-pc/timer/timer_init.c`
//! fail to compile.
//!
//! The transpiler cannot know the target, so the paths are globbed against the
//! tree and each hit is tagged with the architecture it belongs to. CMake keeps
//! the tags that apply, the same arrangement as `%set_archincludes` and
//! `%build_archspecific`.
//!
//! Files containing Make conditionals are skipped and reported: the flag
//! collectors do not evaluate `ifeq`, so they would otherwise apply a branch
//! that was never taken. Three files in the tree are affected, all for hosted
//! ports.

use aros_common::read_source;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One resolved `make.opts` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakeOptsFile {
    /// Architecture tag, or `None` for a file that always applies (one included
    /// through `$(CURDIR)`).
    pub tag: Option<String>,
    /// Path relative to the source root.
    pub path: String,
}

/// Derives the architecture tag from an `arch/<a>-<b>` directory name.
///
/// Matches the tag forms used by `%set_archincludes`: a wildcard half drops out,
/// and a fully qualified directory becomes `<platform>-<cpu>`.
fn tag_from_arch_dir(dir: &str) -> Option<String> {
    let (a, b) = dir.split_once('-')?;
    if a.is_empty() || b.is_empty() {
        return None;
    }
    match (a, b) {
        ("all", other) | (other, "all") => Some(other.to_owned()),
        // Directories are <cpu>-<platform>, tags are <platform>-<cpu>.
        (cpu, platform) => Some(format!("{platform}-{cpu}")),
    }
}

/// Turns a Make path into a glob pattern, or `None` if it cannot be handled.
///
/// `$(SRCDIR)` and `$(CURDIR)` resolve; the target parameters become `*` so the
/// pattern can be matched against the tree.
fn to_glob(raw: &str, root: &Path, rel_dir: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len() + 16);
    let mut rest = raw;
    while let Some(start) = rest.find("$(") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find(')')?;
        match &after[..end] {
            "SRCDIR" | "TOP" => out.push_str(&root.to_string_lossy()),
            "CURDIR" => out.push_str(rel_dir),
            // Target parameters: matched against the tree instead of guessed.
            "ARCH"
            | "CPU"
            | "FAMILY"
            | "AROS_TARGET_VARIANT"
            | "AROS_TARGET_ARCH"
            | "AROS_TARGET_PLATFORM"
            | "AROS_TARGET_CPU" => out.push('*'),
            _ => return None,
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    // A variant directory may be absent, which leaves an empty path element.
    Some(out.replace("//", "/"))
}

/// Reads the `-include` lines of one `mmakefile.src` and resolves them.
///
/// Returns the files that apply plus, for reporting, the patterns that could
/// not be resolved and the files skipped for holding conditionals.
#[must_use]
pub fn collect_make_opts(
    content: &str,
    rel_dir: &Path,
    root: &Path,
) -> (Vec<MakeOptsFile>, Vec<String>) {
    let rel = rel_dir.to_string_lossy().replace('\\', "/");
    let mut out: Vec<MakeOptsFile> = Vec::new();
    let mut skipped = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        let Some(raw) = trimmed
            .strip_prefix("-include ")
            .or_else(|| trimmed.strip_prefix("include "))
        else {
            continue;
        };
        let raw = raw.trim();
        if !raw.ends_with("make.opts") {
            continue;
        }

        let Some(pattern) = to_glob(raw, root, &rel) else {
            skipped.push(format!("{rel}: {raw}"));
            continue;
        };

        let Ok(paths) = glob::glob(&pattern) else {
            skipped.push(format!("{rel}: {raw}"));
            continue;
        };

        for entry in paths.flatten() {
            let Ok(rel_path) = entry.strip_prefix(root) else {
                continue;
            };
            let rel_str = rel_path.to_string_lossy().replace('\\', "/");

            // The flag collectors do not evaluate conditionals, so a file that
            // has any would be applied unconditionally. Skip and report.
            if let Ok(body) = read_source(&entry) {
                if body.lines().any(|l| {
                    let t = l.trim_start();
                    t.starts_with("ifeq")
                        || t.starts_with("ifneq")
                        || t.starts_with("ifdef")
                        || t.starts_with("ifndef")
                }) {
                    skipped.push(format!("{rel_str}: contains Make conditionals"));
                    continue;
                }
            } else {
                continue;
            }

            // The tag comes from the arch/<a>-<b> element, when there is one.
            let mut comps = rel_str.split('/');
            let tag = if comps.next() == Some("arch") {
                comps.next().and_then(tag_from_arch_dir)
            } else {
                None
            };

            if !out.iter().any(|f| f.path == rel_str) {
                out.push(MakeOptsFile { tag, path: rel_str });
            }
        }
    }

    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_derivation_matches_the_set_archincludes_forms() {
        assert_eq!(tag_from_arch_dir("all-pc").as_deref(), Some("pc"));
        assert_eq!(tag_from_arch_dir("x86_64-all").as_deref(), Some("x86_64"));
        assert_eq!(tag_from_arch_dir("all-native").as_deref(), Some("native"));
        // Directories read <cpu>-<platform>, tags read <platform>-<cpu>.
        assert_eq!(tag_from_arch_dir("x86_64-pc").as_deref(), Some("pc-x86_64"));
        assert_eq!(
            tag_from_arch_dir("m68k-amiga").as_deref(),
            Some("amiga-m68k")
        );
        assert_eq!(tag_from_arch_dir("noseparator"), None);
    }

    #[test]
    fn target_parameters_become_wildcards() {
        let root = Path::new("/src");
        let g = to_glob(
            "$(SRCDIR)/arch/all-$(ARCH)/timer/make.opts",
            root,
            "rom/timer",
        );
        assert_eq!(g.as_deref(), Some("/src/arch/all-*/timer/make.opts"));

        let g = to_glob(
            "$(SRCDIR)/arch/$(CPU)-$(ARCH)/kernel/make.opts",
            root,
            "rom/kernel",
        );
        assert_eq!(g.as_deref(), Some("/src/arch/*-*/kernel/make.opts"));
    }

    #[test]
    fn curdir_resolves_to_the_declaring_directory() {
        let g = to_glob(
            "$(SRCDIR)/$(CURDIR)/make.opts",
            Path::new("/src"),
            "rom/usb/pciusb",
        );
        assert_eq!(g.as_deref(), Some("/src/rom/usb/pciusb/make.opts"));
    }

    #[test]
    fn an_absent_variant_directory_collapses() {
        let g = to_glob(
            "$(SRCDIR)/arch/all-$(ARCH)/$(AROS_TARGET_VARIANT)/exec/make.opts",
            Path::new("/src"),
            "rom/exec",
        );
        // Both elements become wildcards; the doubled separator is folded away.
        assert_eq!(g.as_deref(), Some("/src/arch/all-*/*/exec/make.opts"));
    }

    #[test]
    fn an_unknown_variable_is_rejected() {
        assert!(to_glob("$(SOMETHING_ELSE)/make.opts", Path::new("/src"), "d").is_none());
    }

    #[test]
    fn non_make_opts_includes_are_ignored() {
        let (files, skipped) = collect_make_opts(
            "include $(SRCDIR)/config/aros.cfg\n",
            Path::new("rom/timer"),
            Path::new("/nonexistent"),
        );
        assert!(files.is_empty());
        assert!(skipped.is_empty());
    }
}
