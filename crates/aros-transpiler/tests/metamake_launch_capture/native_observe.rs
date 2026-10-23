//! One real MetaMake launch through the native initial-observation frontend.
//!
//! This is private integration evidence for the selected Mesa marker target,
//! not ordinary GNU Make target execution, build, source-closure, or SDK qualification.

#![cfg(unix)]

use super::{literal_template, metadata_variables, pinned, run, Case, INPUTS, OWNER, TARGET};
use aros_common::{
    make_native_collection::{read_native_make_collection, NativeMakeCallSelection},
    make_native_invocation::{NativeMakeInvocation, PrivateNativeCaptureDirectory},
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    measure_regular_file_bounded, measure_tree_content_cas, sha256_bytes, CancellationToken,
    Sha256Digest,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, FileTimes},
    io::Write as _,
    os::unix::fs::symlink,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const OBSERVER_LIMIT: u64 = 256 * 1024 * 1024;
const CONTROL_LIMIT: usize = 4096;
const CONTROL_SHA_PLACEHOLDER: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

fn selected_executable(path: &Path, expected: &Sha256Digest) -> Sha256Digest {
    assert!(
        path.is_absolute(),
        "selected executable path must be absolute"
    );
    assert_eq!(fs::canonicalize(path).unwrap(), path);
    assert!(fs::symlink_metadata(path).unwrap().file_type().is_file());
    assert_ne!(
        fs::metadata(path).unwrap().permissions().mode() & 0o111,
        0,
        "selected executable is not executable"
    );
    let (_, bytes) = measure_regular_file_bounded(path, OBSERVER_LIMIT)
        .unwrap()
        .unwrap();
    let digest = sha256_bytes(&bytes);
    assert_eq!(&digest, expected, "selected executable digest changed");
    digest
}

fn assert_literal_shell_path(path: &Path) -> &str {
    let text = path.to_str().expect("selected path is UTF-8");
    assert!(
        !text.contains(['\'', '\n', '\r', '\0']),
        "selected path cannot be quoted in the private MetaMake configuration"
    );
    text
}

fn assert_private_file(path: &Path) {
    let metadata = fs::metadata(path).expect("private observation output exists");
    assert!(
        metadata.is_file(),
        "private observation output is not a file"
    );
    assert_eq!(
        metadata.permissions().mode() & 0o7777,
        0o600,
        "private observation output mode changed"
    );
}

fn assert_stage_status(path: &Path) {
    let bytes = fs::read(path).expect("private observer stage status exists");
    let status: Value = serde_json::from_slice(&bytes).expect("parse private observer status");
    assert!(
        status.get("status").and_then(Value::as_i64) == Some(2),
        "native GNU Make observer stage did not stop with status 2"
    );
    assert!(
        status.get("timed_out").and_then(Value::as_bool) == Some(false),
        "native GNU Make observer stage timed out"
    );
    assert!(
        status.get("cancelled").and_then(Value::as_bool) == Some(false),
        "native GNU Make observer stage was cancelled"
    );
    assert!(
        status.get("stdout_exact").and_then(Value::as_bool) == Some(true)
            && status.get("stderr_exact").and_then(Value::as_bool) == Some(true),
        "native GNU Make observer stage output was incomplete"
    );
}

fn write_private_receipt(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .expect("create private observation receipt");
    file.write_all(bytes)
        .expect("write private observation receipt");
    file.sync_all().expect("sync private observation receipt");
}

fn source_owned_arguments(build: &Path, source: &Path) -> Vec<String> {
    vec![
        "-r".into(),
        "-n".into(),
        format!("TOP={}", build.display()),
        format!("SRCDIR={}", source.display()),
        "CURDIR=workbench/libs/mesa".into(),
        format!("TARGET={TARGET}"),
        "-s".into(),
        "-r".into(),
        "-n".into(),
        "--file=mmakefile".into(),
        TARGET.into(),
    ]
}

const fn phase_qualification(phase: MakeObservationPhase) -> &'static str {
    match phase {
        MakeObservationPhase::PreFirstFile => "instrumented-initial-observation-only",
        MakeObservationPhase::FirstInputPrelude => "instrumented-first-input-observation-only",
    }
}

fn observation_call_root(
    root: PathBuf,
    maximum: Option<usize>,
    control_sha256: &Sha256Digest,
    context_sha256: &Sha256Digest,
) -> PathBuf {
    if maximum.is_none() {
        return root;
    }
    let mut entries = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    entries.sort();
    assert_eq!(
        entries,
        ["call-0000", "call-0000.claim"].map(std::ffi::OsString::from)
    );
    assert_private_file(&root.join("call-0000.claim"));
    let call = root.join("call-0000");
    let completed = fs::read(call.join("call-completed.json")).unwrap();
    let completed: Value = serde_json::from_slice(&completed).unwrap();
    assert_eq!(completed["control_sha256"], control_sha256.as_str());
    assert_eq!(completed["context_sha256"], context_sha256.as_str());
    assert_eq!(completed["slot"], 0);
    assert_private_file(&call.join("call-completed.json"));
    call
}

fn launch_control(
    phase: MakeObservationPhase,
    root: &Path,
    make: (&Path, &Sha256Digest),
    context: &Sha256Digest,
    maximum: Option<usize>,
    argument_zero: Option<&str>,
) -> Vec<u8> {
    assert!(argument_zero.is_none() || maximum.is_some());
    let mut control = serde_json::json!({
        "version": if argument_zero.is_some() { 4 } else if maximum.is_some() { 3 } else { 2 },
        "phase": phase,
        "output_root": root.to_str().unwrap(),
        "program": make.0.to_str().unwrap(),
        "executable_sha256": make.1.as_str(),
        "context_sha256": context.as_str(),
    });
    if let Some(maximum) = maximum {
        control["maximum_calls"] = maximum.into();
    }
    if let Some(value) = argument_zero {
        control["argument_zero"] = value.into();
    }
    let bytes = serde_json::to_vec(&control).unwrap();
    assert!(bytes.len() <= CONTROL_LIMIT);
    bytes
}

fn assert_observation_files(root: &Path) {
    for name in [
        "observation-started.json",
        "invocation.json",
        "discovery.stdout",
        "discovery.stderr",
        "discovery.status.json",
        "initial.stdout",
        "initial.stderr",
        "initial.status.json",
        "observation.json",
        "observation-selection.json",
    ] {
        assert_private_file(&root.join(name));
    }
    assert_stage_status(&root.join("discovery.status.json"));
    assert_stage_status(&root.join("initial.status.json"));
}

fn load_observation(
    root: &Path,
    maximum: Option<usize>,
    control: &Sha256Digest,
    selected: (&[u8], &Sha256Digest, &MakeObservationBinding),
) -> RetainedMakeObservation {
    let (bytes, digest, binding) = selected;
    if maximum.is_none() {
        return RetainedMakeObservation::load(bytes, digest, binding).unwrap();
    }
    let collection = PrivateNativeCaptureDirectory::open(root.parent().unwrap()).unwrap();
    let mut observations = read_native_make_collection(
        &collection,
        control,
        &[NativeMakeCallSelection {
            document_sha256: digest.clone(),
            binding: binding.clone(),
        }],
        Instant::now() + Duration::from_secs(5),
        &CancellationToken::default(),
    )
    .expect("closed readback of the completed original MetaMake collection");
    assert_eq!(observations.len(), 1);
    observations.pop().unwrap()
}

fn checked_invocation(
    root: &Path,
    binding: &MakeObservationBinding,
    arguments: &[String],
    argument_zero: Option<&str>,
    retained: &RetainedMakeObservation,
) -> NativeMakeInvocation {
    let bytes = fs::read(root.join("invocation.json")).unwrap();
    assert_eq!(sha256_bytes(&bytes), binding.invocation_sha256);
    let invocation = NativeMakeInvocation::parse(&bytes).unwrap();
    assert_eq!(invocation.argument_zero(), argument_zero);
    assert_eq!(retained.invocation().argument_zero(), argument_zero);
    assert!(
        invocation.arguments() == arguments,
        "native original arguments differ"
    );
    assert!(
        retained.invocation().arguments() == arguments,
        "retained original arguments differ"
    );
    invocation
}

#[test]
#[ignore = "requires pinned original MetaMake, Python, GNU Make, and a fresh short output root"]
fn original_metamake_launch_reaches_native_observer_for_generated_mesa_marker() {
    let case_path = PathBuf::from(
        std::env::var_os("AROS_TEST_METAMAKE_OBSERVE_CASE")
            .expect("select a private original MetaMake observation case"),
    );
    let (_, case_bytes) = measure_regular_file_bounded(&case_path, 64 * 1024)
        .unwrap()
        .unwrap();
    let case: Case = serde_json::from_slice(&case_bytes).unwrap();
    let native_make = case
        .native_make
        .as_ref()
        .expect("case selects a pinned GNU Make executable");
    let phase = case
        .native_observation_phase
        .unwrap_or(MakeObservationPhase::PreFirstFile);
    let launch_flag = case
        .native_collection_maximum
        .map_or("--observe-make-launch", |maximum| {
            assert!((1..=256).contains(&maximum));
            "--observe-make-launch-collection"
        });

    for path in [
        &case.source_root,
        &case.metamake,
        &case.python,
        &native_make.program,
        &case_path,
    ] {
        assert!(path.is_absolute());
        assert_eq!(fs::canonicalize(path).unwrap().as_path(), path.as_path());
    }
    assert!(case.output_root.is_absolute());
    assert!(
        !case.output_root.exists(),
        "observation output root must be fresh"
    );
    assert!(
        !aros_common::publication::filesystem_paths_overlap(&case.output_root, &case.source_root)
            .unwrap(),
        "private observation output must not overlap selected Source"
    );
    let output_parent = case.output_root.parent().expect("output root parent");
    assert_eq!(fs::canonicalize(output_parent).unwrap(), output_parent);

    let case_sha256 = sha256_bytes(&case_bytes);
    let before_metamake = selected_executable(&case.metamake, &case.metamake_sha256);
    let before_python = selected_executable(&case.python, &case.python_sha256);
    let before_make = selected_executable(&native_make.program, &native_make.sha256);
    let observer = PathBuf::from(env!("CARGO_BIN_EXE_aros-transpiler"))
        .canonicalize()
        .expect("resolve the built observation frontend");
    let observer_sha256 = {
        let (_, bytes) = measure_regular_file_bounded(&observer, OBSERVER_LIMIT)
            .unwrap()
            .unwrap();
        sha256_bytes(&bytes)
    };

    fs::DirBuilder::new()
        .mode(0o700)
        .create(&case.output_root)
        .unwrap();
    assert_eq!(
        fs::canonicalize(&case.output_root).unwrap(),
        case.output_root
    );
    assert_eq!(
        fs::metadata(&case.output_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );

    assert_eq!(
        case.source_inputs
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        INPUTS.iter().copied().collect()
    );
    let bundle = case.output_root.join("i");
    let source = case.output_root.join("s");
    let build = case.output_root.join("b");
    let empty_path = case.output_root.join("p");
    for directory in [&bundle, &source, &build, &empty_path] {
        fs::DirBuilder::new().mode(0o700).create(directory).unwrap();
    }
    for (relative, expected) in &case.source_inputs {
        let bytes = pinned(&case.source_root.join(relative), expected);
        for destination_root in [&bundle, &source] {
            let destination = destination_root.join(relative);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::write(&destination, &bytes).unwrap();
            pinned(&destination, expected);
        }
    }
    for relative in INPUTS.iter().filter(|relative| {
        Path::new(relative)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmpl"))
    }) {
        let text = fs::read_to_string(bundle.join(relative)).unwrap();
        for included in text
            .lines()
            .filter_map(|line| line.strip_prefix("%include "))
        {
            let included = Path::new(relative).parent().unwrap().join(included.trim());
            assert!(case.source_inputs.contains_key(included.to_str().unwrap()));
        }
    }
    let bundle_before = measure_tree_content_cas(&bundle).unwrap();

    let expanded_path = case.output_root.join("g");
    let generation_environment = BTreeMap::from([
        ("LC_ALL".into(), "C".into()),
        ("PATH".into(), empty_path.to_str().unwrap().into()),
    ]);
    run(
        &case.python,
        &[
            "-I".into(),
            "-S".into(),
            bundle.join("tools/genmf/genmf.py").to_str().unwrap().into(),
            bundle.join("config/make.tmpl").to_str().unwrap().into(),
            bundle
                .join("workbench/libs/mesa/mmakefile.src")
                .to_str()
                .unwrap()
                .into(),
            expanded_path.to_str().unwrap().into(),
        ],
        &bundle,
        &generation_environment,
        &case.output_root.join("genmf"),
        0,
    );
    let expanded = fs::read_to_string(&expanded_path).unwrap();
    assert!(
        expanded.contains(TARGET),
        "GenMF output lacks the selected marker target"
    );
    let metadata_names = metadata_variables(&expanded);

    let source_owner = source.join("workbench/libs/mesa/mmakefile.src");
    let build_owner = build.join(OWNER);
    fs::create_dir_all(build_owner.parent().unwrap()).unwrap();
    fs::copy(&expanded_path, &build_owner).unwrap();
    let source_owner_time = fs::metadata(&source_owner).unwrap().modified().unwrap();
    let owner_time = source_owner_time
        .checked_add(Duration::from_secs(2))
        .expect("generated owner timestamp bound");
    fs::File::options()
        .write(true)
        .open(&build_owner)
        .unwrap()
        .set_times(FileTimes::new().set_modified(owner_time))
        .unwrap();
    assert!(
        fs::metadata(&build_owner).unwrap().modified().unwrap() > source_owner_time,
        "generated build owner must be newer than its source"
    );
    assert!(!source.join(OWNER).exists());
    let (_, owner_bytes) = measure_regular_file_bounded(&build_owner, 4 * 1024 * 1024)
        .unwrap()
        .unwrap();
    let owner_sha256 = sha256_bytes(&owner_bytes);
    let source_before = measure_tree_content_cas(&source).unwrap();

    let source_config = fs::read_to_string(bundle.join("mmake.config.in")).unwrap();
    let templates = source_config
        .lines()
        .filter_map(|line| line.strip_prefix("maketool "))
        .collect::<Vec<_>>();
    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0].matches("$(HOST_MAKE)").count(), 1);

    let observer_alias = case.output_root.join("o");
    let control_path = case.output_root.join("c");
    let observation_root = case.output_root.join("d");
    symlink(&observer, &observer_alias).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&observation_root)
        .unwrap();
    assert_eq!(fs::canonicalize(&observer_alias).unwrap(), observer);
    assert_eq!(
        fs::metadata(&observation_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );

    let alias_text = assert_literal_shell_path(&observer_alias);
    let control_text = assert_literal_shell_path(&control_path);
    let control_sha_placeholder = CONTROL_SHA_PLACEHOLDER;
    let placeholder_launcher =
        format!("'{alias_text}' {launch_flag} '{control_text}' '{control_sha_placeholder}' --");
    let configuration_choice = format!(
        "[capture]\nmaketool {}\ndefaultmakefilename mmakefile\n",
        templates[0].replace("$(HOST_MAKE)", &placeholder_launcher)
    );
    let configuration_choice_sha256 = sha256_bytes(configuration_choice.as_bytes());
    let metadata_names_sha256 = sha256_bytes(&serde_json::to_vec(&metadata_names).unwrap());
    let context_record = serde_json::json!({
        "version": 1,
        "observation_phase": phase,
        "collection_maximum": case.native_collection_maximum,
        "selected_argument_zero": case.native_argument_zero,
        "case_sha256": case_sha256.as_str(),
        "source_root": case.source_root.to_str().unwrap(),
        "output_root": case.output_root.to_str().unwrap(),
        "source_inputs": &case.source_inputs,
        "metamake_sha256": case.metamake_sha256.as_str(),
        "python_sha256": case.python_sha256.as_str(),
        "gnu_make_path": native_make.program.to_str().unwrap(),
        "gnu_make_sha256": native_make.sha256.as_str(),
        "observer_sha256": observer_sha256.as_str(),
        "source_owner_path": source_owner.to_str().unwrap(),
        "build_owner_path": build_owner.to_str().unwrap(),
        "build_owner_sha256": owner_sha256.as_str(),
        "expanded_metadata_names_sha256": metadata_names_sha256.as_str(),
        "configuration_choice_sha256": configuration_choice_sha256.as_str(),
        "target": TARGET,
    });
    let context_bytes = serde_json::to_vec(&context_record).unwrap();
    let context_sha256 = sha256_bytes(&context_bytes);

    let control_bytes = launch_control(
        phase,
        &observation_root,
        (&native_make.program, &native_make.sha256),
        &context_sha256,
        case.native_collection_maximum,
        case.native_argument_zero.as_deref(),
    );
    let control_sha256 = sha256_bytes(&control_bytes);
    let root_directory = PrivateNativeCaptureDirectory::open(&case.output_root).unwrap();
    root_directory
        .write_new(std::ffi::OsStr::new("context.json"), &context_bytes)
        .unwrap();
    root_directory
        .write_new(std::ffi::OsStr::new("c"), &control_bytes)
        .unwrap();
    assert_private_file(&control_path);

    assert_eq!(
        configuration_choice
            .matches(control_sha_placeholder)
            .count(),
        1
    );
    let configuration =
        configuration_choice.replace(control_sha_placeholder, control_sha256.as_str());
    let native_config_line = configuration
        .lines()
        .find(|line| line.starts_with("maketool "))
        .expect("private selected maketool line");
    assert!(
        native_config_line.len() < 254,
        "native MetaMake configuration line exceeds its fixed bound"
    );
    assert!(!configuration.contains("globalvarfile"));
    assert!(!configuration.contains("preregen"));
    let config_path = case.output_root.join("m");
    fs::write(&config_path, &configuration).unwrap();
    let configuration_sha256 = sha256_bytes(configuration.as_bytes());

    let make_environment = BTreeMap::from([
        ("LC_ALL".into(), "C".into()),
        ("PATH".into(), empty_path.to_str().unwrap().into()),
        ("MKARGS".into(), "-r -n".into()),
        ("MMAKE_CONFIG".into(), config_path.to_str().unwrap().into()),
        ("CAPTURE_LITERAL".into(), "literal-$-not-expanded".into()),
    ]);
    let bindings = BTreeMap::from([
        ("TOP".into(), build.to_str().unwrap().into()),
        ("SRCDIR".into(), source.to_str().unwrap().into()),
        ("CURDIR".into(), "workbench/libs/mesa".into()),
        ("TARGET".into(), TARGET.into()),
        ("MKARGS".into(), "-r -n".into()),
    ]);
    let template = templates[0].replace(
        "$(HOST_MAKE)",
        &format!(
            "'{}' {launch_flag} '{}' '{}' --",
            alias_text,
            control_text,
            control_sha256.as_str()
        ),
    );
    assert_eq!(
        template.replace(
            &format!(
                "'{}' {launch_flag} '{}' '{}' --",
                alias_text,
                control_text,
                control_sha256.as_str()
            ),
            "$(HOST_MAKE)"
        ),
        templates[0]
    );
    let suffix = format!("-s -r -n --file=mmakefile {TARGET}");
    let command = literal_template(&template, &bindings, &suffix)
        .expect("source-owned maketool remains one literal bounded command");
    assert_eq!(command.program(), alias_text);
    let wrapper_prefix = [
        launch_flag.to_owned(),
        control_text.to_owned(),
        control_sha256.to_string(),
        "--".to_owned(),
    ];
    assert!(command.arguments().starts_with(&wrapper_prefix));
    let expected_arguments = source_owned_arguments(&build, &source);
    assert!(
        command.arguments()[wrapper_prefix.len()..] == expected_arguments,
        "source-owned original arguments differ"
    );
    assert!(command.environment_assignments().is_empty());

    assert_eq!(measure_tree_content_cas(&bundle).unwrap(), bundle_before);
    run(
        &case.metamake,
        &[
            format!("--srcdir={}", source.display()),
            format!("--builddir={}", build.display()),
            "--quiet".into(),
            "-r".into(),
            "-n".into(),
            format!("capture.{TARGET}"),
        ],
        &case.output_root,
        &make_environment,
        &case.output_root.join("native"),
        0,
    );

    assert_eq!(measure_tree_content_cas(&source).unwrap(), source_before);
    assert_eq!(measure_tree_content_cas(&bundle).unwrap(), bundle_before);
    pinned(&build_owner, &owner_sha256);
    assert!(build.join("mmake.cache").is_file());
    assert!(!source.join("mmake.cache").exists());
    assert!(!case.output_root.join("mmake.cache").exists());
    assert_eq!(fs::read_dir(&empty_path).unwrap().count(), 0);
    root_directory.revalidate().unwrap();
    assert_eq!(
        sha256_bytes(&fs::read(&control_path).unwrap()),
        control_sha256
    );
    assert_eq!(fs::canonicalize(&observer_alias).unwrap(), observer);
    assert_eq!(
        sha256_bytes(&fs::read(&config_path).unwrap()),
        configuration_sha256
    );
    assert_eq!(
        sha256_bytes(
            &root_directory
                .read_selected(
                    std::ffi::OsStr::new("context.json"),
                    &context_sha256,
                    64 * 1024
                )
                .unwrap()
        ),
        context_sha256
    );

    assert_eq!(
        fs::metadata(&observation_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );
    let observation_root = observation_call_root(
        observation_root,
        case.native_collection_maximum,
        &control_sha256,
        &context_sha256,
    );
    assert_observation_files(&observation_root);

    let selection_bytes = fs::read(observation_root.join("observation-selection.json")).unwrap();
    let selection: Value = serde_json::from_slice(&selection_bytes).unwrap();
    assert_eq!(selection.get("version").and_then(Value::as_u64), Some(1));
    assert_eq!(
        selection.get("qualification").and_then(Value::as_str),
        Some(phase_qualification(phase))
    );
    let observation_bytes = fs::read(observation_root.join("observation.json")).unwrap();
    let observation_sha256 = sha256_bytes(&observation_bytes);
    assert_eq!(
        selection.get("document_sha256").and_then(Value::as_str),
        Some(observation_sha256.as_str())
    );
    let selected_binding: MakeObservationBinding =
        serde_json::from_value(selection["binding"].clone()).unwrap();
    assert_eq!(selected_binding.phase, phase);
    assert_eq!(selected_binding.context_sha256, context_sha256);
    let expected_binding = MakeObservationBinding {
        phase,
        context_sha256: context_sha256.clone(),
        invocation_sha256: sha256_bytes(
            &fs::read(observation_root.join("invocation.json")).unwrap(),
        ),
        stdout_sha256: sha256_bytes(&fs::read(observation_root.join("initial.stdout")).unwrap()),
    };
    assert!(
        selected_binding == expected_binding,
        "retained binding differs"
    );
    let retained = load_observation(
        &observation_root,
        case.native_collection_maximum,
        &control_sha256,
        (&observation_bytes, &observation_sha256, &expected_binding),
    );

    let invocation = checked_invocation(
        &observation_root,
        &expected_binding,
        &expected_arguments,
        case.native_argument_zero.as_deref(),
        &retained,
    );
    assert_eq!(
        invocation.working_directory(),
        build_owner.parent().unwrap()
    );
    assert_eq!(
        retained.invocation().working_directory(),
        build_owner.parent().unwrap()
    );
    assert!(
        make_environment
            .iter()
            .all(|(name, value)| invocation.environment().get(name) == Some(value)),
        "an explicitly selected inherited environment value changed"
    );
    assert!(invocation
        .environment()
        .get("_MMAKE_TARGET")
        .is_some_and(|value| value == TARGET));
    assert!(
        retained.environment_names().contains("_MMAKE_TARGET")
            && make_environment
                .keys()
                .all(|name| retained.environment_names().contains(name)),
        "native observer omitted selected environment membership"
    );

    let (_, after_case_bytes) = measure_regular_file_bounded(&case_path, 64 * 1024)
        .unwrap()
        .unwrap();
    assert_eq!(sha256_bytes(&after_case_bytes), case_sha256);
    for (relative, expected) in &case.source_inputs {
        pinned(&case.source_root.join(relative), expected);
    }
    assert_eq!(
        selected_executable(&case.metamake, &case.metamake_sha256),
        before_metamake
    );
    assert_eq!(
        selected_executable(&case.python, &case.python_sha256),
        before_python
    );
    assert_eq!(
        selected_executable(&native_make.program, &native_make.sha256),
        before_make
    );
    assert_eq!(
        selected_executable(&observer, &observer_sha256),
        observer_sha256
    );

    let receipt = serde_json::to_vec_pretty(&serde_json::json!({
        "qualification": "private-actual-metamake-to-native-initial-observation-only",
        "observation_phase": phase,
        "collection_maximum": case.native_collection_maximum,
        "result": "PASS",
        "target": TARGET,
        "case_sha256": case_sha256.as_str(),
        "metamake_exit_status": 0,
        "frontend_exit_status": 0,
        "gnu_make_observer_statuses": {"discovery": 2, "initial": 2},
        "gnu_make_executable_sha256": native_make.sha256.as_str(),
        "observer_executable_sha256": observer_sha256.as_str(),
        "context_sha256": context_sha256.as_str(),
        "observation_document_sha256": observation_sha256.as_str(),
        "invocation_sha256": expected_binding.invocation_sha256.as_str(),
        "initial_stdout_sha256": expected_binding.stdout_sha256.as_str(),
        "owner_sha256": owner_sha256.as_str(),
        "configuration_sha256": configuration_sha256.as_str(),
        "omissions": [
            "production source and build roots and complete production root context",
            "complete production project, global, and export context",
            "production preparation and build execution",
            "ordinary GNU Make execution and complete Make input closure",
            "Sourcefile parsing and generated recipes",
            "genmodule, compiler, ABI, Mesa, and SDK qualification"
        ]
    }))
    .unwrap();
    write_private_receipt(&case.output_root.join("result.json"), &receipt);
}
