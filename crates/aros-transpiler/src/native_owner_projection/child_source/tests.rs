use super::{CapturedChildSourceRequest, MatchedChildSource};
use crate::{
    dirs::DirVars, make_vars::MakeProgramPaths, native_owner_projection::NativeOwnerProjection,
};
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace},
    make_native_invocation::NativeMakeInvocation,
    make_observer::{command_line_values_fragment, environment_values_fragment, pre_file_fragment},
    make_startup_arguments::LiteralMakeStartupArguments,
    measure_tree_content_cas, run_output_with_control, CancellationToken, DEFAULT_CAPTURE_LIMIT,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

mod batch;
mod declared;
mod folding;
mod mesa_binding;
mod output_links;
mod retained;
mod shell_effects;

const SOURCE_INPUT: &str = concat!(
    "LATER = before\n",
    "SIMPLE := $(LATER)\n",
    "RECURSIVE = $(LATER)\n",
    "LATER = after\n",
    "include extra.mk\n",
    "CLI_VALUE := source-value\n",
);

fn require<T, E>(result: Result<T, E>, diagnostic: &'static str) -> T {
    result.unwrap_or_else(|_| panic!("{diagnostic}"))
}

struct Fixture {
    _source_directory: tempfile::TempDir,
    _build_directory: tempfile::TempDir,
    source_root: PathBuf,
    build_root: PathBuf,
    working_directory: PathBuf,
    projection: NativeOwnerProjection,
}

impl Fixture {
    fn new() -> Self {
        let source_directory = require(
            tempfile::tempdir(),
            "could not create synthetic source root",
        );
        let build_directory = require(tempfile::tempdir(), "could not create synthetic build root");
        let source_root = require(
            source_directory.path().canonicalize(),
            "could not resolve synthetic source root",
        );
        let build_root = require(
            build_directory.path().canonicalize(),
            "could not resolve synthetic build root",
        );
        for owner in ["one", "two"] {
            require(
                std::fs::create_dir_all(source_root.join(owner)),
                "could not create synthetic source owner",
            );
            require(
                std::fs::write(source_root.join(owner).join("mmakefile.src"), "#MM root\n"),
                "could not write synthetic source owner",
            );
        }
        let working_directory = build_root.join("one");
        require(
            std::fs::create_dir_all(&working_directory),
            "could not create synthetic build owner",
        );
        require(
            std::fs::write(working_directory.join("mmakefile"), SOURCE_INPUT),
            "could not write synthetic generated Makefile",
        );
        require(
            std::fs::write(
                working_directory.join("extra.mk"),
                "INCLUDED := include-value\n",
            ),
            "could not write synthetic include",
        );
        let working_directory = require(
            working_directory.canonicalize(),
            "could not resolve synthetic child cwd",
        );
        let projection =
            super::super::tests::projection_for_discovery(&source_root, BTreeSet::new());
        Self {
            _source_directory: source_directory,
            _build_directory: build_directory,
            source_root,
            build_root,
            working_directory,
            projection,
        }
    }

    fn paths<'a>(
        &'a self,
        dirs: &'a DirVars,
        relative_directory: &'a Path,
        working_directory: &'a Path,
    ) -> MakeProgramPaths<'a> {
        MakeProgramPaths {
            dirs,
            source_root: &self.source_root,
            relative_dir: relative_directory,
            top_directory: &self.build_root,
            working_directory,
        }
    }

    fn snapshot(&self) -> MakeInputSnapshot {
        let selections = [&self.source_root, &self.build_root]
            .into_iter()
            .map(|root| MakeInputRootSelection {
                root: root.to_owned(),
                content_sha256: require(
                    measure_tree_content_cas(root),
                    "could not measure synthetic input root",
                )
                .payload_digest_excluding(None),
            })
            .collect::<Vec<_>>();
        require(
            MakeInputSnapshot::capture(&self.working_directory, &selections),
            "could not capture synthetic input roots",
        )
    }
}

fn synthetic_environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "PATH".into(),
            require(
                std::env::var("PATH"),
                "test process has no executable search path",
            ),
        ),
        ("SYNTH_ENV".into(), "environment-value".into()),
    ])
}

fn invocation(
    working_directory: &Path,
    environment: &BTreeMap<String, String>,
) -> NativeMakeInvocation {
    let arguments = [
        "-r",
        "-R",
        "-n",
        "-s",
        "--no-print-directory",
        "--file=mmakefile",
        "root",
        "CLI_VALUE=command-value",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    require(
        NativeMakeInvocation::new(
            arguments,
            working_directory.to_owned(),
            environment
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        ),
        "could not construct synthetic native invocation",
    )
}

fn make_binary() -> &'static str {
    if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    }
}

/// Each observer aborts while parsing `--eval`, before the explicit input or
/// any recipe can run. The two packets belong to the same synthetic invocation.
fn native_output(invocation: &NativeMakeInvocation, observer: &str) -> Vec<u8> {
    let mut command = Command::new(make_binary());
    command
        .args(["--eval", observer])
        .args(invocation.arguments())
        .current_dir(invocation.working_directory())
        .env_clear()
        .envs(invocation.environment())
        .stdin(Stdio::null());
    let output = require(
        run_output_with_control(
            &mut command,
            DEFAULT_CAPTURE_LIMIT,
            Duration::from_secs(5),
            &CancellationToken::default(),
        ),
        "synthetic GNU Make observer did not complete",
    );
    assert!(
        !output.timed_out,
        "synthetic GNU Make observer exceeded its deadline"
    );
    assert!(
        !output.cancelled,
        "synthetic GNU Make observer was cancelled"
    );
    assert!(
        output.status.code() == Some(2),
        "synthetic GNU Make observer did not stop before input files"
    );
    assert!(
        require(
            output.stderr.exact_bytes().ok_or(()),
            "synthetic observer stderr was truncated"
        )
        .windows(b"AROS_CHILD_SOURCE_TEST_STOP_BEFORE_FILES".len())
        .any(|window| window == b"AROS_CHILD_SOURCE_TEST_STOP_BEFORE_FILES"),
        "synthetic observer did not reach its selected pre-file stop",
    );
    require(
        output.stdout.exact_bytes().map(<[u8]>::to_vec).ok_or(()),
        "synthetic GNU Make observer output exceeded its capture bound",
    )
}

fn pre_file_observation(invocation: &NativeMakeInvocation) -> (Vec<u8>, BTreeSet<String>) {
    let startup = require(
        LiteralMakeStartupArguments::admit(invocation.arguments()),
        "synthetic child arguments were not admitted",
    );
    let command_line = require(
        command_line_values_fragment(startup.command_line_names()),
        "could not prepare synthetic command-line packet",
    );
    let discovery = format!(
        "{}{}$(error AROS_CHILD_SOURCE_TEST_STOP_BEFORE_FILES)\n",
        pre_file_fragment(),
        command_line,
    );
    let first_output = native_output(invocation, &discovery);
    let namespace = require(
        ValidatedMakeNamespace::from_make_stdout(&first_output),
        "synthetic namespace packet was invalid",
    );
    let environment_names = namespace
        .to_observation()
        .entries
        .into_iter()
        .filter(|entry| entry.origin == MakeVariableOrigin::Environment)
        .map(|entry| entry.name)
        .collect::<BTreeSet<_>>();
    let environment = require(
        environment_values_fragment(&environment_names),
        "could not prepare synthetic environment packet",
    );
    let complete = format!(
        "{}{}{}$(error AROS_CHILD_SOURCE_TEST_STOP_BEFORE_FILES)\n",
        pre_file_fragment(),
        command_line,
        environment,
    );
    (native_output(invocation, &complete), environment_names)
}

fn read_child(
    projection: &NativeOwnerProjection,
    paths: &MakeProgramPaths<'_>,
    generated_owner: &str,
    invocation: &NativeMakeInvocation,
    initial_stdout: &[u8],
    environment_names: &BTreeSet<String>,
    snapshot: &MakeInputSnapshot,
) -> Result<MatchedChildSource, String> {
    let roots = ["root".to_owned()];
    projection.read_captured_child_source(
        paths,
        CapturedChildSourceRequest {
            roots: &roots,
            generated_owner,
            invocation,
            initial_stdout,
            environment_names,
            snapshot,
        },
    )
}

fn assert_static_error(
    result: Result<MatchedChildSource, String>,
    expected: &str,
    diagnostic: &'static str,
) {
    let Err(error) = result else {
        panic!("{diagnostic}")
    };
    assert!(error == expected, "{diagnostic}");
}

#[test]
fn captured_child_source_joins_inputs_and_native_values_as_consistency_data() {
    let fixture = Fixture::new();
    assert!(
        !fixture.source_root.starts_with(&fixture.build_root)
            && !fixture.build_root.starts_with(&fixture.source_root),
        "synthetic source and build roots must be disjoint"
    );
    assert!(
        fixture.working_directory == fixture.build_root.join("one"),
        "synthetic child cwd must be build/one"
    );
    let environment = synthetic_environment();
    let child = invocation(&fixture.working_directory, &environment);
    let (initial_stdout, environment_names) = pre_file_observation(&child);
    assert!(
        environment_names.contains("SYNTH_ENV"),
        "synthetic environment value was not observed"
    );
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let relative_directory = Path::new("one");
    let paths = fixture.paths(&dirs, relative_directory, &fixture.working_directory);
    let matched = require(
        read_child(
            &fixture.projection,
            &paths,
            "one/mmakefile",
            &child,
            &initial_stdout,
            &environment_names,
            &snapshot,
        ),
        "coordinate-matched captured source could not be read",
    );
    let (coordinates, mut program, file, reads) = matched.into_parts();
    assert!(
        coordinates.generated_owner() == "one/mmakefile"
            && coordinates.source_input() == "one/mmakefile.src"
            && coordinates.goals().len() == 1
            && coordinates.goals()[0] == "root",
        "returned source coordinates did not retain the selected owner"
    );
    assert!(
        !file.events.is_empty(),
        "captured source produced no file events"
    );
    assert!(
        reads.reads().len() == 2
            && reads.reads()[0].requested == "mmakefile"
            && reads.reads()[1].requested == "extra.mk",
        "captured explicit input and include order differed"
    );
    let (values, evaluation_reads) = require(
        program.evaluate_captured(
            "$(SIMPLE)|$(RECURSIVE)|$(INCLUDED)|$(CLI_VALUE)|$(SYNTH_ENV)",
            &paths,
            &snapshot,
        ),
        "captured generic source values could not be evaluated",
    );
    assert!(
        values == "before|after|include-value|command-value|environment-value",
        "captured timing, include, command-line, or environment values differed"
    );
    assert!(
        evaluation_reads.reads().is_empty(),
        "scalar value evaluation unexpectedly performed captured reads"
    );

    require(
        std::fs::write(
            fixture.working_directory.join("mmakefile"),
            "LATER := changed-after-capture\n",
        ),
        "could not change the synthetic build input",
    );
    assert!(
        read_child(
            &fixture.projection,
            &paths,
            "one/mmakefile",
            &child,
            &initial_stdout,
            &environment_names,
            &snapshot,
        )
        .is_err(),
        "changed captured build input was accepted"
    );
}

#[test]
fn child_coordinates_and_required_packets_are_checked_before_source_reading() {
    let fixture = Fixture::new();
    let environment = synthetic_environment();
    let child = invocation(&fixture.working_directory, &environment);
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let one = Path::new("one");
    let two = Path::new("two");
    let valid_paths = fixture.paths(&dirs, one, &fixture.working_directory);
    let no_environment_names = BTreeSet::new();

    let foreign_owner_paths = fixture.paths(&dirs, two, &fixture.working_directory);
    assert_static_error(
        read_child(
            &fixture.projection,
            &foreign_owner_paths,
            "two/mmakefile",
            &child,
            b"invalid observation packet",
            &no_environment_names,
            &snapshot,
        ),
        "child cwd differs from selected owner directory",
        "foreign owner coordinates were not refused before packet parsing",
    );

    let foreign_cwd = fixture.build_root.join("two");
    let foreign_cwd_paths = fixture.paths(&dirs, one, &foreign_cwd);
    assert_static_error(
        read_child(
            &fixture.projection,
            &foreign_cwd_paths,
            "one/mmakefile",
            &child,
            b"invalid observation packet",
            &no_environment_names,
            &snapshot,
        ),
        "child source interpreter paths differ from selected coordinates",
        "foreign cwd was not refused before packet parsing",
    );

    let foreign_relative_paths = fixture.paths(&dirs, two, &fixture.working_directory);
    assert_static_error(
        read_child(
            &fixture.projection,
            &foreign_relative_paths,
            "one/mmakefile",
            &child,
            b"invalid observation packet",
            &no_environment_names,
            &snapshot,
        ),
        "child source interpreter paths differ from selected coordinates",
        "foreign relative path was not refused before packet parsing",
    );

    let aliased_relative_paths =
        fixture.paths(&dirs, Path::new("./one"), &fixture.working_directory);
    assert_static_error(
        read_child(
            &fixture.projection,
            &aliased_relative_paths,
            "one/mmakefile",
            &child,
            b"invalid observation packet",
            &no_environment_names,
            &snapshot,
        ),
        "child source interpreter paths differ from selected coordinates",
        "lexically aliased relative path was not refused before packet parsing",
    );

    assert_static_error(
        read_child(
            &fixture.projection,
            &valid_paths,
            "one/mmakefile",
            &child,
            b"",
            &no_environment_names,
            &snapshot,
        ),
        "invalid native initial namespace",
        "missing native packets were accepted",
    );
}

#[test]
fn changed_source_discovery_is_refused_before_native_packet_parsing() {
    let fixture = Fixture::new();
    let new_owner = fixture.source_root.join("three");
    require(
        std::fs::create_dir_all(&new_owner),
        "could not add synthetic source owner",
    );
    require(
        std::fs::write(new_owner.join("mmakefile.src"), "#MM root\n"),
        "could not add synthetic source input",
    );
    let environment = synthetic_environment();
    let child = invocation(&fixture.working_directory, &environment);
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    assert_static_error(
        read_child(
            &fixture.projection,
            &paths,
            "one/mmakefile",
            &child,
            b"invalid observation packet",
            &BTreeSet::new(),
            &snapshot,
        ),
        "child source projection revalidation failed",
        "changed source discovery was not refused before packet parsing",
    );
}

#[test]
fn foreign_directory_aliases_do_not_replace_measured_native_absence() {
    let fixture = Fixture::new();
    let child = invocation(&fixture.working_directory, &synthetic_environment());
    let (stdout, environment_names) = pre_file_observation(&child);
    let dirs = DirVars::from_config_text("FOREIGN_ONLY := injected-from-other-config\n");
    assert_eq!(
        dirs.expand("$(FOREIGN_ONLY)"),
        Some("injected-from-other-config".into())
    );
    require(
        std::fs::write(
            fixture.working_directory.join("mmakefile"),
            "MEASURED_ABSENCE := $(FOREIGN_ONLY)\n",
        ),
        "could not install synthetic fallback counter",
    );
    let snapshot = fixture.snapshot();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let matched = require(
        read_child(
            &fixture.projection,
            &paths,
            "one/mmakefile",
            &child,
            &stdout,
            &environment_names,
            &snapshot,
        ),
        "native source read did not preserve measured absence",
    );
    let (_, mut program, _, _) = matched.into_parts();
    let (value, _) = require(
        program.evaluate_captured("$(MEASURED_ABSENCE)", &paths, &snapshot),
        "measured-absence value was unavailable",
    );
    assert!(
        value.is_empty(),
        "native source borrowed the foreign directory value instead of measured absence"
    );
}

#[test]
fn source_content_drift_is_refused_even_with_a_fresh_input_snapshot() {
    let mut fixture = Fixture::new();
    fixture.projection.snapshots.insert(
        "one/mmakefile.src".into(),
        aros_common::sha256_bytes(b"#MM root\n").to_string(),
    );
    require(
        fixture.projection.verify(&fixture.source_root),
        "synthetic source hash positive failed",
    );
    require(
        std::fs::write(
            fixture.source_root.join("one/mmakefile.src"),
            "#MM changed\n",
        ),
        "could not alter synthetic source",
    );
    let child = invocation(&fixture.working_directory, &synthetic_environment());
    let snapshot = fixture.snapshot();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    assert_static_error(
        read_child(
            &fixture.projection,
            &paths,
            "one/mmakefile",
            &child,
            b"invalid packet",
            &BTreeSet::new(),
            &snapshot,
        ),
        "child source projection revalidation failed",
        "source byte drift was accepted before packet parsing",
    );
}
