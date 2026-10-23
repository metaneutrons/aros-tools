//! Separate reviewed generator-local Make variables from global compile scope.
//!
//! This does not execute or translate generator recipes. The existing Mesa26
//! capability owns their Python jobs; it seals the same complete source file.

use super::configured_scope::defined_macro_name;
use crate::make_vars::strip_make_comment;
use crate::{
    capability::require_file_fingerprint, fingerprints::fingerprint, parser::TargetContext,
};
use std::{
    collections::BTreeSet,
    path::{Component, Path},
};

const PREFIX: &str = "$(top_builddir)/$(CUR_MESADIR)/";
const NAMES: [&str; 2] = ["PRIVATE_SCRIPT", "PRIVATE_XML"];

/// Productive graph admission also requires the existing capability's generated
/// owner; a matching source pathname alone is not a generator declaration.
pub(super) fn require_core_owner(groups: &[crate::ast::PythonOutputsDecl]) -> Result<(), String> {
    let selected = groups
        .iter()
        .filter(|group| {
            group.dir_path == Path::new("workbench/libs/mesa/libmesa")
                || group.owner == "mesa3d-linklib-mesa-generated"
                || group
                    .consumers
                    .iter()
                    .any(|consumer| consumer == "mesa3d-linklib-mesa")
        })
        .collect::<Vec<_>>();
    let [group] = selected.as_slice() else {
        return Err("configured Mesa core requires exactly one reviewed generator owner".into());
    };
    if group.owner != "mesa3d-linklib-mesa-generated"
        || group.dir_path != Path::new("workbench/libs/mesa/libmesa")
        || group.consumers != ["mesa3d-linklib-mesa"]
        || group.source_root != super::mesa26::SOURCE_ROOT
        || group.build_root != super::mesa26::BUILD_ROOT
        || group.fetch_target != "mesa3d-fetch"
        || group.jobs.len() != 25
        || group
            .jobs
            .iter()
            .map(|job| &job.output)
            .collect::<BTreeSet<_>>()
            .len()
            != 25
    {
        return Err(
            "configured Mesa core generator owner differs from its reviewed capability".into(),
        );
    }
    Ok(())
}

pub(super) fn reviewed_lines(
    text: &str,
    root: &Path,
    relative: &Path,
    target: &TargetContext,
) -> Result<BTreeSet<usize>, String> {
    let lines = classify(text)?;
    if lines.is_empty() {
        return Ok(lines);
    }
    if relative != Path::new("workbench/libs/mesa/libmesa")
        || target.mesa_version.as_deref() != Some("26.0.0")
    {
        return Err(
            "generator-local Mesa variables require the reviewed Mesa26 core capability".into(),
        );
    }
    require_file_fingerprint(
        root,
        "workbench/libs/mesa/libmesa/mmakefile.src",
        fingerprint("mesa26-core-recipe")?,
        "configured Mesa generator-local scope",
    )?;
    require_file_fingerprint(
        root,
        "workbench/libs/mesa/mesa.cfg",
        fingerprint("mesa26-config")?,
        "configured Mesa generator-local scope",
    )?;
    Ok(lines)
}

/// These values are target-specific, not global Make assignments. Do not let
/// any global expression consume them as though the compiler inherited them.
fn classify(text: &str) -> Result<BTreeSet<usize>, String> {
    let mut lines = BTreeSet::new();
    let mut define_depth = 0usize;
    for (index, line) in text.lines().enumerate() {
        if line.starts_with('\t') {
            continue;
        }
        let visible = strip_make_comment(line).trim();
        if let Some(name) = defined_macro_name(line) {
            if NAMES.contains(&name.as_str()) {
                return Err("generator-local variable cannot have a global Make definition".into());
            }
            define_depth += 1;
            continue;
        }
        if visible == "endef" {
            define_depth = define_depth
                .checked_sub(1)
                .ok_or("unmatched generator-scope endef")?;
            continue;
        }
        if define_depth != 0 || !NAMES.iter().any(|name| visible.contains(name)) {
            continue;
        }
        if !is_binding(visible) {
            return Err(format!(
                "generator-local Make variable used outside a reviewed target binding at line {}",
                index + 1
            ));
        }
        lines.insert(index);
    }
    if define_depth != 0 {
        return Err("unterminated generator-scope definition".into());
    }
    Ok(lines)
}

fn is_binding(line: &str) -> bool {
    let Some((target, assignment)) = line.split_once(':') else {
        return false;
    };
    let Some(output) = target.trim().strip_prefix(PREFIX) else {
        return false;
    };
    if output.is_empty()
        || !output
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_./-".contains(&byte))
        || output
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || !Path::new(output)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        || !matches!(
            Path::new(output).extension().and_then(|part| part.to_str()),
            Some("c" | "h")
        )
    {
        return false;
    }
    let Some((name, _value)) = assignment.trim().split_once(":=") else {
        return false;
    };
    NAMES.contains(&name.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_make_keeps_generator_variables_local_to_the_output_target() {
        use std::{
            io::Write,
            process::{Command, Stdio},
        };
        let source = "USER_CFLAGS := -DORDER=1 -DORDER=1\n.PHONY: all output.h\noutput.h: PRIVATE_SCRIPT := -DLOCAL=1\nall: output.h\n\t@printf 'global|%s|%s\\n' '$(PRIVATE_SCRIPT)' '$(USER_CFLAGS)'\noutput.h:\n\t@printf 'local|%s|%s\\n' '$(PRIVATE_SCRIPT)' '$(USER_CFLAGS)'\n";
        let run = |text: &str| {
            let make = if cfg!(target_os = "macos") {
                "gmake"
            } else {
                "make"
            };
            let mut child = Command::new(make)
                .args(["--no-print-directory", "-rR", "-f", "-", "all"])
                .env_remove("MAKEFLAGS")
                .env_remove("MFLAGS")
                .env_remove("PRIVATE_SCRIPT")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        assert_eq!(
            run(source),
            "local|-DLOCAL=1|-DORDER=1 -DORDER=1\nglobal||-DORDER=1 -DORDER=1\n"
        );
        let global = source.replace("output.h: PRIVATE_SCRIPT :=", "PRIVATE_SCRIPT :=");
        assert_eq!(
            run(&global),
            "local|-DLOCAL=1|-DORDER=1 -DORDER=1\nglobal|-DLOCAL=1|-DORDER=1 -DORDER=1\n"
        );
        assert!(classify(&global).is_err());
    }

    #[test]
    fn productive_scope_requires_the_unique_core_generator_owner() {
        let mut group: crate::ast::PythonOutputsDecl = serde_json::from_value(serde_json::json!({
            "owner": "mesa3d-linklib-mesa-generated", "source_root": super::super::mesa26::SOURCE_ROOT,
            "build_root": super::super::mesa26::BUILD_ROOT, "fetch_target": "mesa3d-fetch",
            "source_inputs": [], "jobs": (0..25).map(|index| serde_json::json!({
                "script": "generator.py", "output": format!("main/output{index}.h"), "arguments": []
            })).collect::<Vec<_>>(), "audited_source_dir": super::super::mesa26::SOURCE_ROOT,
            "local_patch_files": [], "consumers": ["mesa3d-linklib-mesa"],
            "dir_path": "workbench/libs/mesa/libmesa"
        })).unwrap();
        assert!(require_core_owner(&[group.clone()]).is_ok());
        assert!(require_core_owner(&[]).is_err());
        assert!(require_core_owner(&[group.clone(), group.clone()]).is_err());
        group.consumers = vec!["unrelated".into()];
        assert!(require_core_owner(&[group.clone()]).is_err());
        group.consumers = vec!["mesa3d-linklib-mesa".into()];
        group.jobs[1].output = group.jobs[0].output.clone();
        assert!(require_core_owner(&[group]).is_err());
    }

    #[test]
    fn private_bindings_are_target_local_not_global_option_inputs() {
        let text = "USER_CFLAGS := -DORDER=1 -DORDER=1\n$(top_builddir)/$(CUR_MESADIR)/main/dispatch.h: PRIVATE_SCRIPT := $(PYTHON) $(glapi)/gl_table.py\n$(top_builddir)/$(CUR_MESADIR)/main/dispatch.h: PRIVATE_XML := -f $(glapi)/gl_and_es_API.xml\n";
        assert_eq!(classify(text).unwrap(), BTreeSet::from([1, 2]));
        let (scope, _) = crate::make_vars::collect_vars_impl(text, None);
        assert!(scope.raw_at("PRIVATE_SCRIPT", usize::MAX).is_none());
        assert!(scope.raw_at("PRIVATE_XML", usize::MAX).is_none());
        assert_eq!(
            scope.raw_at("USER_CFLAGS", usize::MAX).unwrap(),
            "-DORDER=1 -DORDER=1"
        );
        for effect in [
            "USER_CFLAGS += $(PRIVATE_SCRIPT)",
            "USER_CPPFLAGS := ${PRIVATE_XML}",
            "ifdef PRIVATE_SCRIPT",
            "PRIVATE_XML := -DLEAK=1",
            "define PRIVATE_SCRIPT\n-DLEAK\nendef",
        ] {
            assert!(classify(&format!("{text}{effect}\n")).is_err(), "{effect}");
        }
    }

    #[test]
    fn only_exact_local_binding_shape_is_classified() {
        let valid = "$(top_builddir)/$(CUR_MESADIR)/main/dispatch.h: PRIVATE_SCRIPT := $(PYTHON) generator.py";
        for replacement in [
            "../escape.h",
            "main/*.h",
            "$(UNKNOWN)/a.h",
            "/absolute.h",
            "main/a.h other.h",
            "main/a.o",
        ] {
            assert!(classify(&valid.replace("main/dispatch.h", replacement)).is_err());
        }
        for replacement in [
            "PRIVATE_SCRIPT +=",
            "PRIVATE_SCRIPT =",
            "export PRIVATE_SCRIPT :=",
            "PRIVATE_SCRIPT$(SUFFIX) :=",
        ] {
            assert!(classify(&valid.replace("PRIVATE_SCRIPT :=", replacement)).is_err());
        }
        assert!(classify("recipe:\n\t$(PRIVATE_SCRIPT) $(PRIVATE_XML)\ndefine es-gen\n\t$(PRIVATE_SCRIPT) $(PRIVATE_XML)\nendef\n").unwrap().is_empty());
    }
}
