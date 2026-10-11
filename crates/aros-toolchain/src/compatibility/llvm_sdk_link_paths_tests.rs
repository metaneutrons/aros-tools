use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::PathBuf;

use super::validate_link_paths;

const MAP: &[u8] = b"synthetic LLVM link map\n";
const VALID_TEMPORARY: &str = "sealed-probe-Az09Qq.o";

struct Roots {
    _temporary_directory: tempfile::TempDir,
    root: PathBuf,
    sdk: PathBuf,
    compiler: PathBuf,
    temporary: PathBuf,
    original_sdk: PathBuf,
    fixture: PathBuf,
}

impl Roots {
    fn new() -> Self {
        let temporary_directory = tempfile::tempdir().unwrap();
        let root = temporary_directory.path().canonicalize().unwrap();
        let sdk = root.join("sdk");
        let compiler = root.join("compiler");
        let temporary = root.join("tmp");
        let original_sdk = root.join("original-sdk");
        for directory in [&sdk, &compiler, &temporary, &original_sdk] {
            fs::create_dir(directory).unwrap();
        }
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700)).unwrap();

        fs::write(sdk.join("libclient.a"), b"synthetic SDK archive").unwrap();
        fs::write(
            compiler.join("libclang_rt.a"),
            b"synthetic compiler archive",
        )
        .unwrap();
        fs::write(original_sdk.join("libold.a"), b"synthetic original archive").unwrap();
        let fixture = root.join("sealed-probe.c");
        fs::write(&fixture, b"int main(void) { return 0; }\n").unwrap();

        Self {
            _temporary_directory: temporary_directory,
            root,
            sdk,
            compiler,
            temporary,
            original_sdk,
            fixture,
        }
    }

    fn good_trace(&self) -> String {
        format!(
            "{}(objects/client.o)\n{}\n{}\n",
            self.sdk.join("libclient.a").display(),
            self.compiler.join("libclang_rt.a").display(),
            self.temporary.join(VALID_TEMPORARY).display(),
        )
    }

    fn validate(
        &self,
        trace: &[u8],
        map: &[u8],
        selection: &[String],
    ) -> Result<(), crate::ContractError> {
        validate_link_paths(
            trace,
            map,
            &self.sdk,
            &self.compiler,
            &self.temporary,
            &self.original_sdk,
            (true, selection, &self.fixture),
        )
    }
}

#[test]
fn accepts_sdk_compiler_and_fixture_derived_temporary_inputs() {
    let roots = Roots::new();
    let trace = roots.good_trace();
    roots
        .validate(&trace.into_bytes(), MAP, &["client".to_owned()])
        .expect("SDK archive, compiler runtime and six-character LLVM suffix should pass");
}

#[test]
fn temporary_basename_must_match_the_sealed_fixture_stem() {
    let roots = Roots::new();
    let mismatched = roots
        .temporary
        .join("other-probe-Az09Qq.o")
        .display()
        .to_string();
    let trace = format!(
        "{}\n{}\n",
        roots.sdk.join("libclient.a").display(),
        mismatched,
    );
    assert!(roots
        .validate(trace.as_bytes(), MAP, &["client".to_owned()])
        .is_err());
}

#[test]
fn temporary_inputs_must_be_direct_children_and_not_symlinks() {
    let roots = Roots::new();
    let nested = roots.temporary.join("nested");
    fs::create_dir(&nested).unwrap();
    let nested_trace = format!(
        "{}\n{}\n",
        roots.sdk.join("libclient.a").display(),
        nested.join(VALID_TEMPORARY).display(),
    );
    assert!(roots
        .validate(nested_trace.as_bytes(), MAP, &["client".to_owned()])
        .is_err());

    let outside = roots.root.join("outside.o");
    fs::write(&outside, b"outside temporary").unwrap();
    let linked = roots.temporary.join(VALID_TEMPORARY);
    symlink(&outside, &linked).unwrap();
    let linked_trace = format!(
        "{}\n{}\n",
        roots.sdk.join("libclient.a").display(),
        linked.display(),
    );
    assert!(roots
        .validate(linked_trace.as_bytes(), MAP, &["client".to_owned()])
        .is_err());
}

#[test]
fn temporary_root_must_have_private_permissions() {
    let roots = Roots::new();
    fs::set_permissions(&roots.temporary, fs::Permissions::from_mode(0o755)).unwrap();
    let trace = roots.good_trace();
    let error = roots
        .validate(trace.as_bytes(), MAP, &["client".to_owned()])
        .unwrap_err()
        .to_string();
    assert!(error.contains("private real directory"));
}

#[test]
fn non_temporary_inputs_must_resolve_inside_authenticated_roots() {
    let roots = Roots::new();
    let foreign = roots.root.join("foreign.a");
    fs::write(&foreign, b"foreign archive").unwrap();
    for trace in [
        format!(
            "{}\n{}\n",
            roots.sdk.join("libclient.a").display(),
            foreign.display(),
        ),
        format!(
            "{}\n{}\n",
            roots.sdk.join("libclient.a").display(),
            roots.root.join("missing.a").display(),
        ),
    ] {
        assert!(roots
            .validate(trace.as_bytes(), MAP, &["client".to_owned()])
            .is_err());
    }

    let escaped_link = roots.sdk.join("liboutside.a");
    symlink(&foreign, &escaped_link).unwrap();
    let symlink_trace = format!(
        "{}\n{}\n",
        roots.sdk.join("libclient.a").display(),
        escaped_link.display(),
    );
    assert!(roots
        .validate(symlink_trace.as_bytes(), MAP, &["client".to_owned()])
        .is_err());
}

#[test]
fn ambiguous_raw_trace_paths_are_rejected() {
    let roots = Roots::new();
    for suffix in [
        "/../sdk/libclient.a",
        "/sdk//libclient.a",
        "/path/with,comma.a",
    ] {
        let trace = format!("{}\n{}\n", roots.sdk.join("libclient.a").display(), suffix);
        let error = roots
            .validate(trace.as_bytes(), MAP, &["client".to_owned()])
            .expect_err("ambiguous trace path must be refused")
            .to_string();
        assert!(!error.contains(roots.root.to_str().unwrap()));
    }
}

#[test]
fn relocated_trace_and_map_must_not_reference_the_original_sdk() {
    let roots = Roots::new();
    let good = roots.good_trace();
    let leaked_trace = format!("{good}{}\n", roots.original_sdk.join("libold.a").display());
    let trace_error = roots
        .validate(leaked_trace.as_bytes(), MAP, &["client".to_owned()])
        .unwrap_err()
        .to_string();
    assert!(trace_error.contains("original SDK"));
    assert!(!trace_error.contains(roots.root.to_str().unwrap()));

    let leaked_map = format!("link map path: {}\n", roots.original_sdk.display());
    let map_error = roots
        .validate(
            good.as_bytes(),
            leaked_map.as_bytes(),
            &["client".to_owned()],
        )
        .unwrap_err()
        .to_string();
    assert!(map_error.contains("original SDK"));
    assert!(!map_error.contains(roots.root.to_str().unwrap()));
}

#[test]
fn every_source_declared_library_must_resolve_from_the_sdk() {
    let roots = Roots::new();
    let trace = roots.good_trace();
    let error = roots
        .validate(
            trace.as_bytes(),
            MAP,
            &["client".to_owned(), "missing".to_owned()],
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("source-declared LLVM SDK library"));

    let compiler_fallback = format!(
        "{}\n{}\n",
        roots.compiler.join("libclient.a").display(),
        roots.sdk.join("libclient.a").display(),
    );
    fs::write(roots.compiler.join("libclient.a"), b"compiler fallback").unwrap();
    let fallback_error = roots
        .validate(compiler_fallback.as_bytes(), MAP, &["client".to_owned()])
        .unwrap_err()
        .to_string();
    assert!(fallback_error.contains("outside the selected SDK"));
}

#[test]
fn invalid_utf8_truncation_and_empty_maps_are_rejected() {
    let roots = Roots::new();
    assert!(roots
        .validate(&[0xff], MAP, &["client".to_owned()])
        .unwrap_err()
        .to_string()
        .contains("trace is not UTF-8"));
    assert!(roots
        .validate(
            roots.good_trace().as_bytes(),
            &[0xff],
            &["client".to_owned()]
        )
        .unwrap_err()
        .to_string()
        .contains("map is not UTF-8"));
    assert!(roots
        .validate(
            b"/sdk/libclient.a\n[32 bytes omitted by aros]\n",
            MAP,
            &["client".to_owned()],
        )
        .unwrap_err()
        .to_string()
        .contains("truncated"));
    assert!(roots
        .validate(roots.good_trace().as_bytes(), b"", &["client".to_owned()])
        .unwrap_err()
        .to_string()
        .contains("map is empty"));
}
