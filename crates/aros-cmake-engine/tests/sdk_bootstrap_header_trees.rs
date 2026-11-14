//! The SDK include root must not depend on how the source tree's timestamps
//! happen to be laid out. CMake's file(COPY) keeps the first of two same-named
//! files when their timestamps are equal and takes the last when they differ,
//! so an unfiltered tree copy published `compiler/include/mmakefile.src` on
//! one host and `compiler/crt/posixc/include/mmakefile.src` on another.

#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

#[derive(Clone, Copy, PartialEq)]
enum Stamps {
    /// Every source file carries the same timestamp, as a tarball extraction does.
    Equal,
    /// Later trees are newer than earlier ones, as a git checkout may produce.
    Ascending,
    /// Later trees are older than earlier ones.
    Descending,
}

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn stamp(path: &Path, timestamp: &str) {
    let status = Command::new("touch")
        .args(["-t", timestamp])
        .arg(path)
        .status()
        .expect("run touch");
    assert!(status.success());
}

fn apply_stamps(source: &Path, stamps: Stamps) {
    // Trees in the order BootstrapSDK stages them.
    for (index, tree) in [
        "compiler/include",
        "compiler/crt/stdc/include",
        "compiler/crt/posixc/include",
    ]
    .iter()
    .enumerate()
    {
        let timestamp = match stamps {
            Stamps::Equal => "202001010000".to_owned(),
            Stamps::Ascending => format!("20200101{:02}00", index + 1),
            Stamps::Descending => format!("20200101{:02}00", 9 - index),
        };
        let mut pending = vec![source.join(tree)];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    stamp(&path, &timestamp);
                }
            }
        }
    }
}

struct Run {
    output: Output,
    sdk: PathBuf,
    _keep: tempfile::TempDir,
}

fn run_bootstrap(stamps: Stamps, extra: impl Fn(&Path)) -> Run {
    let temporary = tempfile::tempdir().expect("create SDK bootstrap fixture");
    let root = fs::canonicalize(temporary.path()).expect("canonicalize fixture root");
    let source = root.join("source");
    let build = root.join("build");
    fs::create_dir_all(&build).unwrap();

    write(&source, "compiler/include/asm/cpu.h", "/* cpu */\n");
    write(&source, "compiler/include/proto/exec.h", "/* proto */\n");
    write(
        &source,
        "compiler/include/mmakefile.src",
        "# big upstream makefile\n",
    );
    write(&source, "compiler/include/.gitignore", "geninc.cfg\n");
    write(&source, "compiler/include/geninc.cfg.in", "template\n");
    write(
        &source,
        "compiler/arossupport/include/support.h",
        "/* support */\n",
    );
    write(
        &source,
        "compiler/crt/stdc/include/mmakefile.src",
        "# stdc\n",
    );
    write(
        &source,
        "compiler/crt/stdc/include/.gitignore",
        "mmakefile\n",
    );
    write(
        &source,
        "compiler/crt/stdc/include/aros/stdc/stdio.h",
        "/* stdc stdio */\n",
    );
    write(
        &source,
        "compiler/crt/posixc/include/mmakefile.src",
        "# posixc\n",
    );
    write(
        &source,
        "compiler/crt/posixc/include/.gitignore",
        "mmakefile\n",
    );
    write(
        &source,
        "compiler/crt/posixc/include/aros/posixc/stdio.h",
        "/* posixc stdio */\n",
    );
    write(
        &source,
        "compiler/crt/posixc/include/libraries/posix.h",
        "/* posix */\n",
    );
    extra(&source);
    apply_stamps(&source, stamps);

    let genmodule = root.join("genmodule-mock");
    fs::write(&genmodule, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&genmodule, fs::Permissions::from_mode(0o755)).unwrap();
    let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("engine");
    let script = root.join("bootstrap.cmake");
    fs::write(
        &script,
        "cmake_minimum_required(VERSION 3.22)\n\
         set(AROS_SOURCE_DIR \"${ROOT}/source\")\n\
         set(AROS_SDK_INCLUDE_DIR \"${CMAKE_BINARY_DIR}/SDK/include\")\n\
         set(AROS_GENINC_DIR \"${CMAKE_BINARY_DIR}/GENINCDIR\")\n\
         set(AROS_TARGET_CPU riscv)\n\
         set(AROS_TARGET_PLATFORM esp32p4)\n\
         set(AROS_ABI_FLAVOUR standalone)\n\
         set(AROS_ABI_PLATFORM_SMP OFF)\n\
         set(AROS_NATIVE_BUILD_EXEC_SMP OFF)\n\
         set(AROS_NATIVE_CONSUMER_EXEC_SMP OFF)\n\
         set(AROS_GENMODULE_BIN \"${ROOT}/genmodule-mock\")\n\
         include(\"${ENGINE}/BootstrapSDK.cmake\")\n\
         aros_bootstrap_sdk_includes()\n",
    )
    .unwrap();
    let output = Command::new("cmake")
        .current_dir(&build)
        .arg(format!("-DROOT={}", root.display()))
        .arg(format!("-DENGINE={}", engine.display()))
        .arg("-P")
        .arg(&script)
        .output()
        .expect("run CMake SDK bootstrap probe");
    Run {
        output,
        sdk: build.join("SDK/include"),
        _keep: temporary,
    }
}

fn files(root: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                found.push((relative, fs::read_to_string(&path).unwrap_or_default()));
            }
        }
    }
    found.sort();
    found
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn sdk_include_root_is_independent_of_source_timestamps() {
    let runs: Vec<_> = [Stamps::Equal, Stamps::Ascending, Stamps::Descending]
        .into_iter()
        .map(|stamps| run_bootstrap(stamps, |_| {}))
        .collect();
    for run in &runs {
        assert!(run.output.status.success(), "{}", text(&run.output));
    }
    let reference = files(&runs[0].sdk);
    assert!(!reference.is_empty());
    for run in &runs[1..] {
        assert_eq!(files(&run.sdk), reference);
    }
}

#[test]
fn build_metadata_is_not_published_into_the_sdk() {
    let run = run_bootstrap(Stamps::Equal, |_| {});
    assert!(run.output.status.success(), "{}", text(&run.output));
    for (relative, _) in files(&run.sdk) {
        let name = relative.rsplit('/').next().unwrap();
        assert!(
            !matches!(
                name,
                "mmakefile.src" | "mmakefile" | ".gitignore" | "geninc.cfg.in"
            ),
            "build metadata published into the SDK: {relative}"
        );
    }
    assert_eq!(
        fs::read_to_string(run.sdk.join("proto/exec.h")).unwrap(),
        "/* proto */\n"
    );
    assert_eq!(
        fs::read_to_string(run.sdk.join("libraries/posix.h")).unwrap(),
        "/* posix */\n"
    );
}

#[test]
fn declared_stdc_precedence_over_posixc_holds_for_every_timestamp_layout() {
    for stamps in [Stamps::Equal, Stamps::Ascending, Stamps::Descending] {
        let run = run_bootstrap(stamps, |_| {});
        assert!(run.output.status.success(), "{}", text(&run.output));
        assert_eq!(
            fs::read_to_string(run.sdk.join("stdio.h")).unwrap(),
            "/* stdc stdio */\n"
        );
    }
}

#[test]
fn conflicting_headers_from_different_trees_fail_instead_of_picking_a_host_winner() {
    let run = run_bootstrap(Stamps::Equal, |source| {
        write(source, "compiler/include/clash.h", "/* generic */\n");
        write(
            source,
            "compiler/crt/posixc/include/clash.h",
            "/* posixc */\n",
        );
    });
    assert!(!run.output.status.success());
    let output = text(&run.output);
    assert!(output.contains("clash.h"), "{output}");
    assert!(output.contains("two source"), "{output}");
}
