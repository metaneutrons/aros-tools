//! Offline provider and consistent fixture "world" for the command tests.
#![allow(
    clippy::type_complexity,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::redundant_closure_for_method_calls,
    reason = "scripted fixtures favour one flat, readable probe table per command"
)]

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::claim::{
    claim_bytes, sha256_hex, Dispatcher, CLAIM_NAME, JOB as DISPATCH_JOB,
    WORKFLOW as DISPATCH_WORKFLOW,
};
use super::json::canonical_line;
use super::provider::{Attestation, Provider};
use super::start::{INPUT_PATHS, JOB as START_JOB, RUNTIME_LOCK_INPUT, WORKFLOW as START_WORKFLOW};
use super::zip_reader::{BUNDLE_BASENAME, RECORD_BASENAME};
use super::{integrity, Result};

pub const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
pub const REPO: &str = "metaneutrons/aros-toolchains";
pub const API_PATH: &str = "repos/metaneutrons/aros-toolchains";
const API: &str = API_PATH;

/// Scripted provider: every unknown path fails closed.
#[derive(Default)]
pub struct Fake {
    pub env: RefCell<BTreeMap<String, String>>,
    pub gets: RefCell<BTreeMap<String, Vec<Value>>>,
    pub log: RefCell<Vec<String>>,
    pub artifact_zip: RefCell<Vec<u8>>,
    pub artifact_downloads: Cell<u32>,
    pub asset: RefCell<Vec<u8>>,
    pub asset_downloads: Cell<u32>,
    pub committed: RefCell<BTreeMap<String, Vec<u8>>>,
    pub check_id: Cell<u64>,
    pub check_id_queue: RefCell<Vec<u64>>,
    pub attestation: RefCell<Option<Value>>,
    pub attested: RefCell<Vec<(String, Vec<u8>, Vec<u8>)>>,
}

impl Fake {
    pub fn set(&self, path: &str, value: Value) {
        self.gets.borrow_mut().insert(path.to_owned(), vec![value]);
    }

    /// Answer `old` for the next `reads` observations, then `new` forever.
    pub fn change_after(&self, path: &str, reads: usize, new: Value) {
        let mut gets = self.gets.borrow_mut();
        let queue = gets.get_mut(path).expect("path is scripted");
        let old = queue[0].clone();
        *queue = std::iter::repeat_n(old, reads).chain([new]).collect();
    }

    pub fn edit(&self, path: &str, change: impl FnOnce(&mut Value)) {
        let mut gets = self.gets.borrow_mut();
        change(&mut gets.get_mut(path).expect("path is scripted")[0]);
    }

    pub fn reads(&self, path: &str) -> usize {
        self.log
            .borrow()
            .iter()
            .filter(|p| p.as_str() == path)
            .count()
    }
}

impl Provider for Fake {
    fn var(&self, name: &str) -> Option<String> {
        self.env.borrow().get(name).cloned()
    }

    fn get(&self, path: &str) -> Result<Value> {
        super::provider::require_fixed_path(path)?;
        self.log.borrow_mut().push(path.to_owned());
        let mut gets = self.gets.borrow_mut();
        match gets.get_mut(path) {
            Some(queue) if queue.len() > 1 => Ok(queue.remove(0)),
            Some(queue) => Ok(queue[0].clone()),
            None => integrity(format!("unscripted provider path {path}")),
        }
    }

    fn committed_file(&self, commit: &str, path: &str, limit: usize) -> Result<Vec<u8>> {
        let bytes = self
            .committed
            .borrow()
            .get(&format!("{commit}:{path}"))
            .cloned();
        match bytes {
            Some(bytes) if bytes.len() <= limit => Ok(bytes),
            _ => integrity("committed file unavailable"),
        }
    }

    fn download_artifact(&self, _: u64, limit: usize) -> Result<Vec<u8>> {
        self.artifact_downloads
            .set(self.artifact_downloads.get() + 1);
        let bytes = self.artifact_zip.borrow().clone();
        if bytes.len() > limit {
            return integrity("artifact exceeds its bound");
        }
        Ok(bytes)
    }

    fn download_asset(&self, _: u64, limit: usize) -> Result<Vec<u8>> {
        self.asset_downloads.set(self.asset_downloads.get() + 1);
        let bytes = self.asset.borrow().clone();
        if bytes.len() > limit {
            return integrity("asset exceeds its bound");
        }
        Ok(bytes)
    }

    fn oidc_check_id(&self, _: &Value, _: &str) -> Result<u64> {
        let mut queue = self.check_id_queue.borrow_mut();
        if !queue.is_empty() {
            return Ok(queue.remove(0));
        }
        match self.check_id.get() {
            0 => integrity("OIDC unavailable"),
            id => Ok(id),
        }
    }

    fn attest(&self, request: &Attestation<'_>) -> Result<Value> {
        let subject = std::fs::read(request.subject).unwrap();
        let bundle = std::fs::read(request.bundle).unwrap();
        let name = request
            .subject
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        self.attested.borrow_mut().push((name, subject, bundle));
        self.attestation
            .borrow()
            .clone()
            .map_or_else(|| integrity("attestation unavailable"), Ok)
    }
}

pub fn hex(label: &str) -> String {
    sha256_hex(label.as_bytes())
}

pub fn zip_of(members: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        for (name, data, deflate) in members {
            let method = if *deflate {
                zip::CompressionMethod::Deflated
            } else {
                zip::CompressionMethod::Stored
            };
            writer
                .start_file(
                    *name,
                    zip::write::SimpleFileOptions::default().compression_method(method),
                )
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }
    buffer.into_inner()
}

pub const RUNTIME_VERSION: &str = "0.9.9";

/// Everything one locked runtime closure consists of.
pub struct RuntimeFixture {
    pub lock: Vec<u8>,
    pub receipt: Vec<u8>,
    pub executable: Vec<u8>,
    pub manifest: Vec<u8>,
}

/// Write a complete, consistent runtime closure (lock bytes returned, the
/// directory `runtime/` with every file, the manifest and the receipt).
pub fn write_runtime(base: &Path) -> RuntimeFixture {
    use std::os::unix::fs::PermissionsExt;
    let root = base.join("runtime");
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let executable = b"aros-executable".to_vec();
    let target = "x86_64-unknown-linux-gnu";
    let archive = format!("aros-tools-v{RUNTIME_VERSION}-{target}.tar.gz");
    let mut files = Vec::new();
    for (path, mode, _) in super::runtime::EXPECTED_FILES {
        let content = if path == "bin/aros" {
            executable.clone()
        } else {
            format!("content of {path}\n").into_bytes()
        };
        std::fs::write(root.join(path), &content).unwrap();
        std::fs::set_permissions(
            root.join(path),
            std::fs::Permissions::from_mode(u32::from_str_radix(mode, 8).unwrap()),
        )
        .unwrap();
        files.push(
            json!({"path": path, "mode": mode, "sha256": sha256_hex(&content),
                          "size": content.len()}),
        );
    }
    let manifest = canonical_line(&json!({
        "schema": 1, "package": "aros-tools", "version": RUNTIME_VERSION, "target": target,
        "source_commit": "b".repeat(40), "source_date_epoch": 1_700_000_000,
        "archive": archive, "archive_sha256": sha256_hex(b"archive"), "archive_size": 7,
        "files": files}));
    std::fs::write(root.join(super::runtime::MANIFEST_NAME), &manifest).unwrap();
    let mut hosts = serde_json::Map::new();
    for (host, target) in super::runtime::HOST_TARGETS {
        let archive = format!("aros-tools-v{RUNTIME_VERSION}-{target}.tar.gz");
        let asset = |name: String, bytes: &[u8]| json!({"sha256": sha256_hex(bytes), "size": bytes.len().max(1), "name": name});
        let manifest_asset = if host == "linux-x86_64" {
            asset(format!("{archive}.manifest.json"), &manifest)
        } else {
            asset(format!("{archive}.manifest.json"), host.as_bytes())
        };
        hosts.insert(host.to_owned(), json!({
            "target": target,
            "archive": {"name": archive, "sha256": sha256_hex(b"archive"), "size": 7},
            "manifest": manifest_asset,
            "archive_bundle": asset(format!("{archive}.sigstore.json"), format!("a{host}").as_bytes()),
            "manifest_bundle": asset(format!("{archive}.manifest.json.sigstore.json"),
                                     format!("m{host}").as_bytes())}));
    }
    let lock_value = json!({
        "schema": "aros-tools-runtime-v1", "repository": "metaneutrons/aros-tools",
        "release_id": 1, "version": RUNTIME_VERSION, "tag": format!("v{RUNTIME_VERSION}"),
        "tag_object": "c".repeat(40), "source_commit": "b".repeat(40),
        "source_tree": "d".repeat(40), "source_date_epoch": 1_700_000_000,
        "signer_workflow": "metaneutrons/aros-tools/.github/workflows/release.yml",
        "certificate_identity": format!(
            "https://github.com/metaneutrons/aros-tools/.github/workflows/release.yml@refs/tags/v{RUNTIME_VERSION}"),
        "certificate_oidc_issuer": "https://token.actions.githubusercontent.com",
        "hosts": hosts});
    let lock = canonical_line(&lock_value);
    let manifest_value: Value = serde_json::from_slice(&manifest).unwrap();
    let receipt = canonical_line(&super::runtime::receipt(
        &lock_value,
        "linux-x86_64",
        &manifest_value,
        &sha256_hex(&manifest),
        &sha256_hex(&lock),
    ));
    std::fs::write(root.join(super::runtime::RECEIPT_NAME), &receipt).unwrap();
    RuntimeFixture {
        lock,
        receipt,
        executable,
        manifest,
    }
}

/// One consistent world: start run, dispatcher run, candidate and runtime.
pub struct World {
    pub fake: Fake,
    #[allow(dead_code, reason = "keeps the temporary directory alive")]
    pub dir: tempfile::TempDir,
    pub base: PathBuf,
    pub source: Value,
    pub dispatcher: Value,
    pub job: Value,
    pub record: Value,
    pub record_bytes: Vec<u8>,
    pub bundle: Vec<u8>,
    pub claim: Vec<u8>,
    pub asset_row: Value,
    pub event: Value,
    pub zip: Vec<u8>,
}

fn run_row(id: u64, workflow: &str, status: &str, conclusion: Value) -> Value {
    json!({"id": id, "run_attempt": 1, "head_sha": COMMIT, "head_branch": "main", "path": workflow,
           "event": "workflow_run", "status": status, "conclusion": conclusion,
           "head_repository": {"id": 4242, "full_name": REPO}})
}

fn job_page(id: u64, check: u64, run: u64, name: &str, status: &str, conclusion: Value) -> Value {
    json!({"total_count": 1, "jobs": [{"id": id, "name": name, "head_sha": COMMIT, "run_id": run,
        "check_run_url": format!("https://api.github.com/{API}/check-runs/{check}"),
        "status": status, "conclusion": conclusion, "labels": ["ubuntu-24.04"]}]})
}

impl World {
    pub fn path(&self, name: &str) -> PathBuf {
        self.base.join(name)
    }

    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let source = json!({"repository": REPO, "repository_id": 4242, "producer_commit": COMMIT,
            "source_ref": "refs/heads/main", "workflow": START_WORKFLOW, "run_id": 1000, "run_attempt": 1});
        let dispatcher = json!({"repository": REPO, "repository_id": 4242, "producer_commit": COMMIT,
            "source_ref": "refs/heads/main", "workflow": DISPATCH_WORKFLOW, "run_id": 2000, "run_attempt": 1});
        let job = json!({"id": 21, "check_run_id": 22, "name": DISPATCH_JOB});

        // Runtime evidence: a real locked closure whose digests the signed start pins.
        let runtime = write_runtime(&base);
        let lock = runtime.lock.clone();
        let receipt = runtime.receipt.clone();
        let executable = runtime.executable;
        let interface = canonical_line(&json!({
            "schema": "aros-tools-runtime-interface-preflight-v1",
            "assurance": "interface-only-exact-selected-binary",
            "runtime": {"path": base.join("runtime/bin/aros").display().to_string(),
                        "sha256": sha256_hex(&executable), "version": RUNTIME_VERSION,
                        "execution_binding": "held-file-descriptor",
                        "file_identity": {"device": 1, "inode": 1, "mode": 0o755,
                                          "size": executable.len(), "mtime_ns": 1, "ctime_ns": 1}}}));
        std::fs::write(base.join("lock.json"), &lock).unwrap();
        std::fs::write(base.join("interface.json"), &interface).unwrap();

        let mut inputs = serde_json::Map::new();
        let fake = Fake::default();
        for path in INPUT_PATHS {
            let content: Vec<u8> = if path == RUNTIME_LOCK_INPUT {
                lock.clone()
            } else {
                path.as_bytes().to_vec()
            };
            inputs.insert(path.to_owned(), sha256_hex(&content).into());
            fake.committed
                .borrow_mut()
                .insert(format!("{COMMIT}:{path}"), content);
        }
        let record = json!({
            "schema": super::start::SCHEMA, "assurance": super::start::ASSURANCE,
            "context": source, "event": "workflow_run",
            "job": {"id": 11, "check_run_id": 12, "name": START_JOB},
            "admission": {
                "schema": "aros-toolchains-release-admission-v2", "assurance": "metadata-only",
                "repository": REPO, "commit": COMMIT, "version": "2.0.0", "tag": "v2.0.0",
                "pull_request": 7, "runs": {"producer_contracts": {"id": 9, "attempt": 1}},
                "governance": {"ruleset_id": 5, "policy_sha256": hex("policy"), "checked_at": 1_700_000_000},
                "inputs": inputs,
                "release_absence": {"candidate_tag_ref": "absent", "visible_release": "absent",
                    "private_draft_absence": "unproven",
                    "required_start_recheck": {"method": "GET",
                        "endpoint": format!("{API}/releases/tags/v2.0.0"),
                        "permission": "contents:write", "absence_status": 404}}},
            "tag": {"name": "v2.0.0", "object": "a".repeat(40), "commit": COMMIT, "epoch": 1_700_000_100},
            "draft": {"id": 31, "tag_name": "v2.0.0", "target_commitish": COMMIT},
            "runtime": {"lock_sha256": sha256_hex(&lock), "receipt_sha256": sha256_hex(&receipt),
                        "interface_sha256": sha256_hex(&interface), "executable_sha256": sha256_hex(&executable)},
        });
        let record_bytes = canonical_line(&record);
        let bundle = b"{\"mediaType\":\"start-bundle\"}".to_vec();
        let claim = claim_bytes(
            &record_bytes,
            &bundle,
            &source,
            &Dispatcher {
                context: &dispatcher,
                job: &job,
                event: "workflow_run",
                trigger_context: &source,
            },
        )
        .unwrap();
        let asset_row = json!({"id": 41, "name": CLAIM_NAME, "state": "uploaded",
            "content_type": "application/json", "size": claim.len(),
            "digest": format!("sha256:{}", sha256_hex(&claim)),
            "url": format!("https://api.github.com/{API}/releases/assets/41")});

        // Provider state.
        for (key, value) in [
            ("GH_HOST", "github.com"),
            ("GH_TOKEN", "job-token"),
            ("GITHUB_EVENT_NAME", "workflow_run"),
            ("GITHUB_REPOSITORY", REPO),
            ("GITHUB_REPOSITORY_ID", "4242"),
            ("GITHUB_SHA", COMMIT),
            ("GITHUB_REF", "refs/heads/main"),
            ("GITHUB_RUN_ID", "2000"),
            ("GITHUB_RUN_ATTEMPT", "1"),
        ] {
            fake.env
                .borrow_mut()
                .insert(key.to_owned(), value.to_owned());
        }
        fake.env.borrow_mut().insert(
            "GITHUB_EVENT_PATH".into(),
            base.join("event.json").display().to_string(),
        );
        fake.check_id.set(22);
        fake.set(
            &format!("{API}/actions/runs/1000/attempts/1"),
            run_row(1000, START_WORKFLOW, "completed", json!("success")),
        );
        fake.set(
            &format!("{API}/actions/runs/1000"),
            run_row(1000, START_WORKFLOW, "completed", json!("success")),
        );
        fake.set(
            &format!("{API}/actions/runs/2000/attempts/1"),
            run_row(2000, DISPATCH_WORKFLOW, "in_progress", Value::Null),
        );
        fake.set(
            &format!("{API}/actions/runs/2000"),
            run_row(2000, DISPATCH_WORKFLOW, "in_progress", Value::Null),
        );
        fake.set(
            &format!("{API}/actions/runs/2000/attempts/1/jobs?per_page=100&page=1"),
            job_page(21, 22, 2000, DISPATCH_JOB, "in_progress", Value::Null),
        );
        fake.set(
            &format!("{API}/actions/runs/1000/attempts/1/jobs?per_page=100&page=1"),
            job_page(11, 12, 1000, START_JOB, "completed", json!("success")),
        );
        fake.set(API, json!({"full_name": REPO, "archived": false, "disabled": false, "default_branch": "main"}));
        fake.set(&format!("{API}/git/ref/tags/v2.0.0"), json!({"ref": "refs/tags/v2.0.0",
            "object": {"type": "tag", "sha": "a".repeat(40), "url": format!("https://api.github.com/{API}/git/tags/{}", "a".repeat(40))}}));
        fake.set(&format!("{API}/git/tags/{}", "a".repeat(40)), json!({"sha": "a".repeat(40), "tag": "v2.0.0",
            "object": {"type": "commit", "sha": COMMIT}, "tagger": {"date": "2023-11-14T22:15:00Z"}}));
        let release = json!({"id": 31, "tag_name": "v2.0.0", "target_commitish": COMMIT, "draft": true,
            "prerelease": false, "immutable": false, "assets": [asset_row]});
        fake.set(&format!("{API}/releases/31"), release.clone());
        fake.set(&format!("{API}/releases/tags/v2.0.0"), release);
        *fake.asset.borrow_mut() = claim.clone();
        *fake.attestation.borrow_mut() = Some(Self::attestation_for(&record_bytes, &source));

        // Start artifact transport.
        let zip = zip_of(&[
            (RECORD_BASENAME, &record_bytes, true),
            (BUNDLE_BASENAME, &bundle, false),
        ]);
        let artifact = json!({"id": 51, "name": "compiler-sdk-release-start-v2-1000-1", "size_in_bytes": zip.len(),
            "digest": format!("sha256:{}", sha256_hex(&zip)), "expired": false,
            "workflow_run": {"id": 1000, "head_sha": COMMIT, "repository_id": 4242, "head_repository_id": 4242}});
        fake.set(
            &format!("{API}/actions/runs/1000/artifacts?per_page=100&page=1"),
            json!({"total_count": 1, "artifacts": [artifact]}),
        );
        fake.set(&format!("{API}/actions/artifacts/51"), artifact);
        *fake.artifact_zip.borrow_mut() = zip.clone();
        let event = json!({"action": "completed", "repository": {"id": 4242, "full_name": REPO},
            "workflow_run": {"id": 1000, "run_attempt": 1, "head_sha": COMMIT, "head_branch": "main",
                "event": "workflow_run", "status": "completed", "conclusion": "success",
                "head_repository": {"id": 4242, "full_name": REPO}}});
        std::fs::write(base.join("event.json"), serde_json::to_vec(&event).unwrap()).unwrap();

        // Selected input files.
        std::fs::write(base.join(RECORD_BASENAME), &record_bytes).unwrap();
        std::fs::write(base.join(BUNDLE_BASENAME), &bundle).unwrap();
        std::fs::write(base.join(CLAIM_NAME), &claim).unwrap();
        Self {
            fake,
            dir,
            base,
            source,
            dispatcher,
            job,
            record,
            record_bytes,
            bundle,
            claim,
            asset_row,
            event,
            zip,
        }
    }

    pub fn attestation_for(subject: &[u8], context: &Value) -> Value {
        json!([{"verificationResult": {
            "signature": {"certificate": {"runInvocationURI": format!(
                "https://github.com/{REPO}/actions/runs/{}/attempts/1", context["run_id"])}},
            "statement": {"subject": [{"name": RECORD_BASENAME, "digest": {"sha256": sha256_hex(subject)}}]}}}])
    }
}
