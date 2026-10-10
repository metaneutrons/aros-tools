use aros_transpiler::{
    ast::{CopyDirectoryDecl, MetaTargetRule},
    generate_cmake, DependencyGraph, TargetContext,
};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const SCC_ENTRYPOINTS: &[&str] = &["aggregate-a", "aggregate-b", "copy-one", "copy-two"];
const EXTERNAL_PREREQUISITE: &str = "external-prerequisite";
const COPY_ONE_SEED: &[u8] = b"copy one seed\n";
const COPY_TWO_SEED: &[u8] = b"copy two seed\n";
const COPY_ONE_GENERATED: &[u8] = b"copy one written by external prerequisite\n";
const COPY_TWO_GENERATED: &[u8] = b"copy two written by external prerequisite\n";

fn require_tool(program: &str, args: &[&str]) {
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("required test tool {program} is unavailable: {error}"));
    assert!(
        output.status.success(),
        "required test tool {program} {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn require_success(output: &Output, action: &str) {
    assert!(
        output.status.success(),
        "{action} failed (status {}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn cmake_quote(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn add_meta_rule(graph: &mut DependencyGraph, name: &str, dependencies: &[&str]) {
    graph.add_meta_rule(MetaTargetRule {
        name: name.to_owned(),
        dependencies: dependencies
            .iter()
            .map(|dependency| (*dependency).to_owned())
            .collect(),
    });
}

fn copy_decl(name: &str, source: &Path, destination: &str) -> CopyDirectoryDecl {
    CopyDirectoryDecl {
        name: name.to_owned(),
        source: source.to_string_lossy().into_owned(),
        destination: destination.to_owned(),
        file: "fixture/mmakefile.src".to_owned(),
        line: 1,
        dependencies: Vec::new(),
    }
}

fn copy_action(graph: &DependencyGraph, source: &Path) -> String {
    let source = source.to_string_lossy();
    let mut actions = graph
        .copy_directories
        .iter()
        .filter(|declaration| declaration.source == source.as_ref())
        .map(|declaration| declaration.name.clone());
    let action = actions
        .next()
        .expect("copy declaration has a private action");
    assert!(
        actions.next().is_none(),
        "source has duplicate copy actions"
    );
    assert!(
        action.starts_with("aros-meta-copy-action-"),
        "copy declaration was not lifted to a private action: {action}"
    );
    action
}

fn fixture_graph(source_root: &Path) -> DependencyGraph {
    let mut graph = DependencyGraph::new();
    // Every node in this SCC has a reciprocal metadata path: aggregate-a and
    // aggregate-b link both ways; each copy owner links to an aggregate that
    // in turn links back to that copy owner.
    add_meta_rule(
        &mut graph,
        "aggregate-a",
        &["aggregate-b", "copy-one", EXTERNAL_PREREQUISITE],
    );
    add_meta_rule(&mut graph, "aggregate-b", &["aggregate-a", "copy-two"]);
    add_meta_rule(&mut graph, "copy-one", &["aggregate-a"]);
    add_meta_rule(&mut graph, "copy-two", &["aggregate-b"]);
    add_meta_rule(&mut graph, EXTERNAL_PREREQUISITE, &[]);

    // This separate cycle guards selection retention: its copy action must
    // not leak into any selected project built from the first SCC.
    add_meta_rule(&mut graph, "unrelated-aggregate", &["unrelated-copy"]);
    add_meta_rule(&mut graph, "unrelated-copy", &["unrelated-aggregate"]);

    let declarations = vec![
        copy_decl(
            "copy-one",
            &source_root.join("copy-one"),
            "${AROS_BUILD_DIR}/staged/copy-one",
        ),
        copy_decl(
            "copy-two",
            &source_root.join("copy-two"),
            "${AROS_BUILD_DIR}/staged/copy-two",
        ),
        copy_decl(
            "unrelated-copy",
            &source_root.join("unrelated-copy"),
            "${AROS_BUILD_DIR}/staged/unrelated-copy",
        ),
    ];
    assert!(graph.add_copy_directories(declarations).is_empty());
    graph.make_meta_providers.extend(
        ["copy-one", "copy-two", "unrelated-copy"]
            .into_iter()
            .map(str::to_owned),
    );
    graph
}

fn isolated_cmake_project(project: &Path, source_root: &Path, engine: &Path, generated: &Path) {
    fs::create_dir_all(project).expect("create isolated CMake project");
    let prepare_script = project.join("prepare-copy-inputs.cmake");
    fs::write(
        &prepare_script,
        format!(
            "file(WRITE \"{}/copy-one/include/one.h\" \"copy one written by external prerequisite\\n\")\nfile(WRITE \"{}/copy-two/include/two.h\" \"copy two written by external prerequisite\\n\")\nfile(WRITE \"${{CMAKE_BINARY_DIR}}/{EXTERNAL_PREREQUISITE}.ran\" \"ran\\n\")\n",
            cmake_quote(source_root),
            cmake_quote(source_root),
        ),
    )
    .expect("write external prerequisite fixture script");
    let cmake_lists = format!(
        r#"cmake_minimum_required(VERSION 3.22)
project(meta_copy_cycle C)
set(_bootstrap "${{CMAKE_BINARY_DIR}}/bootstrap")
file(MAKE_DIRECTORY "${{_bootstrap}}")
file(WRITE "${{_bootstrap}}/BootstrapSDK.cmake" "function(aros_bootstrap_sdk_includes)\nendfunction()\n")
list(PREPEND CMAKE_MODULE_PATH "${{_bootstrap}}")
set(AROS_SOURCE_DIR "{}")
set(AROS_TARGET_CPU riscv)
set(AROS_TARGET_PLATFORM fixture)
include("{}/AROS.cmake")
add_custom_target({EXTERNAL_PREREQUISITE}
    COMMAND "${{CMAKE_COMMAND}}" -E sleep 0.5
    COMMAND "${{CMAKE_COMMAND}}" -P "{}"
    VERBATIM)
include("{}")
"#,
        cmake_quote(source_root),
        cmake_quote(engine),
        cmake_quote(&prepare_script),
        cmake_quote(generated),
    );
    fs::write(project.join("CMakeLists.txt"), cmake_lists).expect("write isolated CMake project");
}

fn configure_and_build(project: &Path, build: &Path, entrypoint: &str) {
    let configure = Command::new("cmake")
        .args(["-S"])
        .arg(project)
        .arg("-B")
        .arg(build)
        .args(["-G", "Ninja"])
        .output()
        .expect("run CMake configure");
    require_success(&configure, &format!("configure {entrypoint} project"));

    let execution = Command::new("cmake")
        .arg("--build")
        .arg(build)
        .args(["--target", entrypoint, "--parallel", "8"])
        .output()
        .expect("run CMake build");
    require_success(
        &execution,
        &format!("build original entrypoint {entrypoint}"),
    );
}

#[test]
fn copy_actions_survive_recursive_meta_scc_selection_and_cmake_builds() {
    // Missing tools are test failures, not skips: this test's oracle is actual
    // CMake generation and a Ninja build of each original public endpoint.
    require_tool("cmake", &["--version"]);
    require_tool("ninja", &["--version"]);

    let temporary = tempfile::tempdir().expect("private fixture root");
    let source_root = temporary.path().join("source");
    for (directory, header, contents) in [
        ("copy-one", "one.h", COPY_ONE_SEED),
        ("copy-two", "two.h", COPY_TWO_SEED),
        (
            "unrelated-copy",
            "unrelated.h",
            b"unrelated header\n".as_slice(),
        ),
    ] {
        let include = source_root.join(directory).join("include");
        fs::create_dir_all(&include).expect("create distinct copy source");
        fs::write(include.join(header), contents).expect("write distinct source header");
    }
    let startup = source_root.join("compiler/startup");
    fs::create_dir_all(&startup).expect("create AROS startup fixture directory");
    fs::write(
        startup.join("startup.c"),
        "int fixture_startup(void) { return 0; }\n",
    )
    .expect("write CMake engine startup fixture");
    fs::write(
        startup.join("detach.c"),
        "int fixture_detach(void) { return 0; }\n",
    )
    .expect("write CMake engine detach fixture");

    let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine")
        .canonicalize()
        .expect("resolve the engine that owns aros_copy_dir_recursive");
    let context = TargetContext::default();

    for entrypoint in SCC_ENTRYPOINTS {
        let mut graph = fixture_graph(&source_root);
        let reports = graph
            .flatten_meta_cycles()
            .expect("flatten copy-producing and unrelated metadata SCCs");
        assert!(reports.iter().any(|report| report.contains("copy-one")));
        assert!(reports.iter().any(|report| report.contains("copy-two")));
        assert!(reports
            .iter()
            .any(|report| report.contains("unrelated-copy")));

        let copy_one_action = copy_action(&graph, &source_root.join("copy-one"));
        let copy_two_action = copy_action(&graph, &source_root.join("copy-two"));
        let unrelated_action = copy_action(&graph, &source_root.join("unrelated-copy"));
        assert_ne!(copy_one_action, copy_two_action);
        assert_ne!(copy_one_action, unrelated_action);
        assert_ne!(copy_two_action, unrelated_action);

        let expected_shared = BTreeSet::from([
            copy_one_action.clone(),
            copy_two_action.clone(),
            EXTERNAL_PREREQUISITE.to_owned(),
        ]);
        for owner in SCC_ENTRYPOINTS {
            let dependencies = graph
                .meta_targets
                .get(*owner)
                .unwrap_or_else(|| panic!("missing public SCC alias {owner}"));
            assert_eq!(
                dependencies.iter().cloned().collect::<BTreeSet<_>>(),
                expected_shared,
                "each original SCC entrypoint must reach both copy actions and the external prerequisite"
            );
        }
        assert!(graph
            .copy_directories
            .iter()
            .all(|declaration| declaration.dependencies.is_empty()));
        for action in [&copy_one_action, &copy_two_action] {
            let action_dependencies = graph
                .meta_targets
                .get(action)
                .unwrap_or_else(|| panic!("private action {action} has no metadata node"));
            assert!(
                action_dependencies.contains(EXTERNAL_PREREQUISITE),
                "private action {action} must order after the external prerequisite"
            );
            let action_selection = graph
                .selected_dependency_closure(std::slice::from_ref(action), &context, &[])
                .unwrap_or_else(|error| panic!("select private action {action}: {error}"));
            assert!(
                action_selection.contains(EXTERNAL_PREREQUISITE),
                "private action {action} selection omitted the external prerequisite"
            );
            assert!(!action_selection.contains(&unrelated_action));
        }

        let selected = graph
            .selected_dependency_closure(&[(*entrypoint).to_owned()], &context, &[])
            .unwrap_or_else(|error| panic!("select {entrypoint}: {error}"));
        for required in &expected_shared {
            assert!(
                selected.contains(required),
                "{entrypoint} selection omitted required endpoint {required}"
            );
        }
        assert!(!selected.contains(&unrelated_action));
        assert!(!selected.contains("unrelated-aggregate"));
        assert!(!selected.contains("unrelated-copy"));
        graph
            .retain_native_selection(&selected, &context)
            .unwrap_or_else(|error| panic!("retain {entrypoint} selection: {error}"));
        assert_eq!(graph.copy_directories.len(), 2);
        assert!(graph
            .copy_directories
            .iter()
            .all(|declaration| declaration.name == copy_one_action
                || declaration.name == copy_two_action));

        let generated_cmake = generate_cmake(&graph);
        assert_eq!(
            generated_cmake.matches("aros_copy_dir_recursive(").count(),
            2
        );
        assert!(generated_cmake.contains(&copy_one_action));
        assert!(generated_cmake.contains(&copy_two_action));
        assert!(generated_cmake.contains(EXTERNAL_PREREQUISITE));
        assert!(!generated_cmake.contains(&unrelated_action));
        assert!(!generated_cmake.contains("staged/unrelated-copy"));

        let project = temporary.path().join(format!("project-{entrypoint}"));
        let build = temporary.path().join(format!("build-{entrypoint}"));
        fs::write(source_root.join("copy-one/include/one.h"), COPY_ONE_SEED)
            .expect("reset copy-one input before the ordering probe");
        fs::write(source_root.join("copy-two/include/two.h"), COPY_TWO_SEED)
            .expect("reset copy-two input before the ordering probe");
        fs::create_dir_all(&project).expect("create entrypoint project directory");
        let generated_path = project.join("generated.cmake");
        fs::write(&generated_path, generated_cmake).expect("write selected generated CMake");
        isolated_cmake_project(&project, &source_root, &engine, &generated_path);
        configure_and_build(&project, &build, entrypoint);

        for (directory, header, contents) in [
            ("copy-one", "one.h", COPY_ONE_GENERATED),
            ("copy-two", "two.h", COPY_TWO_GENERATED),
        ] {
            let copied = build
                .join("staged")
                .join(directory)
                .join("include")
                .join(header);
            assert_eq!(
                fs::read(&copied).unwrap_or_else(|error| panic!(
                    "{entrypoint} did not copy {}: {error}",
                    copied.display()
                )),
                contents,
                "{entrypoint} must preserve the distinct source header bytes"
            );
        }
        assert!(
            build.join(format!("{EXTERNAL_PREREQUISITE}.ran")).is_file(),
            "{entrypoint} did not execute the external prerequisite"
        );
        assert!(
            !build.join("staged/unrelated-copy").exists(),
            "{entrypoint} selection unexpectedly retained the unrelated copy SCC"
        );
    }

    // Counterprobe the ordering oracle by deliberately breaking only the
    // private-action -> external-prerequisite edges. The public aliases still
    // reach both copy actions and the prerequisite, so selection remains the
    // same, but the engine no longer has an edge to mirror onto either copy
    // command. The delayed prerequisite should then run after the copy, leaving
    // the seed bytes in the destination.
    let entrypoint = "aggregate-a";
    let mut broken_graph = fixture_graph(&source_root);
    broken_graph
        .flatten_meta_cycles()
        .expect("flatten counterprobe metadata SCCs");
    let broken_copy_one_action = copy_action(&broken_graph, &source_root.join("copy-one"));
    let broken_copy_two_action = copy_action(&broken_graph, &source_root.join("copy-two"));
    let broken_unrelated_action = copy_action(&broken_graph, &source_root.join("unrelated-copy"));
    for action in [&broken_copy_one_action, &broken_copy_two_action] {
        let dependencies = broken_graph
            .meta_targets
            .get_mut(action)
            .unwrap_or_else(|| panic!("private action {action} has no metadata node"));
        assert!(
            dependencies.remove(EXTERNAL_PREREQUISITE),
            "counterprobe expected {action} to depend on the external prerequisite"
        );
    }
    for owner in SCC_ENTRYPOINTS {
        let dependencies = broken_graph
            .meta_targets
            .get(*owner)
            .unwrap_or_else(|| panic!("missing public SCC alias {owner}"));
        assert!(dependencies.contains(EXTERNAL_PREREQUISITE));
        assert!(dependencies.contains(&broken_copy_one_action));
        assert!(dependencies.contains(&broken_copy_two_action));
    }
    assert!(broken_graph
        .copy_directories
        .iter()
        .all(|declaration| declaration.dependencies.is_empty()));

    let broken_selected = broken_graph
        .selected_dependency_closure(&[entrypoint.to_owned()], &context, &[])
        .expect("select broken-order counterprobe entrypoint");
    assert!(broken_selected.contains(EXTERNAL_PREREQUISITE));
    assert!(broken_selected.contains(&broken_copy_one_action));
    assert!(broken_selected.contains(&broken_copy_two_action));
    assert!(!broken_selected.contains(&broken_unrelated_action));
    broken_graph
        .retain_native_selection(&broken_selected, &context)
        .expect("retain broken-order counterprobe selection");

    let broken_cmake = generate_cmake(&broken_graph);
    assert_eq!(broken_cmake.matches("aros_copy_dir_recursive(").count(), 2);
    assert!(broken_cmake.contains(EXTERNAL_PREREQUISITE));
    assert!(!broken_cmake.contains(&broken_unrelated_action));
    fs::write(source_root.join("copy-one/include/one.h"), COPY_ONE_SEED)
        .expect("reset copy-one input before broken-order counterprobe");
    fs::write(source_root.join("copy-two/include/two.h"), COPY_TWO_SEED)
        .expect("reset copy-two input before broken-order counterprobe");
    let broken_project = temporary.path().join("project-broken-order");
    let broken_build = temporary.path().join("build-broken-order");
    fs::create_dir_all(&broken_project).expect("create broken-order project directory");
    let broken_generated = broken_project.join("generated.cmake");
    fs::write(&broken_generated, broken_cmake).expect("write broken-order generated CMake");
    isolated_cmake_project(&broken_project, &source_root, &engine, &broken_generated);
    configure_and_build(&broken_project, &broken_build, entrypoint);

    for (directory, header, contents) in [
        ("copy-one", "one.h", COPY_ONE_SEED),
        ("copy-two", "two.h", COPY_TWO_SEED),
    ] {
        let copied = broken_build
            .join("staged")
            .join(directory)
            .join("include")
            .join(header);
        assert_eq!(
            fs::read(&copied).unwrap_or_else(|error| panic!(
                "broken-order counterprobe did not copy {}: {error}",
                copied.display()
            )),
            contents,
            "broken-order counterprobe should expose the missing prerequisite edge"
        );
    }
    assert!(
        broken_build
            .join(format!("{EXTERNAL_PREREQUISITE}.ran"))
            .is_file(),
        "broken-order counterprobe did not execute the external prerequisite"
    );
}
