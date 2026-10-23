//! Snapshot-bound Root dispatch derivation interprets source, not authority.

use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{
        MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin, ValidatedMakeNamespace,
    },
    make_startup_arguments::LiteralMakeStartupArguments,
    measure_tree_content_cas, sha256_bytes, Sha256Digest,
};
use aros_transpiler::{
    dirs::DirVars,
    make_vars::{MakeInitialFlavor, MakeInitialValue, MakeProgramPaths, MakeVariableProgram},
    parser::TargetContext,
    root_dispatch_capture::{
        RootDispatchCapturePlan, RootDispatchCaptureSelection, SourceCheckedRootDispatchPlan,
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[path = "root_dispatch_source/batched.rs"]
mod batched;

const ROOT_INPUT: &str = "Root.mk";
const TARGET_PATTERN: &str = "%-quick";
const GOAL: &str = "build-quick";
const PROJECT: &str = "Engine";
const PREFIX_VARIABLE: &str = "COMMAND_ENV";
const PROGRAM_VARIABLE: &str = "TOOL";
const OPTIONS_VARIABLE: &str = "TOOL_OPTIONS";
const ORIGINAL_PREFIX: &str = "env ROOT_DISPATCH_CAPTURE_PREFIX_SENTINEL=from-prefix";
const OPTIONS: &str = "--literal-option";

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    tool: PathBuf,
    observer: PathBuf,
    output: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().expect("temporary Root source fixture");
        let root = temporary
            .path()
            .canonicalize()
            .expect("canonical temporary Root source directory");
        Self {
            tool: root.join("selected/mmake"),
            observer: root.join("selected/observer"),
            output: root.join("recorder.json"),
            _temporary: temporary,
            root,
        }
    }

    fn tool(&self) -> &Path {
        &self.tool
    }

    fn observer(&self) -> &Path {
        &self.observer
    }

    fn output(&self) -> &Path {
        &self.output
    }

    fn write_inputs(&self, root_source: &str, roles_source: &str) {
        fs::write(self.root.join(ROOT_INPUT), root_source).expect("write synthetic Root source");
        fs::write(self.root.join("roles.mk"), roles_source)
            .expect("write synthetic Root role include");
    }

    fn snapshot(&self) -> MakeInputSnapshot {
        let content_sha256 = measure_tree_content_cas(&self.root)
            .expect("measure complete synthetic source root")
            .payload_digest_excluding(None);
        MakeInputSnapshot::capture(
            &self.root,
            &[MakeInputRootSelection {
                root: self.root.clone(),
                content_sha256,
            }],
        )
        .expect("capture complete synthetic source root")
        .with_include_directories(&[])
        .expect("bind an explicitly empty Make include search path")
    }

    fn selection<'a>(
        &'a self,
        original_prefix: &'a str,
        program: &'a str,
        options: &'a str,
        target_pattern: &'a str,
    ) -> RootDispatchCaptureSelection<'a> {
        RootDispatchCaptureSelection {
            target_pattern,
            goal: GOAL,
            prefix_variable: PREFIX_VARIABLE,
            program_variable: PROGRAM_VARIABLE,
            options_variable: OPTIONS_VARIABLE,
            project: PROJECT,
            original_prefix,
            program,
            options,
            observer: self.observer(),
            output: self.output(),
            working_directory: &self.root,
        }
    }

    fn selected(&self) -> RootDispatchCaptureSelection<'_> {
        let tool = self.tool().to_str().expect("selected tool path is UTF-8");
        self.selection(ORIGINAL_PREFIX, tool, OPTIONS, TARGET_PATTERN)
    }

    fn assert_recorder_absent(&self) {
        assert!(
            !self.output().exists(),
            "source interpretation must not create a recorder file"
        );
    }
}

fn roles_source(fixture: &Fixture) -> String {
    format!(
        "TOOL := {}\nTOOL_OPTIONS := {OPTIONS}\n",
        fixture.tool().display()
    )
}

fn root_source(extra: &str, pattern: &str, recipe: &str) -> String {
    format!(
        "COMMAND_ENV := {ORIGINAL_PREFIX}\n\
         include roles.mk\n\
         {extra}\n\
         ORDINARY := retained-value\n\
         UNRELATED_TARGET := build-diagnostics\n\
         $(UNRELATED_TARGET):\n\
         .PHONY: $(UNRELATED_TARGET)\n\
         {pattern}:\n\
         \t{recipe}\n"
    )
}

fn valid_root_source() -> String {
    root_source(
        "",
        TARGET_PATTERN,
        "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@",
    )
}

#[test]
fn source_suffix_candidates_are_checked_after_source_ordered_resets() {
    let fixture = Fixture::new();
    for (suffix_rules, goal, accepted) in [
        (
            ".SUFFIXES: .input .x-quick\n.input.x-quick:\n\t@false\n",
            "build.x-quick",
            false,
        ),
        (".SUFFIXES: .input\n.input:\n\t@false\n", GOAL, false),
        (".SUFFIXES: .c .h .d .s .S\n.c.h:\n\t@false\n", GOAL, true),
        (
            ".SUFFIXES: .input .x-quick\n.input.x-quick:\n\t@false\n.SUFFIXES:\n",
            "build.x-quick",
            true,
        ),
    ] {
        let source = root_source(
            suffix_rules,
            TARGET_PATTERN,
            "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@",
        );
        fixture.write_inputs(&source, &roles_source(&fixture));
        let snapshot = fixture.snapshot();
        let selected = RootDispatchCaptureSelection {
            goal,
            ..fixture.selected()
        };
        let startup = LiteralMakeStartupArguments::admit(&[
            "-r".into(),
            "-f".into(),
            ROOT_INPUT.into(),
            goal.into(),
        ])
        .unwrap();
        let result = capture_with_startup(
            &fixture,
            &expected_sha256(&source),
            selected,
            &snapshot,
            &startup,
        );
        assert_eq!(result.is_ok(), accepted);
        if !accepted {
            assert!(result.err().unwrap().contains("suffix rule"));
        }
        fixture.assert_recorder_absent();
    }
}

fn capture(
    fixture: &Fixture,
    expected_sha256: &Sha256Digest,
    selected: RootDispatchCaptureSelection<'_>,
    snapshot: &MakeInputSnapshot,
) -> Result<SourceCheckedRootDispatchPlan, String> {
    let startup = LiteralMakeStartupArguments::admit(&[
        "-r".into(),
        "-f".into(),
        ROOT_INPUT.into(),
        GOAL.into(),
    ])
    .unwrap();
    capture_with_startup(fixture, expected_sha256, selected, snapshot, &startup)
}

fn capture_with_startup(
    fixture: &Fixture,
    expected_sha256: &Sha256Digest,
    selected: RootDispatchCaptureSelection<'_>,
    snapshot: &MakeInputSnapshot,
    startup: &LiteralMakeStartupArguments,
) -> Result<SourceCheckedRootDispatchPlan, String> {
    let dirs = DirVars::from_config_text("");
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &fixture.root,
        relative_dir: Path::new(""),
        top_directory: &fixture.root,
        working_directory: &fixture.root,
    };
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[])?;
    RootDispatchCapturePlan::derive_captured(
        ROOT_INPUT,
        expected_sha256,
        selected,
        &mut program,
        &paths,
        snapshot,
        startup,
    )
}

#[test]
fn captured_recipe_controls_keep_source_values_and_refuse_missing_or_effectful_inputs() {
    let fixture = Fixture::new();
    for (raw_flags, expected) in [
        (
            Some("$(SELECTED_FLAGS)"),
            Some(("/selected/posix-shell", "-ec")),
        ),
        (Some(""), Some(("/selected/posix-shell", ""))),
        (None, None),
        (Some("$(shell touch forbidden-output)"), None),
    ] {
        let source = root_source(
            "SHELL := /selected/posix-shell\nSELECTED_FLAGS := -ec\n",
            TARGET_PATTERN,
            "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@",
        );
        fixture.write_inputs(&source, &roles_source(&fixture));
        let snapshot = fixture.snapshot();
        let dirs = DirVars::default();
        let paths = MakeProgramPaths {
            dirs: &dirs,
            source_root: &fixture.root,
            relative_dir: Path::new(""),
            top_directory: &fixture.root,
            working_directory: &fixture.root,
        };
        let namespace = ValidatedMakeNamespace::validate(MakeNamespaceObservation {
            version: 1,
            entries: [".VARIABLES", "MAKE_VERSION", ".SHELLFLAGS"]
                .map(|name| MakeNamespaceEntry {
                    name: name.into(),
                    origin: MakeVariableOrigin::Default,
                })
                .to_vec(),
        })
        .unwrap();
        let values = raw_flags.map(|value| MakeInitialValue {
            name: ".SHELLFLAGS",
            value,
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Default,
        });
        let mut program = MakeVariableProgram::new_source_program(
            &paths,
            &namespace,
            &values.into_iter().collect::<Vec<_>>(),
        )
        .unwrap();
        let startup = LiteralMakeStartupArguments::admit(&[
            "-r".into(),
            "-f".into(),
            ROOT_INPUT.into(),
            GOAL.into(),
        ])
        .unwrap();
        let plan = RootDispatchCapturePlan::derive_captured(
            ROOT_INPUT,
            &sha256_bytes(source.as_bytes()),
            fixture.selected(),
            &mut program,
            &paths,
            &snapshot,
            &startup,
        )
        .unwrap();
        assert!(plan.recipe_controls().is_none());
        let bound = plan.with_captured_recipe_controls(&mut program, &paths);
        if let Some((shell, flags)) = expected {
            let bound = bound.unwrap();
            let controls = bound.recipe_controls().unwrap();
            assert_eq!(controls.shell(), shell);
            assert_eq!(controls.flags(), flags);
            bound.revalidate().unwrap();
            fs::write(fixture.root.join("roles.mk"), "changed\n").unwrap();
            assert!(bound.revalidate().is_err());
        } else {
            assert!(bound.is_err());
        }
        assert!(!fixture.root.join("forbidden-output").exists());
        fixture.assert_recorder_absent();
    }
}

#[test]
fn instrumented_projection_binds_literal_recorder_frame_and_checks_deferred_exports() {
    let fixture = Fixture::new();
    let large_prefix = format!("env EXTRA={}", "p".repeat(2048));
    let wide_prefix_export =
        format!("COMMAND_ENV := {large_prefix}\nexport PROOF = $@|$*|$<|$(@D)|$(@F)\n");
    for (export, prefix, accepted) in [
        ("export PROOF = $@|$*|$<|$(@D)|$(@F)\n", ORIGINAL_PREFIX, true),
        (
            "COMMAND_ENV := env Z_SAFE='last word' A_SAFE=first\nexport PROOF = $@|$*|$<|$(@D)|$(@F)\n",
            "env Z_SAFE='last word' A_SAFE=first",
            true,
        ),
        (
            "COMMAND_ENV := env ROLE_PROOF=prefix-wins\nexport PROOF = $@|$*|$<|$(@D)|$(@F)\n",
            "env ROLE_PROOF=prefix-wins",
            true,
        ),
        (wide_prefix_export.as_str(), large_prefix.as_str(), true),
        ("export PROOF = $(eval LEAK := changed)\n", ORIGINAL_PREFIX, false),
        ("export PROOF = $(shell touch forbidden-output)\n", ORIGINAL_PREFIX, false),
    ] {
        let role_exports = "export COMMAND_ENV TOOL TOOL_OPTIONS\n";
        let source = root_source(
            &format!("{export}{role_exports}export ROLE_PROOF = $(COMMAND_ENV)|$(TOOL)|$(TOOL_OPTIONS)\n"),
            TARGET_PATTERN,
            "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@",
        );
        fixture.write_inputs(&source, &roles_source(&fixture));
        let snapshot = fixture.snapshot();
        let dirs = DirVars::default();
        let paths = MakeProgramPaths {
            dirs: &dirs,
            source_root: &fixture.root,
            relative_dir: Path::new(""),
            top_directory: &fixture.root,
            working_directory: &fixture.root,
        };
        let namespace = ValidatedMakeNamespace::validate(MakeNamespaceObservation {
            version: 1,
            entries: [".VARIABLES", "MAKE_VERSION"]
                .map(|name| MakeNamespaceEntry {
                    name: name.into(),
                    origin: MakeVariableOrigin::Default,
                })
                .to_vec(),
        })
        .unwrap();
        let mut program = MakeVariableProgram::new_source_program(&paths, &namespace, &[]).unwrap();
        let startup = LiteralMakeStartupArguments::admit(&[
            "-r".into(),
            "-f".into(),
            ROOT_INPUT.into(),
            GOAL.into(),
        ])
        .unwrap();
        let plan = RootDispatchCapturePlan::derive_captured(
            ROOT_INPUT,
            &sha256_bytes(source.as_bytes()),
            RootDispatchCaptureSelection { original_prefix: prefix, ..fixture.selected() },
            &mut program,
            &paths,
            &snapshot,
            &startup,
        )
        .unwrap();
        let projected = plan.project_instrumented_recipe(&mut program, &paths);
        if accepted {
            let (projection, _) = projected.unwrap();
            assert_eq!(
                projection.source_exports()["PROOF"],
                "build-quick|build||.|build-quick"
            );
            let parsed = projection.parse_literal_shell().unwrap();
            let [aros_transpiler::make_shell_data::LiteralRecipeStatement::Command(command)] =
                parsed.statements.as_slice()
            else {
                panic!("expected single recorder command")
            };
            assert_eq!(command.words()[0], fixture.observer.to_str().unwrap());
            assert_eq!(command.words()[1], "--capture-make-invocation");
            assert_eq!(command.words().last().unwrap(), "Engine.build-quick");
            assert!(command.redirections().is_empty());
            let instrumented_roles = projection.source_exports()["ROLE_PROOF"].clone();
            let plan = plan.with_original_recipe(&mut program, &paths).unwrap();
            let original = plan.original_recipe().unwrap();
            assert_eq!(
                original.source_exports()["ROLE_PROOF"],
                format!("{prefix}|{}|{OPTIONS}", fixture.tool.display())
            );
            assert_eq!(original.source_exports()["ROLE_PROOF"], instrumented_roles,
                "static-pattern recorder leaves Source role values unchanged");
            assert_eq!(
                original.source_exports()["PROOF"],
                projection.source_exports()["PROOF"]
            );
            assert!(plan.original_recipe_reads().is_some());
            let prefix = aros_common::make_environment_prefix::LiteralMakeEnvironmentPrefix::parse(prefix).unwrap();
            let mut arguments = vec!["env".to_owned()];
            arguments.extend(prefix.assignments().iter().map(|(key, value)| format!("{key}={value}")));
            arguments.extend([fixture.tool.to_str().unwrap().to_owned(), OPTIONS.into(), format!("{PROJECT}.{GOAL}")]);
            let mut observed_exports = projection.source_exports().clone();
            observed_exports.insert("1ODD".into(), "opaque$literal".into());
            let raw = aros_common::make_native_invocation::NativeMakeInvocation::new(arguments, fixture.root.clone(), observed_exports).unwrap();
            let mut wrong_exports = raw.environment().clone();
            wrong_exports.insert("COMMAND_ENV".into(), "unverified role export".into());
            let wrong_raw = aros_common::make_native_invocation::NativeMakeInvocation::new(raw.arguments().to_vec(), fixture.root.clone(), wrong_exports).unwrap();
            assert!(plan.capture_plan().bind_capture_with_source_exports(wrong_raw, original, &projection).is_err());
            let restored = plan.capture_plan().bind_capture_with_source_exports(raw, original, &projection).unwrap();
            let expected_role = prefix.assignments().get("ROLE_PROOF").unwrap_or_else(|| &original.source_exports()["ROLE_PROOF"]);
            assert_eq!(&restored.invocation().environment()["ROLE_PROOF"], expected_role);
            assert_eq!(restored.invocation().environment()["1ODD"], "opaque$literal");
            assert_eq!(restored.raw().environment()["ROLE_PROOF"], projection.source_exports()["ROLE_PROOF"]);
            assert!(plan.with_original_recipe(&mut program, &paths).is_err());
        } else {
            assert!(projected.is_err());
            assert!(plan.with_original_recipe(&mut program, &paths).is_err());
        }
        assert_eq!(
            program.evaluate("$(origin LEAK)", &paths).unwrap(),
            "undefined"
        );
        assert!(!fixture.root.join("forbidden-output").exists());
        fixture.assert_recorder_absent();
    }
}

#[test]
fn rejects_indirect_or_additional_goal_inputs_and_enabled_builtin_rules() {
    let fixture = Fixture::new();
    let source = valid_root_source();
    fixture.write_inputs(&source, &roles_source(&fixture));
    let snapshot = fixture.snapshot();
    for arguments in [
        vec!["-r", "-f", ROOT_INPUT, "parent"],
        vec!["-r", "-f", ROOT_INPUT, GOAL, "parent"],
        vec!["-r", "-f", ROOT_INPUT, "-f", "roles.mk", GOAL],
        vec!["-f", ROOT_INPUT, GOAL],
    ] {
        let arguments = arguments.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let startup = LiteralMakeStartupArguments::admit(&arguments).unwrap();
        let result = capture_with_startup(
            &fixture,
            &expected_sha256(&source),
            fixture.selected(),
            &snapshot,
            &startup,
        );
        assert!(result.is_err_and(|error| error.contains("sole direct goal")));
        fixture.assert_recorder_absent();
    }
}

fn expected_sha256(root_source: &str) -> Sha256Digest {
    sha256_bytes(root_source.as_bytes())
}

#[test]
fn interprets_root_and_include_and_accepts_only_the_selected_literal_dispatch() {
    let fixture = Fixture::new();
    let root_source = valid_root_source();
    fixture.write_inputs(&root_source, &roles_source(&fixture));
    let snapshot = fixture.snapshot();
    let expected_sha256 = expected_sha256(&root_source);

    let checked = capture(&fixture, &expected_sha256, fixture.selected(), &snapshot)
        .expect("derive dispatch from the original Root and its included roles");

    assert_eq!(checked.capture_plan().original_sha256(), &expected_sha256);
    checked
        .revalidate()
        .expect("source snapshot remains unchanged");
    let reads = checked.source_reads().reads();
    assert_eq!(reads.len(), 2);
    assert_eq!(reads[0].identity, ROOT_INPUT);
    assert_eq!(reads[1].identity, "roles.mk");
    fixture.assert_recorder_absent();
}

#[test]
fn rejects_an_expanded_exact_competitor_from_an_included_file() {
    let fixture = Fixture::new();
    let root_source = root_source(
        "include competitor.mk",
        TARGET_PATTERN,
        "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@",
    );
    fixture.write_inputs(&root_source, &roles_source(&fixture));
    fs::write(
        fixture.root.join("competitor.mk"),
        "NAME := build-quick\n$(NAME):\n",
    )
    .expect("write included expanded exact competitor");
    let snapshot = fixture.snapshot();

    let result = capture(
        &fixture,
        &expected_sha256(&root_source),
        fixture.selected(),
        &snapshot,
    );

    assert!(
        result.is_err(),
        "included exact rule competes with the selected pattern"
    );
    fixture.assert_recorder_absent();
}

#[test]
fn rejects_an_eval_generated_matching_pattern_competitor() {
    let fixture = Fixture::new();
    let root_source = root_source(
        "$(eval build-%:)",
        TARGET_PATTERN,
        "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@",
    );
    fixture.write_inputs(&root_source, &roles_source(&fixture));
    let snapshot = fixture.snapshot();

    let result = capture(
        &fixture,
        &expected_sha256(&root_source),
        fixture.selected(),
        &snapshot,
    );

    assert!(
        result.is_err(),
        "eval-generated matching pattern rule competes with the selected rule"
    );
    fixture.assert_recorder_absent();
}

#[test]
fn rejects_relevant_target_assignments_and_a_phony_selected_goal() {
    let fixture = Fixture::new();
    let target_assignment = root_source(
        "include target-assignment.mk",
        TARGET_PATTERN,
        "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@",
    );
    fixture.write_inputs(&target_assignment, &roles_source(&fixture));
    fs::write(
        fixture.root.join("target-assignment.mk"),
        "NAME := build-quick\n$(NAME): TOOL := /selected/other-mmake\n",
    )
    .expect("write included expanded target-specific assignment");
    let target_snapshot = fixture.snapshot();
    assert!(capture(
        &fixture,
        &expected_sha256(&target_assignment),
        fixture.selected(),
        &target_snapshot,
    )
    .is_err());
    fixture.assert_recorder_absent();

    let phony_goal = root_source(
        ".PHONY: build-quick",
        TARGET_PATTERN,
        "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@",
    );
    fixture.write_inputs(&phony_goal, &roles_source(&fixture));
    let phony_snapshot = fixture.snapshot();
    assert!(capture(
        &fixture,
        &expected_sha256(&phony_goal),
        fixture.selected(),
        &phony_snapshot,
    )
    .is_err());
    fixture.assert_recorder_absent();
}

#[test]
fn rejects_selected_role_values_that_disagree_with_source_evaluation() {
    let fixture = Fixture::new();
    let root_source = valid_root_source();
    fixture.write_inputs(&root_source, &roles_source(&fixture));
    let snapshot = fixture.snapshot();
    let selected_tool = fixture
        .tool()
        .to_str()
        .expect("selected tool path is UTF-8");
    let wrong_tool = fixture.root.join("selected/not-mmake");
    let wrong_tool = wrong_tool.to_str().expect("alternate tool path is UTF-8");

    for (prefix, program, options) in [
        ("env WRONG_SENTINEL=value", selected_tool, OPTIONS),
        (ORIGINAL_PREFIX, wrong_tool, OPTIONS),
        (ORIGINAL_PREFIX, selected_tool, "--different-option"),
    ] {
        let selected = fixture.selection(prefix, program, options, TARGET_PATTERN);
        assert!(capture(
            &fixture,
            &expected_sha256(&root_source),
            selected,
            &snapshot,
        )
        .is_err());
        fixture.assert_recorder_absent();
    }
}

#[test]
fn rejects_a_nonliteral_pattern_and_a_different_selected_recipe() {
    let fixture = Fixture::new();
    let expanded_pattern = root_source(
        "PATTERN := %-quick",
        "$(PATTERN)",
        "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@",
    );
    fixture.write_inputs(&expanded_pattern, &roles_source(&fixture));
    let expanded_snapshot = fixture.snapshot();
    assert!(capture(
        &fixture,
        &expected_sha256(&expanded_pattern),
        fixture.selected(),
        &expanded_snapshot,
    )
    .is_err());
    fixture.assert_recorder_absent();

    let different_recipe = root_source(
        "",
        TARGET_PATTERN,
        "@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Other.$@",
    );
    fixture.write_inputs(&different_recipe, &roles_source(&fixture));
    let recipe_snapshot = fixture.snapshot();
    assert!(capture(
        &fixture,
        &expected_sha256(&different_recipe),
        fixture.selected(),
        &recipe_snapshot,
    )
    .is_err());
    fixture.assert_recorder_absent();
}

#[test]
fn rejects_a_root_digest_that_differs_from_the_selected_source() {
    let fixture = Fixture::new();
    let root_source = valid_root_source();
    fixture.write_inputs(&root_source, &roles_source(&fixture));
    let snapshot = fixture.snapshot();
    let wrong_sha256 = sha256_bytes(b"different Root source");

    assert!(capture(&fixture, &wrong_sha256, fixture.selected(), &snapshot,).is_err());
    fixture.assert_recorder_absent();
}

#[test]
fn rejects_snapshot_drift_before_source_interpretation() {
    let fixture = Fixture::new();
    let root_source = valid_root_source();
    fixture.write_inputs(&root_source, &roles_source(&fixture));
    let snapshot = fixture.snapshot();
    fs::write(
        fixture.root.join("roles.mk"),
        format!(
            "TOOL := {}\nTOOL_OPTIONS := changed\n",
            fixture.tool().display()
        ),
    )
    .expect("change an included source after snapshot capture");

    assert!(capture(
        &fixture,
        &expected_sha256(&root_source),
        fixture.selected(),
        &snapshot,
    )
    .is_err());
    fixture.assert_recorder_absent();
}

#[test]
fn revalidation_refuses_source_drift_after_successful_interpretation() {
    let fixture = Fixture::new();
    let root_source = valid_root_source();
    fixture.write_inputs(&root_source, &roles_source(&fixture));
    let snapshot = fixture.snapshot();
    let checked = capture(
        &fixture,
        &expected_sha256(&root_source),
        fixture.selected(),
        &snapshot,
    )
    .expect("derive dispatch before changing its captured include");
    fixture.assert_recorder_absent();

    fs::write(
        fixture.root.join("roles.mk"),
        format!(
            "TOOL := {}\nTOOL_OPTIONS := --changed\n",
            fixture.tool().display()
        ),
    )
    .expect("change an included source after derivation");

    assert!(checked.revalidate().is_err());
    fixture.assert_recorder_absent();
}
