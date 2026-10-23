//! Compare GNU Make's child environment with the narrower Source export map.

use super::{MakeProgramPaths, MakeVariableProgram};
use crate::dirs::DirVars;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace},
    make_native_invocation::NativeMakeInvocation,
    make_observer::{
        command_line_values_fragment, environment_values_fragment, pre_file_fragment,
        recipe_controls_fragment,
    },
    make_startup_arguments::LiteralMakeStartupArguments,
    measure_tree_content_cas,
};
use std::os::unix::fs::PermissionsExt;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

fn make_binary() -> &'static str {
    if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    }
}

fn require_gnu_make_42() {
    let output = Command::new(make_binary())
        .arg("--version")
        .output()
        .expect("GNU Make >= 4.2 must be installed for this differential test");
    assert!(output.status.success(), "GNU Make version query failed");
    let version = String::from_utf8(output.stdout).expect("GNU Make version is UTF-8");
    let version = version
        .lines()
        .find_map(|line| line.strip_prefix("GNU Make "))
        .expect("the selected make binary must be GNU Make");
    let mut parts = version.split('.');
    let major = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .expect("GNU Make version has a numeric major component");
    let minor = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .expect("GNU Make version has a numeric minor component");
    assert!(
        (major, minor) >= (4, 2),
        "GNU Make >= 4.2 is required for .SHELLSTATUS observations"
    );
}

fn arguments() -> Vec<String> {
    [
        "-r",
        "-s",
        "--no-print-directory",
        "-f",
        "Source.mk",
        "SYNTH_CLI=selected",
        "probe",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn controlled_environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "PATH".into(),
            std::env::var("PATH").expect("PATH is available"),
        ),
        ("LC_ALL".into(), "C".into()),
        ("SYNTH_INHERITED".into(), "literal-inherited".into()),
        ("SYNTH_DROP".into(), "old-value".into()),
    ])
}

fn native_output(
    cwd: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
    observer: Option<&str>,
) -> Output {
    let mut command = Command::new(make_binary());
    if let Some(observer) = observer {
        command.args(["--eval", observer]);
    }
    command
        .args(arguments)
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command.output().unwrap()
}

/// Observe the exact startup namespace and environment values before Source.mk.
fn pre_file_observation(
    cwd: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
) -> (Vec<u8>, BTreeSet<String>) {
    let startup = LiteralMakeStartupArguments::admit(arguments).unwrap();
    let cli = command_line_values_fragment(startup.command_line_names()).unwrap();
    let discovery = format!(
        "{}{}{}$(error AROS_TEST_STOP_BEFORE_FILES)\n",
        pre_file_fragment(),
        cli,
        recipe_controls_fragment()
    );
    let first = native_output(cwd, arguments, environment, Some(&discovery));
    assert!(!first.status.success());
    let namespace = ValidatedMakeNamespace::from_make_stdout(&first.stdout).unwrap();
    let environment_names = namespace
        .to_observation()
        .entries
        .into_iter()
        .filter(|entry| entry.origin == MakeVariableOrigin::Environment)
        .map(|entry| entry.name)
        .collect::<BTreeSet<_>>();
    let complete = format!(
        "{}{}{}{}$(error AROS_TEST_STOP_BEFORE_FILES)\n",
        pre_file_fragment(),
        cli,
        environment_values_fragment(&environment_names).unwrap(),
        recipe_controls_fragment()
    );
    let output = native_output(cwd, arguments, environment, Some(&complete));
    assert!(!output.status.success());
    (output.stdout, environment_names)
}

fn invocation(
    arguments: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
) -> NativeMakeInvocation {
    NativeMakeInvocation::new(
        arguments.to_vec(),
        cwd.to_owned(),
        environment
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    )
    .unwrap()
}

fn query_arguments(arguments: &[String], query_file: &Path) -> Vec<String> {
    let query_file = query_file.to_str().expect("private query path is UTF-8");
    let mut result = Vec::with_capacity(arguments.len());
    let mut index = 0;
    let mut replaced = 0;
    while index < arguments.len() {
        if arguments[index] == "-f" {
            assert_eq!(
                arguments.get(index + 1).map(String::as_str),
                Some("Source.mk")
            );
            result.push("-f".into());
            result.push(query_file.into());
            index += 2;
            replaced += 1;
        } else {
            result.push(arguments[index].clone());
            index += 1;
        }
    }
    assert_eq!(
        replaced, 1,
        "prototype replaces its sole selected -f operand"
    );
    result
}

fn native_source_program(
    paths: &MakeProgramPaths<'_>,
    observation: &[u8],
    invocation: &NativeMakeInvocation,
    environment_names: &BTreeSet<String>,
    environment: &BTreeMap<String, String>,
) -> MakeVariableProgram {
    let mut program = MakeVariableProgram::from_native_invocation_observation(
        paths,
        observation,
        invocation,
        environment_names,
    )
    .unwrap();
    program
        .require_captured_shell_policy("/bin/sh", &["-c"], environment)
        .unwrap();
    program
}

fn assert_source_exports(
    source_exports: &BTreeMap<String, String>,
    environment: &BTreeMap<String, String>,
) {
    assert_eq!(source_exports.get("PATH"), environment.get("PATH"));
    assert_eq!(source_exports.get("LC_ALL"), Some(&"C".to_owned()));
    assert_eq!(
        source_exports.get("SYNTH_INHERITED"),
        Some(&"literal-inherited".to_owned())
    );
    assert_eq!(
        source_exports.get("SYNTH_CLI"),
        Some(&"selected".to_owned())
    );
    assert_eq!(
        source_exports.get("SYNTH_CHANGED"),
        Some(&"source-value".to_owned())
    );
    assert!(!source_exports.contains_key("SYNTH_DROP"));
}

fn paths<'a>(dirs: &'a DirVars, cwd: &'a Path) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: cwd,
        relative_dir: Path::new(""),
        top_directory: cwd,
        working_directory: cwd,
    }
}

fn snapshot(root: &Path) -> MakeInputSnapshot {
    MakeInputSnapshot::capture(
        root,
        &[MakeInputRootSelection {
            root: root.to_owned(),
            content_sha256: measure_tree_content_cas(root)
                .unwrap()
                .payload_digest_excluding(None),
        }],
    )
    .unwrap()
    .with_include_directories(&[])
    .unwrap()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

enum EffectStrategy {
    DirectShell,
    GnuQuery {
        arguments: Vec<String>,
        query_file: PathBuf,
        query_bytes: Vec<u8>,
    },
    LiteralGnuQuery {
        arguments: Vec<String>,
        environment: BTreeMap<String, String>,
        directory: PathBuf,
    },
}

struct NaiveSourceExportEffects {
    cwd: PathBuf,
    command: String,
    script: PathBuf,
    script_bytes: Vec<u8>,
    strategy: EffectStrategy,
    makeflags_control: Option<bool>,
    query_count: Cell<usize>,
    revalidation_count: Cell<usize>,
    finish_count: Cell<usize>,
    observed_source_exports: RefCell<Option<BTreeMap<String, String>>>,
    rendered_query: RefCell<Option<crate::make_shell_effects::gnu_query::GnuShellQuery>>,
}

impl crate::make_shell_effects::CapturedShellEffects for NaiveSourceExportEffects {
    fn revalidate(&self) -> Result<(), String> {
        self.revalidation_count
            .set(self.revalidation_count.get() + 1);
        let script = std::fs::read(&self.script)
            .map_err(|_| "test fixture script disappeared".to_owned())?;
        let mode = std::fs::metadata(&self.script)
            .map_err(|_| "test fixture script metadata disappeared".to_owned())?
            .permissions()
            .mode();
        if script != self.script_bytes || mode & 0o111 == 0 {
            return Err("test fixture script changed".into());
        }
        if let EffectStrategy::GnuQuery {
            query_file,
            query_bytes,
            ..
        } = &self.strategy
        {
            if std::fs::read(query_file)
                .map_err(|_| "private GNU query fixture disappeared".to_owned())?
                != *query_bytes
            {
                return Err("private GNU query fixture changed".into());
            }
        }
        if let Some(bundle) = self.rendered_query.borrow().as_ref() {
            let directory = bundle.makefile().parent().unwrap().to_owned();
            for (filename, expected) in bundle.files() {
                if std::fs::read(directory.join(filename))
                    .map_err(|_| "literal query payload disappeared".to_owned())?
                    != *expected
                {
                    return Err("literal query payload changed".into());
                }
            }
        }
        Ok(())
    }

    fn query(
        &self,
        request: &crate::make_shell_effects::ShellEffectRequest<'_>,
    ) -> Result<crate::make_shell_effects::ShellEffectOutput, String> {
        let ordinal = self.query_count.get();
        if request.native_context.is_some()
            || ordinal != 0
            || request.ordinal != 0
            || request.command != self.command
            || request.working_directory != self.cwd
            || request.shell != "/bin/sh"
            || request.shell_flags != "-c"
            || request.source_export_controls.get("MAKEFLAGS").copied() != self.makeflags_control
        {
            return Err("test fixture selected an unexpected Source effect".into());
        }
        self.query_count.set(ordinal + 1);
        *self.observed_source_exports.borrow_mut() = Some(request.source_exports.clone());

        // Both branches are deliberately test-only environment candidates,
        // not implementations or qualifications of a native provider.
        // This controlled fixture uses the current export map as incoming
        // environment. Engine-variable edits and special export controls are
        // deliberately outside this candidate's equivalence claim.
        let (stdout, status) = match &self.strategy {
            EffectStrategy::DirectShell => {
                let output = Command::new(request.shell)
                    .arg(request.shell_flags)
                    .arg(request.command)
                    .current_dir(request.working_directory)
                    .env_clear()
                    .envs(request.source_exports)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .output()
                    .map_err(|_| "test fixture shell invocation failed".to_owned())?;
                if !output.status.success() {
                    return Err("test fixture shell returned failure".into());
                }
                let status = output
                    .status
                    .code()
                    .and_then(|code| u8::try_from(code).ok())
                    .ok_or_else(|| {
                        "test fixture shell returned an unsupported status".to_owned()
                    })?;
                let stdout = String::from_utf8(output.stdout)
                    .map_err(|_| "test fixture shell stdout is not UTF-8".to_owned())?;
                (stdout, status)
            }
            EffectStrategy::GnuQuery { arguments, .. } => {
                let output = Command::new(make_binary())
                    .args(arguments)
                    .current_dir(request.working_directory)
                    .env_clear()
                    .envs(request.source_exports)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .output()
                    .map_err(|_| "private GNU query invocation failed".to_owned())?;
                if !output.status.success() || !output.stdout.is_empty() {
                    return Err("private GNU query did not complete its empty probe".into());
                }
                (String::new(), 0)
            }
            EffectStrategy::LiteralGnuQuery {
                arguments,
                environment,
                directory,
            } => {
                use crate::make_shell_effects::gnu_query::{
                    GnuShellQuery, GnuShellQueryResult, LiteralQueryVariable, QUERY_GOAL,
                };
                use std::io::Write;
                // Exercise the real renderer from captured controls rather
                // than selecting a directive from the fixture's test flag.
                // This remains a finite, test-only scope-selection algorithm:
                // arbitrary Source edits to Make-engine state are not proved.
                let mut scope = request
                    .source_exports
                    .iter()
                    .map(|(name, value)| (name.as_str(), (Some(value.as_str()), Some(true))))
                    .collect::<BTreeMap<_, _>>();
                for name in environment.keys() {
                    if !request.source_exports.contains_key(name) {
                        scope.insert(name, (None, Some(false)));
                    }
                }
                for (name, selection) in request.source_export_controls {
                    scope.entry(name).or_insert((None, None)).1 = Some(*selection);
                }
                scope.entry("SHELL").or_insert((None, None)).0 = Some(request.shell);
                scope.entry(".SHELLFLAGS").or_insert((None, None)).0 = Some(request.shell_flags);
                let variables = scope
                    .into_iter()
                    .map(|(name, (value, export))| LiteralQueryVariable {
                        name,
                        value,
                        export,
                    })
                    .collect::<Vec<_>>();
                let bundle = GnuShellQuery::render(directory, request.command, &variables)?;
                for (filename, bytes) in bundle.files() {
                    let mut file = std::fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(directory.join(filename))
                        .map_err(|_| "cannot create literal query payload")?;
                    file.write_all(bytes)
                        .map_err(|_| "cannot write literal query payload")?;
                }
                for path in [bundle.result_file(), bundle.status_file()] {
                    if std::fs::symlink_metadata(path).is_ok() {
                        return Err("literal query output already exists".into());
                    }
                }
                let mut arguments = query_arguments(arguments, &bundle.makefile());
                if arguments.last().map(String::as_str) != Some("probe") {
                    return Err("literal query fixture goal changed".into());
                }
                *arguments.last_mut().unwrap() = QUERY_GOAL.into();
                let output =
                    native_output(request.working_directory, &arguments, environment, None);
                if !output.status.success()
                    || !output.stdout.is_empty()
                    || !output.stderr.is_empty()
                {
                    return Err("literal query fixture did not complete".into());
                }
                let result = GnuShellQueryResult::decode(
                    &std::fs::read(bundle.result_file())
                        .map_err(|_| "literal query result absent")?,
                    &std::fs::read(bundle.status_file())
                        .map_err(|_| "literal query status absent")?,
                )?;
                *self.rendered_query.borrow_mut() = Some(bundle);
                return Ok(crate::make_shell_effects::ShellEffectOutput::GnuValue(
                    result,
                ));
            }
        };
        Ok(crate::make_shell_data::ShellDataOutput { stdout, status }.into())
    }

    fn finish(&self, effects: usize) -> Result<(), String> {
        self.finish_count.set(self.finish_count.get() + 1);
        if effects == 1 && self.query_count.get() == 1 {
            Ok(())
        } else {
            Err("test fixture effect coverage mismatch".into())
        }
    }
}

fn environment_map(bytes: &[u8]) -> BTreeMap<Vec<u8>, Vec<u8>> {
    assert!(
        bytes.last() == Some(&0),
        "env -0 output must be NUL terminated"
    );
    let mut result = BTreeMap::new();
    for entry in bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let separator = entry
            .iter()
            .position(|byte| *byte == b'=')
            .expect("environment entry has a name/value separator");
        let previous = result.insert(entry[..separator].to_vec(), entry[separator + 1..].to_vec());
        assert!(previous.is_none(), "environment contains a duplicate name");
    }
    result
}

fn reported_variable(name: &[u8]) -> Option<&'static str> {
    if name.starts_with(b"SYNTH_") {
        Some("synthetic")
    } else if name == b"PATH" {
        Some("controlled")
    } else if matches!(name, b"PWD" | b"SHLVL" | b"_") {
        Some("shell-owned")
    } else if name.starts_with(b"MAKE") || name == b"MFLAGS" {
        Some("Make-engine")
    } else {
        None
    }
}

fn displayed_value(name: &[u8], value: Option<&Vec<u8>>) -> String {
    if name == b"PATH" {
        "<omitted>".into()
    } else {
        value.map_or_else(
            || "<absent>".into(),
            |value| format!("{:?}", String::from_utf8_lossy(value)),
        )
    }
}

fn relevant_environment_differences(
    native: &BTreeMap<Vec<u8>, Vec<u8>>,
    source_exports: &BTreeMap<Vec<u8>, Vec<u8>>,
) -> String {
    let names = native
        .keys()
        .chain(source_exports.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    names
        .into_iter()
        .filter_map(|name| {
            if native.get(&name) == source_exports.get(&name) {
                return None;
            }
            let category = reported_variable(&name)?;
            Some(format!(
                "{} [{}]: native={}, Source-exports-only={}",
                String::from_utf8_lossy(&name),
                category,
                displayed_value(&name, native.get(&name)),
                displayed_value(&name, source_exports.get(&name)),
            ))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn native_gnu_make_environment_differs_from_direct_shell_but_matches_gnu_query() {
    compare_native_environment(false);
}

#[test]
fn native_gnu_query_requires_explicit_unexport_of_makeflags() {
    compare_native_environment(true);
}

fn compare_native_environment(unexport_makeflags: bool) {
    require_gnu_make_42();

    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().canonicalize().unwrap();
    let source_root = parent.join("source");
    let evidence_root = parent.join("private-evidence");
    std::fs::create_dir(&source_root).unwrap();
    std::fs::create_dir(&evidence_root).unwrap();
    let source_root = source_root.canonicalize().unwrap();
    let evidence_root = evidence_root.canonicalize().unwrap();
    let evidence_path = evidence_root.join("environment.bin");
    let evidence_argument = shell_quote(evidence_path.to_str().unwrap());
    let command = format!("./renamed-fixture-script {evidence_argument}");
    assert!(!command.contains('$'));
    assert!(!command.contains('\n'));
    assert!(!command.contains('\r'));
    let script_bytes = b"#!/bin/sh\nexec /usr/bin/env -0 > \"$1\"\n".to_vec();
    let script_path = source_root.join("renamed-fixture-script");
    std::fs::write(&script_path, &script_bytes).unwrap();
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        source_root.join("Source.mk"),
        format!(
            "SHELL := /bin/sh\nexport SYNTH_CHANGED := source-value\nunexport SYNTH_DROP\n{}CAPTURED_ENV := $(shell {command})\nprobe:\n",
            if unexport_makeflags { "unexport MAKEFLAGS\n" } else { "" },
        ),
    )
    .unwrap();
    let query_file = evidence_root.join("Query.mk");
    assert!(!query_file.starts_with(&source_root));
    let uncontrolled_query_bytes = format!(
        "SHELL := /bin/sh\n.SHELLFLAGS := -c\nQUERY_RESULT := $(shell {command})\nprobe:\n"
    )
    .into_bytes();
    let query_bytes = if unexport_makeflags {
        format!(
            "SHELL := /bin/sh\n.SHELLFLAGS := -c\nunexport MAKEFLAGS\nQUERY_RESULT := $(shell {command})\nprobe:\n"
        ).into_bytes()
    } else {
        uncontrolled_query_bytes.clone()
    };
    std::fs::write(&query_file, &query_bytes).unwrap();

    let arguments = arguments();
    let gnu_query_arguments = query_arguments(&arguments, &query_file);
    let environment = controlled_environment();
    let (observation, environment_names) =
        pre_file_observation(&source_root, &arguments, &environment);
    let invocation = invocation(&arguments, &source_root, &environment);
    let source_snapshot = snapshot(&source_root);
    let dirs = DirVars::default();
    let paths = paths(&dirs, &source_root);

    // The native run writes only the sibling evidence fixture. Hold its bytes
    // before the test provider overwrites that same fixture with its result.
    let native = native_output(&source_root, &arguments, &environment, None);
    assert!(native.status.success(), "controlled GNU Make run failed");
    assert!(native.stdout.is_empty(), "probe must have no recipe output");
    let native_environment_bytes = std::fs::read(&evidence_path).unwrap();
    let native_environment = environment_map(&native_environment_bytes);

    let direct_provider = NaiveSourceExportEffects {
        cwd: source_root.clone(),
        command: command.clone(),
        script: script_path.clone(),
        script_bytes: script_bytes.clone(),
        strategy: EffectStrategy::DirectShell,
        makeflags_control: unexport_makeflags.then_some(false),
        query_count: Cell::new(0),
        revalidation_count: Cell::new(0),
        finish_count: Cell::new(0),
        observed_source_exports: RefCell::new(None),
        rendered_query: RefCell::new(None),
    };
    let mut direct_program = native_source_program(
        &paths,
        &observation,
        &invocation,
        &environment_names,
        &environment,
    );
    direct_program
        .read_captured_files_with_shell_effects(
            &["Source.mk"],
            &paths,
            &source_snapshot,
            &direct_provider,
        )
        .unwrap();
    let direct_exports = direct_provider
        .observed_source_exports
        .borrow()
        .clone()
        .expect("the single selected effect was queried");
    let direct_environment = environment_map(&std::fs::read(&evidence_path).unwrap());
    assert_eq!(direct_provider.query_count.get(), 1);
    assert_eq!(direct_provider.finish_count.get(), 1);
    assert_eq!(direct_provider.revalidation_count.get(), 2);
    assert_source_exports(&direct_exports, &environment);
    if unexport_makeflags {
        assert!(!native_environment.contains_key(b"MAKEFLAGS".as_slice()));
        assert!(!direct_exports.contains_key("MAKEFLAGS"));
        // Absence from the exported-value map alone does not suppress an
        // engine-owned variable: a fresh Make process auto-exports MAKEFLAGS.
        // Hold the native bytes, then measure this deliberately incomplete
        // candidate before restoring the explicitly controlled query scope.
        std::fs::write(&query_file, &uncontrolled_query_bytes).unwrap();
        let uncontrolled = native_output(&source_root, &gnu_query_arguments, &direct_exports, None);
        assert!(uncontrolled.status.success());
        assert!(uncontrolled.stdout.is_empty());
        let uncontrolled_environment = environment_map(&std::fs::read(&evidence_path).unwrap());
        assert!(uncontrolled_environment.contains_key(b"MAKEFLAGS".as_slice()));
        assert_ne!(native_environment, uncontrolled_environment);
        std::fs::write(&query_file, &query_bytes).unwrap();
    }
    assert!(
        native_environment != direct_environment,
        "direct shell from Source exports unexpectedly reproduced GNU Make's child environment"
    );
    let direct_differences =
        relevant_environment_differences(&native_environment, &direct_environment);
    if !direct_differences.is_empty() {
        eprintln!("{direct_differences}");
    }

    let gnu_provider = NaiveSourceExportEffects {
        cwd: source_root.clone(),
        command: command.clone(),
        script: script_path.clone(),
        script_bytes: script_bytes.clone(),
        strategy: EffectStrategy::GnuQuery {
            arguments: gnu_query_arguments,
            query_file,
            query_bytes,
        },
        makeflags_control: unexport_makeflags.then_some(false),
        query_count: Cell::new(0),
        revalidation_count: Cell::new(0),
        finish_count: Cell::new(0),
        observed_source_exports: RefCell::new(None),
        rendered_query: RefCell::new(None),
    };
    let mut gnu_program = native_source_program(
        &paths,
        &observation,
        &invocation,
        &environment_names,
        &environment,
    );
    gnu_program
        .read_captured_files_with_shell_effects(
            &["Source.mk"],
            &paths,
            &source_snapshot,
            &gnu_provider,
        )
        .unwrap();
    let gnu_exports = gnu_provider
        .observed_source_exports
        .borrow()
        .clone()
        .expect("the single selected effect was queried");
    let gnu_environment = environment_map(&std::fs::read(&evidence_path).unwrap());
    assert_eq!(gnu_provider.query_count.get(), 1);
    assert_eq!(gnu_provider.finish_count.get(), 1);
    assert_eq!(gnu_provider.revalidation_count.get(), 2);
    assert_source_exports(&gnu_exports, &environment);

    // Compare all variables, including PWD, SHLVL and `_`; on failure, print
    // only sorted controlled/synthetic, Make-engine and shell-owned deltas.
    if native_environment != gnu_environment {
        let differences = relevant_environment_differences(&native_environment, &gnu_environment);
        if !differences.is_empty() {
            eprintln!("{differences}");
        }
        panic!(
            "the private GNU query environment did not match native GNU Make; selected differences:\n{differences}"
        );
    }

    let literal_directory = evidence_root.join("literal-query");
    std::fs::create_dir(&literal_directory).unwrap();
    let literal_provider = NaiveSourceExportEffects {
        cwd: source_root.clone(),
        command,
        script: script_path,
        script_bytes,
        strategy: EffectStrategy::LiteralGnuQuery {
            arguments,
            environment: environment.clone(),
            directory: literal_directory,
        },
        makeflags_control: unexport_makeflags.then_some(false),
        query_count: Cell::new(0),
        revalidation_count: Cell::new(0),
        finish_count: Cell::new(0),
        observed_source_exports: RefCell::new(None),
        rendered_query: RefCell::new(None),
    };
    let mut literal_program = native_source_program(
        &paths,
        &observation,
        &invocation,
        &environment_names,
        &environment,
    );
    literal_program
        .read_captured_files_with_shell_effects(
            &["Source.mk"],
            &paths,
            &source_snapshot,
            &literal_provider,
        )
        .unwrap();
    let literal_environment = environment_map(&std::fs::read(&evidence_path).unwrap());
    assert!(
        native_environment == literal_environment,
        "renderer-backed query must reproduce the complete controlled native environment"
    );
    assert_eq!(literal_provider.query_count.get(), 1);
    assert_eq!(literal_provider.revalidation_count.get(), 2);
    assert_eq!(literal_provider.finish_count.get(), 1);
    assert!(literal_provider.rendered_query.borrow().is_some());
}
