use super::*;
use crate::compatibility::environment::ResolvedProbeEnvironment;
use std::collections::BTreeMap;

struct Outputs {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    build: PathBuf,
    before: TreeContentCas,
}

impl Outputs {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temporary.path()).unwrap();
        let build = root.join("configure");
        fs::create_dir(&build).unwrap();
        fs::write(build.join("Makefile"), b"original Root\n").unwrap();
        fs::write(build.join(CONFIG), b"original configuration\n").unwrap();
        let before = measure_tree_content_cas(&root).unwrap();
        Self {
            _temporary: temporary,
            root,
            build,
            before,
        }
    }

    fn select(&self, generated: &[&str]) -> Result<Vec<String>, ContractError> {
        selected_outputs(
            &self.before,
            &self.root,
            &self.build,
            &generated
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>(),
        )
    }
}

#[test]
fn collection_write_set_permits_only_exact_new_files_and_required_parent_directories() {
    let fixture = Outputs::new();
    let selected = fixture
        .select(&["mmakefile", "libs/nested/mmakefile"])
        .unwrap();
    for path in &selected {
        let output = fixture.root.join(path);
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(output, b"new selected output\n").unwrap();
    }
    let selected: Vec<&str> = selected.iter().map(String::as_str).collect();
    let measured = measure_tree_content_cas(&fixture.root).unwrap();
    assert!(fixture
        .before
        .matches_fresh_outputs(&measured, &selected, &[]));
    assert_eq!(
        fs::read(fixture.build.join(CONFIG)).unwrap(),
        b"original configuration\n"
    );
    fs::write(
        fixture.build.join("libs/nested/unselected"),
        b"foreign output\n",
    )
    .unwrap();
    let changed = measure_tree_content_cas(&fixture.root).unwrap();
    assert!(!fixture
        .before
        .matches_fresh_outputs(&changed, &selected, &[]));
}

#[test]
fn collection_write_set_does_not_allow_mutating_original_configuration_or_retaining_tmp() {
    for mutation in ["original", "temporary"] {
        let fixture = Outputs::new();
        let selected = fixture.select(&["mmakefile"]).unwrap();
        for path in &selected {
            fs::write(fixture.root.join(path), b"selected\n").unwrap();
        }
        match mutation {
            "original" => {
                fs::write(fixture.build.join(CONFIG), b"rewritten configuration\n").unwrap();
            }
            "temporary" => {
                fs::write(fixture.build.join("mmakefiletmp"), b"unfinished GenMF\n").unwrap();
            }
            _ => unreachable!(),
        }
        let after = measure_tree_content_cas(&fixture.root).unwrap();
        let selected: Vec<&str> = selected.iter().map(String::as_str).collect();
        assert!(
            !fixture.before.matches_fresh_outputs(&after, &selected, &[]),
            "accepted {mutation}"
        );
    }
}

#[test]
fn collection_write_set_refuses_stale_outputs_and_ambiguous_or_escaping_coordinates() {
    for generated in [
        vec!["../escape"],
        vec!["/absolute"],
        vec!["./current"],
        vec![""],
        vec!["same", "same"],
        vec!["ancestor", "ancestor/child"],
        vec!["same", "sametmp"],
        vec![CONFIG],
        vec![CACHE],
        vec![FALLBACK],
    ] {
        let fixture = Outputs::new();
        assert!(
            fixture.select(&generated).is_err(),
            "accepted {generated:?}"
        );
    }
    for stale in ["mmakefile", "mmakefiletmp"] {
        let fixture = Outputs::new();
        fs::write(fixture.build.join(stale), b"unexpected existing output\n").unwrap();
        assert!(
            fixture.select(&["mmakefile"]).is_err(),
            "accepted stale {stale}"
        );
    }
}

#[test]
fn collection_home_fallback_never_uses_ambient_home_and_refuses_existing_candidates() {
    let fixture = Outputs::new();
    let mut environment = ResolvedProbeEnvironment {
        variables: BTreeMap::new(),
        host_tools: BTreeMap::new(),
    };
    check_home_fallback(&environment).unwrap();
    environment
        .variables
        .insert("HOME".into(), fixture.root.to_str().unwrap().into());
    check_home_fallback(&environment).unwrap();
    fs::write(fixture.root.join(FALLBACK), b"unselected HOME fallback\n").unwrap();
    assert!(check_home_fallback(&environment).is_err());
    environment
        .variables
        .insert("HOME".into(), "relative/home".into());
    assert!(check_home_fallback(&environment).is_err());
}

#[test]
fn collection_root_arguments_preserve_explicit_inputs_and_goals_without_dry_run() {
    let fixture = Outputs::new();
    let evidence = fixture.root.join("evidence");
    let inputs = [(fixture.build.join("Makefile"), sha256_bytes(b"Root"))];
    let absent = [fixture.build.join("Build.extra")];
    let goals = ["selected-library-quick".to_owned()];
    let arguments = root_arguments(&fixture.build, &evidence, &inputs, &absent, &goals).unwrap();
    let parsed = LiteralMakeStartupArguments::admit(&arguments).unwrap();
    assert_eq!(parsed.goals(), goals);
    assert_eq!(
        parsed.input_files(),
        [
            fixture.build.join("Makefile").to_str().unwrap(),
            evidence.join(FENCE_NAME).to_str().unwrap(),
        ]
    );
    assert!(arguments.contains(&format!("--old-file={}", absent[0].display())));
    assert!(!arguments
        .iter()
        .any(|argument| ["-n", "--dry-run"].contains(&argument.as_str())));
    assert!(root_arguments(
        &fixture.build,
        &evidence,
        &inputs,
        &absent,
        &["NAME=value".into()]
    )
    .is_err());
}

#[test]
fn collection_globals_require_original_root_bindings_before_any_generator_can_run() {
    let fixture = Outputs::new();
    let config = "[probe]\nmaketool make leaf\nglobalvarfile $(TOP)/global.cfg\ngenglobalvarfile sh configure\n";
    let project = ConfiguredProject::parse_only_project(config, "probe").unwrap();
    let path = fixture.build.join("global.cfg");
    fs::write(&path, b"original global\n").unwrap();
    let values = BTreeMap::from([
        ("TOP".into(), fixture.build.to_str().unwrap().into()),
        ("SRCDIR".into(), fixture.root.to_str().unwrap().into()),
    ]);
    let inputs = [(path.clone(), sha256_bytes(b"original global\n"))];
    check_global_inputs(&project, &values, &inputs, &fixture.build).unwrap();
    let mut missing_source = values.clone();
    missing_source.remove("SRCDIR");
    assert!(check_global_inputs(&project, &missing_source, &inputs, &fixture.build).is_err());
    assert!(check_global_inputs(&project, &values, &[], &fixture.build).is_err());
    let unqueried = ConfiguredProject::parse_only_project(
        "[probe]\nmaketool make leaf\nglobalvarfile $(UNKNOWN)/global.cfg\n",
        "probe",
    )
    .unwrap();
    assert!(check_global_inputs(&unqueried, &values, &inputs, &fixture.build).is_err());
    fs::write(&path, b"changed global\n").unwrap();
    assert!(check_global_inputs(&project, &values, &inputs, &fixture.build).is_err());
    fs::remove_file(&path).unwrap();
    assert!(check_global_inputs(&project, &values, &inputs, &fixture.build).is_err());
    assert!(!fixture.build.join("generator-ran").exists());
}

#[test]
fn collection_globals_use_project_precedence_and_sequential_global_assignments() {
    let fixture = Outputs::new();
    let first = fixture.build.join("first.cfg");
    let second = fixture.build.join("second.cfg");
    let text = format!(
        "[probe]\nmaketool make leaf\npick {}\nglobalvarfile $(pick)\nglobalvarfile $(pick)\n",
        first.display()
    );
    let project = ConfiguredProject::parse_only_project(&text, "probe").unwrap();
    let first_bytes = format!("pick := {}\n", second.display());
    fs::write(&first, first_bytes.as_bytes()).unwrap();
    fs::write(&second, b"done := yes\n").unwrap();
    let mut values = BTreeMap::from([(
        "pick".into(),
        fixture
            .build
            .join("wrong-root-fallback")
            .to_str()
            .unwrap()
            .into(),
    )]);
    values.insert("SRCDIR".into(), fixture.root.to_str().unwrap().into());
    let inputs = [
        (first, sha256_bytes(first_bytes.as_bytes())),
        (second.clone(), sha256_bytes(b"done := yes\n")),
    ];
    check_global_inputs(&project, &values, &inputs, &fixture.build).unwrap();
    assert!(check_global_inputs(&project, &values, &inputs[..1], &fixture.build).is_err());
    fs::remove_file(second).unwrap();
    assert!(check_global_inputs(&project, &values, &inputs, &fixture.build).is_err());
}

#[test]
fn collection_native_options_refuse_logging_foreign_roots_and_arbitrary_flags() {
    let fixture = Outputs::new();
    let source = fixture.root.join("source");
    let base = format!(
        "--srcdir={} --builddir={}",
        source.display(),
        fixture.build.display()
    );
    check_native_options(&base, &source, &fixture.build).unwrap();
    check_native_options(&format!("{base} -q"), &source, &fixture.build).unwrap();
    for arguments in [
        format!("{base} --logdeps"),
        format!("{base} --logfailed"),
        format!("{base} -n"),
        "--srcdir=/foreign --builddir=/foreign".into(),
        format!("{base}; false"),
        format!("{base} -q -q"),
    ] {
        assert!(
            check_native_options(&arguments, &source, &fixture.build).is_err(),
            "accepted {arguments}"
        );
    }
}

#[test]
fn collection_optional_include_fences_cannot_be_silently_omitted() {
    let fixture = Outputs::new();
    let generated = fixture.build.join("actual-selected-gen");
    let values = BTreeMap::from([("GENDIR".into(), generated.to_str().unwrap().into())]);
    let absent = [
        fixture.build.join("Build.extra"),
        generated.join("config/make.opts"),
    ];
    let selected = MesaNativeHostBuildSelection {
        output_root: &fixture.root,
        goal: "mmake",
        additional_goals: &[],
        executable_variable: "MMAKE",
        fresh_files: &[],
        fresh_directories: &[],
        absent_inputs: &absent,
    };
    check_optional_fences(&values, &selected, &fixture.build).unwrap();
    let incomplete = MesaNativeHostBuildSelection {
        absent_inputs: &absent[..1],
        ..selected
    };
    assert!(check_optional_fences(&values, &incomplete, &fixture.build).is_err());
    assert!(check_optional_fences(&BTreeMap::new(), &selected, &fixture.build).is_err());
}
