use std::fs;
use std::time::Duration;

use aros_common::CancellationToken;

use crate::genmf_cache::{
    list, refresh, select, status, verify, GenmfCacheEntryState, GenmfCacheRequest,
};

fn fixture() -> (tempfile::TempDir, GenmfCacheRequest) {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    let cache = temporary.path().join("cache");
    fs::create_dir_all(source.join("config")).unwrap();
    fs::create_dir_all(source.join("tools/genmf")).unwrap();
    fs::create_dir_all(source.join("rom")).unwrap();
    fs::create_dir(&cache).unwrap();
    fs::write(
        source.join("config/make.tmpl"),
        "%include make-common.tmpl\nmain-template\n",
    )
    .unwrap();
    fs::write(source.join("config/make-common.tmpl"), "common-template\n").unwrap();
    fs::write(
        source.join("tools/genmf/genmf.py"),
        "import pathlib, sys\n".to_owned()
            + "template, source, output = map(pathlib.Path, sys.argv[1:])\n"
            + "output.write_bytes(template.read_bytes() + source.read_bytes())\n",
    )
    .unwrap();
    fs::write(source.join("rom/mmakefile"), "%build_program one\n").unwrap();
    let request = GenmfCacheRequest {
        source_dir: source,
        cache_dir: cache,
        python: which::which("python3").unwrap(),
        timeout: Duration::from_secs(10),
    };
    (temporary, request)
}

fn replace_preserving_mtime(path: &std::path::Path, bytes: &[u8]) {
    let modified = fs::metadata(path)
        .expect("inspect fixture input")
        .modified()
        .expect("read fixture input mtime");
    fs::write(path, bytes).expect("replace fixture input");
    fs::File::open(path)
        .expect("open replaced fixture input")
        .set_times(fs::FileTimes::new().set_modified(modified))
        .expect("restore fixture input mtime");
}

#[test]
fn status_is_passive_and_refresh_publishes_a_verified_immutable_generation() {
    let (_temporary, request) = fixture();
    let report = status(&request.cache_dir).unwrap();
    assert_eq!(report.operation, "genmf.status");
    assert!(!report.side_effects.creates_state);
    assert!(!request.cache_dir.join("genmf").exists());

    let refreshed = refresh(&request, &CancellationToken::default()).unwrap();
    assert_eq!(refreshed.entries.len(), 1);
    assert!(refreshed.entries[0]
        .generation_dir
        .join("expansion.mk")
        .is_file());
    let verified = verify(&request).unwrap();
    assert_eq!(verified.entries.len(), 1);
    assert_eq!(
        fs::read(verified.entries[0].generation_dir.join("expansion.mk")).unwrap(),
        b"%include make-common.tmpl\nmain-template\n%build_program one\n"
    );
}

#[test]
fn content_identities_reject_preserved_mtime_staleness_and_legacy_path_collisions() {
    let (_temporary, request) = fixture();
    let first = refresh(&request, &CancellationToken::default()).unwrap();
    let first_generation = first.entries[0].selection.generation.clone();
    let source = request.source_dir.join("rom/mmakefile");
    let metadata = fs::metadata(&source).unwrap();
    let modified = metadata.modified().unwrap();
    fs::write(&source, "%build_program two\n").unwrap();
    fs::File::open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();

    let refreshed = refresh(&request, &CancellationToken::default()).unwrap();
    assert_ne!(refreshed.entries[0].selection.generation, first_generation);
    assert!(verify(&request).is_ok());

    fs::create_dir_all(request.source_dir.join("a%b")).unwrap();
    fs::create_dir_all(request.source_dir.join("a")).unwrap();
    fs::write(
        request.source_dir.join("a%b/mmakefile"),
        "%build_program three\n",
    )
    .unwrap();
    fs::write(
        request.source_dir.join("a/b%mmakefile"),
        "%build_program four\n",
    )
    .unwrap();
    let selected = select(&request).unwrap();
    let unique: std::collections::BTreeSet<_> = selected
        .entries
        .iter()
        .map(|entry| entry.generation.as_str())
        .collect();
    assert_eq!(unique.len(), selected.entries.len());
}

#[test]
fn template_and_generator_content_changes_select_new_immutable_generations() {
    let (_temporary, request) = fixture();
    let first = refresh(&request, &CancellationToken::default()).unwrap();
    let first_generation = first.entries[0].selection.generation.clone();

    replace_preserving_mtime(
        &request.source_dir.join("config/make-common.tmpl"),
        b"changed-template\n",
    );
    let template_changed = refresh(&request, &CancellationToken::default()).unwrap();
    let template_generation = template_changed.entries[0].selection.generation.clone();
    assert_ne!(template_generation, first_generation);

    replace_preserving_mtime(
        &request.source_dir.join("tools/genmf/genmf.py"),
        b"import pathlib, sys\ntemplate, source, output = map(pathlib.Path, sys.argv[1:])\noutput.write_bytes(b'changed-generator\\n' + template.read_bytes() + source.read_bytes())\n",
    );
    let generator_changed = refresh(&request, &CancellationToken::default()).unwrap();
    assert_ne!(
        generator_changed.entries[0].selection.generation,
        template_generation
    );
    assert!(verify(&request).is_ok());
}

#[test]
fn failed_or_timed_out_generators_never_publish_a_final_generation() {
    let (_temporary, mut request) = fixture();
    fs::write(
        request.source_dir.join("tools/genmf/genmf.py"),
        "import sys\nsys.stderr.write('intentional failure\\n')\nsys.exit(9)\n",
    )
    .unwrap();
    let failed = refresh(&request, &CancellationToken::default()).unwrap_err();
    assert!(failed.to_string().contains("genmf exited with"));
    assert_eq!(
        list(&request).unwrap().entries[0].state,
        GenmfCacheEntryState::Missing
    );

    fs::write(
        request.source_dir.join("tools/genmf/genmf.py"),
        "import time\ntime.sleep(60)\n",
    )
    .unwrap();
    request.timeout = Duration::from_millis(100);
    let timed_out = refresh(&request, &CancellationToken::default()).unwrap_err();
    assert!(timed_out.timed_out());
    assert_eq!(
        list(&request).unwrap().entries[0].state,
        GenmfCacheEntryState::Missing
    );
}

#[cfg(unix)]
#[test]
fn interpreter_content_and_version_select_a_new_generation() {
    use std::os::unix::fs::PermissionsExt;

    let (_temporary, mut request) = fixture();
    let wrapper = request.source_dir.join("fixture-python");
    let system_python = which::which("python3").unwrap();
    let write_wrapper = |version: &str| {
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf '%s\\n' '{version}'; exit 0; fi\nexec '{}' \"$@\"\n",
                system_python.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    };
    write_wrapper("Python fixture 1");
    request.python = wrapper.clone();
    let first = refresh(&request, &CancellationToken::default()).unwrap();
    let first_generation = first.entries[0].selection.generation.clone();
    let modified = fs::metadata(&wrapper).unwrap().modified().unwrap();
    write_wrapper("Python fixture 2");
    fs::File::open(&wrapper)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let changed = refresh(&request, &CancellationToken::default()).unwrap();
    assert_ne!(changed.entries[0].selection.generation, first_generation);
}

#[test]
fn concurrent_refreshers_publish_or_reuse_only_one_complete_generation() {
    let (_temporary, request) = fixture();
    fs::write(
        request.source_dir.join("tools/genmf/genmf.py"),
        "import pathlib, sys, time\ntime.sleep(0.2)\ntemplate, source, output = map(pathlib.Path, sys.argv[1:])\noutput.write_bytes(template.read_bytes() + source.read_bytes())\n",
    )
    .unwrap();
    let first_request = request.clone();
    let second_request = request.clone();
    let first = std::thread::spawn(move || refresh(&first_request, &CancellationToken::default()));
    let second =
        std::thread::spawn(move || refresh(&second_request, &CancellationToken::default()));
    let first = first.join().expect("first refresher joins").unwrap();
    let second = second.join().expect("second refresher joins").unwrap();
    assert_eq!(
        first.entries[0].selection.generation,
        second.entries[0].selection.generation
    );
    assert!(verify(&request).is_ok());
}

#[test]
fn verification_never_accepts_an_inflight_refresh_generation() {
    let (_temporary, request) = fixture();
    fs::write(
        request.source_dir.join("tools/genmf/genmf.py"),
        "import pathlib, sys, time\ntime.sleep(0.5)\ntemplate, source, output = map(pathlib.Path, sys.argv[1:])\noutput.write_bytes(template.read_bytes() + source.read_bytes())\n",
    )
    .unwrap();
    let refresh_request = request.clone();
    let refresher =
        std::thread::spawn(move || refresh(&refresh_request, &CancellationToken::default()));
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        verify(&request).is_err(),
        "verify must reject a missing final generation while refresh has only private output"
    );
    refresher.join().expect("refresher joins").unwrap();
    assert!(verify(&request).is_ok());
}

#[test]
fn refresh_runs_genmf_without_user_python_or_cargo_environment() {
    let (_temporary, request) = fixture();
    fs::write(
        request.source_dir.join("tools/genmf/genmf.py"),
        "import os, pathlib, sys\nfor name in ('PYTHONPATH', 'PYTHONUSERBASE', 'VIRTUAL_ENV', 'CARGO_HOME'):\n    if os.environ.get(name):\n        raise SystemExit(f'ambient {name} leaked')\ntemplate, source, output = map(pathlib.Path, sys.argv[1:])\noutput.write_bytes(template.read_bytes() + source.read_bytes())\n",
    )
    .unwrap();
    assert!(refresh(&request, &CancellationToken::default()).is_ok());
}

#[test]
fn list_never_reads_generation_payloads_and_verify_rejects_tampering() {
    let (_temporary, request) = fixture();
    let before = list(&request).unwrap();
    assert_eq!(before.entries[0].state, GenmfCacheEntryState::Missing);
    let refreshed = refresh(&request, &CancellationToken::default()).unwrap();
    let generation = &refreshed.entries[0].generation_dir;
    let listed = list(&request).unwrap();
    assert_eq!(
        listed.entries[0].state,
        GenmfCacheEntryState::PresentUnverified
    );
    fs::write(generation.join("expansion.mk"), "tampered\n").unwrap();
    assert!(verify(&request).is_err());
}

#[test]
fn cancelled_refresh_never_publishes_a_generation() {
    let (_temporary, request) = fixture();
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    assert!(refresh(&request, &cancellation).is_err());
    let listed = list(&request).unwrap();
    assert_eq!(listed.entries[0].state, GenmfCacheEntryState::Missing);
}

#[test]
fn template_include_escapes_are_rejected_before_cache_mutation() {
    let (_temporary, request) = fixture();
    fs::write(
        request.source_dir.join("config/make.tmpl"),
        "%include ../outside.tmpl\n",
    )
    .unwrap();
    fs::write(
        request.source_dir.parent().unwrap().join("outside.tmpl"),
        "outside\n",
    )
    .unwrap();
    assert!(refresh(&request, &CancellationToken::default()).is_err());
    assert!(!request.cache_dir.join("genmf").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_template_inputs_are_rejected_before_cache_mutation() {
    use std::os::unix::fs::symlink;

    let (_temporary, request) = fixture();
    let outside = request.source_dir.parent().unwrap().join("outside.tmpl");
    fs::write(&outside, "outside\n").unwrap();
    fs::remove_file(request.source_dir.join("config/make-common.tmpl")).unwrap();
    symlink(&outside, request.source_dir.join("config/make-common.tmpl")).unwrap();
    assert!(refresh(&request, &CancellationToken::default()).is_err());
    assert!(!request.cache_dir.join("genmf").exists());
}
