//! Explicit opt-in native Root/MetaMake boundary experiment, not a new receipt.
//! It deliberately changes the fresh configure output after its sealed producer
//! was checked. That original CompletedMesaPreflight must then refuse reuse.

use super::*;
use aros_common::make_child_command::LiteralMakeChildCommand;
use serde::Deserialize;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[path = "mesa_preflight_host_bootstrap_tests.rs"]
mod host_bootstrap;
#[path = "mesa_preflight_root_observation_tests.rs"]
mod observation;

enum CaptureState<'a> {
    Completed(&'a CompletedMesaPreflight),
    Prepared(&'a preparation::PreparedMesaPreflight),
}

impl CaptureState<'_> {
    fn configuration(&self) -> &ValidatedMesaConfiguration {
        match self {
            Self::Completed(completed) => &completed.configuration,
            Self::Prepared(prepared) => &prepared.configured.initial_configuration,
        }
    }

    fn configured_invocation(&self) -> &invocation::InvocationContext {
        match self {
            Self::Completed(completed) => &completed.configured_invocation,
            Self::Prepared(prepared) => prepared.configured_invocation().unwrap(),
        }
    }

    fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        match self {
            Self::Completed(completed) => completed.revalidate(request, cancellation),
            Self::Prepared(prepared) => prepared.revalidate(request, cancellation),
        }
    }

    fn execution_timeout(&self) -> Duration {
        match self {
            // Historical postseal experiment has its own bounded child clock.
            Self::Completed(_) => Duration::from_secs(120),
            // The live preseal experiment must not renew the owner's clock.
            Self::Prepared(prepared) => prepared.remaining().unwrap().min(Duration::from_secs(120)),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    output: PathBuf,
    metamake: Option<PathBuf>,
    metamake_sha256: Option<Sha256Digest>,
    #[serde(default)]
    bootstrap_metamake: bool,
    python: PathBuf,
    python_sha256: Sha256Digest,
    #[serde(default)]
    native_observer: Option<NativeObserver>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeObserver {
    program: PathBuf,
    sha256: Sha256Digest,
    #[serde(default)]
    collection: bool,
}

fn check_observer(observer: &NativeObserver) {
    assert!(observer.program.is_absolute());
    assert_eq!(
        fs::canonicalize(&observer.program).unwrap(),
        observer.program
    );
    assert!(observer
        .program
        .to_str()
        .unwrap()
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/._-@".contains(&byte)));
    let (_, bytes) = measure_regular_file_bounded(&observer.program, 256 * 1024 * 1024)
        .unwrap()
        .unwrap();
    assert_eq!(sha256_bytes(&bytes), observer.sha256);
}

fn pinned(path: &Path, digest: &Sha256Digest) -> Vec<u8> {
    let (_, bytes) = measure_regular_file_bounded(path, 4 * 1024 * 1024)
        .unwrap()
        .unwrap();
    assert_eq!(
        &sha256_bytes(&bytes),
        digest,
        "selected capture program changed"
    );
    bytes
}

fn install_exact(path: &Path, bytes: &[u8], executable: bool) -> Result<(), ContractError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file()
                || read_bytes(path)? != bytes
                || (executable && metadata.permissions().mode() & 0o111 == 0)
            {
                return Err(failure(
                    "existing capture tool differs from selected bytes or file kind",
                ));
            }
            // Configure already installs GenMF. Retain an exact regular file
            // without rewriting its bytes, timestamps, permissions or inode.
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write(path, bytes)?;
            if executable {
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            Ok(())
        }
        Err(_) => Err(failure("cannot inspect capture tool destination")),
    }
}

#[test]
fn exact_capture_install_preserves_existing_bytes_and_refuses_different_or_linked_files() {
    use std::os::unix::fs::{symlink, MetadataExt};
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("tool");
    install_exact(&path, b"selected", true).unwrap();
    let before = fs::symlink_metadata(&path).unwrap();
    install_exact(&path, b"selected", true).unwrap();
    let after = fs::symlink_metadata(&path).unwrap();
    assert_eq!(before.ino(), after.ino());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    assert_eq!(before.permissions().mode(), after.permissions().mode());
    assert!(install_exact(&path, b"different", true).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"selected");
    let link = temporary.path().join("link");
    symlink(&path, &link).unwrap();
    assert!(install_exact(&link, b"selected", true).is_err());
    let directory = temporary.path().join("directory");
    fs::create_dir(&directory).unwrap();
    assert!(install_exact(&directory, b"selected", false).is_err());
    let nonexecutable = temporary.path().join("nonexecutable");
    fs::write(&nonexecutable, b"selected").unwrap();
    fs::set_permissions(&nonexecutable, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(install_exact(&nonexecutable, b"selected", true).is_err());
}

fn execute(
    command: &mut Command,
    output: &Path,
    name: &str,
    cancellation: &CancellationToken,
    expected_status: i32,
    timeout: Duration,
) -> Vec<u8> {
    let result =
        aros_common::run_output_with_control(command, 1024 * 1024, timeout, cancellation).unwrap();
    let stdout = result.stdout.exact_bytes().unwrap();
    fs::write(output.join(format!("{name}.stdout")), stdout).unwrap();
    fs::write(
        output.join(format!("{name}.stderr")),
        result.stderr.exact_bytes().unwrap(),
    )
    .unwrap();
    fs::write(output.join(format!("{name}.status.json")), serde_json::to_vec(&serde_json::json!({"status":result.status.code(),"timed_out":result.timed_out,"cancelled":result.cancelled})).unwrap()).unwrap();
    assert!(
        !result.timed_out && !result.cancelled && result.status.code() == Some(expected_status),
        "native Root capture refused; private logs retained"
    );
    stdout.to_vec()
}

pub(super) fn capture(
    request: &MesaPreflightRequest<'_>,
    completed: &CompletedMesaPreflight,
    selection: &serde_json::Value,
    cancellation: &CancellationToken,
) {
    capture_state(
        request,
        &CaptureState::Completed(completed),
        selection,
        cancellation,
    );
}

pub(super) fn capture_prepared(
    request: &MesaPreflightRequest<'_>,
    prepared: &preparation::PreparedMesaPreflight,
    selection: &serde_json::Value,
    cancellation: &CancellationToken,
) {
    for name in ["receipt.json", INPUT_FILE, "mesa-configuration.json"] {
        assert!(!request.output_root.join(name).exists());
    }
    capture_state(
        request,
        &CaptureState::Prepared(prepared),
        selection,
        cancellation,
    );
    for name in ["receipt.json", INPUT_FILE, "mesa-configuration.json"] {
        assert!(!request.output_root.join(name).exists());
    }
}

fn capture_state(
    request: &MesaPreflightRequest<'_>,
    state: &CaptureState<'_>,
    selection: &serde_json::Value,
    cancellation: &CancellationToken,
) {
    let selected: Selection = serde_json::from_value(selection.clone()).unwrap();
    state.revalidate(request, cancellation).unwrap();
    assert!(
        matches!(
            (
                &selected.metamake,
                &selected.metamake_sha256,
                selected.bootstrap_metamake
            ),
            (Some(_), Some(_), false) | (None, None, true)
        ),
        "select either exact original MetaMake bytes or the explicit source host bootstrap"
    );
    for path in selected
        .metamake
        .iter()
        .chain(std::iter::once(&selected.python))
    {
        assert!(path.is_absolute());
        assert_eq!(fs::canonicalize(path).unwrap(), *path);
    }
    for path in [&selected.output, &selected.python] {
        assert!(path.is_absolute());
        assert!(path
            .to_str()
            .unwrap()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/._-@".contains(&byte)));
    }
    assert!(!aros_common::publication::filesystem_paths_overlap(
        &selected.output,
        request.source_root
    )
    .unwrap());
    let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = request.environment else {
        panic!("Root capture requires sealed host tools");
    };
    let compiler_root = request
        .drivers
        .clang_path()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    for input in [compiler_root, host_tools.root.as_path()] {
        assert!(
            !aros_common::publication::filesystem_paths_overlap(&selected.output, input).unwrap()
        );
    }
    assert!(!aros_common::publication::filesystem_paths_overlap(
        &selected.output,
        request.output_root
    )
    .unwrap());
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&selected.output)
        .unwrap();
    assert_eq!(fs::canonicalize(&selected.output).unwrap(), selected.output);
    let held_metamake = selected
        .metamake
        .as_ref()
        .map(|path| pinned(path, selected.metamake_sha256.as_ref().unwrap()));
    pinned(&selected.python, &selected.python_sha256);
    let build = request.output_root.join("configure");
    let invocation = state.configured_invocation();
    assert_eq!(invocation.working_directory(), build);
    invocation.revalidate().unwrap();
    let makefile = read_text(&build.join("Makefile")).unwrap();
    let missing_options = Path::new(
        &state
            .configuration()
            .recipe_configuration()
            .make_coordinates
            .generated_root,
    )
    .join("config/make.opts");
    for absent in [&build.join("Build.extra"), &missing_options] {
        assert_eq!(
            fs::symlink_metadata(absent).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }
    assert!(state
        .configuration()
        .initial_values()
        .unwrap()
        .entries()
        .any(|entry| entry.name == "CROSSTOOLS_TARGET" && entry.value.is_empty()));
    let original_config = read_text(&build.join("mmake.config")).unwrap();
    assert_eq!(
        original_config
            .lines()
            .filter(|line| line.starts_with('['))
            .collect::<Vec<_>>(),
        vec!["[AROS]"]
    );
    assert!(original_config.lines().any(|line| line == "ignoredir .git"));
    let global_expressions = original_config
        .lines()
        .filter_map(|line| line.strip_prefix("globalvarfile "))
        .collect::<Vec<_>>();
    assert!(!global_expressions.is_empty());
    // Root must remain the first input (its config tests MAKEFILE_LIST). The
    // second input intentionally aborts during parsing, BEFORE GNU Make can
    // rebuild missing includes through Source's compiler-building .DEFAULT.
    // Auditing the selected parse-time closure remains a separate precondition.
    let query = selected.output.join("paths.mk");
    assert!(!makefile.contains("__aros_root_capture_paths"));
    let mut query_text = "$(info MMAKE=$(MMAKE))\n$(info HOST_CC=$(HOST_CC))\n$(info HOST_MAKE=$(HOST_MAKE))\n$(info GENMFSCRIPT=$(GENMFSCRIPT))\n$(info MKARGS=$(MKARGS))\n$(info READ_FILES=$(MAKEFILE_LIST))\n".to_owned();
    let mut keys = vec![
        "MMAKE".to_owned(),
        "HOST_CC".into(),
        "HOST_MAKE".into(),
        "GENMFSCRIPT".into(),
        "MKARGS".into(),
        "READ_FILES".into(),
    ];
    for (index, expression) in global_expressions.iter().enumerate() {
        let key = format!("GLOBAL_FILE_{index}");
        writeln!(query_text, "$(info {key}={expression})").unwrap();
        keys.push(key);
    }
    query_text.push_str("$(error __aros_root_capture_paths_stop_before_remake)\n");
    write(&query, query_text.as_bytes()).unwrap();
    let inherited = &invocation.environment().variables;
    let mut command = Command::new(invocation.program());
    command
        .env_clear()
        .envs(inherited)
        .current_dir(&build)
        .args([
            "--silent",
            "--no-builtin-rules",
            "--file",
            "Makefile",
            "--file",
            query.to_str().unwrap(),
            "mesa3dgl-library-quick",
        ]);
    let stdout = execute(
        &mut command,
        &selected.output,
        "root-paths",
        cancellation,
        2,
        state.execution_timeout(),
    );
    assert!(read_text(&selected.output.join("root-paths.stderr"))
        .unwrap()
        .contains("__aros_root_capture_paths_stop_before_remake"));
    let paths = parse_values(
        &stdout,
        &keys.iter().map(String::as_str).collect::<Vec<_>>(),
    )
    .unwrap();
    let mut read_inputs = Vec::new();
    let mut old_files = Vec::new();
    for name in paths["READ_FILES"].split_ascii_whitespace() {
        let path = if Path::new(name).is_absolute() {
            PathBuf::from(name)
        } else {
            build.join(name)
        };
        let canonical = fs::canonicalize(&path).unwrap();
        assert!(
            canonical.starts_with(&build)
                || canonical.starts_with(request.source_root)
                || canonical == query
        );
        let bytes = read_bytes(&path).unwrap();
        read_inputs.push((path.clone(), sha256_bytes(&bytes)));
        if canonical != query {
            old_files.push(format!("--old-file={}", path.display()));
            old_files.push(format!("--old-file={name}"));
        }
    }
    assert!(!old_files.is_empty() && old_files.len() < 256);
    old_files.push(format!("--old-file={}", missing_options.display()));
    old_files.push(format!(
        "--old-file={}",
        build.join("Build.extra").display()
    ));
    old_files.push(format!(
        "--old-file={}",
        build.join("mmake.config").display()
    ));
    for index in 0..global_expressions.len() {
        let path = PathBuf::from(&paths[&format!("GLOBAL_FILE_{index}")]);
        assert!(path.starts_with(&build));
        assert!(read_inputs.iter().any(|(read, _)| read == &path));
        read_bytes(&path).unwrap();
    }
    for (path, digest) in &read_inputs {
        pinned(path, digest);
    }
    let metamake_bytes = held_metamake.unwrap_or_else(|| {
        host_bootstrap::bootstrap_original_host_tool(
            request,
            state,
            &selected.output,
            &paths,
            &old_files,
            &read_inputs,
            cancellation,
        )
    });
    for (path, digest) in &read_inputs {
        pinned(path, digest);
    }
    let mut installed = Vec::new();
    for (name, bytes) in [
        ("MMAKE", metamake_bytes),
        (
            "GENMFSCRIPT",
            read_bytes(&request.source_root.join("tools/genmf/genmf.py")).unwrap(),
        ),
    ] {
        let path = PathBuf::from(&paths[name]);
        let relative = path.strip_prefix(&build).unwrap();
        assert!(!relative.as_os_str().is_empty());
        assert!(relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_))));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        assert!(fs::canonicalize(path.parent().unwrap())
            .unwrap()
            .starts_with(&build));
        install_exact(&path, &bytes, name == "MMAKE").unwrap();
        installed.push((path, sha256_bytes(&bytes)));
    }
    let helper = selected.native_observer.as_ref().map_or_else(
        || selected.output.join("capture.py"),
        |observer| observer.program.clone(),
    );
    let helper_bytes = format!(
        "#!{} -IS\n{}",
        selected.python.display(),
        include_str!("../../tests/support/mesa_root_capture.py")
    );
    let private_config = selected.output.join("mmake.config");
    let mut parent_environment = invocation.environment().clone();
    parent_environment.variables.insert(
        "MMAKE_CONFIG".into(),
        private_config.to_str().unwrap().into(),
    );
    parent_environment.variables.insert(
        "AROS_TEST_ROOT_CAPTURE".into(),
        selected.output.to_str().unwrap().into(),
    );
    let mut parent_arguments = ["--silent", "--no-builtin-rules", "--file", "Makefile"]
        .map(str::to_owned)
        .to_vec();
    parent_arguments.extend(old_files.iter().cloned());
    parent_arguments.extend(["--".into(), "mesa3dgl-library-quick".into()]);
    let parent_invocation = invocation::InvocationContext::new(
        invocation.program(),
        &parent_arguments,
        &build,
        parent_environment,
    )
    .unwrap();
    let native_plan = selected
        .native_observer
        .as_ref()
        .filter(|observer| observer.collection)
        .map(|observer| {
            check_observer(observer);
            observation::RootObservation::prepare(
                &selected.output,
                observer,
                &parent_invocation,
                &paths["HOST_MAKE"],
                &serde_json::json!({
                    "qualification":"private-root-first-input-experiment-not-ordinary-preparation",
                    "configuration_identity": state.configuration().identity(),
                    "profile_sha256": request.profile.document_sha256(),
                    "configured_invocation_sha256": invocation.invocation_sha256(),
                    "configured_environment_sha256": invocation.environment_sha256(),
                    "root_parent_invocation_sha256": parent_invocation.invocation_sha256(),
                    "root_parent_environment_sha256": parent_invocation.environment_sha256(),
                    "root_read_inputs": read_inputs,
                    "installed_host_inputs": installed,
                    "original_mmake_configuration_sha256":sha256_bytes(original_config.as_bytes()),
                    "selected_source_make_word":paths["HOST_MAKE"],
                    "root_goal":"mesa3dgl-library-quick",
                }),
            )
        });
    let replacement = if let Some(plan) = &native_plan {
        plan.launcher()
    } else if let Some(observer) = &selected.native_observer {
        check_observer(observer);
        format!(
            "{} --capture-make-invocation {} --",
            helper.display(),
            selected.output.join("capture.json").display()
        )
    } else {
        write(&helper, helper_bytes.as_bytes()).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        helper.to_str().unwrap().to_owned()
    };
    let mut count = 0;
    let mut config = String::new();
    for line in original_config.lines() {
        if let Some(template) = line.strip_prefix("maketool ") {
            assert_eq!(template.matches("$(HOST_MAKE)").count(), 1);
            assert!(template.starts_with("$(HOST_MAKE) "));
            config.push_str("maketool ");
            config.push_str(&template.replacen("$(HOST_MAKE)", &replacement, 1));
            count += 1;
        } else {
            config.push_str(line);
        }
        config.push('\n');
    }
    assert_eq!(count, 1);
    assert!(config.lines().all(|line| line.len() < 254));
    // Preflight the fully bound leaf shell command before native popen. This
    // is an experiment-level literal fence, not a second project evaluator.
    let templates = config
        .lines()
        .filter_map(|line| line.strip_prefix("maketool "))
        .collect::<Vec<_>>();
    assert_eq!(templates.len(), 1);
    let mut expanded = templates[0].to_owned();
    for (name, value) in [
        ("MKARGS", paths["MKARGS"].as_str()),
        ("TOP", build.to_str().unwrap()),
        ("SRCDIR", request.source_root.to_str().unwrap()),
        ("CURDIR", "workbench/libs/mesa"),
        ("TARGET", "mesa3dgl-library-quick"),
    ] {
        assert!(value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/._-@ =".contains(&byte)));
        expanded = expanded.replace(&format!("$({name})"), value);
    }
    expanded.push_str(" -s --file=mmakefile mesa3dgl-library-quick");
    let expected = LiteralMakeChildCommand::parse(&expanded).unwrap();
    assert_eq!(
        expected.program(),
        native_plan
            .as_ref()
            .map_or(
                helper.as_path(),
                observation::RootObservation::launcher_program
            )
            .to_str()
            .unwrap()
    );
    assert!(expected.environment_assignments().is_empty());
    write(&private_config, config.as_bytes()).unwrap();
    // Fail-closed fallback too: if a parent unexpectedly drops MMAKE_CONFIG,
    // MetaMake's cwd/default config still cannot launch real leaf Make.
    fs::write(build.join("mmake.config"), config.as_bytes()).unwrap();
    for (path, digest) in &read_inputs {
        pinned(path, digest);
    }
    validate_selection(request, cancellation).unwrap();
    parent_invocation.revalidate().unwrap();
    let mut command = parent_invocation.command();
    execute(
        &mut command,
        &selected.output,
        "root-launch",
        cancellation,
        0,
        state.execution_timeout(),
    );
    parent_invocation.revalidate().unwrap();
    write(
        &selected.output.join("root-launch.identity.json"),
        &serde_json::to_vec(&serde_json::json!({
            "program_sha256":parent_invocation.program_sha256(),
            "invocation_sha256":parent_invocation.invocation_sha256(),
            "environment_sha256":parent_invocation.environment_sha256(),
            "qualification":"private-actually-supervised-root-launch-not-ordinary-sdk-preparation",
        }))
        .unwrap(),
    )
    .unwrap();
    let prefix_length = if native_plan.is_some() { 4 } else { 3 };
    let record = native_plan.as_ref().map_or_else(
        || read_bytes(&selected.output.join("capture.json")).unwrap(),
        |plan| {
            plan.read_after_reaped(
                &expected.arguments()[prefix_length..],
                &build.join("workbench/libs/mesa"),
                state.execution_timeout(),
                cancellation,
            )
        },
    );
    let child: serde_json::Value = if selected.native_observer.is_some() {
        let native =
            aros_common::make_native_invocation::NativeMakeInvocation::parse(&record).unwrap();
        // The absolute observer must not mask an unavailable original leaf
        // command in the captured PATH. Bind its source-selected spelling to
        // the independently measured GNU Make used by configuration.
        let leaf_make = which::which_in(
            &paths["HOST_MAKE"],
            Some(native.environment().get("PATH").unwrap().as_str()),
            native.working_directory(),
        )
        .unwrap();
        assert_eq!(
            sha256_bytes(&fs::read(leaf_make).unwrap()),
            *invocation.program_sha256()
        );
        assert_eq!(native.arguments(), &expected.arguments()[prefix_length..]);
        serde_json::json!({"argv":native.arguments(), "cwd":native.working_directory(), "environment":native.environment()})
    } else {
        serde_json::from_slice(&record).unwrap()
    };
    assert_eq!(
        child["cwd"],
        build.join("workbench/libs/mesa").to_str().unwrap()
    );
    assert_eq!(
        child["argv"].as_array().unwrap().last().unwrap(),
        "mesa3dgl-library-quick"
    );
    assert_eq!(
        child["argv"],
        serde_json::to_value(if selected.native_observer.is_some() {
            &expected.arguments()[prefix_length..]
        } else {
            expected.arguments()
        })
        .unwrap()
    );
    assert_eq!(
        child["environment"]["_MMAKE_TARGET"],
        "mesa3dgl-library-quick"
    );
    assert_eq!(
        child["environment"]["AROS_TEST_ROOT_CAPTURE"],
        selected.output.to_str().unwrap()
    );
    assert_eq!(
        child["environment"]["MMAKE_CONFIG"],
        private_config.to_str().unwrap()
    );
    for (path, digest) in &read_inputs {
        pinned(path, digest);
    }
    for (path, digest) in installed {
        pinned(&path, &digest);
    }
    if let Some(metamake) = &selected.metamake {
        pinned(metamake, selected.metamake_sha256.as_ref().unwrap());
    }
    pinned(&selected.python, &selected.python_sha256);
    if let Some(observer) = &selected.native_observer {
        check_observer(observer);
    } else {
        assert_eq!(read_bytes(&helper).unwrap(), helper_bytes.as_bytes());
    }
    assert_eq!(read_bytes(&private_config).unwrap(), config.as_bytes());
    assert_eq!(
        read_bytes(&build.join("mmake.config")).unwrap(),
        config.as_bytes()
    );
    invocation.revalidate().unwrap();
    validate_selection(request, cancellation).unwrap();
    let error = state.revalidate(request, cancellation).unwrap_err();
    let expected = match state {
        CaptureState::Completed(_) => "Mesa preflight output changed",
        CaptureState::Prepared(_) => "Mesa preparation output changed before completion",
    };
    assert!(error.to_string().contains(expected), "{error}");
    // Raw capture is private data, not a replacement portable producer receipt.
    fs::write(selected.output.join("result.json"), serde_json::to_vec_pretty(&serde_json::json!({"root_goal":"mesa3dgl-library-quick","capture_sha256":sha256_bytes(&record),"configuration_identity":state.configuration().identity(),"profile_sha256":request.profile.document_sha256(),"native_collection":native_plan.is_some(),"preseal":matches!(state, CaptureState::Prepared(_)),"unbound_output_refused_after_capture":true,"sdk_qualification":false})).unwrap()).unwrap();
}
