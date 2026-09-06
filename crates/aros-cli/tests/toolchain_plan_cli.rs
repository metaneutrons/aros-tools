//! Real isolated Git fixtures prove the planning frontend stays read-only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use aros_common::sha256_bytes;
use serde_json::{json, Value};

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    recipe: Value,
}

impl Fixture {
    fn recommit(&mut self, name: &str) {
        let checkout = self.root.join(name);
        git(&checkout, &["add", "."]);
        git(&checkout, &["commit", "-qm", "test: source audit fixture"]);
        self.recipe[format!("{name}_commit")] = json!(git(&checkout, &["rev-parse", "HEAD"]));
        self.recipe[format!("{name}_tree")] = json!(git(&checkout, &["rev-parse", "HEAD^{tree}"]));
        sign(&mut self.recipe);
        self.save();
    }

    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("plan ü space");
        fs::create_dir(&root).unwrap();
        for name in ["source", "producer", "tools"] {
            fs::create_dir(root.join(name)).unwrap();
            git(&root.join(name), &["init", "-q"]);
        }
        fs::create_dir(root.join("producer/toolchains")).unwrap();
        fs::create_dir_all(root.join("producer/scripts/toolchain")).unwrap();
        fs::write(root.join("source/patch.diff"), "fixture patch\n").unwrap();
        fs::write(
            root.join("source/.gitattributes"),
            "* filter=must-not-run\n",
        )
        .unwrap();
        fs::write(root.join("source/configure"), "#!/bin/sh\nexit 99\n").unwrap();
        fs::write(root.join("tools/Cargo.toml"), "# collector fixture\n").unwrap();
        let profiles = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v1", "upstream_commit": "4".repeat(40),
            "profiles": [{"name":"pc-x86_64", "configure_target":"pc-x86_64",
                "upstream_output_target":"pc-x86_64", "target_triple":"x86_64-unknown-aros",
                "cpu":"x86_64", "platform":"pc", "float_abi":"", "capabilities":["c","cxx"]}]
        }))
        .unwrap();
        // Deliberately not a usable source lock: this slice checks identity,
        // not M2 semantic completeness, and must never mark it build-ready.
        let lock = b"{\"schema\":\"aros-toolchain-source-lock-v2\"}\n";
        fs::write(root.join("producer/toolchains/profiles-v1.json"), &profiles).unwrap();
        fs::write(
            root.join("producer/toolchains/arbitrary-version.sources.json"),
            lock,
        )
        .unwrap();
        fs::write(
            root.join("producer/scripts/toolchain/build-release.sh"),
            "#!/bin/sh\nexit 98\n",
        )
        .unwrap();
        for name in ["source", "producer", "tools"] {
            git(&root.join(name), &["add", "."]);
            git(
                &root.join(name),
                &["commit", "-qm", "test: committed inputs"],
            );
        }
        let mut recipe = json!({
            "schema": "aros-toolchain-recipe-v2",
            "source_commit": git(&root.join("source"), &["rev-parse", "HEAD"]),
            "source_tree": git(&root.join("source"), &["rev-parse", "HEAD^{tree}"]),
            "producer_commit": git(&root.join("producer"), &["rev-parse", "HEAD"]),
            "producer_tree": git(&root.join("producer"), &["rev-parse", "HEAD^{tree}"]),
            "tools_commit": git(&root.join("tools"), &["rev-parse", "HEAD"]),
            "tools_tree": git(&root.join("tools"), &["rev-parse", "HEAD^{tree}"]),
            "source_date_epoch": 0,
            "source_lock_sha256": sha256_bytes(lock), "profiles_sha256": sha256_bytes(&profiles),
            "patches": [{"path":"patch.diff", "sha256":sha256_bytes(b"fixture patch\n")}]
        });
        sign(&mut recipe);
        let fixture = Self {
            _temporary: temporary,
            root,
            recipe,
        };
        fixture.save();
        fixture
    }

    fn save(&self) {
        fs::write(
            self.root.join("recipe.json"),
            serde_json::to_vec(&self.recipe).unwrap(),
        )
        .unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
        command
            .current_dir(&self.root)
            .env_remove("AROS_LOG_FILE")
            .env_remove("AROS_LOG_LEVEL")
            .env_remove("AROS_LOG_FORMAT")
            .env_remove("AROS_OFFLINE")
            .args([
                "--diagnostic-format=json",
                "toolchain",
                "plan",
                "--format=json",
                "--backend=legacy-preview",
                "--preset=pc-x86_64",
                "--recipe=recipe.json",
                "--source-dir=source",
                "--producer-dir=producer",
                "--tools-dir=tools",
            ]);
        command
    }

    fn plan(&self) -> Value {
        let output = self.command().output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Plan fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Plan fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn sign(recipe: &mut Value) {
    recipe.as_object_mut().unwrap().remove("recipe_sha256");
    recipe["recipe_sha256"] = json!(sha256_bytes(
        &aros_toolchain::canonical::bytes(recipe).unwrap()
    ));
}

fn failure(output: &Output, code: &str) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(diagnostic["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(diagnostic["diagnostics"][0]["code"], code);
    assert!(diagnostic["diagnostics"][0]["hint"].is_string());
}

fn inventory(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, result: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                result.push((entry.path(), Vec::new()));
                visit(&entry.path(), result);
            } else {
                result.push((entry.path(), fs::read(entry.path()).unwrap()));
            }
        }
    }
    let mut result = Vec::new();
    visit(root, &mut result);
    result.sort();
    result
}

#[test]
fn global_json_plan_has_exact_identity_and_no_filesystem_mutation() {
    let fixture = Fixture::new();
    let before = inventory(&fixture.root);
    let plan = fixture.plan();
    assert_eq!(inventory(&fixture.root), before);
    assert_eq!(plan["schema"], "aros-toolchain-plan-v1");
    assert_eq!(plan["operation"], "plan");
    assert_eq!(plan["backend"], "legacy-preview");
    assert_eq!(plan["readiness"], "blocked");
    assert_eq!(plan["steps"], json!(["legacy-driver"]));
    assert_eq!(
        plan["identity"]["tools_commit"],
        fixture.recipe["tools_commit"]
    );
    assert!(plan["identity"]["executor"]["tools_commit"].is_null());
    assert!(plan["identity"]["executor"]["origin_evidence_sha256"].is_null());
    assert_eq!(
        plan["identity"]["executor"]["binary_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert!(plan["resources"]["jobs"].is_null());
    assert!(plan["resources"]["timeout_seconds"].is_null());
    assert!(plan["resources"]["free_bytes"].is_null());
    assert!(plan["paths"]["work"].is_null());
    let keys: Vec<_> = plan
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "backend",
            "findings",
            "identity",
            "operation",
            "paths",
            "readiness",
            "resources",
            "schema",
            "steps"
        ]
    );
}

#[test]
fn complete_budgets_remain_blocked_and_do_not_create_roots() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args([
            "--jobs=2",
            "--timeout-seconds=3600",
            "--work-dir=missing/work",
            "--output-dir=missing/output",
            "--cache-dir=missing/cache",
            "--offline",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!fixture.root.join("missing").exists());
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["readiness"], "blocked");
    assert_eq!(plan["resources"]["jobs"], 2);
    assert_eq!(plan["resources"]["offline"], true);
    assert_eq!(
        plan["paths"]["work"],
        json!(fixture.root.join("missing/work"))
    );
}

#[test]
fn recipe_corruption_and_checkout_mismatch_keep_native_diagnostics() {
    let mut fixture = Fixture::new();
    fixture.recipe["recipe_sha256"] = json!("0".repeat(64));
    fixture.save();
    failure(&fixture.command().output().unwrap(), "AX0102");
    fixture.recipe["source_commit"] = json!("0".repeat(40));
    sign(&mut fixture.recipe);
    fixture.save();
    failure(&fixture.command().output().unwrap(), "AX0102");
}

#[test]
fn changed_raw_metadata_fails_without_filters_or_script_execution() {
    let fixture = Fixture::new();
    let marker = fixture.root.join("filter-ran");
    git(
        &fixture.root.join("source"),
        &[
            "config",
            "filter.must-not-run.clean",
            &format!("touch '{}'", marker.display()),
        ],
    );
    git(
        &fixture.root.join("source"),
        &[
            "config",
            "core.fsmonitor",
            &format!("touch '{}'", marker.display()),
        ],
    );
    fixture.plan();
    assert!(!marker.exists());
    fs::write(fixture.root.join("source/patch.diff"), "changed\n").unwrap();
    failure(&fixture.command().output().unwrap(), "AX0102");
    assert!(!marker.exists());
}

#[test]
fn raw_nonmetadata_changes_fail_for_each_selected_checkout() {
    for (root, file) in [
        ("source", "configure"),
        ("producer", "scripts/toolchain/build-release.sh"),
        ("tools", "Cargo.toml"),
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.root.join(root).join(file), "uncommitted source\n").unwrap();
        let output = fixture.command().output().unwrap();
        failure(&output, "AX0102");
        assert!(String::from_utf8_lossy(&output.stderr).contains(&format!("--{root}-dir")));
        let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(diagnostic["diagnostics"][0]["location"]["path"], file);
    }
}

#[test]
fn ignored_untracked_and_empty_directories_fail_without_cleanup() {
    for relative in [
        "source/ignored-private",
        "producer/untracked-private",
        "tools/empty-private",
    ] {
        let fixture = Fixture::new();
        fs::write(
            fixture.root.join("source/.git/info/exclude"),
            "ignored-private\n",
        )
        .unwrap();
        let extra = fixture.root.join(relative);
        if relative.ends_with("empty-private") {
            fs::create_dir(&extra).unwrap();
        } else {
            fs::write(&extra, "must be retained\n").unwrap();
        }
        let before = inventory(&fixture.root);
        let output = fixture.command().output().unwrap();
        failure(&output, "AX0102");
        assert_eq!(inventory(&fixture.root), before);
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private"));
    }
}

#[test]
fn index_flags_and_clean_filters_cannot_hide_raw_changes() {
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        let fixture = Fixture::new();
        let source = fixture.root.join("source");
        let marker = fixture.root.join("filter-must-not-run");
        git(&source, &["update-index", flag, "configure"]);
        git(
            &source,
            &[
                "config",
                "filter.must-not-run.clean",
                &format!("touch '{}'", marker.display()),
            ],
        );
        fs::write(source.join("configure"), "hidden change\n").unwrap();
        assert!(!marker.exists(), "fixture preparation ran a filter");
        failure(&fixture.command().output().unwrap(), "AX0102");
        assert!(!marker.exists());
    }
}

#[test]
fn staged_change_fails_even_when_worktree_bytes_match_head() {
    let fixture = Fixture::new();
    let source = fixture.root.join("source");
    let original = fs::read(source.join("configure")).unwrap();
    fs::write(source.join("configure"), "staged change\n").unwrap();
    git(&source, &["add", "configure"]);
    fs::write(source.join("configure"), original).unwrap();
    failure(&fixture.command().output().unwrap(), "AX0102");
}

#[cfg(unix)]
#[test]
fn raw_links_are_measured_without_following_them_and_modes_are_checked() {
    use std::os::unix::{fs::symlink, fs::PermissionsExt};
    let mut fixture = Fixture::new();
    let source = fixture.root.join("source");
    symlink("../nonexistent-target", source.join("committed-link")).unwrap();
    fixture.recommit("source");
    fixture.plan();
    fs::remove_file(source.join("committed-link")).unwrap();
    symlink("../another-target", source.join("committed-link")).unwrap();
    failure(&fixture.command().output().unwrap(), "AX0102");

    let fixture = Fixture::new();
    let file = fixture.root.join("source/configure");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    git(
        &fixture.root.join("source"),
        &["config", "core.filemode", "false"],
    );
    failure(&fixture.command().output().unwrap(), "AX0102");
}

#[cfg(unix)]
#[test]
fn tracked_fifo_and_symlink_directory_fail_without_following_or_blocking() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let file = fixture.root.join("source/configure");
    fs::remove_file(&file).unwrap();
    assert!(Command::new("mkfifo")
        .arg(&file)
        .status()
        .unwrap()
        .success());
    failure(&fixture.command().output().unwrap(), "AX0102");

    let mut fixture = Fixture::new();
    fs::create_dir(fixture.root.join("source/subdir")).unwrap();
    fs::write(fixture.root.join("source/subdir/file"), "content").unwrap();
    fixture.recommit("source");
    fs::rename(
        fixture.root.join("source/subdir"),
        fixture.root.join("outside"),
    )
    .unwrap();
    symlink("../outside", fixture.root.join("source/subdir")).unwrap();
    failure(&fixture.command().output().unwrap(), "AX0102");
}

#[test]
fn recursively_pinned_modules_require_clean_initialized_exact_checkouts() {
    let mut fixture = Fixture::new();
    let module = fixture.root.join("source/module");
    let nested = module.join("nested");
    for checkout in [&module, &nested] {
        fs::create_dir(checkout).unwrap();
        git(checkout, &["init", "-q"]);
        fs::write(checkout.join("file"), "module content\n").unwrap();
    }
    for checkout in [&nested, &module] {
        git(checkout, &["add", "."]);
        git(checkout, &["commit", "-qm", "test: module fixture"]);
    }
    fixture.recommit("source");
    let before = inventory(&fixture.root);
    fixture.plan();
    assert_eq!(inventory(&fixture.root), before);
    fs::write(nested.join("file"), "dirty module\n").unwrap();
    let output = fixture.command().output().unwrap();
    failure(&output, "AX0102");
    let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        diagnostic["diagnostics"][0]["location"]["path"],
        "module/nested/file"
    );
    fs::write(nested.join("file"), "module content\n").unwrap();
    fs::write(nested.join(".git/info/exclude"), "ignored\n").unwrap();
    fs::write(nested.join("ignored"), "ignored module content\n").unwrap();
    failure(&fixture.command().output().unwrap(), "AX0102");
    fs::remove_file(nested.join("ignored")).unwrap();
    fs::write(nested.join("file"), "different committed module\n").unwrap();
    git(&nested, &["add", "."]);
    git(
        &nested,
        &["commit", "-qm", "test: different module revision"],
    );
    failure(&fixture.command().output().unwrap(), "AX0102");
    fs::rename(&nested, fixture.root.join("retained-module")).unwrap();
    fs::create_dir(&nested).unwrap();
    let output = fixture.command().output().unwrap();
    failure(&output, "AX0102");
    assert!(nested.read_dir().unwrap().next().is_none());
}

#[test]
fn raw_blob_batches_accept_binary_content_and_cross_batch_boundary() {
    let mut fixture = Fixture::new();
    let data: Vec<u8> = (0..9 * 1024 * 1024)
        .map(|index| (index % 256) as u8)
        .collect();
    fs::write(fixture.root.join("source/binary"), &data).unwrap();
    fs::write(fixture.root.join("tools/empty"), []).unwrap();
    fixture.recommit("source");
    fixture.recommit("tools");
    fixture.plan();
}

#[test]
fn missing_raw_object_is_not_fetched_or_repaired() {
    let fixture = Fixture::new();
    let source = fixture.root.join("source");
    let oid = git(&source, &["rev-parse", "HEAD:configure"]);
    let object = source.join(".git/objects").join(&oid[..2]).join(&oid[2..]);
    fs::rename(&object, fixture.root.join("retained-object")).unwrap();
    let before = inventory(&fixture.root);
    let output = fixture.command().output().unwrap();
    failure(&output, "AX0102");
    assert_eq!(inventory(&fixture.root), before);
    assert!(!object.exists());
}

#[test]
fn linked_worktree_metadata_is_supported_without_source_mutation() {
    let mut fixture = Fixture::new();
    let source = fixture.root.join("source");
    let origin = fixture.root.join("source-origin");
    fs::rename(&source, &origin).unwrap();
    git(
        &origin,
        &[
            "worktree",
            "add",
            "--detach",
            source.to_str().unwrap(),
            "HEAD",
        ],
    );
    assert!(source.join(".git").is_file());
    // No new recipe: a linked checkout with exactly the selected identity is
    // supported. Its external administrative files are never modified.
    let before = inventory(&fixture.root);
    fixture.plan();
    assert_eq!(inventory(&fixture.root), before);
    // Keep the fixture binding exercised independently of worktree layout.
    fs::write(source.join("another"), "new material").unwrap();
    fixture.recommit("source");
    fixture.plan();
}

#[test]
fn ambient_git_redirection_and_credentials_do_not_change_inspection() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("GIT_DIR", "/missing/private-repository")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.worktree")
        .env("GIT_CONFIG_VALUE_0", "/missing/secret-value")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-value"));
}

#[test]
fn invalid_profile_and_duplicate_profile_fields_fail_closed() {
    let mut fixture = Fixture::new();
    let profiles = fixture.root.join("producer/toolchains/profiles-v1.json");
    let bad = fs::read_to_string(&profiles).unwrap().replacen(
        '{',
        "{\"schema\":\"aros-toolchain-profiles-v1\",",
        1,
    );
    fs::write(&profiles, &bad).unwrap();
    git(&fixture.root.join("producer"), &["add", "."]);
    git(
        &fixture.root.join("producer"),
        &["commit", "-qm", "test: duplicate profile key"],
    );
    fixture.recipe["profiles_sha256"] = json!(sha256_bytes(bad.as_bytes()));
    fixture.recipe["producer_commit"] =
        json!(git(&fixture.root.join("producer"), &["rev-parse", "HEAD"]));
    fixture.recipe["producer_tree"] = json!(git(
        &fixture.root.join("producer"),
        &["rev-parse", "HEAD^{tree}"]
    ));
    sign(&mut fixture.recipe);
    fixture.save();
    failure(&fixture.command().output().unwrap(), "AX0101");
}

#[test]
fn overlapping_roots_are_errors_not_successful_plans() {
    let fixture = Fixture::new();
    for path in ["source/build", "producer", "tools", "source/../build", "/"] {
        failure(
            &fixture
                .command()
                .arg("--work-dir")
                .arg(path)
                .output()
                .unwrap(),
            "AX0202",
        );
    }
}

#[cfg(unix)]
#[test]
fn aliases_are_resolved_for_ownership_but_input_links_and_fifos_are_refused() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    symlink(fixture.root.join("source"), fixture.root.join("alias")).unwrap();
    failure(
        &fixture
            .command()
            .arg("--work-dir=alias/build")
            .output()
            .unwrap(),
        "AX0202",
    );
    fs::remove_file(fixture.root.join("source/patch.diff")).unwrap();
    symlink(
        fixture.root.join("recipe.json"),
        fixture.root.join("source/patch.diff"),
    )
    .unwrap();
    failure(&fixture.command().output().unwrap(), "AX0202");
    fs::remove_file(fixture.root.join("recipe.json")).unwrap();
    assert!(Command::new("mkfifo")
        .arg(fixture.root.join("recipe.json"))
        .status()
        .unwrap()
        .success());
    failure(&fixture.command().output().unwrap(), "AX0202");
}

#[test]
fn native_default_fails_before_discovering_or_opening_paths() {
    let temporary = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_aros"))
        .current_dir(temporary.path())
        .env_remove("AROS_LOG_FILE")
        .env_remove("AROS_LOG_LEVEL")
        .args([
            "--diagnostic-format=json",
            "toolchain",
            "plan",
            "--preset=pc-x86_64",
            "--recipe=missing",
            "--source-dir=missing",
            "--producer-dir=missing",
            "--tools-dir=missing",
        ])
        .output()
        .unwrap();
    failure(&output, "AX0101");
    assert!(fs::read_dir(temporary.path()).unwrap().next().is_none());
}

#[test]
fn resource_parser_rejects_zero_negative_and_overflow_values() {
    let fixture = Fixture::new();
    for value in ["0", "-1", "18446744073709551616", "1.5"] {
        for option in ["--jobs", "--timeout-seconds"] {
            failure(
                &fixture
                    .command()
                    .arg(format!("{option}={value}"))
                    .output()
                    .unwrap(),
                "AR0001",
            );
        }
    }
}

#[test]
fn oversized_recipe_is_bounded_and_invalid_input_is_not_echoed() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("recipe.json"),
        vec![b' '; 1024 * 1024 + 1],
    )
    .unwrap();
    failure(&fixture.command().output().unwrap(), "AX0101");
    fs::write(
        fixture.root.join("recipe.json"),
        b"{\"private-secret\":\"must-not-echo\"}",
    )
    .unwrap();
    let output = fixture.command().output().unwrap();
    failure(&output, "AX0101");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("must-not-echo"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-secret"));
}

#[test]
fn offline_environment_is_reflected_without_fetching() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("AROS_OFFLINE", "true")
        .output()
        .unwrap();
    assert!(output.status.success());
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["resources"]["offline"], true);
}

#[test]
fn two_digest_matching_locks_are_ambiguous_not_a_hidden_pin() {
    let mut fixture = Fixture::new();
    fs::copy(
        fixture
            .root
            .join("producer/toolchains/arbitrary-version.sources.json"),
        fixture.root.join("producer/toolchains/second.sources.json"),
    )
    .unwrap();
    git(&fixture.root.join("producer"), &["add", "."]);
    git(
        &fixture.root.join("producer"),
        &["commit", "-qm", "test: ambiguous locks"],
    );
    fixture.recipe["producer_commit"] =
        json!(git(&fixture.root.join("producer"), &["rev-parse", "HEAD"]));
    fixture.recipe["producer_tree"] = json!(git(
        &fixture.root.join("producer"),
        &["rev-parse", "HEAD^{tree}"]
    ));
    sign(&mut fixture.recipe);
    fixture.save();
    failure(&fixture.command().output().unwrap(), "AX0102");
}

#[cfg(unix)]
#[test]
fn git_failure_and_deadline_preserve_one_safe_diagnostic() {
    use std::os::unix::fs::PermissionsExt as _;
    let fixture = Fixture::new();
    let bin = fixture.root.join("mock-bin");
    fs::create_dir(&bin).unwrap();
    let program = bin.join("git");
    fs::write(
        &program,
        "#!/bin/sh\necho 'private-child-output' >&2\nexit 7\n",
    )
    .unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let output = fixture.command().env("PATH", &bin).output().unwrap();
    failure(&output, "AX0201");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-child-output"));
    let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["diagnostics"][0]["context"]["tool"], "git");
    assert_eq!(diagnostic["diagnostics"][0]["context"]["exit_code"], 7);
    assert_eq!(diagnostic["diagnostics"][0]["context"]["timed_out"], false);
    assert_eq!(
        diagnostic["diagnostics"][0]["context"]["target"],
        "pc-x86_64"
    );
    fs::write(&program, "#!/bin/sh\nexec /bin/sleep 60\n").unwrap();
    let started = std::time::Instant::now();
    let timeout = fixture.command().env("PATH", &bin).output().unwrap();
    failure(&timeout, "AX0201");
    let diagnostic: Value = serde_json::from_slice(&timeout.stderr).unwrap();
    assert_eq!(diagnostic["diagnostics"][0]["context"]["timed_out"], true);
    assert_eq!(diagnostic["diagnostics"][0]["context"]["timeout_ms"], 10000);
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}

#[test]
fn explicit_logging_keeps_machine_result_and_diagnostics_separate() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["--log-file=inspection.jsonl", "--log-format=jsonl"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["readiness"], "blocked");
    let events = fs::read_to_string(fixture.root.join("inspection.jsonl")).unwrap();
    assert!(events
        .lines()
        .all(|line| serde_json::from_str::<Value>(line).is_ok()));
    assert!(events.contains("invocation.complete"));
}
