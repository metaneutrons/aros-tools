use super::*;
use std::fs;

fn write_configuration_fixture(build: &Path, cache: &str) {
    fs::create_dir_all(build.join("CMakeFiles")).unwrap();
    fs::write(build.join("CMakeCache.txt"), cache).unwrap();
    fs::write(build.join("build.ninja"), b"fixture build rules").unwrap();
    fs::write(build.join("CMakeFiles/rules.ninja"), b"fixture rules").unwrap();
}

fn identity() -> LocalNativeInputs {
    let digest = aros_common::sha256_bytes(b"fixture identity");
    LocalNativeInputs {
        schema: "aros-local-native-inputs-v1".into(),
        source: LocalSourceIdentity {
            schema: "aros-local-source-v1".into(),
            head_baseline: "a".repeat(40),
            submodules_sha256: digest.clone(),
            generated_subtree: "build/fixture".into(),
            content_sha256: digest.clone(),
            entry_count: 1,
            regular_file_bytes: 7,
        },
        native_contract_sha256: digest.clone(),
        toolchain_descriptor_sha256: digest.clone(),
        toolchain_tree_sha256: digest.clone(),
        engine_payload_sha256: digest.clone(),
        executable_sha256: BTreeMap::from([("aros".into(), digest)]),
        executor_paths: BTreeMap::new(),
        environment_sha256: BTreeMap::new(),
        configure_arguments: vec!["-DAROS_TARGET_PROFILE=fixture".into()],
    }
}

#[test]
fn local_native_stamp_is_prebuild_binding_and_refuses_mixed_inputs() {
    let temp = tempfile::tempdir().unwrap();
    let original = identity();
    original.bind_build_tree(temp.path()).unwrap();
    let path = temp.path().join(STAMP_NAME);
    let bytes = fs::read(&path).unwrap();
    original.bind_build_tree(temp.path()).unwrap();
    let mut changed = original.clone();
    changed.source.content_sha256 = aros_common::sha256_bytes(b"dirty source");
    assert!(changed.bind_build_tree(temp.path()).is_err());
    assert!(original.require_unchanged(&changed).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    for field in ["release_id", "build_succeeded"] {
        let mut json = serde_json::to_value(&original).unwrap();
        json[field] = true.into();
        assert!(serde_json::from_value::<LocalNativeInputs>(json).is_err());
    }
}

#[test]
fn local_native_stamp_refuses_unbound_existing_build_and_unsafe_metadata() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("CMakeCache.txt"), b"historical build").unwrap();
    assert!(identity().bind_build_tree(temp.path()).is_err());
    assert!(!temp.path().join(STAMP_NAME).exists());
    fs::remove_file(temp.path().join("CMakeCache.txt")).unwrap();
    let path = temp.path().join(STAMP_NAME);
    for bytes in [
        b"invalid JSON".as_slice(),
        &vec![b'x'; MAX_STAMP_BYTES as usize + 1],
    ] {
        fs::write(&path, bytes).unwrap();
        assert!(identity().bind_build_tree(temp.path()).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(identity().bind_build_tree(temp.path()).is_err());
}

#[cfg(unix)]
#[test]
fn local_native_stamp_refuses_symlink_without_mutating_target() {
    let temp = tempfile::tempdir().unwrap();
    let sentinel = temp.path().join("sentinel");
    fs::write(&sentinel, b"must remain unchanged").unwrap();
    std::os::unix::fs::symlink(&sentinel, temp.path().join(STAMP_NAME)).unwrap();
    assert!(identity().bind_build_tree(temp.path()).is_err());
    assert_eq!(fs::read(sentinel).unwrap(), b"must remain unchanged");
}

#[cfg(unix)]
#[test]
fn local_native_consumer_stamp_binds_reloaded_contract_and_inputs() {
    use std::process::Command;

    let (toolchain_root, _, _) = crate::build::tests::local_compiler_variables_fixture();
    let (source_root, profile, _) = crate::build::tests::native_consumer_source_fixture();
    let selected = NativeContractSelection::load(source_root.path(), &profile)
        .unwrap()
        .unwrap();

    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(source_root.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "user.email", "fixture@example.invalid"]);
    git(&[
        "add",
        "--",
        "aros-targets.toml",
        "consumer.json",
        "policy.json",
    ]);
    git(&["commit", "-m", "fixture source"]);

    let engine = tempfile::tempdir().unwrap();
    fs::write(engine.path().join("AROS.cmake"), b"fixture engine").unwrap();
    let tools = tempfile::tempdir().unwrap();
    for name in TOOL_NAMES {
        fs::write(tools.path().join(name), format!("fixture {name}")).unwrap();
    }
    let executable = std::env::current_exe().unwrap();
    let mut configure = Command::new(&executable);
    configure
        .arg("-S")
        .arg(engine.path())
        .arg("-B")
        .arg(source_root.path().join("build/fixture"))
        .arg(format!("-DCMAKE_MAKE_PROGRAM={}", executable.display()))
        .arg(format!(
            "-DAROS_COMPILER_CACHE_EXECUTABLE={}",
            executable.display()
        ));
    for (key, value) in selected.cmake_variables().unwrap() {
        configure.arg(format!("-D{key}={value}"));
    }

    let build_tree = source_root.path().join("build/fixture");
    fs::create_dir_all(&build_tree).unwrap();
    let identity = LocalNativeInputs::capture(&LocalNativeCapture {
        root: source_root.path(),
        preset: "fixture",
        profile: &profile,
        toolchain_root: toolchain_root.path(),
        engine: engine.path(),
        tools: tools.path(),
        configure: &configure,
        selected_contract: &selected,
    })
    .unwrap();
    assert_eq!(identity.native_contract_sha256, *selected.sha256());
    assert!(identity
        .configure_arguments
        .iter()
        .any(|argument| { argument.starts_with("-DAROS_NATIVE_CONSUMER_CONTRACT_SHA256=") }));
    identity.bind_build_tree(&build_tree).unwrap();
    let stamp_path = build_tree.join(STAMP_NAME);
    let stamp_before = fs::read(&stamp_path).unwrap();

    fs::write(source_root.path().join("policy.json"), b"changed policy").unwrap();
    assert!(LocalNativeInputs::capture(&LocalNativeCapture {
        root: source_root.path(),
        preset: "fixture",
        profile: &profile,
        toolchain_root: toolchain_root.path(),
        engine: engine.path(),
        tools: tools.path(),
        configure: &configure,
        selected_contract: &selected,
    })
    .is_err());
    assert_eq!(fs::read(stamp_path).unwrap(), stamp_before);
}

#[test]
fn local_native_configuration_refuses_cache_and_rule_tampering() {
    let temp = tempfile::tempdir().unwrap();
    verify_configured_tree(temp.path(), false).unwrap();
    fs::create_dir(temp.path().join("CMakeFiles")).unwrap();
    for name in CONFIGURATION_FILES {
        fs::write(temp.path().join(name), name.as_bytes()).unwrap();
    }
    assert!(verify_configured_tree(temp.path(), false).is_err());
    verify_configured_tree(temp.path(), true).unwrap();
    verify_configured_tree(temp.path(), false).unwrap();
    let stamp = fs::read(temp.path().join(CONFIGURATION_STAMP)).unwrap();
    for name in CONFIGURATION_FILES {
        fs::write(temp.path().join(name), b"tampered configuration").unwrap();
        assert!(verify_configured_tree(temp.path(), false).is_err());
        assert!(verify_configured_tree(temp.path(), true).is_err());
        assert_eq!(
            fs::read(temp.path().join(CONFIGURATION_STAMP)).unwrap(),
            stamp
        );
        fs::write(temp.path().join(name), name.as_bytes()).unwrap();
    }
}

#[test]
fn local_native_configuration_binds_literal_ninja_include_closure() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("CMakeFiles")).unwrap();
    fs::write(temp.path().join("CMakeCache.txt"), b"fixture cache").unwrap();
    fs::write(temp.path().join("CMakeFiles/rules.ninja"), b"fixture rules").unwrap();
    fs::write(
        temp.path().join("build.ninja"),
        b"include CMakeFiles/impl-Release.ninja\n",
    )
    .unwrap();
    let included = temp.path().join("CMakeFiles/impl-Release.ninja");
    fs::write(&included, b"subninja\tCMakeFiles/no-suffix\n").unwrap();
    fs::write(
        temp.path().join("CMakeFiles/no-suffix"),
        b"include CMakeFiles/rules.ninja\n",
    )
    .unwrap();
    let first = ConfiguredTree::capture(temp.path()).unwrap();
    assert!(first.files.contains_key("CMakeFiles/impl-Release.ninja"));
    assert!(first.files.contains_key("CMakeFiles/no-suffix"));
    verify_configured_tree(temp.path(), true).unwrap();
    fs::write(&included, b"changed command edge").unwrap();
    assert!(verify_configured_tree(temp.path(), false).is_err());
    for text in [
        "include ../outside.ninja\n",
        "include $dynamic\n",
        "include /outside.ninja\n",
    ] {
        fs::write(temp.path().join("build.ninja"), text).unwrap();
        assert!(ConfiguredTree::capture(temp.path()).is_err());
    }
    assert_eq!(
        literal_ninja_path("CMakeFiles/with$ space.ninja").unwrap(),
        "CMakeFiles/with space.ninja"
    );
}

#[test]
fn local_native_configuration_binds_discovered_external_program_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let build = temp.path().join("build");
    fs::create_dir_all(build.join("CMakeFiles")).unwrap();
    for name in CONFIGURATION_FILES {
        fs::write(build.join(name), b"fixture configuration").unwrap();
    }
    let program = temp.path().join("host-compiler");
    fs::write(&program, b"original external compiler").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(
        build.join("CMakeCache.txt"),
        format!("AROS_HOST_CC:STRING={}\n", program.display()),
    )
    .unwrap();
    verify_configured_tree(&build, true).unwrap();
    let first = ConfiguredTree::capture(&build).unwrap();
    assert!(first.programs.contains_key("AROS_HOST_CC"));
    fs::write(&program, b"replaced compiler at identical path").unwrap();
    assert!(verify_configured_tree(&build, false).is_err());
    fs::write(
        build.join("CMakeCache.txt"),
        format!("AROS_HOST_CC:STRING={}/../host-compiler\n", build.display()),
    )
    .unwrap();
    assert!(ConfiguredTree::capture(&build).is_err());
}

#[cfg(unix)]
#[test]
fn local_native_configuration_resolves_build_program_symlink_before_exclusion() {
    use std::os::unix::fs::{symlink, PermissionsExt as _};

    let temp = tempfile::tempdir().unwrap();
    let build = temp.path().join("build");
    fs::create_dir_all(build.join("CMakeFiles")).unwrap();
    let external_tools = temp.path().join("external-tools");
    fs::create_dir(&external_tools).unwrap();
    let compiler = external_tools.join("compiler");
    fs::write(&compiler, b"external compiler bytes").unwrap();
    fs::set_permissions(&compiler, fs::Permissions::from_mode(0o755)).unwrap();
    symlink(&external_tools, build.join("host-tools")).unwrap();
    for name in CONFIGURATION_FILES {
        fs::write(build.join(name), b"fixture configuration").unwrap();
    }
    let candidate = build.join("host-tools/compiler");
    fs::write(
        build.join("CMakeCache.txt"),
        format!("AROS_HOST_CC:FILEPATH={}\n", candidate.display()),
    )
    .unwrap();

    verify_configured_tree(&build, true).unwrap();
    let captured = ConfiguredTree::capture(&build).unwrap();
    assert_eq!(
        captured.programs["AROS_HOST_CC"].resolved_path,
        compiler.canonicalize().unwrap().to_str().unwrap()
    );
    fs::write(&compiler, b"mutated external compiler").unwrap();
    assert!(verify_configured_tree(&build, false).is_err());
}

#[cfg(unix)]
#[test]
fn local_native_configuration_excludes_only_missing_generated_programs() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let build = temp.path().join("build");
    fs::create_dir_all(build.join("CMakeFiles")).unwrap();
    for name in CONFIGURATION_FILES {
        fs::write(build.join(name), b"fixture configuration").unwrap();
    }
    let generated = build.join("generated/compiler");
    fs::write(
        build.join("CMakeCache.txt"),
        format!("CMAKE_C_COMPILER:FILEPATH={}\n", generated.display()),
    )
    .unwrap();
    let captured = ConfiguredTree::capture(&build).unwrap();
    assert!(!captured.programs.contains_key("CMAKE_C_COMPILER"));

    let external = temp.path().join("external-tools");
    fs::create_dir(&external).unwrap();
    symlink(&external, build.join("external-tools")).unwrap();
    fs::write(
        build.join("CMakeCache.txt"),
        format!(
            "CMAKE_C_COMPILER:FILEPATH={}\n",
            build.join("external-tools/compiler").display()
        ),
    )
    .unwrap();
    assert!(ConfiguredTree::capture(&build).is_err());
}

#[test]
fn local_native_configuration_binds_only_configured_board_input_members() {
    let temp = tempfile::tempdir().unwrap();
    let build = temp.path().join("build");
    let external = temp.path().join("board-inputs");
    let objects = external.join("kobjs");
    fs::create_dir_all(&objects).unwrap();
    let dtb = external.join("bcm2711-rpi-4-b.dtb");
    fs::write(&dtb, b"device tree bytes").unwrap();
    for name in KOBJ_MEMBERS {
        fs::write(objects.join(name), format!("{name} contents")).unwrap();
    }
    let unrelated = objects.join("unconsumed.o");
    fs::write(&unrelated, b"not consumed by these CMake edges").unwrap();
    write_configuration_fixture(
        &build,
        &format!(
            "AROS_RPI_DTB:FILEPATH={}\nAROS_RPI_CORE_KOBJ_DIR:PATH={}\nOTHER_SEARCH_PATH:PATH={}\n",
            dtb.display(),
            objects.display(),
            external.display(),
        ),
    );

    let original = ConfiguredTree::capture(&build).unwrap();
    assert!(original.board_file_inputs.contains_key("AROS_RPI_DTB"));
    let kobj = &original.board_object_directories["AROS_RPI_CORE_KOBJ_DIR"];
    assert!(kobj.is_directory);
    assert_eq!(kobj.members.len(), KOBJ_MEMBERS.len());
    assert!(!original
        .board_object_directories
        .contains_key("OTHER_SEARCH_PATH"));

    fs::write(
        &unrelated,
        b"sibling changes do not widen the closed input set",
    )
    .unwrap();
    assert!(original == ConfiguredTree::capture(&build).unwrap());

    fs::write(&dtb, b"changed device tree bytes").unwrap();
    assert!(original != ConfiguredTree::capture(&build).unwrap());
    fs::write(&dtb, b"device tree bytes").unwrap();
    fs::write(objects.join("task_resource.o"), b"changed terminal object").unwrap();
    assert!(original != ConfiguredTree::capture(&build).unwrap());
}

#[test]
fn local_native_configuration_binds_configured_board_inputs_inside_build() {
    let temp = tempfile::tempdir().unwrap();
    let build = temp.path().join("build");
    let dtb = build.join("board-inputs/bcm2711-rpi-4-b.dtb");
    let objects = build.join("board-inputs/kobjs");
    fs::create_dir_all(&objects).unwrap();
    fs::write(&dtb, b"configured in-build device tree").unwrap();
    for name in KOBJ_MEMBERS {
        fs::write(objects.join(name), format!("in-build {name} contents")).unwrap();
    }
    write_configuration_fixture(
        &build,
        &format!(
            "AROS_RPI_DTB:FILEPATH={}\nAROS_RPI_CORE_KOBJ_DIR:PATH={}\n",
            dtb.display(),
            objects.display(),
        ),
    );

    verify_configured_tree(&build, true).unwrap();
    let original = ConfiguredTree::capture(&build).unwrap();
    assert!(original.board_file_inputs.contains_key("AROS_RPI_DTB"));
    assert!(original.board_object_directories["AROS_RPI_CORE_KOBJ_DIR"]
        .members
        .values()
        .all(|member| member.internal_to_build && member.sha256.is_some()));

    fs::write(&dtb, b"mutated in-build device tree").unwrap();
    assert!(verify_configured_tree(&build, false).is_err());
    fs::write(&dtb, b"configured in-build device tree").unwrap();
    verify_configured_tree(&build, false).unwrap();

    fs::write(objects.join("task_resource.o"), b"mutated in-build KOBJ").unwrap();
    assert!(verify_configured_tree(&build, false).is_err());
}

#[test]
#[ignore = "requires explicitly installed CMake and Ninja; configuration admission only"]
fn local_native_configuration_accepts_actual_cmake_ninja_and_refuses_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let build = temp.path().join("build");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("CMakeLists.txt"), b"cmake_minimum_required(VERSION 3.20)\nproject(ConfigurationIdentity NONE)\nadd_custom_target(probe ALL COMMAND \"${CMAKE_COMMAND}\" -E touch \"${CMAKE_BINARY_DIR}/probe.out\")\n").unwrap();
    let configure = Command::new("cmake")
        .arg("-S")
        .arg(&source)
        .arg("-B")
        .arg(&build)
        .args(["-G", "Ninja"])
        .output()
        .unwrap();
    assert!(
        configure.status.success(),
        "{}",
        String::from_utf8_lossy(&configure.stderr)
    );
    verify_configured_tree(&build, true).unwrap();
    let execute = Command::new("cmake")
        .arg("--build")
        .arg(&build)
        .output()
        .unwrap();
    assert!(
        execute.status.success(),
        "{}",
        String::from_utf8_lossy(&execute.stderr)
    );
    assert!(build.join("probe.out").is_file());
    verify_configured_tree(&build, false).unwrap();
    let rules = build.join("CMakeFiles/rules.ninja");
    let before = fs::read(&rules).unwrap();
    fs::write(&rules, [before.as_slice(), b"\n# changed rules\n"].concat()).unwrap();
    assert!(verify_configured_tree(&build, false).is_err());
}
