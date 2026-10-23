use super::*;
use crate::compatibility::{
    prepare_host_tool_closure, CompatibilityHostTool, HostToolClosureRequest,
};
use std::{fs, os::unix::fs::PermissionsExt as _, process::Command, time::Duration};

#[test]
fn original_command_preserves_dynamic_body_and_closes_environment() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().canonicalize().unwrap();
    let host = closure(temp.path(), "sh", Path::new("/bin/sh"));
    let inherited = BTreeMap::from([("PATH".into(), host.root.to_str().unwrap().to_owned())]);
    let mut exports = inherited.clone();
    exports.insert("SELECTED_VALUE".into(), "quoted value".into());
    let shell = BoundRecipeShell::bind(
        Path::new("/bin/sh"),
        "/bin/sh",
        "-c",
        &host,
        &inherited,
        &exports,
    )
    .unwrap();
    let body = "for object in one two; do printf '%s|' \"$object:$SELECTED_VALUE\"; done; test -z \"${HOME+x}\"";
    let selected = selected_environment(&host);
    let invocation =
        original_invocation(&shell, &directory, body, selected.clone(), &exports).unwrap();
    invocation.revalidate().unwrap();
    let mut command = invocation.command();
    assert_eq!(command.get_program(), "/bin/sh");
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["-c", body]);
    assert_eq!(command.get_current_dir(), Some(directory.as_path()));
    let output =
        aros_common::run_output_with_timeout(&mut command, 4096, Duration::from_secs(5)).unwrap();
    assert!(output.status.success() && !output.timed_out);
    assert_eq!(
        output.stdout.exact_bytes().unwrap(),
        b"one:quoted value|two:quoted value|"
    );
    let mut failed = original_invocation(&shell, &directory, "exit 19", selected, &exports)
        .unwrap()
        .command();
    let output =
        aros_common::run_output_with_timeout(&mut failed, 4096, Duration::from_secs(5)).unwrap();
    assert_eq!(output.status.code(), Some(19));
}

#[test]
fn original_command_does_not_resurrect_unexported_preparation_variables() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().canonicalize().unwrap();
    let host = closure(temp.path(), "sh", Path::new("/bin/sh"));
    let mut selected = selected_environment(&host);
    selected
        .variables
        .insert("REMOVED_BY_SOURCE".into(), "preparation-value".into());
    let exports = BTreeMap::from([("PATH".into(), host.root.to_str().unwrap().to_owned())]);
    let shell = BoundRecipeShell::bind(
        Path::new("/bin/sh"),
        "/bin/sh",
        "-c",
        &host,
        &selected.variables,
        &exports,
    )
    .unwrap();
    let invocation = original_invocation(
        &shell,
        &directory,
        "test -z \"${REMOVED_BY_SOURCE+x}\" && printf preserved",
        selected.clone(),
        &exports,
    )
    .unwrap();
    let output = aros_common::run_output_with_timeout(
        &mut invocation.command(),
        4096,
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.exact_bytes().unwrap(), b"preserved");
    assert!(
        original_invocation(&shell, &directory, "exit 0", selected, &BTreeMap::new()).is_err(),
        "unexported PATH cannot silently enable the shell's ambient search default"
    );
}

#[test]
fn original_command_refuses_exported_path_startup_and_size_escapes() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().canonicalize().unwrap();
    let host = closure(temp.path(), "sh", Path::new("/bin/sh"));
    let inherited = BTreeMap::from([("PATH".into(), host.root.to_str().unwrap().to_owned())]);
    let empty = BTreeMap::new();
    let selected = selected_environment(&host);
    let shell = BoundRecipeShell::bind(
        Path::new("/bin/sh"),
        "/bin/sh",
        "-c",
        &host,
        &inherited,
        &empty,
    )
    .unwrap();
    for exports in [
        BTreeMap::from([("PATH".into(), "/bin:/usr/bin".into())]),
        BTreeMap::from([("ENV".into(), "/private/startup".into())]),
        BTreeMap::from([("SELECTED_VALUE".into(), "x".repeat(65536))]),
        BTreeMap::from([("SELECTED_VALUE".into(), "line\nbreak".into())]),
    ] {
        assert!(
            original_invocation(&shell, &directory, "exit 0", selected.clone(), &exports).is_err()
        );
    }
    assert!(original_invocation(&shell, &directory, "exit\0 0", selected.clone(), &empty).is_err());
    // Repeating the exact selected PATH is data preservation, not an escape.
    assert!(original_invocation(&shell, &directory, "exit 0", selected, &inherited).is_ok());
}

fn selected_environment(
    host: &HostToolClosure,
) -> super::super::environment::ResolvedProbeEnvironment {
    super::super::environment::resolve(&super::super::CompatibilityEnvironment::SealedHostTools {
        variables: BTreeMap::from([("PATH".into(), "/nonexistent".into())]),
        host_tools: host.clone(),
    })
    .unwrap()
}

fn closure(root: &Path, role: &str, program: &Path) -> HostToolClosure {
    prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: root.join("closure"),
        tools: vec![CompatibilityHostTool {
            name: role.into(),
            program: program.into(),
        }],
    })
    .unwrap()
}

#[test]
fn selected_shell_preserves_source_spelling_and_supported_command_forms() {
    let temp = tempfile::tempdir().unwrap();
    let host = closure(temp.path(), "sh", Path::new("/bin/sh"));
    let empty = BTreeMap::new();
    for flags in ["-c", "-ec"] {
        let binding = BoundRecipeShell::bind(
            Path::new("/bin/sh"),
            "/bin/sh",
            flags,
            &host,
            &empty,
            &empty,
        )
        .unwrap();
        assert_eq!(binding.spelling, Path::new("/bin/sh"));
        binding
            .revalidate("/bin/sh", flags, &host, &empty, &empty)
            .unwrap();
        let mut command = Command::new(&binding.spelling);
        command
            .env_clear()
            .args([flags, "printf '%s' 'literal space' 'quote'\"'\"'value'"]);
        let result =
            aros_common::run_output_with_timeout(&mut command, 4096, Duration::from_secs(5))
                .unwrap();
        assert!(result.status.success() && !result.timed_out);
        assert_eq!(
            result.stdout.exact_bytes().unwrap(),
            b"literal spacequote'value"
        );
        let mut failing = Command::new(&binding.spelling);
        failing.env_clear().args([flags, "exit 17"]);
        let failure =
            aros_common::run_output_with_timeout(&mut failing, 4096, Duration::from_secs(5))
                .unwrap();
        assert_eq!(failure.status.code(), Some(17));
    }
}

#[test]
fn shell_binding_rejects_unselected_program_flags_and_startup_controls() {
    let temp = tempfile::tempdir().unwrap();
    let host = closure(temp.path(), "sh", Path::new("/bin/sh"));
    let empty = BTreeMap::new();
    for flags in ["", "-c ", "-e -c", "--rcfile private -c", "-ic", "-xc"] {
        assert!(BoundRecipeShell::bind(
            Path::new("/bin/sh"),
            "/bin/sh",
            flags,
            &host,
            &empty,
            &empty
        )
        .is_err());
    }
    for shell in [
        "sh",
        "/bin/false",
        "/bin/../bin/sh",
        "/bin/./sh",
        "/bin//sh",
    ] {
        assert!(
            BoundRecipeShell::bind(Path::new("/bin/sh"), shell, "-c", &host, &empty, &empty)
                .is_err()
        );
    }
    for name in [
        "ENV",
        "BASH_ENV",
        "IFS",
        "CDPATH",
        "SHELLOPTS",
        "BASHOPTS",
        "BASH_COMPAT",
        "POSIXLY_CORRECT",
        "GLOBIGNORE",
        "BASH_FUNC_printf%%",
        "LD_PRELOAD",
        "DYLD_INSERT_LIBRARIES",
    ] {
        let changed = BTreeMap::from([(name.into(), "private-unmodeled-control".into())]);
        for (inherited, exports) in [(&changed, &empty), (&empty, &changed)] {
            let error = BoundRecipeShell::bind(
                Path::new("/bin/sh"),
                "/bin/sh",
                "-c",
                &host,
                inherited,
                exports,
            )
            .err()
            .unwrap();
            assert!(!error.to_string().contains("private-unmodeled-control"));
        }
    }
    let binding =
        BoundRecipeShell::bind(Path::new("/bin/sh"), "/bin/sh", "-c", &host, &empty, &empty)
            .unwrap();
    assert!(binding
        .revalidate("/bin/sh", "-ec", &host, &empty, &empty)
        .is_err());
    let inactive = BTreeMap::from([
        ("ENV".into(), String::new()),
        ("BASH_ENV".into(), String::new()),
        ("BASH_USER_LABEL".into(), "unrelated-preserved-data".into()),
    ]);
    for (inherited, exports) in [(&inactive, &empty), (&empty, &inactive)] {
        assert!(BoundRecipeShell::bind(
            Path::new("/bin/sh"),
            "/bin/sh",
            "-c",
            &host,
            inherited,
            exports
        )
        .is_ok());
    }
    let other = tempfile::tempdir().unwrap();
    let no_shell = closure(other.path(), "bash", Path::new("/bin/sh"));
    assert!(BoundRecipeShell::bind(
        Path::new("/bin/sh"),
        "/bin/sh",
        "-c",
        &no_shell,
        &empty,
        &empty
    )
    .is_err());
}

#[test]
fn shell_binding_refuses_scripts_and_revalidates_selected_native_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("unselected-interpreter");
    fs::write(&script, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let host = closure(temp.path(), "sh", &script);
    let empty = BTreeMap::new();
    let script_text = script.to_str().unwrap();
    let error = BoundRecipeShell::bind(&script, script_text, "-c", &host, &empty, &empty)
        .err()
        .unwrap();
    assert!(error.to_string().contains("not a script"));

    let native = tempfile::tempdir().unwrap();
    let program = native.path().join("selected-image");
    fs::copy("/bin/sh", &program).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    let host = closure(native.path(), "sh", &program);
    let program_text = program.to_str().unwrap();
    let binding =
        BoundRecipeShell::bind(&program, program_text, "-c", &host, &empty, &empty).unwrap();
    let alias = native.path().join("different-argv-zero");
    std::os::unix::fs::symlink(&program, &alias).unwrap();
    assert!(BoundRecipeShell::bind(
        &program,
        alias.to_str().unwrap(),
        "-c",
        &host,
        &empty,
        &empty
    )
    .is_err());
    fs::write(&program, b"changed image\n").unwrap();
    assert!(binding
        .revalidate(program_text, "-c", &host, &empty, &empty)
        .is_err());
}
