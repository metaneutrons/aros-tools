#![cfg(unix)]

use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

struct Fixture {
    temporary: TempDir,
    source: PathBuf,
    build: PathBuf,
    compiler: PathBuf,
    trace: PathBuf,
    source_file: PathBuf,
    assembly: PathBuf,
    header: PathBuf,
}

#[derive(Clone, Copy)]
struct HeaderRequest<'a> {
    name: &'a str,
    aggregate: &'a str,
    source: &'a Path,
    assembly: &'a Path,
    output: &'a Path,
    header_root: Option<&'a Path>,
    token: &'a str,
    arguments: &'a [String],
    dependencies: &'a [&'a str],
}

impl<'a> HeaderRequest<'a> {
    fn for_fixture(fixture: &'a Fixture, name: &'a str, aggregate: &'a str) -> Self {
        Self {
            name,
            aggregate,
            source: &fixture.source_file,
            assembly: &fixture.assembly,
            output: &fixture.header,
            header_root: None,
            token: ".ascii",
            arguments: &[],
            dependencies: &[],
        }
    }

    const fn with_source(mut self, source: &'a Path) -> Self {
        self.source = source;
        self
    }

    const fn with_assembly(mut self, assembly: &'a Path) -> Self {
        self.assembly = assembly;
        self
    }

    const fn with_output(mut self, output: &'a Path) -> Self {
        self.output = output;
        self
    }

    const fn with_header_root(mut self, header_root: &'a Path) -> Self {
        self.header_root = Some(header_root);
        self
    }

    const fn with_token(mut self, token: &'a str) -> Self {
        self.token = token;
        self
    }

    const fn with_arguments(mut self, arguments: &'a [String]) -> Self {
        self.arguments = arguments;
        self
    }

    const fn with_dependencies(mut self, dependencies: &'a [&'a str]) -> Self {
        self.dependencies = dependencies;
        self
    }
}

impl Fixture {
    fn new(assembly_text: &str, require_markers: bool) -> Self {
        let temporary = tempfile::tempdir().expect("create assembly-header fixture");
        let root = fs::canonicalize(temporary.path()).expect("canonicalize fixture root");
        let source = root.join("source");
        let build = root.join("build");
        fs::create_dir_all(source.join("include")).expect("create source include directory");
        fs::create_dir_all(&build).expect("create build directory");
        let source_file = source.join("asm.c");
        fs::write(
            &source_file,
            "int assembly_header_fixture(void) { return 0; }\n",
        )
        .expect("write C input");

        let compiler = root.join("mock-cc");
        let trace = root.join("compiler-argv.txt");
        fs::write(&trace, "").expect("initialize compiler argv trace");
        let compiler_script = mock_compiler_script(&trace, &build, assembly_text, require_markers);
        fs::write(&compiler, compiler_script).expect("write mock compiler");
        let mut permissions = fs::metadata(&compiler)
            .expect("read mock compiler permissions")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&compiler, permissions).expect("make mock compiler executable");

        let assembly = build.join("gen/dir/asm.s");
        let header = build.join("GENINCDIR/aros/cpu/asm.h");
        Self {
            temporary,
            source,
            build,
            compiler,
            trace,
            source_file,
            assembly,
            header,
        }
    }

    fn configure(&self, body: &str, compiler_id: &str) -> Output {
        let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("engine");
        let project = format!(
            "cmake_minimum_required(VERSION 3.22)\n\
             project(AssemblyHeaderFixture NONE)\n\
             set(AROS_SOURCE_DIR {})\n\
             set(AROS_BUILD_DIR \"${{CMAKE_BINARY_DIR}}\")\n\
             set(AROS_GENINC_DIR \"${{CMAKE_BINARY_DIR}}/GENINCDIR\")\n\
             set(CMAKE_C_COMPILER {})\n\
             set(CMAKE_C_COMPILER_ID \"{}\")\n\
             set(CMAKE_C_COMPILER_ARG1 \"\")\n\
             set(CMAKE_C_COMPILER_LAUNCHER \"\")\n\
             set(CMAKE_C_FLAGS \"-DLEAKED_CMAKE_C_FLAGS\")\n\
             include({})\n\
             include({})\n\
             {}\n",
            cmake_quote(&self.source),
            cmake_quote(&self.compiler),
            compiler_id,
            cmake_quote(&engine.join("LiteralObjects.cmake")),
            cmake_quote(&engine.join("AssemblyHeaders.cmake")),
            body
        );
        fs::write(self.source.join("CMakeLists.txt"), project).expect("write CMake project");
        Command::new("cmake")
            .arg("-S")
            .arg(&self.source)
            .arg("-B")
            .arg(&self.build)
            .arg("-G")
            .arg("Ninja")
            .output()
            .expect("run CMake configure")
    }

    fn build_target(&self, target: &str) -> Output {
        Command::new("cmake")
            .arg("--build")
            .arg(&self.build)
            .arg("--target")
            .arg(target)
            .output()
            .expect("build CMake target")
    }

    fn change_compiler_output(&self, contents: &str) {
        let compiler_script = mock_compiler_script(&self.trace, &self.build, contents, false);
        fs::write(&self.compiler, compiler_script).expect("change mock compiler output");
        fs::write(
            &self.source_file,
            "int assembly_header_fixture(void) { return 1; }\n",
        )
        .expect("modify C source");
        let modified = SystemTime::now() + Duration::from_secs(10);
        fs::File::open(&self.compiler)
            .expect("open updated mock compiler")
            .set_times(fs::FileTimes::new().set_modified(modified))
            .expect("advance compiler timestamp");
        fs::File::open(&self.source_file)
            .expect("open updated C source")
            .set_times(fs::FileTimes::new().set_modified(modified))
            .expect("advance C source timestamp");
    }
}

fn render_declaration(request: HeaderRequest<'_>) -> String {
    let HeaderRequest {
        name,
        aggregate,
        source,
        assembly,
        output,
        header_root,
        token,
        arguments,
        dependencies,
    } = request;
    let mut declaration = format!(
        "aros_generate_assembly_header(\n    NAME {name}\n    AGGREGATE {aggregate}\n    SOURCE {}\n    ASSEMBLY {}\n    OUTPUT {}\n",
        cmake_quote(source),
        cmake_quote(assembly),
        cmake_quote(output)
    );
    if let Some(header_root) = header_root {
        writeln!(declaration, "    HEADER_ROOT {}", cmake_quote(header_root))
            .expect("write header-root declaration");
    }
    writeln!(declaration, "    TOKEN {token}").expect("write token declaration");
    if !arguments.is_empty() {
        declaration.push_str("    ARGUMENTS\n");
        for argument in arguments {
            declaration.push_str("        ");
            declaration.push_str(&cmake_quote_text(argument));
            declaration.push('\n');
        }
    }
    if !dependencies.is_empty() {
        declaration.push_str("    DEPENDS ");
        declaration.push_str(&dependencies.join(" "));
        declaration.push('\n');
    }
    declaration.push_str(")\n");
    declaration
}

fn project_body(
    declaration: &str,
    create_aggregate: bool,
    create_dependencies: bool,
    record_aggregate_dependencies: bool,
) -> String {
    let mut body = String::new();
    if create_aggregate {
        body.push_str("add_custom_target(aggregate-owner)\n");
    }
    if create_dependencies {
        body.push_str(
            "add_custom_target(include-copy COMMAND \"${CMAKE_COMMAND}\" -E touch \"${CMAKE_BINARY_DIR}/include-copy.done\")\n\
             add_custom_target(other-dep COMMAND \"${CMAKE_COMMAND}\" -E touch \"${CMAKE_BINARY_DIR}/other-dep.done\")\n",
        );
    }
    body.push_str(declaration);
    if record_aggregate_dependencies {
        body.push_str(
            "get_target_property(_aggregate_dependencies aggregate-owner MANUALLY_ADDED_DEPENDENCIES)\n\
             file(WRITE \"${CMAKE_BINARY_DIR}/aggregate-dependencies.txt\" \"${_aggregate_dependencies}\")\n",
        );
    }
    body
}

fn mock_compiler_script(
    trace: &Path,
    build: &Path,
    assembly_text: &str,
    require_markers: bool,
) -> String {
    let marker_check = if require_markers {
        format!(
            "test -f {}\ntest -f {}\n",
            shell_quote(&build.join("include-copy.done").to_string_lossy()),
            shell_quote(&build.join("other-dep.done").to_string_lossy())
        )
    } else {
        String::new()
    };
    format!(
        "#!/bin/sh\n\
         set -eu\n\
         trace={}\n\
         for argument do printf '%s\\n' \"$argument\" >> \"$trace\"; done\n\
         {}\
         depfile=\n\
         target=\n\
         source=\n\
         output=\n\
         while [ \"$#\" -gt 0 ]; do\n\
             case \"$1\" in\n\
                 -MF) shift; depfile=$1 ;;\n\
                 -MT) shift; target=$1 ;;\n\
                 -o) shift; output=$1 ;;\n\
                 *.c) source=$1 ;;\n\
             esac\n\
             shift\n\
         done\n\
         test -n \"$depfile\"\n\
         test -n \"$target\"\n\
         test -n \"$source\"\n\
         test -n \"$output\"\n\
         mkdir -p \"$(dirname \"$output\")\"\n\
         printf '%s' {} > \"$output\"\n\
         printf '%s: %s\\n' \"$target\" \"$source\" > \"$depfile\"\n",
        shell_quote(&trace.to_string_lossy()),
        marker_check,
        shell_quote(assembly_text)
    )
}

fn cmake_quote(path: &Path) -> String {
    cmake_quote_text(&path.to_string_lossy())
}

fn cmake_quote_text(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "/").replace('"', "\\\""))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn path_has_extension(path: &str, expected: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_configure_fails(fixture: &Fixture, body: &str, expected_text: Option<&str>) {
    let output = fixture.configure(body, "GNU");
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "unexpected configure success:\n{text}"
    );
    if let Some(expected) = expected_text {
        assert!(
            text.to_ascii_lowercase()
                .contains(&expected.to_ascii_lowercase()),
            "expected diagnostic containing {expected:?}, got:\n{text}"
        );
    }
}

fn setup_with_request(fixture: &Fixture, token: &str, arguments: &[String]) -> String {
    let declaration = render_declaration(
        HeaderRequest::for_fixture(fixture, "cpu-owner", "aggregate-owner")
            .with_token(token)
            .with_arguments(arguments)
            .with_dependencies(&["include-copy", "other-dep"]),
    );
    project_body(&declaration, true, true, true)
}

#[test]
fn assembly_header_build_uses_exact_arguments_and_cpu_aggregate_dependencies() {
    let assembly_text = ".ascii \"value$\"\n";
    let fixture = Fixture::new(assembly_text, true);
    let arguments = vec![
        "-DUSER_FLAG=17".to_owned(),
        "-O2".to_owned(),
        format!("-I{}", fixture.source.join("include").display()),
    ];
    let body = setup_with_request(&fixture, ".ascii", &arguments);
    let configure = fixture.configure(&body, "GNU");
    let configure_text = output_text(&configure);
    assert!(
        configure.status.success(),
        "configure failed:\n{configure_text}"
    );

    let owner_build = fixture.build_target("cpu-owner");
    let owner_text = output_text(&owner_build);
    assert!(
        owner_build.status.success(),
        "CPU owner build failed:\n{owner_text}"
    );
    assert!(fixture.assembly.is_file(), "assembly output was not built");
    assert_eq!(fs::read_to_string(&fixture.header).unwrap(), "value\n");
    assert!(fixture.build.join("include-copy.done").is_file());
    assert!(fixture.build.join("other-dep.done").is_file());

    let aggregate_dependencies =
        fs::read_to_string(fixture.build.join("aggregate-dependencies.txt"))
            .expect("read aggregate dependencies");
    let aggregate_dependencies: Vec<_> = aggregate_dependencies.split(';').collect();
    for expected in ["cpu-owner", "include-copy", "other-dep"] {
        assert!(
            aggregate_dependencies.contains(&expected),
            "aggregate is missing {expected}: {aggregate_dependencies:?}"
        );
    }

    let invocation: Vec<String> = fs::read_to_string(&fixture.trace)
        .expect("read mock compiler argv")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(&invocation[..arguments.len()], arguments.as_slice());
    assert!(!invocation
        .iter()
        .any(|argument| argument.contains("LEAKED_CMAKE_C_FLAGS")));
    assert_eq!(
        invocation.len(),
        arguments.len() + 9,
        "unexpected argv: {invocation:?}"
    );
    let runtime = &invocation[arguments.len()..];
    assert_eq!(runtime[0], "-MD");
    assert_eq!(runtime[1], "-MF");
    assert!(runtime[2].starts_with(&format!("{}.d.literal-", fixture.assembly.display())));
    assert!(path_has_extension(&runtime[2], "tmp"));
    assert_eq!(runtime[3], "-MT");
    assert_eq!(runtime[4], fixture.assembly.to_string_lossy());
    assert_eq!(runtime[5], "-S");
    assert_eq!(runtime[6], fixture.source_file.to_string_lossy());
    assert_eq!(runtime[7], "-o");
    assert!(runtime[8].starts_with(&format!("{}.literal-", fixture.assembly.display())));
    assert!(path_has_extension(&runtime[8], "tmp"));

    let aggregate_build = fixture.build_target("aggregate-owner");
    assert!(
        aggregate_build.status.success(),
        "aggregate build failed:\n{}",
        output_text(&aggregate_build)
    );
}

#[test]
fn multiple_assembly_header_providers_share_one_aggregate_without_cycles() {
    let fixture = Fixture::new(".ascii \"shared$\"\n", true);
    let second_source = fixture.source.join("second.c");
    let second_assembly = fixture.build.join("gen/second/asm.s");
    let second_header = fixture.build.join("GENINCDIR/aros/second/asm.h");
    fs::write(&second_source, "int second_fixture(void) { return 0; }\n").unwrap();

    let first = render_declaration(
        HeaderRequest::for_fixture(&fixture, "cpu-owner", "aggregate-owner")
            .with_dependencies(&["include-copy", "other-dep"]),
    );
    let second = render_declaration(
        HeaderRequest::for_fixture(&fixture, "second-owner", "aggregate-owner")
            .with_source(&second_source)
            .with_assembly(&second_assembly)
            .with_output(&second_header)
            .with_dependencies(&["include-copy", "other-dep"]),
    );
    let body = project_body(&format!("{first}\n{second}"), true, true, true);
    let configure = fixture.configure(&body, "GNU");
    assert!(configure.status.success(), "{}", output_text(&configure));

    let aggregate_build = fixture.build_target("aggregate-owner");
    assert!(
        aggregate_build.status.success(),
        "shared aggregate build failed (including any dependency cycle):\n{}",
        output_text(&aggregate_build)
    );
    assert_eq!(fs::read_to_string(&fixture.header).unwrap(), "shared\n");
    assert_eq!(fs::read_to_string(&second_header).unwrap(), "shared\n");
    assert_eq!(
        fs::read_to_string(&fixture.trace).unwrap().lines().count(),
        18,
        "each provider should compile exactly once"
    );

    let aggregate_dependencies =
        fs::read_to_string(fixture.build.join("aggregate-dependencies.txt")).unwrap();
    let aggregate_dependencies: Vec<_> = aggregate_dependencies.split(';').collect();
    for expected in ["cpu-owner", "second-owner", "include-copy", "other-dep"] {
        assert!(
            aggregate_dependencies.contains(&expected),
            "shared aggregate is missing {expected}: {aggregate_dependencies:?}"
        );
    }
}

#[test]
fn assembly_header_selects_only_the_requested_gnu_or_clang_token() {
    for (compiler_id, token, assembly_text, expected) in [
        (
            "Clang",
            ".ascii",
            ".ascii \"first$\"\n.ascii \"inside$dollar$$\" \"discarded\"\n.ascii \"semi;colon\\path$\"\n.asciz \"wrong$\"\n.ascii \"first field\" \"second field\"\n",
            "first\ninside$dollar$\nsemi;colon\\path\nfirst field\n",
        ),
        (
            "GNU",
            ".asciz",
            ".asciz \"gnu$\"\n.ascii \"not-selected$\"\n.asciz \"embedded$dollar$\"\n",
            "gnu\nembedded$dollar\n",
        ),
    ] {
        let fixture = Fixture::new(assembly_text, false);
        let declaration = render_declaration(
            HeaderRequest::for_fixture(&fixture, "cpu-owner", "aggregate-owner").with_token(token),
        );
        let body = project_body(&declaration, false, false, false);
        let configure = fixture.configure(&body, compiler_id);
        assert!(
            configure.status.success(),
            "{compiler_id} configure failed:\n{}",
            output_text(&configure)
        );
        let build = fixture.build_target("cpu-owner");
        assert!(
            build.status.success(),
            "{compiler_id} build failed:\n{}",
            output_text(&build)
        );
        assert_eq!(fs::read_to_string(&fixture.header).unwrap(), expected);
    }
}

#[test]
fn assembly_header_rejects_invalid_tokens_and_empty_extractions() {
    let invalid_token = Fixture::new(".ascii \"valid$\"\n", false);
    let declaration = render_declaration(
        HeaderRequest::for_fixture(&invalid_token, "cpu-owner", "aggregate-owner")
            .with_token(".word"),
    );
    let body = project_body(&declaration, false, false, false);
    assert_configure_fails(
        &invalid_token,
        &body,
        Some("token must be .ascii or .asciz"),
    );

    let no_selected_lines = Fixture::new(".asciz \"not-selected$\"\n", false);
    let declaration = render_declaration(HeaderRequest::for_fixture(
        &no_selected_lines,
        "cpu-owner",
        "aggregate-owner",
    ));
    let body = project_body(&declaration, false, false, false);
    let configure = no_selected_lines.configure(&body, "GNU");
    assert!(configure.status.success(), "{}", output_text(&configure));
    let build = no_selected_lines.build_target("cpu-owner");
    let text = output_text(&build);
    assert!(
        !build.status.success(),
        "empty extraction unexpectedly succeeded"
    );
    assert!(
        text.contains("empty extraction"),
        "unexpected build error:\n{text}"
    );
    assert!(!no_selected_lines.header.exists());
}

#[test]
fn stale_header_does_not_hide_empty_extraction_and_is_preserved_on_failure() {
    let fixture = Fixture::new(".ascii \"initial$\"\n", false);
    let declaration = render_declaration(HeaderRequest::for_fixture(
        &fixture,
        "cpu-owner",
        "aggregate-owner",
    ));
    let body = project_body(&declaration, false, false, false);
    let configure = fixture.configure(&body, "GNU");
    assert!(configure.status.success(), "{}", output_text(&configure));
    let first_build = fixture.build_target("cpu-owner");
    assert!(
        first_build.status.success(),
        "{}",
        output_text(&first_build)
    );
    assert_eq!(fs::read_to_string(&fixture.header).unwrap(), "initial\n");

    fixture.change_compiler_output(".asciz \"not-selected$\"\n");
    let failed_build = fixture.build_target("cpu-owner");
    let text = output_text(&failed_build);
    assert!(
        !failed_build.status.success(),
        "stale header hid an empty extraction; build output:\n{}",
        output_text(&failed_build)
    );
    assert!(
        text.contains("empty extraction"),
        "unexpected build error:\n{text}"
    );
    assert_eq!(fs::read_to_string(&fixture.header).unwrap(), "initial\n");
}

#[test]
fn changed_c_source_reruns_assembly_and_header_generation() {
    let fixture = Fixture::new(".ascii \"before$\"\n", false);
    let declaration = render_declaration(HeaderRequest::for_fixture(
        &fixture,
        "cpu-owner",
        "aggregate-owner",
    ));
    let body = project_body(&declaration, false, false, false);
    let configure = fixture.configure(&body, "GNU");
    assert!(configure.status.success(), "{}", output_text(&configure));
    let first_build = fixture.build_target("cpu-owner");
    assert!(
        first_build.status.success(),
        "{}",
        output_text(&first_build)
    );
    let original_invocation = fs::read_to_string(&fixture.trace).expect("read initial argv");
    assert_eq!(fs::read_to_string(&fixture.header).unwrap(), "before\n");

    fixture.change_compiler_output(".ascii \"after$\"\n");
    let second_build = fixture.build_target("cpu-owner");
    assert!(
        second_build.status.success(),
        "{}",
        output_text(&second_build)
    );
    assert_eq!(
        fs::read_to_string(&fixture.header).unwrap(),
        "after\n",
        "second build output:\n{}",
        output_text(&second_build)
    );
    let trace_lines: Vec<_> = fs::read_to_string(&fixture.trace)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        trace_lines.len(),
        18,
        "compiler should run twice: {trace_lines:?}"
    );
    let original_lines: Vec<_> = original_invocation.lines().collect();
    assert_eq!(original_lines.len(), 9);
    assert_eq!(&trace_lines[..9], original_lines.as_slice());
    assert_eq!(&trace_lines[9], "-MD");
    assert_eq!(&trace_lines[10], "-MF");
    assert_eq!(&trace_lines[12], "-MT");
    assert_eq!(
        &trace_lines[13],
        fixture.assembly.to_string_lossy().as_ref()
    );
    assert_eq!(&trace_lines[14], "-S");
    assert_eq!(
        &trace_lines[15],
        fixture.source_file.to_string_lossy().as_ref()
    );
    assert_eq!(&trace_lines[16], "-o");
}

#[test]
fn modifying_generated_assembly_reruns_its_producer_and_refreshes_the_header() {
    let fixture = Fixture::new(".ascii \"before$\"\n", false);
    let declaration = render_declaration(HeaderRequest::for_fixture(
        &fixture,
        "cpu-owner",
        "aggregate-owner",
    ));
    let body = project_body(&declaration, false, false, false);
    let configure = fixture.configure(&body, "GNU");
    assert!(configure.status.success(), "{}", output_text(&configure));
    let first_build = fixture.build_target("cpu-owner");
    assert!(
        first_build.status.success(),
        "{}",
        output_text(&first_build)
    );
    assert_eq!(fs::read_to_string(&fixture.header).unwrap(), "before\n");
    let original_invocation = fs::read_to_string(&fixture.trace).unwrap();

    fs::write(&fixture.assembly, ".ascii \"edited$\"\n").expect("modify assembly output");
    fs::write(&fixture.header, "stale header\n").expect("make prior header stale");
    fs::File::open(&fixture.header)
        .expect("open stale header")
        .set_times(fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(10)))
        .expect("make header older than modified assembly");

    let rebuild = fixture.build_target("cpu-owner");
    assert!(rebuild.status.success(), "{}", output_text(&rebuild));
    assert_eq!(
        fs::read_to_string(&fixture.header).unwrap(),
        "before\n",
        "header was not restored from regenerated assembly; build output:\n{}",
        output_text(&rebuild)
    );
    let trace = fs::read_to_string(&fixture.trace).unwrap();
    assert_eq!(
        trace.lines().count(),
        original_invocation.lines().count() * 2
    );
    assert!(output_text(&rebuild).contains("Compiling literal object asm.s"));
    assert!(output_text(&rebuild).contains("Generating GENINCDIR/aros/cpu/asm.h"));
}

#[test]
fn assembly_header_private_command_file_is_digest_guarded() {
    let fixture = Fixture::new(".ascii \"protected$\"\n", false);
    let declaration = render_declaration(HeaderRequest::for_fixture(
        &fixture,
        "cpu-owner",
        "aggregate-owner",
    ));
    let body = project_body(&declaration, false, false, false);
    let configure = fixture.configure(&body, "GNU");
    assert!(configure.status.success(), "{}", output_text(&configure));
    let first_build = fixture.build_target("cpu-owner");
    assert!(
        first_build.status.success(),
        "{}",
        output_text(&first_build)
    );
    let original_header = fs::read(&fixture.header).expect("read protected header");

    let command_file = fs::read_dir(fixture.build.join("CMakeFiles"))
        .expect("list private CMake command files")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("aros-assembly-header-") && path_has_extension(name, "cmake")
                })
        })
        .expect("find private assembly-header command file");
    let mut changed_contents = fs::read_to_string(&command_file).expect("read command file");
    changed_contents.push_str("# unexpected change\n");
    fs::write(&command_file, changed_contents).expect("tamper with private command file");

    let failed_build = fixture.build_target("cpu-owner");
    let text = output_text(&failed_build);
    assert!(
        !failed_build.status.success(),
        "modified command file was accepted"
    );
    assert!(
        text.contains("changed command file"),
        "unexpected build error:\n{text}"
    );
    assert_eq!(fs::read(&fixture.header).unwrap(), original_header);
}

#[test]
fn assembly_header_rejects_bad_source_output_roles_and_duplicate_owners_at_configure() {
    let missing_source = Fixture::new(".ascii \"unused$\"\n", false);
    let missing = missing_source.source.join("missing.c");
    let declaration = render_declaration(
        HeaderRequest::for_fixture(&missing_source, "cpu-owner", "aggregate-owner")
            .with_source(&missing),
    );
    let body = project_body(&declaration, false, false, false);
    assert_configure_fails(&missing_source, &body, None);

    let symlink_source = Fixture::new(".ascii \"unused$\"\n", false);
    let real_source = symlink_source.source.join("real.c");
    fs::write(&real_source, "int real_source(void) { return 0; }\n").unwrap();
    fs::remove_file(&symlink_source.source_file).unwrap();
    symlink(&real_source, &symlink_source.source_file).expect("create source symlink");
    let declaration = render_declaration(HeaderRequest::for_fixture(
        &symlink_source,
        "cpu-owner",
        "aggregate-owner",
    ));
    let body = project_body(&declaration, false, false, false);
    assert_configure_fails(&symlink_source, &body, Some("symlink"));

    let bad_assembly_role = Fixture::new(".ascii \"unused$\"\n", false);
    let outside_assembly = bad_assembly_role.build.join("outside-gen/asm.s");
    let declaration = render_declaration(
        HeaderRequest::for_fixture(&bad_assembly_role, "cpu-owner", "aggregate-owner")
            .with_assembly(&outside_assembly),
    );
    let body = project_body(&declaration, false, false, false);
    assert_configure_fails(&bad_assembly_role, &body, None);

    let bad_header_role = Fixture::new(".ascii \"unused$\"\n", false);
    let outside_header = bad_header_role.build.join("outside-gen/asm.h");
    let declaration = render_declaration(
        HeaderRequest::for_fixture(&bad_header_role, "cpu-owner", "aggregate-owner")
            .with_output(&outside_header),
    );
    let body = project_body(&declaration, false, false, false);
    assert_configure_fails(&bad_header_role, &body, None);

    let duplicate = Fixture::new(".ascii \"unused$\"\n", false);
    let first = render_declaration(HeaderRequest::for_fixture(
        &duplicate,
        "cpu-owner",
        "aggregate-owner",
    ));
    let second_assembly = duplicate.build.join("gen/dir/another.s");
    let second_header = duplicate.build.join("GENINCDIR/aros/cpu/another.h");
    let second = render_declaration(
        HeaderRequest::for_fixture(&duplicate, "cpu-owner", "aggregate-owner")
            .with_assembly(&second_assembly)
            .with_output(&second_header),
    );
    let body = format!("{first}\n{second}");
    assert_configure_fails(&duplicate, &body, Some("duplicate owner"));
}

#[test]
fn assembly_header_rejects_each_private_target_name_as_aggregate_or_dependency() {
    let private_suffixes = [
        "assembly",
        "assembly-compile",
        "assembly-check",
        "extract",
        "header-check",
    ];

    for suffix in private_suffixes {
        let fixture = Fixture::new(".ascii \"unused$\"\n", false);
        let private_name = format!("cpu-owner-{suffix}");
        let declaration = render_declaration(HeaderRequest::for_fixture(
            &fixture,
            "cpu-owner",
            &private_name,
        ));
        let body = project_body(&declaration, false, false, false);
        assert_configure_fails(&fixture, &body, Some("private producer target"));
    }

    for suffix in private_suffixes {
        let fixture = Fixture::new(".ascii \"unused$\"\n", false);
        let private_name = format!("cpu-owner-{suffix}");
        let declaration = render_declaration(
            HeaderRequest::for_fixture(&fixture, "cpu-owner", "aggregate-owner")
                .with_dependencies(&[private_name.as_str()]),
        );
        let body = project_body(&declaration, false, false, false);
        assert_configure_fails(&fixture, &body, Some("unsafe dependency"));
    }

    let shared = Fixture::new(".ascii \"unused$\"\n", false);
    let second_source = shared.source.join("second.c");
    let second_assembly = shared.build.join("gen/second/asm.s");
    let second_header = shared.build.join("GENINCDIR/aros/second/asm.h");
    fs::write(&second_source, "int second_fixture(void) { return 0; }\n").unwrap();
    let first = render_declaration(HeaderRequest::for_fixture(
        &shared,
        "cpu-owner",
        "aggregate-owner",
    ));
    let second = render_declaration(
        HeaderRequest::for_fixture(&shared, "other-owner", "cpu-owner-extract")
            .with_source(&second_source)
            .with_assembly(&second_assembly)
            .with_output(&second_header),
    );
    assert_configure_fails(
        &shared,
        &format!("{first}\n{second}"),
        Some("private producer target"),
    );

    let shared_dependency = Fixture::new(".ascii \"unused$\"\n", false);
    let second_source = shared_dependency.source.join("second.c");
    let second_assembly = shared_dependency.build.join("gen/second/asm.s");
    let second_header = shared_dependency.build.join("GENINCDIR/aros/second/asm.h");
    fs::write(&second_source, "int second_fixture(void) { return 0; }\n").unwrap();
    let first = render_declaration(HeaderRequest::for_fixture(
        &shared_dependency,
        "cpu-owner",
        "aggregate-owner",
    ));
    let second = render_declaration(
        HeaderRequest::for_fixture(&shared_dependency, "other-owner", "aggregate-owner")
            .with_source(&second_source)
            .with_assembly(&second_assembly)
            .with_output(&second_header)
            .with_dependencies(&["cpu-owner-assembly"]),
    );
    assert_configure_fails(
        &shared_dependency,
        &format!("{first}\n{second}"),
        Some("unsafe dependency"),
    );
}

#[test]
fn assembly_header_accepts_only_nonsymlink_header_roots_below_build() {
    let accepted = Fixture::new(".ascii \"custom$\"\n", false);
    let custom_root = accepted.build.join("generated/custom-headers");
    let custom_header = custom_root.join("aros/cpu/asm.h");
    fs::create_dir_all(&custom_root).unwrap();
    let declaration = render_declaration(
        HeaderRequest::for_fixture(&accepted, "cpu-owner", "aggregate-owner")
            .with_output(&custom_header)
            .with_header_root(&custom_root),
    );
    let body = project_body(&declaration, false, false, false);
    let configure = accepted.configure(&body, "GNU");
    assert!(configure.status.success(), "{}", output_text(&configure));
    let build = accepted.build_target("cpu-owner");
    assert!(build.status.success(), "{}", output_text(&build));
    assert_eq!(fs::read_to_string(custom_header).unwrap(), "custom\n");

    let outside = Fixture::new(".ascii \"unused$\"\n", false);
    let outside_root = outside.temporary.path().join("outside-header-root");
    let outside_header = outside_root.join("aros/cpu/asm.h");
    fs::create_dir_all(&outside_root).unwrap();
    let declaration = render_declaration(
        HeaderRequest::for_fixture(&outside, "cpu-owner", "aggregate-owner")
            .with_output(&outside_header)
            .with_header_root(&outside_root),
    );
    let body = project_body(&declaration, false, false, false);
    assert_configure_fails(&outside, &body, Some("must be below the build root"));

    let symlinked = Fixture::new(".ascii \"unused$\"\n", false);
    let real_root = symlinked.build.join("real-header-root");
    let symlink_root = symlinked.build.join("symlink-header-root");
    fs::create_dir_all(&real_root).unwrap();
    symlink(&real_root, &symlink_root).expect("create symlinked header root");
    let symlink_header = symlink_root.join("aros/cpu/asm.h");
    let declaration = render_declaration(
        HeaderRequest::for_fixture(&symlinked, "cpu-owner", "aggregate-owner")
            .with_output(&symlink_header)
            .with_header_root(&symlink_root),
    );
    let body = project_body(&declaration, false, false, false);
    assert_configure_fails(&symlinked, &body, Some("symlink"));
}

#[test]
fn source_and_output_symlink_guards_run_again_during_build() {
    let source_symlink = Fixture::new(".ascii \"unused$\"\n", false);
    let declaration = render_declaration(HeaderRequest::for_fixture(
        &source_symlink,
        "cpu-owner",
        "aggregate-owner",
    ));
    let body = project_body(&declaration, false, false, false);
    let configure = source_symlink.configure(&body, "GNU");
    assert!(configure.status.success(), "{}", output_text(&configure));
    let first_build = source_symlink.build_target("cpu-owner");
    assert!(
        first_build.status.success(),
        "{}",
        output_text(&first_build)
    );
    let original_header = fs::read(&source_symlink.header).expect("read source-guard header");
    let real_source = source_symlink.source.join("real-after-configure.c");
    fs::write(&real_source, "int changed_source(void) { return 1; }\n").unwrap();
    fs::remove_file(&source_symlink.source_file).unwrap();
    symlink(&real_source, &source_symlink.source_file).expect("replace C input by symlink");
    let failed_build = source_symlink.build_target("cpu-owner");
    let text = output_text(&failed_build);
    assert!(
        !failed_build.status.success(),
        "build accepted a symlink source"
    );
    assert!(
        text.to_ascii_lowercase().contains("symlink"),
        "unexpected build error:\n{text}"
    );
    assert_eq!(fs::read(&source_symlink.header).unwrap(), original_header);

    let output_symlink = Fixture::new(".ascii \"unused$\"\n", false);
    let declaration = render_declaration(HeaderRequest::for_fixture(
        &output_symlink,
        "cpu-owner",
        "aggregate-owner",
    ));
    let body = project_body(&declaration, false, false, false);
    let configure = output_symlink.configure(&body, "GNU");
    assert!(configure.status.success(), "{}", output_text(&configure));
    let sentinel = output_symlink.temporary.path().join("header-sentinel");
    fs::write(&sentinel, "do not overwrite\n").unwrap();
    fs::create_dir_all(output_symlink.header.parent().unwrap()).unwrap();
    symlink(&sentinel, &output_symlink.header).expect("create build-time output symlink");
    let failed_build = output_symlink.build_target("cpu-owner");
    let text = output_text(&failed_build);
    assert!(
        !failed_build.status.success(),
        "build accepted a symlink output"
    );
    assert!(
        text.to_ascii_lowercase().contains("symlink"),
        "unexpected build error:\n{text}"
    );
    assert_eq!(fs::read_to_string(sentinel).unwrap(), "do not overwrite\n");
}

#[test]
fn assembly_header_rejects_source_and_header_symlinks_during_configure() {
    let bad_source = Fixture::new(".ascii \"unused$\"\n", false);
    let source_target = bad_source.temporary.path().join("source-target.c");
    fs::write(&source_target, "int outside_source(void) { return 0; }\n").unwrap();
    fs::remove_file(&bad_source.source_file).unwrap();
    symlink(source_target, &bad_source.source_file).expect("create configure-time source symlink");
    let declaration = render_declaration(HeaderRequest::for_fixture(
        &bad_source,
        "cpu-owner",
        "aggregate-owner",
    ));
    let body = project_body(&declaration, false, false, false);
    assert_configure_fails(&bad_source, &body, Some("symlink"));

    let bad_output = Fixture::new(".ascii \"unused$\"\n", false);
    fs::create_dir_all(bad_output.header.parent().unwrap()).unwrap();
    let sentinel = bad_output
        .temporary
        .path()
        .join("configure-header-sentinel");
    fs::write(&sentinel, "do not overwrite\n").unwrap();
    symlink(&sentinel, &bad_output.header).expect("create configure-time output symlink");
    let declaration = render_declaration(HeaderRequest::for_fixture(
        &bad_output,
        "cpu-owner",
        "aggregate-owner",
    ));
    let body = project_body(&declaration, false, false, false);
    assert_configure_fails(&bad_output, &body, Some("symlink"));
    assert_eq!(fs::read_to_string(sentinel).unwrap(), "do not overwrite\n");
}
