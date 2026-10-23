use super::*;
use std::time::Duration;

#[test]
#[ignore = "requires explicit retained actual config and fresh short evidence selection; data-only layout proof"]
fn actual_retained_project_layout_preflight() {
    let config = PathBuf::from(std::env::var_os("AROS_TEST_RETAINED_MMAKE_CONFIG").unwrap());
    let long = PathBuf::from(std::env::var_os("AROS_TEST_MMAKE_LONG_EVIDENCE_ROOT").unwrap());
    let short = PathBuf::from(std::env::var_os("AROS_TEST_MMAKE_SHORT_EVIDENCE_ROOT").unwrap());
    assert!(config.is_absolute() && long.is_absolute() && short.is_absolute());
    assert!(!short.exists());
    let original = fs::read(&config).unwrap();
    let digest = sha256_bytes(&original);
    let error =
        CollectionObserver::check_project_layout(&long, &original, &digest, "AROS", "$(HOST_MAKE)")
            .unwrap_err();
    assert!(error.to_string().contains("255-byte limit"));
    CollectionObserver::check_project_layout(&short, &original, &digest, "AROS", "$(HOST_MAKE)")
        .unwrap();
    assert_eq!(
        fs::read(&config).unwrap(),
        original,
        "retained failed inputs must remain unchanged"
    );
    assert!(
        !short.exists(),
        "pure preflight grants no output/alias authority"
    );
    eprintln!(
        "actual retained configuration digest: {digest}; long refused, short accepted; no writes"
    );
}

#[test]
fn project_layout_preflight_refuses_long_evidence_before_private_writes() {
    let temporary = tempfile::tempdir_in("/private/tmp").unwrap();
    let short = fs::canonicalize(temporary.path()).unwrap().join("c");
    let long = short.join("oversized-private-evidence-root-component");
    let original = b"[AROS]\nmaketool $(HOST_MAKE) $(MKARGS) TOP=$(TOP) SRCDIR=$(SRCDIR) CURDIR=$(CURDIR) TARGET=$(TARGET)\n";
    let digest = sha256_bytes(original);
    CollectionObserver::check_project_layout(&short, original, &digest, "AROS", "$(HOST_MAKE)")
        .unwrap();
    let error =
        CollectionObserver::check_project_layout(&long, original, &digest, "AROS", "$(HOST_MAKE)")
            .unwrap_err();
    assert!(error.to_string().contains("255-byte limit"));
    assert!(
        !short.exists(),
        "layout preflight must create no evidence or alias"
    );
    assert!(!long.exists());
    let control = sha256_bytes(b"actual-control-different-from-layout-stand-in");
    let real_launcher = collection_launcher(&short, &control);
    aros_transpiler::metamake_project::MetaMakeLauncherConfiguration::derive_confined_project(
        original,
        &digest,
        "AROS",
        "$(HOST_MAKE)",
        &real_launcher,
    )
    .unwrap();
    assert!(CollectionObserver::check_project_layout(
        &short,
        original,
        &sha256_bytes(b"wrong"),
        "AROS",
        "$(HOST_MAKE)"
    )
    .is_err());
    assert!(!short.exists());
}

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    observer: PathBuf,
    program: PathBuf,
    digest: Sha256Digest,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(temporary.path()).unwrap();
        let root = base.join("collection");
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let observer = base.join("observer");
        let program = base.join("make");
        let bytes = b"#!/bin/sh\nexit 0\n";
        for path in [&observer, &program] {
            fs::write(path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self {
            _temporary: temporary,
            root,
            observer,
            program,
            digest: sha256_bytes(bytes),
        }
    }

    fn prepare(&self) -> CollectionObserver {
        CollectionObserver::prepare(
            &self.root,
            &self.selection(),
            &serde_json::json!({"selected_source": "fixture-only"}),
        )
        .unwrap()
    }

    fn selection(&self) -> CollectionObserverSelection<'_> {
        CollectionObserverSelection {
            observer: &self.observer,
            observer_sha256: &self.digest,
            program: &self.program,
            program_sha256: &self.digest,
            argument_zero: "gmake",
            maximum_calls: 3,
        }
    }
}

#[test]
fn collection_capacity_versions_and_aggregate_memory_remain_bounded() {
    for (capacity, version) in [(256, 4), (257, 5), (551, 5), (4096, 5)] {
        let fixture = Fixture::new();
        let mut selected = fixture.selection();
        selected.maximum_calls = capacity;
        let observer =
            CollectionObserver::prepare(&fixture.root, &selected, &serde_json::json!({})).unwrap();
        observer.revalidate().unwrap();
        let control: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.root.join("c")).unwrap()).unwrap();
        assert_eq!(control["version"], version);
        assert_eq!(control["maximum_calls"], capacity);
    }
    for capacity in [0, 4097] {
        let fixture = Fixture::new();
        let mut selected = fixture.selection();
        selected.maximum_calls = capacity;
        assert!(
            CollectionObserver::prepare(&fixture.root, &selected, &serde_json::json!({})).is_err()
        );
        assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 0);
    }
    let limit = aros_common::make_native_collection::MAX_NATIVE_MAKE_COLLECTION_BYTES;
    let mut bytes = limit - 1;
    charge_collection_bytes(&mut bytes, 1).unwrap();
    assert!(charge_collection_bytes(&mut bytes, 1).is_err());
    assert_eq!(bytes, limit);
    assert!(charge_collection_bytes(&mut bytes, usize::MAX).is_err());
}

#[test]
fn observer_collection_controls_bind_selected_inputs_without_running_programs() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare();
    prepared.revalidate().unwrap();
    assert_eq!(
        fs::read_link(fixture.root.join("o")).unwrap(),
        fixture.observer
    );
    assert!(fs::read_dir(fixture.root.join("d"))
        .unwrap()
        .next()
        .is_none());
    let control_bytes = fs::read(fixture.root.join("c")).unwrap();
    let context_bytes = fs::read(fixture.root.join("native-context.json")).unwrap();
    assert_eq!(&sha256_bytes(&control_bytes), prepared.control_sha256());
    assert_eq!(&sha256_bytes(&context_bytes), prepared.context_sha256());
    let control: serde_json::Value = serde_json::from_slice(&control_bytes).unwrap();
    assert_eq!(control["version"], 4);
    assert_eq!(control["maximum_calls"], 3);
    assert_eq!(control["argument_zero"], "gmake");
    assert_eq!(control["program"], fixture.program.to_str().unwrap());
    assert_eq!(
        prepared.launcher(),
        format!(
            "{} --observe-make-launch-collection {} {} --",
            fixture.root.join("o").display(),
            fixture.root.join("c").display(),
            prepared.control_sha256()
        )
    );
}

#[test]
fn observer_collection_rejects_changed_controls_alias_and_executables() {
    for changed in [
        "c",
        "native-context.json",
        "alias",
        "observer",
        "program",
        "mode",
        "nonexecutable",
    ] {
        let fixture = Fixture::new();
        let prepared = fixture.prepare();
        match changed {
            "alias" => {
                fs::remove_file(fixture.root.join("o")).unwrap();
                std::os::unix::fs::symlink(&fixture.program, fixture.root.join("o")).unwrap();
            }
            "observer" => fs::write(&fixture.observer, b"changed observer").unwrap(),
            "program" => fs::write(&fixture.program, b"changed program").unwrap(),
            "mode" => {
                fs::set_permissions(&fixture.program, fs::Permissions::from_mode(0o755)).unwrap();
            }
            "nonexecutable" => {
                fs::set_permissions(&fixture.observer, fs::Permissions::from_mode(0o600)).unwrap();
            }
            name => fs::write(fixture.root.join(name), b"changed control").unwrap(),
        }
        assert!(
            prepared.revalidate().is_err(),
            "accepted mutation: {changed}"
        );
    }
}

#[test]
fn observer_collection_refuses_ambiguous_launcher_paths_before_writing_controls() {
    for name in [
        "with space",
        "with;semicolon",
        "with$expansion",
        "with'quote",
        "with\\escape",
    ] {
        let fixture = Fixture::new();
        let root = fixture.root.join(name);
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        assert!(
            CollectionObserver::prepare(&root, &fixture.selection(), &serde_json::json!({}))
                .is_err(),
            "accepted unsupported path {name}"
        );
        assert!(fs::read_dir(&root).unwrap().next().is_none());
    }
}

#[test]
fn observer_collection_refuses_nonexecutable_selection_before_writing_controls() {
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.observer, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(CollectionObserver::prepare(
        &fixture.root,
        &fixture.selection(),
        &serde_json::json!({})
    )
    .is_err());
    assert!(fs::read_dir(&fixture.root).unwrap().next().is_none());
}

#[test]
fn observer_collection_cannot_treat_empty_cancelled_or_expired_capture_as_success() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare();
    let cancellation = CancellationToken::default();
    assert!(prepared
        .read_after_reaped(
            &fixture.root,
            Instant::now() + Duration::from_secs(5),
            &cancellation
        )
        .is_err());
    assert!(prepared
        .read_after_reaped(&fixture.root, Instant::now(), &cancellation)
        .is_err());
    cancellation.cancel();
    assert!(prepared
        .read_after_reaped(
            &fixture.root,
            Instant::now() + Duration::from_secs(5),
            &cancellation
        )
        .is_err());
}

#[test]
fn observer_slot_names_require_complete_contiguous_bounded_pairs() {
    let names = |values: &[&str]| values.iter().map(OsString::from).collect();
    assert_eq!(
        selected_observed_slots(
            names(&[
                "call-0001.claim",
                "call-0000",
                "call-0001",
                "call-0000.claim"
            ]),
            3
        )
        .unwrap(),
        vec![0, 1]
    );
    for values in [
        vec![],
        vec!["call-0000"],
        vec!["call-0000", "call-0001.claim"],
        vec!["call-0001", "call-0001.claim"],
        vec!["call-0003", "call-0003.claim"],
        vec!["call-0", "call-0.claim"],
        vec!["call-0000", "call-0000"],
        vec![
            "call-0000",
            "call-0000.claim",
            "unexpected",
            "unexpected.claim",
        ],
    ] {
        assert!(
            selected_observed_slots(names(&values), 3).is_err(),
            "accepted {values:?}"
        );
    }
}

#[test]
fn observer_collection_reads_real_first_input_sampler_data_without_running_leaf_input() {
    for capacity in [3, 257] {
        real_sampler_readback_fixture(capacity, false, false);
    }
}

#[test]
fn recipe_control_collection_requires_its_selected_actual_packet() {
    real_sampler_readback_fixture(3, true, true);
    real_sampler_readback_fixture(3, true, false);
}

fn real_sampler_readback_fixture(capacity: usize, selected_controls: bool, sampled_controls: bool) {
    use aros_common::make_native_collection::native_make_call_completion;
    use aros_common::make_native_invocation::NativeMakeInvocation;
    use aros_common::make_native_sampler::{
        observe_first_input_make, observe_first_input_make_with_recipe_controls,
        InitialMakeObservationRequest, NativeMakeObservationStage,
    };

    let fixture = Fixture::new();
    let make =
        fs::canonicalize(which::which("gmake").expect("GNU Make fixture dependency")).unwrap();
    let (_, measured) = measure_regular_file_digest_bounded(&make, MAX_EXECUTABLE_BYTES).unwrap();
    let selection = CollectionObserverSelection {
        program: &make,
        program_sha256: &measured.digest,
        maximum_calls: capacity,
        ..fixture.selection()
    };
    let source_context = serde_json::json!({"selected_source": "real-sampler-fixture-only"});
    let prepared = if selected_controls {
        CollectionObserver::prepare_with_recipe_controls(&fixture.root, &selection, &source_context)
    } else {
        CollectionObserver::prepare(&fixture.root, &selection, &source_context)
    }
    .unwrap();
    let control: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.root.join("c")).unwrap()).unwrap();
    assert_eq!(
        control["version"],
        if selected_controls {
            6
        } else if capacity > 256 {
            5
        } else {
            4
        }
    );
    let context: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.root.join("native-context.json")).unwrap())
            .unwrap();
    assert_eq!(
        context["recipe_controls"],
        if selected_controls {
            serde_json::json!(true)
        } else {
            serde_json::Value::Null
        }
    );
    let build = fixture.root.parent().unwrap().join("build");
    fs::create_dir(&build).unwrap();
    fs::write(
        build.join("leaf.mk"),
        b"$(file >leaf-input-ran,unexpected input execution)\nprobe:\n\t@touch leaf-recipe-ran\n",
    )
    .unwrap();
    let invocation = NativeMakeInvocation::new(
        ["-r", "-f", "leaf.mk", "probe"].map(str::to_owned),
        build.clone(),
        [(
            "PATH".to_owned(),
            format!("{}:/usr/bin:/bin", make.parent().unwrap().display()),
        )],
    )
    .unwrap()
    .with_argument_zero("gmake".to_owned())
    .unwrap();
    let (slot, call) = prepared
        .collection
        .claim_observation_slot_bounded(capacity)
        .unwrap();
    call.write_new(
        OsStr::new("invocation.json"),
        &invocation.to_bytes().unwrap(),
    )
    .unwrap();
    let cancellation = CancellationToken::default();
    let request = InitialMakeObservationRequest {
        program: &make,
        executable_sha256: &measured.digest,
        invocation: &invocation,
        context_sha256: prepared.context_sha256(),
        timeout: Duration::from_secs(10),
    };
    let retain = |stage, output: &aros_common::ProcessOutput| {
        if matches!(stage, NativeMakeObservationStage::Initial) {
            let mut stdout = Vec::new();
            output
                .stdout
                .write_rendered(&mut stdout)
                .map_err(|error| error.to_string())?;
            call.write_new(OsStr::new("initial.stdout"), &stdout)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    };
    let observation = if sampled_controls {
        observe_first_input_make_with_recipe_controls(&request, &cancellation, retain)
    } else {
        observe_first_input_make(&request, &cancellation, retain)
    }
    .unwrap();
    call.write_new(
        OsStr::new("observation.json"),
        &observation.to_bytes().unwrap(),
    )
    .unwrap();
    call.write_new(
        OsStr::new("call-completed.json"),
        &native_make_call_completion(
            slot,
            prepared.control_sha256(),
            MakeObservationPhase::FirstInputPrelude,
            prepared.context_sha256(),
        )
        .unwrap(),
    )
    .unwrap();
    let count = if capacity > 256 { capacity } else { 1 };
    let mut last_path = call.path().to_owned();
    // Repeated packets are a bounded readback fixture, not a claim that257
    // native Source calls or leaf producers ran. One real initial sampler
    // supplied these packet bytes under the same explicitly selected context.
    for _ in 1..count {
        let (slot, extra) = prepared
            .collection
            .claim_observation_slot_bounded(capacity)
            .unwrap();
        for (name, bytes) in [
            ("invocation.json", invocation.to_bytes().unwrap()),
            ("initial.stdout", observation.initial_stdout().to_vec()),
            ("observation.json", observation.to_bytes().unwrap()),
            (
                "call-completed.json",
                native_make_call_completion(
                    slot,
                    prepared.control_sha256(),
                    MakeObservationPhase::FirstInputPrelude,
                    prepared.context_sha256(),
                )
                .unwrap(),
            ),
        ] {
            extra.write_new(OsStr::new(name), &bytes).unwrap();
        }
        last_path = extra.path().to_owned();
    }
    let retained = prepared.read_after_reaped(
        &build,
        Instant::now() + Duration::from_secs(10),
        &cancellation,
    );
    if selected_controls && !sampled_controls {
        let Err(error) = retained else {
            panic!("legacy packet must not supply selected recipe controls");
        };
        assert!(error.to_string().contains("recipe controls"));
        assert!(!build.join("leaf-input-ran").exists());
        assert!(!build.join("leaf-recipe-ran").exists());
        return;
    }
    let retained = retained.unwrap();
    assert_eq!(retained.len(), count);
    assert_eq!(retained[0].binding(), observation.binding());
    assert_eq!(retained[0].invocation().arguments(), invocation.arguments());
    assert!(!build.join("leaf-input-ran").exists());
    assert!(!build.join("leaf-recipe-ran").exists());

    fs::write(last_path.join("initial.stdout"), b"changed sampler bytes").unwrap();
    assert!(prepared
        .read_after_reaped(
            &build,
            Instant::now() + Duration::from_secs(10),
            &cancellation
        )
        .is_err());
}
