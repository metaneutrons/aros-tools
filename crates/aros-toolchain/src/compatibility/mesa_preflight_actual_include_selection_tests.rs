//! Read-only diagnostic replay, never restoration of a failed live owner.
use super::*;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_native_observation::{MakeObservationBinding, RetainedMakeObservation},
};
use aros_transpiler::{
    dirs::DirVars,
    make_vars::{MakeFileEventKind, MakeProgramPaths, MakeVariableProgram},
};

#[test]
#[ignore = "requires explicit actual retained case; data-only active/inactive Source inclusion diagnostic"]
fn actual_original_include_selection_active_and_inactive_targets() {
    let case_path = PathBuf::from(std::env::var_os("AROS_TEST_MESA_PREFLIGHT_CASE").unwrap());
    let case: serde_json::Value = serde_json::from_slice(&fs::read(&case_path).unwrap()).unwrap();
    let source = PathBuf::from(case["source_root"].as_str().unwrap())
        .canonicalize()
        .unwrap();
    let compiler = PathBuf::from(case["compiler_root"].as_str().unwrap())
        .canonicalize()
        .unwrap();
    let output = PathBuf::from(case["output_root"].as_str().unwrap());
    let build = output.join("configure").canonicalize().unwrap();
    let direct = &case["native_dispatch"]["direct_collection"];
    let collection = PathBuf::from(direct["output_root"].as_str().unwrap());
    let parent = output.parent().unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(parent.join("native-direct-collection.json")).unwrap())
            .unwrap();
    let selected_source = "compiler/crt/posixc/mmakefile.src";
    let include_basename = "Makefile.posixclibrary";
    let (local_source, _) = actual_source_selection(&case, &source);
    let local_source = local_source.expect("explicit local Source selection for this diagnostic");
    let roots = [build.clone(), compiler].map(|root| MakeInputRootSelection {
        content_sha256: measure_tree_content_cas(&root)
            .unwrap()
            .payload_digest_excluding(None),
        root,
    });
    let before = measure_tree_content_cas(&build).unwrap();
    let dirs = DirVars::default();
    let mut active_site = None;
    for (goal, expected_active) in [
        ("compiler-posixc-includes", true),
        ("posixc-lfa-gen-stubs", false),
    ] {
        let slots: Vec<_> = report["coordinates"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, coordinate)| {
                coordinate["source"] == selected_source
                    && coordinate["goals"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|value| value == goal)
            })
            .collect();
        let [(slot, coordinate)] = slots.as_slice() else {
            panic!("exactly one recorded original call required");
        };
        let packet = collection
            .join("d")
            .join(format!("call-{slot:04}"))
            .join("observation.json");
        let bytes = fs::read(&packet).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let binding: MakeObservationBinding =
            serde_json::from_value(document["binding"].clone()).unwrap();
        // Hashes establish only retained-data consistency here. They are NOT
        // independent authentication or admission to reuse a failed owner.
        let observed =
            RetainedMakeObservation::load(&bytes, &sha256_bytes(&bytes), &binding).unwrap();
        assert!(observed
            .invocation()
            .arguments()
            .contains(&format!("TARGET={goal}")));
        let paths = MakeProgramPaths {
            dirs: &dirs,
            source_root: &source,
            relative_dir: Path::new(coordinate["owner"].as_str().unwrap())
                .parent()
                .unwrap(),
            top_directory: &build,
            working_directory: observed.invocation().working_directory(),
        };
        let expected_input = build
            .join(coordinate["owner"].as_str().unwrap())
            .canonicalize()
            .unwrap();
        let text = fs::read_to_string(&expected_input).unwrap();
        let sites: Vec<_> = text
            .lines()
            .enumerate()
            .filter(|(_, line)| line.trim() == "-include $(compiler-posixc-makefile)")
            // MakeFileEvent source positions use zero-based physical indices.
            .map(|(index, _)| index)
            .collect();
        let [expected_line] = sites.as_slice() else {
            panic!("exact original Source include site required");
        };
        let snapshot = MakeInputSnapshot::capture_with_local_source(
            paths.working_directory,
            &source,
            &local_source,
            &roots,
        )
        .unwrap();
        let (_, file, reads) = MakeVariableProgram::read_native_captured_inputs(
            &paths,
            observed.initial_stdout(),
            observed.invocation(),
            observed.environment_names(),
            &snapshot,
        )
        .unwrap_or_else(|reason| {
            panic!("isolated Source reader refused selected original slot {slot}: {reason}")
        });
        let includes: Vec<_> = file.events.iter().filter(|event| event.active && matches!(
            &event.kind, MakeFileEventKind::Include { requested, present: false, .. }
                if Path::new(requested).file_name() == Some(std::ffi::OsStr::new(include_basename))
        )).collect();
        if expected_active {
            let [event] = includes.as_slice() else {
                panic!("positive original call must expose exactly one active missing include");
            };
            let event_path = Path::new(&event.input);
            let event_path = if event_path.is_absolute() {
                event_path.to_owned()
            } else {
                paths.working_directory.join(event_path)
            };
            assert_eq!(event_path.canonicalize().unwrap(), expected_input);
            assert_eq!(event.source_line, *expected_line);
            assert!(
                !event.generated,
                "a generated alias must not substitute for the original site"
            );
            active_site = Some((event.input.clone(), event.source_line));
        } else {
            assert!(
                includes.is_empty(),
                "inactive original target must not select the include"
            );
            let (input, line) = active_site.as_ref().unwrap();
            assert!(!file.events.iter().any(|event| event.active
                && &event.input == input
                && &event.source_line == line
                && matches!(event.kind, MakeFileEventKind::Include { .. })));
        }
        assert!(!reads.reads().is_empty());
        eprintln!("read-only original slot {slot}: active missing include count={} expected_active={expected_active}", includes.len());
    }
    assert_eq!(
        measure_tree_content_cas(&build).unwrap(),
        before,
        "diagnostic must not write build outputs"
    );
    assert!(
        !parent.join("original-include-evidence").exists(),
        "diagnostic must not run mkdir/GENMODULE"
    );
}
