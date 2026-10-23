//! Source-selected transport fragment, not a complete MetaMake launch proof.

use super::*;
use aros_common::{
    make_child_command::LiteralMakeChildCommand,
    make_initial_values::{MakeInitialValueFlavor, ValidatedMakeCommandLineValues},
    make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation},
    make_observer::{
        command_line_values_fragment, environment_values_fragment, source_prelude_fragment,
        validate_source_prelude_inputs,
    },
    make_startup_arguments::LiteralMakeStartupArguments,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    process::Command,
};

fn namespace(
    environment: &BTreeMap<String, String>,
    command_names: &BTreeSet<String>,
) -> ValidatedMakeNamespace {
    let mut entries = [".VARIABLES", "MAKE_VERSION"]
        .map(|name| MakeNamespaceEntry {
            name: name.into(),
            origin: MakeVariableOrigin::Default,
        })
        .to_vec();
    entries.extend(environment.keys().map(|name| MakeNamespaceEntry {
        name: name.clone(),
        origin: MakeVariableOrigin::Environment,
    }));
    for name in command_names {
        entries.push(MakeNamespaceEntry {
            name: name.clone(),
            origin: MakeVariableOrigin::CommandLine,
        });
    }
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries,
    })
    .unwrap()
}

fn observed(output: &std::process::Output, prefix: &str) -> String {
    assert!(
        output.status.success(),
        "private native transport fixture failed"
    );
    let text = std::str::from_utf8(&output.stdout).unwrap();
    let lines = text
        .lines()
        .filter_map(|line| line.strip_prefix(prefix))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    lines[0].into()
}

fn origins(program: &MakeVariableProgram, names: &BTreeSet<String>) -> String {
    let label = |name: &str| match program.state.origins.get(name) {
        Some(MakeVariableOrigin::Environment) => "environment",
        Some(MakeVariableOrigin::File) => "file",
        Some(MakeVariableOrigin::CommandLine) => "command line",
        _ => panic!("unexpected selected variable origin"),
    };
    let records = names
        .iter()
        .map(|name| format!("{name}={};", label(name)))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{records}CLI={}", label("CLI_ONLY"))
}

fn observed_initial(
    value: &aros_common::make_initial_values::MakeInitialValueObservation,
) -> MakeInitialValue<'_> {
    MakeInitialValue {
        name: value.name.as_str(),
        value: value.value.as_str(),
        flavor: match value.flavor {
            MakeInitialValueFlavor::Simple => MakeInitialFlavor::Simple,
            MakeInitialValueFlavor::Recursive => MakeInitialFlavor::Recursive,
        },
        origin: value.origin,
    }
}

#[test]
fn actual_source_prefix_transports_environment_origin_not_file_origin() {
    let selected_source = crate::testing::mesa26_root();
    let configuration =
        aros_common::text::read_source(&selected_source.join("config/make.cfg.in")).unwrap();
    let joined = join_continuations(&configuration);
    let assignments = joined
        .lines()
        .filter(|line| {
            line.split_once(":=")
                .is_some_and(|(name, _)| name.trim() == "CALL")
        })
        .collect::<Vec<_>>();
    assert_eq!(assignments.len(), 1);
    let assignment = assignments[0];
    let references = regex::Regex::new(r"\$\(([A-Z_][A-Z_0-9]*)\)").unwrap();
    let names = references
        .captures_iter(assignment)
        .map(|capture| capture[1].to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        references.find_iter(assignment).count(),
        assignment.matches("$(").count()
    );
    assert!(names.len() > 1);
    // The names come from the selected Source; these are deliberately synthetic
    // data values, not inferred CPU/ABI facts or a production variable table.
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &root,
    };
    let mut source_values = String::new();
    for (index, name) in names.iter().enumerate() {
        writeln!(source_values, "{name} := selected{index}").unwrap();
    }
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let parent_source = format!(
        "{source_values}{assignment}\nRUN := $(CALL) {make} -r -n -s --no-print-directory -f child.mk 'CLI_ONLY=command value' probe\nprobe:\n\t@$(RUN)\n"
    );
    let inherited = BTreeMap::from([
        ("PATH".into(), std::env::var("PATH").unwrap()),
        ("LC_ALL".into(), "C".into()),
    ]);
    let ns = namespace(&inherited, &BTreeSet::new());
    let parent_initial = inherited
        .iter()
        .map(|(name, value)| MakeInitialValue {
            name,
            value,
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        })
        .collect::<Vec<_>>();
    let mut parent = MakeVariableProgram::new_source_program(&paths, &ns, &parent_initial).unwrap();
    parent
        .read_file("parent.mk", &parent_source, &paths, &|_, _| Ok(None))
        .unwrap();
    let literal = parent.evaluate("$(RUN)", &paths).unwrap();
    let command = LiteralMakeChildCommand::parse(&literal).unwrap();
    let startup = LiteralMakeStartupArguments::admit(command.arguments()).unwrap();
    assert_eq!(command.program(), make);
    assert_eq!(
        command
            .environment_assignments()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        names
    );
    let parent_exports = parent.source_export_values(&paths).unwrap();
    assert_eq!(parent_exports, inherited);
    let child_environment = command.apply_environment(&parent_exports).unwrap();
    let first = names.first().unwrap();
    let mut child_source = String::new();
    for name in &names {
        writeln!(child_source, "{name} ?= wrong-default").unwrap();
    }
    let name_list = names.iter().cloned().collect::<Vec<_>>().join(" ");
    write!(child_source, "{first} := file-wins\nCLI_ONLY := wrong-file-value\nRESULT := $(foreach NAME,{name_list},$(NAME)=$($(NAME));)CLI=$(CLI_ONLY)\nprobe:\n\t$(info OBSERVED=$(RESULT))\n\t$(info ORIGINS=$(foreach NAME,{name_list},$(NAME)=$(origin $(NAME));)CLI=$(origin CLI_ONLY))\n").unwrap();
    std::fs::write(root.join("parent.mk"), &parent_source).unwrap();
    std::fs::write(root.join("child.mk"), &child_source).unwrap();
    let native_parent = Command::new(make)
        .current_dir(&root)
        .env_clear()
        .envs(&inherited)
        .args([
            "-r",
            "-s",
            "--no-print-directory",
            "-f",
            "parent.mk",
            "probe",
        ])
        .output()
        .unwrap();
    let expected = observed(&native_parent, "OBSERVED=");
    let expected_origins = observed(&native_parent, "ORIGINS=");
    let command_names = startup.command_line_names();
    let prelude = format!(
        "{}{}",
        source_prelude_fragment(),
        command_line_values_fragment(command_names).unwrap()
    );
    let observer = root.join("child-observer.mk");
    std::fs::write(&observer, prelude).unwrap();
    // Startup admission already checks each literal CLI control occurrence;
    // this additionally checks the exact inherited environment, not ambient env.
    validate_source_prelude_inputs(&child_environment, &BTreeMap::new()).unwrap();
    let mut observed_arguments = vec![
        "-f".into(),
        observer.to_str().unwrap().into(),
        "-o".into(),
        observer.to_str().unwrap().into(),
        "-o".into(),
        "child.mk".into(),
    ];
    observed_arguments.extend_from_slice(startup.arguments());
    let observed_startup = LiteralMakeStartupArguments::admit(&observed_arguments).unwrap();
    assert_eq!(observed_startup.command_line_names(), command_names);
    let direct = Command::new(command.program())
        .current_dir(&root)
        .env_clear()
        .envs(&child_environment)
        .args(observed_startup.arguments())
        .output()
        .unwrap();
    assert_eq!(observed(&direct, "OBSERVED="), expected);
    assert_eq!(observed(&direct, "ORIGINS="), expected_origins);
    // Only source-selected values/origins are compared here. Automatic native
    // MAKELEVEL/MAKEFLAGS, executable resolution and full inherited context
    // remain separate required invocation/receipt binding work.
    // This packet belongs to the explicit pre-selected-file observer invocation,
    // not an assertion that its added file/old-file flags were absent upstream.
    let child_namespace = ValidatedMakeNamespace::from_make_stdout(&direct.stdout).unwrap();
    let environment_names = child_namespace
        .to_observation()
        .entries
        .into_iter()
        .filter(|entry| entry.origin == MakeVariableOrigin::Environment)
        .map(|entry| entry.name)
        .collect::<BTreeSet<_>>();
    std::fs::write(
        &observer,
        format!(
            "{}{}{}",
            source_prelude_fragment(),
            command_line_values_fragment(command_names).unwrap(),
            environment_values_fragment(&environment_names).unwrap()
        ),
    )
    .unwrap();
    let measured = Command::new(command.program())
        .current_dir(&root)
        .env_clear()
        .envs(&child_environment)
        .args(observed_startup.arguments())
        .output()
        .unwrap();
    assert_eq!(observed(&measured, "OBSERVED="), expected);
    assert_eq!(observed(&measured, "ORIGINS="), expected_origins);
    assert_eq!(
        ValidatedMakeNamespace::from_make_stdout(&measured.stdout).unwrap(),
        child_namespace
    );
    let command_values = ValidatedMakeCommandLineValues::from_make_stdout(
        &measured.stdout,
        &child_namespace,
        command_names,
    )
    .unwrap();
    let command_initial = command_values
        .entries()
        .map(observed_initial)
        .collect::<Vec<_>>();
    let original_environment_names = child_environment.keys().cloned().collect();
    let mut child = MakeVariableProgram::from_native_source_observation(
        &paths,
        &measured.stdout,
        &observed_startup,
        &original_environment_names,
        &environment_names,
    )
    .unwrap();
    child
        .read_file("child.mk", &child_source, &paths, &|_, _| Ok(None))
        .unwrap();
    assert_eq!(child.evaluate("$(RESULT)", &paths).unwrap(), expected);
    assert!(expected.contains(&format!("{first}=file-wins;")));
    assert!(expected.contains("CLI=command value"));
    assert_eq!(origins(&child, &names), expected_origins);
    let wrong_environment = BTreeMap::new();
    let wrong_namespace = namespace(&wrong_environment, command_names);
    let mut wrong =
        MakeVariableProgram::new_source_program(&paths, &wrong_namespace, &command_initial)
            .unwrap();
    let mut file_imports = String::new();
    for (name, value) in command.environment_assignments() {
        writeln!(file_imports, "{name} := {value}").unwrap();
    }
    wrong
        .read_file(
            "child.mk",
            &format!("{file_imports}{child_source}"),
            &paths,
            &|_, _| Ok(None),
        )
        .unwrap();
    assert_eq!(
        wrong.evaluate("$(RESULT)", &paths).unwrap(),
        expected,
        "equal values alone must not prove native origin parity"
    );
    assert_ne!(
        origins(&wrong, &names),
        expected_origins,
        "file-origin reconstruction must not impersonate native environment transport"
    );
}
