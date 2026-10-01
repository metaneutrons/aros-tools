use crate::parser::TargetContext;
use std::fmt::Write;

/// The banner for `generated_targets.cmake`.
///
/// Emitted separately from the body because it names the target configuration
/// this file was written for, which is an argument of the run rather than a
/// property of the graph.
///
/// Deliberately carries no timestamp: CMake rewrites this file on every
/// configure, and a changing byte would relink the world each time.
#[must_use]
pub fn generated_header(target: Option<&TargetContext>) -> String {
    let mut out = String::new();
    let rule = "# ============================================================================";
    writeln!(out, "{rule}").unwrap();
    writeln!(out, "# GENERATED FILE - DO NOT EDIT").unwrap();
    writeln!(out, "{rule}").unwrap();
    writeln!(out, "#").unwrap();
    writeln!(
        out,
        "# Written by aros-transpiler {} from the mmakefile.src tree, and rewritten",
        env!("CARGO_PKG_VERSION")
    )
    .unwrap();
    writeln!(
        out,
        "# in full on every CMake configure. An edit here is lost at the next"
    )
    .unwrap();
    writeln!(out, "# configure, without a warning.").unwrap();
    writeln!(out, "#").unwrap();
    writeln!(
        out,
        "# The source of truth is the legacy build description. To change a target,"
    )
    .unwrap();
    writeln!(
        out,
        "# edit the declaration in its <directory>/mmakefile.src, or the CMake"
    )
    .unwrap();
    writeln!(
        out,
        "# function under cmake/ that consumes it, then reconfigure. Every target"
    )
    .unwrap();
    writeln!(
        out,
        "# below states the DIRECTORY it came from, and its MMAKE_ID is the"
    )
    .unwrap();
    writeln!(
        out,
        "# `mmake=` of the declaration, so both ends are greppable."
    )
    .unwrap();
    writeln!(out, "#").unwrap();
    writeln!(
        out,
        "# Anything a declaration asked for and this file does not express is"
    )
    .unwrap();
    writeln!(
        out,
        "# reported beside it, in generated_targets.*.txt. Those reports are the"
    )
    .unwrap();
    writeln!(out, "# record of what was left out, and why.").unwrap();
    writeln!(out, "#").unwrap();

    // Only the target-selecting arguments. --source-dir and --output are
    // deliberately absent: they are absolute host paths, and naming them would
    // tie the file to one checkout location.
    let stated: Vec<(&str, &str)> = target.map_or_else(Vec::new, |target| {
        [
            ("--cpu", target.cpu.as_deref()),
            ("--platform", target.platform.as_deref()),
            ("--family", target.family.as_deref()),
            ("--variant", target.variant.as_deref()),
            ("--toolchain", target.toolchain.as_deref()),
            ("--cpu32", target.cpu32.as_deref()),
            ("--use-mmu", target.use_mmu.as_deref()),
            ("--float-abi", target.float_abi.as_deref()),
            ("--mesa-version", target.mesa_version.as_deref()),
            ("--target-llvm-ver", target.target_llvm_ver.as_deref()),
            (
                "--target-llvm-runtimes-style",
                target.target_llvm_runtimes_style.as_deref(),
            ),
            ("--target-rust", target.target_rust.as_deref()),
            ("--target-rust-ver", target.target_rust_ver.as_deref()),
        ]
        .into_iter()
        .filter_map(|(flag, value)| value.map(|value| (flag, value)))
        .collect()
    });
    if stated.is_empty() {
        writeln!(
            out,
            "# Written with no target selected, so nothing here is architecture-filtered."
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "# Written for this target. A different one yields a different file:"
        )
        .unwrap();
        writeln!(out, "#").unwrap();
        for (flag, value) in stated {
            let shown = if value.is_empty() { "\"\"" } else { value };
            writeln!(out, "#     {flag:<12} {shown}").unwrap();
        }
    }
    writeln!(out, "#").unwrap();
    writeln!(
        out,
        "# --source-dir and --output come from CMakeLists.txt and are omitted here,"
    )
    .unwrap();
    writeln!(
        out,
        "# so this file does not depend on where the tree is checked out. To"
    )
    .unwrap();
    writeln!(
        out,
        "# reproduce it, reconfigure the preset that built it, or replay the full"
    )
    .unwrap();
    writeln!(
        out,
        "# argv that CMake recorded in generated_targets.cmake.invocation beside"
    )
    .unwrap();
    writeln!(out, "# this file -- which is what `aros golden` does.").unwrap();
    writeln!(out, "{rule}").unwrap();
    writeln!(out).unwrap();

    // First statement, before anything can call into the engine. The calls
    // below are a contract, and a graph meeting an engine that does not
    // implement it should say so rather than fail as an unknown function
    // eighty thousand lines further down.
    writeln!(
        out,
        "aros_require_engine_api_version({})",
        aros_cmake_engine::api_version()
    )
    .unwrap();
    writeln!(out).unwrap();
    out
}
