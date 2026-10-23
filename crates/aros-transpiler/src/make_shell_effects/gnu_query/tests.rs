use super::{GnuShellQuery, GnuShellQueryResult, LiteralQueryVariable, QUERY_GOAL};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn make_binary() -> &'static str {
    if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    }
}

fn write_bundle(bundle: &GnuShellQuery, root: &Path) {
    for (filename, bytes) in bundle.files() {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(filename))
            .unwrap();
        file.write_all(bytes).unwrap();
    }
}

fn run_query(bundle: &GnuShellQuery) -> GnuShellQueryResult {
    let version = Command::new(make_binary())
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success());
    let version = String::from_utf8(version.stdout).unwrap();
    let version = version
        .lines()
        .find_map(|line| line.strip_prefix("GNU Make "))
        .unwrap();
    let mut parts = version.split('.');
    let major = parts.next().unwrap().parse::<u32>().unwrap();
    let minor = parts.next().unwrap().parse::<u32>().unwrap();
    assert!(
        (major, minor) >= (4, 2),
        "GNU Make >= 4.2 required for status"
    );
    let output = Command::new(make_binary())
        .args(["-r", "-s", "--no-print-directory", "-f"])
        .arg(bundle.makefile())
        .arg(QUERY_GOAL)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "literal query must complete");
    assert!(
        output.stdout.is_empty(),
        "query cannot leak its payload to stdout"
    );
    assert!(output.stderr.is_empty(), "query cannot emit a diagnostic");
    GnuShellQueryResult::decode(
        &std::fs::read(bundle.result_file()).unwrap(),
        &std::fs::read(bundle.status_file()).unwrap(),
    )
    .unwrap()
}

#[test]
fn literal_payloads_do_not_become_make_syntax_and_preserve_exit_status() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let value = "  literal#$(error NOT_A_MAKE_EXPRESSION)\\\nnext $HOME $$ trailing \n\n";
    let command =
        "printf '%s\\n' \"$VALUE\"; printf '%s\\n' '#$(still-shell-data)' 'tail'; exit 23\n\n";
    let bundle = GnuShellQuery::render(
        &root,
        command,
        &[
            LiteralQueryVariable {
                name: "SHELL",
                value: Some("/bin/sh"),
                export: None,
            },
            LiteralQueryVariable {
                name: ".SHELLFLAGS",
                value: Some("-c"),
                export: None,
            },
            LiteralQueryVariable {
                name: "VALUE",
                value: Some(value),
                export: Some(true),
            },
        ],
    )
    .unwrap();
    write_bundle(&bundle, &root);
    let result = run_query(&bundle);
    let direct = Command::new("/bin/sh")
        .args(["-c", command])
        .env_clear()
        .env("VALUE", value)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(direct.status.code(), Some(23));
    let stdout = String::from_utf8(direct.stdout).unwrap();
    assert_eq!(
        result,
        GnuShellQueryResult {
            value: crate::make_shell_data::fold_stdout(&stdout),
            status: 23,
        }
    );
    assert!(result.value.contains("$(error NOT_A_MAKE_EXPRESSION)"));
    assert!(result.value.contains("$HOME $$"));
    let syntax = std::fs::read_to_string(bundle.makefile()).unwrap();
    assert!(!syntax.contains("NOT_A_MAKE_EXPRESSION"));
    assert!(!syntax.contains("printf"));
}

#[test]
fn explicit_export_and_unexport_controls_are_consumed_from_input() {
    for export in [true, false] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let bundle = GnuShellQuery::render(
            &root,
            "if test \"${VALUE+x}\" = x; then printf '%s' \"$VALUE\"; else printf absent; fi",
            &[
                LiteralQueryVariable {
                    name: "SHELL",
                    value: Some("/bin/sh"),
                    export: None,
                },
                LiteralQueryVariable {
                    name: ".SHELLFLAGS",
                    value: Some("-c"),
                    export: None,
                },
                LiteralQueryVariable {
                    name: "VALUE",
                    value: Some("selected"),
                    export: Some(export),
                },
            ],
        )
        .unwrap();
        write_bundle(&bundle, &root);
        assert_eq!(
            run_query(&bundle),
            GnuShellQueryResult {
                value: if export { "selected" } else { "absent" }.into(),
                status: 0,
            }
        );
    }
}

#[test]
fn empty_output_and_failed_command_are_not_make_process_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let bundle = GnuShellQuery::render(&root, "exit 255", &[]).unwrap();
    write_bundle(&bundle, &root);
    assert_eq!(
        run_query(&bundle),
        GnuShellQueryResult {
            value: String::new(),
            status: 255
        }
    );
}

#[test]
fn engine_export_directive_does_not_replace_its_value() {
    for suppress in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let variables = if suppress {
            vec![LiteralQueryVariable {
                name: "MAKEFLAGS",
                value: None,
                export: Some(false),
            }]
        } else {
            Vec::new()
        };
        let bundle = GnuShellQuery::render(&root,
            "if test \"${MAKEFLAGS+x}\" = x; then printf '%s' \"$MAKEFLAGS\"; else printf absent; fi",
            &variables,
        ).unwrap();
        write_bundle(&bundle, &root);
        let result = run_query(&bundle);
        assert_eq!(result.status, 0);
        if suppress {
            assert_eq!(result.value, "absent");
            let syntax = std::fs::read_to_string(bundle.makefile()).unwrap();
            assert!(syntax.contains("unexport MAKEFLAGS\n"));
            assert!(!syntax.contains("override MAKEFLAGS"));
            assert_eq!(bundle.files().len(), 2);
        } else {
            assert_ne!(result.value, "absent");
            assert!(result.value.contains("--no-print-directory"));
        }
    }
}

#[test]
fn syntax_coordinates_and_payloads_fail_closed() {
    assert!(GnuShellQuery::render(
        Path::new("/tmp/query"),
        "true",
        &[LiteralQueryVariable {
            name: "A",
            value: None,
            export: None
        },]
    )
    .is_err());
    for path in [
        "relative",
        "/tmp/has space",
        "/tmp/$x",
        "/tmp/a,b",
        "/tmp/a)b",
        "/tmp/../x",
    ] {
        assert!(GnuShellQuery::render(Path::new(path), "true", &[]).is_err());
    }
    for name in ["", "A B", "A:=x", "A\nX", "$(error x)", "A#"] {
        assert!(GnuShellQuery::render(
            Path::new("/tmp/query"),
            "true",
            &[LiteralQueryVariable {
                name,
                value: Some("literal"),
                export: None
            },]
        )
        .is_err());
    }
    assert!(GnuShellQuery::render(Path::new("/tmp/query"), "x\0y", &[]).is_err());
    assert!(GnuShellQuery::render(
        Path::new("/tmp/query"),
        "true",
        &[LiteralQueryVariable {
            name: "A",
            value: Some("x\0y"),
            export: None
        },]
    )
    .is_err());
    assert!(GnuShellQuery::render(
        Path::new("/tmp/query"),
        "true",
        &[
            LiteralQueryVariable {
                name: "A",
                value: Some("one"),
                export: None
            },
            LiteralQueryVariable {
                name: "A",
                value: Some("two"),
                export: Some(true)
            },
        ]
    )
    .is_err());
    assert!(GnuShellQuery::render(
        Path::new("/tmp/query"),
        &"x".repeat(super::MAX_QUERY_BYTES),
        &[]
    )
    .is_err());
}

#[test]
fn decode_requires_exact_framing_and_canonical_status() {
    assert_eq!(
        GnuShellQueryResult::decode(b"\n", b"0\n").unwrap(),
        GnuShellQueryResult {
            value: String::new(),
            status: 0
        }
    );
    assert!(GnuShellQueryResult::decode(b"one\n\n", b"17\n").is_err());
    for status in [
        b"".as_slice(),
        b"0",
        b"00\n",
        b"+1\n",
        b"256\n",
        b"1\n\n",
        b"x\n",
    ] {
        assert!(GnuShellQueryResult::decode(b"ok\n", status).is_err());
    }
    for value in [b"".as_slice(), b"no-frame", b"nul\0\n", b"\xff\n"] {
        assert!(GnuShellQueryResult::decode(value, b"0\n").is_err());
    }
    let private = GnuShellQueryResult {
        value: "PRIVATE-QUERY-VALUE".into(),
        status: 23,
    };
    let debug = format!("{private:?}");
    assert!(!debug.contains("PRIVATE-QUERY-VALUE"));
    assert!(debug.contains("status: 23"));
}
