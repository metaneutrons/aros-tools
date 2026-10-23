//! Actual MetaMake launch-shape proof, not a production owner/SDK receipt.

use aros_common::make_child_command::LiteralMakeChildCommand;
use aros_common::publication::filesystem_paths_overlap;
use aros_common::{
    measure_regular_file_bounded, measure_tree_content_cas, run_output_with_control, sha256_bytes,
    CancellationToken, Sha256Digest,
};
use aros_transpiler::metamake_owner_graph::{Limits, MetaMakeOwnerGraph};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const OWNER: &str = "workbench/libs/mesa/mmakefile";
const TARGET: &str = "mesa3dgl-library-genmakefile";
const INPUTS: &[&str] = &[
    "tools/genmf/genmf.py",
    "config/make.tmpl",
    "config/make-autotools.tmpl",
    "config/make-cmake.tmpl",
    "config/make-meson.tmpl",
    "mmake.config.in",
    "workbench/libs/mesa/mmakefile.src",
];

#[path = "metamake_launch_capture/native_observe.rs"]
mod native_observe;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MakeProgramSelection {
    program: PathBuf,
    sha256: Sha256Digest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    source_root: PathBuf,
    output_root: PathBuf,
    metamake: PathBuf,
    metamake_sha256: Sha256Digest,
    python: PathBuf,
    python_sha256: Sha256Digest,
    source_inputs: BTreeMap<String, Sha256Digest>,
    #[serde(default)]
    native_make: Option<MakeProgramSelection>,
    #[serde(default)]
    native_observation_phase: Option<aros_common::make_native_observation::MakeObservationPhase>,
    #[serde(default)]
    native_collection_maximum: Option<usize>,
    #[serde(default)]
    native_argument_zero: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    argv: Vec<String>,
    cwd: PathBuf,
    environment: BTreeMap<String, String>,
}

fn pinned(path: &Path, expected: &Sha256Digest) -> Vec<u8> {
    let (_, bytes) = measure_regular_file_bounded(path, 4 * 1024 * 1024)
        .unwrap()
        .unwrap();
    assert_eq!(
        &sha256_bytes(&bytes),
        expected,
        "selected native fixture input changed"
    );
    bytes
}

fn run(
    program: &Path,
    argv: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    log: &Path,
    expected_status: i32,
) {
    let mut command = Command::new(program);
    command
        .args(argv)
        .current_dir(cwd)
        .env_clear()
        .envs(environment);
    let result = run_output_with_control(
        &mut command,
        1024 * 1024,
        Duration::from_secs(30),
        &CancellationToken::default(),
    )
    .unwrap();
    fs::write(
        log.with_extension("stdout"),
        result.stdout.exact_bytes().unwrap(),
    )
    .unwrap();
    fs::write(
        log.with_extension("stderr"),
        result.stderr.exact_bytes().unwrap(),
    )
    .unwrap();
    fs::write(
        log.with_extension("status.json"),
        serde_json::to_vec(&serde_json::json!({
            "status": result.status.code(), "timed_out": result.timed_out,
            "cancelled": result.cancelled, "expected_status": expected_status,
            "success": result.status.success()
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        !result.timed_out && !result.cancelled && result.status.code() == Some(expected_status),
        "private native capture stage failed"
    );
}

// This is a test-launch safety check, not the productive MetaMake evaluator.
// Refuse anything outside one literal, completely bound command before popen.
fn literal_template(
    template: &str,
    bindings: &BTreeMap<String, String>,
    suffix: &str,
) -> Option<LiteralMakeChildCommand> {
    let mut expanded = String::new();
    let mut remaining = template;
    while let Some(offset) = remaining.find('$') {
        expanded.push_str(&remaining[..offset]);
        let reference = remaining[offset..].strip_prefix("$(")?;
        let end = reference.find(')')?;
        let value = bindings.get(&reference[..end])?;
        // Native variable expansion happens before the shell. Do not allow a
        // fixture binding to introduce syntax or even a second expansion.
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/._-@ =".contains(&byte))
        {
            return None;
        }
        expanded.push_str(value);
        remaining = &reference[end + 1..];
    }
    expanded.push_str(remaining);
    expanded.push(' ');
    expanded.push_str(suffix);
    LiteralMakeChildCommand::parse(&expanded).ok()
}

fn check_capture_template(
    template: &str,
    source: &Path,
    build: &Path,
    python: &Path,
    helper: &Path,
    forwarded: bool,
) {
    let bindings = BTreeMap::from([
        ("TOP".into(), build.to_str().unwrap().into()),
        ("SRCDIR".into(), source.to_str().unwrap().into()),
        ("CURDIR".into(), "workbench/libs/mesa".into()),
        ("TARGET".into(), TARGET.into()),
        ("MKARGS".into(), "-r -n".into()),
    ]);
    let suffix = format!(
        "-s {}--file=mmakefile {TARGET}",
        if forwarded { "-r -n " } else { "" }
    );
    let command = literal_template(template, &bindings, &suffix).unwrap();
    assert_eq!(command.program(), python.to_str().unwrap());
    assert_eq!(
        &command.arguments()[..3],
        ["-I", "-S", helper.to_str().unwrap()]
    );
    assert!(command.environment_assignments().is_empty());
}

#[test]
fn capture_template_refuses_unbound_and_effectful_commands() {
    let bindings = BTreeMap::from([("FLAGS".into(), "-r -n".into())]);
    assert!(literal_template(
        "'/private/python' '/private/helper' $(FLAGS)",
        &bindings,
        "target"
    )
    .is_some());
    for template in [
        "/private/python $(UNKNOWN)",
        "/private/python $FLAGS",
        "/private/python $(FLAGS); /usr/bin/make",
        "/private/python `make`",
        "/private/python $(FLAGS) && make",
    ] {
        assert!(literal_template(template, &bindings, "target").is_none());
    }
    let poison = BTreeMap::from([("FLAGS".into(), "-n; /usr/bin/make".into())]);
    assert!(literal_template("/private/python $(FLAGS)", &poison, "target").is_none());
}

fn metadata_variables(text: &str) -> BTreeSet<String> {
    let reference = regex::Regex::new(r"\$\(([^)]+)\)").unwrap();
    let mut bare_next = false;
    let mut names = BTreeSet::new();
    for line in text.lines() {
        let relevant = line.starts_with("#MM") || bare_next;
        bare_next = matches!(line.trim(), "#MM" | "#MM-");
        if relevant {
            for found in reference.captures_iter(line) {
                names.insert(found[1].into());
            }
        }
    }
    names
}

fn selected_fields_match(
    capture: &Capture,
    args: &[String],
    cwd: &Path,
    inherited: &BTreeMap<String, String>,
    environment_target: &str,
) -> bool {
    capture.argv == args
        && capture.cwd == cwd
        && inherited
            .iter()
            .all(|(key, value)| capture.environment.get(key) == Some(value))
        && capture
            .environment
            .get("_MMAKE_TARGET")
            .is_some_and(|value| value == environment_target)
}

fn duplicate_owner_counter(
    case: &Case,
    expanded_path: &Path,
    configuration: &str,
    clean: &BTreeMap<String, String>,
) -> serde_json::Value {
    let root = case.output_root.join("duplicate-counter");
    fs::create_dir(&root).unwrap();
    let source = root.join("source");
    let build = root.join("build");
    let captures = root.join("captures");
    for path in [&source, &build, &captures] {
        fs::create_dir(path).unwrap();
    }
    for relative in [OWNER, "duplicate/mmakefile"] {
        let path = source.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::copy(expanded_path, path).unwrap();
    }
    let config = root.join("mmake.config");
    fs::write(&config, configuration).unwrap();
    let mut environment = clean.clone();
    environment.extend(BTreeMap::from([
        ("MMAKE_CONFIG".into(), config.to_str().unwrap().into()),
        (
            "AROS_TEST_CAPTURE_DIRECTORY".into(),
            captures.to_str().unwrap().into(),
        ),
        ("MKARGS".into(), "-r -n".into()),
    ]));
    let source_before = measure_tree_content_cas(&source).unwrap();
    let template = configuration
        .lines()
        .find_map(|line| line.strip_prefix("maketool "))
        .unwrap();
    check_capture_template(
        template,
        &source,
        &build,
        &case.python,
        &case.output_root.join("capture.py"),
        false,
    );
    run(
        &case.metamake,
        &[
            format!("--srcdir={}", source.display()),
            format!("--builddir={}", build.display()),
            "--quiet".into(),
            format!("capture.{TARGET}"),
        ],
        &root,
        &environment,
        &root.join("native"),
        0,
    );
    assert_eq!(measure_tree_content_cas(&source).unwrap(), source_before);
    let records = fs::read_dir(captures)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(
        records.len(),
        2,
        "native MetaMake must expose both real owners, not a first match"
    );
    let directories = records
        .iter()
        .map(|path| {
            let record: Capture = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            assert_eq!(record.argv.last().unwrap(), TARGET);
            record.cwd
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        directories,
        BTreeSet::from([source.join("workbench/libs/mesa"), source.join("duplicate")])
    );
    serde_json::json!({"expected_single_owner_admission": false, "native_owner_calls": records.len(), "captures": records})
}

fn missing_launcher_counter(case: &Case, expanded: &Path, clean: &BTreeMap<String, String>) {
    let root = case.output_root.join("missing-launcher-counter");
    fs::create_dir(&root).unwrap();
    let source = root.join("source");
    let build = root.join("build");
    let captures = root.join("captures");
    for path in [&source, &build, &captures] {
        fs::create_dir(path).unwrap();
    }
    let owner = source.join(OWNER);
    fs::create_dir_all(owner.parent().unwrap()).unwrap();
    fs::copy(expanded, owner).unwrap();
    let config = root.join("mmake.config");
    // Deliberately select native default `make`. It must be unreachable even
    // when capture configuration is incomplete; -n is not an isolation fence.
    fs::write(&config, "[capture]\ndefaultmakefilename mmakefile\n").unwrap();
    let mut environment = clean.clone();
    environment.insert("MMAKE_CONFIG".into(), config.to_str().unwrap().into());
    environment.insert(
        "AROS_TEST_CAPTURE_DIRECTORY".into(),
        captures.to_str().unwrap().into(),
    );
    let source_before = measure_tree_content_cas(&source).unwrap();
    run(
        &case.metamake,
        &[
            format!("--srcdir={}", source.display()),
            format!("--builddir={}", build.display()),
            "--quiet".into(),
            format!("capture.{TARGET}"),
        ],
        &root,
        &environment,
        &root.join("native"),
        10,
    );
    let stderr = fs::read_to_string(root.join("native.stderr")).unwrap();
    assert!(stderr.contains("make: command not found") || stderr.contains("make: not found"));
    assert_eq!(fs::read_dir(&captures).unwrap().count(), 0);
    assert_eq!(measure_tree_content_cas(&source).unwrap(), source_before);
}

#[test]
#[ignore = "requires pinned original host MetaMake/Python, selected real Source and fresh private output"]
fn original_metamake_captures_real_mesa_genmakefile_launch_without_make() {
    let case_path = PathBuf::from(std::env::var_os("AROS_TEST_METAMAKE_CAPTURE_CASE").unwrap());
    let (_, case_bytes) = measure_regular_file_bounded(&case_path, 64 * 1024)
        .unwrap()
        .unwrap();
    let case: Case = serde_json::from_slice(&case_bytes).unwrap();
    for root in [&case.source_root, &case.metamake, &case.python] {
        assert!(root.is_absolute());
        assert_eq!(&fs::canonicalize(root).unwrap(), root);
    }
    assert!(case.output_root.is_absolute());
    assert!(!filesystem_paths_overlap(&case.output_root, &case.source_root).unwrap());
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&case.output_root)
        .unwrap();
    assert_eq!(
        case.source_inputs
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        INPUTS.iter().copied().collect()
    );
    let before_metamake = pinned(&case.metamake, &case.metamake_sha256);
    let before_python = pinned(&case.python, &case.python_sha256);
    let bundle = case.output_root.join("inputs");
    fs::create_dir(&bundle).unwrap();
    for (relative, expected) in &case.source_inputs {
        let bytes = pinned(&case.source_root.join(relative), expected);
        let path = bundle.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        pinned(&path, expected);
    }
    // The original generator can read only this independently pinned template
    // closure. New includes must extend the selection before execution.
    for relative in INPUTS.iter().filter(|relative| {
        Path::new(relative)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmpl"))
    }) {
        let text = aros_common::text::read_source(&bundle.join(relative)).unwrap();
        for line in text
            .lines()
            .filter_map(|line| line.strip_prefix("%include "))
        {
            let included = Path::new(relative).parent().unwrap().join(line.trim());
            assert!(case.source_inputs.contains_key(included.to_str().unwrap()));
        }
    }
    let inputs_before = measure_tree_content_cas(&bundle).unwrap();
    let expanded_path = case.output_root.join("expanded-mmakefile");
    let empty_path = case.output_root.join("empty-path");
    fs::create_dir(&empty_path).unwrap();
    let clean = BTreeMap::from([
        ("PATH".into(), empty_path.to_str().unwrap().into()),
        ("LC_ALL".into(), "C".into()),
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
        &clean,
        &case.output_root.join("genmf"),
        0,
    );
    let expanded = aros_common::text::read_source(&expanded_path).unwrap();
    missing_launcher_counter(&case, &expanded_path, &clean);
    let source_config = fs::read_to_string(bundle.join("mmake.config.in")).unwrap();
    let templates = source_config
        .lines()
        .filter_map(|line| line.strip_prefix("maketool "))
        .collect::<Vec<_>>();
    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0].matches("$(HOST_MAKE)").count(), 1);
    let helper = case.output_root.join("capture.py");
    fs::write(&helper, include_str!("support/metamake_capture.py")).unwrap();
    let helper_before = sha256_bytes(include_bytes!("support/metamake_capture.py"));
    // These substitutions alter only the private launcher executable. All
    // remaining source-owned template arguments stay intact.
    let replacement = format!("'{}' -I -S '{}'", case.python.display(), helper.display());
    assert!(!replacement.contains(['\n', '\r', '$']));
    assert!(!case.python.to_str().unwrap().contains('\''));
    assert!(!helper.to_str().unwrap().contains('\''));
    let template = templates[0].replace("$(HOST_MAKE)", &replacement);
    let config_text = format!("[capture]\nmaketool {template}\ndefaultmakefilename mmakefile\n");
    assert!(config_text.lines().all(|line| line.len() < 254));
    let mut receipts = Vec::new();
    for generated in [false, true] {
        for inherited_target in [false, true] {
            let root = case.output_root.join(format!(
                "layout-{}-{}",
                u8::from(generated),
                u8::from(inherited_target)
            ));
            fs::create_dir(&root).unwrap();
            let source = root.join("source");
            let build = root.join("build");
            let captures = root.join("captures");
            for directory in [&source, &build, &captures] {
                fs::create_dir(directory).unwrap();
            }
            let owner_root = if generated { &build } else { &source };
            let owner = owner_root.join(OWNER);
            fs::create_dir_all(owner.parent().unwrap()).unwrap();
            if generated {
                let original = source.join("workbench/libs/mesa/mmakefile.src");
                fs::create_dir_all(original.parent().unwrap()).unwrap();
                fs::copy(bundle.join("workbench/libs/mesa/mmakefile.src"), &original).unwrap();
            }
            fs::copy(&expanded_path, &owner).unwrap();
            if generated {
                assert!(
                    fs::metadata(&owner).unwrap().modified().unwrap()
                        >= fs::metadata(source.join("workbench/libs/mesa/mmakefile.src"))
                            .unwrap()
                            .modified()
                            .unwrap()
                );
            }
            let config = root.join("mmake.config");
            fs::write(&config, &config_text).unwrap();
            let mut environment = clean.clone();
            environment.extend(BTreeMap::from([
                ("MMAKE_CONFIG".into(), config.to_str().unwrap().into()),
                (
                    "AROS_TEST_CAPTURE_DIRECTORY".into(),
                    captures.to_str().unwrap().into(),
                ),
                ("MKARGS".into(), "-r -n".into()),
                ("MAKEFLAGS".into(), "--warn-undefined-variables".into()),
                ("MAKEFILES".into(), "must-not-be-loaded.mk".into()),
                ("CAPTURE_LITERAL".into(), "literal-$-not-expanded".into()),
            ]));
            if inherited_target {
                environment.insert("_MMAKE_TARGETS".into(), "prior".into());
                environment.insert("_MMAKE_TARGET".into(), "retained".into());
            }
            let globals = BTreeMap::from([
                ("TOP".into(), build.to_str().unwrap().into()),
                ("SRCDIR".into(), source.to_str().unwrap().into()),
                ("CURDIR".into(), String::new()),
            ]);
            let absent = metadata_variables(&expanded)
                .into_iter()
                .filter(|name| !globals.contains_key(name) && !environment.contains_key(name))
                .collect::<BTreeSet<_>>();
            let mut bound_globals = environment.clone();
            bound_globals.extend(globals);
            let files = BTreeMap::from([(OWNER.into(), expanded.clone())]);
            let graph = MetaMakeOwnerGraph::parse_with_declared_absence(
                &files,
                &bound_globals,
                &absent,
                Limits::default(),
            )
            .unwrap();
            let selection = graph.select(&[TARGET.into()], Limits::default()).unwrap();
            assert!(selection.missing_endpoints.is_empty());
            assert_eq!(selection.reached_targets, BTreeSet::from([TARGET.into()]));
            assert_eq!(
                selection.selected_owner_files,
                BTreeSet::from([OWNER.into()])
            );
            assert!(graph.dependencies(TARGET).unwrap().is_empty());
            // A duplicate-owner graph cannot establish single-owner admission.
            // The isolated capture-only native counter below must see both.
            let duplicates = BTreeMap::from([
                (OWNER.into(), expanded.clone()),
                ("duplicate/mmakefile".into(), expanded.clone()),
            ]);
            let duplicate_graph = MetaMakeOwnerGraph::parse_with_declared_absence(
                &duplicates,
                &bound_globals,
                &absent,
                Limits::default(),
            )
            .unwrap();
            assert_eq!(
                duplicate_graph
                    .select(&[TARGET.into()], Limits::default())
                    .unwrap()
                    .selected_owner_files
                    .len(),
                2
            );
            let source_before = measure_tree_content_cas(&source).unwrap();
            let (_, owner_bytes) = measure_regular_file_bounded(&owner, 4 * 1024 * 1024)
                .unwrap()
                .unwrap();
            let owner_before = sha256_bytes(&owner_bytes);
            check_capture_template(&template, &source, &build, &case.python, &helper, true);
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
                &root,
                &environment,
                &root.join("native"),
                0,
            );
            assert_eq!(measure_tree_content_cas(&source).unwrap(), source_before);
            pinned(&owner, &owner_before);
            let records = fs::read_dir(&captures)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            assert_eq!(
                records.len(),
                1,
                "selected real target must launch exactly one owner"
            );
            let bytes = fs::read(&records[0]).unwrap();
            let record: Capture = serde_json::from_slice(&bytes).unwrap();
            let cwd = owner.parent().unwrap();
            let expected_args = vec![
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
            ];
            let target_env = if inherited_target { "retained" } else { TARGET };
            assert!(selected_fields_match(
                &record,
                &expected_args,
                cwd,
                &environment,
                target_env
            ));
            let mut wrong = record.clone();
            wrong.cwd = root.clone();
            assert!(!selected_fields_match(
                &wrong,
                &expected_args,
                cwd,
                &environment,
                target_env
            ));
            let mut wrong = record.clone();
            wrong.environment.remove("MAKEFLAGS");
            assert!(!selected_fields_match(
                &wrong,
                &expected_args,
                cwd,
                &environment,
                target_env
            ));
            let mut wrong = record.clone();
            wrong.argv.pop();
            assert!(!selected_fields_match(
                &wrong,
                &expected_args,
                cwd,
                &environment,
                target_env
            ));
            receipts.push(serde_json::json!({"generated": generated, "inherited_target": inherited_target, "capture_sha256": sha256_bytes(&bytes), "capture_file": records[0], "owner_sha256": owner_before}));
        }
    }
    let duplicate_counter = duplicate_owner_counter(&case, &expanded_path, &config_text, &clean);
    assert_eq!(fs::read_dir(&empty_path).unwrap().count(), 0);
    assert_eq!(measure_tree_content_cas(&bundle).unwrap(), inputs_before);
    pinned(&helper, &helper_before);
    assert_eq!(
        pinned(&case.metamake, &case.metamake_sha256),
        before_metamake
    );
    assert_eq!(pinned(&case.python, &case.python_sha256), before_python);
    for (relative, expected) in &case.source_inputs {
        pinned(&case.source_root.join(relative), expected);
    }
    fs::write(case.output_root.join("result.json"), serde_json::to_vec_pretty(&serde_json::json!({"qualification": "private-real-metamake-launch-shape-only", "case_sha256": sha256_bytes(&case_bytes), "metamake_sha256": case.metamake_sha256, "python_sha256": case.python_sha256, "helper_sha256": helper_before, "target": TARGET, "captures": receipts, "duplicate_owner_counter": duplicate_counter, "omissions": ["production source/build absolute locations", "complete production project/global/export context", "GNU Make execution and input closure", "genmodule execution", "compiler/ABI/Mesa/SDK qualification"]})).unwrap()).unwrap();
}
