//! Opt-in data-only replay against an explicitly selected current Source/build
//! tree. This is an experiment over those exact bytes, not historical owner,
//! producer, command, or receipt qualification.
//!
//! Audit the explicit case before opting in: its environment values are retained
//! raw in private artifacts and must contain no operator credentials. Verify the
//! executable independently; a digest supplied by the same case proves only
//! consistency with that selection, not trusted GNU Make provenance.

#[cfg(unix)]
mod unix {
    use aros_common::{
        bounded_output_detail,
        make_initial_values::{
            ValidatedMakeCommandLineValues, ValidatedMakeEnvironmentValues,
            ValidatedMakeInitialValues, ValidatedMakeParserControls,
        },
        make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
        make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace},
        make_observer::{
            command_line_values_fragment, environment_values_fragment, source_prelude_fragment,
            validate_source_prelude_inputs,
        },
        make_startup_arguments::LiteralMakeStartupArguments,
        measure_regular_file_bounded, measure_tree_content_cas_bounded,
        publication::filesystem_paths_overlap,
        run_output_with_control, sha256_bytes, CancellationToken, Sha256Digest, TreeContentCas,
        TreeTraversalLimits,
    };
    use aros_transpiler::{
        dirs::DirVars,
        make_rule_header::RuleHeader,
        make_vars::{MakeFileEventKind, MakeFileProgram, MakeProgramPaths, MakeVariableProgram},
    };
    use serde::Deserialize;
    use serde_json::{json, Value};
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs::{self, DirBuilder, OpenOptions},
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        path::{Component, Path, PathBuf},
        process::{Command, Stdio},
        time::Duration,
    };

    const CASE_ENVIRONMENT: &str = "AROS_TEST_CAPTURED_MAKE_ROOT_CASE";
    const STREAM_LIMIT: usize = 256 * 1024;
    const MAKE_BYTES_LIMIT: u64 = 16 * 1024 * 1024;
    const ROOT_ENTRY_LIMIT: usize = 100_000;
    const ROOT_BYTES_LIMIT: u64 = 4 * 1024 * 1024 * 1024;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct CapturedRootCase {
        source_root: PathBuf,
        build_root: PathBuf,
        make: PathBuf,
        make_sha256: Sha256Digest,
        output_root: PathBuf,
        environment: BTreeMap<String, String>,
        #[serde(default)]
        dispatch: Option<DispatchCase>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct DispatchCase {
        root_sha256: Sha256Digest,
        goal: String,
        pattern: String,
        prefix_variable: String,
        program_variable: String,
        options_variable: String,
        project: String,
        prefix: String,
        program: String,
        options: String,
    }

    struct ValidatedPaths {
        source_root: PathBuf,
        build_root: PathBuf,
        make: PathBuf,
        output_root: PathBuf,
    }

    struct PrivateOutput {
        root: PathBuf,
    }

    struct NativeResult {
        stdout: Vec<u8>,
        status: Option<i32>,
        elapsed_ms: u128,
    }

    fn default_rule_records(source: &MakeFileProgram) -> Result<Vec<Value>, String> {
        let mut declarations = BTreeMap::new();
        for (index, event) in source.events.iter().enumerate() {
            if let MakeFileEventKind::DefaultRule(header) = &event.kind {
                let RuleHeader::Rule {
                    prerequisites,
                    order_only,
                    double_colon,
                    inline_recipe,
                    target_pattern: None,
                    ..
                } = header
                else {
                    return Err("captured default declaration has an unsupported header".into());
                };
                declarations.insert(
                    index,
                    json!({
                        "event": index,
                        "input": event.input,
                        "source_line": event.source_line,
                        "generated": event.generated,
                        "prerequisites": prerequisites,
                        "order_only": order_only,
                        "double_colon": double_colon,
                        "inline_recipe": inline_recipe,
                        "recipes": [],
                        "fallback_selection_or_execution_proven": false,
                    }),
                );
            }
        }
        for (index, event) in source.events.iter().enumerate() {
            let MakeFileEventKind::Recipe { rule_event, text } = &event.kind else {
                continue;
            };
            let Some(declaration) = declarations.get_mut(rule_event) else {
                continue;
            };
            let recipes = declaration
                .get_mut("recipes")
                .and_then(Value::as_array_mut)
                .ok_or("captured default record lost its recipe array")?;
            recipes.push(json!({
                "event": index,
                "input": event.input,
                "source_line": event.source_line,
                "generated": event.generated,
                "text": text,
            }));
        }
        Ok(declarations.into_values().collect())
    }

    fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
        if !path.is_absolute() {
            return Err("selected input directory is not absolute".into());
        }
        let canonical = fs::canonicalize(path)
            .map_err(|error| format!("cannot canonicalize selected input directory: {error}"))?;
        if canonical.as_path() != path || !canonical.is_dir() || canonical.to_str().is_none() {
            return Err("selected input directory must be a canonical UTF-8 directory".into());
        }
        Ok(canonical)
    }

    fn canonical_make(path: &Path) -> Result<PathBuf, String> {
        if !path.is_absolute() {
            return Err("selected Make executable is not absolute".into());
        }
        let canonical = fs::canonicalize(path)
            .map_err(|error| format!("cannot canonicalize selected Make executable: {error}"))?;
        if canonical.as_path() != path || canonical.to_str().is_none() {
            return Err("selected Make executable must be canonical UTF-8".into());
        }
        let metadata = fs::metadata(&canonical)
            .map_err(|error| format!("cannot inspect selected Make executable: {error}"))?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err("selected Make executable is not a regular executable".into());
        }
        Ok(canonical)
    }

    fn prospective_output(path: &Path) -> Result<PathBuf, String> {
        if !path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
        {
            return Err("private output directory must be normalized and absolute".into());
        }
        let name = path
            .file_name()
            .ok_or_else(|| "private output directory has no final component".to_owned())?;
        let parent = path
            .parent()
            .ok_or_else(|| "private output directory has no parent".to_owned())?;
        let canonical_parent = fs::canonicalize(parent)
            .map_err(|error| format!("cannot canonicalize private output parent: {error}"))?;
        if !canonical_parent.is_dir() {
            return Err("private output parent is not a directory".into());
        }
        let output = canonical_parent.join(name);
        if output.as_path() != path {
            return Err("private output directory must use a canonical parent".into());
        }
        match fs::symlink_metadata(&output) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err("private output directory already exists".into()),
            Err(error) => {
                return Err(format!("cannot inspect private output path: {error}"));
            }
        }
        Ok(output)
    }

    fn validate_paths(case: &CapturedRootCase) -> Result<ValidatedPaths, String> {
        let source_root = canonical_directory(&case.source_root)?;
        let build_root = canonical_directory(&case.build_root)?;
        let make = canonical_make(&case.make)?;
        if filesystem_paths_overlap(&source_root, &build_root)
            .map_err(|error| format!("cannot compare selected input roots: {error}"))?
        {
            return Err("selected Source and build roots overlap".into());
        }
        let output_root = prospective_output(&case.output_root)?;
        for input in [&source_root, &build_root] {
            if filesystem_paths_overlap(input, &output_root).map_err(|error| {
                format!("cannot compare private output and input paths: {error}")
            })? {
                return Err("private output directory overlaps a selected input root".into());
            }
        }
        Ok(ValidatedPaths {
            source_root,
            build_root,
            make,
            output_root,
        })
    }

    impl PrivateOutput {
        fn create(root: PathBuf) -> Result<Self, String> {
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            builder
                .create(&root)
                .map_err(|error| format!("cannot create private output directory: {error}"))?;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("cannot restrict private output directory: {error}"))?;
            let metadata = fs::symlink_metadata(&root)
                .map_err(|error| format!("cannot inspect private output directory: {error}"))?;
            if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o777 != 0o700 {
                return Err("private output directory did not retain mode 0700".into());
            }
            Ok(Self { root })
        }

        fn write_new(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
            let path = self.root.join(name);
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(|error| format!("cannot create private artifact: {error}"))?;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("cannot restrict private artifact: {error}"))?;
            file.write_all(bytes)
                .map_err(|error| format!("cannot write private artifact: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("cannot sync private artifact: {error}"))?;
            Ok(())
        }

        fn replace_owned(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
            let path = self.root.join(name);
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("cannot inspect private fixture: {error}"))?;
            if !metadata.file_type().is_file() {
                return Err("private fixture is not a regular file".into());
            }
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
                .map_err(|error| format!("cannot update private fixture: {error}"))?;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("cannot restrict private fixture: {error}"))?;
            file.write_all(bytes)
                .map_err(|error| format!("cannot update private fixture: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("cannot sync private fixture: {error}"))?;
            Ok(())
        }

        fn write_json(&self, name: &str, value: &Value) -> Result<(), String> {
            let bytes = serde_json::to_vec_pretty(value)
                .map_err(|_| "cannot serialize private metadata".to_owned())?;
            self.write_new(name, &bytes)
        }

        fn write_stream(
            &self,
            name: &str,
            stream: &aros_common::CapturedStream,
        ) -> Result<(), String> {
            let path = self.root.join(name);
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(|error| format!("cannot create private native stream: {error}"))?;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("cannot restrict private native stream: {error}"))?;
            stream
                .write_rendered(&mut file)
                .map_err(|error| format!("cannot retain private native stream: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("cannot sync private native stream: {error}"))?;
            Ok(())
        }
    }

    fn measure_root(path: &Path, limits: TreeTraversalLimits) -> Result<TreeContentCas, String> {
        measure_tree_content_cas_bounded(path, limits)
            .map_err(|error| format!("complete input-root measurement failed: {error}"))
    }

    fn root_record(path: &Path, tree: &TreeContentCas) -> Result<Value, String> {
        let path = path
            .to_str()
            .ok_or_else(|| "selected input root identity is not UTF-8".to_owned())?;
        Ok(json!({
            "path": path,
            "entries": tree.entry_count(),
            "content_sha256": tree.payload_digest_excluding(None),
        }))
    }

    fn verify_after_capture_failure(
        paths: &ValidatedPaths,
        expected_source: &TreeContentCas,
        expected_build: &TreeContentCas,
        limits: TreeTraversalLimits,
        output: &PrivateOutput,
    ) -> Result<(), String> {
        let source_after = measure_root(&paths.source_root, limits);
        let build_after = measure_root(&paths.build_root, limits);
        let source_matches = source_after
            .as_ref()
            .is_ok_and(|tree| tree == expected_source);
        let build_matches = build_after
            .as_ref()
            .is_ok_and(|tree| tree == expected_build);
        output.write_json(
            "capture-refusal-verification.json",
            &json!({
                "source_unchanged": source_matches,
                "source_error": source_after.as_ref().err(),
                "build_unchanged": build_matches,
                "build_error": build_after.as_ref().err(),
            }),
        )?;
        if source_matches && build_matches {
            Ok(())
        } else {
            Err("an input root changed or could not be remeasured after snapshot refusal".into())
        }
    }

    fn pin_make(path: &Path, expected: &Sha256Digest) -> Result<Value, String> {
        let canonical = fs::canonicalize(path)
            .map_err(|error| format!("cannot revalidate selected Make executable: {error}"))?;
        if canonical.as_path() != path {
            return Err("selected Make executable identity changed".into());
        }
        let (_, bytes) = measure_regular_file_bounded(path, MAKE_BYTES_LIMIT)
            .map_err(|error| format!("cannot measure selected Make executable: {error}"))?
            .ok_or_else(|| "selected Make executable is absent".to_owned())?;
        let digest = sha256_bytes(&bytes);
        if &digest != expected {
            return Err("selected Make executable digest differs from the pinned digest".into());
        }
        Ok(json!({ "sha256": digest, "bytes": bytes.len() }))
    }

    fn observer_text(environment_names: Option<&BTreeSet<String>>) -> Result<String, String> {
        let command_names = BTreeSet::new();
        let mut text = source_prelude_fragment();
        text.push_str(
            &command_line_values_fragment(&command_names)
                .map_err(|error| format!("command-line observer construction refused: {error}"))?,
        );
        if let Some(names) = environment_names {
            text.push_str(
                &environment_values_fragment(names).map_err(|error| {
                    format!("environment observer construction refused: {error}")
                })?,
            );
        }
        Ok(text)
    }

    fn run_native(
        paths: &ValidatedPaths,
        case: &CapturedRootCase,
        startup: &LiteralMakeStartupArguments,
        output: &PrivateOutput,
        phase: &str,
    ) -> Result<NativeResult, String> {
        let mut command = Command::new(&paths.make);
        command
            .args(startup.arguments())
            .current_dir(&paths.build_root)
            .env_clear()
            .envs(&case.environment)
            .stdin(Stdio::null());
        let result = run_output_with_control(
            &mut command,
            STREAM_LIMIT,
            Duration::from_secs(30),
            &CancellationToken::default(),
        )
        .map_err(|error| format!("controlled native Make launch failed: {error}"))?;
        let stdout_bytes = result.stdout.total_bytes();
        let stderr_bytes = result.stderr.total_bytes();
        output.write_stream(&format!("{phase}.stdout"), &result.stdout)?;
        output.write_stream(&format!("{phase}.stderr"), &result.stderr)?;
        output.write_json(
            &format!("{phase}.status.json"),
            &json!({
                "status": result.status.code(),
                "timed_out": result.timed_out,
                "cancelled": result.cancelled,
                "elapsed_ms": result.elapsed.as_millis(),
                "stdout_bytes": stdout_bytes,
                "stderr_bytes": stderr_bytes,
                "stdout_truncated": result.stdout.is_truncated(),
                "stderr_truncated": result.stderr.is_truncated(),
            }),
        )?;
        if result.timed_out || result.cancelled {
            return Err(format!(
                "native Make {phase} did not complete within its control"
            ));
        }
        if result.stdout.is_truncated() || result.stderr.is_truncated() {
            return Err(format!(
                "native Make {phase} exceeded the stream retention bound"
            ));
        }
        if !result.status.success() {
            let detail = bounded_output_detail(&result.stdout, &result.stderr);
            return Err(format!("native Make {phase} failed: {detail}"));
        }
        let stdout = result
            .stdout
            .exact_bytes()
            .ok_or_else(|| format!("native Make {phase} stdout is incomplete"))?
            .to_vec();
        Ok(NativeResult {
            stdout,
            status: result.status.code(),
            elapsed_ms: result.elapsed.as_millis(),
        })
    }

    fn environment_names(namespace: &ValidatedMakeNamespace) -> BTreeSet<String> {
        namespace
            .to_observation()
            .entries
            .into_iter()
            .filter(|entry| entry.origin == MakeVariableOrigin::Environment)
            .map(|entry| entry.name)
            .collect()
    }

    fn read_records(
        reads: &aros_common::make_input_snapshot::VerifiedMakeInputReads,
    ) -> Vec<Value> {
        reads
            .reads()
            .iter()
            .map(|read| {
                json!({
                    "requested": read.requested,
                    "identity": read.identity,
                    "content_sha256": read.content_sha256,
                })
            })
            .collect()
    }

    fn run_observation_and_replay(
        paths: &ValidatedPaths,
        case: &CapturedRootCase,
        snapshot: &MakeInputSnapshot,
        output: &PrivateOutput,
    ) -> Result<Value, String> {
        let observer_path = output.root.join("observer.mk");
        let empty_path = output.root.join("empty.mk");
        let observer_identity = observer_path
            .to_str()
            .ok_or_else(|| "private observer path is not UTF-8".to_owned())?;
        let empty_identity = empty_path
            .to_str()
            .ok_or_else(|| "private empty-input path is not UTF-8".to_owned())?;
        let goal = case
            .dispatch
            .as_ref()
            .map_or("_capture_probe", |dispatch| dispatch.goal.as_str());
        if goal.is_empty()
            || goal.starts_with('-')
            || !goal.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+')
            })
        {
            return Err("private source-observation goal is not a literal token".into());
        }
        let arguments = vec![
            "-r".to_owned(),
            "-n".to_owned(),
            "-s".to_owned(),
            "-f".to_owned(),
            observer_identity.to_owned(),
            "-o".to_owned(),
            observer_identity.to_owned(),
            "-f".to_owned(),
            empty_identity.to_owned(),
            "-o".to_owned(),
            empty_identity.to_owned(),
            goal.to_owned(),
        ];
        let startup = LiteralMakeStartupArguments::admit(&arguments)
            .map_err(|error| format!("synthetic native startup admission refused: {error}"))?;
        if startup.arguments() != arguments {
            return Err("native startup admission changed the exact argument vector".into());
        }
        validate_source_prelude_inputs(&case.environment, &BTreeMap::new())
            .map_err(|error| format!("selected environment controls are not closed: {error}"))?;

        output.write_new("empty.mk", format!(".PHONY: {goal}\n{goal}:\n").as_bytes())?;
        output.write_new("observer.mk", observer_text(None)?.as_bytes())?;
        let first = run_native(paths, case, &startup, output, "first")?;
        let first_namespace = ValidatedMakeNamespace::from_make_stdout(&first.stdout)
            .map_err(|error| format!("first native namespace is invalid: {error}"))?;
        let first_defaults =
            ValidatedMakeInitialValues::from_make_stdout(&first.stdout, &first_namespace)
                .map_err(|error| format!("first native default packet is invalid: {error}"))?;
        let first_controls =
            ValidatedMakeParserControls::from_make_stdout(&first.stdout, &first_namespace)
                .map_err(|error| {
                    format!("first native parser-control packet is invalid: {error}")
                })?;
        let empty_command_names = BTreeSet::new();
        let first_command = ValidatedMakeCommandLineValues::from_make_stdout(
            &first.stdout,
            &first_namespace,
            &empty_command_names,
        )
        .map_err(|error| format!("first native command packet is invalid: {error}"))?;
        let expected_environment_names = environment_names(&first_namespace);

        // The second call keeps the identical argv, cwd and environment. Only
        // the private observer file gains the explicitly selected raw-value
        // fragment; no actual Source Makefile is passed to GNU Make.
        output.replace_owned(
            "observer.mk",
            observer_text(Some(&expected_environment_names))?.as_bytes(),
        )?;
        let second = run_native(paths, case, &startup, output, "second")?;
        let namespace = ValidatedMakeNamespace::from_make_stdout(&second.stdout)
            .map_err(|error| format!("second native namespace is invalid: {error}"))?;
        if namespace != first_namespace {
            return Err("native namespace changed between identical contexts".into());
        }
        let defaults = ValidatedMakeInitialValues::from_make_stdout(&second.stdout, &namespace)
            .map_err(|error| format!("second native default packet is invalid: {error}"))?;
        if defaults != first_defaults {
            return Err("native default-value packet changed between observer phases".into());
        }
        let controls = ValidatedMakeParserControls::from_make_stdout(&second.stdout, &namespace)
            .map_err(|error| format!("second native parser-control packet is invalid: {error}"))?;
        if controls != first_controls {
            return Err("native parser controls changed between observer phases".into());
        }
        let command_values = ValidatedMakeCommandLineValues::from_make_stdout(
            &second.stdout,
            &namespace,
            &empty_command_names,
        )
        .map_err(|error| format!("second native command packet is invalid: {error}"))?;
        if command_values != first_command {
            return Err("native command-value packet changed between observer phases".into());
        }
        let environment_values = ValidatedMakeEnvironmentValues::from_make_stdout(
            &second.stdout,
            &namespace,
            &expected_environment_names,
        )
        .map_err(|error| format!("native environment packet is invalid: {error}"))?;

        let dirs = DirVars::default();
        let make_paths = MakeProgramPaths {
            dirs: &dirs,
            source_root: &paths.source_root,
            relative_dir: Path::new(""),
            top_directory: &paths.build_root,
            working_directory: &paths.build_root,
        };
        let original_environment_names = case.environment.keys().cloned().collect();
        let mut program = MakeVariableProgram::from_native_source_observation(
            &make_paths,
            &second.stdout,
            &startup,
            &original_environment_names,
            &expected_environment_names,
        )
        .map_err(|error| format!("native initial-state composition refused: {error}"))?;

        if let Some(dispatch) = &case.dispatch {
            use aros_transpiler::root_dispatch_capture::{
                RootDispatchCapturePlan, RootDispatchCaptureSelection,
            };
            let pending = output.root.join("pending-command.json");
            let direct_startup = LiteralMakeStartupArguments::admit(&[
                "-r".into(),
                "-s".into(),
                "--no-print-directory".into(),
                "-f".into(),
                "Makefile".into(),
                dispatch.goal.clone(),
            ])
            .map_err(|_| "selected Root dispatch startup refused")?;
            // No recorder or original Source recipe is launched by this test.
            let plan = RootDispatchCapturePlan::derive_captured(
                "Makefile",
                &dispatch.root_sha256,
                RootDispatchCaptureSelection {
                    target_pattern: &dispatch.pattern,
                    goal: &dispatch.goal,
                    prefix_variable: &dispatch.prefix_variable,
                    program_variable: &dispatch.program_variable,
                    options_variable: &dispatch.options_variable,
                    project: &dispatch.project,
                    original_prefix: &dispatch.prefix,
                    program: &dispatch.program,
                    options: &dispatch.options,
                    observer: Path::new(env!("CARGO_BIN_EXE_aros-transpiler")),
                    output: &pending,
                    working_directory: &paths.build_root,
                },
                &mut program,
                &make_paths,
                snapshot,
                &direct_startup,
            )
            .map_err(|error| format!("captured Root dispatch selection refused: {error}"))?;
            plan.revalidate()?;
            if pending.exists() {
                return Err("data-only dispatch test unexpectedly wrote a capture".into());
            }
            output.write_json(
                "dispatch-source-reads.json",
                &json!(read_records(plan.source_reads())),
            )?;
            return Ok(json!({
                "qualification": "current-captured-source-dispatch-data-only-not-live-launch-or-original-owner-proof",
                "root_sha256": plan.capture_plan().original_sha256(),
                "override_sha256": sha256_bytes(plan.capture_plan().override_bytes()),
                "source_read_count": plan.source_reads().reads().len(),
                "role_read_count": plan.role_reads().reads().len(),
                "original_program_executed": false,
            }));
        }

        // This source interpreter reads only from the previously measured,
        // complete Source/build snapshot. Include search remains unbound so an
        // include causes an explicit refusal instead of an invented empty path.
        let (source, source_reads) = program
            .read_captured_file("Makefile", &make_paths, snapshot)
            .map_err(|error| format!("captured root Makefile replay refused: {error}"))?;
        let suffixes = source
            .suffixes(None)
            .map_err(|error| format!("captured suffix state refused: {error}"))?;
        let suffix_bytes = serde_json::to_vec(&suffixes)
            .map_err(|_| "cannot serialize private captured suffix state".to_owned())?;
        output.write_new("captured-suffix-state.json", &suffix_bytes)?;
        let default_rules = default_rule_records(&source)?;
        let default_bytes = serde_json::to_vec(&default_rules)
            .map_err(|_| "cannot serialize private captured default declarations".to_owned())?;
        output.write_new("captured-default-rules.json", &default_bytes)?;
        let (result, evaluation_reads) = program
            .evaluate_captured("$(CALL) $(MMAKE) $(MMAKE_OPTIONS)", &make_paths, snapshot)
            .map_err(|error| format!("captured command expression evaluation refused: {error}"))?;
        output.write_new("captured-expression-result.bin", result.as_bytes())?;
        let (source_exports, export_reads) = program
            .source_export_values_captured(&make_paths, snapshot)
            .map_err(|error| format!("captured source export projection refused: {error}"))?;
        let export_bytes = serde_json::to_vec(&source_exports)
            .map_err(|_| "cannot serialize private source export projection".to_owned())?;
        output.write_new("captured-source-exports.json", &export_bytes)?;

        let result_digest = sha256_bytes(result.as_bytes());
        Ok(json!({
            "result": "data-only replay completed",
            "observation_phase": "source prelude before selected empty input",
            "argv_count": startup.arguments().len(),
            "command_line_names": startup.command_line_names().len(),
            "namespace_entries": namespace.to_observation().entries.len(),
            "default_value_entries": defaults.entries().count(),
            "parser_control_entries": controls.entries().count(),
            "command_line_value_entries": command_values.entries().count(),
            "environment_name_count": expected_environment_names.len(),
            "environment_value_entries": environment_values.entries().count(),
            "first_native_status": first.status,
            "second_native_status": second.status,
            "first_native_elapsed_ms": first.elapsed_ms,
            "second_native_elapsed_ms": second.elapsed_ms,
            "first_stdout_sha256": sha256_bytes(&first.stdout),
            "second_stdout_sha256": sha256_bytes(&second.stdout),
            "source_event_count": source.events.len(),
            "source_suffix_list_known": suffixes.is_some(),
            "source_suffix_count": suffixes.as_ref().map(Vec::len),
            "source_suffix_state_sha256": sha256_bytes(&suffix_bytes),
            "source_default_declaration_count": default_rules.len(),
            "source_default_declarations_sha256": sha256_bytes(&default_bytes),
            "source_read_count": source_reads.reads().len(),
            "source_reads": read_records(&source_reads),
            "evaluation_read_count": evaluation_reads.reads().len(),
            "evaluation_reads": read_records(&evaluation_reads),
            "expression_result_bytes": result.len(),
            "expression_result_sha256": result_digest,
            "expression_result_file": "captured-expression-result.bin",
            "source_export_count": source_exports.len(),
            "source_export_bytes": export_bytes.len(),
            "source_exports_sha256": sha256_bytes(&export_bytes),
            "source_exports_file": "captured-source-exports.json",
            "source_export_read_count": export_reads.reads().len(),
            "source_export_reads": read_records(&export_reads),
            "complete_native_child_environment_proven": false,
            "complete_source_program_or_receipt_proven": false,
        }))
    }

    fn execute_case(
        case: &CapturedRootCase,
        paths: &ValidatedPaths,
        output: &PrivateOutput,
    ) -> Result<Value, String> {
        if case.environment.len() > 4096 {
            return Err("explicit environment exceeds the retained name bound".into());
        }
        let mut environment_bytes = 0usize;
        for (name, value) in &case.environment {
            if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
                return Err("explicit environment contains an invalid OS binding".into());
            }
            environment_bytes = environment_bytes
                .checked_add(name.len())
                .and_then(|size| size.checked_add(value.len()))
                .ok_or_else(|| "explicit environment byte count overflowed".to_owned())?;
            if environment_bytes > 1024 * 1024 {
                return Err("explicit environment exceeds the byte bound".into());
            }
        }

        let limits = TreeTraversalLimits::new(ROOT_ENTRY_LIMIT, ROOT_BYTES_LIMIT)
            .map_err(|_| "invalid complete input-tree measurement limits".to_owned())?;
        let source_tree = measure_root(&paths.source_root, limits)?;
        let build_tree = measure_root(&paths.build_root, limits)?;
        let source_record = root_record(&paths.source_root, &source_tree)?;
        let build_record = root_record(&paths.build_root, &build_tree)?;
        output.write_json(
            "selected-input-cas.json",
            &json!({
                "classification": "current input data selection; not an authenticated historical receipt",
                "source": source_record,
                "build": build_record,
            }),
        )?;

        let snapshot = match MakeInputSnapshot::capture(
            &paths.build_root,
            &[
                MakeInputRootSelection {
                    root: paths.source_root.clone(),
                    content_sha256: source_tree.payload_digest_excluding(None),
                },
                MakeInputRootSelection {
                    root: paths.build_root.clone(),
                    content_sha256: build_tree.payload_digest_excluding(None),
                },
            ],
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let verification =
                    verify_after_capture_failure(paths, &source_tree, &build_tree, limits, output);
                return Err(match verification {
                    Ok(()) => format!("complete input snapshot refused: {error}"),
                    Err(verify_error) => {
                        format!("complete input snapshot refused: {error}; {verify_error}")
                    }
                });
            }
        };

        let make_before = pin_make(&paths.make, &case.make_sha256);
        let operation = match make_before {
            Ok(make_before) => {
                let result = run_observation_and_replay(paths, case, &snapshot, output);
                let make_after = pin_make(&paths.make, &case.make_sha256);
                let roots_verified = snapshot.verify();
                output.write_json(
                    "post-run-verification.json",
                    &json!({
                        "make_before": make_before,
                        "make_after": make_after.as_ref().ok(),
                        "make_after_error": make_after.as_ref().err(),
                        "input_roots_verified": roots_verified.is_ok(),
                        "input_roots_error": roots_verified.as_ref().err(),
                    }),
                )?;
                match (result, make_after, roots_verified) {
                    (Ok(report), Ok(_), Ok(())) => Ok(report),
                    (result, make_after, roots_verified) => {
                        let mut failures = Vec::new();
                        if let Err(error) = result {
                            failures.push(error);
                        }
                        if let Err(error) = make_after {
                            failures.push(error);
                        }
                        if let Err(error) = roots_verified {
                            failures.push(format!("input root verification failed: {error}"));
                        }
                        Err(failures.join("\n"))
                    }
                }
            }
            Err(error) => {
                let roots_verified = snapshot.verify();
                output.write_json(
                    "post-run-verification.json",
                    &json!({
                        "make_before_error": error,
                        "input_roots_verified": roots_verified.is_ok(),
                        "input_roots_error": roots_verified.as_ref().err(),
                    }),
                )?;
                match roots_verified {
                    Ok(()) => Err(error),
                    Err(verify_error) => Err(format!(
                        "{error}; input root verification failed: {verify_error}"
                    )),
                }
            }
        }?;

        Ok(json!({
            "status": "success",
            "source_root_sha256": source_tree.payload_digest_excluding(None),
            "source_root_entries": source_tree.entry_count(),
            "build_root_sha256": build_tree.payload_digest_excluding(None),
            "build_root_entries": build_tree.entry_count(),
            "environment_name_count": case.environment.len(),
            "native_and_replay": operation,
            "qualification": "current selected source/build data replay under the explicit test environment; not original owner-37705 qualification or native receipt proof",
        }))
    }

    fn create_private_output(
        case: &CapturedRootCase,
    ) -> Result<(ValidatedPaths, PrivateOutput), String> {
        let paths = validate_paths(case)?;
        let output = PrivateOutput::create(paths.output_root.clone())?;
        Ok((paths, output))
    }

    #[test]
    #[ignore = "requires explicit canonical Source/build roots, pinned Make, environment, and a new private output directory"]
    fn actual_captured_root_source_replay() {
        let encoded_case = std::env::var(CASE_ENVIRONMENT)
            .expect("explicit captured-root case JSON is required for this ignored test");
        assert!(
            encoded_case.len() <= 1024 * 1024,
            "case JSON exceeds its bound"
        );
        let case: CapturedRootCase = serde_json::from_str(&encoded_case)
            .expect("captured-root case JSON must match its closed schema");
        let (paths, output) = create_private_output(&case)
            .expect("captured-root paths and new private output directory must be safe");

        // The native observer below receives only the private empty goal file.
        // The actual Source/build Makefile is opened later by the data-only
        // interpreter through MakeInputSnapshot, never by GNU Make here.
        let result = execute_case(&case, &paths, &output);
        let report = match &result {
            Ok(report) => json!({ "status": "success", "details": report }),
            Err(detail) => {
                output
                    .write_new("refusal.txt", detail.as_bytes())
                    .expect("retain private refusal details");
                json!({ "status": "refused", "private_detail_file": "refusal.txt" })
            }
        };
        output
            .write_json("report.json", &report)
            .expect("retain private captured-root report");
        assert!(
            result.is_ok(),
            "captured-root source replay refused; private diagnostics were retained"
        );
    }
}
