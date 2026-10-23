//! Complete-source interpretation must not inject engine coordinate aliases.

use super::*;
use aros_common::make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation};

fn namespace(extra: &[(&str, MakeVariableOrigin)]) -> ValidatedMakeNamespace {
    let mut entries = vec![
        MakeNamespaceEntry {
            name: ".VARIABLES".into(),
            origin: MakeVariableOrigin::Default,
        },
        MakeNamespaceEntry {
            name: "MAKE_VERSION".into(),
            origin: MakeVariableOrigin::Default,
        },
    ];
    entries.extend(extra.iter().map(|(name, origin)| MakeNamespaceEntry {
        name: (*name).into(),
        origin: *origin,
    }));
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries,
    })
    .unwrap()
}

fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/selected/source"),
        relative_dir: Path::new(""),
        top_directory: Path::new("/selected/build"),
        working_directory: Path::new("/selected/build"),
    }
}

#[test]
fn source_program_coordinates_are_source_values_not_preseeded_engine_aliases() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let ns = namespace(&[]);
    let mut program = MakeVariableProgram::new_source_program(&paths, &ns, &[]).unwrap();
    assert_eq!(
        program
            .evaluate("$(TOP)|$(SRCDIR)|$(CURDIR)|$(USE_MMU)", &paths)
            .unwrap(),
        "|||"
    );
    assert!(matches!(
        program.evaluate("$(wildcard /selected/source/*)", &paths),
        Err(MakeExprError::FilesystemAccessDisabled { .. })
    ));
    let source = concat!(
        "TOP := /declared/build\nSRCDIR := /declared/source\nCURDIR := owner\n",
        "export FROZEN := $(TOP)|$(SRCDIR)|$(CURDIR)\n",
        "TOP := /later/build\nexport CURRENT = $(TOP)|$(SRCDIR)|$(CURDIR)\n",
    );
    program
        .read_file("root.mk", source, &paths, &|_, _| Ok(None))
        .unwrap();
    let exports = program.explicit_export_values(&paths).unwrap();
    assert_eq!(exports["FROZEN"], "/declared/build|/declared/source|owner");
    assert_eq!(exports["CURRENT"], "/later/build|/declared/source|owner");
    assert_eq!(
        program.evaluate("$(abspath $(TOP)/leaf)", &paths).unwrap(),
        "/later/build/leaf"
    );

    // Legacy projection remains fail-closed; the new data interpreter is not
    // a switch that silently loosens its existing graph-admission boundary.
    let mut projected =
        MakeVariableProgram::new_with_namespace(&TargetContext::default(), &[], &paths, &ns)
            .unwrap();
    assert!(projected
        .read_file("root.mk", source, &paths, &|_, _| Ok(None))
        .is_err());
    assert!(projected.events().is_empty());
    assert_eq!(
        projected.evaluate("$(TOP)", &paths).unwrap(),
        "${AROS_BUILD_DIR}"
    );
}

#[test]
fn source_coordinate_initial_values_keep_environment_and_command_line_priorities() {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let ns = namespace(&[
        ("TOP", MakeVariableOrigin::CommandLine),
        ("SRCDIR", MakeVariableOrigin::Environment),
    ]);
    let initial = [
        MakeInitialValue {
            name: "TOP",
            value: "/cli/root",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::CommandLine,
        },
        MakeInitialValue {
            name: "SRCDIR",
            value: "/environment/source",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        },
    ];
    let source = concat!(
        "TOP := /file/root\nSRCDIR := /file/source\nCURDIR := leaf\n",
        "WILDCARD = declared_$(1)\nCUSTOM := $(call WILDCARD,one)\n",
        "CAPTURE := $(TOP)|$(SRCDIR)|$(CURDIR)\n",
        "undefine TOP\nundefine CURDIR\nCURDIR ?= recreated\n",
        "probe:\n",
    );
    let mut program = MakeVariableProgram::new_source_program(&paths, &ns, &initial).unwrap();
    program
        .read_file("root.mk", source, &paths, &|_, _| Ok(None))
        .unwrap();
    let expression = "$(CAPTURE)|$(TOP)|$(CURDIR)|$(CUSTOM)";
    let result = program.evaluate(expression, &paths).unwrap();
    assert_eq!(
        result,
        "/cli/root|/file/source|leaf|/cli/root|recreated|declared_one"
    );
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let query = |top: &str| {
        let mut child = Command::new(make)
            .args(["-rR", "--no-print-directory", "-s", "-f", "-", "probe"])
            .arg(format!("TOP={top}"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("SRCDIR", "/environment/source")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let text = format!("{source}$(info AROS_TEST_COORDINATES={expression})\n");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "synthetic source-coordinate query failed"
        );
        let text = String::from_utf8(output.stdout).unwrap();
        let rows = text
            .lines()
            .filter_map(|line| line.strip_prefix("AROS_TEST_COORDINATES="))
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 1);
        rows[0].to_owned()
    };
    assert_eq!(query("/cli/root"), result);
    assert_ne!(
        query("/changed/root"),
        result,
        "changed CLI input must affect the native observation"
    );
    assert_eq!(
        program.state.origins.get("TOP"),
        Some(&MakeVariableOrigin::CommandLine)
    );
    assert_eq!(
        program.state.origins.get("SRCDIR"),
        Some(&MakeVariableOrigin::File)
    );
}

#[test]
fn source_program_does_not_adopt_foreign_includes_or_changed_execution_paths() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let ns = namespace(&[]);
    let mut program = MakeVariableProgram::new_source_program(&paths, &ns, &[]).unwrap();
    let result = program.read_file(
        "root.mk",
        "TOP := /foreign\ninclude $(TOP)/config.mk\n",
        &paths,
        &|name, _| {
            assert_eq!(name, "/foreign/config.mk");
            Err("not in independently selected input closure".into())
        },
    );
    assert!(result.is_err());
    assert!(program.events().is_empty());
    assert_eq!(program.evaluate("$(TOP)", &paths).unwrap(), "");
    let changed = MakeProgramPaths {
        top_directory: Path::new("/different"),
        ..paths
    };
    assert!(program.evaluate("$(TOP)", &changed).is_err());
}
