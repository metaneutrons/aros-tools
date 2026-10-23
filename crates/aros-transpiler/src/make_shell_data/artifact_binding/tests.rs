#![cfg(unix)]

use super::{
    bind_recipe_artifacts, RecipeArtifactDirection, RecipeArtifactRequest, RecipeArtifactSlot,
};
use crate::make_shell_data::{
    parse_literal_recipe, program_binding::bind_literal_programs, RecipeRedirectionKind,
};
use aros_common::{
    captured_programs::{CapturedProgramSet, ProgramRootSelection, ProgramSelection},
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    measure_tree_content_cas, sha256_bytes, TreeContentEntryKind,
};
use std::{
    fs,
    ops::Range,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
};

const TOOL_BYTES: &[u8] = b"opaque selected compiler fixture; never executed";
const SCRIPT_BYTES: &[u8] = b"#!/usr/bin/env python3\nprint('fixture')\n";
const C_INPUT_BYTES: &[u8] = b"\x00C source\xff\n";

struct Fixture {
    temporary: tempfile::TempDir,
    source: PathBuf,
    build: PathBuf,
    tool: PathBuf,
    snapshot: MakeInputSnapshot,
    programs: CapturedProgramSet,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let source = base.join("source");
        let build = base.join("build");
        let program_root = base.join("programs");
        fs::create_dir_all(source.join("scripts")).unwrap();
        fs::create_dir_all(source.join("c")).unwrap();
        fs::create_dir_all(build.join("generated")).unwrap();
        fs::create_dir_all(build.join("elsewhere")).unwrap();
        fs::create_dir_all(&program_root).unwrap();

        fs::write(source.join("scripts/convert-é.py"), SCRIPT_BYTES).unwrap();
        fs::write(source.join("c/input-é.c"), C_INPUT_BYTES).unwrap();
        fs::set_permissions(
            source.join("scripts/convert-é.py"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let tool = program_root.join("target-compiler");
        fs::write(&tool, TOOL_BYTES).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();

        let snapshot =
            MakeInputSnapshot::capture(&build, &[input_root(&source), input_root(&build)]).unwrap();
        let programs = CapturedProgramSet::capture(
            &[program_root_selection(&program_root)],
            &[program_selection(&tool, TOOL_BYTES)],
        )
        .unwrap();
        Self {
            temporary,
            source,
            build,
            tool,
            snapshot,
            programs,
        }
    }

    fn bind(
        &self,
        recipe: &str,
        requests: &[RecipeArtifactRequest],
        output_root: Option<&Path>,
    ) -> Result<Vec<super::BoundRecipeArtifact>, String> {
        let statements = parse_literal_recipe(recipe)?;
        let bindings = bind_literal_programs(&statements, &self.programs)?;
        self.snapshot
            .transaction(|reader| bind_recipe_artifacts(&[bindings], reader, output_root, requests))
            .map(|(artifacts, _)| artifacts)
    }

    fn main_recipe(&self, output: &str) -> String {
        format!(
            "{} {} {} < {} > {}",
            shell_quote(self.tool.to_str().unwrap()),
            shell_quote(&format!("--script={}", self.script().display())),
            shell_quote(&format!("--input={}", self.c_input().display())),
            shell_quote(self.c_input().to_str().unwrap()),
            shell_quote(output),
        )
    }

    fn script(&self) -> PathBuf {
        self.source.join("scripts/convert-é.py")
    }

    fn c_input(&self) -> PathBuf {
        self.source.join("c/input-é.c")
    }

    fn main_requests(&self) -> Vec<RecipeArtifactRequest> {
        let script_word = format!("--script={}", self.script().display());
        let input_word = format!("--input={}", self.c_input().display());
        vec![
            word_request(
                1,
                "--script=".len()..script_word.len(),
                RecipeArtifactDirection::Input,
            ),
            word_request(
                2,
                "--input=".len()..input_word.len(),
                RecipeArtifactDirection::Input,
            ),
            redirection_request(0, RecipeArtifactDirection::Input),
            redirection_request(1, RecipeArtifactDirection::Output),
        ]
    }
}

fn input_root(path: &Path) -> MakeInputRootSelection {
    let root = path.canonicalize().unwrap();
    MakeInputRootSelection {
        content_sha256: measure_tree_content_cas(&root)
            .unwrap()
            .payload_digest_excluding(None),
        root,
    }
}

fn program_root_selection(path: &Path) -> ProgramRootSelection {
    let root = path.canonicalize().unwrap();
    ProgramRootSelection {
        content_sha256: measure_tree_content_cas(&root)
            .unwrap()
            .payload_digest_excluding(None),
        root,
    }
}

fn program_selection(path: &Path, bytes: &[u8]) -> ProgramSelection {
    ProgramSelection {
        role: "compiler".into(),
        root_index: 0,
        invocation_path: path.to_owned(),
        executable_sha256: sha256_bytes(bytes),
    }
}

const fn word_request(
    index: usize,
    range: Range<usize>,
    direction: RecipeArtifactDirection,
) -> RecipeArtifactRequest {
    RecipeArtifactRequest {
        recipe: 0,
        command: 0,
        slot: RecipeArtifactSlot::Word { index, range },
        direction,
    }
}

const fn redirection_request(
    index: usize,
    direction: RecipeArtifactDirection,
) -> RecipeArtifactRequest {
    redirection_request_at(0, index, direction)
}

const fn redirection_request_at(
    command: usize,
    index: usize,
    direction: RecipeArtifactDirection,
) -> RecipeArtifactRequest {
    RecipeArtifactRequest {
        recipe: 0,
        command,
        slot: RecipeArtifactSlot::Redirection(index),
        direction,
    }
}

fn shell_quote(value: &str) -> String {
    let mut quoted = String::from("'");
    for character in value.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

#[test]
fn and_or_artifact_coordinates_keep_later_operand_inputs_without_execution() {
    let fixture = Fixture::new();
    let output = "generated/and-or.bin";
    let recipe = format!("false && {} || report", fixture.main_recipe(output));
    let mut requests = fixture.main_requests();
    for request in &mut requests {
        request.command = 1;
    }
    let artifacts = fixture
        .bind(&recipe, &requests, Some(&fixture.build))
        .unwrap();
    assert_eq!(artifacts.len(), 4);
    assert_eq!(artifacts[0].path(), fixture.script());
    assert_eq!(
        artifacts[1].input_sha256(),
        Some(&sha256_bytes(C_INPUT_BYTES))
    );
    assert_eq!(artifacts[2].path(), fixture.c_input());
    assert!(!fixture.build.join(output).exists());
    for wrong in [0, 2, 3] {
        requests[0].command = wrong;
        assert!(fixture
            .bind(&recipe, &requests, Some(&fixture.build))
            .is_err());
    }
}

#[test]
fn binds_non_executable_script_and_binary_inputs_by_explicit_utf8_byte_spans() {
    let fixture = Fixture::new();
    let output = "generated/result.bin";
    let recipe = fixture.main_recipe(output);
    let requests = fixture.main_requests();

    let artifacts = fixture
        .bind(&recipe, &requests, Some(&fixture.build))
        .unwrap();

    assert_eq!(artifacts.len(), 4);
    assert_eq!(artifacts[0].path(), fixture.script().as_path());
    assert_eq!(
        fs::metadata(fixture.script()).unwrap().permissions().mode() & 0o111,
        0
    );
    assert_eq!(
        artifacts[0].previous_kind(),
        Some(TreeContentEntryKind::RegularFile)
    );
    assert_eq!(
        artifacts[0].input_sha256(),
        Some(&sha256_bytes(SCRIPT_BYTES))
    );
    assert_eq!(artifacts[1].path(), fixture.c_input().as_path());
    assert_eq!(
        artifacts[1].input_sha256(),
        Some(&sha256_bytes(C_INPUT_BYTES))
    );
    assert_eq!(artifacts[2].path(), fixture.c_input().as_path());
    assert_eq!(
        artifacts[2].input_sha256(),
        Some(&sha256_bytes(C_INPUT_BYTES))
    );
    assert_eq!(artifacts[3].path(), fixture.build.join(output).as_path());
    assert_eq!(artifacts[3].previous_kind(), None);
    assert!(artifacts[3].input_sha256().is_none());
    assert!(
        !fixture.build.join(output).exists(),
        "binding must not create output files"
    );
    assert_eq!(fs::read(fixture.script()).unwrap(), SCRIPT_BYTES);
}

#[test]
fn accepts_existing_regular_output_without_overwriting_and_keeps_repeated_output_records() {
    let fixture = Fixture::new();
    let output = fixture.build.join("generated/result.bin");
    let old_bytes = b"pre-existing output bytes";
    fs::write(&output, old_bytes).unwrap();
    // The snapshot must bind the original pre-existing output state.
    let fixture = Fixture {
        snapshot: MakeInputSnapshot::capture(
            &fixture.build,
            &[input_root(&fixture.source), input_root(&fixture.build)],
        )
        .unwrap(),
        ..fixture
    };
    let recipe = format!(
        "{} > {} ; {} >> {}",
        shell_quote(fixture.tool.to_str().unwrap()),
        shell_quote("generated/result.bin"),
        shell_quote(fixture.tool.to_str().unwrap()),
        shell_quote("generated/result.bin"),
    );
    let requests = [
        redirection_request_at(0, 0, RecipeArtifactDirection::Output),
        redirection_request_at(1, 0, RecipeArtifactDirection::Output),
    ];

    let artifacts = fixture
        .bind(&recipe, &requests, Some(&fixture.build))
        .unwrap();

    assert_eq!(artifacts.len(), 2);
    assert!(artifacts
        .iter()
        .all(|artifact| artifact.path() == output.as_path()));
    assert!(artifacts
        .iter()
        .all(|artifact| artifact.previous_kind() == Some(TreeContentEntryKind::RegularFile)));
    assert_eq!(artifacts[0].request(), &requests[0]);
    assert_eq!(artifacts[1].request(), &requests[1]);
    assert_ne!(
        artifacts[0].request().command,
        artifacts[1].request().command
    );
    assert_eq!(fs::read(output).unwrap(), old_bytes);
}

#[test]
fn redirection_slots_enforce_input_and_output_direction() {
    let fixture = Fixture::new();
    let base_recipe = fixture.main_recipe("generated/result.bin");
    let recipe = format!("{base_recipe} >> {}", shell_quote("generated/result.bin"));
    let parsed = parse_literal_recipe(&recipe).unwrap();
    let binding = bind_literal_programs(&parsed, &fixture.programs).unwrap();
    let command = binding.commands()[0].command();
    assert_eq!(command.redirections().len(), 3);
    assert_eq!(
        command.redirections()[0].kind(),
        RecipeRedirectionKind::Input
    );
    assert_eq!(
        command.redirections()[1].kind(),
        RecipeRedirectionKind::Output
    );
    assert_eq!(
        command.redirections()[2].kind(),
        RecipeRedirectionKind::Append
    );

    let input = fixture
        .bind(
            &recipe,
            &[redirection_request(0, RecipeArtifactDirection::Input)],
            Some(&fixture.build),
        )
        .unwrap();
    assert_eq!(input[0].path(), fixture.c_input().as_path());
    assert_eq!(input[0].input_sha256(), Some(&sha256_bytes(C_INPUT_BYTES)));
    assert!(fixture
        .bind(
            &recipe,
            &[redirection_request(0, RecipeArtifactDirection::Output)],
            Some(&fixture.build),
        )
        .is_err());
    assert!(fixture
        .bind(
            &recipe,
            &[redirection_request(1, RecipeArtifactDirection::Input)],
            Some(&fixture.build),
        )
        .is_err());
    assert!(fixture
        .bind(
            &recipe,
            &[redirection_request(1, RecipeArtifactDirection::Output)],
            None,
        )
        .unwrap_err()
        .contains("scope is absent"));
}

#[test]
fn word_ranges_must_land_on_utf8_boundaries_and_leave_other_bytes_unselected() {
    let fixture = Fixture::new();
    let recipe = fixture.main_recipe("generated/result.bin");
    let input_word = format!("--input={}", fixture.c_input().display());
    let accent = input_word.find('é').unwrap();
    let bad = word_request(2, accent + 1..accent + 2, RecipeArtifactDirection::Input);
    let error = fixture
        .bind(&recipe, &[bad], Some(&fixture.build))
        .unwrap_err();
    assert!(error.contains("range is invalid"));

    let good = word_request(
        2,
        "--input=".len()..input_word.len(),
        RecipeArtifactDirection::Input,
    );
    let artifacts = fixture
        .bind(&recipe, &[good], Some(&fixture.build))
        .unwrap();
    assert_eq!(artifacts[0].path(), fixture.c_input().as_path());
}

#[test]
fn rejects_command_word_duplicate_or_overlapping_slots_and_invalid_indexes() {
    let fixture = Fixture::new();
    let recipe = fixture.main_recipe("generated/result.bin");
    let command_word = word_request(0, 0..1, RecipeArtifactDirection::Input);
    assert!(fixture
        .bind(&recipe, &[command_word], Some(&fixture.build))
        .unwrap_err()
        .contains("program word"));

    let input_word = format!("--input={}", fixture.c_input().display());
    let input_start = "--input=".len();
    let overlapping = [
        word_request(
            2,
            input_start..input_word.len(),
            RecipeArtifactDirection::Input,
        ),
        word_request(
            2,
            input_start + 1..input_word.len() - 1,
            RecipeArtifactDirection::Input,
        ),
    ];
    assert!(fixture
        .bind(&recipe, &overlapping, Some(&fixture.build))
        .unwrap_err()
        .contains("overlap"));
    let script_word = format!("--script={}", fixture.script().display());
    let script_range = "--script=".len()..script_word.len();
    let duplicate = [
        word_request(1, script_range.clone(), RecipeArtifactDirection::Input),
        word_request(1, script_range, RecipeArtifactDirection::Input),
    ];
    assert!(fixture
        .bind(&recipe, &duplicate, Some(&fixture.build))
        .unwrap_err()
        .contains("overlap"));
    let duplicate_redirection = [
        redirection_request(1, RecipeArtifactDirection::Output),
        redirection_request(1, RecipeArtifactDirection::Output),
    ];
    assert!(fixture
        .bind(&recipe, &duplicate_redirection, Some(&fixture.build))
        .unwrap_err()
        .contains("overlap"));

    let invalid_recipe = RecipeArtifactRequest {
        recipe: 1,
        command: 0,
        slot: RecipeArtifactSlot::Word {
            index: 1,
            range: 0..1,
        },
        direction: RecipeArtifactDirection::Input,
    };
    assert!(fixture
        .bind(&recipe, &[invalid_recipe], Some(&fixture.build))
        .unwrap_err()
        .contains("command is absent"));
    let invalid_command = RecipeArtifactRequest {
        recipe: 0,
        command: 1,
        slot: RecipeArtifactSlot::Word {
            index: 1,
            range: 0..1,
        },
        direction: RecipeArtifactDirection::Input,
    };
    assert!(fixture
        .bind(&recipe, &[invalid_command], Some(&fixture.build))
        .unwrap_err()
        .contains("command is absent"));
}

#[test]
fn rejects_absent_symlink_unsafe_out_of_scope_and_unparented_inputs_or_outputs() {
    let fixture = Fixture::new();
    let source_link = fixture.source.join("scripts/link.py");
    symlink("convert-é.py", &source_link).unwrap();
    let outside = fixture
        .temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("outside.c");
    fs::write(&outside, b"outside").unwrap();
    let output_link = fixture.build.join("generated/link.out");
    symlink("missing-target", &output_link).unwrap();
    let fixture = Fixture {
        snapshot: MakeInputSnapshot::capture(
            &fixture.build,
            &[input_root(&fixture.source), input_root(&fixture.build)],
        )
        .unwrap(),
        ..fixture
    };

    for (path, suffix) in [
        (fixture.source.join("c/missing.c"), "input is absent"),
        (source_link, "not a captured regular file"),
        (outside, "leaves captured roots"),
    ] {
        let recipe = format!(
            "{} {}",
            shell_quote(fixture.tool.to_str().unwrap()),
            shell_quote(path.to_str().unwrap())
        );
        let request = word_request(
            1,
            0..path.to_str().unwrap().len(),
            RecipeArtifactDirection::Input,
        );
        let error = fixture
            .bind(&recipe, &[request], Some(&fixture.build))
            .unwrap_err();
        assert!(error.contains(suffix), "{error}");
    }

    let narrow_output_root = fixture.build.join("generated");
    for (output, output_root, suffix) in [
        (
            "generated/link.out",
            fixture.build.clone(),
            "absent or a regular file",
        ),
        (
            "../../outside/out.bin",
            fixture.build.clone(),
            "leaves captured roots",
        ),
        (
            "elsewhere/out.bin",
            narrow_output_root,
            "outside selected scope",
        ),
        (
            "missing-parent/out.bin",
            fixture.build.clone(),
            "captured directory",
        ),
        ("generated/", fixture.build.clone(), "directory-only suffix"),
    ] {
        let recipe = fixture.main_recipe(output);
        let request = redirection_request(1, RecipeArtifactDirection::Output);
        let error = fixture
            .bind(&recipe, &[request], Some(&output_root))
            .unwrap_err();
        assert!(error.contains(suffix), "{error}");
    }
}

#[test]
fn late_invalid_request_discards_batch_without_exposing_prior_artifacts_or_values() {
    let fixture = Fixture::new();
    let output = fixture.build.join("generated/result.bin");
    let old_bytes = b"must remain unchanged";
    fs::write(&output, old_bytes).unwrap();
    let fixture = Fixture {
        snapshot: MakeInputSnapshot::capture(
            &fixture.build,
            &[input_root(&fixture.source), input_root(&fixture.build)],
        )
        .unwrap(),
        ..fixture
    };
    let recipe = fixture.main_recipe("generated/result.bin");
    let requests = [
        word_request(
            1,
            "--script=".len()..format!("--script={}", fixture.script().display()).len(),
            RecipeArtifactDirection::Input,
        ),
        word_request(
            1,
            format!("--script={}", fixture.script().display()).len() + 1
                ..format!("--script={}", fixture.script().display()).len() + 10,
            RecipeArtifactDirection::Input,
        ),
    ];

    let error = fixture
        .bind(&recipe, &requests, Some(&fixture.build))
        .unwrap_err();

    assert!(!error.contains(fixture.script().to_str().unwrap()));
    assert!(!error.contains("convert-é.py"));
    assert_eq!(fs::read(output).unwrap(), old_bytes);
}

#[test]
fn disjoint_fields_keep_request_order_and_limits_refuse_before_partial_results() {
    let fixture = Fixture::new();
    let first = fixture.script().to_str().unwrap().to_owned();
    let second = fixture.c_input().to_str().unwrap().to_owned();
    let joined = format!("{first}:{second}");
    let recipe = format!(
        "{} {}",
        shell_quote(fixture.tool.to_str().unwrap()),
        shell_quote(&joined)
    );
    // Select the later field first: insertion order is not field position order.
    let requests = [
        word_request(
            1,
            first.len() + 1..joined.len(),
            RecipeArtifactDirection::Input,
        ),
        word_request(1, 0..first.len(), RecipeArtifactDirection::Input),
    ];
    let artifacts = fixture.bind(&recipe, &requests, None).unwrap();
    assert_eq!(artifacts[0].path(), fixture.c_input());
    assert_eq!(artifacts[1].path(), fixture.script());
    assert_eq!(artifacts[0].request(), &requests[0]);
    assert_eq!(artifacts[1].request(), &requests[1]);
    let oversized = vec![requests[0].clone(); 65_537];
    assert!(fixture
        .bind(&recipe, &oversized, None)
        .unwrap_err()
        .contains("request limit"));
    let output_recipe = fixture.main_recipe("generated/new-output");
    assert!(fixture
        .bind(
            &output_recipe,
            &[redirection_request(1, RecipeArtifactDirection::Output)],
            None
        )
        .unwrap_err()
        .contains("output scope is absent"));
    assert!(!fixture.build.join("generated/new-output").exists());
}
