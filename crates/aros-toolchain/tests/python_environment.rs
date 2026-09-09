//! The native environment imports only archived lock-owned modules.

use std::fs;

use aros_common::sha256_file;
use aros_toolchain::{python_environment::PythonEnvironment, source_lock::SourceLock};
use flate2::{write::GzEncoder, Compression};
use serde_json::json;
use tar::{Builder, Header};

fn archive(path: &std::path::Path, entries: &[(&str, &[u8])]) {
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
    let encoder = builder.into_inner().unwrap();
    encoder.finish().unwrap();
}

fn lock(cache: &std::path::Path) -> SourceLock {
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
    let document = json!({
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
    });
    SourceLock::parse(&serde_json::to_vec(&document).unwrap()).unwrap()
}

#[test]
fn prepares_and_probes_the_exact_private_mako_environment() {
    let temporary = tempfile::tempdir().unwrap();
    let cache = temporary.path().join("cache");
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
    let environment =
        PythonEnvironment::prepare(&lock(&cache), &cache, &temporary.path().join("environment"))
            .unwrap();
    assert!(environment.interpreter().version.starts_with("Python 3."));
    assert_eq!(environment.import_roots().len(), 2);
    assert_eq!(
        environment
            .packages()
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>(),
        ["mako", "markupsafe"]
    );
    assert!(environment
        .packages()
        .iter()
        .all(|package| package.size > 0));
    let compatibility = environment.compatibility_environment().unwrap();
    assert_eq!(
        compatibility.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "PATH",
            "PYTHON",
            "PYTHONDONTWRITEBYTECODE",
            "PYTHONHASHSEED",
            "PYTHONNOUSERSITE",
            "PYTHONPATH",
        ]
    );
    assert_eq!(compatibility["PATH"], "/nonexistent");
    assert_eq!(
        compatibility["PYTHON"],
        environment.interpreter().path.to_str().unwrap()
    );
    assert_eq!(
        compatibility["PYTHONPATH"],
        std::env::join_paths(
            environment
                .import_roots()
                .iter()
                .map(fs::canonicalize)
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        )
        .unwrap()
        .into_string()
        .unwrap()
    );
    assert!(!compatibility.contains_key("PYTHONHOME"));
    assert!(!compatibility.contains_key("HOME"));

    let mut command = std::process::Command::new(&environment.interpreter().path);
    environment.apply_to(&mut command);
    command.args([
        "-s",
        "-B",
        "-c",
        "import mako, markupsafe; print(mako.__version__, markupsafe.__version__)",
    ]);
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "1.3.10 3.0.2\n");
}

#[test]
fn existing_environment_root_is_never_reused() {
    let temporary = tempfile::tempdir().unwrap();
    let destination = temporary.path().join("environment");
    fs::create_dir(&destination).unwrap();
    let empty = SourceLock::parse(
        br#"{"schema":"aros-toolchain-source-lock-v2","family":"llvm","version":"11.0.0","sources":[{"component":"llvm","version":"11.0.0","purpose":"toolchain-component","filename":"llvm.tar.xz","url":"https://example.invalid/llvm.tar.xz","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":1}],"host_python_packages":[{"name":"mako","version":"1.3.10","filename":"mako.tar.gz","url":"https://example.invalid/mako.tar.gz","sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","size":1,"source_root":"mako","python_path":"."}]}"#,
    )
    .unwrap();
    assert!(PythonEnvironment::prepare(&empty, temporary.path(), &destination).is_err());
}

#[test]
fn rejects_an_unsupported_python_package_closure_before_cache_access() {
    let temporary = tempfile::tempdir().unwrap();
    let lock = SourceLock::parse(
        br#"{"schema":"aros-toolchain-source-lock-v2","family":"llvm","version":"11.0.0","sources":[{"component":"llvm","version":"11.0.0","purpose":"toolchain-component","filename":"llvm.tar.xz","url":"https://example.invalid/llvm.tar.xz","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":1}],"host_python_packages":[{"name":"other","version":"1.0","filename":"other.tar.gz","url":"https://example.invalid/other.tar.gz","sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","size":1,"source_root":"other","python_path":"."}]}"#,
    )
    .unwrap();
    let error = PythonEnvironment::prepare(&lock, temporary.path(), &temporary.path().join("env"))
        .unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code.to_string(),
        "AX0401"
    );
}
