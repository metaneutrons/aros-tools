//! Physical SDK config-header probes; these do not qualify a compiler or an SDK.

#![cfg(unix)]

use aros_common::{native_consumer_contract::load_bound_native_consumer_contract, TargetProfile};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Probe {
    output: Output,
    successful_marker: bool,
    config_header: Option<String>,
}

fn run_bootstrap(flavour: &str, platform_smp: &str, exec_smp: &str) -> Probe {
    let temporary = tempfile::tempdir().expect("create SDK bootstrap fixture");
    let root = fs::canonicalize(temporary.path()).expect("canonicalize fixture root");
    let source = root.join("source");
    let build = root.join("build");
    fs::create_dir_all(source.join("compiler/include/asm")).unwrap();
    fs::create_dir_all(source.join("compiler/arossupport/include")).unwrap();
    fs::create_dir_all(&build).unwrap();
    fs::write(
        source.join("compiler/include/asm/cpu.h"),
        "/* cpu dispatcher */\n",
    )
    .unwrap();
    fs::write(
        source.join("compiler/arossupport/include/fixture.h"),
        "/* AROS support header */\n",
    )
    .unwrap();
    // asm.c is deliberately absent: this probe exercises header publication,
    // not compiler invocation or assembly-header generation.

    let genmodule = root.join("genmodule-mock");
    fs::write(&genmodule, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = fs::metadata(&genmodule).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&genmodule, permissions).unwrap();

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
         set(AROS_ABI_FLAVOUR \"${FLAVOUR}\")\n\
         set(AROS_ABI_PLATFORM_SMP \"${PLATFORM_SMP}\")\n\
         set(AROS_NATIVE_BUILD_EXEC_SMP OFF)\n\
         set(AROS_NATIVE_CONSUMER_EXEC_SMP \"${EXEC_SMP}\")\n\
         set(AROS_GENMODULE_BIN \"${ROOT}/genmodule-mock\")\n\
         include(\"${ENGINE}/BootstrapSDK.cmake\")\n\
         aros_bootstrap_sdk_includes()\n\
         file(WRITE \"${CMAKE_BINARY_DIR}/successful-bootstrap\" \"done\")\n",
    )
    .unwrap();

    let output = Command::new("cmake")
        .current_dir(&build)
        .arg(format!("-DROOT={}", root.display()))
        .arg(format!("-DENGINE={}", engine.display()))
        .arg(format!("-DFLAVOUR={flavour}"))
        .arg(format!("-DPLATFORM_SMP={platform_smp}"))
        .arg(format!("-DEXEC_SMP={exec_smp}"))
        .arg("-P")
        .arg(&script)
        .output()
        .expect("run CMake SDK bootstrap probe");
    let successful_marker = build.join("successful-bootstrap").is_file();
    let config_header = fs::read_to_string(build.join("SDK/include/aros/config.h")).ok();

    Probe {
        output,
        successful_marker,
        config_header,
    }
}

fn assert_header_flags(probe: &Probe, platform_smp: bool, exec_smp: bool) {
    assert!(
        probe.output.status.success(),
        "{}",
        cmake_output(&probe.output)
    );
    assert!(
        probe.successful_marker,
        "bootstrap did not reach its success marker"
    );
    let header = probe
        .config_header
        .as_deref()
        .expect("bootstrap did not materialize SDK aros/config.h");
    assert!(
        header.lines().any(|line| {
            let mut fields = line.split_whitespace();
            fields.next() == Some("#define")
                && fields.next() == Some("AROS_FLAVOUR")
                && fields.next() == Some("AROS_FLAVOUR_STANDALONE")
                && fields.next().is_none()
        }),
        "standalone flavour definition missing from generated config header:\n{header}"
    );
    assert_define(header, "__AROSPLATFORM_SMP__", platform_smp);
    assert_define(header, "__AROSEXEC_SMP__", exec_smp);
}

fn assert_define(header: &str, name: &str, expected: bool) {
    let definition = format!("#define {name}");
    let count = header
        .lines()
        .filter(|line| *line == definition.as_str())
        .count();
    assert_eq!(
        count,
        usize::from(expected),
        "unexpected materialized definition for {name}:\n{header}"
    );
}

fn cmake_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn bootstrap_materializes_platform_and_exec_smp_independently() {
    for platform_smp in [false, true] {
        for exec_smp in [false, true] {
            let probe = run_bootstrap(
                "standalone",
                if platform_smp { "ON" } else { "OFF" },
                if exec_smp { "ON" } else { "OFF" },
            );
            assert_header_flags(&probe, platform_smp, exec_smp);
        }
    }
}

#[test]
fn bootstrap_rejects_unknown_abi_selectors_before_success_publication() {
    for (flavour, platform_smp, expected_error) in [
        (
            "standalone",
            "",
            "AROS_ABI_PLATFORM_SMP must be explicit ON or OFF",
        ),
        (
            "standalone",
            "true",
            "AROS_ABI_PLATFORM_SMP must be explicit ON or OFF",
        ),
        (
            "standalone",
            "ON;unsafe",
            "AROS_ABI_PLATFORM_SMP must be explicit ON or OFF",
        ),
        (
            "mystery",
            "ON",
            "Explicit bootstrap ABI requires a valid flavour and platform_smp",
        ),
    ] {
        let probe = run_bootstrap(flavour, platform_smp, "OFF");
        assert!(
            !probe.output.status.success(),
            "accepted {flavour}/{platform_smp}"
        );
        assert!(
            !probe.successful_marker,
            "published success after rejecting an ABI selector"
        );
        assert!(
            cmake_output(&probe.output).contains(expected_error),
            "{}",
            cmake_output(&probe.output)
        );
    }
}

#[test]
#[ignore = "requires AROS_TEST_P4_SOURCE with the current native SDK declaration"]
fn source_owned_p4_contract_flags_materialize_and_project_independently() {
    let source = PathBuf::from(
        std::env::var_os("AROS_TEST_P4_SOURCE").expect("AROS_TEST_P4_SOURCE selects source"),
    );
    let target_file = source.join("aros-targets.toml");
    let target_text = fs::read_to_string(&target_file).unwrap();
    let config = TargetProfile::parse_config(&target_text, "aros-targets.toml").unwrap();
    let selected = config
        .targets
        .iter()
        .filter(|profile| profile.name == "esp32p4-riscv-gnu")
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 1, "P4 SDK profile must be unique");
    let profile = selected[0];
    let relative = Path::new(profile.native_consumer_contract.as_deref().unwrap());
    let binding = load_bound_native_consumer_contract(&source, relative, profile).unwrap();
    let contract = &binding.contract;
    let abi = &contract.abi;
    assert_eq!(abi.source_cpu, "riscv");
    assert_eq!(abi.target_triple, "riscv-aros");
    let generated = contract
        .generated_make_templates
        .get("compiler/include/geninc.cfg")
        .expect("P4 contract binds the generated include configuration");
    let exec_substitution = generated
        .substitutions
        .get("@ENABLE_EXECSMP@")
        .expect("P4 contract declares ENABLE_EXECSMP");
    let exec_smp = match exec_substitution.as_str() {
        "#define __AROSEXEC_SMP__" => true,
        "" => false,
        value => panic!("unsupported ENABLE_EXECSMP contract value {value:?}"),
    };
    let platform_smp = abi.platform_smp;
    assert!(
        platform_smp,
        "P4 source contract is expected to declare platform SMP"
    );
    assert!(
        exec_smp,
        "P4 source contract is expected to declare Exec SMP"
    );

    let declared = run_bootstrap(
        &abi.flavour,
        if platform_smp { "ON" } else { "OFF" },
        if exec_smp { "ON" } else { "OFF" },
    );
    assert_header_flags(&declared, platform_smp, exec_smp);

    // These are fixture projections of one dimension at a time. They do not
    // mutate or re-admit the source-owned contract.
    let platform_off = run_bootstrap(&abi.flavour, "OFF", if exec_smp { "ON" } else { "OFF" });
    assert_header_flags(&platform_off, false, exec_smp);

    let exec_off = run_bootstrap(&abi.flavour, if platform_smp { "ON" } else { "OFF" }, "OFF");
    assert_header_flags(&exec_off, platform_smp, false);
}
