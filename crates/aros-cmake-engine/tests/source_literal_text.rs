#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const TEXT: &str =
    "// selected source facts\n#define VALUE 17\nmessage(FATAL_ERROR data_must_not_execute)\n";
const FILE: &str = "compiler/fixture/mmakefile.src";

struct Fixture {
    _temp: TempDir,
    source: PathBuf,
    project: PathBuf,
    build: PathBuf,
    output: PathBuf,
    hash: String,
}

fn checked(command: &mut Command) -> Output {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        let project = root.join("project");
        let build = root.join("build");
        for directory in [
            source.join("compiler/fixture"),
            project.clone(),
            build.clone(),
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        let bytes = b"# source recipe, not ambient data\n";
        fs::write(source.join(FILE), bytes).unwrap();
        let hash = aros_common::sha256_bytes(bytes).as_str().to_owned();
        let output = build.join("gen/compiler/fixture/platform.h");
        Self {
            _temp: temp,
            source,
            project,
            build,
            output,
            hash,
        }
    }

    fn declaration(name: &str, output: &Path, hash: &str) -> String {
        format!("aros_write_source_literal_text(NAME {name} OUTPUT \"{}\" FILE {FILE} FILE_SHA256 {hash} TEXT [==[{TEXT}]==])\n", output.display())
    }

    fn configure(&self, declarations: &str) -> Output {
        let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("engine");
        fs::write(self.project.join("CMakeLists.txt"), format!(
            "cmake_minimum_required(VERSION 3.22)\nproject(literal NONE)\nset(AROS_SOURCE_DIR \"{}\")\ninclude(\"{}/SourceLiteralTextRules.cmake\")\n{declarations}",
            self.source.display(), engine.display())).unwrap();
        Command::new("cmake")
            .args(["-S"])
            .arg(&self.project)
            .args(["-B"])
            .arg(&self.build)
            .args(["-G", "Ninja"])
            .output()
            .unwrap()
    }

    fn run(&self) -> Output {
        Command::new("cmake")
            .arg("--build")
            .arg(&self.build)
            .args(["--target", "literal-owner"])
            .output()
            .unwrap()
    }
}

fn assert_failed(result: &Output, message: &str) {
    assert!(!result.status.success());
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(text.contains(message), "expected {message:?}: {text}");
}

#[test]
fn source_literal_bytes_are_data_and_second_build_is_incremental() {
    let fixture = Fixture::new();
    let result = fixture.configure(&Fixture::declaration(
        "literal-owner",
        &fixture.output,
        &fixture.hash,
    ));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let first = fixture.run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stdout)
    );
    assert_eq!(fs::read(&fixture.output).unwrap(), TEXT.as_bytes());
    let modified = fs::metadata(&fixture.output).unwrap().modified().unwrap();
    let second = fixture.run();
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("no work to do"));
    assert_eq!(
        fs::metadata(&fixture.output).unwrap().modified().unwrap(),
        modified
    );
}

#[test]
fn source_drift_after_configuration_refuses_output() {
    let fixture = Fixture::new();
    assert!(fixture
        .configure(&Fixture::declaration(
            "literal-owner",
            &fixture.output,
            &fixture.hash
        ))
        .status
        .success());
    fs::write(fixture.source.join(FILE), b"changed source recipe\n").unwrap();
    assert_failed(&fixture.run(), "SHA256 drifted");
    assert!(!fixture.output.exists());
}

#[test]
fn newer_unchanged_source_does_not_leave_a_perpetually_dirty_build() {
    let fixture = Fixture::new();
    assert!(fixture
        .configure(&Fixture::declaration(
            "literal-owner",
            &fixture.output,
            &fixture.hash
        ))
        .status
        .success());
    assert!(fixture.run().status.success());
    checked(
        Command::new("cmake")
            .args(["-E", "touch"])
            .arg(fixture.source.join(FILE)),
    );
    assert!(fixture.run().status.success());
    assert_eq!(fs::read(&fixture.output).unwrap(), TEXT.as_bytes());
    let refreshed = fs::metadata(&fixture.output).unwrap().modified().unwrap();
    let clean = fixture.run();
    assert!(clean.status.success());
    assert!(String::from_utf8_lossy(&clean.stdout).contains("no work to do"));
    assert_eq!(
        fs::metadata(&fixture.output).unwrap().modified().unwrap(),
        refreshed
    );
}

#[test]
fn private_payload_drift_after_configuration_refuses_output() {
    let fixture = Fixture::new();
    assert!(fixture
        .configure(&Fixture::declaration(
            "literal-owner",
            &fixture.output,
            &fixture.hash
        ))
        .status
        .success());
    let files = fs::read_dir(fixture.build.join(".aros-source-literal-rules"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1);
    fs::write(&files[0], b"tampered payload\n").unwrap();
    assert_failed(&fixture.run(), "payload changed after configuration");
    assert!(!fixture.output.exists());
}

#[test]
fn declaring_hash_output_root_and_duplicate_producers_are_checked() {
    let wrong_hash = Fixture::new();
    assert_failed(
        &wrong_hash.configure(&Fixture::declaration(
            "literal-owner",
            &wrong_hash.output,
            &"0".repeat(64),
        )),
        "SHA256 drifted",
    );
    let wrong_root = Fixture::new();
    assert_failed(
        &wrong_root.configure(&Fixture::declaration(
            "literal-owner",
            &wrong_root.build.join("gen/other/platform.h"),
            &wrong_root.hash,
        )),
        "one direct file",
    );
    let duplicate = Fixture::new();
    let calls = format!(
        "{}{}",
        Fixture::declaration("literal-owner", &duplicate.output, &duplicate.hash),
        Fixture::declaration("another-owner", &duplicate.output, &duplicate.hash)
    );
    assert_failed(&duplicate.configure(&calls), "already belongs");
    let keywords = Fixture::new();
    let call = Fixture::declaration("literal-owner", &keywords.output, &keywords.hash)
        .replace("FILE_SHA256", "FILE");
    assert_failed(&keywords.configure(&call), "duplicate keyword");
}

#[test]
fn source_and_generated_directory_symlinks_are_rejected() {
    let source = Fixture::new();
    let file = source.source.join(FILE);
    let external = source.project.join("external");
    fs::rename(&file, &external).unwrap();
    symlink(&external, &file).unwrap();
    assert_failed(
        &source.configure(&Fixture::declaration(
            "literal-owner",
            &source.output,
            &source.hash,
        )),
        "symlink",
    );
    let generated = Fixture::new();
    symlink(&generated.project, generated.build.join("gen")).unwrap();
    assert_failed(
        &generated.configure(&Fixture::declaration(
            "literal-owner",
            &generated.output,
            &generated.hash,
        )),
        "symlink",
    );
}

#[test]
fn direct_runner_rechecks_payload_and_output_path() {
    let fixture = Fixture::new();
    assert!(fixture
        .configure(&Fixture::declaration(
            "literal-owner",
            &fixture.output,
            &fixture.hash
        ))
        .status
        .success());
    let payloads = fs::read_dir(fixture.build.join(".aros-source-literal-rules"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(payloads.len(), 1);
    let payload = &payloads[0];
    let hash = aros_common::sha256_bytes(&fs::read(payload).unwrap())
        .as_str()
        .to_owned();
    let runner =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("engine/RunSourceLiteralTextRule.cmake");
    let invoke = || {
        Command::new("cmake")
            .args([
                format!("-DBINARY_ROOT={}", fixture.build.display()),
                format!("-DSOURCE_ROOT={}", fixture.source.display()),
                format!("-DFILE={FILE}"),
                format!("-DFILE_SHA256={}", fixture.hash),
                format!("-DOUTPUT={}", fixture.output.display()),
                format!("-DPAYLOAD={}", payload.display()),
                format!("-DPAYLOAD_SHA256={hash}"),
            ])
            .arg("-P")
            .arg(&runner)
            .output()
            .unwrap()
    };
    let valid = invoke();
    assert!(
        valid.status.success(),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );
    fs::remove_file(&fixture.output).unwrap();
    let outside = fixture.project.join("outside");
    fs::write(&outside, b"must remain untouched").unwrap();
    symlink(&outside, &fixture.output).unwrap();
    assert_failed(&invoke(), "symlink");
    assert_eq!(fs::read(&outside).unwrap(), b"must remain untouched");
    checked(Command::new("cmake").arg("--version"));
}
