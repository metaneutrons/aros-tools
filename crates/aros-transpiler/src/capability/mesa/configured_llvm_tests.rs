use super::*;
use std::fs;
use std::process::Command;

const RELATIVE: &str = "workbench/libs/mesa/libgalliumvm";

fn project(text: &str, relative: &str) -> Result<String, String> {
    project_version_format(
        text,
        &TargetContext::default(),
        &DirVars::default(),
        Path::new("/source"),
        Path::new(relative),
        &|_| None,
    )
}

#[test]
fn reviewed_format_matches_real_make_printf_and_freezes_assignment_time() {
    let temporary = tempfile::tempdir().unwrap();
    for (major, minor, expected) in [
        (11, 0, "0x0b00"),
        (18, 1, "0x1201"),
        (0, 0, "0x0000"),
        (255, 255, "0xffff"),
    ] {
        let source = format!("LLVM_VERSION_MAJOR := {major}\nLLVM_VERSION_MINOR := {minor}\n{VERSION_ASSIGNMENT}\nLLVM_VERSION_MAJOR := 1\n");
        let projected = project(&source, RELATIVE).unwrap();
        assert!(projected.contains(&format!("LLVM_VERSION_HEX := {expected}")));
        let path = temporary.path().join("probe.mk");
        let mut outputs = Vec::new();
        for text in [&source, &projected] {
            fs::write(
                &path,
                format!("{text}\n.PHONY: probe\nprobe:\n\t@printf '%s\\n' '$(LLVM_VERSION_HEX)'\n"),
            )
            .unwrap();
            let make = if cfg!(target_os = "macos") {
                "gmake"
            } else {
                "make"
            };
            let output = Command::new(make)
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .args(["--no-builtin-rules", "--no-builtin-variables", "-f"])
                .arg(&path)
                .arg("probe")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            outputs.push(String::from_utf8(output.stdout).unwrap());
        }
        assert_eq!(outputs[0], format!("{expected}\n"));
        assert_eq!(outputs[1], outputs[0]);
    }
}

#[test]
fn formatting_refuses_unmeasured_noncanonical_or_out_of_range_components() {
    for value in ["", "08", "-1", "+1", "256", "1 2", "0x0b", "$(UNKNOWN)"] {
        let source =
            format!("LLVM_VERSION_MAJOR := {value}\nLLVM_VERSION_MINOR := 0\n{VERSION_ASSIGNMENT}");
        assert!(project(&source, RELATIVE).is_err(), "{value}");
    }
    assert!(project(VERSION_ASSIGNMENT, RELATIVE).is_err());
}

#[test]
fn only_one_exact_active_top_level_assignment_in_reviewed_paths_is_projected() {
    let base = format!("LLVM_VERSION_MAJOR := 11\nLLVM_VERSION_MINOR := 0\n{VERSION_ASSIGNMENT}\n");
    for text in [
        base.replace("%02x%02x", "%x%x"),
        base.replace("printf", "other-command"),
        base.replace(
            "$(LLVM_VERSION_MINOR))",
            "$(LLVM_VERSION_MINOR); touch /unused/probe)",
        ),
        format!("{base}{VERSION_ASSIGNMENT}\n"),
        format!("define UNUSED\n{base}endef\n"),
        format!("define\tUNUSED\n{base}endef\n"),
        format!("override define UNUSED\n{base}endef\n"),
        format!("define .RECIPEPREFIX\nL\nendef\n{base}"),
        format!("private define .RECIPEPREFIX\nL\nendef\n{base}"),
        format!("define $(INDIRECT_NAME)\nL\nendef\n{base}"),
        format!(".RECIPEPREFIX := $(subst x,,x )\nunused:\n {VERSION_ASSIGNMENT}\n"),
        format!("unused:\n\t{VERSION_ASSIGNMENT}\n"),
        format!("ifeq (0,1)\n{base}endif\n"),
        format!("ifeq ($(UNKNOWN),1)\n{base}endif\n"),
    ] {
        assert!(project(&text, RELATIVE).is_err(), "{text}");
    }
    assert_eq!(project(&base, "unreviewed/path").unwrap(), base);
    assert!(project(&base, "workbench/libs/mesa/libllvmpipe").is_ok());
}
