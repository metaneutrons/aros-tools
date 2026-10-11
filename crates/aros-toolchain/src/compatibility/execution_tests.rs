//! Legacy LLVM and receipt regression coverage for native compatibility.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use aros_common::{
    run_output, run_status, sha256_file, ArosToolchainManifest, CancellationToken, Sha256Digest,
};
use flate2::{write::GzEncoder, Compression};
use serde_json::json;
use tar::{Builder, Header};

use super::{
    execute_native_compatibility, execute_native_compatibility_with_readback,
    CompatibilityReceiptDocument, CompatibilityReceiptPortsSource, NativeCompatibilityRequest,
    StandaloneFixtures, COMPATIBILITY_RECEIPT_SCHEMA,
};
use crate::compatibility::{
    prepare, prepare_host_tool_closure, CompatibilityHostTool, CompatibilityPreparationRequest,
    HostToolClosureRequest, TwoRootRelocation, CXX_COLLECTOR_SYMBOL, C_COLLECTOR_SYMBOL,
    REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS,
};
use crate::compatibility_ports::{materialize, CompatibilityPortsLock, CompatibilityPortsSources};
use crate::package_extract::ExtractedPackage;
use crate::package_verify::VerifiedPackage;
use crate::profiles::Profiles;
use crate::python_environment::PythonEnvironment;
use crate::source_lock::SourceLock;

#[test]
fn receipt_accepts_safe_nested_upstream_fetch_markers() {
    let digest = Sha256Digest::parse(&"a".repeat(64)).unwrap();
    let document = CompatibilityReceiptDocument {
        schema: COMPATIBILITY_RECEIPT_SCHEMA.into(),
        operation: "native-compatibility".into(),
        upstream_source_commit: "b".repeat(40),
        upstream_source_tree: "c".repeat(40),
        ports_sources: vec![CompatibilityReceiptPortsSource {
            id: "codesets-6-22".into(),
            cache_filename: "codesets-6.22.tar.gz".into(),
            relative_path: "codesets/6.22.tar.gz".into(),
            fetch_marker: "codesets/.6.22-fetched".into(),
            sha256: digest,
            size: 1,
        }],
        phase_reports: Vec::new(),
        standalone_targets: BTreeMap::new(),
        package: None,
        native_sdk: None,
    };

    let error = document.validate().unwrap_err();
    assert!(error.to_string().contains("required ordered phase set"));
}

#[test]
fn executes_every_phase_with_two_roots_and_closed_environments() {
    let temporary = tempfile::tempdir().unwrap();
    let (request, cmake_log, make_log) = request(temporary.path());
    let profiles = fixture_profiles(request.upstream_source_commit.as_str());
    let report = execute_native_compatibility_with_readback(
        &request,
        &profiles,
        &CancellationToken::default(),
    )
    .unwrap();

    assert_eq!(report.probes.reports.len(), 6);
    assert!(
        report.native_sdk.is_none(),
        "legacy execution must not gain a source-v2 SDK gate"
    );
    assert!(report
        .probes
        .reports
        .values()
        .all(|probe| !probe.commands.is_empty()));
    assert_eq!(
        report.probes.reports[&crate::compatibility::CompatibilityPhase::CmakeConsumer]
            .host_tools
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS
    );
    assert_eq!(
        report.probes.reports[&crate::compatibility::CompatibilityPhase::UpstreamConfigure]
            .host_tools
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS
    );
    assert_eq!(report.standalone.targets.len(), 2);
    assert!(report
        .standalone
        .targets
        .contains_key("x86_64-unknown-aros"));
    assert!(report.standalone.targets.contains_key("i386-unknown-aros"));
    assert!(report.receipt.path.is_file());
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&report.receipt.path).unwrap()).unwrap();
    assert_eq!(
        receipt["schema"],
        "aros-toolchain-native-compatibility-receipt-v2"
    );
    assert_eq!(receipt["phase_reports"].as_array().unwrap().len(), 6);
    assert_eq!(receipt["ports_sources"].as_array().unwrap().len(), 3);
    assert_eq!(receipt["standalone_targets"].as_object().unwrap().len(), 2);
    assert_eq!(
        aros_common::sha256_file(&report.receipt.path)
            .unwrap()
            .digest,
        report.receipt.sha256
    );

    let cmake_arguments = fs::read_to_string(cmake_log).unwrap();
    assert!(cmake_arguments.contains("-S"));
    assert!(cmake_arguments.contains("aros-cmake-engine"));
    assert!(cmake_arguments.contains("AROS_SOURCE_DIR="));
    assert!(cmake_arguments.contains(&format!(
        "-DAROS_HOST_CC={}",
        request.host_tools.root.join("cc").display()
    )));
    assert!(cmake_arguments.contains("-DAROS_COMPILER_CACHE_MODE=off"));
    assert!(cmake_arguments.contains("-DAROS_FETCH_OFFLINE=ON"));
    let make_arguments = fs::read_to_string(make_log).unwrap();
    assert!(make_arguments.contains("includes"));
    assert!(make_arguments.contains("linklibs"));
}

#[test]
fn rejects_a_reused_native_output_root_before_any_child_starts() {
    let temporary = tempfile::tempdir().unwrap();
    let (request, _, _) = request(temporary.path());
    fs::create_dir(&request.reports_root).unwrap();
    let error = execute_native_compatibility(&request, &CancellationToken::default()).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        aros_common::DiagnosticCode::ProducerCompatibility
    );
}

#[test]
fn rejects_a_missing_measured_host_c_compiler_before_cmake_starts() {
    let temporary = tempfile::tempdir().unwrap();
    let (mut request, cmake_log, _) = request(temporary.path());
    let make = request.host_tools.tools["make"].program.clone();
    let python3 = request.host_tools.tools["python3"].program.clone();
    request.host_tools = prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: temporary.path().join("host-tools-without-cc"),
        tools: vec![
            CompatibilityHostTool {
                name: "make".into(),
                program: make,
            },
            CompatibilityHostTool {
                name: "python3".into(),
                program: python3,
            },
        ],
    })
    .unwrap();

    let error = execute_native_compatibility(&request, &CancellationToken::default()).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        aros_common::DiagnosticCode::ProducerCompatibility
    );
    assert!(!cmake_log.exists());
}

#[test]
fn rejects_missing_archive_validation_tools_before_any_phase_starts() {
    for missing in ["od", "shasum"] {
        let temporary = tempfile::tempdir().unwrap();
        let (mut request, cmake_log, make_log) = request(temporary.path());
        let tools = request
            .host_tools
            .tools
            .iter()
            .filter(|(name, _)| name.as_str() != missing)
            .map(|(name, identity)| CompatibilityHostTool {
                name: name.clone(),
                program: identity.program.clone(),
            })
            .collect();
        request.host_tools = prepare_host_tool_closure(&HostToolClosureRequest {
            output_root: temporary.path().join("incomplete-host-tools"),
            tools,
        })
        .unwrap();
        let error =
            execute_native_compatibility(&request, &CancellationToken::default()).unwrap_err();
        assert!(error.to_string().contains(&format!("missing {missing}")));
        assert!(!cmake_log.exists());
        assert!(!make_log.exists());
        assert!(!request.reports_root.exists());
    }
}

#[test]
fn rejects_an_unselected_measured_host_tool_before_cmake_starts() {
    let temporary = tempfile::tempdir().unwrap();
    let (mut request, cmake_log, _) = request(temporary.path());
    let mut tools = request
        .host_tools
        .tools
        .iter()
        .map(|(name, identity)| CompatibilityHostTool {
            name: name.clone(),
            program: identity.program.clone(),
        })
        .collect::<Vec<_>>();
    let extra = temporary.path().join("unexpected-host-tool");
    script(&extra, "exit 0");
    tools.push(CompatibilityHostTool {
        name: "unexpected".into(),
        program: extra,
    });
    request.host_tools = prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: temporary.path().join("host-tools-with-unexpected-entry"),
        tools,
    })
    .unwrap();

    let error = execute_native_compatibility(&request, &CancellationToken::default()).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        aros_common::DiagnosticCode::ProducerCompatibility
    );
    assert!(!cmake_log.exists());
}

#[test]
fn rejects_a_dirty_or_mismatched_pristine_upstream_source_before_any_child_starts() {
    let temporary = tempfile::tempdir().unwrap();
    let (dirty, _, _) = request(temporary.path());
    fs::write(dirty.upstream_source_root.join("configure"), "exit 0\n").unwrap();
    assert!(execute_native_compatibility(&dirty, &CancellationToken::default()).is_err());
    assert!(!dirty.reports_root.exists());

    let temporary = tempfile::tempdir().unwrap();
    let (mut mismatched, _, _) = request(temporary.path());
    mismatched.upstream_source_commit =
        crate::recipe::GitObjectId::try_from("f".repeat(40)).unwrap();
    assert!(execute_native_compatibility(&mismatched, &CancellationToken::default()).is_err());
    assert!(!mismatched.reports_root.exists());
}

pub(super) fn request(root: &Path) -> (NativeCompatibilityRequest, PathBuf, PathBuf) {
    let source = root.join("engine-free-source");
    let engine_work = root.join("engine-work");
    let helpers = root.join("helpers");
    for directory in [&source, &engine_work, &helpers] {
        fs::create_dir(directory).unwrap();
    }
    for helper in crate::compatibility::REQUIRED_HELPERS {
        script(&helpers.join(helper), "exit 0");
    }
    let preparation = prepare(&CompatibilityPreparationRequest {
        source_root: source,
        work_root: engine_work,
        helpers_root: helpers,
    })
    .unwrap();

    let first = root.join("first-toolchain");
    let second = root.join("second-toolchain");
    for toolchain in [&first, &second] {
        fs::create_dir(toolchain).unwrap();
        fs::create_dir(toolchain.join("bin")).unwrap();
    }
    let c_elf = root.join("c.elf");
    let cxx_elf = root.join("cxx.elf");
    let c_elf32 = root.join("c-i386.elf");
    let cxx_elf32 = root.join("cxx-i386.elf");
    fs::write(&c_elf, fixture_elf64(C_COLLECTOR_SYMBOL)).unwrap();
    fs::write(&cxx_elf, fixture_elf64(CXX_COLLECTOR_SYMBOL)).unwrap();
    fs::write(&c_elf32, fixture_elf32(C_COLLECTOR_SYMBOL)).unwrap();
    fs::write(&cxx_elf32, fixture_elf32(CXX_COLLECTOR_SYMBOL)).unwrap();
    for toolchain in [&first, &second] {
        script(
            &toolchain.join("bin/clang"),
            &format!(
                "out=\ncase \" $* \" in *' --target=i386-unknown-aros '*) fixture='{}';; *) fixture='{}';; esac\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = -o ]; then shift; out=$1; fi\n  shift\ndone\n/bin/cp \"$fixture\" \"$out\"",
                c_elf32.display(),
                c_elf.display(),
            ),
        );
        script(
            &toolchain.join("bin/clang++"),
            &format!(
                "out=\ncase \" $* \" in *' --target=i386-unknown-aros '*) fixture='{}';; *) fixture='{}';; esac\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = -o ]; then shift; out=$1; fi\n  shift\ndone\n/bin/cp \"$fixture\" \"$out\"",
                cxx_elf32.display(),
                cxx_elf.display(),
            ),
        );
    }
    let verified = VerifiedPackage {
        manifest: manifest(),
        archive_sha256: aros_common::Sha256Digest::parse(&"a".repeat(64)).unwrap(),
        archive_size: 1,
    };
    let relocation = TwoRootRelocation {
        first: ExtractedPackage {
            root: first,
            verified: verified.clone(),
        },
        second: ExtractedPackage {
            root: second,
            verified,
        },
    };
    let ports_sources = ports_sources(root);

    let upstream = root.join("upstream-source");
    fs::create_dir(&upstream).unwrap();
    script(
        &upstream.join("configure"),
        &format!(
            "[ \"$PATH\" != /nonexistent ] || exit 20\n[ \"${{ac_cv_prog_cc_c23+x}}\" = x ] && [ -z \"$ac_cv_prog_cc_c23\" ] || exit 21\n[ \"$AROS_FETCH_OFFLINE\" = 1 ] || exit 23\ncase \" $* \" in *\" --with-portssources={} \"*) ;; *) exit 22;; esac\npython3 -S -P -c 'import mako, markupsafe'",
            ports_sources.root.display(),
        ),
    );
    git(&upstream, &["init", "-q"]);
    git(&upstream, &["config", "user.email", "test@example.invalid"]);
    git(&upstream, &["config", "user.name", "AROS Tools Test"]);
    git(&upstream, &["add", "configure"]);
    git(
        &upstream,
        &["commit", "-qm", "test: pristine upstream source"],
    );
    let upstream_commit = git_output(&upstream, &["rev-parse", "HEAD"]);
    let cmake_log = root.join("cmake-arguments.log");
    let cmake = root.join("cmake");
    script(
        &cmake,
        &format!("printf '%s\\n' \"$@\" > '{}'", cmake_log.display()),
    );
    let ninja = root.join("ninja");
    script(&ninja, "exit 0");
    let make_log = root.join("make-arguments.log");
    let make = root.join("make");
    script(
        &make,
        &format!("printf '%s\\n' \"$@\" >> '{}'", make_log.display()),
    );
    let cc = root.join("cc");
    script(&cc, "exit 0");
    let python = python_environment(root);
    let mut closure_tools = Vec::new();
    for role in crate::compatibility::REQUIRED_NATIVE_COMPATIBILITY_HOST_TOOLS {
        let program = match *role {
            "cc" => cc.clone(),
            "make" => make.clone(),
            "python3" => python.interpreter().path.clone(),
            role => {
                let program = root.join(format!("host-{role}"));
                script(&program, "exit 0");
                program
            }
        };
        closure_tools.push(CompatibilityHostTool {
            name: (*role).into(),
            program,
        });
    }
    let closure = prepare_host_tool_closure(&HostToolClosureRequest {
        output_root: root.join("host-tools"),
        tools: closure_tools,
    })
    .unwrap();
    let c_fixture = root.join("smoke.c");
    let cxx_fixture = root.join("smoke.cpp");
    fs::write(&c_fixture, b"int main(void) { return 0; }\n").unwrap();
    fs::write(&cxx_fixture, b"int main() { return 0; }\n").unwrap();
    let profiles = fixture_profiles(&upstream_commit);
    (
        NativeCompatibilityRequest {
            preparation,
            relocation,
            profile: profiles.select("pc-x86_64").unwrap().clone(),
            package_source_commit: None,
            source_preset: None,
            host_generator_cache_root: None,
            cmake_program: cmake,
            ninja_program: ninja,
            cmake_build_root: root.join("cmake-build"),
            upstream_source_root: upstream,
            upstream_source_commit: profiles.upstream_commit().clone(),
            upstream_build_root: root.join("upstream-build"),
            host_python: python,
            host_tools: closure,
            ports_sources,
            host: "linux-x86_64".into(),
            make_jobs: 2,
            standalone_fixtures: StandaloneFixtures {
                c: c_fixture,
                cxx: cxx_fixture,
            },
            standalone_output_root: root.join("standalone"),
            reports_root: root.join("reports"),
            // Include fixture I/O and executable revalidation under parallel test load.
            // Deadline counterprobes select their own shorter phase budgets.
            timeout: Duration::from_secs(30),
        },
        cmake_log,
        make_log,
    )
}

pub(super) fn fixture_profiles(upstream_commit: &str) -> Profiles {
    Profiles::parse(
        serde_json::to_vec(&serde_json::json!({
            "schema": "aros-toolchain-profiles-v1",
            "upstream_commit": upstream_commit,
            "profiles": [{
                "name": "pc-x86_64", "configure_target": "pc-x86_64",
                "upstream_output_target": "pc-x86_64",
                "target_triple": "x86_64-unknown-aros", "cpu": "x86_64",
                "platform": "pc", "float_abi": "",
                "capabilities": ["c", "cxx", "standalone-collector"]
            }]
        }))
        .unwrap()
        .as_slice(),
    )
    .unwrap()
}

fn git(root: &Path, arguments: &[&str]) {
    let mut command = Command::new("git");
    command.args(arguments).current_dir(root);
    let status = run_status(&mut command).unwrap();
    assert!(status.status.success());
}

fn git_output(root: &Path, arguments: &[&str]) -> String {
    let mut command = Command::new("git");
    command.args(arguments).current_dir(root);
    let output = run_output(&mut command).unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout.exact_bytes().unwrap().to_vec())
        .unwrap()
        .trim()
        .to_owned()
}

fn manifest() -> ArosToolchainManifest {
    ArosToolchainManifest {
        schema: 1,
        release_id: "toolchain-v1-test".into(),
        host: "linux-x86_64".into(),
        target_profile: "pc-x86_64".into(),
        target_triple: "x86_64-unknown-aros".into(),
        tree_sha256: "b".repeat(64),
        llvm_version: Some("11.0.0".into()),
        compiler: None,
        recipe_sha256: "c".repeat(64),
        source_lock_sha256: "d".repeat(64),
        profiles_sha256: "e".repeat(64),
        source_commit: "1".repeat(40),
        producer_commit: "2".repeat(40),
        tools_commit: "3".repeat(40),
        source_date_epoch: 1,
        capabilities: vec!["c".into(), "cxx".into()],
        build_environment: serde_json::Map::default(),
        files: Vec::new(),
    }
}

fn python_environment(root: &Path) -> PythonEnvironment {
    let cache = root.join("python-cache");
    fs::create_dir(&cache).unwrap();
    archive(
        &cache.join("mako-1.3.10.tar.gz"),
        &[
            ("mako-1.3.10/mako/__init__.py", b"__version__ = '1.3.10'\n"),
            (
                "mako-1.3.10/mako/template.py",
                b"import markupsafe\nclass Template:\n def __init__(self, value): self.value = value\n def render(self): assert markupsafe.__version__ == '3.0.2'; return self.value\n",
            ),
        ],
    );
    archive(
        &cache.join("markupsafe-3.0.2.tar.gz"),
        &[(
            "markupsafe-3.0.2/src/markupsafe/__init__.py",
            b"__version__ = '3.0.2'\n",
        )],
    );
    let package =
        |name: &str, version: &str, filename: &str, source_root: &str, python_path: &str| {
            let measured = sha256_file(&cache.join(filename)).unwrap();
            json!({
                "name": name, "version": version, "filename": filename,
                "url": format!("https://example.invalid/{filename}"),
                "sha256": measured.digest, "size": measured.size,
                "source_root": source_root, "python_path": python_path
            })
        };
    let lock = SourceLock::parse(
        &serde_json::to_vec(&json!({
            "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
            "sources": [{
                "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
                "filename": "llvm.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
                "sha256": "a".repeat(64), "size": 1
            }],
            "host_python_packages": [
                package("mako", "1.3.10", "mako-1.3.10.tar.gz", "mako-1.3.10", "."),
                package("markupsafe", "3.0.2", "markupsafe-3.0.2.tar.gz", "markupsafe-3.0.2", "src")
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    PythonEnvironment::prepare(&lock, &cache, &root.join("python-environment")).unwrap()
}

fn ports_sources(root: &Path) -> CompatibilityPortsSources {
    let cache = root.join("ports-cache");
    fs::create_dir(&cache).unwrap();
    let unicode = b"0000;<control>;Cc;0;BN;;;;;N;NULL;;;;\n";
    let special = b"# SpecialCasing-16.0.0.txt\n";
    let bzip2 = b"bzip2 source archive";
    fs::write(cache.join("UnicodeData.txt"), unicode).unwrap();
    fs::write(cache.join("SpecialCasing.txt"), special).unwrap();
    fs::write(cache.join("bzip2-1.0.8.tar.gz"), bzip2).unwrap();
    let measured = |id: &str, filename: &str| {
        let measured = sha256_file(&cache.join(filename)).unwrap();
        let url = if filename == "bzip2-1.0.8.tar.gz" {
            "https://sourceware.org/pub/bzip2/bzip2-1.0.8.tar.gz".to_owned()
        } else {
            format!("https://www.unicode.org/Public/16.0.0/ucd/{filename}")
        };
        json!({
            "id": id,
            "cache_filename": filename,
            "relative_path": filename,
            "cmake_cache_path": if filename == "bzip2-1.0.8.tar.gz" { Some("portssources/bzip2-1.0.8.tar.gz") } else { None },
            "fetch_marker": if filename == "bzip2-1.0.8.tar.gz" { ".bzip2-1.0.8-fetched" } else { "" },
            "url": url,
            "sha256": measured.digest,
            "size": measured.size,
        })
    };
    let lock = CompatibilityPortsLock::parse(
        serde_json::to_vec(&json!({
            "schema": "aros-toolchain-compatibility-ports-v3",
            "upstream_commit": "a".repeat(40),
            "inputs": [
                measured("unicode-data", "UnicodeData.txt"),
                measured("special-casing", "SpecialCasing.txt"),
                measured("bzip2", "bzip2-1.0.8.tar.gz"),
            ],
            "profiles": [{
                "name": "pc-x86_64",
                "inputs": ["unicode-data", "special-casing", "bzip2"],
            }],
        }))
        .unwrap()
        .as_slice(),
    )
    .unwrap();
    let upstream_commit = lock.upstream_commit().clone();
    materialize(
        &cache,
        &lock,
        &upstream_commit,
        "pc-x86_64",
        &root.join("ports-sources"),
    )
    .unwrap()
}

fn archive(path: &Path, entries: &[(&str, &[u8])]) {
    let file = fs::File::create(path).unwrap();
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = Builder::new(encoder);
    for (name, contents) in entries {
        let mut header = Header::new_gnu();
        header.set_size(u64::try_from(contents.len()).unwrap());
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, *contents).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap();
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

pub fn fixture_elf64(symbol: &str) -> Vec<u8> {
    let mut names = Vec::from([0_u8]);
    names.extend_from_slice(symbol.as_bytes());
    names.push(0);
    let section_offset = 64_usize;
    let section_size = 64_usize;
    let strtab_offset = section_offset + 3 * section_size;
    let symtab_offset = strtab_offset + names.len();
    let mut object = vec![0_u8; symtab_offset + 2 * 24];
    object[..4].copy_from_slice(b"\x7fELF");
    object[4] = 2;
    object[5] = 1;
    object[6] = 1;
    object[7] = aros_common::elf::OS_ABI_AROS;
    object[8] = aros_common::elf::AROS_ABI_VERSION;
    write_u32(&mut object, 0x14, 1);
    write_u64(&mut object, 0x28, section_offset as u64);
    write_u16(&mut object, 0x34, 64);
    write_u16(&mut object, 0x3a, section_size as u16);
    write_u16(&mut object, 0x3c, 3);
    let strtab = section_offset + section_size;
    write_u32(&mut object, strtab + 4, 3);
    write_u64(&mut object, strtab + 24, strtab_offset as u64);
    write_u64(&mut object, strtab + 32, names.len() as u64);
    write_u64(&mut object, strtab + 48, 1);
    let symtab = strtab + section_size;
    write_u32(&mut object, symtab + 4, 2);
    write_u64(&mut object, symtab + 24, symtab_offset as u64);
    write_u64(&mut object, symtab + 32, 48);
    write_u32(&mut object, symtab + 40, 1);
    write_u64(&mut object, symtab + 48, 8);
    write_u64(&mut object, symtab + 56, 24);
    object[strtab_offset..strtab_offset + names.len()].copy_from_slice(&names);
    let symbol_entry = symtab_offset + 24;
    write_u32(&mut object, symbol_entry, 1);
    object[symbol_entry + 4] = 0x10;
    write_u16(&mut object, symbol_entry + 6, 1);
    object
}

pub fn fixture_elf32(symbol: &str) -> Vec<u8> {
    let mut names = Vec::from([0_u8]);
    names.extend_from_slice(symbol.as_bytes());
    names.push(0);
    let section_offset = 52_usize;
    let section_size = 40_usize;
    let strtab_offset = section_offset + 3 * section_size;
    let symtab_offset = strtab_offset + names.len();
    let mut object = vec![0_u8; symtab_offset + 2 * 16];
    object[..4].copy_from_slice(b"\x7fELF");
    object[4] = 1;
    object[5] = 1;
    object[6] = 1;
    object[7] = aros_common::elf::OS_ABI_AROS;
    object[8] = aros_common::elf::AROS_ABI_VERSION;
    write_u32(&mut object, 0x14, 1);
    write_u32(&mut object, 0x20, section_offset as u32);
    write_u16(&mut object, 0x28, 52);
    write_u16(&mut object, 0x2e, section_size as u16);
    write_u16(&mut object, 0x30, 3);
    let strtab = section_offset + section_size;
    write_u32(&mut object, strtab + 4, 3);
    write_u32(&mut object, strtab + 16, strtab_offset as u32);
    write_u32(&mut object, strtab + 20, names.len() as u32);
    write_u32(&mut object, strtab + 32, 1);
    let symtab = strtab + section_size;
    write_u32(&mut object, symtab + 4, 2);
    write_u32(&mut object, symtab + 16, symtab_offset as u32);
    write_u32(&mut object, symtab + 20, 32);
    write_u32(&mut object, symtab + 24, 1);
    write_u32(&mut object, symtab + 36, 16);
    object[strtab_offset..strtab_offset + names.len()].copy_from_slice(&names);
    let symbol_entry = symtab_offset + 16;
    write_u32(&mut object, symbol_entry, 1);
    object[symbol_entry + 4] = 0x10;
    write_u16(&mut object, symbol_entry + 14, 1);
    object
}

fn write_u16(buffer: &mut [u8], offset: usize, value: u16) {
    buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(buffer: &mut [u8], offset: usize, value: u64) {
    buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
