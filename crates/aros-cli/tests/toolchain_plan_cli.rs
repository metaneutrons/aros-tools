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
    fn enable_grouped_native(&mut self) {
        let llvm_lock_path = "toolchains/arbitrary-version.sources.json";
        let llvm_profiles_path = "toolchains/profiles-v1.json";
        let gnu_lock_path = "toolchains/gnu.sources.json";
        let gnu_profiles_path = "toolchains/gnu-profiles.json";
        let llvm_lock = fs::read(self.root.join("producer").join(llvm_lock_path)).unwrap();
        let llvm_profiles = fs::read(self.root.join("producer").join(llvm_profiles_path)).unwrap();
        let gnu_lock =
            include_bytes!("../../aros-toolchain/tests/fixtures/gnu-source-lock-v3.json");
        let gnu_profiles = serde_json::to_vec(&json!({
            "schema":"aros-toolchain-profiles-v2", "family":"gnu",
            "upstream_commit": self.recipe["source_commit"], "profiles":[{
                "name":"rv32-esp32p4", "configure_target":"esp32p4-riscv",
                "upstream_output_target":"esp32p4-riscv", "target_triple":"riscv-aros",
                "cpu":"riscv", "platform":"esp32p4", "float_abi":"ilp32f",
                "capabilities":["c", "libgcc", "standalone-collector"],
                "target":{"schema":"aros-riscv-target-v1", "isa":"rv32imafc", "abi":"ilp32f",
                    "code_model":"medany", "architecture":"rv32i2p1_m2p0_a2p1_f2p2_c2p0",
                    "unaligned_access":false, "atomic_abi":0, "x3_reg_usage":0}
            }]
        }))
        .unwrap();
        fs::write(self.root.join("producer").join(gnu_lock_path), gnu_lock).unwrap();
        fs::write(
            self.root.join("producer").join(gnu_profiles_path),
            &gnu_profiles,
        )
        .unwrap();
        let contract =
            fs::read(self.root.join("tools/contracts/toolchain-producer-v1.toml")).unwrap();
        fs::write(
            self.root.join("producer/toolchains/producer-executor-v1.toml"),
            format!(
                "schema_version = 2\ncontract_id = 'aros-toolchain-producer-v1'\ncontract_path = 'contracts/toolchain-producer-v1.toml'\ncontract_sha256 = '{}'\ntools_commit = '{}'\n\
                [[groups]]\nid = 'gnu-rv32'\nsource_lock = '{gnu_lock_path}'\nsource_lock_sha256 = '{}'\nprofiles = '{gnu_profiles_path}'\nprofiles_sha256 = '{}'\n\
                [[groups]]\nid = 'llvm'\nsource_lock = '{llvm_lock_path}'\nsource_lock_sha256 = '{}'\nprofiles = '{llvm_profiles_path}'\nprofiles_sha256 = '{}'\n",
                sha256_bytes(&contract), self.recipe["tools_commit"].as_str().unwrap(),
                sha256_bytes(gnu_lock), sha256_bytes(&gnu_profiles),
                sha256_bytes(&llvm_lock), sha256_bytes(&llvm_profiles),
            ),
        ).unwrap();
        self.recommit("producer");
    }

    fn select_gnu_recipe(&mut self) {
        self.recipe["source_lock_sha256"] = json!(sha256_bytes(
            &fs::read(self.root.join("producer/toolchains/gnu.sources.json")).unwrap(),
        ));
        self.recipe["profiles_sha256"] = json!(sha256_bytes(
            &fs::read(self.root.join("producer/toolchains/gnu-profiles.json")).unwrap(),
        ));
        self.recipe["patches"] = json!([]);
        sign(&mut self.recipe);
        self.save();
    }

    fn enable_native(&mut self) {
        let patch_path = "tools/crosstools/llvm/llvm-11.0.0.src-aros.diff";
        fs::create_dir_all(self.root.join("source/tools/crosstools/llvm")).unwrap();
        fs::write(
            self.root.join("source").join(patch_path),
            b"fixture native patch\n",
        )
        .unwrap();
        self.recipe["patches"] = json!([{
            "path": patch_path,
            "sha256": sha256_bytes(b"fixture native patch\n"),
        }]);
        self.recommit("source");
        let lock = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
            "sources": [{
                "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
                "patch": patch_path,
                "filename": "llvm-11.0.0.src.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
                "sha256": "a".repeat(64), "size": 1
            }],
            "host_python_packages": [
                {"name": "mako", "version": "1.3.10", "filename": "mako.tar.gz", "url": "https://example.invalid/mako.tar.gz", "sha256": "b".repeat(64), "size": 1, "source_root": "mako", "python_path": "."},
                {"name": "markupsafe", "version": "3.0.2", "filename": "markupsafe.tar.gz", "url": "https://example.invalid/markupsafe.tar.gz", "sha256": "c".repeat(64), "size": 1, "source_root": "markupsafe", "python_path": "."}
            ]
        }))
        .unwrap();
        let contract = b"fixture native contract\n";
        fs::create_dir_all(self.root.join("tools/contracts")).unwrap();
        fs::write(
            self.root.join("tools/contracts/toolchain-producer-v1.toml"),
            contract,
        )
        .unwrap();
        self.recommit("tools");
        fs::write(
            self.root
                .join("producer/toolchains/arbitrary-version.sources.json"),
            &lock,
        )
        .unwrap();
        fs::write(
            self.root.join("producer/toolchains/producer-executor-v1.toml"),
            format!(
                "schema_version = 1\ncontract_id = \"aros-toolchain-producer-v1\"\ncontract_path = \"contracts/toolchain-producer-v1.toml\"\ncontract_sha256 = \"{}\"\ntools_commit = \"{}\"\nsource_lock = \"toolchains/arbitrary-version.sources.json\"\nprofiles = \"toolchains/profiles-v1.json\"\n",
                sha256_bytes(contract),
                self.recipe["tools_commit"].as_str().unwrap(),
            ),
        )
        .unwrap();
        self.recipe["source_lock_sha256"] = json!(sha256_bytes(&lock));
        self.recommit("producer");
    }

    fn refresh_native_tools_commit(&mut self) {
        let declaration = self
            .root
            .join("producer/toolchains/producer-executor-v1.toml");
        let replacement = format!(
            "tools_commit = \"{}\"",
            self.recipe["tools_commit"].as_str().unwrap()
        );
        let updated = fs::read_to_string(&declaration)
            .unwrap()
            .lines()
            .map(|line| {
                if line.starts_with("tools_commit = ") {
                    replacement.as_str()
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&declaration, format!("{updated}\n")).unwrap();
        self.recommit("producer");
    }

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
        let mut fixture = Self {
            _temporary: temporary,
            root,
            recipe,
        };
        fixture.save();
        fixture.enable_native();
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
        self.command_for("pc-x86_64")
    }

    fn command_for(&self, preset: &str) -> Command {
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
                "--preset",
                preset,
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

fn first_inventory_difference(
    root: &Path,
    before: &[(PathBuf, Vec<u8>)],
    after: &[(PathBuf, Vec<u8>)],
) -> String {
    let mut before_index = 0;
    let mut after_index = 0;
    loop {
        match (before.get(before_index), after.get(after_index)) {
            (Some((before_path, before_bytes)), Some((after_path, after_bytes))) => {
                match before_path.cmp(after_path) {
                    std::cmp::Ordering::Less => {
                        return format!(
                            "{} removed (before: {} bytes, sha256={})",
                            before_path
                                .strip_prefix(root)
                                .unwrap_or(before_path)
                                .display(),
                            before_bytes.len(),
                            sha256_bytes(before_bytes),
                        );
                    }
                    std::cmp::Ordering::Greater => {
                        return format!(
                            "{} added (after: {} bytes, sha256={})",
                            after_path
                                .strip_prefix(root)
                                .unwrap_or(after_path)
                                .display(),
                            after_bytes.len(),
                            sha256_bytes(after_bytes),
                        );
                    }
                    std::cmp::Ordering::Equal => {
                        if before_bytes != after_bytes {
                            return format!(
                                "{} content changed (before: {} bytes, sha256={}; after: {} bytes, sha256={})",
                                before_path.strip_prefix(root).unwrap_or(before_path).display(),
                                before_bytes.len(),
                                sha256_bytes(before_bytes),
                                after_bytes.len(),
                                sha256_bytes(after_bytes),
                            );
                        }
                    }
                }
                before_index += 1;
                after_index += 1;
            }
            (Some((before_path, before_bytes)), None) => {
                return format!(
                    "{} removed (before: {} bytes, sha256={})",
                    before_path
                        .strip_prefix(root)
                        .unwrap_or(before_path)
                        .display(),
                    before_bytes.len(),
                    sha256_bytes(before_bytes),
                );
            }
            (None, Some((after_path, after_bytes))) => {
                return format!(
                    "{} added (after: {} bytes, sha256={})",
                    after_path
                        .strip_prefix(root)
                        .unwrap_or(after_path)
                        .display(),
                    after_bytes.len(),
                    sha256_bytes(after_bytes),
                );
            }
            (None, None) => return "inventories differed without a path difference".to_owned(),
        }
    }
}

#[test]
fn first_inventory_difference_reports_changed_added_and_removed_entries() {
    let root = PathBuf::from("/fixture");
    let entry = |name: &str, bytes: &[u8]| (root.join(name), bytes.to_vec());
    let common_before = entry("a-common", b"same");
    let common_after = entry("a-common", b"same");

    assert_eq!(
        first_inventory_difference(
            &root,
            &[common_before.clone(), entry("b-changed", b"old")],
            &[common_after.clone(), entry("b-changed", b"new")],
        ),
        format!(
            "b-changed content changed (before: 3 bytes, sha256={}; after: 3 bytes, sha256={})",
            sha256_bytes(b"old"),
            sha256_bytes(b"new"),
        ),
    );
    assert_eq!(
        first_inventory_difference(
            &root,
            std::slice::from_ref(&common_before),
            &[common_after.clone(), entry("c-added", b"new")],
        ),
        format!(
            "c-added added (after: 3 bytes, sha256={})",
            sha256_bytes(b"new"),
        ),
    );
    assert_eq!(
        first_inventory_difference(
            &root,
            &[common_before, entry("d-removed", b"old")],
            &[common_after],
        ),
        format!(
            "d-removed removed (before: 3 bytes, sha256={})",
            sha256_bytes(b"old"),
        ),
    );
}

#[test]
fn global_json_plan_has_exact_identity_and_no_filesystem_mutation() {
    let fixture = Fixture::new();
    let before = inventory(&fixture.root);
    let plan = fixture.plan();
    assert_eq!(inventory(&fixture.root), before);
    assert_eq!(plan["schema"], "aros-toolchain-plan-v2");
    assert_eq!(plan["operation"], "plan");
    assert_eq!(plan["readiness"], "incomplete");
    assert_eq!(
        plan["steps"],
        json!([
            "preflight",
            "sources",
            "environment",
            "configure",
            "compiler",
            "collector"
        ])
    );
    assert_eq!(
        plan["identity"]["tools_commit"],
        fixture.recipe["tools_commit"]
    );
    assert_eq!(
        plan["identity"]["executor"]["tools_commit"],
        fixture.recipe["tools_commit"]
    );
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
fn v2_native_plan_selects_llvm_and_rv32_from_one_producer_without_mutation() {
    let mut fixture = Fixture::new();
    fixture.enable_grouped_native();
    let producer_commit = fixture.recipe["producer_commit"].clone();
    let llvm_recipe_digest = fixture.recipe["recipe_sha256"].clone();
    let before = inventory(&fixture.root);
    let llvm = fixture.plan();
    assert_eq!(inventory(&fixture.root), before);
    assert_eq!(llvm["identity"]["producer_commit"], producer_commit);
    assert_eq!(llvm["identity"]["target_profile"], "pc-x86_64");
    fixture.select_gnu_recipe();
    assert_ne!(fixture.recipe["recipe_sha256"], llvm_recipe_digest);
    let before = inventory(&fixture.root);
    let output = fixture.command_for("rv32-esp32p4").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let gnu: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(gnu["identity"]["producer_commit"], producer_commit);
    assert_eq!(gnu["identity"]["target_profile"], "rv32-esp32p4");
    assert_eq!(
        gnu["identity"]["recipe_sha256"],
        fixture.recipe["recipe_sha256"]
    );
    assert_eq!(gnu["readiness"], "incomplete");
    assert_eq!(inventory(&fixture.root), before);
}

#[test]
fn v2_native_plan_rejects_changed_unselected_committed_group() {
    let mut fixture = Fixture::new();
    fixture.enable_grouped_native();
    let path = fixture.root.join("producer/toolchains/gnu.sources.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b' ');
    fs::write(&path, bytes).unwrap();
    // Rebind the producer commit/tree: the expected failure must be the
    // declaration's stale digest, not an uncommitted-change or Git mismatch.
    fixture.recommit("producer");
    let before = inventory(&fixture.root);
    let output = fixture.command().output().unwrap();
    failure(&output, "AX0102");
    let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(diagnostic["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("declared digests"));
    assert_eq!(inventory(&fixture.root), before);
}

#[test]
fn v2_native_plan_rejects_preset_from_the_other_recipe_group() {
    let mut fixture = Fixture::new();
    fixture.enable_grouped_native();
    let before = inventory(&fixture.root);
    failure(
        &fixture.command_for("rv32-esp32p4").output().unwrap(),
        "AX0102",
    );
    failure(&fixture.command_for("unknown").output().unwrap(), "AX0102");
    assert_eq!(inventory(&fixture.root), before);
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
    assert_eq!(plan["readiness"], "ready");
    assert_eq!(plan["resources"]["jobs"], 2);
    assert_eq!(plan["resources"]["input_mode"], "prepared-cache-only");
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
        let after = inventory(&fixture.root);
        assert!(
            after == before,
            "filesystem inventory changed: {}",
            first_inventory_difference(&fixture.root, &before, &after),
        );
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
    fixture.refresh_native_tools_commit();
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
fn input_links_and_fifos_are_refused_without_following_them() {
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
    failure(&fixture.command().output().unwrap(), "AX0102");
    fs::remove_file(fixture.root.join("recipe.json")).unwrap();
    assert!(Command::new("mkfifo")
        .arg(fixture.root.join("recipe.json"))
        .status()
        .unwrap()
        .success());
    failure(&fixture.command().output().unwrap(), "AX0202");
}

#[test]
fn native_default_requires_real_selected_inputs_without_creating_paths() {
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
    failure(&output, "AX0202");
    assert!(fs::read_dir(temporary.path()).unwrap().next().is_none());
}

#[test]
fn public_toolchain_commands_do_not_expose_a_legacy_backend_switch() {
    for command in ["plan", "build"] {
        let output = Command::new(env!("CARGO_BIN_EXE_aros"))
            .args(["toolchain", command, "--help"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(!String::from_utf8(output.stdout)
            .unwrap()
            .contains("--backend"));
    }
}

#[test]
fn native_plan_binds_its_declared_contract_without_mutation() {
    let fixture = Fixture::new();
    let before = inventory(&fixture.root);
    let output = fixture.command().output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(inventory(&fixture.root), before);
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["readiness"], "incomplete");
    assert_eq!(
        plan["steps"],
        json!([
            "preflight",
            "sources",
            "environment",
            "configure",
            "compiler",
            "collector"
        ])
    );
    assert_eq!(
        plan["identity"]["executor"]["tools_commit"],
        fixture.recipe["tools_commit"]
    );
    assert!(plan["identity"]["executor"]["contract_sha256"].is_string());
}

#[test]
fn native_plan_exposes_a_fixed_prepared_input_mode_when_ready() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args([
            "--work-dir=work",
            "--output-dir=output",
            "--cache-dir=cache",
            "--jobs=1",
            "--timeout-seconds=60",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["readiness"], "ready");
    assert_eq!(plan["resources"]["input_mode"], "prepared-cache-only");
    assert!(plan["resources"].get("offline").is_none());
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
fn offline_environment_does_not_change_the_fixed_native_input_mode() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("AROS_OFFLINE", "true")
        .output()
        .unwrap();
    assert!(output.status.success());
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["resources"]["input_mode"], "prepared-cache-only");
    assert!(plan["resources"].get("offline").is_none());
}

#[test]
fn selected_native_lock_is_unambiguous_even_with_a_matching_sibling() {
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
    fixture.plan();
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
    assert_eq!(
        diagnostic["diagnostics"][0]["context"]["mode"],
        "toolchain.plan"
    );
    assert!(diagnostic["diagnostics"][0]["hint"]
        .as_str()
        .is_some_and(|hint| !hint.is_empty()));
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
    assert_eq!(
        diagnostic["diagnostics"][0]["context"]["mode"],
        "toolchain.plan"
    );
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
    assert_eq!(plan["readiness"], "incomplete");
    let events = fs::read_to_string(fixture.root.join("inspection.jsonl")).unwrap();
    assert!(events
        .lines()
        .all(|line| serde_json::from_str::<Value>(line).is_ok()));
    assert!(events.contains("invocation.complete"));
}
