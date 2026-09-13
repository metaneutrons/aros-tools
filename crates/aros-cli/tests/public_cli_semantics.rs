//! Process-level semantic cases for public `aros` examples and contracts.
//!
//! The structural contract snapshots cover every public command and option. These
//! cases exercise the meanings that cannot be inferred from Clap alone: default
//! effects, rejected stale values, environment resolution, and controlled local
//! publication. They deliberately use only temporary directories and never
//! discover a checkout, contact a network service, or touch a physical board.

use aros_release::archive::BINARIES;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const BOARDS_WORKFLOW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs-site/src/content/docs/workflows/boards.md"
));
const INSTALLATION_GUIDE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs-site/src/content/docs/getting-started/installation.md"
));

const fn aros() -> &'static str {
    env!("CARGO_BIN_EXE_aros")
}

fn run(arguments: &[&str]) -> Output {
    Command::new(aros())
        .args(arguments)
        .output()
        .expect("public CLI semantic case must execute")
}

fn assert_success(output: &Output, case: &str) {
    assert!(
        output.status.success(),
        "{case} failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &Output, case: &str) -> String {
    assert!(
        !output.status.success(),
        "{case} unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn source_cache_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("aros-cli-source-cache-")
        // macOS commonly exposes its system temporary directory through the
        // /var -> /private/var symlink. CACHE-M2 correctly refuses a source
        // cache root with symlink components, so fixtures deliberately live
        // below this real workspace path instead.
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("create real source-cache fixture root")
}

fn genmf_cache_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temporary = source_cache_tempdir();
    let source = temporary.path().join("source");
    let cache = temporary.path().join("cache");
    fs::create_dir_all(source.join("config")).expect("create GenMF template directory");
    fs::create_dir_all(source.join("tools/genmf")).expect("create GenMF generator directory");
    fs::create_dir_all(source.join("rom")).expect("create GenMF MMake directory");
    fs::create_dir(&cache).expect("create real GenMF cache root");
    fs::write(source.join("config/make.tmpl"), "template\n").expect("write GenMF template");
    fs::write(
        source.join("tools/genmf/genmf.py"),
        "import pathlib, sys\ntemplate, source, output = map(pathlib.Path, sys.argv[1:])\noutput.write_bytes(template.read_bytes() + source.read_bytes())\n",
    )
    .expect("write GenMF generator");
    fs::write(source.join("rom/mmakefile"), "%build_program fixture\n")
        .expect("write GenMF MMake input");
    (temporary, source, cache)
}

#[cfg(unix)]
fn cargo_lifecycle_fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    use std::os::unix::fs::PermissionsExt as _;

    let temporary = source_cache_tempdir();
    let producer = temporary.path().join("producer");
    let tools = temporary.path().join("tools");
    let cache = temporary.path().join("cache");
    let bin = temporary.path().join("bin");
    fs::create_dir_all(producer.join("toolchains")).expect("create producer fixture");
    fs::create_dir(&tools).expect("create tools fixture");
    fs::create_dir(&cache).expect("create Cargo cache root");
    fs::create_dir(&bin).expect("create fixture executable root");
    fs::write(
        producer.join("toolchains/rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.96.1\"\nprofile = \"minimal\"\n",
    )
    .expect("write producer Rust pin");
    fs::write(
        tools.join("Cargo.toml"),
        "[package]\nname = \"fixture-tools\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .expect("write tools manifest");
    let package_checksum = "a".repeat(64);
    fs::write(
        tools.join("Cargo.lock"),
        format!(
            "version = 4\n\n[[package]]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{package_checksum}\"\n"
        ),
    )
    .expect("write tools lock");
    let git_init = Command::new("git")
        .current_dir(&tools)
        .args(["init", "-q", "--template="])
        .output()
        .expect("initialize tools fixture repository");
    assert_success(&git_init, "initialize tools fixture repository");
    let git_add = Command::new("git")
        .current_dir(&tools)
        .args(["add", "."])
        .output()
        .expect("stage tools fixture repository");
    assert_success(&git_add, "stage tools fixture repository");
    let git_commit = Command::new("git")
        .current_dir(&tools)
        .args([
            "-c",
            "user.name=AROS cache fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "test: fixture Cargo workspace",
        ])
        .output()
        .expect("commit tools fixture repository");
    assert_success(&git_commit, "commit tools fixture repository");

    let manifest = b"[package]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\n";
    let source = b"pub fn value() {}\n";
    let cargo = bin.join("cargo");
    fs::write(
        &cargo,
        format!(
            r#"#!/bin/sh
set -eu
case "$1" in
  --version)
    printf '%s\n' 'cargo 1.96.1 (fixture)'
    ;;
  vendor)
    vendor=
    for argument in "$@"; do vendor="$argument"; done
    /bin/mkdir -p "$vendor/fixture-dependency-1.0.0/src"
    printf '%s' '[package]
name = "fixture-dependency"
version = "1.0.0"
' > "$vendor/fixture-dependency-1.0.0/Cargo.toml"
    printf '%s' 'pub fn value() {{}}
' > "$vendor/fixture-dependency-1.0.0/src/lib.rs"
    printf '%s' '{{"files":{{"Cargo.toml":"{}","src/lib.rs":"{}"}},"package":"{}"}}' > "$vendor/fixture-dependency-1.0.0/.cargo-checksum.json"
    printf '%s\n' '[source.crates-io]' 'replace-with = "vendored-sources"' '[source.vendored-sources]' "directory = \"$vendor\""
    ;;
  *)
    exit 64
    ;;
esac
"#,
            aros_common::sha256_bytes(manifest),
            aros_common::sha256_bytes(source),
            package_checksum,
        ),
    )
    .expect("write fixture Cargo executable");
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))
        .expect("make fixture Cargo executable");
    (temporary, producer, tools, cache, cargo)
}

#[test]
#[cfg(unix)]
fn cargo_cache_lifecycle_is_exact_retained_and_preview_applied() {
    let (_temporary, producer, tools, cache, cargo) = cargo_lifecycle_fixture();
    let producer = producer.to_str().expect("producer path is UTF-8");
    let tools = tools.to_str().expect("tools path is UTF-8");
    let cache = cache.to_str().expect("cache path is UTF-8");
    let cargo = cargo.to_str().expect("Cargo path is UTF-8");
    let selection = [
        "--producer-dir",
        producer,
        "--tools-dir",
        tools,
        "--dir",
        cache,
        "--cargo",
        cargo,
    ];

    let status = run(&[
        "cache", "cargo", "status", "--dir", cache, "--format", "json",
    ]);
    assert_success(&status, "Cargo cache passive status");
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["schema"], "aros-cache-cargo-status-v1");
    assert_eq!(
        status["capabilities"],
        serde_json::json!(["status", "list", "fetch", "verify", "keep", "release", "remove"])
    );

    let mut fetch_arguments = vec!["cache", "cargo", "fetch"];
    fetch_arguments.extend(selection);
    fetch_arguments.extend(["--format", "json"]);
    let fetched = run(&fetch_arguments);
    assert_success(&fetched, "Cargo vendor fetch");
    let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
    assert_eq!(fetched["schema"], "aros-cache-cargo-fetch-v1");

    let mut keep_arguments = vec!["cache", "cargo", "keep"];
    keep_arguments.extend(selection);
    keep_arguments.extend(["--name", "release-candidate", "--format", "json"]);
    let kept = run(&keep_arguments);
    assert_success(&kept, "Cargo vendor retention creation");
    let kept: Value = serde_json::from_slice(&kept.stdout).unwrap();
    assert_eq!(kept["schema"], "aros-cache-cargo-keep-v1");
    assert_eq!(kept["retention"]["objects"].as_array().unwrap().len(), 1);

    let mut remove_arguments = vec!["cache", "cargo", "remove"];
    remove_arguments.extend(selection);
    remove_arguments.extend(["--format", "json"]);
    let blocked = run(&remove_arguments);
    assert_success(&blocked, "retained Cargo removal preview");
    let blocked: Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(blocked["schema"], "aros-cache-cargo-remove-v1");
    assert_eq!(blocked["preview"]["eligible"], false);
    assert_eq!(
        blocked["preview"]["blockers"][0]["name"],
        "release-candidate"
    );
    assert!(blocked["recoverability"]
        .as_str()
        .expect("Cargo preview reports recovery")
        .contains("cache cargo fetch"));
    assert!(blocked["offline_impact"]
        .as_str()
        .expect("Cargo preview reports offline impact")
        .contains("offline"));

    let released = run(&[
        "cache",
        "cargo",
        "release",
        "--dir",
        cache,
        "--name",
        "release-candidate",
        "--format",
        "json",
    ]);
    assert_success(&released, "Cargo vendor retention release");
    let released: Value = serde_json::from_slice(&released.stdout).unwrap();
    assert_eq!(released["schema"], "aros-cache-cargo-release-v1");

    let preview = run(&remove_arguments);
    assert_success(&preview, "eligible Cargo removal preview");
    let preview: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(preview["preview"]["eligible"], true);
    let token = preview["preview"]["apply_token"]
        .as_str()
        .expect("Cargo removal preview returns a token")
        .to_owned();
    let mut apply_arguments = vec!["cache", "cargo", "remove"];
    apply_arguments.extend(selection);
    apply_arguments.extend(["--apply", &token, "--format", "json"]);
    let removed = run(&apply_arguments);
    assert_success(&removed, "Cargo removal apply");
    let removed: Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(removed["operation"], "cargo.remove.apply");
    assert_eq!(removed["removal"]["outcome"], "object_removed");
}

#[test]
fn genmf_cache_commands_keep_content_addressed_generations_explicit() {
    let (_temporary, source, cache) = genmf_cache_fixture();
    let source = source.to_str().expect("source path is UTF-8");
    let cache = cache.to_str().expect("cache path is UTF-8");

    let status = run(&[
        "cache", "genmf", "status", "--dir", cache, "--format", "json",
    ]);
    assert_success(&status, "GenMF cache passive status");
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["schema"], "aros-cache-genmf-status-v1");
    assert_eq!(status["root"]["state"], "directory");
    assert_eq!(status["side_effects"]["creates_state"], false);
    assert!(
        !Path::new(cache).join("genmf").exists(),
        "status must not create a GenMF namespace"
    );

    let listed = run(&[
        "cache",
        "genmf",
        "list",
        "--source-dir",
        source,
        "--dir",
        cache,
        "--format",
        "json",
    ]);
    assert_success(&listed, "GenMF cache metadata list");
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed["schema"], "aros-cache-genmf-list-v1");
    assert_eq!(listed["entries"][0]["state"], "missing");

    let refreshed = run(&[
        "cache",
        "genmf",
        "refresh",
        "--source-dir",
        source,
        "--dir",
        cache,
        "--format",
        "json",
    ]);
    assert_success(&refreshed, "GenMF cache refresh");
    let refreshed: Value = serde_json::from_slice(&refreshed.stdout).unwrap();
    assert_eq!(refreshed["schema"], "aros-cache-genmf-refresh-v1");
    assert_eq!(refreshed["entries"].as_array().unwrap().len(), 1);
    assert!(
        refreshed["entries"][0]["generation_dir"]
            .as_str()
            .expect("generation directory is rendered")
            .contains("/genmf/v1/"),
        "refresh must publish only in the versioned GenMF namespace"
    );

    let verified = run(&[
        "cache",
        "genmf",
        "verify",
        "--source-dir",
        source,
        "--dir",
        cache,
        "--format",
        "json",
    ]);
    assert_success(&verified, "GenMF cache verification");
    let verified: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(verified["schema"], "aros-cache-genmf-verify-v1");
    assert_eq!(
        verified["entries"][0]["selection"]["source_relative_path"],
        "rom/mmakefile"
    );
    assert!(
        !Path::new(cache).join("rom%mmakefile.mk").exists(),
        "the public command must never recreate the legacy flat mtime cache"
    );
}

#[test]
fn public_board_init_semantic_cases_cover_models_defaults_and_environment() {
    let temporary = tempfile::tempdir().expect("temporary semantic-case root");
    let cases = [
        ("rpi3", "native-tftp"),
        ("rpi4", "native-tftp"),
        ("rpi5", "native-tftp"),
        ("milk-v-titan", "uefi-esp"),
    ];

    for (model, transport) in cases {
        let config = temporary.path().join(format!("{model}.toml"));
        let config = config.to_str().expect("temporary path is UTF-8");
        let output = run(&[
            "board",
            "init",
            "--profile",
            model,
            "--model",
            model,
            "--config",
            config,
        ]);
        assert_success(&output, "board init dry-run default transport");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(&format!("Model: {model}")));
        assert!(stdout.contains(&format!("Transport: {transport}")));
        assert!(
            !Path::new(config).exists(),
            "board init must stay non-mutating until --apply"
        );
    }

    let applied_config = temporary.path().join("configured-by-environment.toml");
    let output = Command::new(aros())
        .env("AROS_BOARDS_FILE", &applied_config)
        .args([
            "board",
            "init",
            "--profile",
            "semantic-rpi5",
            "--model",
            "rpi5",
            "--apply",
        ])
        .output()
        .expect("environment semantic case must execute");
    assert_success(&output, "board init environment-selected apply");
    let contents = fs::read_to_string(&applied_config).expect("applied template exists");
    assert!(contents.contains("[boards.semantic-rpi5]"));
    assert!(contents.contains("model = \"rpi5\""));
    assert!(contents.contains("transport = \"native-tftp\""));

    assert!(
        BOARDS_WORKFLOW.contains("aros board init --profile rpi4-usb --model rpi4"),
        "the documented board example must stay aligned with the exercised model selector"
    );
}

#[test]
fn relative_board_configuration_stays_at_the_invocation_directory() {
    let temporary = tempfile::tempdir().expect("temporary board configuration root");
    let nested = temporary.path().join("nested");
    fs::create_dir_all(&nested).expect("create nested invocation directory");

    let output = Command::new(aros())
        .current_dir(&nested)
        .args([
            "board",
            "init",
            "--profile",
            "relative-rpi4",
            "--model",
            "rpi4",
            "--config",
            "boards.toml",
            "--apply",
        ])
        .output()
        .expect("relative board configuration case must execute");
    assert_success(&output, "relative board configuration");
    assert!(
        nested.join("boards.toml").is_file(),
        "a relative board configuration must be created below the invocation directory"
    );
    assert!(
        !temporary.path().join("boards.toml").exists(),
        "repository discovery must not reinterpret board configuration relative to an ancestor"
    );
}

#[test]
fn source_cache_product_plan_keeps_unpinned_measurements_explicit() {
    let temporary = source_cache_tempdir();
    let cache = temporary.path().join("cache");
    fs::create_dir(&cache).expect("create source cache root");
    let payload = b"reviewed product input without an upstream pin\n";
    fs::write(cache.join("grub-2.12.tar.xz"), payload).expect("write cached product input");
    let plan = temporary.path().join("grub.fetch-plan.json");
    fs::write(
        &plan,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "aros-cache-source-fetch-plan-v1",
            "entries": [{
                "role": "product:grub@2.12",
                "filename": "grub-2.12.tar.xz",
                "candidates": [{
                    "url": "https://example.invalid/grub-2.12.tar.xz",
                }],
                "representation": "archive",
                "normalization": "exact-bytes-v1",
                "integrity": {
                    "kind": "unverified",
                    "max_size": 1_048_576,
                }
            }]
        }))
        .expect("serialize product source plan"),
    )
    .expect("write product source plan");
    let cache = cache.to_str().expect("cache path is UTF-8");
    let plan = plan.to_str().expect("plan path is UTF-8");

    let status = run(&[
        "cache", "sources", "status", "--dir", cache, "--format", "json",
    ]);
    assert_success(&status, "source cache passive status");
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["schema"], "aros-cache-sources-status-v1");
    assert_eq!(status["side_effects"]["hashes_payloads"], false);

    let list = run(&[
        "cache",
        "sources",
        "list",
        "--source-fetch-plan",
        plan,
        "--dir",
        cache,
        "--format",
        "json",
    ]);
    assert_success(&list, "source cache metadata list");
    let list: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(list["schema"], "aros-cache-sources-list-v1");
    assert_eq!(list["entries"][0]["state"], "present_unverified");
    assert_eq!(list["entries"][0]["integrity"]["kind"], "unverified");
    assert_eq!(list["side_effects"]["hashes_payloads"], false);

    let rejected = run(&[
        "cache",
        "sources",
        "fetch",
        "--source-fetch-plan",
        plan,
        "--dir",
        cache,
        "--offline",
    ]);
    let diagnostic = assert_failure(&rejected, "unverified source fetch without opt-in");
    assert!(diagnostic.contains("--allow-unverified"), "{diagnostic}");
    assert_eq!(
        fs::read(Path::new(cache).join("grub-2.12.tar.xz")).unwrap(),
        payload
    );

    let fetched = run(&[
        "cache",
        "sources",
        "fetch",
        "--source-fetch-plan",
        plan,
        "--dir",
        cache,
        "--offline",
        "--allow-unverified",
        "--format",
        "json",
    ]);
    assert_success(&fetched, "offline unverified source-cache measurement");
    let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
    assert_eq!(fetched["schema"], "aros-cache-sources-fetch-v1");
    assert_eq!(fetched["entries"][0]["integrity"], "measured_unpinned");
    assert_eq!(fetched["side_effects"]["network"], false);

    let parser_rejection = run(&[
        "cache",
        "sources",
        "fetch",
        "--source-lock",
        plan,
        "--dir",
        cache,
        "--allow-unverified",
    ]);
    let parser_diagnostic = assert_failure(
        &parser_rejection,
        "allow-unverified without product source-fetch plan",
    );
    assert!(
        parser_diagnostic.contains("--source-fetch-plan"),
        "{parser_diagnostic}"
    );
}

#[test]
fn source_cache_lifecycle_retains_a_closed_selection_and_removes_only_one_role() {
    let temporary = source_cache_tempdir();
    let cache = temporary.path().join("cache");
    fs::create_dir(&cache).expect("create source cache root");
    let payload = b"reviewed product input without an upstream pin\n";
    fs::write(cache.join("grub-2.12.tar.xz"), payload).expect("write cached product input");
    let plan = temporary.path().join("grub.fetch-plan.json");
    fs::write(
        &plan,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "aros-cache-source-fetch-plan-v1",
            "entries": [{
                "role": "product:grub@2.12",
                "filename": "grub-2.12.tar.xz",
                "candidates": [{"url": "https://example.invalid/grub-2.12.tar.xz"}],
                "representation": "archive",
                "normalization": "exact-bytes-v1",
                "integrity": {"kind": "unverified", "max_size": 1_048_576}
            }]
        }))
        .expect("serialize product source plan"),
    )
    .expect("write product source plan");
    let cache = cache.to_str().expect("cache path is UTF-8");
    let plan = plan.to_str().expect("plan path is UTF-8");
    let role = "product:grub@2.12";

    let kept = run(&[
        "cache",
        "sources",
        "keep",
        "--source-fetch-plan",
        plan,
        "--dir",
        cache,
        "--name",
        "release-candidate",
        "--format",
        "json",
    ]);
    assert_success(&kept, "source cache keep");
    let kept: Value = serde_json::from_slice(&kept.stdout).unwrap();
    assert_eq!(kept["schema"], "aros-cache-sources-keep-v1");
    assert_eq!(kept["retention"]["objects"].as_array().unwrap().len(), 1);

    let blocked = run(&[
        "cache",
        "sources",
        "remove",
        "--source-fetch-plan",
        plan,
        "--dir",
        cache,
        "--role",
        role,
        "--format",
        "json",
    ]);
    assert_success(&blocked, "retained source cache removal preview");
    let blocked: Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(blocked["schema"], "aros-cache-sources-remove-v1");
    assert_eq!(blocked["preview"]["eligible"], false);
    assert_eq!(
        blocked["preview"]["blockers"][0]["name"],
        "release-candidate"
    );
    assert!(blocked["recoverability"]
        .as_str()
        .unwrap()
        .contains("same reviewed selector"));

    let released = run(&[
        "cache",
        "sources",
        "release",
        "--dir",
        cache,
        "--name",
        "release-candidate",
        "--format",
        "json",
    ]);
    assert_success(&released, "source cache release");
    let released: Value = serde_json::from_slice(&released.stdout).unwrap();
    assert_eq!(released["schema"], "aros-cache-sources-release-v1");

    let preview = run(&[
        "cache",
        "sources",
        "remove",
        "--source-fetch-plan",
        plan,
        "--dir",
        cache,
        "--role",
        role,
        "--format",
        "json",
    ]);
    assert_success(&preview, "unretained source cache removal preview");
    let preview: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(preview["preview"]["eligible"], true);
    let token = preview["preview"]["apply_token"]
        .as_str()
        .expect("preview returns a token")
        .to_owned();

    let removed = run(&[
        "cache",
        "sources",
        "remove",
        "--source-fetch-plan",
        plan,
        "--dir",
        cache,
        "--role",
        role,
        "--apply",
        &token,
        "--format",
        "json",
    ]);
    assert_success(&removed, "source cache removal apply");
    let removed: Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(removed["removal"]["outcome"], "object_removed");
    assert!(
        !Path::new(cache).join("grub-2.12.tar.xz").exists(),
        "source lifecycle removal must delete only the previewed direct object"
    );
}

#[test]
fn archive_cache_uses_one_explicit_cross_host_selection_without_installing() {
    let temporary = tempfile::tempdir().expect("temporary archive-cache semantic root");
    let project = temporary.path().join("AROS");
    fs::create_dir(&project).expect("create selected project");
    for directory in ["arch", "compiler", "rom"] {
        fs::create_dir(project.join(directory)).expect("create AROS checkout marker directory");
    }
    fs::write(project.join("configure"), "").expect("write AROS configure marker");
    fs::write(project.join("Makefile.in"), "").expect("write AROS Makefile marker");
    let payload = b"reviewed compiler archive bytes\n";
    let payload_file = temporary.path().join("payload.tar.xz");
    fs::write(&payload_file, payload).expect("write archive payload fixture");
    let sha256 = aros_common::sha256_file(&payload_file)
        .expect("hash archive payload fixture")
        .digest
        .to_string();
    let tree_sha256 = "b".repeat(64);
    fs::write(
        project.join("aros-targets.toml"),
        format!(
            "[host_compiler]\nllvm_version = '18.1.8'\nbase_url = 'https://example.invalid/llvm/{{version}}'\n\n[host_compiler.hosts.linux-x86_64]\nasset = 'llvm-{{version}}-linux-x86_64.tar.xz'\nsha256 = '{sha256}'\n\n[[targets]]\nname = 'pc-x86_64'\narch = 'x86_64'\nplatform = 'pc'\nbsp = 'pc'\n"
        ),
    )
    .expect("write target configuration");
    fs::write(
        project.join("aros-toolchains.lock.toml"),
        format!(
            "schema = 1\nrelease_id = 'fixture-release'\nbase_url = 'https://example.invalid/toolchains'\n\n[[artifacts]]\nhost = 'linux-x86_64'\ntarget_profile = 'pc-x86_64'\ntarget_triple = 'x86_64-unknown-aros'\nasset = 'fixture.tar.xz'\nsha256 = '{sha256}'\ntree_sha256 = '{tree_sha256}'\nsize = {}\nenabled = true\nrequired_paths = []\n",
            payload.len()
        ),
    )
    .expect("write toolchain lock");

    let empty_root = temporary.path().join("empty-archive-cache");
    let status = Command::new(aros())
        .env("AROS_CACHE_DIR", &empty_root)
        .args(["cache", "archives", "status", "--format", "json"])
        .output()
        .expect("archive status executes");
    assert_success(&status, "archive cache passive status");
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["schema"], "aros-cache-archives-status-v1");
    assert_eq!(status["root"]["state"], "missing");
    assert_eq!(
        status["capabilities"],
        serde_json::json!(["status", "list", "fetch", "verify", "keep", "release", "remove"])
    );
    assert_eq!(status["side_effects"]["creates_state"], false);
    assert!(
        !empty_root.exists(),
        "archive status must not create its root"
    );

    let offline_miss = Command::new(aros())
        .env("AROS_CACHE_DIR", &empty_root)
        .args([
            "cache",
            "archives",
            "fetch",
            "--project",
            project.to_str().expect("project path is UTF-8"),
            "--toolchain",
            "--preset",
            "pc-x86_64",
            "--host",
            "linux-x86_64",
            "--offline",
        ])
        .output()
        .expect("offline archive miss executes");
    let offline_diagnostic = assert_failure(&offline_miss, "offline archive cache miss");
    assert!(
        offline_diagnostic.contains("offline mode"),
        "{offline_diagnostic}"
    );
    assert!(
        !empty_root.exists(),
        "offline archive fetch must not create a missing cache root"
    );

    let cache_root = temporary.path().join("archive-cache");
    let installed_host_compiler = temporary.path().join("installed-host-compiler");
    let installed_cross_toolchains = temporary.path().join("installed-cross-toolchains");
    fs::create_dir(&installed_host_compiler).expect("create host compiler sentinel root");
    fs::create_dir(&installed_cross_toolchains).expect("create cross-toolchain sentinel root");
    fs::write(
        installed_host_compiler.join("sentinel"),
        b"host installation",
    )
    .expect("write host compiler sentinel");
    fs::write(
        installed_cross_toolchains.join("sentinel"),
        b"cross installation",
    )
    .expect("write cross-toolchain sentinel");
    let cache_path = cache_root
        .join("downloads/sha256")
        .join(format!("{sha256}.tar.xz"));
    fs::create_dir_all(cache_path.parent().unwrap()).expect("create archive cache fixture");
    fs::write(&cache_path, payload).expect("write cached archive fixture");
    let project = project.to_str().expect("project path is UTF-8");
    let cache_arguments = [
        "cache",
        "archives",
        "list",
        "--project",
        project,
        "--toolchain",
        "--preset",
        "pc-x86_64",
        "--host",
        "linux-x86_64",
        "--format",
        "json",
    ];
    let listed = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .args(cache_arguments)
        .output()
        .expect("archive list executes");
    assert_success(&listed, "cross-host archive metadata list");
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed["schema"], "aros-cache-archives-list-v1");
    assert_eq!(listed["selection"]["host"], "linux-x86_64");
    assert_eq!(listed["selection"]["host_selection"], "explicit");
    assert_eq!(
        listed["selection"]["configuration_kind"],
        "aros-toolchains.lock.toml"
    );
    assert_eq!(listed["selection"]["expected_size"], payload.len());
    assert_eq!(listed["entry"]["state"], "present_unverified");
    assert_eq!(listed["side_effects"]["hashes_payloads"], false);

    let verified = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .args([
            "cache",
            "archives",
            "verify",
            "--project",
            project,
            "--toolchain",
            "--preset",
            "pc-x86_64",
            "--host",
            "linux-x86_64",
            "--format",
            "json",
        ])
        .output()
        .expect("archive verify executes");
    assert_success(&verified, "cross-host archive byte verification");
    let verified: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(verified["schema"], "aros-cache-archives-verify-v1");
    assert_eq!(
        verified["verification_scope"],
        "archive_bytes_exact_size_and_sha256"
    );
    assert!(verified["not_verified"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "payload tree identity"));

    let kept = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .args([
            "cache",
            "archives",
            "keep",
            "--project",
            project,
            "--toolchain",
            "--preset",
            "pc-x86_64",
            "--host",
            "linux-x86_64",
            "--name",
            "release-candidate",
            "--format",
            "json",
        ])
        .output()
        .expect("archive keep executes");
    assert_success(&kept, "archive retention creation");
    let kept: Value = serde_json::from_slice(&kept.stdout).unwrap();
    assert_eq!(kept["schema"], "aros-cache-archives-keep-v1");
    assert_eq!(kept["retention"]["objects"].as_array().unwrap().len(), 1);

    let blocked = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .args([
            "cache",
            "archives",
            "remove",
            "--project",
            project,
            "--toolchain",
            "--preset",
            "pc-x86_64",
            "--host",
            "linux-x86_64",
            "--format",
            "json",
        ])
        .output()
        .expect("blocked archive removal preview executes");
    assert_success(&blocked, "retained archive removal preview");
    let blocked: Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(blocked["schema"], "aros-cache-archives-remove-v1");
    assert_eq!(blocked["preview"]["eligible"], false);
    assert_eq!(
        blocked["preview"]["blockers"][0]["name"],
        "release-candidate"
    );

    let released = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .args([
            "cache",
            "archives",
            "release",
            "--name",
            "release-candidate",
            "--format",
            "json",
        ])
        .output()
        .expect("archive release executes");
    assert_success(&released, "archive retention release");
    let released: Value = serde_json::from_slice(&released.stdout).unwrap();
    assert_eq!(released["schema"], "aros-cache-archives-release-v1");

    let preview = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .args([
            "cache",
            "archives",
            "remove",
            "--project",
            project,
            "--toolchain",
            "--preset",
            "pc-x86_64",
            "--host",
            "linux-x86_64",
            "--format",
            "json",
        ])
        .output()
        .expect("eligible archive removal preview executes");
    assert_success(&preview, "eligible archive removal preview");
    let preview: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(preview["preview"]["eligible"], true);
    let token = preview["preview"]["apply_token"]
        .as_str()
        .expect("removal preview returns an apply token");

    let removed = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .args([
            "cache",
            "archives",
            "remove",
            "--project",
            project,
            "--toolchain",
            "--preset",
            "pc-x86_64",
            "--host",
            "linux-x86_64",
            "--apply",
            token,
            "--format",
            "json",
        ])
        .output()
        .expect("archive removal apply executes");
    assert_success(&removed, "token-confirmed archive removal");
    let removed: Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(removed["operation"], "archives.remove.apply");
    assert!(!cache_path.exists(), "exact selected archive was removed");

    fs::write(&cache_path, payload).expect("restore cached archive fixture after lifecycle test");

    fs::write(&cache_path, b"corrupt compiler archive files!\n")
        .expect("corrupt cached archive fixture");
    let mismatch = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .args([
            "cache",
            "archives",
            "verify",
            "--project",
            project,
            "--toolchain",
            "--preset",
            "pc-x86_64",
            "--host",
            "linux-x86_64",
        ])
        .output()
        .expect("mismatched archive verify executes");
    let mismatch_diagnostic = assert_failure(&mismatch, "mismatched archive verification");
    assert!(
        mismatch_diagnostic.contains("SHA256 mismatch"),
        "{mismatch_diagnostic}"
    );
    assert_eq!(
        fs::read(&cache_path).unwrap(),
        b"corrupt compiler archive files!\n",
        "archive verification must not repair or replace a mismatched object"
    );
    fs::write(&cache_path, payload).expect("restore cached archive fixture");

    let fetched = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .env("AROS_HOST_COMPILER_DIR", &installed_host_compiler)
        .env("AROS_CROSS_TOOLCHAINS_DIR", &installed_cross_toolchains)
        .args([
            "cache",
            "archives",
            "fetch",
            "--project",
            project,
            "--toolchain",
            "--preset",
            "pc-x86_64",
            "--host",
            "linux-x86_64",
            "--offline",
            "--format",
            "json",
        ])
        .output()
        .expect("offline archive fetch executes");
    assert_success(&fetched, "offline archive cache reuse");
    let fetched: Value = serde_json::from_slice(&fetched.stdout).unwrap();
    assert_eq!(fetched["schema"], "aros-cache-archives-fetch-v1");
    assert_eq!(fetched["side_effects"]["network"], false);
    assert_eq!(fetched["side_effects"]["creates_state"], false);
    assert_eq!(fs::read(&cache_path).unwrap(), payload);
    assert_eq!(
        fs::read(installed_host_compiler.join("sentinel")).unwrap(),
        b"host installation"
    );
    assert_eq!(
        fs::read(installed_cross_toolchains.join("sentinel")).unwrap(),
        b"cross installation"
    );

    let online_reuse = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .args([
            "cache",
            "archives",
            "fetch",
            "--project",
            project,
            "--toolchain",
            "--preset",
            "pc-x86_64",
            "--host",
            "linux-x86_64",
            "--format",
            "json",
        ])
        .output()
        .expect("online archive cache reuse executes");
    assert_success(&online_reuse, "verified archive cache reuse");
    let online_reuse: Value = serde_json::from_slice(&online_reuse.stdout).unwrap();
    assert_eq!(online_reuse["side_effects"]["network"], false);
    assert_eq!(online_reuse["side_effects"]["creates_state"], false);

    let host_compiler = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .args([
            "cache",
            "archives",
            "list",
            "--project",
            project,
            "--host-compiler",
            "--host",
            "linux-x86_64",
            "--format",
            "json",
        ])
        .output()
        .expect("host compiler archive list executes");
    assert_success(&host_compiler, "configured host compiler archive list");
    let host_compiler: Value = serde_json::from_slice(&host_compiler.stdout).unwrap();
    assert_eq!(host_compiler["selection"]["kind"], "host_compiler");
    assert_eq!(
        host_compiler["selection"]["configuration_kind"],
        "aros-targets.toml host_compiler"
    );
    assert!(host_compiler["selection"]["expected_size"].is_null());
    assert_eq!(
        host_compiler["selection"]["cache_path"], listed["selection"]["cache_path"],
        "host and cross-toolchain selection with one SHA-256 must share exactly one archive object"
    );

    let overridden_host_compiler = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .env(
            "AROS_HOST_COMPILER_URL",
            "https://mirror.example.invalid/llvm",
        )
        .args([
            "cache",
            "archives",
            "list",
            "--project",
            project,
            "--host-compiler",
            "--host",
            "linux-x86_64",
            "--format",
            "json",
        ])
        .output()
        .expect("overridden host compiler archive list executes");
    assert_success(
        &overridden_host_compiler,
        "host compiler archive transport override",
    );
    let overridden_host_compiler: Value =
        serde_json::from_slice(&overridden_host_compiler.stdout).unwrap();
    assert_eq!(
        overridden_host_compiler["selection"]["transport_source"],
        "AROS_HOST_COMPILER_URL"
    );
    assert!(overridden_host_compiler["selection"]["url"]
        .as_str()
        .unwrap()
        .starts_with("https://mirror.example.invalid/llvm/"));

    let rejected_override = Command::new(aros())
        .env("AROS_CACHE_DIR", &cache_root)
        .env(
            "AROS_HOST_COMPILER_URL",
            "https://operator:secret@example.invalid/llvm",
        )
        .args([
            "cache",
            "archives",
            "list",
            "--project",
            project,
            "--host-compiler",
            "--host",
            "linux-x86_64",
        ])
        .output()
        .expect("invalid host compiler transport override executes");
    let override_diagnostic = assert_failure(
        &rejected_override,
        "credential-bearing host compiler transport override",
    );
    assert!(
        override_diagnostic.contains("URL must not contain credentials"),
        "{override_diagnostic}"
    );
    assert!(
        !override_diagnostic.contains("secret"),
        "credential-bearing transport must be redacted: {override_diagnostic}"
    );

    let missing_preset = assert_failure(
        &run(&[
            "cache",
            "archives",
            "list",
            "--project",
            project,
            "--toolchain",
        ]),
        "toolchain archive selector without preset",
    );
    assert!(missing_preset.contains("--preset"), "{missing_preset}");
    let conflicting_transport = assert_failure(
        &run(&[
            "cache",
            "archives",
            "fetch",
            "--project",
            project,
            "--host-compiler",
            "--offline",
            "--refresh",
        ]),
        "archive offline and refresh conflict",
    );
    assert!(
        conflicting_transport.contains("--offline"),
        "{conflicting_transport}"
    );
}

#[test]
fn relative_local_prefix_and_sd_artifact_stay_at_the_invocation_directory() {
    let temporary = tempfile::tempdir().expect("temporary relative-path root");
    let checkout = temporary.path().join("AROS");
    create_checkout_markers(&checkout);
    let nested = checkout.join("developer/invocation");
    fs::create_dir_all(&nested).expect("create nested invocation directory");

    let local_prefix = nested.join("local-toolchain");
    fs::create_dir_all(&local_prefix).expect("create relative local prefix");
    let local = Command::new(aros())
        .current_dir(&nested)
        .args([
            "toolchain",
            "path",
            "--preset",
            "pc-x86_64",
            "--local",
            "local-toolchain",
        ])
        .output()
        .expect("relative local-prefix case must execute");
    let local_diagnostic = assert_failure(&local, "relative local prefix");
    assert!(
        local_diagnostic.contains("has no manifest"),
        "the existing nested prefix must be selected before its validation fails: {local_diagnostic}"
    );

    let artifact = nested.join("sd-artifact");
    fs::create_dir_all(&artifact).expect("create relative artifact directory");
    let artifact_result = Command::new(aros())
        .current_dir(&nested)
        .args([
            "--diagnostic-format=json",
            "board",
            "sd",
            "scan",
            "--artifact",
            "sd-artifact",
        ])
        .output()
        .expect("relative SD artifact case must execute");
    let artifact_diagnostic = assert_failure(&artifact_result, "relative SD artifact");
    assert!(
        artifact_diagnostic.contains(&artifact.display().to_string()),
        "the diagnostic must identify the nested artifact path: {artifact_diagnostic}"
    );
}

#[test]
fn public_parser_semantic_cases_reject_stale_values_and_conflicts_before_setup() {
    let missing_model = assert_failure(
        &run(&["board", "init", "--profile", "missing-model"]),
        "board init without its required model",
    );
    assert!(missing_model.contains("--model <MODEL>"));

    let stale_model = assert_failure(
        &run(&[
            "board",
            "init",
            "--profile",
            "stale-model",
            "--model",
            "rpi2",
        ]),
        "board init with a deliberately stale model value",
    );
    assert!(stale_model.contains("invalid value 'rpi2'"));
    assert!(stale_model.contains("rpi3, rpi4, rpi5, milk-v-titan"));

    let conflicting_setup = assert_failure(
        &run(&["setup", "--preset", "pc-x86_64", "--all"]),
        "setup with mutually exclusive scope selectors",
    );
    assert!(conflicting_setup.contains("cannot be used with '--all'"));
    assert!(
        !conflicting_setup.contains("repository discovery"),
        "parser conflicts must fail before repository discovery"
    );
}

#[test]
fn diagnostic_format_environment_has_the_documented_process_effect() {
    let output = Command::new(aros())
        .env("AROS_DIAGNOSTIC_FORMAT", "json")
        .arg("deliberately-stale-public-command")
        .output()
        .expect("diagnostic environment semantic case must execute");
    assert_failure(&output, "unknown command under JSON diagnostic environment");
    let diagnostic: Value =
        serde_json::from_slice(&output.stderr).expect("environment-selected diagnostics are JSON");
    assert_eq!(diagnostic["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(diagnostic["diagnostics"][0]["code"], "AR0001");
}

#[test]
fn installation_intent_conflicts_are_rejected_before_logging_or_repository_access() {
    let temporary = tempfile::tempdir().expect("temporary invocation root");
    let cases: &[&[&str]] = &[
        &["setup", "--preset", "pc-x86_64", "--force"],
        &["host-compiler", "install", "--force"],
        &["toolchain", "install", "--preset", "pc-x86_64", "--force"],
    ];
    for (index, arguments) in cases.iter().enumerate() {
        let log_file = temporary.path().join(format!("conflict-{index}.jsonl"));
        let output = Command::new(aros())
            .current_dir(temporary.path())
            .env("AROS_OFFLINE", "true")
            .arg("--log-file")
            .arg(&log_file)
            .args(*arguments)
            .output()
            .expect("installation conflict semantic case must execute");
        let diagnostic = assert_failure(&output, "environment-selected offline conflict");
        assert!(diagnostic.contains("cannot be used with"));
        assert!(diagnostic.contains("--force"));
        assert!(diagnostic.contains("--offline"));
        assert!(
            !diagnostic.contains("repository discovery"),
            "an invocation conflict must precede repository discovery"
        );
        assert!(
            !log_file.exists(),
            "an invocation conflict must precede log-file creation"
        );
    }
}

fn create_checkout_markers(root: &Path) {
    for directory in ["arch", "compiler", "rom", "nested"] {
        fs::create_dir_all(root.join(directory)).expect("checkout marker directory");
    }
    for file in ["configure", "Makefile.in"] {
        fs::write(root.join(file), "").expect("checkout marker file");
    }
}

#[test]
fn explicit_clean_scope_and_relative_log_file_keep_the_invocation_origin() {
    let temporary = tempfile::tempdir().expect("temporary checkout root");
    let checkout = temporary.path().join("AROS");
    create_checkout_markers(&checkout);
    let selected = checkout.join("build/pc-x86_64");
    let preserved = checkout.join("build/other-preset");
    let cache = checkout.join("cache/sentinel");
    fs::create_dir_all(&selected).expect("selected build directory");
    fs::create_dir_all(&preserved).expect("other build directory");
    fs::create_dir_all(cache.parent().expect("cache parent")).expect("cache directory");
    fs::write(selected.join("selected"), "remove").expect("selected build payload");
    fs::write(preserved.join("preserved"), "keep").expect("preserved build payload");
    fs::write(&cache, "keep").expect("cache sentinel");

    let nested = checkout.join("nested");
    let preview = Command::new(aros())
        .current_dir(&nested)
        .args(["clean", "--preset", "pc-x86_64", "--dry-run"])
        .output()
        .expect("clean preview semantic case must execute");
    assert_success(&preview, "explicit clean preview");
    assert!(String::from_utf8_lossy(&preview.stdout).contains(&selected.display().to_string()));
    assert!(
        selected.exists(),
        "clean preview must not remove its target"
    );

    let log_file = nested.join("invocation.jsonl");
    let applied = Command::new(aros())
        .current_dir(&nested)
        .args([
            "--log-level",
            "info",
            "--log-file",
            "invocation.jsonl",
            "clean",
            "--preset",
            "pc-x86_64",
        ])
        .output()
        .expect("explicit clean apply semantic case must execute");
    assert_success(&applied, "explicit preset clean");
    assert!(
        !selected.exists(),
        "selected build directory must be removed"
    );
    assert!(preserved.exists(), "other build presets must be preserved");
    assert!(cache.exists(), "checkout caches must be preserved");
    assert!(
        log_file.is_file(),
        "a relative log file must resolve from the invocation directory"
    );
}

#[cfg(unix)]
fn write_executable(path: &Path, contents: &[u8]) {
    use std::os::unix::fs::PermissionsExt as _;

    fs::create_dir_all(path.parent().expect("synthetic executable parent"))
        .expect("create synthetic executable parent");
    fs::write(path, contents).expect("write synthetic suite member");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("make synthetic suite member executable");
}

#[cfg(unix)]
fn write_legacy_pc_toolchain(root: &Path) {
    for tool in [
        "clang",
        "clang++",
        "ld.lld",
        "llvm-ar",
        "aros-collect",
        "collect-aros",
        "collect-aros32",
    ] {
        write_executable(
            &root.join("bin").join(tool),
            b"#!/bin/sh\nprintf '%s\\n' 'fixture 1.0'\n",
        );
    }
    for marker in [
        ".installflag-llvm-x86_64",
        ".installflag-compiler_rt-x86_64",
    ] {
        fs::write(root.join(marker), b"complete\n").expect("write legacy toolchain marker");
    }
    let headers = root.join("include/c++/v1");
    fs::create_dir_all(&headers).expect("create legacy C++ header directory");
    for header in [
        "algorithm",
        "cerrno",
        "cinttypes",
        "cstddef",
        "cstdint",
        "deque",
        "memory",
        "string",
        "system_error",
        "vector",
    ] {
        fs::write(headers.join(header), b"fixture\n").expect("write legacy C++ header");
    }
    let libraries = root.join("lib");
    fs::create_dir_all(&libraries).expect("create legacy library directory");
    for library in ["libc++.a", "libc++abi.a", "libunwind.a"] {
        fs::write(libraries.join(library), b"fixture\n").expect("write legacy C++ library");
    }
}

#[cfg(unix)]
fn write_complete_build_tool_suite(root: &Path) {
    for tool in [
        "aros-transpiler",
        "aros-genmodule",
        "aros-romtool",
        "aros-collect",
        "aros-ahi-runner",
        "aros-fetch",
    ] {
        write_executable(
            &root.join(tool),
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{tool} {}'\n",
                env!("CARGO_PKG_VERSION")
            )
            .as_bytes(),
        );
    }
}

#[cfg(unix)]
#[test]
fn relative_engine_override_stays_at_the_invocation_directory() {
    let temporary = tempfile::tempdir().expect("temporary engine-path root");
    let checkout = temporary.path().join("AROS");
    create_checkout_markers(&checkout);
    let nested = checkout.join("developer/invocation");
    fs::create_dir_all(&nested).expect("create nested invocation directory");

    let local_toolchain = nested.join("local-toolchain");
    write_legacy_pc_toolchain(&local_toolchain);
    let build_tools = nested.join("build-tools");
    write_complete_build_tool_suite(&build_tools);
    let engine = nested.join("engine");
    fs::create_dir_all(&engine).expect("create relative engine override");

    let output = Command::new(aros())
        .current_dir(&nested)
        .env("AROS_BUILD_TOOLS_DIR", &build_tools)
        .args([
            "build",
            "--preset",
            "pc-x86_64",
            "--offline",
            "--toolchain-dir",
            "local-toolchain",
            "--engine-dir",
            "engine",
        ])
        .output()
        .expect("relative engine override case must execute");
    let diagnostic = assert_failure(&output, "relative engine override");
    assert!(
        diagnostic.contains(&engine.display().to_string()),
        "the engine diagnostic must identify the nested explicit override: {diagnostic}"
    );
    assert!(
        diagnostic.contains("AROS.cmake is missing"),
        "the existing nested engine must reach engine validation: {diagnostic}"
    );
}

#[cfg(unix)]
#[test]
fn native_suite_install_semantic_cases_require_an_exact_snapshotted_suite() {
    let temporary = tempfile::tempdir().expect("temporary suite root");
    let source = temporary.path().join("source");
    let prefix = temporary.path().join("prefix");
    fs::create_dir_all(&source).expect("create source suite directory");
    fs::create_dir_all(&prefix).expect("create installation prefix");
    for (index, name) in BINARIES.iter().enumerate() {
        write_executable(
            &source.join(name),
            format!("semantic-suite-member-{index}\n").as_bytes(),
        );
    }

    let output = Command::new(aros())
        .args(["install", "--source-bin"])
        .arg(&source)
        .arg("--prefix")
        .arg(&prefix)
        .output()
        .expect("native suite install semantic case must execute");
    assert_success(&output, "exact native suite installation");
    for (index, name) in BINARIES.iter().enumerate() {
        assert_eq!(
            fs::read(prefix.join("bin").join(name)).expect("installed suite member"),
            format!("semantic-suite-member-{index}\n").as_bytes()
        );
    }

    let stale_source = temporary.path().join("stale-source");
    let stale_prefix = temporary.path().join("stale-prefix");
    fs::create_dir_all(&stale_source).expect("create stale source suite directory");
    fs::create_dir_all(&stale_prefix).expect("create stale installation prefix");
    for name in BINARIES {
        write_executable(&stale_source.join(name), b"expected-member\n");
    }
    write_executable(&stale_source.join("unexpected-tool"), b"stale-member\n");
    let rejected = Command::new(aros())
        .args(["install", "--source-bin"])
        .arg(&stale_source)
        .arg("--prefix")
        .arg(&stale_prefix)
        .output()
        .expect("stale suite semantic case must execute");
    let rejected_diagnostic = assert_failure(&rejected, "stale suite inventory");
    assert!(rejected_diagnostic.contains("source suite inventory is not exact"));
    assert!(
        !stale_prefix.join("bin").exists(),
        "a rejected suite must not publish a partial destination"
    );
    assert!(
        INSTALLATION_GUIDE.contains("validates the exact eight-file inventory"),
        "the installation guide must keep the exercised exact-inventory boundary"
    );
}
