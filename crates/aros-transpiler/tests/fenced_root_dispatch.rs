//! Second-pass Root dispatch data consistency; no recipe or selected tool runs.

#![cfg(unix)]

use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot, VerifiedMakeInputReads},
    make_native_invocation::NativeMakeInvocation,
    make_native_sampler::{
        observe_first_input_make_with_recipe_controls, InitialMakeObservationRequest,
    },
    measure_tree_content_cas, sha256_bytes, CancellationToken, Sha256Digest,
};
use aros_transpiler::{
    dirs::DirVars,
    make_shell_data::LiteralRecipeStatement,
    make_vars::{MakeProgramPaths, MakeVariableProgram},
    root_dispatch_capture::{
        FencedRootDispatchPlan, FencedRootDispatchRequest, RootDispatchCapturePlan,
        RootDispatchCaptureSelection, RootDispatchFencedFiles, SourceCheckedRootDispatchPlan,
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const ROOT_INPUT: &str = "Root.mk";
const TARGET_PATTERN: &str = "%-quick";
const GOAL: &str = "build-quick";
const PREFIX_VARIABLE: &str = "CALL";
const PROGRAM_VARIABLE: &str = "MM";
const OPTIONS_VARIABLE: &str = "MM_OPTS";
const PROJECT: &str = "Fixture";
const PREFIX: &str = "env ONLY_SAFE=1";
const OPTIONS: &str = "--literal-option";

fn gnu_make() -> PathBuf {
    let search_path = std::env::var_os("PATH").expect("test runner provides PATH");
    let names: &[&str] = if cfg!(target_os = "macos") {
        &["gmake", "make"]
    } else {
        &["make", "gmake"]
    };
    for name in names {
        for directory in std::env::split_paths(&search_path) {
            let candidate = directory.join(name);
            if !candidate.is_file() {
                continue;
            }
            let Ok(canonical) = candidate.canonicalize() else {
                continue;
            };
            let Ok(version) = Command::new(&canonical).arg("--version").output() else {
                continue;
            };
            if version.status.success() && version.stdout.starts_with(b"GNU Make ") {
                return canonical;
            }
        }
    }
    panic!("fenced Root dispatch coverage requires GNU Make");
}

struct Fixture {
    _source_temporary: tempfile::TempDir,
    _generated_temporary: tempfile::TempDir,
    root: PathBuf,
    generated_root: PathBuf,
    program: PathBuf,
    observer: PathBuf,
    output: PathBuf,
    override_path: PathBuf,
    fence_path: PathBuf,
}

impl Fixture {
    fn new(late_read: bool) -> Self {
        let source_temporary = tempfile::tempdir().expect("temporary Root source fixture");
        let generated_temporary =
            tempfile::tempdir().expect("temporary generated evidence fixture");
        let root = source_temporary
            .path()
            .canonicalize()
            .expect("canonical source root");
        let generated_root = generated_temporary
            .path()
            .canonicalize()
            .expect("canonical generated evidence root");
        let program = root.join("selected-mmake");
        let observer = generated_root.join("selected-observer");
        let output = generated_root.join("private-capture.json");
        let override_path = generated_root.join("root-dispatch-override.mk");
        let fence_path = generated_root.join("root-dispatch-fence.mk");

        let root_source = format!(
            "include roles.mk\n\
             -include {}\n\
             export LIST = $(MAKEFILE_LIST)\n\
             export ORIGINAL_ONLY = $(realpath $(if $(filter env,$(firstword $(CALL))),original-only.txt,recorder-only.txt))\n\
             {TARGET_PATTERN}:\n\
             \t@+$(CALL) $(MM) $(MM_OPTS) {PROJECT}.$@\n",
            root.join("optional-absent.mk").display()
        );
        fs::write(
            root.join("original-only.txt"),
            b"original deferred export dependency\n",
        )
        .unwrap();
        fs::write(root.join("recorder-only.txt"), b"recorder dependency\n").unwrap();
        if late_read {
            let late_input = generated_root.join("late-input.mk");
            fs::write(&late_input, b"new read after instrumentation\n").unwrap();
        }
        let roles_source = format!(
            "{PROGRAM_VARIABLE} := {}\n{OPTIONS_VARIABLE} := {OPTIONS}\n",
            program.to_str().expect("dummy program path is UTF-8")
        );
        fs::write(root.join(ROOT_INPUT), root_source).expect("write synthetic Root source");
        fs::write(root.join("roles.mk"), roles_source).expect("write synthetic role include");

        Self {
            _source_temporary: source_temporary,
            _generated_temporary: generated_temporary,
            root,
            generated_root,
            program,
            observer,
            output,
            override_path,
            fence_path,
        }
    }

    fn selection<'a>(&'a self, prefix: &'a str) -> RootDispatchCaptureSelection<'a> {
        RootDispatchCaptureSelection {
            target_pattern: TARGET_PATTERN,
            goal: GOAL,
            prefix_variable: PREFIX_VARIABLE,
            program_variable: PROGRAM_VARIABLE,
            options_variable: OPTIONS_VARIABLE,
            project: PROJECT,
            original_prefix: prefix,
            program: self.program.to_str().expect("dummy program path is UTF-8"),
            options: OPTIONS,
            observer: &self.observer,
            output: &self.output,
            working_directory: &self.root,
        }
    }

    fn paths<'a>(&'a self, dirs: &'a DirVars) -> MakeProgramPaths<'a> {
        MakeProgramPaths {
            dirs,
            source_root: &self.root,
            relative_dir: Path::new(""),
            top_directory: &self.root,
            working_directory: &self.root,
        }
    }

    fn invocation(&self, arguments: Vec<String>, prefix: &str) -> NativeMakeInvocation {
        NativeMakeInvocation::new(
            arguments,
            self.root.clone(),
            [(PREFIX_VARIABLE.to_owned(), prefix.to_owned())],
        )
        .expect("construct inert native Make invocation")
    }

    fn snapshot(&self, include_generated: bool) -> MakeInputSnapshot {
        let mut roots = vec![root_selection(&self.root)];
        if include_generated {
            roots.push(root_selection(&self.generated_root));
        }
        MakeInputSnapshot::capture(&self.root, &roots)
            .expect("capture selected Root and generated evidence roots")
            .with_include_directories(&[])
            .expect("bind an explicitly empty Make include search path")
    }

    fn files(&self) -> RootDispatchFencedFiles<'_> {
        RootDispatchFencedFiles {
            override_input: self
                .override_path
                .to_str()
                .expect("generated override path is UTF-8"),
            fence_input: self
                .fence_path
                .to_str()
                .expect("generated fence path is UTF-8"),
        }
    }

    fn write_generated(&self, plan: &RootDispatchCapturePlan) {
        fs::write(&self.override_path, plan.override_bytes())
            .expect("write generated Root dispatch override");
        fs::write(
            &self.fence_path,
            RootDispatchCapturePlan::fallback_fence_bytes(),
        )
        .expect("write generated fallback fence");
    }

    fn assert_recorder_absent(&self) {
        assert!(
            !self.output.exists(),
            "data interpretation must not create the recorder output"
        );
    }
}

fn root_selection(root: &Path) -> MakeInputRootSelection {
    MakeInputRootSelection {
        root: root.to_owned(),
        content_sha256: measure_tree_content_cas(root)
            .expect("measure complete selected input root")
            .payload_digest_excluding(None),
    }
}

fn initial_arguments() -> Vec<String> {
    ["-s", "-r", "--no-print-directory", "-f", ROOT_INPUT, GOAL]
        .map(str::to_owned)
        .to_vec()
}

fn observe_program(
    make: &Path,
    executable_sha256: &Sha256Digest,
    context_sha256: &Sha256Digest,
    invocation: &NativeMakeInvocation,
    paths: &MakeProgramPaths<'_>,
) -> MakeVariableProgram {
    let observation = observe_first_input_make_with_recipe_controls(
        &InitialMakeObservationRequest {
            program: make,
            executable_sha256,
            invocation,
            context_sha256,
            timeout: Duration::from_secs(20),
        },
        &CancellationToken::default(),
        |_, _| Ok(()),
    )
    .expect("observe exact inert native GNU Make startup");
    MakeVariableProgram::from_native_invocation_observation(
        paths,
        observation.initial_stdout(),
        invocation,
        observation.environment_names(),
    )
    .expect("import initial state from the exact native observation")
}

struct Candidate {
    fixture: Fixture,
    make: PathBuf,
    executable_sha256: Sha256Digest,
    context_sha256: Sha256Digest,
    source: SourceCheckedRootDispatchPlan,
    recorder_reads: VerifiedMakeInputReads,
}

impl Candidate {
    fn new() -> Self {
        Self::with_late_read(false)
    }

    fn with_late_read(late_read: bool) -> Self {
        let fixture = Fixture::new(late_read);
        let make = gnu_make();
        let executable_sha256 = sha256_bytes(&fs::read(&make).expect("read GNU Make executable"));
        let context_sha256 = sha256_bytes(b"fenced-root-dispatch-differential-test");
        let source_snapshot = fixture.snapshot(false);
        let startup = aros_common::make_startup_arguments::LiteralMakeStartupArguments::admit(
            &initial_arguments(),
        )
        .expect("admit candidate native startup");
        let invocation = fixture.invocation(initial_arguments(), PREFIX);
        let dirs = DirVars::default();
        let paths = fixture.paths(&dirs);
        let mut program = observe_program(
            &make,
            &executable_sha256,
            &context_sha256,
            &invocation,
            &paths,
        );
        let root_bytes = fs::read(fixture.root.join(ROOT_INPUT)).expect("read synthetic Root");
        let source = RootDispatchCapturePlan::derive_captured(
            ROOT_INPUT,
            &sha256_bytes(&root_bytes),
            fixture.selection(PREFIX),
            &mut program,
            &paths,
            &source_snapshot,
            &startup,
        )
        .expect("derive candidate from the captured native initial state")
        .with_captured_recipe_controls(&mut program, &paths)
        .expect("retain measured candidate shell controls")
        .with_original_recipe(&mut program, &paths)
        .expect("retain original deferred exports and their distinct input reads");
        let (_, candidate_recorder_reads) = source
            .project_instrumented_recipe(&mut program, &paths)
            .expect("project candidate recorder command and reads");
        fixture.write_generated(source.capture_plan());
        fixture.assert_recorder_absent();

        Self {
            fixture,
            make,
            executable_sha256,
            context_sha256,
            source,
            recorder_reads: candidate_recorder_reads,
        }
    }

    fn prospective_startup(
        &self,
    ) -> aros_common::make_startup_arguments::LiteralMakeStartupArguments {
        self.source
            .prospective_fenced_startup(self.fixture.files(), &self.recorder_reads)
            .expect("construct exact prospective full fenced startup")
    }

    fn final_program(
        &self,
        startup: &aros_common::make_startup_arguments::LiteralMakeStartupArguments,
        prefix: &str,
    ) -> (NativeMakeInvocation, MakeVariableProgram) {
        let invocation = self
            .fixture
            .invocation(startup.arguments().to_vec(), prefix);
        let dirs = DirVars::default();
        let paths = self.fixture.paths(&dirs);
        let program = observe_program(
            &self.make,
            &self.executable_sha256,
            &self.context_sha256,
            &invocation,
            &paths,
        );
        (invocation, program)
    }

    fn rebind(
        &self,
        startup: &aros_common::make_startup_arguments::LiteralMakeStartupArguments,
        program: &mut MakeVariableProgram,
        snapshot: &MakeInputSnapshot,
    ) -> Result<FencedRootDispatchPlan, String> {
        let dirs = DirVars::default();
        let paths = self.fixture.paths(&dirs);
        self.source.rebind_fenced(
            FencedRootDispatchRequest {
                root_input: ROOT_INPUT,
                selected: self.fixture.selection(PREFIX),
                files: self.fixture.files(),
                startup,
                candidate_recorder_reads: &self.recorder_reads,
            },
            program,
            &paths,
            snapshot,
        )
    }
}

#[test]
fn fenced_rebind_uses_fresh_full_startup_and_tracks_generated_snapshot() {
    let candidate = Candidate::new();
    let startup = candidate.prospective_startup();
    let (invocation, mut program) = candidate.final_program(&startup, PREFIX);
    let final_snapshot = candidate.fixture.snapshot(true);
    let fenced = candidate
        .rebind(&startup, &mut program, &final_snapshot)
        .expect("rebind from a fresh observation of the exact prospective argv");

    assert_eq!(
        fenced.source().startup().arguments(),
        invocation.arguments(),
        "the final source plan retains the complete observed argument sequence"
    );
    assert_eq!(startup.arguments(), invocation.arguments());
    assert_eq!(
        &startup.arguments()[..initial_arguments().len()],
        initial_arguments().as_slice(),
        "the prospective full startup preserves the original ordered argv"
    );
    let expected_inputs = [
        ROOT_INPUT.to_owned(),
        candidate.fixture.files().override_input.to_owned(),
        candidate.fixture.files().fence_input.to_owned(),
    ];
    assert_eq!(startup.input_files(), expected_inputs.as_slice());
    let absent = candidate
        .source
        .source_reads()
        .reads()
        .iter()
        .find(|read| read.content_sha256.is_none())
        .expect("optional input absence was measured");
    assert!(startup.old_files().contains(&absent.identity));
    let original_only = candidate.fixture.root.join("original-only.txt");
    // realpath is a captured inventory query, not a Make input content read.
    // The complete Source snapshot binds it; do not invent an old-file entry.
    assert!(candidate.source.original_recipe_reads().is_some());
    assert_eq!(
        candidate.source.original_recipe().unwrap().source_exports()["ORIGINAL_ONLY"],
        original_only.to_str().unwrap()
    );
    assert_eq!(
        fenced.projection().source_exports()["ORIGINAL_ONLY"],
        original_only.to_str().unwrap(),
        "Source role-dependent inventory queries stay original; only input-list metadata changes"
    );
    assert!(startup
        .old_files()
        .contains(&candidate.fixture.files().override_input.to_owned()));
    assert!(startup
        .old_files()
        .contains(&candidate.fixture.files().fence_input.to_owned()));

    let expected_list = format!(
        "{ROOT_INPUT} roles.mk {} {}",
        candidate.fixture.files().override_input,
        candidate.fixture.files().fence_input,
    );
    assert_eq!(
        fenced.projection().source_exports().get("LIST"),
        Some(&expected_list),
        "the Source export sees Root, its include, override, then fence"
    );

    let shell = fenced
        .projection()
        .parse_literal_shell()
        .expect("parse inert recorder recipe");
    let [LiteralRecipeStatement::Command(command)] = shell.statements.as_slice() else {
        panic!("the projected recipe must contain exactly one recorder command");
    };
    let expected_words = [
        candidate
            .fixture
            .observer
            .to_str()
            .expect("dummy observer path is UTF-8")
            .to_owned(),
        "--capture-make-invocation".to_owned(),
        candidate
            .fixture
            .output
            .to_str()
            .expect("dummy output path is UTF-8")
            .to_owned(),
        "--".to_owned(),
        "env".to_owned(),
        "ONLY_SAFE=1".to_owned(),
        candidate
            .fixture
            .program
            .to_str()
            .expect("dummy program path is UTF-8")
            .to_owned(),
        OPTIONS.to_owned(),
        "Fixture.build-quick".to_owned(),
    ];
    assert_eq!(command.words(), expected_words.as_slice());
    assert!(command.redirections().is_empty());
    assert!(fenced.revalidate().is_ok());
    candidate.fixture.assert_recorder_absent();

    fs::write(
        candidate.fixture.generated_root.join("new-evidence.txt"),
        b"separately authorized by a caller\n",
    )
    .unwrap();
    let extended = candidate.fixture.snapshot(true);
    fenced
        .verify_read_closure(&extended)
        .expect("old reads remain exact in an explicitly extended snapshot");
    assert!(
        fenced.revalidate().is_err(),
        "read checking must not refresh the original complete snapshot"
    );

    fs::write(
        candidate.fixture.root.join("optional-absent.mk"),
        b"new input\n",
    )
    .unwrap();
    let changed_absence = candidate.fixture.snapshot(true);
    assert!(
        fenced.verify_read_closure(&changed_absence).is_err(),
        "a formerly absent input must still refuse in a fresh snapshot"
    );
    fs::remove_file(candidate.fixture.root.join("optional-absent.mk")).unwrap();

    fs::write(
        &candidate.fixture.fence_path,
        b"changed after successful bind\n",
    )
    .expect("change generated evidence after successful bind");
    assert!(
        fenced.revalidate().is_err(),
        "the final plan snapshot covers the generated evidence root"
    );
    let changed_bytes = candidate.fixture.snapshot(true);
    assert!(fenced.verify_read_closure(&changed_bytes).is_err());
    candidate.fixture.assert_recorder_absent();
}

#[test]
fn fenced_rebind_refuses_inconsistent_candidate_read_proof() {
    let candidate = Candidate::with_late_read(true);
    let snapshot = candidate.fixture.snapshot(true);
    // Deliberately supply a real verified read that was not produced by the
    // candidate recipe projection. Matching the final argv alone cannot make
    // this inconsistent manifest acceptable.
    let extra = candidate.fixture.generated_root.join("late-input.mk");
    let ((), inconsistent_reads) = snapshot
        .transaction(|reader| {
            reader
                .read_bytes(extra.to_str().unwrap())?
                .ok_or("missing counter input")?;
            Ok(())
        })
        .unwrap();
    let startup = candidate
        .source
        .prospective_fenced_startup(candidate.fixture.files(), &inconsistent_reads)
        .unwrap();
    let (_, mut program) = candidate.final_program(&startup, PREFIX);
    let dirs = DirVars::default();
    let paths = candidate.fixture.paths(&dirs);
    let result = candidate.source.rebind_fenced(
        FencedRootDispatchRequest {
            root_input: ROOT_INPUT,
            selected: candidate.fixture.selection(PREFIX),
            files: candidate.fixture.files(),
            startup: &startup,
            candidate_recorder_reads: &inconsistent_reads,
        },
        &mut program,
        &paths,
        &snapshot,
    );
    let Err(error) = result else {
        panic!("inconsistent candidate read proof must refuse");
    };
    assert!(
        error.contains("final read closure differs"),
        "unexpected failure boundary: {error}"
    );
    candidate.fixture.assert_recorder_absent();
}

#[test]
fn fenced_rebind_rejects_missing_old_file_changed_roles_and_generated_bytes() {
    let candidate = Candidate::new();
    let startup = candidate.prospective_startup();
    let good_snapshot = candidate.fixture.snapshot(true);

    let override_old_file = format!("--old-file={}", candidate.fixture.files().override_input);
    let mut missing_old_file_arguments = startup.arguments().to_vec();
    let old_file_index = missing_old_file_arguments
        .iter()
        .position(|argument| argument == &override_old_file)
        .expect("prospective startup protects the generated override");
    missing_old_file_arguments.remove(old_file_index);
    let missing_old_file_startup =
        aros_common::make_startup_arguments::LiteralMakeStartupArguments::admit(
            &missing_old_file_arguments,
        )
        .expect("admit the deliberately incomplete old-file set");
    let (_, mut missing_old_file_program) =
        candidate.final_program(&missing_old_file_startup, PREFIX);
    assert!(candidate
        .rebind(
            &missing_old_file_startup,
            &mut missing_old_file_program,
            &good_snapshot
        )
        .is_err());
    candidate
        .source
        .revalidate()
        .expect("candidate source is unchanged");

    let changed_prefix = "env ONLY_SAFE=2";
    let (_, mut changed_program) = candidate.final_program(&startup, changed_prefix);
    let Err(changed_error) = candidate.rebind(&startup, &mut changed_program, &good_snapshot)
    else {
        panic!("a fresh native packet with a changed Source role must refuse");
    };
    assert!(changed_error.contains("role values differ"));
    candidate
        .source
        .revalidate()
        .expect("candidate source is unchanged");

    let mut corrupt_override = fs::read(&candidate.fixture.override_path)
        .expect("read generated override for a byte-level countertest");
    corrupt_override.push(b'\n');
    fs::write(&candidate.fixture.override_path, corrupt_override)
        .expect("corrupt only the generated override bytes");
    let corrupted_snapshot = candidate.fixture.snapshot(true);
    let (_, mut corrupted_program) = candidate.final_program(&startup, PREFIX);
    let Err(bytes_error) = candidate.rebind(&startup, &mut corrupted_program, &corrupted_snapshot)
    else {
        panic!("a fresh snapshot with altered generated bytes must refuse");
    };
    assert!(bytes_error.contains("generated input bytes differ"));
    candidate
        .source
        .revalidate()
        .expect("candidate source is unchanged");
    candidate.fixture.assert_recorder_absent();
}
