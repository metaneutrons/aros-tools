//! Explicit original Root host bootstrap experiment, not a completed owner.
//! Do not promote this into production without pre-seal stage/receipt binding.

use super::*;

const DEFAULT_FENCE: &str =
    ".DEFAULT:\n.DEFAULT:\n\t$(error AROS_HOST_BOOTSTRAP_UNEXPECTED_DEFAULT)\n";

fn admit_paths(
    paths: &BTreeMap<String, String>,
    build: &Path,
    hosts: &Path,
) -> Result<PathBuf, &'static str> {
    let executable = PathBuf::from(paths.get("MMAKE").ok_or("missing host output")?);
    let relative = executable
        .strip_prefix(build)
        .map_err(|_| "host output escaped build")?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        || fs::symlink_metadata(&executable).is_ok()
    {
        return Err("host output is not a fresh build-local path");
    }
    let compiler = LiteralMakeChildCommand::parse(paths.get("HOST_CC").ok_or("missing host CC")?)
        .map_err(|_| "host compiler command is not literal")?;
    if compiler.program() != hosts.join("cc").to_str().ok_or("host path is not UTF-8")?
        || !compiler.environment_assignments().is_empty()
    {
        return Err("host compiler differs from sealed role");
    }
    let make = LiteralMakeChildCommand::parse(paths.get("HOST_MAKE").ok_or("missing host Make")?)
        .map_err(|_| "host Make command is not literal")?;
    if !make.arguments().is_empty()
        || !make.environment_assignments().is_empty()
        || !matches!(make.program(), "make" | "gmake")
    {
        return Err("host Make differs from sealed role");
    }
    Ok(executable)
}

pub(super) fn bootstrap_original_host_tool(
    request: &MesaPreflightRequest<'_>,
    state: &CaptureState<'_>,
    output: &Path,
    paths: &BTreeMap<String, String>,
    old_files: &[String],
    read_inputs: &[(PathBuf, Sha256Digest)],
    cancellation: &CancellationToken,
) -> Vec<u8> {
    let build = request.output_root.join("configure");
    let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = request.environment else {
        panic!("original host bootstrap requires the existing sealed closure");
    };
    let executable = admit_paths(paths, &build, &host_tools.root)
        .expect("selected original host bootstrap paths must be admitted before launch");
    assert!(fs::canonicalize(executable.parent().unwrap())
        .unwrap()
        .starts_with(&build));
    admit_recursive_make(
        host_tools,
        &paths["HOST_MAKE"],
        state.configured_invocation().program_sha256(),
    )
    .unwrap();
    for relative in [
        "tools/MetaMake/configure.in",
        "tools/MetaMake/config.h.in",
        "tools/MetaMake/Makefile.in",
        "tools/genmf/genmf.py",
    ] {
        read_bytes(&request.source_root.join(relative)).unwrap();
    }
    read_bytes(&PathBuf::from(&paths["GENMFSCRIPT"])).unwrap();
    // The caller already observed Root through a fatal second input BEFORE
    // Make's include-remake phase, checked CROSSTOOLS_TARGET empty, and fenced
    // all observed inputs plus the explicitly absent optional inputs. Preserve
    // that admission rather than calling `make -n` or a broad tools goal.
    assert!(!old_files.is_empty());
    let invocation = state.configured_invocation();
    invocation.revalidate().unwrap();
    validate_selection(request, cancellation).unwrap();
    let fence = output.join("host-bootstrap-default-fence.mk");
    write(&fence, DEFAULT_FENCE.as_bytes()).unwrap();
    assert_eq!(read_bytes(&fence).unwrap(), DEFAULT_FENCE.as_bytes());
    let mut command = Command::new(invocation.program());
    command
        .env_clear()
        .envs(&invocation.environment().variables)
        .current_dir(&build)
        .args([
            "--silent",
            "--no-builtin-rules",
            "--file",
            "Makefile",
            "--file",
        ])
        .arg(&fence)
        .arg(format!("--old-file={}", fence.display()))
        .args(old_files)
        .arg("mmake");
    for (path, digest) in read_inputs {
        pinned(path, digest);
    }
    execute(
        &mut command,
        output,
        "original-host-metamake-build",
        cancellation,
        0,
        state.execution_timeout(),
    );
    for (path, digest) in read_inputs {
        pinned(path, digest);
    }
    invocation.revalidate().unwrap();
    validate_selection(request, cancellation).unwrap();
    assert_eq!(read_bytes(&fence).unwrap(), DEFAULT_FENCE.as_bytes());
    let (digest, _) = super::super::super::measure_executable(&executable).unwrap();
    let bytes = read_bytes(&executable).unwrap();
    assert_eq!(sha256_bytes(&bytes), digest);
    write(
        &output.join("host-metamake-identity.json"),
        &serde_json::to_vec(&serde_json::json!({
            "qualification":"original-source-host-bootstrap-with-default-fence-experiment-only",
            "program":executable,"sha256":digest,"goal":"mmake",
            "preseal":matches!(state, CaptureState::Prepared(_)),
            "unbound_output_refused_before_completion":true,
        }))
        .unwrap(),
    )
    .unwrap();
    bytes
}

fn admit_recursive_make(
    hosts: &super::super::super::HostToolClosure,
    role: &str,
    expected: &Sha256Digest,
) -> Result<(), &'static str> {
    hosts.revalidate().map_err(|_| "host closure changed")?;
    let selected = hosts
        .tools
        .get(role)
        .ok_or("recursive Make role is absent")?;
    if selected.sha256 != *expected {
        return Err("recursive Make differs from selected driver");
    }
    Ok(())
}

#[test]
fn recursive_make_uses_held_closure_identity_and_refuses_foreign_or_replaced_alias() {
    use super::super::super::{
        prepare_host_tool_closure, CompatibilityHostTool, HostToolClosureRequest,
    };
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let program = root.join("selected-make");
    fs::write(&program, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let hosts = prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: root.join("hosts"),
        tools: ["make", "gmake"]
            .into_iter()
            .map(|name| CompatibilityHostTool {
                name: name.into(),
                program: program.clone(),
            })
            .collect(),
    })
    .unwrap();
    let expected = hosts.tools["make"].sha256.clone();
    assert!(admit_recursive_make(&hosts, "gmake", &expected).is_ok());
    assert!(admit_recursive_make(&hosts, "gmake", &sha256_bytes(b"foreign")).is_err());
    assert!(admit_recursive_make(&hosts, "absent", &expected).is_err());
    fs::remove_file(hosts.root.join("gmake")).unwrap();
    std::os::unix::fs::symlink(root.join("foreign-make"), hosts.root.join("gmake")).unwrap();
    assert!(admit_recursive_make(&hosts, "gmake", &expected).is_err());
}

#[test]
fn default_fence_preserves_explicit_host_goal_and_stops_fallback_without_effects() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fs::write(root.join("Makefile"), b".DEFAULT: dangerous\n\t@printf forbidden > forbidden\n.PHONY: host\nhost:\n\t@printf admitted > admitted\ndangerous:\n\t@printf forbidden > forbidden-dependency\n").unwrap();
    fs::write(root.join("fence.mk"), DEFAULT_FENCE).unwrap();
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    for (goal, success) in [("host", true), ("unknown-target", false)] {
        let mut command = Command::new(make);
        command.current_dir(&root).args([
            "--no-builtin-rules",
            "-f",
            "Makefile",
            "-f",
            "fence.mk",
            goal,
        ]);
        let result =
            aros_common::run_output_with_timeout(&mut command, 4096, Duration::from_secs(5))
                .unwrap();
        assert!(!result.timed_out);
        assert_eq!(result.status.success(), success);
        if !success {
            assert!(result
                .stderr
                .rendered_lossy()
                .contains("AROS_HOST_BOOTSTRAP_UNEXPECTED_DEFAULT"));
        }
        assert!(!root.join("forbidden").exists());
        assert!(!root.join("forbidden-dependency").exists());
    }
    assert_eq!(fs::read(root.join("admitted")).unwrap(), b"admitted");
}

#[test]
fn original_host_bootstrap_path_admission_has_positive_and_effect_free_counters() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let build = root.join("build");
    let hosts = root.join("hosts");
    let mut paths = BTreeMap::from([
        (
            "MMAKE".into(),
            build.join("tools/mmake").to_str().unwrap().into(),
        ),
        (
            "HOST_CC".into(),
            format!("{} -std=gnu23", hosts.join("cc").display()),
        ),
        ("HOST_MAKE".into(), "gmake".into()),
    ]);
    assert_eq!(
        admit_paths(&paths, &build, &hosts).unwrap(),
        build.join("tools/mmake")
    );
    for (name, value) in [
        (
            "MMAKE",
            root.join("foreign/mmake").to_str().unwrap().to_owned(),
        ),
        (
            "MMAKE",
            build.join("../escaped").to_str().unwrap().to_owned(),
        ),
        ("HOST_CC", "/target-toolchain/bin/clang".to_owned()),
        ("HOST_CC", "env TARGET=bad /host/cc".to_owned()),
        ("HOST_MAKE", "gmake all".to_owned()),
        ("HOST_MAKE", "unknown".to_owned()),
    ] {
        let original = paths.insert(name.into(), value).unwrap();
        assert!(admit_paths(&paths, &build, &hosts).is_err());
        paths.insert(name.into(), original);
    }
    assert!(!build.exists());
    fs::create_dir_all(build.join("tools")).unwrap();
    fs::write(
        build.join("tools/mmake"),
        b"existing must not be overwritten",
    )
    .unwrap();
    assert!(admit_paths(&paths, &build, &hosts).is_err());
    assert_eq!(
        fs::read(build.join("tools/mmake")).unwrap(),
        b"existing must not be overwritten"
    );
}
