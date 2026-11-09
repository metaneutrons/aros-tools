//! Positive probe and counter-probes of the one-shot claim upload.
#![allow(
    clippy::type_complexity,
    clippy::too_many_lines,
    reason = "one flat probe table per phase keeps the counter-probes readable"
)]

use serde_json::{json, Value};

use super::claim::{sha256_hex, CLAIM_NAME};
use super::claim_upload::{upload_claim, Request};
use super::testing::{World, API_PATH};
use super::zip_reader::{BUNDLE_BASENAME, RECORD_BASENAME};

fn release_path() -> String {
    format!("{API_PATH}/releases/31")
}

fn release_row(world: &World, assets: &[Value]) -> Value {
    json!({"id": 31, "tag_name": "v2.0.0", "target_commitish": super::testing::COMMIT,
           "draft": true, "prerelease": false, "immutable": false, "assets": assets,
           "name": world.job["name"]})
}

/// An unclaimed world: the draft is empty until the upload, then holds the claim.
fn unclaimed() -> World {
    let world = World::new();
    let empty = release_row(&world, &[]);
    world.fake.set(&release_path(), empty);
    // verify_start observes the candidate twice, the readback once more.
    world.fake.change_after(
        &release_path(),
        2,
        release_row(&world, std::slice::from_ref(&world.asset_row)),
    );
    *world.fake.upload_answer.borrow_mut() =
        Some((201, serde_json::to_vec(&world.asset_row).unwrap()));
    world
}

fn request(world: &World) -> Request<'_> {
    let leak =
        |name: &str| -> &'static std::path::Path { Box::leak(world.path(name).into_boxed_path()) };
    Request {
        record_path: leak(RECORD_BASENAME),
        bundle_path: leak(BUNDLE_BASENAME),
        source_context: &world.source,
        dispatcher_context: &world.dispatcher,
        dispatcher_job: &world.job,
        expected_record_sha256: Box::leak(sha256_hex(&world.record_bytes).into_boxed_str()),
        expected_bundle_sha256: Box::leak(sha256_hex(&world.bundle).into_boxed_str()),
        runtime_lock: leak("lock.json"),
        runtime_directory: leak("runtime"),
        runtime_interface: leak("interface.json"),
        runtime_host: "linux-x86_64",
    }
}

#[test]
fn uploads_the_canonical_claim_exactly_once_and_selects_the_asset() {
    let world = unclaimed();
    let uploaded = upload_claim(&world.fake, &request(&world)).expect("clean claim upload");
    assert_eq!(uploaded.claim, world.claim, "the canonical claim bytes");
    assert_eq!(uploaded.asset["id"], 41);
    assert_eq!(uploaded.asset["name"], CLAIM_NAME);
    assert_eq!(uploaded.asset["sha256"], sha256_hex(&world.claim));
    let uploads = world.fake.uploads.borrow();
    assert_eq!(uploads.len(), 1, "one single mutation");
    assert_eq!(uploads[0].0, 31);
    assert_eq!(uploads[0].1, CLAIM_NAME);
    assert_eq!(uploads[0].2, world.claim);
}

#[test]
fn refuses_every_candidate_that_is_not_the_exact_empty_private_draft() {
    let probes: Vec<(&str, Box<dyn Fn(&World) -> Value>)> = vec![
        (
            "claim already present",
            Box::new(|w| release_row(w, std::slice::from_ref(&w.asset_row))),
        ),
        (
            "foreign asset present",
            Box::new(|w| release_row(w, &[json!({"id": 9, "name": "other.json"})])),
        ),
        (
            "public release",
            Box::new(|w| {
                let mut row = release_row(w, &[]);
                row["draft"] = false.into();
                row
            }),
        ),
        (
            "prerelease",
            Box::new(|w| {
                let mut row = release_row(w, &[]);
                row["prerelease"] = true.into();
                row
            }),
        ),
        (
            "immutable",
            Box::new(|w| {
                let mut row = release_row(w, &[]);
                row["immutable"] = true.into();
                row
            }),
        ),
        (
            "other tag",
            Box::new(|w| {
                let mut row = release_row(w, &[]);
                row["tag_name"] = "v9.9.9".into();
                row
            }),
        ),
        (
            "other commit",
            Box::new(|w| {
                let mut row = release_row(w, &[]);
                row["target_commitish"] = "e".repeat(40).into();
                row
            }),
        ),
        (
            "other draft id",
            Box::new(|w| {
                let mut row = release_row(w, &[]);
                row["id"] = 32.into();
                row
            }),
        ),
        (
            "unlisted assets",
            Box::new(|w| {
                let mut row = release_row(w, &[]);
                row.as_object_mut().unwrap().remove("assets");
                row
            }),
        ),
    ];
    for (label, build) in probes {
        let world = unclaimed();
        let row = build(&world);
        world.fake.set(&release_path(), row);
        assert!(
            upload_claim(&world.fake, &request(&world)).is_err(),
            "{label}"
        );
        assert!(world.fake.uploads.borrow().is_empty(), "{label}: no upload");
    }
}

#[test]
fn refuses_unselected_or_changed_start_inputs_before_any_upload() {
    let world = unclaimed();
    let mut wrong_record = request(&world);
    wrong_record.expected_record_sha256 = Box::leak(sha256_hex(b"other").into_boxed_str());
    let mut wrong_bundle = request(&world);
    wrong_bundle.expected_bundle_sha256 = Box::leak(sha256_hex(b"other").into_boxed_str());
    let mut malformed_pin = request(&world);
    malformed_pin.expected_record_sha256 = "NOT-A-DIGEST";
    let mut foreign = world.source.clone();
    foreign["run_id"] = 1001.into();
    let mut foreign_request = request(&world);
    foreign_request.source_context = &foreign;
    let mut wrong_host = request(&world);
    wrong_host.runtime_host = "macos-aarch64";
    for (label, request) in [
        ("record pin", wrong_record),
        ("bundle pin", wrong_bundle),
        ("malformed pin", malformed_pin),
        ("source context", foreign_request),
        ("runtime host", wrong_host),
    ] {
        assert!(upload_claim(&world.fake, &request).is_err(), "{label}");
        assert!(world.fake.uploads.borrow().is_empty(), "{label}: no upload");
    }
    // Without the job token nothing is observed or uploaded.
    let world = unclaimed();
    world.fake.env.borrow_mut().remove("GH_TOKEN");
    assert!(upload_claim(&world.fake, &request(&world)).is_err());
    assert!(world.fake.uploads.borrow().is_empty());
    // A changed runtime closure is refused before any upload.
    let world = unclaimed();
    std::fs::write(world.path("runtime/bin/aros"), b"tampered").unwrap();
    assert!(upload_claim(&world.fake, &request(&world)).is_err());
    assert!(world.fake.uploads.borrow().is_empty());
}

#[test]
fn upload_answers_other_than_a_matching_201_stop_after_one_attempt() {
    let row = |mutate: &dyn Fn(&mut Value)| {
        let mut value = World::new().asset_row;
        mutate(&mut value);
        Some((201, serde_json::to_vec(&value).unwrap()))
    };
    let answers: Vec<(&str, Option<(u16, Vec<u8>)>)> = vec![
        ("ok instead of created", Some((200, b"{}".to_vec()))),
        ("no content", Some((204, Vec::new()))),
        ("conflict", Some((409, b"{}".to_vec()))),
        ("unprocessable", Some((422, b"{}".to_vec()))),
        ("server error", Some((500, b"{}".to_vec()))),
        ("not json", Some((201, b"nope".to_vec()))),
        ("other name", row(&|v| v["name"] = "x.json".into())),
        ("not uploaded", row(&|v| v["state"] = "starter".into())),
        (
            "other content type",
            row(&|v| v["content_type"] = "text/plain".into()),
        ),
        ("other size", row(&|v| v["size"] = 1.into())),
        (
            "other digest",
            row(&|v| v["digest"] = format!("sha256:{}", "0".repeat(64)).into()),
        ),
        (
            "other url",
            row(&|v| v["url"] = "https://example.org/asset".into()),
        ),
        ("boolean id", row(&|v| v["id"] = true.into())),
        ("transport failure", None),
    ];
    for (label, answer) in answers {
        let world = unclaimed();
        *world.fake.upload_answer.borrow_mut() = answer;
        assert!(
            upload_claim(&world.fake, &request(&world)).is_err(),
            "{label}"
        );
        assert_eq!(
            world.fake.uploads.borrow().len(),
            1,
            "{label}: exactly one attempt"
        );
    }
}

#[test]
fn the_uploaded_asset_must_read_back_and_close_the_draft() {
    // Bytes behind the numeric ID differ from the uploaded claim.
    let world = unclaimed();
    *world.fake.asset.borrow_mut() = b"other bytes".to_vec();
    assert!(upload_claim(&world.fake, &request(&world)).is_err());
    assert_eq!(world.fake.uploads.borrow().len(), 1, "never retried");
    // The draft holds a second asset after the upload.
    let world = unclaimed();
    world.fake.change_after(
        &release_path(),
        2,
        release_row(
            &world,
            &[
                world.asset_row.clone(),
                json!({"id": 9, "name": "other.json"}),
            ],
        ),
    );
    assert!(upload_claim(&world.fake, &request(&world)).is_err());
    // The draft holds another asset ID than the uploaded one.
    let world = unclaimed();
    let mut other = world.asset_row.clone();
    other["id"] = 42.into();
    other["url"] = format!("https://api.github.com/{API_PATH}/releases/assets/42").into();
    world
        .fake
        .change_after(&release_path(), 2, release_row(&world, &[other]));
    assert!(upload_claim(&world.fake, &request(&world)).is_err());
}
