//! Offline tests of `select`, `verify-claim`, `dispatch` and `verify-binding`
//! against the scripted provider, with counterprobes for every closed join.
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

use serde_json::{json, Value};

use super::binding::{binding_bytes, Selection, BINDING_NAME};
use super::claim::{sha256_hex, Dispatcher, CLAIM_NAME};
use super::select::{select_start_transport, source_context_from_event, ARTIFACT_PREFIX};
use super::testing::{zip_of, World, API_PATH, COMMIT, REPO};
use super::verify_binding::{verify_binding_signature, Request as BindingRequest};
use super::verify_claim::{verify_claimed_start, Request as ClaimRequest};
use super::zip_reader::{BUNDLE_BASENAME, RECORD_BASENAME};
use super::{dispatch, Kind};

fn runs(id: u64) -> String {
    format!("{API_PATH}/actions/runs/{id}")
}

// ---------------------------------------------------------------- select ---

#[test]
fn select_returns_the_exact_sealed_bytes_with_one_download() {
    let world = World::new();
    let selected = select_start_transport(&world.fake).unwrap();
    assert_eq!(selected.record_bytes, world.record_bytes);
    assert_eq!(selected.bundle_bytes, world.bundle);
    assert_eq!(selected.source_context, world.source);
    assert_eq!(selected.dispatcher_identity["context"], world.dispatcher);
    assert_eq!(selected.dispatcher_identity["job"]["check_run_id"], 22);
    assert_eq!(world.fake.artifact_downloads.get(), 1);
    // Both selector surfaces are observed before and after the one download.
    assert!(
        world
            .fake
            .reads(&format!("{API_PATH}/actions/artifacts/51"))
            >= 2
    );
    assert!(
        world.fake.reads(&format!(
            "{API_PATH}/actions/runs/1000/artifacts?per_page=100&page=1"
        )) >= 2
    );
}

#[test]
fn select_retains_three_exclusive_private_files() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new();
    let out = world.path("out");
    std::fs::create_dir(&out).unwrap();
    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o700)).unwrap();
    let selected = select_start_transport(&world.fake).unwrap();
    super::select::retain(&out, &selected).unwrap();
    assert_eq!(
        std::fs::read(out.join(RECORD_BASENAME)).unwrap(),
        world.record_bytes
    );
    assert_eq!(
        std::fs::read(out.join(BUNDLE_BASENAME)).unwrap(),
        world.bundle
    );
    let report: Value =
        serde_json::from_slice(&std::fs::read(out.join(super::select::SELECTION_NAME)).unwrap())
            .unwrap();
    assert_eq!(report["assurance"], super::select::ASSURANCE);
    assert_eq!(report["record_sha256"], sha256_hex(&world.record_bytes));
    // A second retention never replaces anything.
    assert!(super::select::retain(&out, &selected).is_err());
    // A group-readable parent is refused before any file is written.
    let open = world.path("open");
    std::fs::create_dir(&open).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(super::select::retain(&open, &selected).is_err());
    assert!(std::fs::read_dir(&open).unwrap().next().is_none());
}

#[test]
fn select_environment_counterprobes() {
    for (name, value) in [
        ("GH_HOST", "ghe.example.com"),
        ("GH_TOKEN", "  "),
        ("GITHUB_EVENT_NAME", "push"),
        ("GITHUB_EVENT_PATH", "relative/event.json"),
        ("GITHUB_EVENT_PATH", ""),
        ("GITHUB_RUN_ID", "1000"),
        ("GITHUB_RUN_ATTEMPT", "2"),
        ("GITHUB_REF", "refs/heads/dev"),
        ("GITHUB_REPOSITORY_ID", "1"),
    ] {
        let world = World::new();
        world
            .fake
            .env
            .borrow_mut()
            .insert(name.to_owned(), value.to_owned());
        assert!(
            select_start_transport(&world.fake).is_err(),
            "{name}={value}"
        );
        assert_eq!(world.fake.artifact_downloads.get(), 0, "{name}");
    }
    let world = World::new();
    world.fake.env.borrow_mut().remove("GH_TOKEN");
    assert!(select_start_transport(&world.fake).is_err());
}

#[test]
fn select_event_counterprobes() {
    let edits: [(&str, fn(&mut Value)); 11] = [
        ("action", |e| e["action"] = "requested".into()),
        ("attempt", |e| e["workflow_run"]["run_attempt"] = 2.into()),
        ("branch", |e| {
            e["workflow_run"]["head_branch"] = "dev".into();
        }),
        ("conclusion", |e| {
            e["workflow_run"]["conclusion"] = "failure".into();
        }),
        ("status", |e| {
            e["workflow_run"]["status"] = "in_progress".into();
        }),
        ("event", |e| e["workflow_run"]["event"] = "push".into()),
        ("repository", |e| {
            e["repository"]["full_name"] = "evil/aros-toolchains".into();
        }),
        ("head repository", |e| {
            e["workflow_run"]["head_repository"]["id"] = 7.into();
        }),
        ("sha", |e| e["workflow_run"]["head_sha"] = "XYZ".into()),
        ("typed ids", |e| e["workflow_run"]["id"] = "1000".into()),
        ("forked head", |e| {
            e["workflow_run"]["head_repository"]["full_name"] = "fork/aros-toolchains".into();
        }),
    ];
    for (label, edit) in edits {
        let world = World::new();
        let mut event = world.event.clone();
        edit(&mut event);
        assert!(source_context_from_event(&event).is_err(), "{label}");
        std::fs::write(
            world.path("event.json"),
            serde_json::to_vec(&event).unwrap(),
        )
        .unwrap();
        assert!(select_start_transport(&world.fake).is_err(), "{label}");
    }
    // Duplicate keys and oversized events never parse.
    let world = World::new();
    std::fs::write(
        world.path("event.json"),
        br#"{"action":"completed","action":"completed"}"#,
    )
    .unwrap();
    assert!(select_start_transport(&world.fake).is_err());
    std::fs::write(world.path("event.json"), vec![b' '; 1024 * 1024 + 1]).unwrap();
    assert!(select_start_transport(&world.fake).is_err());
    // A symlinked event path is not canonical.
    let world = World::new();
    std::os::unix::fs::symlink(world.path("event.json"), world.path("link.json")).unwrap();
    world.fake.env.borrow_mut().insert(
        "GITHUB_EVENT_PATH".into(),
        world.path("link.json").display().to_string(),
    );
    assert!(select_start_transport(&world.fake).is_err());
}

#[test]
fn select_dispatcher_authentication_counterprobes() {
    let probes: [(&str, Box<dyn Fn(&World)>); 8] = [
        ("no OIDC", Box::new(|w| w.fake.check_id.set(0))),
        ("OIDC selects no job", Box::new(|w| w.fake.check_id.set(99))),
        (
            "dispatcher rerun",
            Box::new(|w| w.fake.edit(&runs(2000), |v| v["run_attempt"] = 2.into())),
        ),
        (
            "dispatcher finished",
            Box::new(|w| {
                w.fake.edit(&runs(2000), |v| {
                    v["status"] = "completed".into();
                    v["conclusion"] = "success".into();
                });
            }),
        ),
        (
            "dispatcher wrong path",
            Box::new(|w| {
                w.fake.edit(&format!("{}/attempts/1", runs(2000)), |v| {
                    v["path"] = ".github/workflows/other.yml".into();
                });
            }),
        ),
        (
            "dispatcher job name",
            Box::new(|w| {
                w.fake.edit(
                    &format!("{}/attempts/1/jobs?per_page=100&page=1", runs(2000)),
                    |v| v["jobs"][0]["name"] = "other".into(),
                );
            }),
        ),
        (
            "start rerun",
            Box::new(|w| w.fake.edit(&runs(1000), |v| v["run_attempt"] = 2.into())),
        ),
        (
            "start failed",
            Box::new(|w| {
                w.fake.edit(&format!("{}/attempts/1", runs(1000)), |v| {
                    v["conclusion"] = "failure".into();
                });
            }),
        ),
    ];
    for (label, probe) in probes {
        let world = World::new();
        probe(&world);
        assert!(select_start_transport(&world.fake).is_err(), "{label}");
        assert_eq!(
            world.fake.artifact_downloads.get(),
            0,
            "{label}: nothing is downloaded before authentication"
        );
    }
    // The dispatcher job changing between the two authentications is refused.
    let world = World::new();
    let jobs = format!("{}/attempts/1/jobs?per_page=100&page=1", runs(2000));
    let mut other = world.fake.gets.borrow()[&jobs][0].clone();
    other["jobs"][0]["check_run_url"] =
        format!("https://api.github.com/{API_PATH}/check-runs/23").into();
    world.fake.change_after(&jobs, 1, other);
    assert!(select_start_transport(&world.fake).is_err());
}

#[test]
fn select_refuses_a_dispatcher_identity_that_changes_during_transport() {
    // Two valid owning jobs exist; the second OIDC answer selects the other one.
    let world = World::new();
    let jobs = format!("{}/attempts/1/jobs?per_page=100&page=1", runs(2000));
    world.fake.edit(&jobs, |page| {
        let mut second = page["jobs"][0].clone();
        second["id"] = 24.into();
        second["check_run_url"] = format!("https://api.github.com/{API_PATH}/check-runs/23").into();
        page["jobs"].as_array_mut().unwrap().push(second);
        page["total_count"] = 2.into();
    });
    world.fake.check_id_queue.borrow_mut().extend([22, 23]);
    assert!(select_start_transport(&world.fake).is_err());
    // Control: the same two-job page with a stable OIDC answer succeeds.
    let world = World::new();
    world.fake.edit(&jobs, |page| {
        let mut second = page["jobs"][0].clone();
        second["id"] = 24.into();
        second["check_run_url"] = format!("https://api.github.com/{API_PATH}/check-runs/23").into();
        page["jobs"].as_array_mut().unwrap().push(second);
        page["total_count"] = 2.into();
    });
    assert!(select_start_transport(&world.fake).is_ok());
}

fn artifact_row(world: &World) -> Value {
    world.fake.gets.borrow()[&format!("{API_PATH}/actions/artifacts/51")][0].clone()
}

#[test]
fn select_artifact_metadata_counterprobes() {
    let list = format!("{API_PATH}/actions/runs/1000/artifacts?per_page=100&page=1");
    let direct = format!("{API_PATH}/actions/artifacts/51");
    let edits: [(&str, fn(&mut Value)); 12] = [
        ("expired", |a| a["expired"] = true.into()),
        ("name", |a| {
            a["name"] = format!("{ARTIFACT_PREFIX}1000-2").into();
        }),
        ("digest", |a| a["digest"] = "sha256:ABC".into()),
        ("digest prefix", |a| {
            a["digest"] = a["digest"]
                .as_str()
                .unwrap()
                .replace("sha256:", "md5:")
                .into();
        }),
        ("size type", |a| a["size_in_bytes"] = "10".into()),
        ("size zero", |a| a["size_in_bytes"] = 0.into()),
        ("size huge", |a| a["size_in_bytes"] = 99_999_999_u64.into()),
        ("run id", |a| a["workflow_run"]["id"] = 1001.into()),
        ("run sha", |a| {
            a["workflow_run"]["head_sha"] = "f".repeat(40).into();
        }),
        ("repo id", |a| a["workflow_run"]["repository_id"] = 9.into()),
        ("head repo id", |a| {
            a["workflow_run"]["head_repository_id"] = 9.into();
        }),
        ("id type", |a| a["id"] = "51".into()),
    ];
    for (label, edit) in edits {
        let world = World::new();
        let mut row = artifact_row(&world);
        edit(&mut row);
        world
            .fake
            .set(&list, json!({"total_count": 1, "artifacts": [row.clone()]}));
        assert!(select_start_transport(&world.fake).is_err(), "list {label}");
        let world = World::new();
        world.fake.set(&direct, row);
        assert!(
            select_start_transport(&world.fake).is_err(),
            "direct {label}"
        );
    }
    // Direct and listed metadata must be identical.
    let world = World::new();
    let mut other = artifact_row(&world);
    other["id"] = 52.into();
    world.fake.set(&direct, other);
    assert!(select_start_transport(&world.fake).is_err());
    // Neither zero nor two start artifacts select a start.
    let world = World::new();
    world
        .fake
        .set(&list, json!({"total_count": 0, "artifacts": []}));
    assert!(select_start_transport(&world.fake).is_err());
    let world = World::new();
    let first = artifact_row(&world);
    let mut second = first.clone();
    second["id"] = 52.into();
    world.fake.set(
        &list,
        json!({"total_count": 2, "artifacts": [first, second]}),
    );
    assert!(select_start_transport(&world.fake).is_err());
    // Duplicate IDs in the listing.
    let world = World::new();
    let row = artifact_row(&world);
    world.fake.set(
        &list,
        json!({"total_count": 2, "artifacts": [row.clone(), row]}),
    );
    assert!(select_start_transport(&world.fake).is_err());
}

#[test]
fn select_pagination_counterprobes_and_multi_page_success() {
    let page = |n: usize| format!("{API_PATH}/actions/runs/1000/artifacts?per_page=100&page={n}");
    let build = |world: &World, total: usize, first: usize, second: usize| {
        let target = artifact_row(world);
        let filler = |from: usize, count: usize, with_target: bool| -> Vec<Value> {
            let mut rows: Vec<Value> = (0..count)
                .map(|i| json!({"id": 1000 + from + i, "name": format!("other-{from}-{i}")}))
                .collect();
            if with_target && !rows.is_empty() {
                rows[0] = target.clone();
            }
            rows
        };
        world.fake.set(
            &page(1),
            json!({"total_count": total, "artifacts": filler(0, first, false)}),
        );
        world.fake.set(
            &page(2),
            json!({"total_count": total, "artifacts": filler(100, second, true)}),
        );
    };
    let world = World::new();
    build(&world, 150, 100, 50);
    assert!(select_start_transport(&world.fake).is_ok());
    for (label, total, first, second) in [
        ("short first page", 150, 99, 51),
        ("truncated second page", 150, 100, 49),
        ("overlong second page", 150, 100, 51),
        ("total_count beyond the page bound", 100_000, 100, 50),
    ] {
        let world = World::new();
        build(&world, total, first, second);
        assert!(select_start_transport(&world.fake).is_err(), "{label}");
    }
    let world = World::new();
    build(&world, 150, 100, 50);
    let mut changed = world.fake.gets.borrow()[&page(2)][0].clone();
    changed["total_count"] = 151.into();
    world.fake.set(&page(2), changed);
    assert!(
        select_start_transport(&world.fake).is_err(),
        "total changes between pages"
    );
}

#[test]
fn select_transport_counterprobes() {
    // Downloaded bytes differ from the provider digest.
    let world = World::new();
    world.fake.artifact_zip.borrow_mut()[10] ^= 1;
    assert!(select_start_transport(&world.fake).is_err());
    // Short download.
    let world = World::new();
    world.fake.artifact_zip.borrow_mut().pop();
    assert!(select_start_transport(&world.fake).is_err());
    // The listing changes after the one download (a second artifact appears).
    let world = World::new();
    let list = format!("{API_PATH}/actions/runs/1000/artifacts?per_page=100&page=1");
    let row = artifact_row(&world);
    let mut extra = row.clone();
    extra["id"] = 52.into();
    extra["name"] = "unrelated".into();
    world.fake.change_after(
        &list,
        1,
        json!({"total_count": 2, "artifacts": [row, extra]}),
    );
    assert!(select_start_transport(&world.fake).is_err());
    assert_eq!(world.fake.artifact_downloads.get(), 1, "no second download");
    // The direct metadata changes after the download.
    let world = World::new();
    let direct = format!("{API_PATH}/actions/artifacts/51");
    let mut changed = row;
    changed["size_in_bytes"] = (world.zip.len() + 1).into();
    world.fake.change_after(&direct, 1, changed);
    assert!(select_start_transport(&world.fake).is_err());
    // The event file changes during selection.
    let world = World::new();
    let event_path = world.path("event.json");
    let mut tampered = world.event.clone();
    tampered["sender"] = json!({"login": "x"});
    let tampered = serde_json::to_vec(&tampered).unwrap();
    let list_path = list;
    let (fake, path) = (&world.fake, event_path);
    // Swap the file as soon as the first listing was observed.
    struct Swap<'a> {
        inner: &'a super::testing::Fake,
        path: std::path::PathBuf,
        bytes: Vec<u8>,
        trigger: String,
    }
    impl super::provider::Provider for Swap<'_> {
        fn var(&self, n: &str) -> Option<String> {
            self.inner.var(n)
        }
        fn get(&self, p: &str) -> super::Result<Value> {
            if p == self.trigger {
                std::fs::write(&self.path, &self.bytes).unwrap();
            }
            self.inner.get(p)
        }
        fn committed_file(&self, c: &str, p: &str, l: usize) -> super::Result<Vec<u8>> {
            self.inner.committed_file(c, p, l)
        }
        fn download_artifact(&self, i: u64, l: usize) -> super::Result<Vec<u8>> {
            self.inner.download_artifact(i, l)
        }
        fn download_asset(&self, i: u64, l: usize) -> super::Result<Vec<u8>> {
            self.inner.download_asset(i, l)
        }
        fn dispatch_workflow(
            &self,
            f: &str,
            r: &str,
            i: &[(&str, &str)],
        ) -> super::Result<(u16, Vec<u8>)> {
            self.inner.dispatch_workflow(f, r, i)
        }
        fn oidc_check_id(&self, c: &Value, e: &str) -> super::Result<u64> {
            self.inner.oidc_check_id(c, e)
        }
        fn attest(&self, r: &super::provider::Attestation<'_>) -> super::Result<Value> {
            self.inner.attest(r)
        }
    }
    let swap = Swap {
        inner: fake,
        path,
        bytes: tampered,
        trigger: list_path,
    };
    assert!(select_start_transport(&swap).is_err());
}

#[test]
fn select_zip_counterprobes() {
    let record = &World::new().record_bytes;
    let bundle = b"{\"b\":1}".to_vec();
    let attempt = |members: Vec<(&str, Vec<u8>, bool)>| {
        let world = World::new();
        let borrowed: Vec<(&str, &[u8], bool)> = members
            .iter()
            .map(|(n, d, c)| (*n, d.as_slice(), *c))
            .collect();
        let zip = zip_of(&borrowed);
        let mut row = artifact_row(&world);
        row["size_in_bytes"] = zip.len().into();
        row["digest"] = format!("sha256:{}", sha256_hex(&zip)).into();
        let list = format!("{API_PATH}/actions/runs/1000/artifacts?per_page=100&page=1");
        world
            .fake
            .set(&list, json!({"total_count": 1, "artifacts": [row.clone()]}));
        world
            .fake
            .set(&format!("{API_PATH}/actions/artifacts/51"), row);
        *world.fake.artifact_zip.borrow_mut() = zip;
        select_start_transport(&world.fake)
    };
    let good = || {
        vec![
            (RECORD_BASENAME, record.clone(), true),
            (BUNDLE_BASENAME, bundle.clone(), false),
        ]
    };
    assert!(attempt(good()).is_ok());
    // Extra member, missing member, renamed member, directory-like.
    let mut extra = good();
    extra.push(("extra.txt", b"x".to_vec(), false));
    assert!(attempt(extra).is_err());
    assert!(attempt(vec![(RECORD_BASENAME, record.clone(), true)]).is_err());
    assert!(attempt(vec![
        ("other.json", record.clone(), true),
        (BUNDLE_BASENAME, bundle.clone(), false)
    ])
    .is_err());
    assert!(attempt(vec![
        ("sub/".to_owned().leak(), vec![], false),
        (BUNDLE_BASENAME, bundle.clone(), false)
    ])
    .is_err());
    // Empty and oversize members.
    assert!(attempt(vec![
        (RECORD_BASENAME, vec![], false),
        (BUNDLE_BASENAME, bundle.clone(), false)
    ])
    .is_err());
    assert!(attempt(vec![
        (RECORD_BASENAME, vec![b'a'; 128 * 1024 + 1], true),
        (BUNDLE_BASENAME, bundle.clone(), false)
    ])
    .is_err());
    assert!(attempt(vec![
        (RECORD_BASENAME, record.clone(), true),
        (BUNDLE_BASENAME, vec![b'a'; 8 * 1024 * 1024 + 1], true)
    ])
    .is_err());
}

// ---------------------------------------------------------- verify-claim ---

fn claim_request<'a>(world: &'a World, runtime_host: &'a str) -> ClaimRequest<'a> {
    // The paths are leaked for the lifetime of the test process; tiny and local.
    let leak =
        |name: &str| -> &'a std::path::Path { Box::leak(world.path(name).into_boxed_path()) };
    ClaimRequest {
        record_path: leak(RECORD_BASENAME),
        bundle_path: leak(BUNDLE_BASENAME),
        claim_path: leak(CLAIM_NAME),
        claim_asset_id: 41,
        source_context: &world.source,
        dispatcher_context: &world.dispatcher,
        dispatcher_job: &world.job,
        expected_record_sha256: Box::leak(sha256_hex(&world.record_bytes).into_boxed_str()),
        expected_bundle_sha256: Box::leak(sha256_hex(&world.bundle).into_boxed_str()),
        expected_claim_sha256: Box::leak(sha256_hex(&world.claim).into_boxed_str()),
        runtime_lock: leak("lock.json"),
        runtime_directory: leak("runtime"),
        runtime_interface: leak("interface.json"),
        runtime_host,
    }
}

#[test]
fn verify_claim_authenticates_the_claimed_start() {
    let world = World::new();
    let request = claim_request(&world, "linux-x86_64");
    let record = verify_claimed_start(&world.fake, &request).unwrap();
    assert_eq!(record, world.record);
    let attested = world.fake.attested.borrow();
    assert!(!attested.is_empty());
    assert_eq!(attested[0].0, RECORD_BASENAME);
    assert_eq!(attested[0].1, world.record_bytes);
    assert_eq!(attested[0].2, world.bundle);
    assert_eq!(
        world.fake.asset_downloads.get(),
        2,
        "one numeric readback per candidate phase, never a retry"
    );
    let report = super::verify_claim::report(&record, &request);
    assert!(String::from_utf8(report)
        .unwrap()
        .contains("not-native-or-dispatch-authority"));
}

#[test]
fn verify_claim_input_counterprobes() {
    let world = World::new();
    let base = claim_request(&world, "linux-x86_64");
    let wrong = "0".repeat(64);
    for (label, request) in [
        (
            "record pin",
            ClaimRequest {
                expected_record_sha256: &wrong,
                ..claim_request(&world, "linux-x86_64")
            },
        ),
        (
            "bundle pin",
            ClaimRequest {
                expected_bundle_sha256: &wrong,
                ..claim_request(&world, "linux-x86_64")
            },
        ),
        (
            "claim pin",
            ClaimRequest {
                expected_claim_sha256: &wrong,
                ..claim_request(&world, "linux-x86_64")
            },
        ),
        (
            "asset id zero",
            ClaimRequest {
                claim_asset_id: 0,
                ..claim_request(&world, "linux-x86_64")
            },
        ),
        (
            "asset id other",
            ClaimRequest {
                claim_asset_id: 42,
                ..claim_request(&world, "linux-x86_64")
            },
        ),
        ("host", claim_request(&world, "darwin-arm64")),
        (
            "record path = bundle path",
            ClaimRequest {
                bundle_path: base.record_path,
                ..claim_request(&world, "linux-x86_64")
            },
        ),
        (
            "claim name",
            ClaimRequest {
                claim_path: Box::leak(world.path("lock.json").into_boxed_path()),
                ..claim_request(&world, "linux-x86_64")
            },
        ),
        (
            "lock = interface",
            ClaimRequest {
                runtime_interface: base.runtime_lock,
                ..claim_request(&world, "linux-x86_64")
            },
        ),
    ] {
        assert!(
            verify_claimed_start(&world.fake, &request).is_err(),
            "{label}"
        );
    }
    // A claim for another dispatcher context or job is not the selected claim.
    let mut other = world.dispatcher.clone();
    other["run_id"] = 2001.into();
    let request = ClaimRequest {
        dispatcher_context: &other,
        ..claim_request(&world, "linux-x86_64")
    };
    assert!(verify_claimed_start(&world.fake, &request).is_err());
    let mut other_job = world.job.clone();
    other_job["id"] = 11.into();
    let request = ClaimRequest {
        dispatcher_job: &other_job,
        ..claim_request(&world, "linux-x86_64")
    };
    assert!(verify_claimed_start(&world.fake, &request).is_err());
    // Missing token.
    world.fake.env.borrow_mut().remove("GH_TOKEN");
    assert!(verify_claimed_start(&world.fake, &claim_request(&world, "linux-x86_64")).is_err());
}

#[test]
fn verify_claim_candidate_counterprobes() {
    let release = format!("{API_PATH}/releases/31");
    let by_tag = format!("{API_PATH}/releases/tags/v2.0.0");
    let asset = |world: &World| world.asset_row.clone();
    let edits: [(&str, Box<dyn Fn(&mut Value, &World)>); 12] = [
        ("published", Box::new(|r, _| r["draft"] = false.into())),
        ("immutable", Box::new(|r, _| r["immutable"] = true.into())),
        ("prerelease", Box::new(|r, _| r["prerelease"] = true.into())),
        (
            "other tag",
            Box::new(|r, _| r["tag_name"] = "v9.9.9".into()),
        ),
        (
            "other target",
            Box::new(|r, _| r["target_commitish"] = "f".repeat(40).into()),
        ),
        ("no assets", Box::new(|r, _| r["assets"] = json!([]))),
        (
            "two assets",
            Box::new(|r, w| {
                let mut second = asset(w);
                second["id"] = 42.into();
                r["assets"] = json!([asset(w), second]);
            }),
        ),
        (
            "asset state",
            Box::new(|r, _| r["assets"][0]["state"] = "starter".into()),
        ),
        (
            "asset type",
            Box::new(|r, _| r["assets"][0]["content_type"] = "text/plain".into()),
        ),
        (
            "asset digest",
            Box::new(|r, _| r["assets"][0]["digest"] = format!("sha256:{}", "0".repeat(64)).into()),
        ),
        (
            "asset size",
            Box::new(|r, _| r["assets"][0]["size"] = 1.into()),
        ),
        (
            "asset url",
            Box::new(|r, _| r["assets"][0]["url"] = "https://evil.example/x".into()),
        ),
    ];
    for (label, edit) in edits {
        let world = World::new();
        let mut value = world.fake.gets.borrow()[&release][0].clone();
        edit(&mut value, &world);
        world.fake.set(&release, value);
        assert!(
            verify_claimed_start(&world.fake, &claim_request(&world, "linux-x86_64")).is_err(),
            "{label}"
        );
    }
    // The tag lookup selects another draft.
    let world = World::new();
    world.fake.edit(&by_tag, |v| v["id"] = 32.into());
    assert!(verify_claimed_start(&world.fake, &claim_request(&world, "linux-x86_64")).is_err());
    // Numeric readback differs, or the candidate changes during it.
    let world = World::new();
    world.fake.asset.borrow_mut().push(b' ');
    assert!(verify_claimed_start(&world.fake, &claim_request(&world, "linux-x86_64")).is_err());
    let world = World::new();
    let mut changed = world.fake.gets.borrow()[&release][0].clone();
    changed["assets"] = json!([]);
    world.fake.change_after(&release, 1, changed);
    assert!(verify_claimed_start(&world.fake, &claim_request(&world, "linux-x86_64")).is_err());
    assert_eq!(
        world.fake.asset_downloads.get(),
        1,
        "no retry of the readback"
    );
}

#[test]
fn verify_claim_provider_and_signature_counterprobes() {
    let tag_ref = format!("{API_PATH}/git/ref/tags/v2.0.0");
    let tag_object = format!("{API_PATH}/git/tags/{}", "a".repeat(40));
    let start_jobs = format!("{}/attempts/1/jobs?per_page=100&page=1", runs(1000));
    let attestation_runs = |uri: &str| {
        json!([{"verificationResult": {
        "signature": {"certificate": {"runInvocationURI": uri}},
        "statement": {"subject": [{"name": RECORD_BASENAME, "digest": {"sha256": "x"}}]}}}])
    };
    let probes: Vec<(&str, Box<dyn Fn(&World)>)> = vec![
        (
            "tag ref object",
            Box::new(move |w| {
                w.fake
                    .edit(&tag_ref, |v| v["object"]["sha"] = "b".repeat(40).into());
            }),
        ),
        (
            "tag target",
            Box::new(move |w| {
                w.fake
                    .edit(&tag_object, |v| v["object"]["sha"] = "f".repeat(40).into());
            }),
        ),
        (
            "tag epoch",
            Box::new(|w| {
                w.fake
                    .edit(&format!("{API_PATH}/git/tags/{}", "a".repeat(40)), |v| {
                        v["tagger"]["date"] = "2023-11-14T22:15:01Z".into();
                    });
            }),
        ),
        (
            "tag date format",
            Box::new(|w| {
                w.fake
                    .edit(&format!("{API_PATH}/git/tags/{}", "a".repeat(40)), |v| {
                        v["tagger"]["date"] = "yesterday".into();
                    });
            }),
        ),
        (
            "repository archived",
            Box::new(|w| w.fake.edit(API_PATH, |v| v["archived"] = true.into())),
        ),
        (
            "default branch",
            Box::new(|w| {
                w.fake
                    .edit(API_PATH, |v| v["default_branch"] = "dev".into());
            }),
        ),
        (
            "start rerun",
            Box::new(|w| w.fake.edit(&runs(1000), |v| v["run_attempt"] = 2.into())),
        ),
        (
            "start incomplete",
            Box::new(|w| {
                w.fake.edit(&format!("{}/attempts/1", runs(1000)), |v| {
                    v["status"] = "in_progress".into();
                });
            }),
        ),
        (
            "runner label",
            Box::new(move |w| {
                w.fake.edit(&start_jobs, |v| {
                    v["jobs"][0]["labels"] = json!(["self-hosted"]);
                });
            }),
        ),
        (
            "frozen input",
            Box::new(|w| {
                w.fake
                    .committed
                    .borrow_mut()
                    .insert(format!("{COMMIT}:version.txt"), b"tampered".to_vec());
            }),
        ),
        (
            "frozen input missing",
            Box::new(|w| {
                w.fake
                    .committed
                    .borrow_mut()
                    .remove(&format!("{COMMIT}:CHANGELOG.md"));
            }),
        ),
        (
            "no attestation",
            Box::new(|w| *w.fake.attestation.borrow_mut() = None),
        ),
        (
            "two attestations",
            Box::new(|w| {
                let one = w.fake.attestation.borrow().clone().unwrap();
                let mut two = one.clone();
                two.as_array_mut()
                    .unwrap()
                    .extend(one.as_array().unwrap().clone());
                *w.fake.attestation.borrow_mut() = Some(two);
            }),
        ),
        (
            "attestation not a list",
            Box::new(|w| {
                *w.fake.attestation.borrow_mut() = Some(json!({"verificationResult": {}}));
            }),
        ),
        (
            "another run attempt",
            Box::new(move |w| {
                *w.fake.attestation.borrow_mut() = Some(attestation_runs(
                    "https://github.com/metaneutrons/aros-toolchains/actions/runs/1000/attempts/2",
                ));
            }),
        ),
        (
            "another run",
            Box::new(move |w| {
                *w.fake.attestation.borrow_mut() = Some(attestation_runs(
                    "https://github.com/metaneutrons/aros-toolchains/actions/runs/1001/attempts/1",
                ));
            }),
        ),
        (
            "subject digest",
            Box::new(|w| {
                w.fake.attestation.borrow_mut().as_mut().unwrap()[0]["verificationResult"]
                    ["statement"]["subject"][0]["digest"]["sha256"] = "0".repeat(64).into();
            }),
        ),
        (
            "subject name",
            Box::new(|w| {
                w.fake.attestation.borrow_mut().as_mut().unwrap()[0]["verificationResult"]
                    ["statement"]["subject"][0]["name"] = "other.json".into();
            }),
        ),
        (
            "lock bytes",
            Box::new(|w| std::fs::write(w.path("lock.json"), b"tampered").unwrap()),
        ),
        (
            "receipt bytes",
            Box::new(|w| {
                std::fs::write(
                    w.path("runtime/.aros-tools-runtime-receipt-v1.json"),
                    b"tampered",
                )
                .unwrap();
            }),
        ),
        (
            "interface bytes",
            Box::new(|w| std::fs::write(w.path("interface.json"), b"tampered").unwrap()),
        ),
        (
            "executable bytes",
            Box::new(|w| std::fs::write(w.path("runtime/bin/aros"), b"tampered").unwrap()),
        ),
        (
            "executable missing",
            Box::new(|w| std::fs::remove_file(w.path("runtime/bin/aros")).unwrap()),
        ),
        (
            "unlisted runtime file",
            Box::new(|w| std::fs::write(w.path("runtime/extra"), b"x").unwrap()),
        ),
        (
            "runtime data file changed",
            Box::new(|w| std::fs::write(w.path("runtime/README.md"), b"changed").unwrap()),
        ),
        (
            "receipt of another host",
            Box::new(|w| {
                let path = w.path("runtime/.aros-tools-runtime-receipt-v1.json");
                let mut receipt: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                receipt["host"] = "linux-aarch64".into();
                std::fs::write(path, serde_json::to_vec(&receipt).unwrap()).unwrap();
            }),
        ),
    ];
    for (label, probe) in probes {
        let world = World::new();
        probe(&world);
        let result = verify_claimed_start(&world.fake, &claim_request(&world, "linux-x86_64"));
        assert!(result.is_err(), "{label}");
        assert!(world.fake.asset_downloads.get() <= 2, "{label}: no retry");
    }
    // The start job changing during authentication is refused.
    let world = World::new();
    let jobs = format!("{}/attempts/1/jobs?per_page=100&page=1", runs(1000));
    let mut changed = world.fake.gets.borrow()[&jobs][0].clone();
    changed["jobs"][0]["labels"] = json!(["ubuntu-latest"]);
    world.fake.change_after(&jobs, 1, changed);
    assert!(verify_claimed_start(&world.fake, &claim_request(&world, "linux-x86_64")).is_err());
}

// -------------------------------------------------------------- dispatch ---

const NATIVE_RUN: u64 = 3000;

fn native_list() -> String {
    format!(
        "{API_PATH}/actions/workflows/toolchain-release.yml/runs?event=workflow_dispatch&branch=v2.0.0&per_page=100"
    )
}

fn native_run_row() -> Value {
    json!({"id": NATIVE_RUN, "run_attempt": 1, "event": "workflow_dispatch",
           "head_sha": COMMIT, "head_branch": "v2.0.0",
           "path": ".github/workflows/toolchain-release.yml@refs/tags/v2.0.0",
           "repository": {"id": 4242, "full_name": REPO},
           "head_repository": {"id": 4242, "full_name": REPO}})
}

fn confirmed_answer() -> (u16, Vec<u8>) {
    let body = json!({
        "workflow_run_id": NATIVE_RUN,
        "run_url": format!("https://api.github.com/{API_PATH}/actions/runs/{NATIVE_RUN}"),
        "html_url": format!("https://github.com/{REPO}/actions/runs/{NATIVE_RUN}"),
    });
    (200, serde_json::to_vec(&body).unwrap())
}

/// A world whose provider answers every observation of one clean dispatch.
fn dispatch_world() -> World {
    let world = World::new();
    world.fake.set(
        &native_list(),
        json!({"total_count": 0, "workflow_runs": []}),
    );
    world.fake.set(&runs(NATIVE_RUN), native_run_row());
    *world.fake.dispatch_answer.borrow_mut() = Some(confirmed_answer());
    world
}

struct Fixture {
    claim: Vec<u8>,
    record: Vec<u8>,
    bundle: Vec<u8>,
    asset: Value,
    pin: String,
}

fn fixture(world: &World) -> Fixture {
    Fixture {
        claim: world.claim.clone(),
        record: world.record_bytes.clone(),
        bundle: world.bundle.clone(),
        asset: json!({"id": 41, "name": CLAIM_NAME, "size": world.claim.len(),
                      "sha256": sha256_hex(&world.claim)}),
        pin: sha256_hex(b"sig"),
    }
}

fn request<'a>(world: &'a World, f: &'a Fixture, source: &'a Value) -> dispatch::Request<'a> {
    dispatch::Request {
        claim_raw: &f.claim,
        record_bytes: &f.record,
        start_bundle_bytes: &f.bundle,
        source_context: source,
        dispatcher_context: &world.dispatcher,
        dispatcher_job: &world.job,
        claim_asset: &f.asset,
        claim_bundle_sha256: &f.pin,
    }
}

#[test]
fn dispatch_sends_exactly_one_native_dispatch_and_returns_the_binding_subject() {
    let world = dispatch_world();
    let f = fixture(&world);
    let subject = dispatch::dispatch(&world.fake, &request(&world, &f, &world.source)).unwrap();
    let dispatches = world.fake.dispatches.borrow();
    assert_eq!(dispatches.len(), 1, "one single mutation");
    assert_eq!(dispatches[0].0, "toolchain-release.yml");
    assert_eq!(dispatches[0].1, "refs/tags/v2.0.0");
    assert_eq!(
        dispatches[0].2,
        vec![
            ("mode".to_owned(), "release-v2".to_owned()),
            ("scope".to_owned(), "full".to_owned()),
            ("diagnostic_group".to_owned(), String::new()),
        ]
    );
    // The subject is the canonical binding of the observed native run.
    let value: Value = serde_json::from_slice(&subject).unwrap();
    assert_eq!(value["name"], BINDING_NAME);
    assert_eq!(value["native"]["context"]["run_id"], NATIVE_RUN);
    assert_eq!(value["native"]["event"], "workflow_dispatch");
    assert_eq!(value["native"]["context"]["source_ref"], "refs/tags/v2.0.0");
}

#[test]
fn dispatch_contract_violations_never_reach_the_provider() {
    let world = dispatch_world();
    let good = fixture(&world);
    let mut bad_claim = fixture(&world);
    bad_claim.claim = b"{}\n".to_vec();
    let mut bad_asset = fixture(&world);
    bad_asset.asset["size"] = 1.into();
    let mut renamed_asset = fixture(&world);
    renamed_asset.asset["name"] = "other.json".into();
    let mut bad_pin = fixture(&world);
    bad_pin.pin = "NOT-A-DIGEST".to_owned();
    let mut foreign = world.source.clone();
    foreign["run_id"] = 1001.into();
    for (label, f, source) in [
        ("claim bytes", &bad_claim, &world.source),
        ("asset size", &bad_asset, &world.source),
        ("asset name", &renamed_asset, &world.source),
        ("signature pin", &bad_pin, &world.source),
        ("source context", &good, &foreign),
    ] {
        let error = dispatch::dispatch(&world.fake, &request(&world, f, source)).unwrap_err();
        assert!(!error.message.is_empty(), "{label}");
        assert!(
            world.fake.dispatches.borrow().is_empty(),
            "{label}: no mutation"
        );
        assert!(
            !world
                .fake
                .log
                .borrow()
                .iter()
                .any(|path| path.contains("dispatches")),
            "{label}"
        );
    }
}

#[test]
fn dispatch_refuses_a_second_dispatch_before_any_mutation() {
    for (label, listing) in [
        (
            "existing run",
            json!({"total_count": 1, "workflow_runs": [native_run_row()]}),
        ),
        (
            "uncertain total",
            json!({"total_count": null, "workflow_runs": []}),
        ),
        ("missing runs", json!({"total_count": 0})),
        (
            "count without runs",
            json!({"total_count": 2, "workflow_runs": []}),
        ),
    ] {
        let world = dispatch_world();
        world.fake.set(&native_list(), listing);
        let f = fixture(&world);
        let error =
            dispatch::dispatch(&world.fake, &request(&world, &f, &world.source)).unwrap_err();
        assert_eq!(error.kind, Kind::Integrity, "{label}");
        assert!(
            world.fake.dispatches.borrow().is_empty(),
            "{label}: nothing dispatched"
        );
    }
    // An unscripted listing is not absence.
    let world = dispatch_world();
    world.fake.gets.borrow_mut().remove(&native_list());
    let f = fixture(&world);
    assert!(dispatch::dispatch(&world.fake, &request(&world, &f, &world.source)).is_err());
    assert!(world.fake.dispatches.borrow().is_empty());
}

#[test]
fn dispatch_answers_other_than_a_confirmed_run_stop_after_one_attempt() {
    let url = format!("https://api.github.com/{API_PATH}/actions/runs/{NATIVE_RUN}");
    let html = format!("https://github.com/{REPO}/actions/runs/{NATIVE_RUN}");
    let answers: Vec<(&str, Option<(u16, Vec<u8>)>)> = vec![
        ("no content", Some((204, Vec::new()))),
        ("unprocessable", Some((422, b"{}".to_vec()))),
        ("server error", Some((500, b"{}".to_vec()))),
        ("redirect", Some((302, Vec::new()))),
        ("empty 200", Some((200, Vec::new()))),
        ("not json", Some((200, b"not json".to_vec()))),
        (
            "extra field",
            Some((
                200,
                serde_json::to_vec(&json!({"workflow_run_id": NATIVE_RUN,
                "run_url": url, "html_url": html, "extra": 1}))
                .unwrap(),
            )),
        ),
        (
            "other run url",
            Some((
                200,
                serde_json::to_vec(&json!({"workflow_run_id": NATIVE_RUN,
                "run_url": url.replace("3000", "3001"), "html_url": html}))
                .unwrap(),
            )),
        ),
        (
            "boolean id",
            Some((
                200,
                serde_json::to_vec(&json!({"workflow_run_id": true,
                "run_url": url, "html_url": html}))
                .unwrap(),
            )),
        ),
        ("unbounded", Some((200, vec![b' '; 5000]))),
        ("transport failure", None),
    ];
    for (label, answer) in answers {
        let world = dispatch_world();
        *world.fake.dispatch_answer.borrow_mut() = answer;
        let f = fixture(&world);
        assert!(
            dispatch::dispatch(&world.fake, &request(&world, &f, &world.source)).is_err(),
            "{label}"
        );
        assert_eq!(
            world.fake.dispatches.borrow().len(),
            1,
            "{label}: exactly one attempt"
        );
        let reads = world
            .fake
            .log
            .borrow()
            .iter()
            .filter(|p| p.ends_with(&format!("runs/{NATIVE_RUN}")))
            .count();
        assert_eq!(reads, 0, "{label}: no search for a convenient run");
    }
}

#[test]
fn dispatch_requires_the_named_run_to_be_the_claims_native_run() {
    let probes: Vec<(&str, Box<dyn Fn(&mut Value)>)> = vec![
        ("other run id", Box::new(|r| r["id"] = 3001.into())),
        ("rerun", Box::new(|r| r["run_attempt"] = 2.into())),
        ("push event", Box::new(|r| r["event"] = "push".into())),
        (
            "other commit",
            Box::new(|r| r["head_sha"] = "f".repeat(40).into()),
        ),
        ("branch run", Box::new(|r| r["head_branch"] = "main".into())),
        (
            "other workflow",
            Box::new(|r| r["path"] = ".github/workflows/other.yml".into()),
        ),
        (
            "other ref in path",
            Box::new(|r| {
                r["path"] = ".github/workflows/toolchain-release.yml@refs/heads/main".into();
            }),
        ),
        (
            "other repository",
            Box::new(|r| r["repository"]["full_name"] = "a/b".into()),
        ),
        (
            "other repository id",
            Box::new(|r| r["repository"]["id"] = 1.into()),
        ),
        (
            "fork head",
            Box::new(|r| r["head_repository"]["id"] = 2.into()),
        ),
    ];
    for (label, mutate) in probes {
        let world = dispatch_world();
        let mut row = native_run_row();
        mutate(&mut row);
        world.fake.set(&runs(NATIVE_RUN), row);
        let f = fixture(&world);
        assert!(
            dispatch::dispatch(&world.fake, &request(&world, &f, &world.source)).is_err(),
            "{label}"
        );
        assert_eq!(
            world.fake.dispatches.borrow().len(),
            1,
            "{label}: never retried"
        );
    }
    // The run changing between the readback and the returned subject is refused.
    let world = dispatch_world();
    let mut changed = native_run_row();
    changed["head_sha"] = "e".repeat(40).into();
    world.fake.change_after(&runs(NATIVE_RUN), 1, changed);
    let f = fixture(&world);
    assert!(dispatch::dispatch(&world.fake, &request(&world, &f, &world.source)).is_err());
    assert_eq!(world.fake.dispatches.borrow().len(), 1);
}

// -------------------------------------------------------- verify-binding ---

struct Bound {
    world: World,
    binding: Vec<u8>,
    signature: Vec<u8>,
    native: Value,
}

fn bound() -> Bound {
    let world = World::new();
    // The binding is verified after the dispatcher run completed successfully.
    let done = |path: String, world: &World| {
        world.fake.edit(&path, |v| {
            v["status"] = "completed".into();
            v["conclusion"] = "success".into();
        });
    };
    done(format!("{}/attempts/1", runs(2000)), &world);
    done(runs(2000), &world);
    world.fake.edit(
        &format!("{}/attempts/1/jobs?per_page=100&page=1", runs(2000)),
        |v| {
            v["jobs"][0]["status"] = "completed".into();
            v["jobs"][0]["conclusion"] = "success".into();
        },
    );
    let pin = sha256_hex(b"claim-signature");
    let native = json!({"repository": REPO, "repository_id": 4242, "producer_commit": COMMIT,
        "source_ref": "refs/tags/v2.0.0", "workflow": super::claim::NATIVE_WORKFLOW, "run_id": 3000, "run_attempt": 1});
    let response = serde_json::to_vec(&json!({"workflow_run_id": 3000,
        "run_url": format!("https://api.github.com/repos/{REPO}/actions/runs/3000"),
        "html_url": format!("https://github.com/{REPO}/actions/runs/3000")}))
    .unwrap();
    let selection = Selection {
        claim_raw: &world.claim,
        record_bytes: &world.record_bytes,
        start_bundle_bytes: &world.bundle,
        source_context: &world.source,
        dispatcher: Dispatcher {
            context: &world.dispatcher,
            job: &world.job,
            event: "workflow_run",
            trigger_context: &world.source,
        },
        claim_asset: &json!({"id": 41, "name": CLAIM_NAME, "size": world.claim.len(), "sha256": sha256_hex(&world.claim)}),
        expected_claim_bundle_sha256: &pin,
    };
    let binding = binding_bytes(&selection, 200, &response).unwrap();
    let signature = b"{\"binding\":\"signature\"}".to_vec();
    std::fs::write(world.path(BINDING_NAME), &binding).unwrap();
    std::fs::write(world.path("binding-signature.json"), &signature).unwrap();
    *world.fake.attestation.borrow_mut() = Some(json!([{"verificationResult": {
        "signature": {"certificate": {"runInvocationURI": format!("https://github.com/{REPO}/actions/runs/2000/attempts/1")}},
        "statement": {"subject": [{"name": BINDING_NAME, "digest": {"sha256": sha256_hex(&binding)}}]}}}]));
    Bound {
        world,
        binding,
        signature,
        native,
    }
}

fn binding_request<'a>(b: &'a Bound, native_event: &'a str) -> BindingRequest<'a> {
    let leak =
        |name: &str| -> &'a std::path::Path { Box::leak(b.world.path(name).into_boxed_path()) };
    BindingRequest {
        binding_path: leak(BINDING_NAME),
        signature_path: leak("binding-signature.json"),
        expected_binding_sha256: Box::leak(sha256_hex(&b.binding).into_boxed_str()),
        expected_signature_sha256: Box::leak(sha256_hex(&b.signature).into_boxed_str()),
        claim_raw: &b.world.claim,
        record_bytes: &b.world.record_bytes,
        start_bundle_bytes: &b.world.bundle,
        source_context: &b.world.source,
        dispatcher_context: &b.world.dispatcher,
        dispatcher_job: &b.world.job,
        claim_asset: Box::leak(Box::new(
            json!({"id": 41, "name": CLAIM_NAME, "size": b.world.claim.len(), "sha256": sha256_hex(&b.world.claim)}),
        )),
        expected_claim_bundle_sha256: Box::leak(sha256_hex(b"claim-signature").into_boxed_str()),
        actual_native_context: &b.native,
        actual_native_event: native_event,
    }
}

#[test]
fn verify_binding_authenticates_the_original_dispatcher_signature() {
    let b = bound();
    let result =
        verify_binding_signature(&b.world.fake, &binding_request(&b, "workflow_dispatch")).unwrap();
    assert_eq!(result["assurance"], super::verify_binding::ASSURANCE);
    assert_eq!(result["binding"]["native"]["context"]["run_id"], 3000);
    assert_eq!(result["limitations"].as_array().unwrap().len(), 3);
    let attested = b.world.fake.attested.borrow();
    assert_eq!(attested[0].0, BINDING_NAME);
    assert_eq!(attested[0].1, b.binding);
}

#[test]
fn verify_binding_counterprobes() {
    let wrong = "0".repeat(64);
    let probes: Vec<(&str, Box<dyn Fn(&Bound)>)> = vec![
        (
            "no attestation",
            Box::new(|b| *b.world.fake.attestation.borrow_mut() = None),
        ),
        (
            "another dispatcher attempt",
            Box::new(|b| {
                b.world.fake.attestation.borrow_mut().as_mut().unwrap()[0]["verificationResult"]
                    ["signature"]["certificate"]["runInvocationURI"] =
                    format!("https://github.com/{REPO}/actions/runs/2000/attempts/2").into();
            }),
        ),
        (
            "start run signature",
            Box::new(|b| {
                b.world.fake.attestation.borrow_mut().as_mut().unwrap()[0]["verificationResult"]
                    ["signature"]["certificate"]["runInvocationURI"] =
                    format!("https://github.com/{REPO}/actions/runs/1000/attempts/1").into();
            }),
        ),
        (
            "subject digest",
            Box::new(|b| {
                b.world.fake.attestation.borrow_mut().as_mut().unwrap()[0]["verificationResult"]
                    ["statement"]["subject"][0]["digest"]["sha256"] = "0".repeat(64).into();
            }),
        ),
        (
            "subject name",
            Box::new(|b| {
                b.world.fake.attestation.borrow_mut().as_mut().unwrap()[0]["verificationResult"]
                    ["statement"]["subject"][0]["name"] = "x.json".into();
            }),
        ),
        (
            "dispatcher job differs",
            Box::new(|b| {
                b.world.fake.edit(
                    &format!("{}/attempts/1/jobs?per_page=100&page=1", runs(2000)),
                    |v| v["jobs"][0]["name"] = "other".into(),
                );
            }),
        ),
        (
            "dispatcher job not completed",
            Box::new(|b| {
                b.world.fake.edit(
                    &format!("{}/attempts/1/jobs?per_page=100&page=1", runs(2000)),
                    |v| v["jobs"][0]["conclusion"] = "failure".into(),
                );
            }),
        ),
        (
            "dispatcher run failed",
            Box::new(|b| {
                b.world
                    .fake
                    .edit(&runs(2000), |v| v["conclusion"] = "failure".into());
            }),
        ),
        (
            "dispatcher run rerun",
            Box::new(|b| {
                b.world
                    .fake
                    .edit(&runs(2000), |v| v["run_attempt"] = 2.into());
            }),
        ),
        (
            "signature bytes",
            Box::new(|b| {
                std::fs::write(b.world.path("binding-signature.json"), b"tampered").unwrap();
            }),
        ),
        (
            "binding bytes",
            Box::new(|b| {
                let mut raw = b.binding.clone();
                raw[4] ^= 1;
                std::fs::write(b.world.path(BINDING_NAME), raw).unwrap();
            }),
        ),
    ];
    for (label, probe) in probes {
        let b = bound();
        probe(&b);
        assert!(
            verify_binding_signature(&b.world.fake, &binding_request(&b, "workflow_dispatch"))
                .is_err(),
            "{label}"
        );
    }
    let b = bound();
    for (label, request) in [
        ("event", binding_request(&b, "push")),
        ("event workflow_run", binding_request(&b, "workflow_run")),
        (
            "binding pin",
            BindingRequest {
                expected_binding_sha256: &wrong,
                ..binding_request(&b, "workflow_dispatch")
            },
        ),
        (
            "signature pin",
            BindingRequest {
                expected_signature_sha256: &wrong,
                ..binding_request(&b, "workflow_dispatch")
            },
        ),
        (
            "claim signature pin",
            BindingRequest {
                expected_claim_bundle_sha256: &wrong,
                ..binding_request(&b, "workflow_dispatch")
            },
        ),
        (
            "signature = binding path",
            BindingRequest {
                signature_path: binding_request(&b, "x").binding_path,
                ..binding_request(&b, "workflow_dispatch")
            },
        ),
        (
            "binding name",
            BindingRequest {
                binding_path: Box::leak(b.world.path("binding-signature.json").into_boxed_path()),
                ..binding_request(&b, "workflow_dispatch")
            },
        ),
    ] {
        assert!(
            verify_binding_signature(&b.world.fake, &request).is_err(),
            "{label}"
        );
    }
    // The actual native context is independent: a copied binding from another run fails.
    for run in [3001_u64, 1000, 2000] {
        let mut other = b.native.clone();
        other["run_id"] = run.into();
        let request = BindingRequest {
            actual_native_context: Box::leak(Box::new(other)),
            ..binding_request(&b, "workflow_dispatch")
        };
        assert!(
            verify_binding_signature(&b.world.fake, &request).is_err(),
            "native run {run}"
        );
    }
    // No token, no verification.
    b.world.fake.env.borrow_mut().remove("GH_TOKEN");
    assert!(
        verify_binding_signature(&b.world.fake, &binding_request(&b, "workflow_dispatch")).is_err()
    );
}
