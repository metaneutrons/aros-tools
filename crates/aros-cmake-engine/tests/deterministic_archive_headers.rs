#![cfg(unix)]

//! A static library built through `AROS.cmake` must not record who ran `ar`.
//!
//! An archive member header carries the uid, gid and mtime of the user. Two
//! hosts therefore built different bytes from identical objects, and the GNU
//! SDK package differed between the Linux runners (uid 1001) and macOS
//! (uid 501). The fixture builds one library through the engine rules and reads
//! the member headers back; a control archive made with the non-deterministic
//! modifier shows that the reader would notice a non-zero owner.

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const MEMBER_HEADER: usize = 60;

struct Member {
    name: String,
    date: String,
    uid: String,
    gid: String,
}

#[test]
fn engine_archives_carry_no_owner_or_date() {
    let Some(archiver) = deterministic_archiver() else {
        eprintln!("skipped: no GNU ar or llvm-ar with the D modifier on PATH");
        return;
    };
    let compiler = host_compiler();

    let temporary = tempfile::tempdir().expect("create fixture root");
    let root = fs::canonicalize(temporary.path()).expect("canonicalize fixture root");
    let (source, build) = write_fixture(&root);

    let configure = Command::new("cmake")
        .args(["-S"])
        .arg(&source)
        .arg("-B")
        .arg(&build)
        .args(["-G", "Ninja"])
        .arg(format!("-DCMAKE_C_COMPILER={}", compiler.display()))
        .arg(format!("-DCMAKE_AR={}", archiver.display()))
        .output()
        .expect("configure archive fixture");
    assert!(
        configure.status.success(),
        "configure failed:\n{}",
        output_text(&configure)
    );
    // The archive rule itself must ask for the deterministic mode. This holds on
    // every runner, including one whose ar already defaults to it.
    let rules = fs::read_to_string(build.join("CMakeFiles/rules.ninja"))
        .expect("read generated rules.ninja");
    assert!(
        rules.contains(" qcD ") || rules.contains(" qcD\n"),
        "static library rule does not use the deterministic ar modifier:\n{}",
        rules
            .lines()
            .filter(|line| line.contains("command") && line.contains("qc"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let outcome = Command::new("cmake")
        .arg("--build")
        .arg(&build)
        .args(["--target", "detlib"])
        .output()
        .expect("build archive fixture");
    assert!(
        outcome.status.success(),
        "build failed:\n{}",
        output_text(&outcome)
    );

    let archive = build.join("libdetlib.a");
    let members = read_members(&archive);
    assert!(
        members.iter().any(|member| member.name.ends_with(".o/")),
        "fixture archive has no object member: {:?}",
        members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>()
    );
    for member in &members {
        assert_eq!(
            (
                member.date.as_str(),
                member.uid.as_str(),
                member.gid.as_str()
            ),
            ("0", "0", "0"),
            "member {} records date/uid/gid",
            member.name
        );
    }

    // Control: the same objects through the non-deterministic modifier carry the
    // real owner. Skipped for root, whose uid is already 0.
    if effective_uid() != "0" {
        let object = fs::read_dir(build.join("CMakeFiles/detlib.dir"))
            .expect("list object directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "o"))
            .expect("fixture object file");
        let control = root.join("control.a");
        let made = Command::new(&archiver)
            .arg("qcU")
            .arg(&control)
            .arg(&object)
            .output()
            .expect("run ar with the non-deterministic modifier");
        assert!(
            made.status.success(),
            "control ar failed:\n{}",
            output_text(&made)
        );
        let owned = read_members(&control)
            .into_iter()
            .any(|member| member.uid != "0" || member.gid != "0");
        assert!(
            owned,
            "control archive shows no owner, so this test cannot tell the modes apart"
        );
    }
}

fn write_fixture(root: &Path) -> (PathBuf, PathBuf) {
    let source = root.join("source");
    let build = root.join("build");
    fs::create_dir_all(source.join("compiler/startup")).expect("create startup directory");
    fs::create_dir_all(source.join("fixture-modules")).expect("create module directory");
    fs::create_dir_all(&build).expect("create build directory");
    fs::write(
        source.join("archive_c.c"),
        "int archive_c(void) { return 73; }\n",
    )
    .expect("write archive source");
    fs::write(
        source.join("compiler/startup/startup.c"),
        "int fixture_startup;\n",
    )
    .expect("write startup source");
    fs::write(
        source.join("compiler/startup/detach.c"),
        "int fixture_detach;\n",
    )
    .expect("write detach source");
    fs::write(
        source.join("fixture-modules/BootstrapSDK.cmake"),
        "function(aros_bootstrap_sdk_includes)\nendfunction()\n",
    )
    .expect("write SDK bootstrap stub");

    let engine =
        fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("engine/AROS.cmake"))
            .expect("canonicalize AROS.cmake");
    let project = r#"cmake_minimum_required(VERSION 3.22)
project(DeterministicArchiveFixture LANGUAGES C)
file(REAL_PATH "${CMAKE_C_COMPILER}" _fixture_real_c_compiler)
set(CMAKE_C_COMPILER "${_fixture_real_c_compiler}")
set(AROS_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}")
set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_LLD_BIN FALSE)
set(CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/fixture-modules")
include(@AROS_ENGINE_CMAKE@)
add_library(detlib STATIC archive_c.c)
"#
    .replace("@AROS_ENGINE_CMAKE@", &cmake_quote(&engine));
    fs::write(source.join("CMakeLists.txt"), project).expect("write fixture project");
    (
        fs::canonicalize(source).expect("canonicalize source"),
        fs::canonicalize(build).expect("canonicalize build"),
    )
}

fn read_members(archive: &Path) -> Vec<Member> {
    let bytes = fs::read(archive).unwrap_or_else(|_| panic!("read {}", archive.display()));
    assert!(bytes.starts_with(b"!<arch>\n"), "not an ar archive");
    let mut members = Vec::new();
    let mut offset = 8;
    while offset + MEMBER_HEADER <= bytes.len() {
        let header = &bytes[offset..offset + MEMBER_HEADER];
        assert_eq!(&header[58..60], b"`\n", "bad member header magic");
        let field =
            |from: usize, to: usize| String::from_utf8_lossy(&header[from..to]).trim().to_owned();
        let size: usize = field(48, 58).parse().expect("member size");
        members.push(Member {
            name: field(0, 16),
            date: field(16, 28),
            uid: field(28, 34),
            gid: field(34, 40),
        });
        offset += MEMBER_HEADER + size + (size & 1);
    }
    members
}

/// First GNU ar or llvm-ar that reports itself as such (Apple's ar has no D).
fn deterministic_archiver() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for name in ["llvm-ar", "ar"] {
        for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
            candidates.push(directory.join(name));
        }
    }
    candidates.push(PathBuf::from("/opt/homebrew/opt/llvm/bin/llvm-ar"));
    candidates.push(PathBuf::from("/usr/local/opt/llvm/bin/llvm-ar"));
    candidates.into_iter().find(|candidate| {
        candidate.is_file()
            && Command::new(candidate)
                .arg("--version")
                .output()
                .is_ok_and(|out| {
                    out.status.success() && {
                        let text = String::from_utf8_lossy(&out.stdout);
                        text.contains("GNU") || text.contains("LLVM")
                    }
                })
    })
}

fn host_compiler() -> PathBuf {
    let mut names = Vec::<OsString>::new();
    if let Some(configured) = env::var_os("CC") {
        names.push(configured);
    }
    names.extend(["clang", "cc", "gcc"].map(OsString::from));
    let search_path = env::var_os("PATH").unwrap_or_default();
    for name in names {
        let candidates: Vec<PathBuf> = if PathBuf::from(&name).components().count() > 1 {
            vec![PathBuf::from(&name)]
        } else {
            env::split_paths(&search_path)
                .map(|directory| directory.join(&name))
                .collect()
        };
        for candidate in candidates {
            if candidate.is_file() {
                return fs::canonicalize(candidate).expect("canonicalize host compiler");
            }
        }
    }
    panic!("no host C compiler found");
}

fn effective_uid() -> String {
    let out = Command::new("id").arg("-u").output().expect("run id -u");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn cmake_quote(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    format!("\"{}\"", normalized.replace('"', "\\\""))
}
