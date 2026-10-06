//! Independent CMake consumption of the honest local-only compiler contract.

use aros_common::elf::riscv::TargetContract;
use aros_common::local_toolchain::LocalToolchainDescriptor;
use aros_common::{sha256_bytes, ArosCompilerIdentity};
use serde_json::json;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;
use std::process::{Command, Output};

fn write_executable(path: &Path, contents: &[u8]) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn local_prefix(root: &Path) -> Vec<u8> {
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::create_dir_all(root.join("lib")).unwrap();
    for role in [
        "as",
        "ld",
        "ar",
        "ranlib",
        "strip",
        "collect-aros",
        "nm",
        "objcopy",
        "objdump",
    ] {
        write_executable(&root.join("bin").join(role), b"#!/bin/sh\nexit 0\n");
    }
    for runtime in ["libgcc.a", "libstdc++.a", "libsupc++.a"] {
        fs::write(root.join("lib").join(runtime), runtime.as_bytes()).unwrap();
    }
    let escaped_name = "zz-é\"vector.txt";
    fs::write(root.join("lib").join(escaped_name), b"utf8 inventory entry").unwrap();
    symlink(escaped_name, root.join("lib").join("zz-é\"link")).unwrap();
    let driver = format!(
        r#"#!/bin/sh
for arg in "$@"; do
    case "$arg" in
        -print-libgcc-file-name) printf '%s\n' '{}/lib/libgcc.a'; exit 0 ;;
        -print-file-name=libstdc++.a) printf '%s\n' '{}/lib/libstdc++.a'; exit 0 ;;
        -print-file-name=libsupc++.a) printf '%s\n' '{}/lib/libsupc++.a'; exit 0 ;;
    esac
done
exit 0
"#,
        root.display(),
        root.display(),
        root.display()
    );
    for role in ["gcc", "g++"] {
        write_executable(&root.join("bin").join(role), driver.as_bytes());
    }
    let identity = ArosCompilerIdentity::Gnu {
        gcc_version: "16.2.0".into(), binutils_version: "2.47".into(),
        target: TargetContract::parse(br#"{"schema":"aros-riscv-target-v1","isa":"rv32imafc_zicsr_zifencei_zaamo_zalrsc","abi":"ilp32f","code_model":"medany","architecture":"rv32i2p1_m2p0_a2p1_f2p2_c2p0_zicsr2p0_zifencei2p0_zaamo1p0_zalrsc1p0","unaligned_access":false,"atomic_abi":0,"x3_reg_usage":0}"#).unwrap(),
    };
    let layout = json!({"schema":"aros-toolchain-tools-v3", "compiler":identity,
    "target_triple":"riscv-aros", "tools":{
        "c":"bin/gcc", "cxx":"bin/g++", "assembler":"bin/as", "linker":"bin/ld",
        "archive":"bin/ar", "ranlib":"bin/ranlib", "strip":"bin/strip",
        "collector":"bin/collect-aros", "nm":"bin/nm", "objcopy":"bin/objcopy", "objdump":"bin/objdump"
    }});
    fs::write(
        root.join("toolchain-tools.json"),
        serde_json::to_vec(&layout).unwrap(),
    )
    .unwrap();
    let host = if cfg!(target_os = "macos") {
        "macos-aarch64"
    } else if cfg!(target_arch = "aarch64") {
        "linux-aarch64"
    } else {
        "linux-x86_64"
    };
    let descriptor =
        LocalToolchainDescriptor::capture(root, host, "fixture-local", "riscv-aros", identity)
            .unwrap();
    let bytes = serde_json::to_vec(&descriptor).unwrap();
    fs::write(root.join("toolchain-local.json"), &bytes).unwrap();
    bytes
}

fn configure(root: &Path, prefix: &Path, name: &str, bytes: &[u8], qualification: &str) -> Output {
    let project = root.join("project");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("CMakeLists.txt"),
        r#"
cmake_minimum_required(VERSION 3.22)
project(local_gnu_contract NONE)
get_filename_component(toolchain_dir "${CMAKE_TOOLCHAIN_FILE}" DIRECTORY)
include("${toolchain_dir}/../ToolchainIdentity.cmake")
aros_lock_build_tree_toolchain()
if(NOT AROS_CROSS_TOOLCHAIN_RELEASE_ID STREQUAL "")
    message(FATAL_ERROR "Local compiler was misrepresented as a release")
endif()
if(NOT CMAKE_OBJDUMP STREQUAL "${AROS_CROSS_TOOLCHAIN_ROOT}/bin/objdump")
    message(FATAL_ERROR "Local objdump role was not bound")
endif()
file(WRITE "${CMAKE_BINARY_DIR}/qualified" "local-byte-verified\n")
"#,
    )
    .unwrap();
    Command::new("cmake")
        .arg("-S")
        .arg(project)
        .arg("-B")
        .arg(root.join(name))
        .arg("-G")
        .arg("Ninja")
        .arg(format!(
            "-DCMAKE_TOOLCHAIN_FILE={}/engine/toolchains/AROS.cmake",
            root.display()
        ))
        .arg(format!("-DAROS_CROSS_TOOLCHAIN_ROOT={}", prefix.display()))
        .arg(format!(
            "-DAROS_CROSS_TOOLCHAIN_LOCAL_SHA256={}",
            sha256_bytes(bytes)
        ))
        .arg(format!(
            "-DAROS_CROSS_TOOLCHAIN_QUALIFICATION={qualification}"
        ))
        .args([
            "-DAROS_TOOLCHAIN=gnu",
            "-DAROS_TARGET_CPU=riscv",
            "-DAROS_TARGET_PLATFORM=fixture",
            "-DAROS_TARGET_PROFILE=fixture-local",
            "-DAROS_TARGET_TRIPLE=riscv-aros",
        ])
        .output()
        .unwrap()
}

fn refused(output: Output, expected: &str) {
    let Output { status, stderr, .. } = output;
    let diagnostic = String::from_utf8_lossy(&stderr);
    assert!(!status.success(), "unexpected success: {diagnostic}");
    assert!(
        diagnostic.contains(expected),
        "expected {expected}: {diagnostic}"
    );
}

#[test]
fn local_gnu_cmake_checks_every_byte_without_inventing_a_release() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    super::materialize(&root.join("engine")).unwrap();
    let prefix = root.join("local compiler");
    let bytes = local_prefix(&prefix);
    let output = configure(&root, &prefix, "positive", &bytes, "local-byte-verified");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(root.join("positive/qualified")).unwrap(),
        b"local-byte-verified\n"
    );
    let stamp = fs::read_to_string(root.join("positive/.aros-toolchain-id")).unwrap();
    assert!(stamp.starts_with("schema=2\nqualification=local-byte-verified\n"));
    assert!(!stamp.contains("release_id="));

    let mut false_tree: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let false_tree_sha = "0".repeat(64);
    assert_ne!(false_tree["tree_sha256"], false_tree_sha);
    false_tree["tree_sha256"] = json!(false_tree_sha);
    let false_tree_bytes = serde_json::to_vec(&false_tree).unwrap();
    fs::write(prefix.join("toolchain-local.json"), &false_tree_bytes).unwrap();
    refused(
        configure(
            &root,
            &prefix,
            "false-tree-digest",
            &false_tree_bytes,
            "local-byte-verified",
        ),
        "tree digest differs from its inventory",
    );
    fs::write(prefix.join("toolchain-local.json"), &bytes).unwrap();

    refused(
        configure(&root, &prefix, "not-opted-in", &bytes, ""),
        "contract is missing",
    );
    refused(
        configure(
            &root,
            &prefix,
            "unknown-qualification",
            &bytes,
            "release-equivalent",
        ),
        "contract is missing",
    );
    fs::write(prefix.join("lib/libgcc.a"), b"modified runtime").unwrap();
    refused(
        configure(
            &root,
            &prefix,
            "changed-runtime",
            &bytes,
            "local-byte-verified",
        ),
        "inventory bytes or mode changed",
    );
    fs::write(prefix.join("lib/libgcc.a"), b"libgcc.a").unwrap();
    fs::write(prefix.join("extra"), b"not inventoried").unwrap();
    refused(
        configure(&root, &prefix, "extra-entry", &bytes, "local-byte-verified"),
        "missing or extra entries",
    );
    fs::remove_file(prefix.join("extra")).unwrap();
    fs::write(
        prefix.join("toolchain-manifest.json"),
        b"invalid historical manifest",
    )
    .unwrap();
    refused(
        configure(
            &root,
            &prefix,
            "no-downgrade",
            &bytes,
            "local-byte-verified",
        ),
        "coexist with a release manifest",
    );
    fs::remove_file(prefix.join("toolchain-manifest.json")).unwrap();
    fs::write(prefix.join("toolchain-local.json"), b"modified descriptor").unwrap();
    refused(
        configure(
            &root,
            &prefix,
            "descriptor-sha",
            &bytes,
            "local-byte-verified",
        ),
        "descriptor bytes changed",
    );
    fs::write(prefix.join("toolchain-local.json"), &bytes).unwrap();
    let linked_root = root.join("linked-prefix");
    symlink(&prefix, &linked_root).unwrap();
    refused(
        configure(
            &root,
            &linked_root,
            "linked-root",
            &bytes,
            "local-byte-verified",
        ),
        "canonical and unlinked",
    );
    fs::remove_file(prefix.join("lib/libgcc.a")).unwrap();
    fs::write(root.join("outside"), b"libgcc.a").unwrap();
    symlink(root.join("outside"), prefix.join("lib/libgcc.a")).unwrap();
    refused(
        configure(&root, &prefix, "linked-file", &bytes, "local-byte-verified"),
        "became linked",
    );
}
