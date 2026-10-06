//! Opt-in parity check for the source-owned m68k and i386 gencall headers.
//!
//! This exercises the exact P4 Make declarations, their parsed aggregate
//! models, and the generated CMake rules. It does not run Make or build AROS.

use aros_transpiler::{
    dirs::DirVars, generate_cmake, host_header_aggregates::HostHeaderAggregateDecl,
    parse_mmakefile_with_dirs_and_context, DependencyGraph, TargetContext,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, FileTimes, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, SystemTime},
};

const M68K_OWNER: &str = "includes-generated-m68k-libcall";
const M68K_TOOL: &str = "gencall_m68k";
const M68K_SOURCE: &str = "arch/m68k-all/include/gencall.c";
const M68K_MMAKEFILE: &str = "arch/m68k-all/include/mmakefile.src";
const I386_OWNER: &str = "includes-generated-i386-libcall";
const I386_TOOL: &str = "gencall_i386";
const I386_SOURCE: &str = "arch/i386-all/include/gencall.c";
const I386_MMAKEFILE: &str = "arch/i386-all/include/mmakefile.src";

#[test]
#[ignore = "requires explicit AROS_TEST_P4_SOURCE_ROOT"]
fn actual_p4_gencall_aggregates_match_independent_host_tools() {
    let source_root = required_source_root();
    let m68k_source = source_root.join(M68K_SOURCE);
    let i386_source = source_root.join(I386_SOURCE);
    let m68k_makefile = source_root.join(M68K_MMAKEFILE);
    let i386_makefile = source_root.join(I386_MMAKEFILE);
    for path in [&m68k_source, &i386_source, &m68k_makefile, &i386_makefile] {
        assert!(
            path.is_file(),
            "P4 source root is missing {}",
            path.display()
        );
    }

    assert_source_recipe(
        &m68k_makefile,
        &[
            "#MM- compiler-includes: includes-generated-m68k-libcall",
            "#MM includes-generated-m68k-libcall",
            "includes-generated-m68k-libcall: \\\n    $(AROS_INCLUDES)/aros/m68k/libcall.h \\\n    $(AROS_INCLUDES)/aros/m68k/asmcall.h",
            "$(AROS_INCLUDES)/aros/m68k/asmcall.h: $(HOSTGENDIR)/tools/gencall_m68k | $(AROS_INCLUDES)/aros/m68k",
            "\t$(HOSTGENDIR)/tools/gencall_m68k asmcall >$@",
            "$(AROS_INCLUDES)/aros/m68k/libcall.h: $(HOSTGENDIR)/tools/gencall_m68k | $(AROS_INCLUDES)/aros/m68k",
            "\t$(HOSTGENDIR)/tools/gencall_m68k libcall >$@",
            "$(HOSTGENDIR)/tools/gencall_m68k: $(SRCDIR)/$(CURDIR)/gencall.c",
            "\t@$(HOST_CC) -Wall -Werror -o $@ $<",
        ],
    );
    assert_source_recipe(
        &i386_makefile,
        &[
            "#MM- compiler-includes: includes-generated-i386-libcall",
            "#MM includes-generated-i386-libcall",
            "includes-generated-i386-libcall: $(AROS_INCLUDES)/aros/i386/libcall.h $(GENINCDIR)/aros/i386/libcall.h",
            "$(AROS_INCLUDES)/aros/i386/libcall.h: $(HOSTGENDIR)/tools/gencall_i386 | $(AROS_INCLUDES)/aros/i386",
            "\t$(HOSTGENDIR)/tools/gencall_i386 >$@",
            "$(GENINCDIR)/aros/i386/libcall.h: $(AROS_INCLUDES)/aros/i386/libcall.h | $(GENINCDIR)/aros/i386",
            "\t$(CP) $< $@",
            "$(HOSTGENDIR)/tools/gencall_i386: $(SRCDIR)/$(CURDIR)/gencall.c",
            "\t@$(HOST_CC) -Wall -Werror -o $@ $<",
        ],
    );

    let dirs = DirVars::load(&source_root);
    let target = TargetContext {
        cpu: Some("riscv".into()),
        platform: Some("esp32p4".into()),
        family: Some(String::new()),
        variant: Some(String::new()),
        toolchain: Some("gnu".into()),
        cpu32: Some(String::new()),
        use_mmu: Some("0".into()),
        float_abi: Some("ilp32f".into()),
        ..TargetContext::default()
    };
    let parsed_m68k =
        parse_mmakefile_with_dirs_and_context(&m68k_makefile, &source_root, &dirs, &target)
            .expect("parse exact P4 m68k include makefile");
    let parsed_i386 =
        parse_mmakefile_with_dirs_and_context(&i386_makefile, &source_root, &dirs, &target)
            .expect("parse exact P4 i386 include makefile");
    let m68k = only_aggregate(&parsed_m68k.host_header_aggregates, M68K_OWNER);
    let i386 = only_aggregate(&parsed_i386.host_header_aggregates, I386_OWNER);

    assert_aggregate(
        m68k,
        M68K_OWNER,
        M68K_TOOL,
        &source_root,
        M68K_SOURCE,
        &[
            ("aros/m68k/asmcall.h", &["asmcall"], false),
            ("aros/m68k/libcall.h", &["libcall"], false),
        ],
    );
    assert_aggregate(
        i386,
        I386_OWNER,
        I386_TOOL,
        &source_root,
        I386_SOURCE,
        &[("aros/i386/libcall.h", &[], true)],
    );

    let mut graph = DependencyGraph::default();
    graph
        .host_header_aggregates
        .extend([(*m68k).clone(), (*i386).clone()]);
    let generated = generate_cmake(&graph);
    let aggregate_calls = extract_calls(&generated, "aros_host_header_aggregate(\n");
    let output_calls = extract_calls(&generated, "aros_host_header_aggregate_output(\n");
    assert_eq!(aggregate_calls.len(), 2, "{generated}");
    assert_eq!(output_calls.len(), 3, "{generated}");

    let fixture = tempfile::Builder::new()
        .prefix("aros host header aggregate ")
        .tempdir()
        .expect("private host-header aggregate fixture");
    let root = fixture
        .path()
        .canonicalize()
        .expect("canonical fixture root");
    let project = root.join("project");
    let build = root.join("build");
    let direct_tools = root.join("independent host tools");
    fs::create_dir_all(&project).expect("create minimal CMake project");
    fs::create_dir_all(&direct_tools).expect("create independent compiler output directory");
    let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aros-cmake-engine/engine")
        .canonicalize()
        .expect("CMake engine directory");
    let host_cc = host_c_compiler();
    require_success(
        &Command::new(&host_cc)
            .arg("--version")
            .output()
            .expect("find host C compiler"),
        "identify host C compiler",
    );
    let mut calls = String::new();
    for call in aggregate_calls.iter().chain(&output_calls) {
        calls.push_str(call);
        calls.push('\n');
    }
    let cmake_lists = format!(
        r#"cmake_minimum_required(VERSION 3.22)
project(actual_p4_gencall_aggregates NONE)
include([==[{}]==])
set(AROS_SOURCE_DIR [==[{}]==])
set(AROS_SDK_INCLUDE_DIR "${{CMAKE_BINARY_DIR}}/SDK/include")
set(AROS_GENINC_DIR "${{CMAKE_BINARY_DIR}}/GENINCDIR")
set(AROS_HOST_CC [==[{}]==])
{calls}
"#,
        engine.join("HostHeaderAggregates.cmake").display(),
        source_root.display(),
        host_cc.display(),
    );
    fs::write(project.join("CMakeLists.txt"), cmake_lists)
        .expect("write isolated CMake project with only aggregate calls");

    require_success(
        &Command::new("cmake")
            .arg("-S")
            .arg(&project)
            .arg("-B")
            .arg(&build)
            .args(["-G", "Ninja"])
            .output()
            .expect("configure host-header aggregate fixture"),
        "configure generated host-header aggregate calls",
    );

    let m68k_tool = direct_tools.join(M68K_TOOL);
    let i386_tool = direct_tools.join(I386_TOOL);
    compile_reference_tool(&host_cc, &m68k_source, &m68k_tool);
    compile_reference_tool(&host_cc, &i386_source, &i386_tool);
    let expected_headers = BTreeMap::from([
        (
            "aros/m68k/asmcall.h".to_owned(),
            run_reference_generator(&m68k_tool, &["asmcall"]),
        ),
        (
            "aros/m68k/libcall.h".to_owned(),
            run_reference_generator(&m68k_tool, &["libcall"]),
        ),
        (
            "aros/i386/libcall.h".to_owned(),
            run_reference_generator(&i386_tool, &[]),
        ),
    ]);

    build_owner(&build, M68K_OWNER);
    build_owner(&build, I386_OWNER);
    assert_generated_headers(&build, &expected_headers);
    report_generated_header_inventory(&build, &expected_headers);
    assert!(
        !build.join("GENINCDIR/aros/m68k/asmcall.h").exists()
            && !build.join("GENINCDIR/aros/m68k/libcall.h").exists(),
        "m68k outputs must remain SDK-only"
    );

    let tracked_headers = [
        build.join("SDK/include/aros/m68k/asmcall.h"),
        build.join("SDK/include/aros/m68k/libcall.h"),
        build.join("SDK/include/aros/i386/libcall.h"),
        build.join("GENINCDIR/aros/i386/libcall.h"),
    ];
    let mtimes_before_no_op = modified_times(&tracked_headers);
    build_owner(&build, M68K_OWNER);
    build_owner(&build, I386_OWNER);
    assert_eq!(
        modified_times(&tracked_headers),
        mtimes_before_no_op,
        "no-op aggregate builds changed header mtimes"
    );

    let missing_output = build.join("SDK/include/aros/m68k/asmcall.h");
    fs::remove_file(&missing_output).expect("remove fixture-generated m68k output");
    build_owner(&build, M68K_OWNER);
    assert_eq!(
        fs::read(&missing_output).expect("read repaired m68k output"),
        expected_headers["aros/m68k/asmcall.h"],
        "missing aggregate output was not repaired"
    );

    #[cfg(unix)]
    refuse_future_symlink_tool_without_touching_outside_bytes(&build, &root, M68K_OWNER, M68K_TOOL);

    #[cfg(unix)]
    refuse_symlink_output_without_touching_outside_bytes(
        &build,
        &root,
        I386_OWNER,
        "aros/i386/libcall.h",
        &expected_headers["aros/i386/libcall.h"],
    );

    #[cfg(unix)]
    assert_generated_headers(&build, &expected_headers);

    refuse_tampered_recipe_without_touching_output(
        &build,
        M68K_OWNER,
        "aros/m68k/libcall.h",
        &expected_headers["aros/m68k/libcall.h"],
    );
}

fn required_source_root() -> PathBuf {
    let value = std::env::var_os("AROS_TEST_P4_SOURCE_ROOT")
        .unwrap_or_else(|| panic!("AROS_TEST_P4_SOURCE_ROOT must be set explicitly"));
    PathBuf::from(value)
        .canonicalize()
        .unwrap_or_else(|error| panic!("AROS_TEST_P4_SOURCE_ROOT must name a directory: {error}"))
}

fn assert_source_recipe(path: &Path, exact_facts: &[&str]) {
    let contents = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read actual P4 Make recipe {}: {error}", path.display()))
        .replace("\r\n", "\n");
    for fact in exact_facts {
        assert!(
            contents.contains(fact),
            "actual P4 source fact changed in {}: {fact:?}",
            path.display()
        );
    }
}

fn only_aggregate<'a>(
    declarations: &'a [HostHeaderAggregateDecl],
    owner: &str,
) -> &'a HostHeaderAggregateDecl {
    let matches = declarations
        .iter()
        .filter(|declaration| declaration.owner == owner)
        .collect::<Vec<_>>();
    let [declaration] = matches.as_slice() else {
        panic!(
            "expected exactly one {owner} aggregate, got {}; declarations={declarations:#?}",
            matches.len()
        );
    };
    declaration
}

fn assert_aggregate(
    declaration: &HostHeaderAggregateDecl,
    owner: &str,
    tool: &str,
    source_root: &Path,
    expected_source: &str,
    expected_headers: &[(&str, &[&str], bool)],
) {
    assert_eq!(declaration.owner, owner, "{declaration:#?}");
    assert_eq!(declaration.tool, tool, "{declaration:#?}");
    assert_eq!(
        declaration.host_compile_flags,
        vec!["-Wall".to_owned(), "-Werror".to_owned()]
    );
    let actual_source = resolve_tool_source(source_root, &declaration.tool_source)
        .canonicalize()
        .unwrap_or_else(|error| {
            panic!(
                "cannot resolve parsed host tool source {:?}: {error}",
                declaration.tool_source
            )
        });
    let expected_source = source_root
        .join(expected_source)
        .canonicalize()
        .expect("canonical actual P4 tool source");
    assert_eq!(actual_source, expected_source, "{declaration:#?}");

    let actual_headers = declaration
        .headers
        .iter()
        .map(|header| {
            (
                header.header.clone(),
                header.arguments.clone(),
                header.generated_mirror,
            )
        })
        .collect::<BTreeSet<_>>();
    let expected_headers = expected_headers
        .iter()
        .map(|(header, arguments, mirror)| {
            (
                (*header).to_owned(),
                arguments
                    .iter()
                    .map(|argument| (*argument).to_owned())
                    .collect(),
                *mirror,
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_headers, expected_headers, "{declaration:#?}");
}

fn resolve_tool_source(source_root: &Path, tool_source: &str) -> PathBuf {
    let source = tool_source
        .strip_prefix("${AROS_SOURCE_DIR}/")
        .or_else(|| tool_source.strip_prefix("$AROS_SOURCE_DIR/"))
        .unwrap_or(tool_source);
    let path = Path::new(source);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        source_root.join(path)
    }
}

fn extract_calls(generated: &str, signature: &str) -> Vec<String> {
    let mut calls = Vec::new();
    let mut offset = 0;
    while let Some(relative_start) = generated[offset..].find(signature) {
        let start = offset + relative_start;
        let relative_end = generated[start..]
            .find(")\n\n")
            .unwrap_or_else(|| panic!("unterminated generated CMake call {signature:?}"));
        let end = start + relative_end + 1;
        calls.push(generated[start..end].to_owned());
        offset = end + 1;
    }
    calls
}

fn host_c_compiler() -> PathBuf {
    let executable = std::env::split_paths(&std::env::var_os("PATH").expect("PATH is set"))
        .map(|directory| directory.join("cc"))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("no cc executable found on PATH"));
    executable
        .canonicalize()
        .unwrap_or_else(|error| panic!("cannot resolve host cc executable: {error}"))
}

fn compile_reference_tool(host_cc: &Path, source: &Path, output: &Path) {
    let compile = Command::new(host_cc)
        .args(["-Wall", "-Werror", "-o"])
        .arg(output)
        .arg(source)
        .output()
        .unwrap_or_else(|error| panic!("compile {} independently: {error}", source.display()));
    require_success(
        &compile,
        &format!("compile {} with -Wall -Werror", source.display()),
    );
}

fn run_reference_generator(tool: &Path, arguments: &[&str]) -> Vec<u8> {
    let output = Command::new(tool)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("run independent generator {}: {error}", tool.display()));
    require_success(
        &output,
        &format!("run independent generator {}", tool.display()),
    );
    output.stdout
}

fn build_owner(build: &Path, owner: &str) {
    let output = build_owner_output(build, owner);
    require_success(&output, &format!("build aggregate owner {owner}"));
}

fn build_owner_output(build: &Path, owner: &str) -> Output {
    Command::new("cmake")
        .arg("--build")
        .arg(build)
        .args(["--target", owner])
        .output()
        .unwrap_or_else(|error| panic!("build aggregate owner {owner}: {error}"))
}

fn assert_generated_headers(build: &Path, expected_headers: &BTreeMap<String, Vec<u8>>) {
    for (header, expected) in expected_headers {
        let sdk_output = build.join("SDK/include").join(header);
        assert_eq!(
            fs::read(&sdk_output).unwrap_or_else(|error| {
                panic!(
                    "read generated SDK header {}: {error}",
                    sdk_output.display()
                )
            }),
            *expected,
            "generated SDK output differs from direct host-tool stdout: {header}"
        );
        if header == "aros/i386/libcall.h" {
            let mirror = build.join("GENINCDIR").join(header);
            assert_eq!(
                fs::read(&mirror).unwrap_or_else(|error| {
                    panic!(
                        "read generated GENINCDIR mirror {}: {error}",
                        mirror.display()
                    )
                }),
                *expected,
                "i386 generated mirror differs from direct host-tool stdout"
            );
        }
    }
}

fn report_generated_header_inventory(build: &Path, expected_headers: &BTreeMap<String, Vec<u8>>) {
    let mut outputs = BTreeMap::new();
    for (header, expected) in expected_headers {
        outputs.insert(format!("SDK/include/{header}"), expected.clone());
        if header == "aros/i386/libcall.h" {
            outputs.insert(format!("GENINCDIR/{header}"), expected.clone());
        }
    }
    eprintln!("generated host-header inventory (path, bytes, SHA256):");
    for (relative, expected) in outputs {
        let path = build.join(&relative);
        let measured = fs::read(&path)
            .unwrap_or_else(|error| panic!("read inventory output {}: {error}", path.display()));
        assert_eq!(measured, expected, "inventory bytes changed for {relative}");
        eprintln!(
            "{relative}\t{}\t{}",
            measured.len(),
            aros_common::sha256_bytes(&measured)
        );
    }
}

fn modified_times(paths: &[PathBuf]) -> Vec<(PathBuf, SystemTime)> {
    paths
        .iter()
        .map(|path| {
            (
                path.clone(),
                fs::metadata(path)
                    .unwrap_or_else(|error| panic!("inspect {}: {error}", path.display()))
                    .modified()
                    .unwrap_or_else(|error| panic!("read mtime for {}: {error}", path.display())),
            )
        })
        .collect()
}

#[cfg(unix)]
fn refuse_future_symlink_tool_without_touching_outside_bytes(
    build: &Path,
    root: &Path,
    owner: &str,
    tool_name: &str,
) {
    use std::os::unix::fs::symlink;

    let tool = build.join("hosttools").join(tool_name);
    let recipe = build
        .join(".aros-host-header-aggregates")
        .join(format!("{owner}-tool.cmake"));
    let recipe_before = fs::read(&recipe).expect("read configured host-tool recipe");
    let recipe_mtime_before = fs::metadata(&recipe)
        .expect("inspect configured host-tool recipe")
        .modified()
        .expect("read configured host-tool recipe mtime");
    let outside = root.join("outside host-tool sentinel.bin");
    let sentinel = b"outside host-tool bytes must remain unchanged\n";
    fs::write(&outside, sentinel).expect("write private host-tool sentinel");
    fs::remove_file(&tool).expect("remove fixture-generated host tool before symlink test");
    symlink(&outside, &tool).expect("replace only fixture host tool with an outside symlink");
    let future_mtime = set_future_mtime(&outside);
    assert_eq!(
        fs::metadata(&tool)
            .expect("stat future-dated host-tool symlink target")
            .modified()
            .expect("read future-dated host-tool mtime"),
        future_mtime,
        "host-tool symlink should appear newer than every build input"
    );

    let output = build_owner_output(build, owner);
    assert!(
        !output.status.success(),
        "future-dated symlink tool unexpectedly built"
    );
    let diagnostic = combined_output(&output);
    assert!(
        diagnostic.contains("symlink"),
        "future-dated host-tool symlink was rejected without a symlink diagnostic:\n{diagnostic}"
    );
    assert_eq!(
        fs::read(&outside).expect("read outside-output sentinel after refusal"),
        sentinel,
        "helper modified bytes outside the build root"
    );
    assert!(
        fs::symlink_metadata(&tool)
            .expect("inspect rejected host-tool link")
            .file_type()
            .is_symlink(),
        "helper replaced the rejected host-tool symlink"
    );
    assert_eq!(
        fs::read(&recipe).expect("reread host-tool recipe"),
        recipe_before
    );
    assert_eq!(
        fs::metadata(&recipe)
            .expect("restat host-tool recipe")
            .modified()
            .expect("reread host-tool recipe mtime"),
        recipe_mtime_before,
        "host-tool symlink check changed its recipe timestamp"
    );

    fs::remove_file(&tool).expect("remove rejected fixture host-tool symlink");
    build_owner(build, owner);
}

#[cfg(unix)]
fn refuse_symlink_output_without_touching_outside_bytes(
    build: &Path,
    root: &Path,
    owner: &str,
    header: &str,
    expected_header: &[u8],
) {
    use std::os::unix::fs::symlink;

    let mirror = build.join("GENINCDIR").join(header);
    let recipe = output_recipe(build, owner, header);
    let primary = build.join("SDK/include").join(header);
    let primary_before = fs::read(&primary).expect("read SDK primary before mirror test");
    let recipe_before = fs::read(&recipe).expect("read configured output recipe");
    let recipe_mtime_before = fs::metadata(&recipe)
        .expect("inspect configured output recipe")
        .modified()
        .expect("read configured output recipe mtime");
    let outside = root.join("outside output sentinel.bin");
    let sentinel = b"outside bytes must remain unchanged\n";
    fs::write(&outside, sentinel).expect("write private outside-output sentinel");
    fs::remove_file(&mirror).expect("remove fixture-generated mirror before symlink test");
    symlink(&outside, &mirror).expect("replace only fixture mirror with an outside symlink");
    let future_mtime = set_future_mtime(&outside);
    assert_eq!(
        fs::metadata(&mirror)
            .expect("stat future-dated output symlink target")
            .modified()
            .expect("read future-dated output mtime"),
        future_mtime,
        "output symlink should appear newer than every build input"
    );

    let output = build_owner_output(build, owner);
    assert!(
        !output.status.success(),
        "future-dated symlink output unexpectedly built"
    );
    let diagnostic = combined_output(&output);
    assert!(
        diagnostic.contains("symlink"),
        "future-dated symlink output was rejected without a symlink diagnostic:\n{diagnostic}"
    );
    assert_eq!(
        fs::read(&outside).expect("read outside-output sentinel after refusal"),
        sentinel,
        "helper modified bytes outside the build root"
    );
    assert_eq!(
        fs::read(&primary).expect("read SDK primary after mirror refusal"),
        primary_before,
        "failed mirror validation changed the SDK primary"
    );
    assert_eq!(
        fs::read(&recipe).expect("reread output recipe"),
        recipe_before
    );
    assert_eq!(
        fs::metadata(&recipe)
            .expect("restat output recipe")
            .modified()
            .expect("reread output recipe mtime"),
        recipe_mtime_before,
        "mirror symlink check changed its recipe timestamp"
    );
    assert!(
        fs::symlink_metadata(&mirror)
            .expect("inspect rejected output link")
            .file_type()
            .is_symlink(),
        "helper replaced the rejected output symlink"
    );

    fs::remove_file(&mirror).expect("remove rejected fixture mirror symlink");
    build_owner(build, owner);
    assert_eq!(
        fs::read(&primary).expect("read SDK primary after mirror retry"),
        expected_header,
        "mirror retry changed the independently verified SDK header"
    );
    assert_eq!(
        fs::read(&mirror).expect("read repaired GENINCDIR mirror"),
        expected_header,
        "mirror retry did not publish the exact SDK bytes"
    );
}

fn refuse_tampered_recipe_without_touching_output(
    build: &Path,
    owner: &str,
    header: &str,
    expected_output: &[u8],
) {
    let recipe = output_recipe(build, owner, header);
    let primary = build.join("SDK/include").join(header);
    let recipe_bytes = fs::read(&recipe).expect("read configured output recipe");
    let primary_mtime_before = fs::metadata(&primary)
        .expect("inspect header before recipe tamper")
        .modified()
        .expect("read header mtime before recipe tamper");
    fs::write(
        &recipe,
        [recipe_bytes.as_slice(), b"\n# post-configure tampering\n"].concat(),
    )
    .expect("tamper only the temporary output recipe");
    let recipe_mtime = set_future_mtime(&recipe);
    assert!(
        recipe_mtime > primary_mtime_before,
        "tampered recipe must be newer than the existing header"
    );

    let output = build_owner_output(build, owner);
    assert!(
        !output.status.success(),
        "tampered output recipe unexpectedly ran"
    );
    let diagnostic = combined_output(&output);
    assert!(
        diagnostic.contains("recipe differs from configure-time contract"),
        "tampered recipe refusal diagnostic missing:\n{diagnostic}"
    );
    assert_eq!(
        fs::read(&primary).expect("read output after recipe-tamper refusal"),
        expected_output,
        "tampered recipe modified the generated header"
    );
}

fn output_recipe(build: &Path, owner: &str, header: &str) -> PathBuf {
    let id = aros_common::sha256_bytes(format!("{owner}/{header}").as_bytes()).to_string();
    let path = build
        .join(".aros-host-header-aggregates")
        .join(format!("{id}.cmake"));
    assert!(
        path.is_file(),
        "missing aggregate output recipe {}",
        path.display()
    );
    path
}

fn set_future_mtime(path: &Path) -> SystemTime {
    let future = SystemTime::now() + Duration::from_secs(120);
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("open {} to set future mtime: {error}", path.display()))
        .set_times(FileTimes::new().set_modified(future))
        .unwrap_or_else(|error| panic!("set future mtime on {}: {error}", path.display()));
    future
}

fn require_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed ({}):\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
