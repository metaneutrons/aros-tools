//! Native measurement/replay consistency; not permission to launch a recipe.
use aros_common::{
    make_initial_values::{ValidatedMakeRecipeControls, MAKE_RECIPE_CONTROLS_METADATA_KEY},
    make_namespace::ValidatedMakeNamespace,
    make_native_invocation::NativeMakeInvocation,
    make_native_sampler::{
        observe_first_input_make_with_recipe_controls, InitialMakeObservationRequest,
    },
    sha256_bytes, CancellationToken,
};
use aros_transpiler::{
    dirs::DirVars,
    make_vars::{MakeProgramPaths, MakeVariableProgram},
};
use std::{fs, path::Path, process::Command, time::Duration};

#[test]
fn measured_recipe_flags_survive_native_import_and_invalid_packets_refuse() {
    let search_path = std::env::var_os("PATH").expect("test runner provides PATH");
    let program = ["gmake", "make"]
        .into_iter()
        .flat_map(|name| {
            std::env::split_paths(&search_path).map(move |directory| directory.join(name))
        })
        .filter(|path| path.is_file())
        .find(|path| {
            Command::new(path)
                .arg("--version")
                .output()
                .is_ok_and(|output| {
                    output.status.success() && output.stdout.starts_with(b"GNU Make ")
                })
        })
        .expect("test requires GNU Make")
        .canonicalize()
        .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fs::write(root.join("Root.mk"), "$(error SOURCE_MUST_NOT_BE_READ)\n").unwrap();
    let invocation = NativeMakeInvocation::new(
        ["-r", "-f", "Root.mk", "selected-quick"].map(str::to_owned),
        root.clone(),
        [],
    )
    .unwrap();
    let executable_sha256 = sha256_bytes(&fs::read(&program).unwrap());
    let context_sha256 = sha256_bytes(b"transpiler-recipe-controls-test");
    let observed = observe_first_input_make_with_recipe_controls(
        &InitialMakeObservationRequest {
            program: &program,
            executable_sha256: &executable_sha256,
            invocation: &invocation,
            context_sha256: &context_sha256,
            timeout: Duration::from_secs(20),
        },
        &CancellationToken::default(),
        |_, _| Ok(()),
    )
    .unwrap();
    let namespace = ValidatedMakeNamespace::from_make_stdout(observed.initial_stdout()).unwrap();
    let measured =
        ValidatedMakeRecipeControls::from_make_stdout(observed.initial_stdout(), &namespace)
            .unwrap();
    let flags = &measured.entries().next().unwrap().value;
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &root,
    };
    let import = |stdout: &[u8]| {
        MakeVariableProgram::from_native_invocation_observation(
            &paths,
            stdout,
            &invocation,
            observed.environment_names(),
        )
    };
    let mut replay = import(observed.initial_stdout()).unwrap();
    assert_eq!(replay.evaluate("$(.SHELLFLAGS)", &paths).unwrap(), *flags);
    let stdout = String::from_utf8(observed.initial_stdout().to_vec()).unwrap();
    let prefix = format!("{MAKE_RECIPE_CONTROLS_METADATA_KEY}=");
    let packet = stdout
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap();
    let duplicate = format!("{stdout}{packet}\n");
    assert!(import(duplicate.as_bytes()).is_err());
    let malformed = stdout.replace(packet, &format!("{prefix}v1;end"));
    assert!(import(malformed.as_bytes()).is_err());
    // The old packet remains readable, but omission does not invent flags.
    let legacy = stdout
        .lines()
        .filter(|line| !line.starts_with(&prefix))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let mut legacy = import(legacy.as_bytes()).unwrap();
    assert!(legacy.evaluate("$(.SHELLFLAGS)", &paths).is_err());
}
