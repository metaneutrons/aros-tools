//! Ordinary build's retained local Mesa preflight, not SDK/release qualification.

use super::super::toolchain::ResolvedToolchain;
use aros_common::local_source::LocalSourceIdentity;
use aros_common::{
    measure_regular_file_bounded, measure_tree_content_cas, sha256_bytes, CancellationToken,
    Sha256Digest, TargetProfile, TreeContentCas,
};
use aros_toolchain::compatibility::{
    self, CompatibilityEnvironment, CompatibilityHostTool, CompletedMesaPreflight,
    HostToolClosureRequest, LlvmSdkDriverBinding, LlvmSdkDriverRequest,
    MesaGenmodulePreflightSelection, MesaPreflightRequest, SourceMesaPreflightProfile,
};
use aros_toolchain::python_environment::PythonEnvironment;
use miette::{IntoDiagnostic, Result, WrapErr};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub(super) struct Owner {
    source: PathBuf,
    local: LocalSourceIdentity,
    selection: SourceMesaPreflightProfile,
    drivers: LlvmSdkDriverBinding,
    environment: CompatibilityEnvironment,
    build_offline: bool,
    python: PythonEnvironment,
    python_root: PathBuf,
    python_snapshot: TreeContentCas,
    launcher: PathBuf,
    interpreter_sha256: Sha256Digest,
    output: PathBuf,
    completed: CompletedMesaPreflight,
    transpiler: PathBuf,
    transpiler_sha256: Sha256Digest,
}

pub(super) fn required(profile: &TargetProfile) -> bool {
    profile.transpiler.as_ref().is_some_and(|context| {
        context.toolchain == "llvm" && context.mesa_version.as_deref() == Some("26.0.0")
    })
}

pub(super) async fn prepare(
    source: &Path,
    preset: &str,
    profile: &TargetProfile,
    resolved: &ResolvedToolchain,
    offline: bool,
    transpiler: &Path,
) -> Result<Option<Owner>> {
    if !required(profile) {
        return Ok(None);
    }
    let source = source.canonicalize().into_diagnostic()?;
    let selection = SourceMesaPreflightProfile::load(&source, profile)
        .map_err(|error| miette::miette!(error))?;
    let transpiler = transpiler.canonicalize().into_diagnostic()?;
    let transpiler_sha256 = executable_digest(&transpiler)?;
    let lock = selection.python_lock().cloned().ok_or_else(|| {
        miette::miette!("Mesa preflight requires a source-owned sealed host Python lock")
    })?;
    let compiler = resolved.compiler_identity.clone().ok_or_else(|| miette::miette!("Mesa preflight requires a verified compiler identity; unversioned legacy prefixes are not sufficient"))?;
    let cache = aros_cache::archive_cache_root().map_err(|error| miette::miette!(error))?;
    check_external(&cache, &source, &resolved.paths.root)?;
    aros_common::ensure_directory_nofollow(&cache).into_diagnostic()?;
    let cache = cache.canonicalize().into_diagnostic()?;
    check_external(&cache, &source, &resolved.paths.root)?;
    let work = tempfile::Builder::new()
        .prefix("mesa-preflight-")
        .tempdir_in(&cache)
        .into_diagnostic()?
        .keep();
    // Persist failures as evidence and successful configure dependencies for
    // later Ninja regeneration. Deleting this root at Owner::drop would leave
    // the generated build tree pointing at absent Python/input files. This is
    // never a compiler build root; lifecycle cleanup requires a separate policy.
    let result: Result<Option<Owner>> = async {
        let local = LocalSourceIdentity::capture(&source, preset)
            .map_err(|error| miette::miette!(error))?;
        let host = aros_common::target::native_host_key()
            .ok_or_else(|| miette::miette!("unsupported Mesa preflight host"))?;
        let mut tools = select_host_tools(host)?;
        let python_program = tools
            .iter()
            .find(|tool| tool.name == "python3")
            .ok_or_else(|| miette::miette!("selected host closure has no Python"))?
            .program
            .clone();
        let interpreter_sha256 = executable_digest(&python_program)?;
        let payload_cache = cache
            .join("mesa-host-python-v1")
            .join(lock.sha256().to_string());
        aros_common::ensure_directory_nofollow(&payload_cache).into_diagnostic()?;
        for package in lock.packages() {
            let payload = package.payload();
            let snapshot = aros_fetch::engine::cache::acquire_https_cache_payload(
                &payload_cache,
                payload.filename(),
                payload.url(),
                payload.size(),
                payload.sha256(),
                offline,
            )
            .await
            .map_err(|error| miette::miette!(error))?;
            snapshot
                .revalidate()
                .map_err(|error| miette::miette!(error))?;
        }
        let python_root = work.join("python");
        // Preparation observes/rechecks the selected interpreter, imports only
        // lock-owned module versions and never installs with pip.
        let interpreter = observe_python(&python_program)?;
        let python = PythonEnvironment::prepare_from_host_lock_with_interpreter(
            &lock,
            &payload_cache,
            &python_root,
            &interpreter,
        )
        .map_err(|error| miette::miette!(error))?;
        let python_snapshot = measure_tree_content_cas(&python_root).into_diagnostic()?;
        let shell = tools
            .iter()
            .find(|tool| tool.name == "sh")
            .ok_or_else(|| miette::miette!("selected host closure has no shell"))?
            .program
            .clone();
        let launcher = work.join("python3");
        write_python_launcher(&launcher, &shell, &python_program)?;
        tools
            .iter_mut()
            .find(|tool| tool.name == "python3")
            .ok_or_else(|| miette::miette!("Python role disappeared"))?
            .program
            .clone_from(&launcher);
        let host_tools = compatibility::prepare_host_tool_closure(&HostToolClosureRequest {
            output_root: work.join("host-tools"),
            tools,
        })
        .map_err(|error| miette::miette!(error))?;
        let variables = selected_environment(
            python
                .compatibility_environment()
                .map_err(|error| miette::miette!(error))?,
            &launcher,
        )?;
        let environment = CompatibilityEnvironment::SealedHostTools {
            variables,
            host_tools,
        };
        let drivers = compatibility::inspect_llvm_sdk_drivers(
            &LlvmSdkDriverRequest {
                compiler_root: resolved.paths.root.clone(),
                compiler,
                target: selection.target().clone(),
                timeout: Duration::from_secs(30),
            },
            &CancellationToken::default(),
        )
        .map_err(|error| miette::miette!(error))?;
        let output = work.join("configure-owner");
        let completed = compatibility::execute_mesa_preflight(
            &MesaPreflightRequest {
                source_root: &source,
                source_tree_sha256: &local.content_sha256,
                local_source: Some(&local),
                profile: (&selection).into(),
                genmodule_selection: Some(MesaGenmodulePreflightSelection {
                    source_profile: &selection,
                    scope: aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope::FullTarget,
                    transpiler: &transpiler,
                    transpiler_sha256: &transpiler_sha256,
                }),
                drivers: &drivers,
                environment: &environment,
                output_root: &output,
                timeout: Duration::from_secs(600),
            },
            &CancellationToken::default(),
        )
        .map_err(|error| miette::miette!(error))?;
        let owner = Owner {
            source,
            local,
            selection,
            drivers,
            environment,
            build_offline: offline,
            python,
            python_root,
            python_snapshot,
            launcher,
            interpreter_sha256,
            output,
            completed,
            transpiler,
            transpiler_sha256,
        };
        owner.revalidate()?;
        Ok(Some(owner))
    }
    .await;
    result.wrap_err_with(|| format!("Mesa preflight evidence retained at {}", work.display()))
}

fn selected_environment(
    mut variables: BTreeMap<String, String>,
    launcher: &Path,
) -> Result<BTreeMap<String, String>> {
    variables.insert("PYTHON".into(), path_text(launcher)?.into());
    // Configure/features preflight must never fetch. Its retained identity
    // remains offline even when the subsequent build may fetch pinned inputs.
    variables.insert("AROS_FETCH_OFFLINE".into(), "1".into());
    variables.insert("LC_ALL".into(), "C".into());
    Ok(variables)
}

impl Owner {
    fn request(&self) -> MesaPreflightRequest<'_> {
        MesaPreflightRequest {
            source_root: &self.source,
            source_tree_sha256: &self.local.content_sha256,
            local_source: Some(&self.local),
            profile: (&self.selection).into(),
            genmodule_selection: Some(MesaGenmodulePreflightSelection {
                source_profile: &self.selection,
                scope:
                    aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope::FullTarget,
                transpiler: &self.transpiler,
                transpiler_sha256: &self.transpiler_sha256,
            }),
            drivers: &self.drivers,
            environment: &self.environment,
            output_root: &self.output,
            timeout: Duration::from_secs(600),
        }
    }

    pub(super) fn revalidate(&self) -> Result<()> {
        if measure_tree_content_cas(&self.python_root).into_diagnostic()? != self.python_snapshot
            || executable_digest(&self.python.interpreter().path)? != self.interpreter_sha256
        {
            miette::bail!("selected Mesa Python interpreter/modules changed");
        }
        self.python
            .compatibility_environment()
            .map_err(|error| miette::miette!(error))?;
        self.completed
            .revalidate(&self.request(), &CancellationToken::default())
            .map_err(|error| miette::miette!(error))
    }

    pub(super) fn configure(&self, command: &mut Command) -> Result<()> {
        self.revalidate()?;
        self.apply_environment(command)?;
        command.args(
            self.completed
                .cmake_configuration_arguments(&self.request(), &CancellationToken::default())
                .map_err(|error| miette::miette!(error))?,
        );
        command.arg(format!("-DAROS_PYTHON3={}", path_text(&self.launcher)?));
        command.arg(format!(
            "-DPython3_EXECUTABLE={}",
            path_text(&self.launcher)?
        ));
        Ok(())
    }

    pub(super) fn apply_environment(&self, command: &mut Command) -> Result<()> {
        apply_build_environment(&self.environment, self.build_offline, command)
    }
}

fn apply_build_environment(
    preflight: &CompatibilityEnvironment,
    offline: bool,
    command: &mut Command,
) -> Result<()> {
    // Resolve/revalidate the same sealed closure first. Only this explicit
    // build policy differs; no ambient variables or unmeasured tools return.
    preflight
        .apply_to(command)
        .map_err(|error| miette::miette!(error))?;
    command.env("AROS_FETCH_OFFLINE", if offline { "1" } else { "0" });
    Ok(())
}

fn check_external(path: &Path, source: &Path, compiler: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || [source, compiler]
            .iter()
            .any(|input| path.starts_with(input) || input.starts_with(path))
    {
        miette::bail!(
            "Mesa workspace/cache must be absolute and disjoint from source/compiler inputs"
        );
    }
    Ok(())
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .filter(|text| !text.chars().any(char::is_control) && !text.contains(';'))
        .ok_or_else(|| miette::miette!("unsafe Mesa host path"))
}

fn executable_digest(path: &Path) -> Result<Sha256Digest> {
    let (_, bytes) = measure_regular_file_bounded(path, 256 * 1024 * 1024)
        .into_diagnostic()?
        .ok_or_else(|| miette::miette!("selected Mesa interpreter disappeared"))?;
    Ok(sha256_bytes(&bytes))
}

fn observe_python(path: &Path) -> Result<aros_toolchain::python_environment::PythonInterpreter> {
    let mut command = Command::new(path);
    command
        .env_clear()
        .env("PATH", "/nonexistent")
        .args(["-S", "-P", "-B", "--version"]);
    let output =
        aros_common::run_output_with_timeout(&mut command, 16 * 1024, Duration::from_secs(10))
            .into_diagnostic()?;
    let version = output
        .stdout
        .exact_bytes()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| text.lines().next())
        .filter(|text| text.starts_with("Python 3."))
        .ok_or_else(|| {
            miette::miette!("Mesa requires Python 3 with isolated site/safe-path flags")
        })?;
    if !output.status.success() || output.timed_out {
        miette::bail!("selected Mesa Python probe failed");
    }
    Ok(aros_toolchain::python_environment::PythonInterpreter {
        path: path.to_owned(),
        version: version.into(),
    })
}

fn write_python_launcher(path: &Path, shell: &Path, interpreter: &Path) -> Result<()> {
    let shell = path_text(shell)?;
    if shell.chars().any(char::is_whitespace) {
        miette::bail!("selected shell path is unsafe for a Python launcher");
    }
    let interpreter = path_text(interpreter)?.replace('\'', "'\\''");
    let bytes = format!("#!{shell}\nexec '{interpreter}' -S -P -B \"$@\"\n");
    aros_common::publish_atomic_file(
        path,
        bytes.as_bytes(),
        aros_common::AtomicFilePolicy::NoClobber,
    )
    .into_diagnostic()?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).into_diagnostic()
}

fn select_host_tools(host: &str) -> Result<Vec<CompatibilityHostTool>> {
    let mut roles = compatibility::native_compatibility_host_tools(host)
        .map_err(|error| miette::miette!(error))?;
    // The local-input reader rechecks Git/index/submodule identity in each
    // transpiler child. Complete-tree native qualification does not use Git.
    roles.push("git");
    roles
        .into_iter()
        .map(|role| {
            let program = match role {
                "make" => which::which("gmake")
                    .or_else(|_| which::which("make"))
                    .into_diagnostic()?,
                "llvm-arcc" | "llvm-ranlibcc" => {
                    let tool = if role == "llvm-arcc" { "ar" } else { "ranlib" };
                    let mut command = Command::new(which::which("xcrun").into_diagnostic()?);
                    command.args(["--find", tool]);
                    let output = aros_common::run_output_with_timeout(
                        &mut command,
                        16 * 1024,
                        Duration::from_secs(10),
                    )
                    .into_diagnostic()?;
                    if !output.status.success() || output.timed_out {
                        miette::bail!("cannot select Darwin host role {role}");
                    }
                    PathBuf::from(
                        std::str::from_utf8(
                            output
                                .stdout
                                .exact_bytes()
                                .ok_or_else(|| miette::miette!("invalid xcrun output"))?,
                        )
                        .into_diagnostic()?
                        .trim(),
                    )
                }
                other => which::which(other)
                    .into_diagnostic()
                    .wrap_err_with(|| format!("missing Mesa preflight host role {other}"))?,
            }
            .canonicalize()
            .into_diagnostic()?;
            Ok(CompatibilityHostTool {
                name: role.into(),
                program,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn selected_network_policy_reaches_the_closed_child_without_ambient_values() {
        for (offline, expected) in [(true, b"1".as_slice()), (false, b"0".as_slice())] {
            let variables = selected_environment(
                BTreeMap::from([("PATH".into(), "/nonexistent".into())]),
                Path::new("/selected/python3"),
            )
            .unwrap();
            assert_eq!(variables["AROS_FETCH_OFFLINE"], "1");
            let preflight = CompatibilityEnvironment::Poisoned { variables };
            let mut command = Command::new("/bin/sh");
            command
                .env("AROS_FETCH_OFFLINE", "ambient")
                .args(["-c", "printf '%s' \"$AROS_FETCH_OFFLINE\""]);
            apply_build_environment(&preflight, offline, &mut command).unwrap();
            let output =
                aros_common::run_output_with_timeout(&mut command, 4096, Duration::from_secs(5))
                    .unwrap();
            assert!(output.status.success() && !output.timed_out);
            assert_eq!(output.stdout.exact_bytes().unwrap(), expected);
            // Applying build policy must not change the retained preflight.
            let CompatibilityEnvironment::Poisoned { variables } = &preflight else {
                unreachable!()
            };
            assert_eq!(variables["AROS_FETCH_OFFLINE"], "1");
        }
    }

    async fn actual_owner_from_case(offline: bool) -> (Owner, String) {
        let case_path = PathBuf::from(std::env::var_os("AROS_TEST_MESA_PREFLIGHT_CASE").unwrap());
        assert!(case_path.is_absolute());
        let case: serde_json::Value =
            serde_json::from_slice(&fs::read(case_path).unwrap()).unwrap();
        let source = PathBuf::from(case["source_root"].as_str().unwrap());
        let compiler = PathBuf::from(case["compiler_root"].as_str().unwrap());
        let preset = case["source_preset"].as_str().unwrap();
        let profile = TargetProfile::load_config(&source.join("aros-targets.toml"))
            .unwrap()
            .targets
            .into_iter()
            .find(|profile| profile.name == preset)
            .unwrap();
        let resolved = crate::toolchain::resolve_for_build(&source, preset, Some(&compiler), true)
            .await
            .unwrap();
        let helpers = crate::build_tools::ensure(&source).unwrap();
        let owner = prepare(
            &source,
            preset,
            &profile,
            &resolved,
            offline,
            &helpers.bin_dir.join("aros-transpiler"),
        )
        .await
        .unwrap()
        .unwrap();
        (owner, preset.into())
    }

    /// Runs only bounded source configure/features using an existing verified
    /// compiler. It does not invoke CMake, build Mesa/an SDK, or qualify release.
    #[tokio::test]
    #[ignore = "requires explicit retained source/compiler case and host tools"]
    async fn actual_source_owner_prepares_and_retains_bound_handoff() {
        let (owner, preset) = actual_owner_from_case(false).await;
        owner.revalidate().unwrap();
        let input_path = owner
            .output
            .join(aros_common::mesa_configuration_input::INPUT_FILE);
        let input_bytes = fs::read(&input_path).unwrap();
        let input = aros_common::mesa_configuration_input::load_mesa_configuration_input(
            &input_path,
            &sha256_bytes(&input_bytes),
        )
        .unwrap();
        let collection = input.genmodule_collection().unwrap();
        assert_eq!(collection.selection().binding().profile, preset);
        assert_eq!(
            collection.selection().binding().scope,
            aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope::FullTarget
        );
        assert!(collection.outputs().len() > 1);
        aros_common::outputln!(
            "actual full-target genmodule collection: {} producers",
            collection.outputs().len()
        );
        let mut command = Command::new("cmake");
        command.arg("-DAROS_MESA_CONFIGURATION_MODE=off");
        owner.configure(&mut command).unwrap();
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_str().unwrap())
            .collect();
        assert_eq!(arguments[0], "-DAROS_MESA_CONFIGURATION_MODE=off");
        assert_eq!(arguments[1], "-DAROS_MESA_CONFIGURATION_MODE=bound");
        assert!(arguments
            .iter()
            .any(|argument| argument.starts_with("-DPython3_EXECUTABLE=")));
        aros_common::outputln!(
            "local source preflight owner retained: {}",
            owner.output.display()
        );
        // A change to this test-owned launcher invalidates the retained owner.
        fs::OpenOptions::new()
            .append(true)
            .open(&owner.launcher)
            .unwrap()
            .write_all(b"# changed\n")
            .unwrap();
        assert!(owner.revalidate().is_err());
    }

    /// Keep this fresh offline owner unchanged for subsequent whole-program
    /// data replay; the separate test above owns the mutation counter.
    #[tokio::test]
    #[ignore = "requires explicit real source/compiler case and cached host Python inputs"]
    async fn actual_source_owner_retains_namespace_for_whole_program_replay() {
        let (owner, preset) = actual_owner_from_case(true).await;
        owner.revalidate().unwrap();
        let input_path = owner
            .output
            .join(aros_common::mesa_configuration_input::INPUT_FILE);
        let bytes = fs::read(&input_path).unwrap();
        let input = aros_common::mesa_configuration_input::load_mesa_configuration_input(
            &input_path,
            &sha256_bytes(&bytes),
        )
        .unwrap();
        let namespace = input.configuration().initial_namespace().unwrap();
        let stdout = fs::read(owner.output.join("configured-values.stdout")).unwrap();
        assert_eq!(
            namespace,
            &aros_common::make_namespace::ValidatedMakeNamespace::from_make_stdout(&stdout)
                .unwrap()
        );
        let initial = input.configuration().initial_values().unwrap();
        assert_eq!(
            initial,
            &aros_common::make_initial_values::ValidatedMakeInitialValues::from_make_stdout(
                &stdout, namespace
            )
            .unwrap()
        );
        assert!(initial.entries().count() > 1);
        let collection = input.genmodule_collection().unwrap();
        assert_eq!(collection.selection().binding().profile, preset);
        assert_eq!(
            collection.selection().binding().scope,
            aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope::FullTarget
        );
        assert!(collection.outputs().len() > 1);
        input.revalidate().unwrap();
        owner.revalidate().unwrap();
        aros_common::outputln!(
            "unchanged namespace/value owner: {} input {} producers {}",
            owner.output.display(),
            sha256_bytes(&bytes),
            collection.outputs().len()
        );
    }

    #[test]
    fn workspace_requires_disjoint_normalized_external_paths() {
        let source = Path::new("/input/source");
        let compiler = Path::new("/input/compiler");
        assert!(check_external(Path::new("/cache/mesa"), source, compiler).is_ok());
        for path in [
            "relative",
            "/input",
            "/input/source",
            "/input/source/cache",
            "/input/compiler/cache",
            "/cache/../input",
        ] {
            assert!(
                check_external(Path::new(path), source, compiler).is_err(),
                "{path}"
            );
        }
        assert!(path_text(Path::new("/cache/unsafe;list")).is_err());
        assert!(path_text(Path::new("/cache/unsafe\nline")).is_err());
    }

    #[test]
    fn launcher_isolates_site_cwd_and_bytecode_and_refuses_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let python = which::which("python3").unwrap().canonicalize().unwrap();
        let shell = which::which("sh").unwrap().canonicalize().unwrap();
        let launcher = temporary.path().join("python3");
        write_python_launcher(&launcher, &shell, &python).unwrap();
        fs::write(
            temporary.path().join("json.py"),
            b"raise RuntimeError('ambient cwd module')\n",
        )
        .unwrap();
        let probe = "import sys, json; assert sys.flags.no_site and sys.flags.safe_path and sys.dont_write_bytecode; assert '' not in sys.path; print('isolated')";
        let mut command = Command::new(&launcher);
        command
            .current_dir(temporary.path())
            .env_clear()
            .args(["-c", probe]);
        let output =
            aros_common::run_output_with_timeout(&mut command, 16 * 1024, Duration::from_secs(10))
                .unwrap();
        assert!(output.status.success() && !output.timed_out, "{output:?}");
        assert_eq!(output.stdout.exact_bytes().unwrap(), b"isolated\n");
        assert!(!temporary.path().join("__pycache__").exists());

        // The same cwd poison is effective without the controlled launcher.
        let mut counter = Command::new(&python);
        counter
            .current_dir(temporary.path())
            .env_clear()
            .args(["-B", "-c", "import json"]);
        let output =
            aros_common::run_output_with_timeout(&mut counter, 16 * 1024, Duration::from_secs(10))
                .unwrap();
        assert!(!output.status.success() && !output.timed_out);
        let original = fs::read(&launcher).unwrap();
        assert!(write_python_launcher(&launcher, &shell, &python).is_err());
        assert_eq!(fs::read(&launcher).unwrap(), original);
    }

    #[test]
    fn launcher_rejects_unsafe_shell_or_interpreter_before_writing() {
        let temporary = tempfile::tempdir().unwrap();
        let launcher = temporary.path().join("python3");
        assert!(write_python_launcher(
            &launcher,
            Path::new("/shell with space"),
            Path::new("/python")
        )
        .is_err());
        assert!(
            write_python_launcher(&launcher, Path::new("/bin/sh"), Path::new("/python;list"))
                .is_err()
        );
        assert!(!launcher.exists());
    }
}
