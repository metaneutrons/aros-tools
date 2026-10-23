//! Synthetic native startup evidence, not arbitrary Make execution authority.

use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Command, Output},
};

use aros_common::{
    make_initial_values::ValidatedMakeCommandLineValues,
    make_namespace::ValidatedMakeNamespace,
    make_observer::{
        command_line_values_fragment, source_prelude_fragment, validate_source_prelude_inputs,
    },
    make_startup_arguments::LiteralMakeStartupArguments,
};

const fn make() -> &'static str {
    if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    }
}

fn environment() -> BTreeMap<String, String> {
    BTreeMap::from([("PATH".into(), std::env::var("PATH").unwrap())])
}

fn raw_output(arguments: &[String], cwd: &Path) -> Output {
    Command::new(make())
        .args(arguments)
        .current_dir(cwd)
        .env_clear()
        .envs(environment())
        .output()
        .unwrap()
}

fn admitted_output(arguments: &[String], cwd: &Path, starts: &mut usize) -> Result<Output, ()> {
    let admitted = LiteralMakeStartupArguments::admit(arguments).map_err(|_| ())?;
    validate_source_prelude_inputs(&environment(), &BTreeMap::new()).map_err(|_| ())?;
    *starts += 1;
    Ok(raw_output(admitted.arguments(), cwd))
}

fn base() -> Vec<String> {
    [
        "-r", "-n", "-s", "-f", "input.mk", "-o", "input.mk", "probe",
    ]
    .map(str::to_owned)
    .to_vec()
}

#[test]
fn literal_startup_names_bind_native_raw_observation_without_assignment_emulation() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("input.mk"), ".PHONY: probe\nprobe:\n").unwrap();
    let mut original = base();
    original.extend([
        "CUSTOM=first".into(),
        "CUSTOM=last literal ;|% value".into(),
    ]);
    let original_admission = LiteralMakeStartupArguments::admit(&original).unwrap();
    let observer = format!(
        "{}{}",
        source_prelude_fragment(),
        command_line_values_fragment(original_admission.command_line_names()).unwrap()
    );
    fs::write(directory.path().join("observer.mk"), observer).unwrap();
    let mut instrumented = ["-f", "observer.mk", "-o", "observer.mk"]
        .map(str::to_owned)
        .to_vec();
    instrumented.extend(original);
    let mut starts = 0;
    let output = admitted_output(&instrumented, directory.path(), &mut starts).unwrap();
    assert_eq!(starts, 1);
    assert!(output.status.success(), "closed synthetic startup failed");
    let namespace = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
    let values = ValidatedMakeCommandLineValues::from_make_stdout(
        &output.stdout,
        &namespace,
        original_admission.command_line_names(),
    )
    .unwrap();
    let record = values.entries().next().unwrap();
    assert_eq!(record.name, "CUSTOM");
    assert_eq!(record.value, "last literal ;|% value");
}

#[test]
fn startup_effects_refuse_before_launch_and_native_counterprobes_do_have_effects() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("input.mk"), ".PHONY: probe\nprobe:\n").unwrap();
    let mut starts = 0;
    for (index, immediate) in [false, true].into_iter().enumerate() {
        let name = format!("isolated-startup-effect-{index}");
        let marker = directory.path().join(&name);
        // All effect text below is fixed synthetic data, never supplied by the
        // source or operator. Writes stay within this disposable fixture cwd.
        let effect = format!("$(file >{name},synthetic-effect)");
        let argument = if immediate {
            format!("CUSTOM:={effect}")
        } else {
            format!("--eval={effect}")
        };
        let mut arguments = base();
        arguments.push(argument);
        assert!(admitted_output(&arguments, directory.path(), &mut starts).is_err());
        assert_eq!(starts, 0);
        assert!(!marker.exists());
        // Positive counter for the measurement: GNU really performs the
        // isolated write before our file observer, even under -n. Do not infer
        // arbitrary-source launch safety from dry-run or startup admission.
        let output = raw_output(&arguments, directory.path());
        assert!(
            output.status.success(),
            "synthetic native effect counter failed"
        );
        assert_eq!(fs::read(&marker).unwrap(), b"synthetic-effect\n");
    }
}
