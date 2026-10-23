use super::*;
use aros_common::{
    make_initial_values::{
        MAKE_COMMAND_LINE_VALUES_METADATA_KEY, MAKE_ENVIRONMENT_VALUES_METADATA_KEY,
        MAKE_INITIAL_VALUES_METADATA_KEY, MAKE_PARSER_CONTROLS_METADATA_KEY,
    },
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    sha256_bytes,
};

struct ObservationFixture {
    fixture: Fixture,
    invocation_bytes: Vec<u8>,
    initial_stdout: Vec<u8>,
    environment_names: BTreeSet<String>,
    binding: MakeObservationBinding,
}

impl ObservationFixture {
    fn new(environment: &BTreeMap<String, String>) -> Self {
        let fixture = Fixture::new();
        let invocation = invocation(&fixture.working_directory, environment);
        let (initial_stdout, environment_names) = pre_file_observation(&invocation);
        let compact_invocation = require(
            invocation.to_bytes(),
            "could not encode synthetic native invocation",
        );
        let mut invocation_bytes = b" \n".to_vec();
        invocation_bytes.extend_from_slice(&compact_invocation);
        invocation_bytes.extend_from_slice(b"\t");
        let binding = MakeObservationBinding {
            phase: MakeObservationPhase::PreFirstFile,
            context_sha256: sha256_bytes(b"synthetic independent child context"),
            invocation_sha256: sha256_bytes(&invocation_bytes),
            stdout_sha256: sha256_bytes(&initial_stdout),
        };
        Self {
            fixture,
            invocation_bytes,
            initial_stdout,
            environment_names,
            binding,
        }
    }

    fn observation(&self) -> RetainedMakeObservation {
        require(
            RetainedMakeObservation::from_captured_bytes(
                &self.invocation_bytes,
                &self.initial_stdout,
                &self.environment_names,
                &self.binding,
            ),
            "synthetic retained observation was rejected",
        )
    }
}

fn assert_observation_error<T>(
    result: Result<T, String>,
    expected: &str,
    diagnostic: &'static str,
) {
    let Err(error) = result else {
        panic!("{diagnostic}")
    };
    assert!(error == expected, "{diagnostic}");
}

fn document_value(bytes: &[u8]) -> serde_json::Value {
    require(
        serde_json::from_slice(bytes),
        "synthetic retained observation document was not valid JSON",
    )
}

fn document_bytes(value: &serde_json::Value) -> Vec<u8> {
    require(
        serde_json::to_vec(value),
        "could not encode synthetic retained observation mutation",
    )
}

fn without_packet(stdout: &[u8], key: &str) -> Vec<u8> {
    let marker = format!("{key}=");
    let marker = marker.as_bytes();
    let mut retained = Vec::with_capacity(stdout.len());
    let mut removed = 0usize;
    for line in stdout.split_inclusive(|byte| *byte == b'\n') {
        let content = line.strip_suffix(b"\n").unwrap_or(line);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        if content.starts_with(marker) {
            removed += 1;
        } else {
            retained.extend_from_slice(line);
        }
    }
    assert!(removed == 1, "synthetic packet was absent or duplicated");
    retained
}

#[test]
fn retained_observation_round_trips_exact_bytes_and_reads_captured_source() {
    let packet = ObservationFixture::new(&synthetic_environment());
    let observation = packet.observation();
    let document = require(
        observation.to_bytes(),
        "could not serialize synthetic retained observation",
    );
    let document_sha256 = sha256_bytes(&document);
    let loaded = require(
        RetainedMakeObservation::load(&document, &document_sha256, &packet.binding),
        "could not load synthetic retained observation",
    );

    let raw_invocation = require(
        std::str::from_utf8(&packet.invocation_bytes),
        "synthetic invocation bytes were not UTF-8",
    );
    let loaded_document = document_value(&require(
        loaded.to_bytes(),
        "could not re-serialize loaded synthetic observation",
    ));
    assert!(
        loaded_document
            .get("invocation")
            .and_then(serde_json::Value::as_str)
            == Some(raw_invocation),
        "portable round trip normalized retained invocation bytes"
    );
    assert!(
        loaded.initial_stdout() == packet.initial_stdout.as_slice(),
        "portable round trip changed retained stdout bytes"
    );
    assert!(
        loaded.environment_names() == &packet.environment_names,
        "portable round trip changed retained environment membership"
    );
    let parsed_invocation = require(
        NativeMakeInvocation::parse(&packet.invocation_bytes),
        "synthetic raw invocation could not be parsed",
    );
    assert!(
        loaded.invocation() == &parsed_invocation,
        "loaded invocation differed from its retained raw bytes"
    );

    let debug = format!("{loaded:?}");
    let raw_stdout = require(
        std::str::from_utf8(&packet.initial_stdout),
        "synthetic stdout bytes were not UTF-8",
    );
    assert!(
        !debug.contains("environment-value")
            && !debug.contains("command-value")
            && !debug.contains("SYNTH_ENV")
            && !debug.contains("CLI_VALUE")
            && !debug.contains(raw_invocation)
            && !debug.contains(raw_stdout)
            && !debug.contains(packet.fixture.working_directory.to_string_lossy().as_ref()),
        "retained observation Debug exposed private captured values"
    );

    // Either label can be selected over the same packet; this checks metadata
    // consistency only and establishes no execution phase.
    let mut first_input_binding = packet.binding.clone();
    first_input_binding.phase = MakeObservationPhase::FirstInputPrelude;
    let first_input_observation = require(
        RetainedMakeObservation::from_captured_bytes(
            &packet.invocation_bytes,
            &packet.initial_stdout,
            &packet.environment_names,
            &first_input_binding,
        ),
        "externally selected first-input phase label was rejected",
    );
    assert!(
        first_input_observation.binding() == &first_input_binding,
        "retained observation did not preserve the externally selected phase label"
    );

    let dirs = DirVars::default();
    let relative_directory = Path::new("one");
    let paths = packet
        .fixture
        .paths(&dirs, relative_directory, &packet.fixture.working_directory);
    let roots = ["root".to_owned()];
    let snapshot = packet.fixture.snapshot();
    let matched = require(
        packet.fixture.projection.read_retained_child_source(
            &paths,
            &roots,
            "one/mmakefile",
            &loaded,
            &snapshot,
        ),
        "retained observation could not be read through native source adapter",
    );
    let (_, mut program, _, reads) = matched.into_parts();
    assert!(
        reads.reads().len() == 2
            && reads.reads()[0].requested == "mmakefile"
            && reads.reads()[1].requested == "extra.mk",
        "retained observation changed captured explicit input or include order"
    );
    let (values, evaluation_reads) = require(
        program.evaluate_captured(
            "$(SIMPLE)|$(RECURSIVE)|$(INCLUDED)|$(CLI_VALUE)|$(SYNTH_ENV)",
            &paths,
            &snapshot,
        ),
        "retained captured source values could not be evaluated",
    );
    assert!(
        values == "before|after|include-value|command-value|environment-value",
        "retained adapter changed captured source values"
    );
    assert!(
        evaluation_reads.reads().is_empty(),
        "retained scalar evaluation unexpectedly performed captured reads"
    );
}

#[test]
fn retained_document_digest_and_external_binding_select_exact_payload() {
    let packet = ObservationFixture::new(&synthetic_environment());
    let observation = packet.observation();
    let document = require(
        observation.to_bytes(),
        "could not serialize synthetic retained observation",
    );
    let document_sha256 = sha256_bytes(&document);

    assert_observation_error(
        RetainedMakeObservation::load(
            &document,
            &sha256_bytes(b"different selected document"),
            &packet.binding,
        ),
        "retained native observation document differs from selection",
        "wrong external document digest was accepted",
    );

    let mut changed = packet.binding.clone();
    changed.phase = MakeObservationPhase::FirstInputPrelude;
    assert_observation_error(
        RetainedMakeObservation::load(&document, &document_sha256, &changed),
        "retained native observation binding differs from selection",
        "changed external phase selection was accepted",
    );
    let mut changed = packet.binding.clone();
    changed.context_sha256 = sha256_bytes(b"different selected context");
    assert_observation_error(
        RetainedMakeObservation::load(&document, &document_sha256, &changed),
        "retained native observation binding differs from selection",
        "changed external context digest was accepted",
    );
    let mut changed = packet.binding.clone();
    changed.invocation_sha256 = sha256_bytes(b"different selected invocation");
    assert_observation_error(
        RetainedMakeObservation::load(&document, &document_sha256, &changed),
        "retained native observation binding differs from selection",
        "changed external invocation digest was accepted",
    );
    let mut changed = packet.binding.clone();
    changed.stdout_sha256 = sha256_bytes(b"different selected stdout");
    assert_observation_error(
        RetainedMakeObservation::load(&document, &document_sha256, &changed),
        "retained native observation binding differs from selection",
        "changed external stdout digest was accepted",
    );

    for field in ["invocation", "initial_stdout"] {
        let mut swapped = document_value(&document);
        let object = swapped
            .as_object_mut()
            .unwrap_or_else(|| panic!("synthetic document was not an object"));
        object.insert(
            field.to_owned(),
            serde_json::Value::String("swapped".into()),
        );
        let swapped = document_bytes(&swapped);
        let swapped_sha256 = sha256_bytes(&swapped);
        assert_observation_error(
            RetainedMakeObservation::load(&swapped, &swapped_sha256, &packet.binding),
            "retained native observation payload differs from selection",
            "rehashed document replaced a payload without its external digest",
        );
    }
}

#[test]
fn retained_loader_rejects_malformed_membership_metadata_and_oversized_inputs() {
    let packet = ObservationFixture::new(&synthetic_environment());
    let observation = packet.observation();
    let document = require(
        observation.to_bytes(),
        "could not serialize synthetic retained observation",
    );

    let mut duplicate = document_value(&document);
    let names = duplicate
        .get_mut("environment_names")
        .and_then(serde_json::Value::as_array_mut)
        .unwrap_or_else(|| panic!("synthetic environment membership was not an array"));
    let first = names
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("synthetic environment membership was empty"));
    names.push(first);
    let duplicate = document_bytes(&duplicate);
    assert_observation_error(
        RetainedMakeObservation::load(&duplicate, &sha256_bytes(&duplicate), &packet.binding),
        "retained native observation repeats environment membership",
        "duplicate environment membership was accepted",
    );

    let mut unknown = document_value(&document);
    unknown
        .as_object_mut()
        .unwrap_or_else(|| panic!("synthetic document was not an object"))
        .insert("unexpected".into(), serde_json::Value::Null);
    let unknown = document_bytes(&unknown);
    assert_observation_error(
        RetainedMakeObservation::load(&unknown, &sha256_bytes(&unknown), &packet.binding),
        "retained native observation document is malformed",
        "unknown retained document field was accepted",
    );

    let mut unsupported_version = document_value(&document);
    unsupported_version
        .as_object_mut()
        .unwrap_or_else(|| panic!("synthetic document was not an object"))
        .insert("version".into(), serde_json::Value::from(2));
    let unsupported_version = document_bytes(&unsupported_version);
    assert_observation_error(
        RetainedMakeObservation::load(
            &unsupported_version,
            &sha256_bytes(&unsupported_version),
            &packet.binding,
        ),
        "retained native observation binding differs from selection",
        "unsupported retained document version was accepted",
    );

    let malformed = b"{";
    assert_observation_error(
        RetainedMakeObservation::load(malformed, &sha256_bytes(malformed), &packet.binding),
        "retained native observation document is malformed",
        "malformed retained document was accepted",
    );

    let mut unknown_phase = document_value(&document);
    unknown_phase["binding"]["phase"] = serde_json::Value::String("observer-free".into());
    let unknown_phase = document_bytes(&unknown_phase);
    assert_observation_error(
        RetainedMakeObservation::load(
            &unknown_phase,
            &sha256_bytes(&unknown_phase),
            &packet.binding,
        ),
        "retained native observation document is malformed",
        "unknown observation phase was accepted",
    );

    let unsupported_names = BTreeSet::from(["x".repeat(129)]);
    assert_observation_error(
        RetainedMakeObservation::from_captured_bytes(
            &packet.invocation_bytes,
            &packet.initial_stdout,
            &unsupported_names,
            &packet.binding,
        ),
        "native observation environment membership is unsupported",
        "oversized environment name was copied into a retained packet",
    );

    let oversized_document = vec![b' '; 4 * 1024 * 1024 + 1];
    assert_observation_error(
        RetainedMakeObservation::load(
            &oversized_document,
            &sha256_bytes(&oversized_document),
            &packet.binding,
        ),
        "retained native observation document differs from selection",
        "oversized retained document was accepted",
    );
    let too_many_names: BTreeSet<_> = (0..4097).map(|index| format!("NAME_{index}")).collect();
    assert_observation_error(
        RetainedMakeObservation::from_captured_bytes(
            &packet.invocation_bytes,
            &packet.initial_stdout,
            &too_many_names,
            &packet.binding,
        ),
        "native observation exceeds retained data bounds",
        "environment count above its bound was copied into a retained packet",
    );
    let mut excessive_names = document_value(&document);
    excessive_names["environment_names"] = serde_json::json!(too_many_names);
    let excessive_names = document_bytes(&excessive_names);
    assert_observation_error(
        RetainedMakeObservation::load(
            &excessive_names,
            &sha256_bytes(&excessive_names),
            &packet.binding,
        ),
        "native observation exceeds retained data bounds",
        "environment count above its bound was loaded from a retained packet",
    );
    let oversized_invocation = vec![b' '; 2 * 1024 * 1024 + 1];
    assert_observation_error(
        RetainedMakeObservation::from_captured_bytes(
            &oversized_invocation,
            &packet.initial_stdout,
            &packet.environment_names,
            &packet.binding,
        ),
        "native observation exceeds retained data bounds",
        "oversized invocation input was accepted",
    );
    let oversized_stdout = vec![b' '; 256 * 1024 + 1];
    assert_observation_error(
        RetainedMakeObservation::from_captured_bytes(
            &packet.invocation_bytes,
            &oversized_stdout,
            &packet.environment_names,
            &packet.binding,
        ),
        "native observation exceeds retained data bounds",
        "oversized stdout input was accepted",
    );
}

#[test]
fn retained_packets_require_all_initial_values_and_admitted_startup_controls() {
    let packet = ObservationFixture::new(&synthetic_environment());
    for (key, expected, diagnostic) in [
        (
            MAKE_INITIAL_VALUES_METADATA_KEY,
            "retained native observation default packet is invalid",
            "missing default values packet was accepted",
        ),
        (
            MAKE_PARSER_CONTROLS_METADATA_KEY,
            "retained native observation control packet is invalid",
            "missing parser controls packet was accepted",
        ),
        (
            MAKE_COMMAND_LINE_VALUES_METADATA_KEY,
            "retained native observation CLI packet is invalid",
            "missing command-line values packet was accepted",
        ),
        (
            MAKE_ENVIRONMENT_VALUES_METADATA_KEY,
            "retained native observation environment packet is invalid",
            "missing environment values packet was accepted",
        ),
    ] {
        let initial_stdout = without_packet(&packet.initial_stdout, key);
        let mut binding = packet.binding.clone();
        binding.stdout_sha256 = sha256_bytes(&initial_stdout);
        assert_observation_error(
            RetainedMakeObservation::from_captured_bytes(
                &packet.invocation_bytes,
                &initial_stdout,
                &packet.environment_names,
                &binding,
            ),
            expected,
            diagnostic,
        );
    }

    let mut invocation_document = document_value(&packet.invocation_bytes);
    let arguments = invocation_document
        .get_mut("arguments")
        .and_then(serde_json::Value::as_array_mut)
        .unwrap_or_else(|| panic!("synthetic invocation arguments were not an array"));
    arguments.push(serde_json::Value::String("--eval=unmodeled".into()));
    let invocation_bytes = document_bytes(&invocation_document);
    let mut binding = packet.binding.clone();
    binding.invocation_sha256 = sha256_bytes(&invocation_bytes);
    assert_observation_error(
        RetainedMakeObservation::from_captured_bytes(
            &invocation_bytes,
            &packet.initial_stdout,
            &packet.environment_names,
            &binding,
        ),
        "retained native observation startup arguments are unsupported",
        "unmodeled startup option was accepted",
    );

    let mut environment = synthetic_environment();
    // Only this fixture's inert assignment-only include is preloaded. This
    // deliberately bypasses the launch gate to test the retained-data refusal;
    // it is not permission to launch an arbitrary inherited MAKEFILES input.
    environment.insert("MAKEFILES".into(), "extra.mk".into());
    let inherited_makefiles = ObservationFixture::new(&environment);
    assert_observation_error(
        RetainedMakeObservation::from_captured_bytes(
            &inherited_makefiles.invocation_bytes,
            &inherited_makefiles.initial_stdout,
            &inherited_makefiles.environment_names,
            &inherited_makefiles.binding,
        ),
        "retained native observation inherited controls are unsupported",
        "nonempty inherited MAKEFILES control was accepted",
    );
}
