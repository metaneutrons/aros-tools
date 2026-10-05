//! Source proof of the ordinary archive command; not graph admission.

use crate::dirs::DirVars;
use crate::make_vars::VarScope;
use std::fs;
use std::path::{Component, Path};

const EXPECTED: [&str; 5] = [
    "%define mklib_q ar=$(AR) ranlib=$(RANLIB) to=$@ from=$(OBJS)",
    "$(Q)$(ECHO) \"Creating   $(subst $(TARGETDIR)/,,%(to))...\"",
    "$(Q)%(ar) %(to) %(from)",
    "$(Q)%(ranlib) %(to)",
    "%end",
];

/// Explicit roles proven by the source macro and its native configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceArchiveCommand {
    /// Source-selected GNU archiver arguments, not an inferred default.
    pub flags: Vec<String>,
}

/// The caller must supply the effective source-native snapshot and scope
/// used to collect its archive declarations. No generated config is read.
///
/// # Errors
/// Refuses an unbound include, changed/redefined source macro, unsafe source
/// path, or command role different from the admitted GNU archiver/ranlib.
pub fn prove(
    snapshot: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
) -> Result<SourceArchiveCommand, String> {
    if snapshot.lines().any(|line| {
        line.trim_start()
            .strip_prefix("%define ")
            .is_some_and(|tail| tail.split_whitespace().next() == Some("mklib_q"))
    }) {
        return Err("source-local mklib_q redefinition is not admitted".into());
    }
    if snapshot.lines().any(|line| {
        let text = line.trim_start();
        ["include ", "-include ", "sinclude "]
            .iter()
            .any(|prefix| text.starts_with(prefix))
    }) {
        return Err("archive command has an unbound Make include".into());
    }
    if dirs.expand("$(NATIVE_TARGET_AR)").as_deref() != Some("${CMAKE_AR}")
        || dirs.expand("$(NATIVE_TARGET_RANLIB)").as_deref() != Some("${CMAKE_RANLIB}")
    {
        return Err("archive command requires explicit admitted target roles".into());
    }
    let template = read_regular(root, Path::new("config/make.tmpl"))?;
    let mut definitions = Vec::new();
    let lines: Vec<_> = template.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if line
            .trim_start()
            .strip_prefix("%define ")
            .is_none_or(|tail| tail.split_whitespace().next() != Some("mklib_q"))
        {
            continue;
        }
        let mut definition = Vec::new();
        for line in &lines[index..] {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            definition.push(line);
            if line == "%end" {
                break;
            }
        }
        definitions.push(definition);
    }
    if definitions.as_slice() != [EXPECTED.to_vec()] {
        return Err("source mklib_q macro differs from the closed archive command".into());
    }
    for name in ["AR", "RANLIB", "NATIVE_TARGET_AR", "NATIVE_TARGET_RANLIB"] {
        if scope.path_is_conditional_at(name, usize::MAX) {
            return Err(format!(
                "archive role {name} has an unresolved conditional binding"
            ));
        }
    }
    for name in ["NATIVE_TARGET_AR", "NATIVE_TARGET_RANLIB"] {
        if scope.raw_at(name, usize::MAX).is_some() {
            return Err(format!(
                "source cannot redefine reserved target role {name}"
            ));
        }
    }
    if scope.raw_at("AR", usize::MAX).as_deref() != Some("$(NATIVE_TARGET_AR) cr")
        || scope.raw_at("RANLIB", usize::MAX).as_deref() != Some("$(NATIVE_TARGET_RANLIB)")
    {
        return Err("source archive roles must be admitted GNU AR cr and RANLIB".into());
    }
    Ok(SourceArchiveCommand {
        flags: vec!["cr".into()],
    })
}

fn read_regular(root: &Path, relative: &Path) -> Result<String, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    if !root.is_dir() {
        return Err("source root is not a directory".into());
    }
    let mut path = root;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("source macro path is not normalized".into());
        };
        path.push(name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
            return Err("source macro path is symlinked or non-regular".into());
        }
    }
    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 {
        return Err("source macro file is not bounded regular text".into());
    }
    fs::read_to_string(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_vars::collect_vars_impl;
    use crate::TargetContext;

    fn fixture() -> (tempfile::TempDir, DirVars) {
        let tree = tempfile::tempdir().unwrap();
        fs::create_dir(tree.path().join("config")).unwrap();
        fs::write(tree.path().join("config/make.tmpl"), EXPECTED.join("\n")).unwrap();
        let mut dirs = DirVars::load(tree.path());
        dirs.bind_native_target_tool_roles();
        (tree, dirs)
    }

    fn proof(root: &Path, dirs: &DirVars, source: &str) -> Result<SourceArchiveCommand, String> {
        let (scope, _) = collect_vars_impl(source, Some(&TargetContext::default()));
        prove(source, &scope, dirs, root)
    }

    const ROLES: &str = "AR=$(NATIVE_TARGET_AR) cr\nRANLIB=$(NATIVE_TARGET_RANLIB)\n";

    #[test]
    fn admitted_archive_roles_and_exact_macro_are_source_proven() {
        let (tree, dirs) = fixture();
        assert_eq!(proof(tree.path(), &dirs, ROLES).unwrap().flags, ["cr"]);
        for source in [
            ROLES.replace(" cr", " rcs"),
            ROLES.replace("$(NATIVE_TARGET_AR)", "ar"),
            ROLES.replace("$(NATIVE_TARGET_RANLIB)", "ranlib"),
            format!("{ROLES}include generated.cfg\n"),
            format!("{ROLES}%define mklib_q\n%end\n"),
            format!("{ROLES}NATIVE_TARGET_AR = other-ar\n"),
            format!("{ROLES}NATIVE_TARGET_RANLIB = other-ranlib\n"),
            format!("{ROLES}ifeq ($(UNKNOWN),yes)\nAR = other-ar\nendif\n"),
        ] {
            assert!(proof(tree.path(), &dirs, &source).is_err(), "{source}");
        }
        assert!(proof(tree.path(), &DirVars::load(tree.path()), ROLES).is_err());
    }

    #[test]
    fn changed_duplicate_and_symlinked_macros_are_refused() {
        let (tree, dirs) = fixture();
        let path = tree.path().join("config/make.tmpl");
        let original = EXPECTED.join("\n");
        for template in [
            original.replace("%(from)", "ignored.o"),
            format!("{original}\n{original}"),
            original.replace("$(Q)%(ranlib) %(to)", "$(Q)%(ranlib) %(to); echo injected"),
        ] {
            fs::write(&path, template).unwrap();
            assert!(proof(tree.path(), &dirs, ROLES).is_err());
        }
        #[cfg(unix)]
        {
            fs::remove_file(&path).unwrap();
            fs::write(tree.path().join("actual.tmpl"), original).unwrap();
            std::os::unix::fs::symlink("../actual.tmpl", &path).unwrap();
            assert!(proof(tree.path(), &dirs, ROLES).is_err());
        }
    }
}
