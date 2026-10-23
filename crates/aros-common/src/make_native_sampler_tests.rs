use super::*;
use crate::make_initial_values::{ValidatedMakeRecipeControls, MAKE_RECIPE_CONTROLS_METADATA_KEY};

fn gnu_make() -> PathBuf {
    for name in ["gmake", "make"] {
        let Ok(path) = which::which(name) else {
            continue;
        };
        let path = path.canonicalize().unwrap();
        let version = Command::new(&path).arg("--version").output().unwrap();
        if version.status.success() && version.stdout.starts_with(b"GNU Make ") {
            return path;
        }
    }
    panic!("native sampler tests require GNU Make");
}

#[test]
#[ignore = "requires explicit retained invocation/start and fresh probe root; initial-state data only"]
fn retained_first_input_call_data_probe() {
    let explicit = |name| {
        PathBuf::from(std::env::var_os(name).expect("select explicit retained probe inputs"))
    };
    let invocation_path = explicit("AROS_TEST_RETAINED_MAKE_CALL");
    let start_path = explicit("AROS_TEST_RETAINED_MAKE_START");
    let output = explicit("AROS_TEST_RETAINED_MAKE_PROBE_ROOT");
    assert!(invocation_path.is_absolute() && start_path.is_absolute() && output.is_absolute());
    let invocation_bytes = fs::read(&invocation_path).unwrap();
    assert!(invocation_bytes.len() <= 2 * 1024 * 1024);
    let invocation = NativeMakeInvocation::parse(&invocation_bytes).unwrap();
    let start: serde_json::Value = serde_json::from_slice(&fs::read(&start_path).unwrap()).unwrap();
    assert_eq!(start["phase"], "first-input-prelude");
    let program = PathBuf::from(start["program"].as_str().unwrap());
    let executable_sha256 =
        Sha256Digest::parse(start["executable_sha256"].as_str().unwrap()).unwrap();
    let context_sha256 = Sha256Digest::parse(start["context_sha256"].as_str().unwrap()).unwrap();
    fs::create_dir(&output).unwrap();
    fs::set_permissions(&output, fs::Permissions::from_mode(0o700)).unwrap();
    let request = InitialMakeObservationRequest {
        program: &program,
        executable_sha256: &executable_sha256,
        invocation: &invocation,
        context_sha256: &context_sha256,
        timeout: Duration::from_secs(40),
    };
    let result =
        observe_first_input_make(&request, &CancellationToken::default(), |stage, data| {
            let label = match stage {
                NativeMakeObservationStage::Discovery => "discovery",
                NativeMakeObservationStage::Initial => "initial",
            };
            fs::write(
                output.join(format!("{label}.stdout")),
                data.stdout.exact_bytes().ok_or("probe stdout truncated")?,
            )
            .map_err(|_| "cannot retain probe stdout")?;
            fs::write(
                output.join(format!("{label}.stderr")),
                data.stderr.exact_bytes().ok_or("probe stderr truncated")?,
            )
            .map_err(|_| "cannot retain probe stderr")?;
            Ok(())
        });
    let observation = match result {
        Ok(value) => value,
        Err(error) => {
            fs::write(output.join("refusal.txt"), error).unwrap();
            panic!("retained initial-state probe refused; inspect private evidence");
        }
    };
    assert_eq!(
        observation.invocation().to_bytes().unwrap(),
        invocation_bytes
    );
    fs::write(
        output.join("observation.json"),
        observation.to_bytes().unwrap(),
    )
    .unwrap();
    fs::write(
        output.join("scope.json"),
        serde_json::to_vec(&serde_json::json!({
            "qualification":"new-initial-state-probe-of-retained-data-not-original-owner-or-sdk",
            "invocation":sha256_bytes(&invocation_bytes), "binding":observation.binding(),
            "program":program,"program_sha256":executable_sha256,
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn inherited_no_print_directory_is_preserved_before_source_and_does_not_admit_eval() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fs::write(root.join("mmakefile"), "$(error SOURCE_MUST_NOT_BE_READ)\n").unwrap();
    let program = gnu_make();
    let executable_sha256 = sha256_bytes(&fs::read(&program).unwrap());
    let context_sha256 = sha256_bytes(b"inherited-display-only-option");
    let environment = BTreeMap::from([
        ("MAKEFLAGS".into(), "rs --no-print-directory".into()),
        ("MFLAGS".into(), "-rs --no-print-directory".into()),
        ("MAKELEVEL".into(), "1".into()),
        ("_".into(), "retained-parent-program".into()),
    ]);
    let arguments = [
        "--no-print-directory",
        "TOP=/explicit/build",
        "SRCDIR=/explicit/source",
        "CURDIR=component",
        "TARGET=selected-quick",
        "-s",
        "--file=mmakefile",
        "selected-quick",
    ]
    .map(str::to_owned);
    let invocation =
        NativeMakeInvocation::new(arguments.clone(), root.clone(), environment.clone())
            .unwrap()
            .with_argument_zero("gmake".into())
            .unwrap();
    let request = InitialMakeObservationRequest {
        program: &program,
        executable_sha256: &executable_sha256,
        invocation: &invocation,
        context_sha256: &context_sha256,
        timeout: Duration::from_secs(20),
    };
    let mut stages = 0;
    let observation =
        observe_first_input_make(&request, &CancellationToken::default(), |_, output| {
            stages += 1;
            assert!(
                !String::from_utf8_lossy(output.stderr.exact_bytes().unwrap())
                    .contains("SOURCE_MUST_NOT_BE_READ")
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(stages, 2);
    assert_eq!(observation.invocation().environment(), &environment);
    assert_eq!(observation.invocation().arguments(), arguments);
    let mut counter = environment;
    counter.insert(
        "MAKEFLAGS".into(),
        "rs --no-print-directory --eval=$(error FORBIDDEN_EVAL)".into(),
    );
    let invocation = NativeMakeInvocation::new(arguments, root, counter).unwrap();
    let request = InitialMakeObservationRequest {
        invocation: &invocation,
        ..request
    };
    stages = 0;
    assert!(
        observe_first_input_make(&request, &CancellationToken::default(), |_, _| {
            stages += 1;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(stages, 0);
}

#[test]
fn opt_in_recipe_controls_match_native_defaults_without_reading_source() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    // Reading this Source input, let alone running a recipe, must fail the test.
    fs::write(root.join("Root.mk"), "$(error SOURCE_MUST_NOT_BE_READ)\n").unwrap();
    let program = gnu_make();
    let program_sha256 = sha256_bytes(&fs::read(&program).unwrap());
    let context_sha256 = sha256_bytes(b"recipe-control-native-test");
    let invocation = NativeMakeInvocation::new(
        ["-r", "-f", "Root.mk", "selected-quick"].map(str::to_owned),
        root.clone(),
        [],
    )
    .unwrap();
    let request = InitialMakeObservationRequest {
        program: &program,
        executable_sha256: &program_sha256,
        invocation: &invocation,
        context_sha256: &context_sha256,
        timeout: Duration::from_secs(20),
    };
    let observed = observe_first_input_make_with_recipe_controls(
        &request,
        &CancellationToken::default(),
        |_, _| Ok(()),
    )
    .unwrap();
    let namespace = ValidatedMakeNamespace::from_make_stdout(observed.initial_stdout()).unwrap();
    let controls =
        ValidatedMakeRecipeControls::from_make_stdout(observed.initial_stdout(), &namespace)
            .unwrap();
    let value = controls.entries().next().unwrap();
    assert_eq!(value.name, ".SHELLFLAGS");
    assert_eq!(controls.entries().count(), 1);

    // Independently query the selected native executable, not a guessed -c.
    let output = Command::new(&program).env_clear().current_dir(&root)
        .args(["-r", "-f", "/dev/null", "--eval",
            "$(info FLAGS=$(value .SHELLFLAGS))\n$(info ORIGIN=$(origin .SHELLFLAGS))\n.PHONY: probe\nprobe: ;", "probe"])
        .output().unwrap();
    assert!(output.status.success());
    let native = String::from_utf8(output.stdout).unwrap();
    assert!(native
        .lines()
        .any(|line| line == format!("FLAGS={}", value.value)));
    assert!(native.lines().any(|line| line == "ORIGIN=default"));
    assert_eq!(value.origin, MakeVariableOrigin::Default);

    let legacy =
        observe_first_input_make(&request, &CancellationToken::default(), |_, _| Ok(())).unwrap();
    assert!(!String::from_utf8_lossy(legacy.initial_stdout())
        .lines()
        .any(|line| line.starts_with(MAKE_RECIPE_CONTROLS_METADATA_KEY)));
}

#[test]
fn recipe_control_helper_collisions_refuse_before_any_metadata_or_source() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fs::write(root.join("Root.mk"), "$(error SOURCE_MUST_NOT_BE_READ)\n").unwrap();
    let program = gnu_make();
    let program_sha256 = sha256_bytes(&fs::read(&program).unwrap());
    let context_sha256 = sha256_bytes(b"recipe-control-collision-test");
    for helper in ["__AROS_RECIPE_CONTROLS_ITEM", "__AROS_RECIPE_CONTROLS_NL"] {
        let invocation = NativeMakeInvocation::new(
            ["-r", "-f", "Root.mk", "selected-quick"].map(str::to_owned),
            root.clone(),
            [(helper.to_owned(), "occupied".to_owned())],
        )
        .unwrap();
        let request = InitialMakeObservationRequest {
            program: &program,
            executable_sha256: &program_sha256,
            invocation: &invocation,
            context_sha256: &context_sha256,
            timeout: Duration::from_secs(20),
        };
        let mut retained = Vec::new();
        let result = observe_first_input_make_with_recipe_controls(
            &request,
            &CancellationToken::default(),
            |_, output| {
                retained.push((
                    output.stdout.exact_bytes().unwrap().to_vec(),
                    output.stderr.exact_bytes().unwrap().to_vec(),
                ));
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(retained.len(), 1);
        assert!(retained[0].0.is_empty());
        let diagnostic = String::from_utf8_lossy(&retained[0].1);
        assert!(diagnostic.contains("collision"));
        assert!(!diagnostic.contains("SOURCE_MUST_NOT_BE_READ"));
    }
}
