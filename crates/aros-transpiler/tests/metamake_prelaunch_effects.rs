//! Original MetaMake pre-launch effects in private synthetic trees only.
//! This is not a Source/Mesa owner or complete runtime-closure qualification.

use aros_common::{
    measure_regular_file_bounded, measure_tree_content_cas, run_output_with_control, sha256_bytes,
    CancellationToken, Sha256Digest,
};
use serde::Deserialize;
use std::{
    collections::BTreeMap, fs, os::unix::fs::DirBuilderExt, path::PathBuf, process::Command,
    time::Duration,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    output_root: PathBuf,
    metamake: PathBuf,
    metamake_sha256: Sha256Digest,
    python: PathBuf,
    python_sha256: Sha256Digest,
}

const HELPER: &str = include_str!("support/metamake_prelaunch_effects.py");

fn pinned(path: &std::path::Path, expected: &Sha256Digest) {
    let (_, bytes) = measure_regular_file_bounded(path, 4 * 1024 * 1024)
        .unwrap()
        .unwrap();
    assert_eq!(
        &sha256_bytes(&bytes),
        expected,
        "native fixture program changed"
    );
}

fn modified_at(path: &std::path::Path, seconds: u64) {
    let time = std::time::UNIX_EPOCH + Duration::from_secs(seconds);
    fs::File::open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(time))
        .unwrap();
    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), time);
}

#[test]
#[ignore = "requires pinned original MetaMake/Python and a fresh private output root"]
fn original_metamake_generates_before_capture_even_with_dry_run() {
    let case_path = PathBuf::from(std::env::var_os("AROS_TEST_METAMAKE_PRELAUNCH_CASE").unwrap());
    let (_, bytes) = measure_regular_file_bounded(&case_path, 64 * 1024)
        .unwrap()
        .unwrap();
    let case: Case = serde_json::from_slice(&bytes).unwrap();
    for path in [&case.metamake, &case.python] {
        assert!(path.is_absolute());
        assert_eq!(fs::canonicalize(path).unwrap(), *path);
    }
    pinned(&case.metamake, &case.metamake_sha256);
    pinned(&case.python, &case.python_sha256);
    // The native launcher uses a shell. Only these controlled, literal paths
    // enter its private config; no Source/configuration text is executed.
    for path in [&case.output_root, &case.python] {
        assert!(path.is_absolute());
        assert!(path
            .to_str()
            .unwrap()
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-@".contains(&b)));
    }
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&case.output_root)
        .unwrap();
    assert_eq!(
        fs::canonicalize(&case.output_root).unwrap(),
        case.output_root
    );
    let helper = case.output_root.join("helper.py");
    fs::write(&helper, HELPER).unwrap();
    let empty_path = case.output_root.join("empty-path");
    fs::create_dir(&empty_path).unwrap();
    let mut results = Vec::new();
    // Native freshness uses integral modification times, not content hashes.
    // Explicit distant times avoid sleeps and filesystem-resolution races.
    for (name, global_present, owner_present, source_time, template_time, expected) in [
        ("ready", true, true, 100, 100, vec!["leaf"]),
        (
            "missing-global",
            false,
            true,
            100,
            100,
            vec!["globals", "leaf"],
        ),
        (
            "missing-owner",
            true,
            false,
            100,
            100,
            vec!["generate", "leaf"],
        ),
        (
            "both-missing",
            false,
            false,
            100,
            100,
            vec!["globals", "generate", "leaf"],
        ),
        (
            "stale-owner-source",
            true,
            true,
            300,
            100,
            vec!["generate", "leaf"],
        ),
        (
            "stale-owner-template",
            true,
            true,
            100,
            300,
            vec!["generate", "leaf"],
        ),
    ] {
        let root = case.output_root.join(name);
        let source = root.join("source");
        let build = root.join("build");
        fs::create_dir_all(source.join("module")).unwrap();
        fs::create_dir_all(build.join("module")).unwrap();
        let declaration = b"#MM selected\n";
        fs::write(source.join("module/mmakefile.src"), declaration).unwrap();
        modified_at(&source.join("module/mmakefile.src"), source_time);
        fs::write(
            source.join("template"),
            b"synthetic dependency, never executed\n",
        )
        .unwrap();
        modified_at(&source.join("template"), template_time);
        if owner_present {
            fs::write(build.join("module/mmakefile"), declaration).unwrap();
            modified_at(&build.join("module/mmakefile"), 200);
        }
        if global_present {
            fs::write(build.join("globals.cfg"), b"PROBE = present\n").unwrap();
        }
        let config = root.join("mmake.config");
        let interpreter = format!("{} -I -S {}", case.python.display(), helper.display());
        let text = format!(
            "[capture]\nmaketool {interpreter} leaf\ndefaultmakefilename mmakefile\n\
             globalvarfile $(TOP)/globals.cfg\ngenglobalvarfile {interpreter} globals\n\
             genmakefilescript {interpreter} generate $(MMLIST)\n\
             genmakefiledeps $(SRCDIR)/template\n"
        );
        assert!(text.lines().all(|line| line.len() < 254));
        fs::write(&config, text).unwrap();
        let source_before = measure_tree_content_cas(&source).unwrap();
        let mut command = Command::new(&case.metamake);
        command
            .args([
                format!("--srcdir={}", source.display()),
                format!("--builddir={}", build.display()),
                "--quiet".into(),
                "-n".into(),
                "capture.selected".into(),
            ])
            .current_dir(&root)
            .env_clear()
            .envs(BTreeMap::from([
                ("PATH", empty_path.as_os_str()),
                ("MMAKE_CONFIG", config.as_os_str()),
                ("AROS_TEST_EFFECT_ROOT", root.as_os_str()),
            ]));
        let output = run_output_with_control(
            &mut command,
            1024 * 1024,
            Duration::from_secs(30),
            &CancellationToken::default(),
        )
        .unwrap();
        fs::write(
            root.join("native.stdout"),
            output.stdout.exact_bytes().unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("native.stderr"),
            output.stderr.exact_bytes().unwrap(),
        )
        .unwrap();
        assert!(
            !output.timed_out && !output.cancelled && output.status.success(),
            "private pre-launch counter failed"
        );
        let events = fs::read_to_string(root.join("effects.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .map(|event| event["stage"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        // Native MetaMake creates MMLIST under its own system temporary
        // directory. Check only the recorded success-path cleanup; failure or
        // timeout may still leave native temporary data outside this root.
        for event in events.iter().filter(|event| event["stage"] == "generate") {
            let arguments = event["argv"].as_array().unwrap();
            assert_eq!(arguments.len(), 1);
            let list = std::path::Path::new(arguments[0].as_str().unwrap());
            assert!(list.is_absolute());
            assert_eq!(
                fs::symlink_metadata(list).unwrap_err().kind(),
                std::io::ErrorKind::NotFound
            );
        }
        let leaf = events.last().unwrap();
        assert_eq!(leaf["cwd"], build.join("module").to_str().unwrap());
        assert_eq!(
            leaf["argv"],
            serde_json::json!(["-s", "-n", "--file=mmakefile", "selected"])
        );
        assert_eq!(
            fs::read(build.join("module/mmakefile")).unwrap(),
            declaration
        );
        assert_eq!(measure_tree_content_cas(&source).unwrap(), source_before);
        assert!(build.join("mmake.cache").is_file());
        results.push(
            serde_json::json!({"case": name, "events": expected, "status": output.status.code()}),
        );
    }
    pinned(&case.metamake, &case.metamake_sha256);
    pinned(&case.python, &case.python_sha256);
    assert_eq!(fs::read(&helper).unwrap(), HELPER.as_bytes());
    fs::write(
        case.output_root.join("result.json"),
        serde_json::to_vec_pretty(&results).unwrap(),
    )
    .unwrap();
}
