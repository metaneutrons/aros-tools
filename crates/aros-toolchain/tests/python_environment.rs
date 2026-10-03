//! The native environment imports only archived lock-owned modules.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use aros_common::{sha256_file, DiagnosticCode};
#[cfg(unix)]
use aros_toolchain::python_environment::PythonInterpreter;
use aros_toolchain::{python_environment::PythonEnvironment, source_lock::SourceLock};
use flate2::{write::GzEncoder, Compression};
use serde_json::{json, Value};
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

fn package(
    cache: &Path,
    name: &str,
    version: &str,
    filename: &str,
    source_root: &str,
    python_path: &str,
) -> Value {
    let measured = sha256_file(&cache.join(filename)).unwrap();
    json!({
        "name": name, "version": version, "filename": filename,
        "url": format!("https://example.invalid/{filename}"),
        "sha256": measured.digest, "size": measured.size,
        "source_root": source_root, "python_path": python_path
    })
}

fn lock_with_packages(host_python_packages: &[Value]) -> SourceLock {
    let document = json!({
        "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
        "sources": [{
            "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
            "filename": "llvm.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
            "sha256": "a".repeat(64), "size": 1
        }],
        "host_python_packages": host_python_packages
    });
    SourceLock::parse(&serde_json::to_vec(&document).unwrap()).unwrap()
}

fn lock(cache: &Path) -> SourceLock {
    lock_with_packages(&required_package_records(cache))
}

fn gnu_lock_with_packages(host_python_packages: &[Value]) -> SourceLock {
    let mut document: Value =
        serde_json::from_slice(include_bytes!("fixtures/gnu-source-lock-v3.json")).unwrap();
    document["host_python_packages"] = json!(host_python_packages);
    SourceLock::parse(&serde_json::to_vec(&document).unwrap()).unwrap()
}

fn required_package_records(cache: &Path) -> Vec<Value> {
    vec![
        package(
            cache,
            "mako",
            "1.3.10",
            "mako-1.3.10.tar.gz",
            "mako-1.3.10",
            ".",
        ),
        package(
            cache,
            "markupsafe",
            "3.0.2",
            "markupsafe-3.0.2.tar.gz",
            "markupsafe-3.0.2",
            "src",
        ),
    ]
}

fn write_required_archives(cache: &Path) {
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
}

fn optional_yaml_record(cache: &Path) -> Value {
    package(
        cache,
        "yaml",
        "6.0.2",
        "pyyaml-6.0.2.tar.gz",
        "pyyaml-6.0.2",
        ".",
    )
}

#[cfg(unix)]
fn write_interpreter_wrapper(directory: &Path, working_directory: &Path) -> PythonInterpreter {
    let python = which::which("python3").unwrap();
    let python = fs::canonicalize(python).unwrap();
    let version_output = Command::new(&python)
        .args(["-s", "-B", "--version"])
        .output()
        .unwrap();
    assert!(version_output.status.success());
    let version_output = if version_output.stdout.is_empty() {
        &version_output.stderr
    } else {
        &version_output.stdout
    };
    let version = std::str::from_utf8(version_output)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned();
    let wrapper = directory.join("python3-wrapper");
    let contents = format!(
        "#!/bin/sh\nset -eu\ncd {}\nexec {} \"$@\"\n",
        shell_quote(working_directory),
        shell_quote(&python)
    );
    fs::write(&wrapper, contents).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    PythonInterpreter {
        path: wrapper,
        version,
    }
}

#[cfg(unix)]
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[test]
fn prepares_and_probes_the_exact_private_mako_environment() {
    let temporary = tempfile::tempdir().unwrap();
    let cache = temporary.path().join("cache");
    fs::create_dir(&cache).unwrap();
    write_required_archives(&cache);
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
fn llvm_keeps_its_exact_two_package_contract() {
    let temporary = tempfile::tempdir().unwrap();
    let cache = temporary.path().join("cache");
    fs::create_dir(&cache).unwrap();
    write_required_archives(&cache);
    archive(
        &cache.join("pyyaml-6.0.2.tar.gz"),
        &[("pyyaml-6.0.2/yaml/__init__.py", b"__version__ = '6.0.2'\n")],
    );
    let mut packages = required_package_records(&cache);
    packages.push(optional_yaml_record(&cache));
    let lock = lock_with_packages(&packages);
    let destination = temporary.path().join("environment");
    let error = PythonEnvironment::prepare(&lock, &cache, &destination).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        DiagnosticCode::ProducerEnvironment
    );
    assert!(!destination.exists());
}

#[test]
fn prepares_and_probes_an_optional_locked_yaml_module() {
    let temporary = tempfile::tempdir().unwrap();
    let cache = temporary.path().join("cache");
    fs::create_dir(&cache).unwrap();
    write_required_archives(&cache);
    archive(
        &cache.join("pyyaml-6.0.2.tar.gz"),
        &[(
            "pyyaml-6.0.2/yaml/__init__.py",
            b"__version__ = '6.0.2'\nORIGIN = 'locked'\n",
        )],
    );
    let mut packages = required_package_records(&cache);
    packages.push(optional_yaml_record(&cache));
    let lock = gnu_lock_with_packages(&packages);
    let environment =
        PythonEnvironment::prepare(&lock, &cache, &temporary.path().join("environment")).unwrap();

    assert_eq!(environment.import_roots().len(), 3);
    assert_eq!(
        environment
            .packages()
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>(),
        ["mako", "markupsafe", "yaml"]
    );
    let mut command = Command::new(&environment.interpreter().path);
    environment.apply_to(&mut command);
    command.args([
        "-s",
        "-B",
        "-P",
        "-c",
        "import yaml; print(yaml.__version__, yaml.ORIGIN)",
    ]);
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "6.0.2 locked\n");
}

#[cfg(unix)]
#[test]
fn rejects_an_ambient_module_that_shadows_locked_yaml() {
    let temporary = tempfile::tempdir().unwrap();
    let cache = temporary.path().join("cache");
    let ambient = temporary.path().join("ambient");
    fs::create_dir(&cache).unwrap();
    fs::create_dir(&ambient).unwrap();
    write_required_archives(&cache);
    archive(
        &cache.join("pyyaml-6.0.2.tar.gz"),
        &[(
            "pyyaml-6.0.2/yaml/__init__.py",
            b"__version__ = '6.0.2'\nORIGIN = 'locked'\n",
        )],
    );
    fs::create_dir(ambient.join("yaml")).unwrap();
    fs::write(
        ambient.join("yaml/__init__.py"),
        b"__version__ = '6.0.2'\nORIGIN = 'ambient'\n",
    )
    .unwrap();
    let mut packages = required_package_records(&cache);
    packages.push(optional_yaml_record(&cache));
    let lock = gnu_lock_with_packages(&packages);
    let interpreter = write_interpreter_wrapper(temporary.path(), &ambient);
    let error = PythonEnvironment::prepare_with_interpreter(
        &lock,
        &cache,
        &temporary.path().join("environment"),
        &interpreter,
    )
    .unwrap_err();

    assert_eq!(
        error.diagnostics().diagnostics[0].code.to_string(),
        "AX0401"
    );
}

#[test]
fn rejects_a_yaml_version_that_differs_from_the_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let cache = temporary.path().join("cache");
    fs::create_dir(&cache).unwrap();
    write_required_archives(&cache);
    archive(
        &cache.join("pyyaml-6.0.2.tar.gz"),
        &[("pyyaml-6.0.2/yaml/__init__.py", b"__version__ = '6.0.2'\n")],
    );
    let mut packages = required_package_records(&cache);
    let mut yaml = optional_yaml_record(&cache);
    yaml["version"] = json!("6.0.1");
    packages.push(yaml);
    let lock = gnu_lock_with_packages(&packages);
    let error =
        PythonEnvironment::prepare(&lock, &cache, &temporary.path().join("env")).unwrap_err();

    assert_eq!(
        error.diagnostics().diagnostics[0].code.to_string(),
        "AX0401"
    );
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

#[test]
fn rejects_unknown_modules_even_when_required_modules_are_declared() {
    let temporary = tempfile::tempdir().unwrap();
    let cache = temporary.path().join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("mako.tar.gz"), b"locked Mako placeholder").unwrap();
    fs::write(
        cache.join("markupsafe.tar.gz"),
        b"locked MarkupSafe placeholder",
    )
    .unwrap();
    archive(
        &cache.join("other.tar.gz"),
        &[("other-1.0/other.py", b"__version__ = '1.0'\n")],
    );
    let lock = lock_with_packages(&[
        package(&cache, "mako", "1.3.10", "mako.tar.gz", "mako", "."),
        package(
            &cache,
            "markupsafe",
            "3.0.2",
            "markupsafe.tar.gz",
            "markupsafe",
            ".",
        ),
        package(&cache, "other", "1.0", "other.tar.gz", "other-1.0", "."),
    ]);
    let error =
        PythonEnvironment::prepare(&lock, &cache, &temporary.path().join("env")).unwrap_err();

    assert_eq!(
        error.diagnostics().diagnostics[0].code.to_string(),
        "AX0401"
    );
}

#[test]
fn rejects_a_missing_mandatory_module_before_cache_access() {
    let temporary = tempfile::tempdir().unwrap();
    let cache = temporary.path().join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("mako.tar.gz"), b"locked Mako placeholder").unwrap();
    let lock = lock_with_packages(&[package(
        &cache,
        "mako",
        "1.3.10",
        "mako.tar.gz",
        "mako",
        ".",
    )]);
    let error =
        PythonEnvironment::prepare(&lock, &cache, &temporary.path().join("env")).unwrap_err();

    assert_eq!(
        error.diagnostics().diagnostics[0].code.to_string(),
        "AX0401"
    );
}
