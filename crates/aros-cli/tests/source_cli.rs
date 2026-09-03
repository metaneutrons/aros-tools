#![cfg(unix)]

use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const fn aros() -> &'static str {
    env!("CARGO_BIN_EXE_aros")
}

fn run(command: &mut Command) -> Output {
    command
        .env_remove("AROS_DIAGNOSTIC_FORMAT")
        .env_remove("AROS_LOG_LEVEL")
        .env_remove("AROS_LOG_FORMAT")
        .env_remove("AROS_LOG_FILE")
        .env_remove("AROS_UPSTREAM_URL")
        .output()
        .expect("command execution")
}

fn git<const N: usize>(directory: &Path, arguments: [&str; N]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .expect("Git execution");
    assert!(
        output.status.success(),
        "Git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn git_path<const N: usize>(directory: &Path, arguments: [&str; N], path: &Path) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .arg(path)
        .output()
        .expect("Git execution");
    assert!(
        output.status.success(),
        "Git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Upstream {
    root: tempfile::TempDir,
    seed: PathBuf,
    remote: PathBuf,
}

impl Upstream {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary upstream");
        let seed = root.path().join("seed");
        let remote = root.path().join("upstream.git");
        fs::create_dir(&seed).expect("seed directory");

        let init = Command::new("git")
            .arg("-C")
            .arg(&seed)
            .args(["init", "--initial-branch=master"])
            .output()
            .expect("Git init");
        assert!(init.status.success());
        git(&seed, ["config", "user.name", "AROS test"]);
        git(&seed, ["config", "user.email", "aros-test@example.invalid"]);
        for directory in ["arch", "compiler", "rom"] {
            fs::create_dir_all(seed.join(directory)).expect("AROS marker directory");
            fs::write(seed.join(directory).join(".keep"), "").expect("tracked marker");
        }
        fs::write(seed.join("configure"), "#!/bin/sh\n").expect("configure marker");
        fs::write(seed.join("Makefile.in"), "# test\n").expect("Makefile marker");
        fs::write(
            seed.join("aros-targets.toml"),
            "[[targets]]\nname='pc-x86_64'\narch='x86_64'\nplatform='pc'\nbsp='pc'\n\
             [targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='llvm'\ncpu32='i386'\nuse_mmu=true\n",
        )
        .expect("target profiles");
        git(&seed, ["add", "."]);
        git(&seed, ["commit", "-m", "initial AROS source"]);

        let bare = Command::new("git")
            .args(["init", "--bare"])
            .arg(&remote)
            .output()
            .expect("bare Git init");
        assert!(bare.status.success());
        git_path(&seed, ["remote", "add", "origin"], &remote);
        git(&seed, ["push", "--set-upstream", "origin", "master"]);
        git(&remote, ["symbolic-ref", "HEAD", "refs/heads/master"]);

        Self { root, seed, remote }
    }

    fn commit_and_push(&self, name: &str, contents: &str) -> String {
        fs::write(self.seed.join(name), contents).expect("upstream change");
        git(&self.seed, ["add", name]);
        git(&self.seed, ["commit", "-m", name]);
        git(&self.seed, ["push", "origin", "master"]);
        git(&self.seed, ["rev-parse", "HEAD"])
    }

    fn remove_and_push(&self, name: &str) -> String {
        git(&self.seed, ["rm", name]);
        git(
            &self.seed,
            ["commit", "-m", "remove checkout-owned tools configuration"],
        );
        git(&self.seed, ["push", "origin", "master"]);
        git(&self.seed, ["rev-parse", "HEAD"])
    }
}

fn add_component(upstream: &Upstream) -> PathBuf {
    add_component_at(upstream, "external/component")
}

fn add_component_at(upstream: &Upstream, submodule_path: &str) -> PathBuf {
    let component = upstream.root.path().join("component");
    fs::create_dir(&component).expect("component repository");
    let init = Command::new("git")
        .arg("-C")
        .arg(&component)
        .args(["init", "--initial-branch=master"])
        .output()
        .expect("component init");
    assert!(init.status.success());
    git(&component, ["config", "user.name", "AROS test"]);
    git(
        &component,
        ["config", "user.email", "aros-test@example.invalid"],
    );
    fs::write(component.join("payload"), "submodule payload\n").expect("component payload");
    git(&component, ["add", "."]);
    git(&component, ["commit", "-m", "component"]);
    let add = Command::new("git")
        .arg("-C")
        .arg(&upstream.seed)
        .args(["-c", "protocol.file.allow=always", "submodule", "add"])
        .arg(&component)
        .arg(submodule_path)
        .output()
        .expect("submodule add");
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    git(&upstream.seed, ["commit", "-am", "add component"]);
    git(&upstream.seed, ["push", "origin", "master"]);
    component
}

fn source_init(upstream: &Upstream, destination: &Path) -> Output {
    run(Command::new(aros())
        .args(["source", "init"])
        .arg(destination)
        .args([
            "--upstream",
            upstream.remote.to_str().expect("UTF-8 test path"),
        ]))
}

fn install_fake_tools(root: &Path) -> PathBuf {
    let tools = root.join("tools");
    fs::create_dir(&tools).expect("tool directory");
    let transpiler = tools.join("aros-transpiler");
    fs::write(
        &transpiler,
        r#"#!/bin/sh
set -eu
if [ "$#" -gt 0 ] && [ "$1" = "--version" ]; then
    echo "aros-transpiler @AROS_TOOLS_VERSION@"
    exit 0
fi
failure=$(printenv AROS_FAKE_TRANSPILE_FAIL 2>/dev/null || true)
if [ "$failure" = 1 ]; then
    echo "intentional target-graph failure" >&2
    exit 42
fi
output=
source_dir=
while [ "$#" -gt 0 ]; do
    if [ "$1" = "--output" ]; then
        shift
        output=$1
    elif [ "$1" = "--source-dir" ]; then
        shift
        source_dir=$1
    fi
    shift
done
test -n "$output"
test -n "$source_dir"
hook=$(printenv AROS_FAKE_TRANSPILE_HOOK 2>/dev/null || true)
if [ -n "$hook" ]; then
    "$hook"
fi
mkdir -p "$(dirname "$output")"
printf 'validated graph\n' > "$output"
log=$(printenv AROS_FAKE_TRANSPILE_LOG 2>/dev/null || true)
if [ -n "$log" ]; then
    common=$(git -C "$source_dir" rev-parse --git-common-dir)
    printf 'validated\nsource=%s\ncommon=%s\n' "$source_dir" "$common" >> "$log"
fi
"#
        .replace("@AROS_TOOLS_VERSION@", env!("CARGO_PKG_VERSION")),
    )
    .expect("fake transpiler");
    fs::set_permissions(&transpiler, fs::Permissions::from_mode(0o755))
        .expect("transpiler permissions");
    for name in [
        "aros-genmodule",
        "aros-romtool",
        "aros-collect",
        "aros-ahi-runner",
        "aros-fetch",
    ] {
        let tool = tools.join(name);
        fs::write(
            &tool,
            format!(
                "#!/bin/sh\nset -eu\nif [ \"${{1:-}}\" = \"--version\" ]; then\n  echo \"{name} {}\"\n  exit 0\nfi\nexit 0\n",
                env!("CARGO_PKG_VERSION")
            ),
        )
        .expect("fake build tool");
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).expect("tool permissions");
    }
    tools
}

fn install_materialization_failure_git(root: &Path) -> (OsString, PathBuf, PathBuf, PathBuf) {
    let wrapper_dir = root.join("git-wrapper");
    fs::create_dir(&wrapper_dir).expect("Git wrapper directory");
    let wrapper = wrapper_dir.join("git");
    fs::write(
        &wrapper,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$AROS_GIT_INVOCATIONS"
directory=
previous=
saw_read_tree=0
for argument in "$@"; do
    if [ "$previous" = "-C" ]; then
        directory=$argument
    fi
    if [ "$argument" = "read-tree" ]; then
        saw_read_tree=1
    fi
    previous=$argument
done
if [ "$directory" = "$AROS_FAIL_CHECKOUT" ] && [ "$saw_read_tree" = 1 ] && [ ! -e "$AROS_FAIL_MARKER" ]; then
    : > "$AROS_FAIL_MARKER"
    exit 73
fi
exec "$AROS_REAL_GIT" "$@"
"#,
    )
    .expect("Git failure wrapper");
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))
        .expect("Git wrapper permissions");
    let marker = root.join("post-fast-forward-failed");
    let invocations = root.join("git-invocations.log");
    let real_git = which::which("git").expect("system Git");
    let mut paths = vec![wrapper_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths).expect("test PATH");
    (path, marker, real_git, invocations)
}

fn install_ref_cleanup_failure_git(root: &Path) -> (OsString, PathBuf) {
    let wrapper_dir = root.join("cleanup-git-wrapper");
    fs::create_dir(&wrapper_dir).unwrap();
    let wrapper = wrapper_dir.join("git");
    write_executable(
        &wrapper,
        r#"#!/bin/sh
previous=
saw_delete=0
for argument in "$@"; do
    if [ "$previous" = "update-ref" ] && [ "$argument" = "-d" ]; then
        saw_delete=1
    elif [ "$saw_delete" = 1 ] && [ "${argument#refs/aros-tools/source-fetch/}" != "$argument" ]; then
        exit 88
    fi
    previous=$argument
done
exec "$AROS_REAL_GIT" "$@"
"#,
    );
    let real_git = which::which("git").unwrap();
    let mut paths = vec![wrapper_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    (std::env::join_paths(paths).unwrap(), real_git)
}

fn install_lock_replacement_hook(root: &Path, lock: &Path) -> PathBuf {
    let hook = root.join("replace-source-lock.sh");
    write_executable(
        &hook,
        &format!(
            "#!/bin/sh\nset -eu\nmv '{}' '{}.original'\nprintf 'replacement\\n' > '{}'\n",
            lock.display(),
            lock.display(),
            lock.display()
        ),
    );
    hook
}

fn install_concurrent_materialization_git(root: &Path) -> (OsString, PathBuf, PathBuf) {
    let wrapper_dir = root.join("concurrent-git-wrapper");
    fs::create_dir(&wrapper_dir).unwrap();
    let wrapper = wrapper_dir.join("git");
    write_executable(
        &wrapper,
        r#"#!/bin/sh
directory=
previous=
saw_read_tree=0
for argument in "$@"; do
    if [ "$previous" = "-C" ]; then
        directory=$argument
    fi
    if [ "$argument" = "read-tree" ]; then
        saw_read_tree=1
    fi
    previous=$argument
done
if [ "$directory" = "$AROS_CONCURRENT_CHECKOUT" ] && [ "$saw_read_tree" = 1 ] && [ ! -e "$AROS_CONCURRENT_MARKER" ]; then
    : > "$AROS_CONCURRENT_MARKER"
    printf 'concurrent user data\n' > "$directory/advance.txt"
fi
exec "$AROS_REAL_GIT" "$@"
"#,
    );
    let real_git = which::which("git").unwrap();
    let marker = root.join("concurrent-materialization.marker");
    let mut paths = vec![wrapper_dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    (std::env::join_paths(paths).unwrap(), real_git, marker)
}

fn sync(checkout: &Path, upstream: &Upstream, tools: &Path) -> Output {
    run(Command::new(aros())
        .current_dir(checkout)
        .env("AROS_BUILD_TOOLS_DIR", tools)
        .args(["source", "sync", "--upstream"])
        .arg(&upstream.remote))
}

fn diagnostic(output: &Output) -> Value {
    assert!(!output.status.success());
    serde_json::from_slice(&output.stderr).expect("JSON diagnostic")
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("executable fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("executable fixture permissions");
}

#[test]
fn source_init_creates_an_atomic_checkout_with_reviewable_remotes() {
    let upstream = Upstream::new();
    let destination = upstream.root.path().join("checkout");
    let output = source_init(&upstream, &destination);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(destination.join("configure").is_file());
    assert_eq!(
        git(&destination, ["remote", "get-url", "origin"]),
        upstream
            .remote
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(
        git(&destination, ["remote", "get-url", "upstream"]),
        upstream
            .remote
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(
        git(&destination, ["rev-parse", "HEAD"]),
        git(&upstream.seed, ["rev-parse", "HEAD"])
    );
    assert_eq!(
        git(&destination, ["symbolic-ref", "--short", "HEAD"]),
        "master"
    );
}

#[test]
fn source_init_preserves_a_tracked_non_ascii_filename() {
    let upstream = Upstream::new();
    upstream.commit_and_push("arch/Ara±a.anim", "tracked source\n");
    let destination = upstream.root.path().join("unicode-checkout");

    let output = source_init(&upstream, &destination);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(destination.join("arch/Ara±a.anim")).unwrap(),
        "tracked source\n"
    );
    assert_eq!(git(&destination, ["status", "--porcelain=v1"]), "");
}

#[test]
fn source_init_rejects_a_tracked_unsafe_separator_name_before_publication() {
    let upstream = Upstream::new();
    upstream.commit_and_push("arch/unsafe\\name", "tracked source\n");
    let destination = upstream.root.path().join("unsafe-checkout");

    let output = source_init(&upstream, &destination);

    assert!(!output.status.success());
    assert!(!destination.exists());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unsafe path component"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn source_init_canonicalizes_a_reviewed_relative_local_source_before_isolation() {
    let upstream = Upstream::new();
    let destination = upstream.root.path().join("relative-checkout");
    let output = run(Command::new(aros())
        .current_dir(upstream.root.path())
        .args(["source", "init"])
        .arg(&destination)
        .args(["--upstream", "./upstream.git"]));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        git(&destination, ["remote", "get-url", "upstream"]),
        upstream
            .remote
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    );
}

#[test]
fn source_init_uses_a_fork_as_origin_and_selected_ref_detached() {
    let upstream = Upstream::new();
    let selected = upstream.commit_and_push("selected.txt", "selected\n");
    let fork = upstream.root.path().join("fork.git");
    let clone = Command::new("git")
        .args(["clone", "--bare"])
        .arg(&upstream.remote)
        .arg(&fork)
        .output()
        .expect("fork clone");
    assert!(clone.status.success());
    upstream.commit_and_push("later.txt", "later\n");
    let destination = upstream.root.path().join("fork-checkout");

    let output = run(Command::new(aros())
        .args(["source", "init"])
        .arg(&destination)
        .args(["--upstream"])
        .arg(&upstream.remote)
        .args(["--fork"])
        .arg(&fork)
        .args(["--ref", &selected]));

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        git(&destination, ["remote", "get-url", "origin"]),
        fork.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(
        git(&destination, ["remote", "get-url", "upstream"]),
        upstream
            .remote
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(git(&destination, ["rev-parse", "HEAD"]), selected);
    let detached = Command::new("git")
        .arg("-C")
        .arg(&destination)
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .status()
        .expect("detached probe");
    assert!(!detached.success());
}

#[test]
fn source_init_never_reuses_an_existing_destination() {
    let upstream = Upstream::new();
    let destination = upstream.root.path().join("occupied");
    fs::create_dir(&destination).expect("occupied destination");
    fs::write(destination.join("keep"), "do not touch").expect("sentinel");

    let output = run(Command::new(aros())
        .args(["--diagnostic-format=json", "source", "init"])
        .arg(&destination)
        .args(["--upstream"])
        .arg(&upstream.remote));
    let value = diagnostic(&output);

    assert_eq!(value["diagnostics"][0]["code"], "AR0116");
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("already exists"));
    assert_eq!(
        fs::read_to_string(destination.join("keep")).unwrap(),
        "do not touch"
    );
}

#[test]
fn source_init_failure_does_not_publish_a_partial_destination() {
    let upstream = Upstream::new();
    let destination = upstream.root.path().join("failed-checkout");
    let output = run(Command::new(aros())
        .args(["--diagnostic-format=json", "source", "init"])
        .arg(&destination)
        .args(["--upstream"])
        .arg(&upstream.remote)
        .args(["--ref", "refs/heads/missing-ref"]));
    let value = diagnostic(&output);

    assert_eq!(value["diagnostics"][0]["code"], "AR0112");
    assert!(!destination.exists());
}

#[test]
fn source_init_initializes_recursive_submodules_before_publication() {
    let upstream = Upstream::new();
    add_component(&upstream);
    let destination = upstream.root.path().join("submodule-checkout");

    let output = run(Command::new(aros())
        .env("GIT_ALLOW_PROTOCOL", "file")
        .args(["source", "init"])
        .arg(&destination)
        .args(["--upstream"])
        .arg(&upstream.remote));

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(destination.join("external/component/payload")).unwrap(),
        "submodule payload\n"
    );
}

#[test]
fn source_init_classifies_post_rename_durability_failure_and_retains_complete_tree() {
    let upstream = Upstream::new();
    add_component(&upstream);
    let destination = upstream.root.path().join("durability-checkout");

    let output = run(Command::new(aros())
        .env("GIT_ALLOW_PROTOCOL", "file")
        .env(
            "AROS_PUBLICATION_TEST_FAIL_AT",
            "prepared-tree-after-rename-before-sync",
        )
        .args(["--diagnostic-format=json", "source", "init"])
        .arg(&destination)
        .args(["--upstream"])
        .arg(&upstream.remote));
    let value = diagnostic(&output);

    assert_eq!(value["diagnostics"][0]["code"], "AR0116");
    assert_eq!(
        value["diagnostics"][0]["context"]["commit_state"],
        "indeterminate"
    );
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("CommitStateUncertain"));
    assert_eq!(
        fs::read_to_string(destination.join("external/component/payload")).unwrap(),
        "submodule payload\n"
    );
    assert_eq!(git(&destination, ["status", "--porcelain=v1"]), "");
}

#[test]
fn sync_fast_forwards_only_after_real_target_graph_validation() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    let expected = upstream.commit_and_push("advance.txt", "advance\n");
    let tools = install_fake_tools(upstream.root.path());
    let log = upstream.root.path().join("transpiler.log");

    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .env("AROS_FAKE_TRANSPILE_LOG", &log)
        .args(["source", "sync", "--upstream"])
        .arg(&upstream.remote));

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(git(&checkout, ["rev-parse", "HEAD"]), expected);
    let validation_log = fs::read_to_string(log).unwrap();
    assert!(validation_log.starts_with("validated\nsource="));
    let source = validation_log
        .lines()
        .find_map(|line| line.strip_prefix("source="))
        .expect("candidate source path");
    let common = validation_log
        .lines()
        .find_map(|line| line.strip_prefix("common="))
        .expect("candidate common directory");
    assert_ne!(Path::new(source), checkout);
    assert!(Path::new(source).join(common).starts_with(source));
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 declared target profile"));
}

#[test]
fn sync_qualifies_a_pristine_upstream_candidate_with_built_in_profiles() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    let expected = upstream.remove_and_push("aros-targets.toml");
    let tools = install_fake_tools(upstream.root.path());
    let log = upstream.root.path().join("transpiler.log");

    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .env("AROS_FAKE_TRANSPILE_LOG", &log)
        .args(["source", "sync", "--upstream"])
        .arg(&upstream.remote));

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(git(&checkout, ["rev-parse", "HEAD"]), expected);
    assert!(!checkout.join("aros-targets.toml").exists());
    assert_eq!(
        fs::read_to_string(log)
            .unwrap()
            .lines()
            .filter(|line| *line == "validated")
            .count(),
        4
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("4 declared target profile(s)"));
}

#[test]
fn sync_validation_failure_leaves_the_branch_unchanged() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    let before = git(&checkout, ["rev-parse", "HEAD"]);
    upstream.commit_and_push("advance.txt", "advance\n");
    let tools = install_fake_tools(upstream.root.path());

    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .env("AROS_FAKE_TRANSPILE_FAIL", "1")
        .args(["--diagnostic-format=json", "source", "sync", "--upstream"])
        .arg(&upstream.remote));
    let value = diagnostic(&output);

    assert_eq!(value["diagnostics"][0]["code"], "AR0115");
    assert_eq!(
        value["diagnostics"][0]["context"]["tool"],
        tools.join("aros-transpiler").display().to_string()
    );
    assert_eq!(value["diagnostics"][0]["context"]["exit_code"], 42);
    assert_eq!(git(&checkout, ["rev-parse", "HEAD"]), before);
    assert_eq!(
        git(&checkout, ["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1
    );
}

#[test]
fn sync_materialization_failure_restores_ref_without_destructive_reset() {
    let upstream = Upstream::new();
    let component = add_component(&upstream);
    let checkout = upstream.root.path().join("checkout");
    let initialized = run(Command::new(aros())
        .env("GIT_ALLOW_PROTOCOL", "file")
        .args(["source", "init"])
        .arg(&checkout)
        .args(["--upstream"])
        .arg(&upstream.remote));
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let before_head = git(&checkout, ["rev-parse", "HEAD"]);
    let before_index = git(&checkout, ["write-tree"]);
    let before_submodule = git(&checkout.join("external/component"), ["rev-parse", "HEAD"]);

    fs::write(component.join("payload"), "advanced submodule payload\n").unwrap();
    git(&component, ["add", "payload"]);
    git(&component, ["commit", "-m", "advance component"]);
    let seeded_component = upstream.seed.join("external/component");
    git(&seeded_component, ["fetch", "origin", "master"]);
    git(&seeded_component, ["checkout", "--detach", "FETCH_HEAD"]);
    git(&upstream.seed, ["add", "external/component"]);
    git(&upstream.seed, ["commit", "-m", "advance component pin"]);
    git(&upstream.seed, ["push", "origin", "master"]);

    let tools = install_fake_tools(upstream.root.path());
    let (path, marker, real_git, invocations) =
        install_materialization_failure_git(upstream.root.path());
    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("PATH", path)
        .env("GIT_ALLOW_PROTOCOL", "file")
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .env(
            "AROS_FAIL_CHECKOUT",
            checkout.canonicalize().expect("canonical checkout"),
        )
        .env("AROS_FAIL_MARKER", &marker)
        .env("AROS_REAL_GIT", &real_git)
        .env("AROS_GIT_INVOCATIONS", &invocations)
        .args(["--diagnostic-format=json", "source", "sync", "--upstream"])
        .arg(&upstream.remote));
    let value = diagnostic(&output);

    assert_eq!(value["diagnostics"][0]["code"], "AR0116");
    assert_eq!(value["diagnostics"][0]["context"]["tool"], "git");
    assert_eq!(value["diagnostics"][0]["context"]["exit_code"], 73);
    assert_eq!(
        value["diagnostics"][0]["context"]["commit_state"],
        "rolled_back"
    );
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("branch, index, and submodule snapshot was restored"));
    assert!(marker.is_file(), "materialization failure hook ran");
    assert!(!fs::read_to_string(&invocations)
        .unwrap()
        .contains("reset --hard"));
    assert_eq!(git(&checkout, ["rev-parse", "HEAD"]), before_head);
    assert_eq!(git(&checkout, ["write-tree"]), before_index);
    assert_eq!(
        git(&checkout, ["symbolic-ref", "--short", "HEAD"]),
        "master"
    );
    assert_eq!(git(&checkout, ["status", "--porcelain=v1"]), "");
    assert_eq!(
        git(&checkout.join("external/component"), ["rev-parse", "HEAD"]),
        before_submodule
    );
    assert_eq!(
        fs::read_to_string(checkout.join("external/component/payload")).unwrap(),
        "submodule payload\n"
    );
}

#[test]
fn sync_rejects_dirty_and_diverged_branches_without_modifying_them() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    let tools = install_fake_tools(upstream.root.path());

    fs::write(checkout.join("dirty.txt"), "dirty\n").expect("dirty file");
    let dirty = sync(&checkout, &upstream, &tools);
    assert!(!dirty.status.success());
    assert!(String::from_utf8_lossy(&dirty.stderr).contains("not clean"));
    fs::remove_file(checkout.join("dirty.txt")).expect("remove dirty file");

    git(&checkout, ["config", "user.name", "AROS test"]);
    git(
        &checkout,
        ["config", "user.email", "aros-test@example.invalid"],
    );
    fs::write(checkout.join("local.txt"), "local\n").expect("local change");
    git(&checkout, ["add", "local.txt"]);
    git(&checkout, ["commit", "-m", "local change"]);
    let before = git(&checkout, ["rev-parse", "HEAD"]);
    upstream.commit_and_push("remote.txt", "remote\n");

    let diverged = sync(&checkout, &upstream, &tools);
    assert!(!diverged.status.success());
    assert!(String::from_utf8_lossy(&diverged.stderr).contains("diverged"));
    assert_eq!(git(&checkout, ["rev-parse", "HEAD"]), before);
}

#[test]
fn sync_validates_and_retains_a_non_divergent_local_ahead_branch() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    git(&checkout, ["config", "user.name", "AROS test"]);
    git(
        &checkout,
        ["config", "user.email", "aros-test@example.invalid"],
    );
    fs::write(checkout.join("local.txt"), "local\n").unwrap();
    git(&checkout, ["add", "local.txt"]);
    git(&checkout, ["commit", "-m", "local ahead"]);
    let expected = git(&checkout, ["rev-parse", "HEAD"]);
    let tools = install_fake_tools(upstream.root.path());

    let output = sync(&checkout, &upstream, &tools);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(git(&checkout, ["rev-parse", "HEAD"]), expected);
    assert!(String::from_utf8_lossy(&output.stdout).contains("already contains"));
}

#[test]
fn sync_rejects_an_unreviewed_remote_identity_and_detached_head() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    let tools = install_fake_tools(upstream.root.path());
    let different = upstream.root.path().join("different.git");
    let init = Command::new("git")
        .args(["init", "--bare"])
        .arg(&different)
        .output()
        .unwrap();
    assert!(init.status.success());

    let wrong_remote = run(Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .args(["source", "sync", "--upstream"])
        .arg(&different));
    assert!(!wrong_remote.status.success());
    assert!(String::from_utf8_lossy(&wrong_remote.stderr).contains("does not match"));

    git(&checkout, ["checkout", "--detach"]);
    let detached = sync(&checkout, &upstream, &tools);
    assert!(!detached.status.success());
    assert!(String::from_utf8_lossy(&detached.stderr).contains("attached local branch"));
}

#[test]
fn source_commands_have_one_canonical_surface_without_aliases() {
    let old_sync = run(Command::new(aros()).arg("sync"));
    assert!(!old_sync.status.success());
    assert!(String::from_utf8_lossy(&old_sync.stderr).contains("unrecognized subcommand"));

    let old_upstream = run(Command::new(aros()).args([
        "source",
        "sync",
        "--upstream-url",
        "https://example.invalid/AROS.git",
    ]));
    assert!(!old_upstream.status.success());
    assert!(String::from_utf8_lossy(&old_upstream.stderr).contains("unexpected argument"));

    let old_origin = run(Command::new(aros()).args([
        "source",
        "init",
        "unused",
        "--origin",
        "https://example.invalid/fork.git",
    ]));
    assert!(!old_origin.status.success());
    assert!(String::from_utf8_lossy(&old_origin.stderr).contains("unexpected argument"));
}

#[test]
fn source_init_requires_honest_ref_and_transport_semantics_without_leaking_credentials() {
    let root = tempfile::tempdir().unwrap();
    let ambiguous = run(Command::new(aros())
        .args(["--diagnostic-format=json", "source", "init"])
        .arg(root.path().join("ambiguous"))
        .args(["--ref", "master"]));
    let ambiguous = diagnostic(&ambiguous);
    assert_eq!(ambiguous["diagnostics"][0]["code"], "AR0111");
    assert!(ambiguous["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("refs/heads/NAME"));

    let credential = run(Command::new(aros())
        .args(["--diagnostic-format=json", "source", "init"])
        .arg(root.path().join("credential"))
        .args(["--upstream", "https://top-secret@example.invalid/AROS.git"]));
    let rendered = String::from_utf8_lossy(&credential.stderr);
    let credential = diagnostic(&credential);
    assert_eq!(credential["diagnostics"][0]["code"], "AR0111");
    assert!(!rendered.contains("top-secret"));

    let helper = run(Command::new(aros())
        .args(["--diagnostic-format=json", "source", "init"])
        .arg(root.path().join("helper"))
        .args(["--upstream", "ext::sh -c exploit"]));
    let helper = diagnostic(&helper);
    assert_eq!(helper["diagnostics"][0]["code"], "AR0111");
    assert!(helper["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("unsupported Git transport"));
}

#[test]
fn source_transport_ignores_global_url_rewrites_and_rejects_local_network_overrides() {
    let upstream = Upstream::new();
    let global = upstream.root.path().join("host-gitconfig");
    let missing = upstream.root.path().join("redirected-missing.git");
    let configured = Command::new("git")
        .args(["config", "--file"])
        .arg(&global)
        .arg(format!("url.{}.insteadOf", missing.display()))
        .arg(upstream.remote.display().to_string())
        .output()
        .unwrap();
    assert!(configured.status.success());
    let local_rewrite_key = format!("url.{}.insteadOf", missing.display());
    let local_rewrite_value = upstream.remote.display().to_string();
    let configured = Command::new("git")
        .arg("-C")
        .arg(&upstream.seed)
        .args(["config", &local_rewrite_key, &local_rewrite_value])
        .output()
        .unwrap();
    assert!(configured.status.success());
    let destination = upstream.root.path().join("rewrite-safe-checkout");
    let initialized = run(Command::new(aros())
        .current_dir(&upstream.seed)
        .env("GIT_CONFIG_GLOBAL", &global)
        .args(["source", "init"])
        .arg(&destination)
        .args(["--upstream"])
        .arg(&upstream.remote));
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );

    git(
        &destination,
        [
            "config",
            "url.https://evil.invalid/.insteadOf",
            "https://safe.invalid/",
        ],
    );
    let tools = install_fake_tools(upstream.root.path());
    let rejected = run(Command::new(aros())
        .current_dir(&destination)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .args(["--diagnostic-format=json", "source", "sync", "--upstream"])
        .arg(&upstream.remote));
    let rejected = diagnostic(&rejected);
    assert_eq!(rejected["diagnostics"][0]["code"], "AR0114");
    assert!(rejected["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("network-affecting configuration"));
}

#[test]
fn sync_lock_is_repo_wide_and_reports_a_stable_code() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    let lock = checkout.join(".git/aros-tools-source.lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock)
        .unwrap();
    rustix::fs::flock(
        &lock_file,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    )
    .unwrap();
    let tools = install_fake_tools(upstream.root.path());
    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .args(["--diagnostic-format=json", "source", "sync", "--upstream"])
        .arg(&upstream.remote));
    let output = diagnostic(&output);
    assert_eq!(output["diagnostics"][0]["code"], "AR0113");
    assert!(lock.is_file());

    drop(lock_file);
    let retry = sync(&checkout, &upstream, &tools);
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert!(
        lock.is_file(),
        "the kernel lock uses a persistent owner record"
    );
}

#[test]
fn sync_publishes_the_fetched_oid_even_if_upstream_moves_during_validation() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    let fetched = upstream.commit_and_push("first.txt", "first\n");
    let tools = install_fake_tools(upstream.root.path());
    let hook = upstream.root.path().join("move-upstream.sh");
    write_executable(
        &hook,
        &format!(
            "#!/bin/sh\nset -eu\nprintf 'later\\n' > '{seed}/later.txt'\ngit -C '{seed}' add later.txt\ngit -C '{seed}' commit -m later >/dev/null\ngit -C '{seed}' push origin master >/dev/null\n",
            seed = upstream.seed.display()
        ),
    );
    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .env("AROS_FAKE_TRANSPILE_HOOK", &hook)
        .args(["source", "sync", "--upstream"])
        .arg(&upstream.remote));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(git(&checkout, ["rev-parse", "HEAD"]), fetched);
    assert_ne!(git(&upstream.seed, ["rev-parse", "HEAD"]), fetched);
    assert_eq!(
        git(
            &checkout,
            ["for-each-ref", "--format=%(refname)", "refs/aros-tools"]
        ),
        ""
    );
}

#[test]
fn sync_detects_concurrent_user_commit_and_never_rolls_it_back() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    upstream.commit_and_push("upstream.txt", "upstream\n");
    git(&checkout, ["config", "user.name", "Concurrent user"]);
    git(&checkout, ["config", "user.email", "user@example.invalid"]);
    let tools = install_fake_tools(upstream.root.path());
    let hook = upstream.root.path().join("concurrent-user.sh");
    write_executable(
        &hook,
        &format!(
            "#!/bin/sh\nset -eu\ngit -C '{}' commit --allow-empty -m concurrent-user >/dev/null\n",
            checkout.display()
        ),
    );
    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .env("AROS_FAKE_TRANSPILE_HOOK", &hook)
        .args(["--diagnostic-format=json", "source", "sync", "--upstream"])
        .arg(&upstream.remote));
    let output = diagnostic(&output);
    assert_eq!(output["diagnostics"][0]["code"], "AR0116");
    assert_eq!(
        git(&checkout, ["log", "-1", "--format=%s"]),
        "concurrent-user"
    );
    assert_eq!(git(&checkout, ["status", "--porcelain=v1"]), "");
}

#[test]
fn sync_reports_primary_and_cleanup_failures_together() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    upstream.commit_and_push("upstream.txt", "upstream\n");
    let tools = install_fake_tools(upstream.root.path());
    let (path, real_git) = install_ref_cleanup_failure_git(upstream.root.path());
    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("PATH", path)
        .env("AROS_REAL_GIT", real_git)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .env("AROS_FAKE_TRANSPILE_FAIL", "1")
        .args(["--diagnostic-format=json", "source", "sync", "--upstream"])
        .arg(&upstream.remote));
    let output = diagnostic(&output);
    assert_eq!(output["diagnostics"][0]["code"], "AR0115");
    assert_eq!(
        output["diagnostics"][0]["context"]["tool"],
        tools.join("aros-transpiler").display().to_string()
    );
    assert_eq!(output["diagnostics"][0]["context"]["exit_code"], 42);
    let message = output["diagnostics"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("intentional target-graph failure"),
        "{message}"
    );
    assert!(message.contains("run-owned ref cleanup also failed"));
    assert!(message.contains("exit status: 88"));
}

#[test]
fn sync_preserves_an_untracked_file_created_at_the_cas_boundary() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    let before = git(&checkout, ["rev-parse", "HEAD"]);
    upstream.commit_and_push("advance.txt", "upstream data\n");
    let tools = install_fake_tools(upstream.root.path());
    let (path, real_git, marker) = install_concurrent_materialization_git(upstream.root.path());
    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("PATH", path)
        .env("AROS_REAL_GIT", real_git)
        .env("AROS_CONCURRENT_CHECKOUT", checkout.canonicalize().unwrap())
        .env("AROS_CONCURRENT_MARKER", &marker)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .args(["--diagnostic-format=json", "source", "sync", "--upstream"])
        .arg(&upstream.remote));
    let output = diagnostic(&output);
    assert_eq!(output["diagnostics"][0]["code"], "AR0116");
    assert!(marker.is_file());
    assert_eq!(git(&checkout, ["rev-parse", "HEAD"]), before);
    assert_eq!(
        fs::read_to_string(checkout.join("advance.txt")).unwrap(),
        "concurrent user data\n"
    );
}

#[test]
fn sync_rejects_repository_replace_refs_before_candidate_validation() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    let before = git(&checkout, ["rev-parse", "HEAD"]);
    let replace_ref = format!("refs/replace/{before}");
    git(&checkout, ["update-ref", &replace_ref, &before]);
    upstream.commit_and_push("advance.txt", "advance\n");
    let tools = install_fake_tools(upstream.root.path());

    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .args(["--diagnostic-format=json", "source", "sync", "--upstream"])
        .arg(&upstream.remote));
    let output = diagnostic(&output);

    assert_eq!(output["diagnostics"][0]["code"], "AR0114");
    assert!(output["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("replacement refs"));
    assert_eq!(git(&checkout, ["rev-parse", "HEAD"]), before);
}

#[test]
fn sync_never_overwrites_the_callers_fetch_head() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    upstream.commit_and_push("advance.txt", "advance\n");
    let fetch_head = checkout.join(".git/FETCH_HEAD");
    let sentinel = b"caller-owned FETCH_HEAD\n";
    fs::write(&fetch_head, sentinel).unwrap();
    let tools = install_fake_tools(upstream.root.path());

    let output = sync(&checkout, &upstream, &tools);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(fetch_head).unwrap(), sentinel);
}

#[test]
fn sync_compares_local_remote_identity_after_canonical_resolution() {
    use std::os::unix::fs::symlink;

    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    upstream.commit_and_push("advance.txt", "advance\n");
    let alias = upstream.root.path().join("reviewed-upstream-alias.git");
    symlink(&upstream.remote, &alias).unwrap();
    git(
        &checkout,
        ["remote", "set-url", "upstream", alias.to_str().unwrap()],
    );
    let tools = install_fake_tools(upstream.root.path());

    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .args(["source", "sync", "--upstream"])
        .arg(&alias));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn sync_freezes_submodule_sources_independently_of_local_url_overrides() {
    let upstream = Upstream::new();
    let component = add_component(&upstream);
    let checkout = upstream.root.path().join("checkout");
    let initialized = run(Command::new(aros())
        .args(["source", "init"])
        .arg(&checkout)
        .args(["--upstream"])
        .arg(&upstream.remote));
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let configured_override = upstream.root.path().join("missing-component.git");
    git(
        &checkout,
        [
            "config",
            "submodule.external/component.url",
            configured_override.to_str().unwrap(),
        ],
    );
    fs::write(component.join("payload"), "advanced submodule payload\n").unwrap();
    git(&component, ["add", "payload"]);
    git(&component, ["commit", "-m", "advance component"]);
    let seeded_component = upstream.seed.join("external/component");
    git(&seeded_component, ["fetch", "origin", "master"]);
    git(&seeded_component, ["checkout", "--detach", "FETCH_HEAD"]);
    git(&upstream.seed, ["add", "external/component"]);
    git(&upstream.seed, ["commit", "-m", "advance component pin"]);
    git(&upstream.seed, ["push", "origin", "master"]);
    let tools = install_fake_tools(upstream.root.path());

    let output = sync(&checkout, &upstream, &tools);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(checkout.join("external/component/payload")).unwrap(),
        "advanced submodule payload\n"
    );
    assert_eq!(
        git(
            &checkout,
            ["config", "--get", "submodule.external/component.url"]
        ),
        configured_override.display().to_string()
    );
}

#[test]
fn sync_rejects_ignored_files_inside_recursive_submodules() {
    let upstream = Upstream::new();
    add_component(&upstream);
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    let submodule = checkout.join("external/component");
    let git_dir = git(&submodule, ["rev-parse", "--git-dir"]);
    let git_dir = if Path::new(&git_dir).is_absolute() {
        PathBuf::from(git_dir)
    } else {
        submodule.join(git_dir)
    };
    fs::write(git_dir.join("info/exclude"), "ignored.local\n").unwrap();
    fs::write(submodule.join("ignored.local"), "must not be hidden\n").unwrap();
    let tools = install_fake_tools(upstream.root.path());

    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .args(["--diagnostic-format=json", "source", "sync", "--upstream"])
        .arg(&upstream.remote));
    let output = diagnostic(&output);
    assert_eq!(output["diagnostics"][0]["code"], "AR0114");
    assert!(output["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("submodule worktree is not clean"));
}

#[test]
fn sync_materializes_a_new_submodule_from_the_validated_candidate() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    add_component(&upstream);
    let tools = install_fake_tools(upstream.root.path());

    let output = sync(&checkout, &upstream, &tools);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(checkout.join("external/component/payload")).unwrap(),
        "submodule payload\n"
    );
    assert!(!git(
        &checkout.join("external/component"),
        ["remote", "get-url", "origin"]
    )
    .contains("aros-sync-candidate"));
}

#[test]
fn source_init_supports_submodule_paths_and_names_with_spaces() {
    let upstream = Upstream::new();
    add_component_at(&upstream, "external/component with space");
    let destination = upstream.root.path().join("space-submodule-checkout");

    let output = source_init(&upstream, &destination);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(destination.join("external/component with space/payload")).unwrap(),
        "submodule payload\n"
    );
}

#[test]
fn sync_rejects_a_replaced_kernel_lock_before_branch_cas() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    let before = git(&checkout, ["rev-parse", "HEAD"]);
    upstream.commit_and_push("advance.txt", "advance\n");
    let tools = install_fake_tools(upstream.root.path());
    let hook = install_lock_replacement_hook(
        upstream.root.path(),
        &checkout.join(".git/aros-tools-source.lock"),
    );

    let output = run(Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .env("AROS_FAKE_TRANSPILE_HOOK", &hook)
        .args(["--diagnostic-format=json", "source", "sync", "--upstream"])
        .arg(&upstream.remote));
    let output = diagnostic(&output);

    assert_eq!(output["diagnostics"][0]["code"], "AR0116");
    assert!(output["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("replaced or modified"));
    assert_eq!(git(&checkout, ["rev-parse", "HEAD"]), before);
}

#[test]
fn committed_sync_tolerates_a_closed_stdout_consumer() {
    let upstream = Upstream::new();
    let checkout = upstream.root.path().join("checkout");
    assert!(source_init(&upstream, &checkout).status.success());
    let expected = upstream.commit_and_push("advance.txt", "advance\n");
    let tools = install_fake_tools(upstream.root.path());
    let mut child = Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .args(["source", "sync", "--upstream"])
        .arg(&upstream.remote)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first_line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first_line)
        .unwrap();
    assert!(first_line.contains("Fetching"));
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(git(&checkout, ["rev-parse", "HEAD"]), expected);
}

#[test]
fn real_aros_source_sync_runs_the_real_transpiler_when_explicitly_configured() {
    let Some(source_root) = std::env::var_os("AROS_TEST_SOURCE_ROOT") else {
        return;
    };
    let source_root = PathBuf::from(source_root).canonicalize().unwrap();
    assert!(source_root.join("aros-targets.toml").is_file());
    let source_ref = git(&source_root, ["rev-parse", "HEAD"]);
    let tools = std::env::var_os("AROS_TEST_TOOLS_DIR").map_or_else(
        || Path::new(aros()).parent().unwrap().to_path_buf(),
        PathBuf::from,
    );
    for name in [
        "aros-transpiler",
        "aros-genmodule",
        "aros-romtool",
        "aros-collect",
        "aros-ahi-runner",
        "aros-fetch",
    ] {
        assert!(
            tools.join(name).is_file(),
            "AROS_TEST_SOURCE_ROOT requires real tool '{}' in '{}' (or set AROS_TEST_TOOLS_DIR)",
            name,
            tools.display()
        );
    }
    let temporary = tempfile::tempdir().unwrap();
    let qualified_upstream = temporary.path().join("qualified-upstream.git");
    let initialized_upstream = Command::new("git")
        .args(["init", "--bare", "--initial-branch=qualified"])
        .arg(&qualified_upstream)
        .output()
        .expect("qualified upstream initialization");
    assert!(
        initialized_upstream.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized_upstream.stderr)
    );
    // CI deliberately checks out the immutable source shallowly. Build a
    // complete root commit around the exact qualified tree in an isolated
    // repository rather than requiring or mutating the missing source
    // history. Git alternates are confined to this process-local fixture.
    let source_tree = git(
        &source_root,
        ["rev-parse", &format!("{source_ref}^{{tree}}")],
    );
    let source_objects = PathBuf::from(git(
        &source_root,
        [
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "objects",
        ],
    ))
    .canonicalize()
    .expect("qualified source object directory");
    let mut alternate = source_objects.as_os_str().as_encoded_bytes().to_vec();
    assert!(!alternate.contains(&b'\n') && !alternate.contains(&b'\r'));
    alternate.push(b'\n');
    fs::write(
        qualified_upstream.join("objects/info/alternates"),
        alternate,
    )
    .expect("qualified source alternate");
    let published_source = Command::new("git")
        .arg("-C")
        .arg(&qualified_upstream)
        .args(["commit-tree", &source_tree, "-m", "qualified source tree"])
        .env("GIT_AUTHOR_NAME", "AROS qualification")
        .env("GIT_AUTHOR_EMAIL", "aros-qualification@example.invalid")
        .env("GIT_COMMITTER_NAME", "AROS qualification")
        .env("GIT_COMMITTER_EMAIL", "aros-qualification@example.invalid")
        .output()
        .expect("qualified source publication");
    assert!(
        published_source.status.success(),
        "{}",
        String::from_utf8_lossy(&published_source.stderr)
    );
    let qualified_commit = String::from_utf8(published_source.stdout)
        .expect("qualified source commit encoding")
        .trim()
        .to_owned();
    assert_eq!(qualified_commit.len(), 40);
    git(
        &qualified_upstream,
        [
            "update-ref",
            "refs/heads/qualified",
            qualified_commit.as_str(),
        ],
    );
    assert_eq!(
        git(
            &qualified_upstream,
            ["rev-parse", "refs/heads/qualified^{tree}"],
        ),
        source_tree
    );
    let checkout = temporary.path().join("real-source-checkout");
    let initialized = run(Command::new(aros())
        .args(["source", "init"])
        .arg(&checkout)
        .args(["--upstream"])
        .arg(&qualified_upstream)
        .args(["--ref", "refs/heads/qualified"]));
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let attached = Command::new("git")
        .arg("-C")
        .arg(&checkout)
        .args(["switch", "--create", "qualified-sync"])
        .output()
        .expect("qualified source branch attachment");
    assert!(
        attached.status.success(),
        "{}",
        String::from_utf8_lossy(&attached.stderr)
    );
    let synchronized = run(Command::new(aros())
        .current_dir(&checkout)
        .env("AROS_BUILD_TOOLS_DIR", &tools)
        .args(["source", "sync", "--upstream"])
        .arg(&qualified_upstream)
        .args(["--ref", "qualified"]));
    assert!(
        synchronized.status.success(),
        "{}",
        String::from_utf8_lossy(&synchronized.stderr)
    );
}
