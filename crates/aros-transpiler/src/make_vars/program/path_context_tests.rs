//! TOP is the build root; working_directory is GNU Make's actual process cwd.

use super::{MakeProgramPaths, MakeVariableProgram};
use crate::{dirs::DirVars, parser::TargetContext};
use std::path::Path;

const SOURCE_ROOT: &str = "/synthetic/source";
const TOP_DIRECTORY: &str = "/synthetic/build";
const RELATIVE_DIRECTORY: &str = "nested/owner";
const WORKING_DIRECTORY: &str = "/synthetic/build/nested/owner";

fn paths<'a>(
    dirs: &'a DirVars,
    top_directory: &'a Path,
    working_directory: &'a Path,
) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new(SOURCE_ROOT),
        relative_dir: Path::new(RELATIVE_DIRECTORY),
        top_directory,
        working_directory,
    }
}

fn target(top: &str) -> TargetContext {
    let mut target = TargetContext::default();
    target
        .make_variables
        .insert("SRCDIR".into(), SOURCE_ROOT.into());
    target.make_variables.insert("TOP".into(), top.into());
    target
        .make_variables
        .insert("CURDIR".into(), RELATIVE_DIRECTORY.into());
    target
}

#[test]
fn top_stays_the_build_root_while_abspath_uses_the_actual_working_directory() {
    let dirs = DirVars::default();
    let paths = paths(
        &dirs,
        Path::new(TOP_DIRECTORY),
        Path::new(WORKING_DIRECTORY),
    );
    let mut program = MakeVariableProgram::new_with_paths(&target(TOP_DIRECTORY), &[], &paths)
        .expect("separate TOP and process cwd are valid bound coordinates");

    assert_eq!(
        program
            .evaluate("$(SRCDIR)|$(TOP)|$(CURDIR)", &paths)
            .unwrap(),
        "${AROS_SOURCE_DIR}|${AROS_BUILD_DIR}|nested/owner"
    );
    assert_eq!(
        program
            .evaluate("$(abspath .)|$(abspath ../..)", &paths)
            .unwrap(),
        "/synthetic/build/nested/owner|/synthetic/build"
    );
    assert_eq!(
        program
            .evaluate("$(abspath $(TOP)/peer/.. .)", &paths)
            .unwrap(),
        "/synthetic/build /synthetic/build/nested/owner"
    );

    let working_directory_as_top = target(WORKING_DIRECTORY);
    assert!(MakeVariableProgram::new_with_paths(&working_directory_as_top, &[], &paths).is_err());
}

#[test]
fn top_and_working_directory_are_independently_bound_after_initialization() {
    let dirs = DirVars::default();
    let top = Path::new(TOP_DIRECTORY);
    let cwd = Path::new(WORKING_DIRECTORY);
    let bound = paths(&dirs, top, cwd);
    let mut program = MakeVariableProgram::new_with_paths(&target(TOP_DIRECTORY), &[], &bound)
        .expect("initial coordinates are consistent");

    let changed_top = paths(&dirs, Path::new("/different/build"), cwd);
    assert!(program.evaluate("$(TOP)", &changed_top).is_err());
    assert!(program
        .execute("CHANGED_TOP := yes", 1, &changed_top)
        .is_err());

    let changed_cwd = paths(&dirs, top, Path::new("/synthetic/build/nested/other"));
    assert!(program.evaluate("$(abspath .)", &changed_cwd).is_err());
    assert!(program
        .execute("CHANGED_CWD := $(abspath .)", 2, &changed_cwd)
        .is_err());
    assert!(program.events().is_empty());

    program
        .execute("CAPTURED_CWD := $(abspath .)", 3, &bound)
        .unwrap();
    assert_eq!(
        program.evaluate("$(CAPTURED_CWD)", &bound).unwrap(),
        WORKING_DIRECTORY
    );
    assert_eq!(program.events().len(), 1);
}

#[test]
fn independent_top_and_cwd_match_a_print_only_gnu_make_query() {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let top = root.join("build with spaces");
    let source = root.join("source with spaces");
    let relative = Path::new("nested/owner");
    let build_cwd = top.join(relative);
    let source_cwd = source.join(relative);
    std::fs::create_dir_all(&build_cwd).unwrap();
    std::fs::create_dir_all(&source_cwd).unwrap();
    let dirs = DirVars::default();
    let expression =
        "$(abspath $(TOP)/peer/..)|$(abspath $(SRCDIR)/../unit)|$(abspath .)|$(abspath ../..)";
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let query = |bound_top: &Path, cwd: &Path| {
        // Only this synthetic expression is printed. No Source recipe,
        // environment enumeration or object compilation is involved.
        let text = format!("$(info AROS_TEST_PATHS={expression})\n.PHONY: probe\nprobe:\n");
        let mut child = Command::new(make)
            .args(["-rR", "--no-print-directory", "-s", "-f", "-", "probe"])
            .arg(format!("TOP={}", bound_top.display()))
            .arg(format!("SRCDIR={}", source.display()))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .current_dir(cwd)
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
        assert!(output.status.success(), "synthetic path query failed");
        let output = String::from_utf8(output.stdout).unwrap();
        let rows: Vec<_> = output
            .lines()
            .filter_map(|line| line.strip_prefix("AROS_TEST_PATHS="))
            .collect();
        assert_eq!(rows.len(), 1);
        rows[0].to_owned()
    };
    for cwd in [&build_cwd, &source_cwd] {
        let paths = MakeProgramPaths {
            dirs: &dirs,
            source_root: &source,
            relative_dir: relative,
            top_directory: &top,
            working_directory: cwd,
        };
        let mut target = TargetContext::default();
        target
            .make_variables
            .insert("TOP".into(), top.to_str().unwrap().into());
        target
            .make_variables
            .insert("SRCDIR".into(), source.to_str().unwrap().into());
        let mut program = MakeVariableProgram::new_with_paths(&target, &[], &paths).unwrap();
        let observed = query(&top, cwd);
        assert_eq!(program.evaluate(expression, &paths).unwrap(), observed);
        assert_ne!(
            query(cwd, cwd),
            observed,
            "TOP=cwd counter must change the measurement"
        );
        target
            .make_variables
            .insert("TOP".into(), cwd.to_str().unwrap().into());
        assert!(MakeVariableProgram::new_with_paths(&target, &[], &paths).is_err());
    }
}

#[test]
fn relative_build_root_or_process_cwd_is_not_an_execution_binding() {
    let dirs = DirVars::default();
    for (top, cwd) in [
        ("relative/build", WORKING_DIRECTORY),
        (TOP_DIRECTORY, "relative/cwd"),
    ] {
        let paths = paths(&dirs, Path::new(top), Path::new(cwd));
        assert!(
            MakeVariableProgram::new_with_paths(&TargetContext::default(), &[], &paths).is_err()
        );
    }
}
