//! Production conversion counter-probes sharing the raw snapshot fixtures.

use super::*;
use aros_toolchain::snapshot::LegacySourceView;
use std::collections::BTreeSet;

fn snapshot<'a>(fixture: &Fixture, run: &'a RunDirectories) -> SourceSnapshot<'a> {
    SourceSnapshot::prepare(
        run,
        SourceRole::Source,
        &fixture.recipe(),
        TIMEOUT,
        &fixture.token,
    )
    .unwrap()
}

#[test]
fn exact_shallow_identity_passes_legacy_queries_without_history_or_config() {
    let fixture = Fixture::new();
    let original = &fixture.request.source_dir;
    let previous = git(original, &["rev-parse", "HEAD"]);
    fs::write(original.join("empty"), b"selected material\n").unwrap();
    fixture.commit_source();
    git(
        original,
        &[
            "config",
            "remote.origin.url",
            "https://fixture.invalid/not-copied",
        ],
    );
    git(original, &["config", "filter.fixture.clean", "false"]);
    fs::write(
        original.join(".git/private-fixture"),
        b"not source material",
    )
    .unwrap();
    let run = fixture.reserve();
    let raw = snapshot(&fixture, &run);
    let old_root = raw.root().to_owned();
    let view = LegacySourceView::prepare(raw, TIMEOUT, &fixture.token).unwrap();
    assert_eq!(view.root(), fixture.work().join("source-legacy"));
    assert!(!old_root.exists());
    assert!(!fixture.work().join(".source-legacy-pending").exists());
    assert!(!view.root().join(".git/private-fixture").exists());
    assert!(!view.root().join(".git/hooks").exists());
    assert!(!view.root().join(".git/objects/info/alternates").exists());
    let config = fs::read_to_string(view.root().join(".git/config")).unwrap();
    assert!(!config.contains("fixture.invalid") && !config.contains("filter"));
    let objects: BTreeSet<_> = git(
        view.root(),
        &[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname)",
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect();
    assert!(!objects.contains(&previous));
    for query in ["HEAD", "HEAD^{tree}"] {
        assert_eq!(
            git(view.root(), &["rev-parse", query]),
            git(original, &["rev-parse", query])
        );
    }
    // All regular material, including metadata, is independently owned.
    assert_eq!(fs::metadata(view.root().join("empty")).unwrap().nlink(), 1);
    fs::write(original.join("empty"), b"original changed\n").unwrap();
    assert_eq!(
        fs::read(view.root().join("empty")).unwrap(),
        b"selected material\n"
    );
    view.revalidate(TIMEOUT, &fixture.token).unwrap();
    fs::write(view.root().join("empty"), b"changed view\n").unwrap();
    assert!(view.revalidate(TIMEOUT, &fixture.token).is_err());
}

#[test]
fn all_roles_use_distinct_owned_legacy_views() {
    let fixture = Fixture::new();
    let recipe = fixture.recipe();
    let run = fixture.reserve();
    for (role, name) in [
        (SourceRole::Source, "source"),
        (SourceRole::Producer, "producer"),
        (SourceRole::Tools, "tools"),
    ] {
        let raw = SourceSnapshot::prepare(&run, role, &recipe, TIMEOUT, &fixture.token).unwrap();
        let view = LegacySourceView::prepare(raw, TIMEOUT, &fixture.token).unwrap();
        assert_eq!(view.root(), fixture.work().join(format!("{name}-legacy")));
        view.revalidate(TIMEOUT, &fixture.token).unwrap();
    }
}

#[test]
fn multiple_strict_packs_preserve_large_objects_and_descendant_tree_order() {
    let fixture = Fixture::new();
    let bytes = vec![0xa5; 9 * 1024 * 1024];
    let original = &fixture.request.source_dir;
    fs::write(original.join("large"), &bytes).unwrap();
    fs::create_dir(original.join("dir/descendant")).unwrap();
    // Identical blobs at different paths must be transferred once, yet remain
    // present at both paths in the independently checked tree/material graph.
    fs::write(original.join("dir/descendant/duplicate"), &bytes).unwrap();
    fixture.commit_source();
    let run = fixture.reserve();
    let view =
        LegacySourceView::prepare(snapshot(&fixture, &run), TIMEOUT, &fixture.token).unwrap();
    let packs = fs::read_dir(view.root().join(".git/objects/pack"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "pack")
        })
        .count();
    assert!(
        packs >= 2,
        "fixture must exercise independently strict import batches"
    );
    assert_eq!(fs::read(view.root().join("large")).unwrap(), bytes);
    assert_eq!(
        fs::read(view.root().join("dir/descendant/duplicate")).unwrap(),
        bytes
    );
    view.revalidate(TIMEOUT, &fixture.token).unwrap();
}

#[test]
fn recursive_gitlinks_keep_independent_identities_and_raw_bytes() {
    let fixture = Fixture::new();
    let original = &fixture.request.source_dir;
    git(
        original,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            fixture.request.tools_dir.to_str().unwrap(),
            "module",
        ],
    );
    let module = original.join("module");
    git(
        &module,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            fixture.request.producer_dir.to_str().unwrap(),
            "nested",
        ],
    );
    git(&module, &["commit", "-qam", "test: nested module"]);
    symlink("module/nested/dir", original.join("shortcut")).unwrap();
    fixture.commit_source();
    let run = fixture.reserve();
    let raw = snapshot(&fixture, &run);
    // The view must use committed objects, not these changed original bytes.
    fs::write(module.join("nested/empty"), b"changed original").unwrap();
    let view = LegacySourceView::prepare(raw, TIMEOUT, &fixture.token).unwrap();
    for relative in ["", "module", "module/nested"] {
        let isolated = view.root().join(relative);
        assert!(isolated.join(".git").is_dir());
        assert_eq!(
            git(&isolated, &["rev-parse", "HEAD"]),
            git(&original.join(relative), &["rev-parse", "HEAD"])
        );
    }
    assert_eq!(
        fs::read(view.root().join("shortcut/Größe file")).unwrap(),
        b"committed\0bytes\n"
    );
    assert!(fs::read(view.root().join("module/nested/empty"))
        .unwrap()
        .is_empty());
    view.revalidate(TIMEOUT, &fixture.token).unwrap();
    fs::write(
        view.root().join("module/nested/empty"),
        b"changed isolated child",
    )
    .unwrap();
    assert!(view.revalidate(TIMEOUT, &fixture.token).is_err());
}

#[test]
fn metadata_bytes_membership_links_and_kinds_are_never_exempted() {
    for mutation in [
        "config",
        "head",
        "shallow",
        "index",
        "pack",
        "extra",
        "metadata-link",
        "hardlink",
        "fifo",
        "directory",
    ] {
        let fixture = Fixture::new();
        let run = fixture.reserve();
        let view =
            LegacySourceView::prepare(snapshot(&fixture, &run), TIMEOUT, &fixture.token).unwrap();
        let metadata = view.root().join(".git");
        match mutation {
            "config" => fs::write(
                metadata.join("config"),
                b"[include]\npath=/private/not-read\n",
            )
            .unwrap(),
            "head" => fs::write(metadata.join("HEAD"), b"ref: refs/heads/foreign\n").unwrap(),
            "shallow" => fs::write(metadata.join("shallow"), b"\n").unwrap(),
            "index" => fs::write(metadata.join("index"), b"changed index\n").unwrap(),
            "pack" => {
                let pack = fs::read_dir(metadata.join("objects/pack"))
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| path.extension().is_some_and(|ext| ext == "pack"))
                    .unwrap();
                fs::set_permissions(&pack, fs::Permissions::from_mode(0o600)).unwrap();
                fs::write(pack, b"corrupt pack").unwrap();
            }
            "extra" => fs::write(metadata.join("private-extra"), b"not admitted").unwrap(),
            "metadata-link" => {
                fs::remove_file(metadata.join("config")).unwrap();
                symlink("HEAD", metadata.join("config")).unwrap();
            }
            "hardlink" => {
                fs::hard_link(metadata.join("config"), fixture.work().join("alias")).unwrap();
            }
            "fifo" => {
                fs::remove_file(metadata.join("config")).unwrap();
                assert!(Command::new("mkfifo")
                    .arg(metadata.join("config"))
                    .status()
                    .unwrap()
                    .success());
            }
            "directory" => {
                fs::create_dir(view.root().join("extra")).unwrap();
                fs::create_dir(view.root().join("extra/.git")).unwrap();
            }
            _ => unreachable!(),
        }
        let error = view
            .revalidate(TIMEOUT, &fixture.token)
            .expect_err(mutation);
        assert!(!error.to_string().contains("/private/not-read"));
        assert!(!error.to_string().contains("private-extra"));
    }
}

#[test]
fn occupied_destinations_and_cancelled_conversion_never_adopt_or_remove_material() {
    for reason in ["pending", "destination", "cancelled", "zero", "changed-raw"] {
        let fixture = Fixture::new();
        let run = fixture.reserve();
        let raw = snapshot(&fixture, &run);
        let original = raw.root().to_owned();
        match reason {
            "pending" => {
                fs::write(fixture.work().join(".source-legacy-pending"), b"foreign").unwrap();
            }
            "destination" => symlink("source", fixture.work().join("source-legacy")).unwrap(),
            "cancelled" => fixture.token.cancel(),
            "changed-raw" => fs::write(raw.root().join("empty"), b"changed raw").unwrap(),
            "zero" => (),
            _ => unreachable!(),
        }
        let timeout = if reason == "zero" {
            Duration::ZERO
        } else {
            TIMEOUT
        };
        assert!(
            LegacySourceView::prepare(raw, timeout, &fixture.token).is_err(),
            "{reason}"
        );
        assert!(original.is_dir());
        if reason == "pending" {
            assert_eq!(
                fs::read(fixture.work().join(".source-legacy-pending")).unwrap(),
                b"foreign"
            );
        }
        if reason == "destination" {
            assert!(fs::symlink_metadata(fixture.work().join("source-legacy"))
                .unwrap()
                .is_symlink());
        }
    }
}

#[test]
fn missing_or_poisoned_original_objects_retain_failed_staging_without_a_view() {
    for poison in [false, true] {
        let fixture = Fixture::new();
        let original = &fixture.request.source_dir;
        let run = fixture.reserve();
        let raw = snapshot(&fixture, &run);
        let oid = git(original, &["rev-parse", "HEAD:empty"]);
        let object = original
            .join(".git/objects")
            .join(&oid[..2])
            .join(&oid[2..]);
        if poison {
            let other = git(original, &["rev-parse", "HEAD:run"]);
            fs::set_permissions(&object, fs::Permissions::from_mode(0o600)).unwrap();
            fs::copy(
                original
                    .join(".git/objects")
                    .join(&other[..2])
                    .join(&other[2..]),
                &object,
            )
            .unwrap();
        } else {
            fs::remove_file(&object).unwrap();
        }
        assert!(LegacySourceView::prepare(raw, TIMEOUT, &fixture.token).is_err());
        assert!(fixture.work().join(".source-legacy-pending").is_dir());
        assert!(!fixture.work().join("source-legacy").exists());
    }
}

#[test]
fn replacing_a_view_root_is_not_an_equivalent_guard() {
    let fixture = Fixture::new();
    let run = fixture.reserve();
    let view =
        LegacySourceView::prepare(snapshot(&fixture, &run), TIMEOUT, &fixture.token).unwrap();
    fs::rename(view.root(), fixture.work().join("retained-view")).unwrap();
    fs::create_dir(view.root()).unwrap();
    assert!(view.revalidate(TIMEOUT, &fixture.token).is_err());
    assert!(fixture.work().join("retained-view/.git").is_dir());
}

#[test]
fn restored_object_storage_cannot_authorize_previously_poisoned_raw_material() {
    let fixture = Fixture::new();
    let original = &fixture.request.source_dir;
    let run = fixture.reserve();
    let oid = git(original, &["rev-parse", "HEAD:empty"]);
    let other = git(original, &["rev-parse", "HEAD:run"]);
    let loose = |id: &str| original.join(".git/objects").join(&id[..2]).join(&id[2..]);
    let selected = loose(&oid);
    let saved = fs::read(&selected).unwrap();
    fs::set_permissions(&selected, fs::Permissions::from_mode(0o600)).unwrap();
    fs::copy(loose(&other), &selected).unwrap();
    fs::write(
        original.join("empty"),
        fs::read(original.join("run")).unwrap(),
    )
    .unwrap();
    // The raw API deliberately does not claim an independent ODB rehash. Git
    // versions may reject the poisoned source early; if accepted, conversion
    // must bind the rehashed objects to these captured bytes, not just the OIDs.
    let raw = SourceSnapshot::prepare(
        &run,
        SourceRole::Source,
        &fixture.recipe(),
        TIMEOUT,
        &fixture.token,
    );
    // fs::copy preserves the read-only permissions of Git's loose object.
    fs::set_permissions(&selected, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&selected, saved).unwrap();
    fs::write(original.join("empty"), b"").unwrap();
    if let Ok(raw) = raw {
        let error = LegacySourceView::prepare(raw, TIMEOUT, &fixture.token)
            .err()
            .expect("restored objects cannot authorize poisoned snapshot bytes");
        assert!(error.to_string().contains("raw snapshot inventory"));
        assert!(fixture.work().join(".source-legacy-pending").is_dir());
    }
    assert!(!fixture.work().join("source-legacy").exists());
}

#[test]
fn cancellation_during_metadata_preparation_retains_the_owned_stage() {
    let fixture = Fixture::new();
    let run = fixture.reserve();
    let raw = snapshot(&fixture, &run);
    let pending = fixture.work().join(".source-legacy-pending");
    std::thread::scope(|scope| {
        let watcher = scope.spawn(|| {
            let deadline = std::time::Instant::now() + TIMEOUT;
            while !pending.join(".git/config").is_file() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "metadata stage not observed"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            fixture.token.cancel();
        });
        assert!(LegacySourceView::prepare(raw, TIMEOUT, &fixture.token).is_err());
        watcher.join().unwrap();
    });
    assert!(pending.join(".git").is_dir());
    assert!(!fixture.work().join("source-legacy").exists());
    assert!(fixture.request.source_dir.join(".git").is_dir());
}

#[test]
fn revalidation_budgets_cannot_be_ignored_or_remove_material() {
    let fixture = Fixture::new();
    let run = fixture.reserve();
    let view =
        LegacySourceView::prepare(snapshot(&fixture, &run), TIMEOUT, &fixture.token).unwrap();
    for timeout in [Duration::ZERO, Duration::MAX, Duration::from_nanos(1)] {
        assert!(view.revalidate(timeout, &fixture.token).is_err());
        assert!(view.root().join(".git/index").is_file());
    }
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    assert!(view.revalidate(TIMEOUT, &cancelled).is_err());
    view.revalidate(TIMEOUT, &fixture.token).unwrap();
}
