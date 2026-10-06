//! Build-tree identity checks independent of compiler admission fixtures.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Output};

fn run(root: &Path, build: &Path, local: bool, changes: &[(&str, &str)]) -> Output {
    let mut command = Command::new("cmake");
    command.current_dir(build).args([
        "-DAROS_CROSS_TOOLCHAIN_ROOT=/measured/compiler",
        "-DAROS_TOOLCHAIN=gnu",
        "-DAROS_TARGET_PROFILE=fixture-local",
        "-DAROS_TARGET_TRIPLE=riscv-aros",
    ]);
    command.arg(format!(
        "-DAROS_CROSS_TOOLCHAIN_TREE_SHA256={}",
        "a".repeat(64)
    ));
    if local {
        command.arg("-DAROS_CROSS_TOOLCHAIN_QUALIFICATION=local-byte-verified");
        command.arg(format!(
            "-DAROS_CROSS_TOOLCHAIN_LOCAL_SHA256={}",
            "b".repeat(64)
        ));
    } else {
        command.arg("-DAROS_CROSS_TOOLCHAIN_RELEASE_ID=fixture-v1");
    }
    for (name, value) in changes {
        command.arg(format!("-D{name}={value}"));
    }
    command
        .arg("-P")
        .arg(root.join("lock.cmake"))
        .output()
        .unwrap()
}

fn success(output: Output) {
    let Output { status, stderr, .. } = output;
    assert!(status.success(), "{}", String::from_utf8_lossy(&stderr));
}

fn refusal(output: Output, diagnostic: &str) {
    let Output { status, stderr, .. } = output;
    assert!(!status.success(), "identity change was accepted");
    assert!(
        String::from_utf8_lossy(&stderr).contains(diagnostic),
        "{}",
        String::from_utf8_lossy(&stderr)
    );
}

#[test]
fn build_tree_identity_preserves_released_stamps_and_rejects_local_mixing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    super::materialize(&root.join("engine")).unwrap();
    fs::write(
        root.join("lock.cmake"),
        format!(
            "include(\"{}/engine/ToolchainIdentity.cmake\")\naros_lock_build_tree_toolchain()\n",
            root.display()
        ),
    )
    .unwrap();
    let local = root.join("local");
    let release = root.join("released");
    fs::create_dir(&local).unwrap();
    fs::create_dir(&release).unwrap();

    success(run(&root, &local, true, &[]));
    let local_stamp = fs::read(local.join(".aros-toolchain-id")).unwrap();
    let expected_local = format!(
        "schema=2\nqualification=local-byte-verified\nroot=/measured/compiler\ndescriptor_sha256={}\ntarget_profile=fixture-local\ntarget_triple=riscv-aros\ntree_sha256={}\n",
        "b".repeat(64),
        "a".repeat(64)
    );
    assert_eq!(
        String::from_utf8(local_stamp.clone()).unwrap(),
        expected_local,
        "equal board/compiler profiles retain the schema-2 identity bytes"
    );
    success(run(&root, &local, true, &[]));
    for changes in [
        vec![("AROS_CROSS_TOOLCHAIN_ROOT", "/another/compiler")],
        vec![("AROS_TARGET_PROFILE", "another-profile")],
        vec![("AROS_TARGET_TRIPLE", "another-triple")],
        vec![(
            "AROS_CROSS_TOOLCHAIN_TREE_SHA256",
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        )],
        vec![(
            "AROS_CROSS_TOOLCHAIN_LOCAL_SHA256",
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        )],
    ] {
        refusal(
            run(&root, &local, true, &changes),
            "different AROS toolchain",
        );
        assert_eq!(
            fs::read(local.join(".aros-toolchain-id")).unwrap(),
            local_stamp
        );
    }
    refusal(run(&root, &local, false, &[]), "different AROS toolchain");
    refusal(
        run(&root, &local, true, &[("AROS_TOOLCHAIN", "clang")]),
        "cannot claim a release",
    );
    refusal(
        run(
            &root,
            &local,
            true,
            &[("AROS_CROSS_TOOLCHAIN_RELEASE_ID", "fabricated")],
        ),
        "cannot claim a release",
    );
    refusal(
        run(
            &root,
            &local,
            true,
            &[("AROS_CROSS_TOOLCHAIN_LOCAL_SHA256", "bad")],
        ),
        "requires verified",
    );
    refusal(
        run(
            &root,
            &local,
            true,
            &[("AROS_CROSS_TOOLCHAIN_QUALIFICATION", "release-equivalent")],
        ),
        "Unsupported build-tree",
    );
    assert_eq!(
        fs::read(local.join(".aros-toolchain-id")).unwrap(),
        local_stamp
    );

    success(run(&root, &release, false, &[]));
    let expected = format!(
        "schema=1\nroot=/measured/compiler\nrelease_id=fixture-v1\ntarget_profile=fixture-local\ntarget_triple=riscv-aros\ntree_sha256={}\n",
        "a".repeat(64)
    );
    assert_eq!(
        fs::read_to_string(release.join(".aros-toolchain-id")).unwrap(),
        expected
    );
    success(run(&root, &release, false, &[]));
    refusal(run(&root, &release, true, &[]), "different AROS toolchain");
    assert_eq!(
        fs::read_to_string(release.join(".aros-toolchain-id")).unwrap(),
        expected
    );

    let release_distinct = root.join("released-distinct-profile");
    fs::create_dir(&release_distinct).unwrap();
    let compiler_profile = "riscv32-p4-local";
    success(run(
        &root,
        &release_distinct,
        false,
        &[("AROS_CROSS_TOOLCHAIN_PROFILE", compiler_profile)],
    ));
    let distinct_release_stamp = fs::read(release_distinct.join(".aros-toolchain-id")).unwrap();
    let expected_distinct_release = format!(
        "schema=1\nroot=/measured/compiler\nrelease_id=fixture-v1\ntarget_profile=fixture-local\ntarget_triple=riscv-aros\ntree_sha256={}\ntoolchain_profile={compiler_profile}\n",
        "a".repeat(64)
    );
    assert_eq!(
        String::from_utf8(distinct_release_stamp.clone()).unwrap(),
        expected_distinct_release,
        "a distinct released compiler profile is appended after the legacy stamp fields"
    );
    refusal(
        run(
            &root,
            &release_distinct,
            false,
            &[("AROS_CROSS_TOOLCHAIN_PROFILE", "another-compiler-profile")],
        ),
        "different AROS toolchain",
    );
    assert_eq!(
        fs::read(release_distinct.join(".aros-toolchain-id")).unwrap(),
        distinct_release_stamp
    );

    for (name, dangling) in [("linked", false), ("dangling", true)] {
        let build = root.join(name);
        fs::create_dir(&build).unwrap();
        let outside = root.join(format!("outside-{name}"));
        if !dangling {
            fs::write(&outside, b"untouched").unwrap();
        }
        symlink(&outside, build.join(".aros-toolchain-id")).unwrap();
        refusal(run(&root, &build, true, &[]), "regular unlinked file");
        if dangling {
            assert!(!outside.exists());
        } else {
            assert_eq!(fs::read(&outside).unwrap(), b"untouched");
        }
    }
    let invalid = root.join("directory-stamp");
    fs::create_dir(&invalid).unwrap();
    fs::create_dir(invalid.join(".aros-toolchain-id")).unwrap();
    refusal(run(&root, &invalid, true, &[]), "regular unlinked file");
    let oversized = root.join("oversized");
    fs::create_dir(&oversized).unwrap();
    fs::write(oversized.join(".aros-toolchain-id"), vec![b'x'; 8193]).unwrap();
    refusal(run(&root, &oversized, true, &[]), "size limit");

    let local_distinct = root.join("local-distinct-profile");
    fs::create_dir(&local_distinct).unwrap();
    let compiler_profile = "riscv32-p4-local";
    success(run(
        &root,
        &local_distinct,
        true,
        &[("AROS_CROSS_TOOLCHAIN_PROFILE", compiler_profile)],
    ));
    let distinct_stamp = fs::read(local_distinct.join(".aros-toolchain-id")).unwrap();
    let expected_distinct = format!(
        "schema=2\nqualification=local-byte-verified\nroot=/measured/compiler\ndescriptor_sha256={}\ntarget_profile=fixture-local\ntarget_triple=riscv-aros\ntree_sha256={}\ntoolchain_profile={compiler_profile}\n",
        "b".repeat(64),
        "a".repeat(64)
    );
    assert_eq!(
        String::from_utf8(distinct_stamp.clone()).unwrap(),
        expected_distinct,
        "a distinct compiler profile is appended without replacing the board profile"
    );
    refusal(
        run(
            &root,
            &local_distinct,
            true,
            &[("AROS_CROSS_TOOLCHAIN_PROFILE", "another-compiler-profile")],
        ),
        "different AROS toolchain",
    );
    assert_eq!(
        fs::read(local_distinct.join(".aros-toolchain-id")).unwrap(),
        distinct_stamp,
        "a changed compiler profile cannot replace the existing stamp"
    );
}
