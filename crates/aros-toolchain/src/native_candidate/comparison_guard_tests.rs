//! Root separation and post-readback mutation counterprobes; no execution claims.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;

use aros_common::{measure_tree_content_cas, DiagnosticCode};

use super::tests::{package_request, Fixture};
use super::{compare_finished_candidate_packages, package_finished_candidate};

#[test]
fn rejects_canonical_alias_and_nested_cross_side_roots() {
    for alias in [false, true] {
        let packages = tempfile::tempdir().unwrap();
        let mut left = Fixture::new();
        left.remove_old_compiler_checkpoint();
        left.persist();
        let left_candidate = left.readback().unwrap();
        let left_package = package_finished_candidate(
            &package_request(&left, packages.path().join("left")),
            &left_candidate,
        )
        .unwrap();
        let mut right = Fixture::new();
        right.remove_old_compiler_checkpoint();
        right.persist();
        let mut right_candidate = right.readback().unwrap();
        let right_package = package_finished_candidate(
            &package_request(&right, packages.path().join("right")),
            &right_candidate,
        )
        .unwrap();

        right_candidate.work_dir = if alias {
            // The selected leaf is a real directory; the parent is an alias.
            let alias_root = packages.path().join("alias");
            symlink(left_candidate.work_dir.parent().unwrap(), &alias_root).unwrap();
            alias_root.join("work")
        } else {
            let nested = left_candidate.work_dir.join("nested");
            fs::create_dir(&nested).unwrap();
            nested
        };
        let before = measure_tree_content_cas(&left_candidate.work_dir).unwrap();
        let error = compare_finished_candidate_packages(
            &left_candidate,
            &left_package,
            &right_candidate,
            &right_package,
        )
        .unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerComparison
        );
        assert!(error.to_string().contains("roots must be disjoint"));
        assert_eq!(
            measure_tree_content_cas(&left_candidate.work_dir).unwrap(),
            before
        );
    }
}

#[test]
fn rejects_each_side_changed_after_a_successful_comparison_without_writing() {
    for right_side in [false, true] {
        for member in 0..6 {
            let packages = tempfile::tempdir().unwrap();
            let mut left = Fixture::new();
            left.remove_old_compiler_checkpoint();
            left.persist();
            let left_candidate = left.readback().unwrap();
            let left_package = package_finished_candidate(
                &package_request(&left, packages.path().join("left")),
                &left_candidate,
            )
            .unwrap();
            let mut right = Fixture::new();
            right.remove_old_compiler_checkpoint();
            right.persist();
            let right_candidate = right.readback().unwrap();
            let right_package = package_finished_candidate(
                &package_request(&right, packages.path().join("right")),
                &right_candidate,
            )
            .unwrap();
            let comparison = compare_finished_candidate_packages(
                &left_candidate,
                &left_package,
                &right_candidate,
                &right_package,
            )
            .unwrap();
            let (fixture, package) = if right_side {
                (&right, &right_package)
            } else {
                (&left, &left_package)
            };
            let path = match member {
                0 => fixture.candidate().join("bin/clang"),
                1 => fixture.receipts_dir().join("publish.json"),
                2 => package.output().archive.clone(),
                3 => package.output().manifest.clone(),
                4 => package.output().checksum.clone(),
                5 => package.output().sbom.clone(),
                _ => unreachable!(),
            };
            let mut changed = fs::read(&path).unwrap();
            changed.push(b'\n');
            fs::write(&path, changed).unwrap();
            let before = fs::read(&path).unwrap();
            let package_before = measure_tree_content_cas(&package.output().output_dir).unwrap();
            assert!(
                comparison.revalidate().is_err(),
                "side={right_side}, member={member}"
            );
            assert_eq!(fs::read(&path).unwrap(), before);
            assert_eq!(
                measure_tree_content_cas(&package.output().output_dir).unwrap(),
                package_before
            );
        }
    }
}
