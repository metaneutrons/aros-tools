//! Opt-in replay of explicitly selected current Source/build data through the
//! production first-input observer and the captured-input interpreter.
//!
//! This is a data-only configured-root experiment. The external case selects
//! the roots, executable, invocation, and queries; observed hashes are never
//! adopted as input identity. A successful result does not authenticate an
//! owner, establish ordinary-root behavior, prove preparation or SDK execution,
//! or constitute a native execution receipt.

#[cfg(unix)]
mod unix {
    mod recipe;

    use aros_common::{
        make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
        make_native_invocation::{NativeMakeInvocation, PrivateNativeCaptureDirectory},
        make_native_observation::{
            MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
        },
        measure_regular_file_bounded, measure_tree_content_cas_bounded,
        publication::filesystem_paths_overlap,
        run_output_with_control, sha256_bytes, CancellationToken, Sha256Digest, TreeContentCas,
        TreeTraversalLimits,
    };
    use aros_transpiler::{
        dirs::DirVars,
        make_vars::{MakeProgramPaths, MakeVariableProgram},
    };
    use serde::Deserialize;
    use serde_json::{json, Value};
    use std::{
        collections::BTreeMap,
        ffi::OsStr,
        fs::{self, DirBuilder},
        os::unix::fs::{DirBuilderExt, PermissionsExt},
        path::{Component, Path, PathBuf},
        process::Command,
        time::Duration,
    };

    const CASE_ENVIRONMENT: &str = "AROS_TEST_NATIVE_ROOT_REPLAY_CASE";
    const CASE_BYTES_LIMIT: u64 = 1024 * 1024;
    const MAKE_BYTES_LIMIT: u64 = 16 * 1024 * 1024;
    const FRONTEND_BYTES_LIMIT: u64 = 256 * 1024 * 1024;
    const ROOT_ENTRY_LIMIT: usize = 100_000;
    const ROOT_BYTES_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
    const STREAM_LIMIT: usize = 256 * 1024;
    const QUERY_COUNT_LIMIT: usize = 1024;
    const QUERY_TEXT_LIMIT: usize = 1024 * 1024;
    const QUERY_TOTAL_LIMIT: usize = 4 * 1024 * 1024;

    const QUALIFICATION: &str = "data-only configured Source/build replay; not authentication, ordinary-root behavior, preparation, SDK execution, or a native receipt";

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Case {
        source_root: PathBuf,
        source_sha256: Sha256Digest,
        build_root: PathBuf,
        build_sha256: Sha256Digest,
        make: PathBuf,
        make_sha256: Sha256Digest,
        output_root: PathBuf,
        environment: BTreeMap<String, String>,
        arguments: Vec<String>,
        queries: BTreeMap<String, String>,
        #[serde(default)]
        expected_values: Option<BTreeMap<String, String>>,
        #[serde(default)]
        recipe: Option<recipe::Selection>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Selection {
        version: u32,
        qualification: String,
        document_sha256: Sha256Digest,
        binding: MakeObservationBinding,
    }

    struct Paths {
        source_root: PathBuf,
        build_root: PathBuf,
        make: PathBuf,
        frontend: PathBuf,
        frontend_sha256: Sha256Digest,
        output_root: PathBuf,
    }

    struct PrivateOutput {
        root: PathBuf,
        observation: PathBuf,
        directory: PrivateNativeCaptureDirectory,
    }

    fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
        if !path.is_absolute() {
            return Err("selected input directory is not absolute".into());
        }
        let canonical = fs::canonicalize(path)
            .map_err(|_| "cannot canonicalize selected input directory".to_owned())?;
        if canonical.as_path() != path || !canonical.is_dir() || canonical.to_str().is_none() {
            return Err("selected input directory must be canonical UTF-8".into());
        }
        Ok(canonical)
    }

    fn canonical_executable(path: &Path, limit: u64) -> Result<PathBuf, String> {
        if !path.is_absolute() {
            return Err("selected executable is not absolute".into());
        }
        let canonical = fs::canonicalize(path)
            .map_err(|_| "cannot canonicalize selected executable".to_owned())?;
        if canonical.as_path() != path || canonical.to_str().is_none() {
            return Err("selected executable must be canonical UTF-8".into());
        }
        let metadata = fs::metadata(&canonical)
            .map_err(|_| "cannot inspect selected executable".to_owned())?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err("selected executable is not a regular executable".into());
        }
        let _ = measure_regular_file_bounded(&canonical, limit)
            .map_err(|_| "selected executable exceeds its measurement bound".to_owned())?
            .ok_or_else(|| "selected executable is absent".to_owned())?;
        Ok(canonical)
    }

    fn prospective_output(path: &Path) -> Result<PathBuf, String> {
        if !path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
        {
            return Err("private output path must be normalized and absolute".into());
        }
        let name = path
            .file_name()
            .ok_or_else(|| "private output path has no final component".to_owned())?;
        let parent = path
            .parent()
            .ok_or_else(|| "private output path has no parent".to_owned())?;
        let canonical_parent = fs::canonicalize(parent)
            .map_err(|_| "cannot canonicalize private output parent".to_owned())?;
        let output = canonical_parent.join(name);
        if !canonical_parent.is_dir() || output.as_path() != path {
            return Err("private output path must use a canonical parent".into());
        }
        match fs::symlink_metadata(&output) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err("private output directory already exists".into()),
            Err(_) => return Err("cannot inspect private output path".into()),
        }
        Ok(output)
    }

    fn validate_paths(case: &Case) -> Result<Paths, String> {
        let source_root = canonical_directory(&case.source_root)?;
        let build_root = canonical_directory(&case.build_root)?;
        let make = canonical_executable(&case.make, MAKE_BYTES_LIMIT)?;
        let frontend = canonical_executable(
            Path::new(env!("CARGO_BIN_EXE_aros-transpiler")),
            FRONTEND_BYTES_LIMIT,
        )?;
        let (_, frontend_bytes) = measure_regular_file_bounded(&frontend, FRONTEND_BYTES_LIMIT)
            .map_err(|_| "cannot measure selected frontend".to_owned())?
            .ok_or_else(|| "selected frontend is absent".to_owned())?;
        let frontend_sha256 = sha256_bytes(&frontend_bytes);
        if filesystem_paths_overlap(&source_root, &build_root)
            .map_err(|_| "cannot compare selected root paths".to_owned())?
        {
            return Err("selected Source and build roots overlap".into());
        }
        let output_root = prospective_output(&case.output_root)?;
        for input in [&source_root, &build_root, &make, &frontend] {
            if filesystem_paths_overlap(input, &output_root)
                .map_err(|_| "cannot compare output and input paths".to_owned())?
            {
                return Err("private output overlaps a selected input root".into());
            }
        }
        Ok(Paths {
            source_root,
            build_root,
            make,
            frontend,
            frontend_sha256,
            output_root,
        })
    }

    impl PrivateOutput {
        fn create(root: PathBuf) -> Result<Self, String> {
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            builder
                .create(&root)
                .map_err(|_| "cannot create private output directory".to_owned())?;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .map_err(|_| "cannot restrict private output directory".to_owned())?;
            let observation = root.join("observation");
            let mut observation_builder = DirBuilder::new();
            observation_builder.mode(0o700);
            observation_builder
                .create(&observation)
                .map_err(|_| "cannot create private observation directory".to_owned())?;
            fs::set_permissions(&observation, fs::Permissions::from_mode(0o700))
                .map_err(|_| "cannot restrict private observation directory".to_owned())?;
            for path in [&root, &observation] {
                let metadata = fs::symlink_metadata(path)
                    .map_err(|_| "cannot inspect private output directory".to_owned())?;
                if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o777 != 0o700
                {
                    return Err("private output directory did not retain mode 0700".into());
                }
            }
            let directory = PrivateNativeCaptureDirectory::open(&root)
                .map_err(|_| "cannot bind private output directory".to_owned())?;
            Ok(Self {
                root,
                observation,
                directory,
            })
        }

        fn write_new(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
            self.directory
                .write_new(OsStr::new(name), bytes)
                .map_err(|_| "cannot retain private artifact".to_owned())
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
            let mut bytes = Vec::new();
            stream
                .write_rendered(&mut bytes)
                .map_err(|_| "cannot retain private frontend stream".to_owned())?;
            self.write_new(name, &bytes)
        }
    }

    fn load_case() -> Result<(Case, Vec<u8>, PathBuf), String> {
        let raw_path = std::env::var_os(CASE_ENVIRONMENT)
            .ok_or_else(|| "explicit native-root case file is required".to_owned())?;
        let path = fs::canonicalize(PathBuf::from(raw_path))
            .map_err(|_| "cannot canonicalize native-root case file".to_owned())?;
        let (_, bytes) = measure_regular_file_bounded(&path, CASE_BYTES_LIMIT)
            .map_err(|_| "cannot measure native-root case file".to_owned())?
            .ok_or_else(|| "native-root case file is absent".to_owned())?;
        let case = serde_json::from_slice(&bytes)
            .map_err(|_| "native-root case file does not match its closed schema".to_owned())?;
        Ok((case, bytes, path))
    }

    fn pin_executable(path: &Path, expected: &Sha256Digest, limit: u64) -> Result<Value, String> {
        let canonical = fs::canonicalize(path)
            .map_err(|_| "cannot revalidate selected executable".to_owned())?;
        if canonical.as_path() != path {
            return Err("selected executable identity changed".into());
        }
        let metadata =
            fs::metadata(path).map_err(|_| "cannot inspect selected executable".to_owned())?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err("selected executable is no longer executable".into());
        }
        let (_, bytes) = measure_regular_file_bounded(path, limit)
            .map_err(|_| "cannot measure selected executable".to_owned())?
            .ok_or_else(|| "selected executable is absent".to_owned())?;
        let digest = sha256_bytes(&bytes);
        if &digest != expected {
            return Err("selected executable digest differs from its external selection".into());
        }
        Ok(json!({ "sha256": digest, "bytes": bytes.len() }))
    }

    fn measure_root(path: &Path, limits: TreeTraversalLimits) -> Result<TreeContentCas, String> {
        measure_tree_content_cas_bounded(path, limits)
            .map_err(|_| "complete selected input-root measurement failed".to_owned())
    }

    fn root_selection(path: &Path, tree: &TreeContentCas) -> Result<Value, String> {
        Ok(json!({
            "path": path.to_str().ok_or("selected input root is not UTF-8")?,
            "entries": tree.entry_count(),
            "content_sha256": tree.payload_digest_excluding(None),
        }))
    }

    fn validate_case_data(case: &Case) -> Result<NativeMakeInvocation, String> {
        if let Some(selection) = &case.recipe {
            selection.validate_arguments(&case.arguments)?;
        }
        if case.queries.is_empty() || case.queries.len() > QUERY_COUNT_LIMIT {
            return Err("case query count is empty or exceeds its bound".into());
        }
        let mut query_bytes = 0usize;
        for (name, expression) in &case.queries {
            if name.is_empty()
                || name.contains('\0')
                || expression.contains('\0')
                || expression.len() > QUERY_TEXT_LIMIT
            {
                return Err("case query name or expression is invalid".into());
            }
            query_bytes = query_bytes
                .checked_add(name.len())
                .and_then(|total| total.checked_add(expression.len()))
                .ok_or_else(|| "case query byte count overflowed".to_owned())?;
            if query_bytes > QUERY_TOTAL_LIMIT {
                return Err("case query text exceeds its aggregate bound".into());
            }
        }
        if let Some(expected) = &case.expected_values {
            if expected.keys().any(|name| !case.queries.contains_key(name)) {
                return Err("expected-value keys must be a subset of query keys".into());
            }
        }
        NativeMakeInvocation::new(
            case.arguments.clone(),
            case.build_root.clone(),
            case.environment
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        )
        .map_err(|_| "case invocation exceeds native argument or environment constraints".into())
    }

    fn read_private_file(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
        let directory =
            PrivateNativeCaptureDirectory::open(path.parent().ok_or("private file has no parent")?)
                .map_err(|_| "cannot bind retained private file parent".to_owned())?;
        let (_, bytes) = measure_regular_file_bounded(path, limit as u64)
            .map_err(|_| "cannot measure retained private file within bound".to_owned())?
            .ok_or("retained private file is absent")?;
        directory
            .read_selected(
                path.file_name().ok_or("private file has no name")?,
                &sha256_bytes(&bytes),
                limit,
            )
            .map_err(|_| "retained private file differs from bounded measurement".to_owned())
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

    fn retain_context(
        output: &PrivateOutput,
        case_bytes: &[u8],
        case_path: &Path,
        paths: &Paths,
        source_tree: &TreeContentCas,
        build_tree: &TreeContentCas,
        executable_pins: (&Value, &Value),
    ) -> Result<Sha256Digest, String> {
        let (make_pin, frontend_pin) = executable_pins;
        let context = json!({
            "version": 1,
            "case_file": case_path.to_str().ok_or("case file path is not UTF-8")?,
            "case_sha256": sha256_bytes(case_bytes),
            "source": root_selection(&paths.source_root, source_tree)?,
            "build": root_selection(&paths.build_root, build_tree)?,
            "make": {
                "path": paths.make.to_str().ok_or("Make path is not UTF-8")?,
                "measurement": make_pin,
            },
            "frontend": {
                "path": paths.frontend.to_str().ok_or("frontend path is not UTF-8")?,
                "measurement": frontend_pin,
            },
        });
        let bytes = serde_json::to_vec_pretty(&context)
            .map_err(|_| "cannot serialize selected native-root context".to_owned())?;
        let digest = sha256_bytes(&bytes);
        output.write_new("selected-context.json", &bytes)?;
        output.write_new("selected-context.sha256", digest.as_str().as_bytes())?;
        Ok(digest)
    }

    fn retain_frontend_output(
        output: &PrivateOutput,
        paths: &Paths,
        case: &Case,
        control_sha256: &Sha256Digest,
    ) -> Result<(), String> {
        let mut command = Command::new(&paths.frontend);
        command
            .env_clear()
            .envs(&case.environment)
            .current_dir(&paths.build_root)
            .arg("--observe-make-launch")
            .arg(output.root.join("native-control.json"))
            .arg(control_sha256.as_str())
            .arg("--")
            .args(&case.arguments);
        let result = run_output_with_control(
            &mut command,
            STREAM_LIMIT,
            Duration::from_secs(45),
            &CancellationToken::default(),
        );
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                output.write_json(
                    "frontend.status.json",
                    &json!({"launched": false, "error": error.to_string()}),
                )?;
                return Err("controlled native observation frontend launch failed".into());
            }
        };
        output.write_stream("frontend.stdout", &result.stdout)?;
        output.write_stream("frontend.stderr", &result.stderr)?;
        let status_code = result.status.code();
        output.write_json(
            "frontend.status.json",
            &json!({
                "launched": true,
                "status": status_code,
                "success": result.status.success(),
                "timed_out": result.timed_out,
                "cancelled": result.cancelled,
                "elapsed_ms": result.elapsed.as_millis(),
                "stdout_bytes": result.stdout.total_bytes(),
                "stderr_bytes": result.stderr.total_bytes(),
                "stdout_truncated": result.stdout.is_truncated(),
                "stderr_truncated": result.stderr.is_truncated(),
                "stdout_sha256": result.stdout.exact_bytes().map(sha256_bytes),
                "stderr_sha256": result.stderr.exact_bytes().map(sha256_bytes),
            }),
        )?;
        if status_code != Some(0)
            || result.timed_out
            || result.cancelled
            || result.stdout.is_truncated()
            || result.stderr.is_truncated()
        {
            return Err(
                "native observation frontend did not complete successfully within bounds".into(),
            );
        }
        Ok(())
    }

    fn create_control(
        output: &PrivateOutput,
        paths: &Paths,
        make_sha256: &Sha256Digest,
        context_sha256: &Sha256Digest,
    ) -> Result<Sha256Digest, String> {
        let control = json!({
            "version": 2,
            "output_root": output.observation,
            "program": paths.make,
            "executable_sha256": make_sha256,
            "context_sha256": context_sha256,
            "phase": "first-input-prelude",
        });
        let bytes = serde_json::to_vec(&control)
            .map_err(|_| "cannot encode private native control".to_owned())?;
        let directory = PrivateNativeCaptureDirectory::open(&output.root)
            .map_err(|_| "cannot bind private native control directory".to_owned())?;
        directory
            .write_new(OsStr::new("native-control.json"), &bytes)
            .map_err(|_| "cannot retain private native control".to_owned())?;
        Ok(sha256_bytes(&bytes))
    }

    fn bind_observation(
        output: &PrivateOutput,
        context_sha256: &Sha256Digest,
        expected_invocation: &NativeMakeInvocation,
    ) -> Result<RetainedMakeObservation, String> {
        let selection_bytes = read_private_file(
            &output.observation.join("observation-selection.json"),
            64 * 1024,
        )?;
        let selection: Selection = serde_json::from_slice(&selection_bytes)
            .map_err(|_| "retained native observation selection is malformed".to_owned())?;
        if selection.version != 1
            || selection.qualification != "instrumented-first-input-observation-only"
        {
            return Err("retained native observation has the wrong qualification".into());
        }
        let document = read_private_file(
            &output.observation.join("observation.json"),
            4 * 1024 * 1024,
        )?;
        let invocation_bytes =
            read_private_file(&output.observation.join("invocation.json"), 2 * 1024 * 1024)?;
        let stdout = read_private_file(&output.observation.join("initial.stdout"), 256 * 1024)?;
        let document_sha256 = sha256_bytes(&document);
        let invocation_sha256 = sha256_bytes(&invocation_bytes);
        let stdout_sha256 = sha256_bytes(&stdout);
        if selection.document_sha256 != document_sha256 {
            return Err("retained observation document digest differs from its selection".into());
        }
        let invocation = NativeMakeInvocation::parse(&invocation_bytes)
            .map_err(|_| "retained native invocation is malformed".to_owned())?;
        if &invocation != expected_invocation {
            return Err("retained invocation differs from the externally selected case".into());
        }
        let expected_binding = MakeObservationBinding {
            phase: MakeObservationPhase::FirstInputPrelude,
            context_sha256: context_sha256.clone(),
            invocation_sha256,
            stdout_sha256,
        };
        if selection.binding != expected_binding {
            return Err("retained native selection differs from independent bindings".into());
        }
        let retained =
            RetainedMakeObservation::load(&document, &document_sha256, &expected_binding)
                .map_err(|_| "retained native observation failed exact binding".to_owned())?;
        if retained.binding() != &expected_binding
            || retained.invocation() != expected_invocation
            || retained.initial_stdout() != stdout.as_slice()
        {
            return Err("loaded native observation differs from exact retained inputs".into());
        }
        Ok(retained)
    }

    fn replay_queries(
        output: &PrivateOutput,
        case: &Case,
        paths: &Paths,
        snapshot: &MakeInputSnapshot,
        observation: &RetainedMakeObservation,
    ) -> Result<Value, String> {
        let dirs = DirVars::default();
        let make_paths = MakeProgramPaths {
            dirs: &dirs,
            source_root: &paths.source_root,
            relative_dir: Path::new(""),
            top_directory: &paths.build_root,
            working_directory: &paths.build_root,
        };
        let (mut program, source, source_reads) = MakeVariableProgram::read_native_captured_inputs(
            &make_paths,
            observation.initial_stdout(),
            observation.invocation(),
            observation.environment_names(),
            snapshot,
        )
        .map_err(|error| format!("captured native input replay refused: {error}"))?;

        // Select deferred text from the actual parsed declarations before any
        // caller query can change the program. This is declaration-only data;
        // no prerequisite preparation or native recipe phase is inferred.
        let recipe = case
            .recipe
            .as_ref()
            .map(|selection| {
                recipe::project(
                    output,
                    selection,
                    &source,
                    &mut program,
                    &make_paths,
                    snapshot,
                )
            })
            .transpose()?;

        let ordered_queries: Vec<_> = case.queries.iter().collect();
        let expressions: Vec<_> = ordered_queries
            .iter()
            .map(|(_, expression)| expression.as_str())
            .collect();
        let (values, query_reads) =
            match program.evaluate_many_captured(&expressions, &make_paths, snapshot) {
                Ok(batch) => batch,
                Err(error) => {
                    output.write_json(
                        "query-errors.json",
                        &json!({
                            "error": error,
                            "queries": ordered_queries.iter().map(|(name, expression)| {
                                json!({"name": name, "expression": expression})
                            }).collect::<Vec<_>>(),
                        }),
                    )?;
                    return Err("captured query batch failed".into());
                }
            };
        if values.len() != ordered_queries.len() {
            output.write_json(
                "query-errors.json",
                &json!({"error": "captured query batch returned an unexpected value count"}),
            )?;
            return Err("captured query batch returned an unexpected value count".into());
        }

        let mut results = Vec::with_capacity(ordered_queries.len());
        let mut expectations_match = true;
        for ((name, expression), value) in ordered_queries.into_iter().zip(values) {
            let expected_match = case
                .expected_values
                .as_ref()
                .and_then(|expected| expected.get(name))
                .is_none_or(|expected| expected == &value);
            expectations_match &= expected_match;
            results.push(json!({
                "name": name,
                "expression": expression,
                "value": value,
                "expected_match": expected_match,
            }));
        }
        output.write_json(
            "query-results.json",
            &json!({
                "results": results,
                "source_read_count": source_reads.reads().len(),
                "source_reads": read_records(&source_reads),
                "query_reads": read_records(&query_reads),
            }),
        )?;
        if !expectations_match {
            return Err("one or more selected expected values did not match".into());
        }
        Ok(json!({
            "source_event_count": source.events.len(),
            "query_count": case.queries.len(),
            "expected_value_count": case.expected_values.as_ref().map_or(0, BTreeMap::len),
            "all_expected_values_match": expectations_match,
            "raw_query_results_file": "query-results.json",
            "qualification": QUALIFICATION,
            "recipe_projection": recipe,
        }))
    }

    fn execute_case(
        case: &Case,
        case_bytes: &[u8],
        case_path: &Path,
        paths: &Paths,
        output: &PrivateOutput,
    ) -> Result<Value, String> {
        let expected_invocation = validate_case_data(case)?;
        let limits = TreeTraversalLimits::new(ROOT_ENTRY_LIMIT, ROOT_BYTES_LIMIT)
            .map_err(|_| "invalid selected root measurement limits".to_owned())?;
        let source_tree = measure_root(&paths.source_root, limits)?;
        let build_tree = measure_root(&paths.build_root, limits)?;
        if source_tree.payload_digest_excluding(None) != case.source_sha256
            || build_tree.payload_digest_excluding(None) != case.build_sha256
            || source_tree
                .entry_count()
                .saturating_add(build_tree.entry_count())
                > ROOT_ENTRY_LIMIT
        {
            return Err("Source/build roots differ from external case selection or limits".into());
        }
        output.write_json(
            "selected-inputs.json",
            &json!({
                "classification": "externally selected current roots; no owner, producer, or receipt authentication",
                "source": root_selection(&paths.source_root, &source_tree)?,
                "build": root_selection(&paths.build_root, &build_tree)?,
            }),
        )?;

        let snapshot = MakeInputSnapshot::capture(
            &paths.build_root,
            &[
                MakeInputRootSelection {
                    root: paths.source_root.clone(),
                    content_sha256: case.source_sha256.clone(),
                },
                MakeInputRootSelection {
                    root: paths.build_root.clone(),
                    content_sha256: case.build_sha256.clone(),
                },
            ],
        )
        .map_err(|_| "complete selected Make input snapshot refused".to_owned())?;

        let operation = (|| {
            let make_before = pin_executable(&paths.make, &case.make_sha256, MAKE_BYTES_LIMIT)?;
            let frontend_before = pin_executable(
                &paths.frontend,
                &paths.frontend_sha256,
                FRONTEND_BYTES_LIMIT,
            )?;
            let context_sha256 = retain_context(
                output,
                case_bytes,
                case_path,
                paths,
                &source_tree,
                &build_tree,
                (&make_before, &frontend_before),
            )?;
            let control_sha256 = create_control(output, paths, &case.make_sha256, &context_sha256)?;
            retain_frontend_output(output, paths, case, &control_sha256)?;
            let observation = bind_observation(output, &context_sha256, &expected_invocation)?;
            replay_queries(output, case, paths, &snapshot, &observation)
        })();

        let make_after = pin_executable(&paths.make, &case.make_sha256, MAKE_BYTES_LIMIT);
        let frontend_after = pin_executable(
            &paths.frontend,
            &paths.frontend_sha256,
            FRONTEND_BYTES_LIMIT,
        );
        let roots_after = snapshot.verify();
        output.write_json(
            "post-run-verification.json",
            &json!({
                "make_after": make_after.as_ref().ok(),
                "make_after_error": make_after.as_ref().err(),
                "frontend_after": frontend_after.as_ref().ok(),
                "frontend_after_error": frontend_after.as_ref().err(),
                "input_roots_verified": roots_after.is_ok(),
                "input_roots_error": roots_after.as_ref().err(),
            }),
        )?;
        match (operation, make_after, frontend_after, roots_after) {
            (Ok(report), Ok(_), Ok(_), Ok(())) => Ok(report),
            (operation, make_after, frontend_after, roots_after) => {
                let mut failures = Vec::new();
                if let Err(error) = operation {
                    failures.push(error);
                }
                if let Err(error) = make_after {
                    failures.push(error);
                }
                if let Err(error) = frontend_after {
                    failures.push(error);
                }
                if let Err(error) = roots_after {
                    failures.push(format!("captured input root verification failed: {error}"));
                }
                Err(failures.join("\n"))
            }
        }
    }

    #[test]
    #[ignore = "requires a closed external case file, selected Source/build roots, and a new private output directory"]
    fn actual_captured_native_root_replay() {
        let (case, case_bytes, case_path) =
            load_case().expect("explicit bounded native-root case file is required");
        let paths = validate_paths(&case)
            .expect("native-root case paths must be canonical, disjoint, and safe");
        let output = PrivateOutput::create(paths.output_root.clone())
            .expect("new private native-root output directory must be safe");
        let result = execute_case(&case, &case_bytes, &case_path, &paths, &output);
        let summary = match &result {
            Ok(details) => json!({
                "status": "PASS",
                "qualification": QUALIFICATION,
                "details": details,
            }),
            Err(error) => {
                output
                    .write_new("failure.txt", error.as_bytes())
                    .expect("retain private native-root failure detail");
                json!({
                    "status": "FAIL",
                    "qualification": QUALIFICATION,
                    "failure_file": "failure.txt",
                })
            }
        };
        output
            .write_json("report.json", &summary)
            .expect("retain private native-root result summary");
        assert!(
            result.is_ok(),
            "configured native-root replay failed; inspect private output artifacts"
        );
    }
}
