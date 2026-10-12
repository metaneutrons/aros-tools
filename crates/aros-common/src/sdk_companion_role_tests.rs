use super::*;
use crate::sdk_manifest::SdkAbiValue;
use crate::target::{BootstrapAbiProfile, TranspilerProfile};
use crate::Architecture;
use std::collections::BTreeMap;

fn role(cpu: &str, triple: &str, machine: u64, library_dir: &str) -> SdkCompanionRole {
    SdkCompanionRole {
        selector_cpu: cpu.into(),
        target: SdkTarget {
            target_triple: triple.into(),
            abi: BTreeMap::from([
                ("source_cpu".into(), SdkAbiValue::Text(cpu.into())),
                ("pointer_width".into(), SdkAbiValue::Unsigned(32)),
                ("elf_class".into(), SdkAbiValue::Unsigned(1)),
                ("elf_machine".into(), SdkAbiValue::Unsigned(machine)),
                (
                    "endian".into(),
                    SdkAbiValue::Text(
                        if matches!(cpu, "m68k" | "ppc") {
                            "big"
                        } else {
                            "little"
                        }
                        .into(),
                    ),
                ),
            ]),
        },
        compiler_options: vec![format!("--target={triple}")],
        library_dir: library_dir.into(),
        required_runtime_library: format!("builtins-{cpu}"),
    }
}

fn profile(cpu32: &str) -> TargetProfile {
    TargetProfile {
        name: "fixture-x86_64".into(),
        toolchain_profile: None,
        arch: Architecture::X86_64,
        platform: "fixture".into(),
        bsp: "fixture".into(),
        features: Vec::new(),
        float_abi: None,
        bootloader: None,
        transpiler: Some(TranspilerProfile {
            family: String::new(),
            variant: String::new(),
            toolchain: "llvm".into(),
            cpu32: cpu32.into(),
            use_mmu: true,
            mesa_version: None,
        }),
        bootstrap_abi: Some(BootstrapAbiProfile {
            flavour: "standalone".into(),
            platform_smp: true,
        }),
        native_build_contract: None,
        native_consumer_contract: None,
        sdk_consumer_contract: Some("sdk-consumer.json".into()),
    }
}

fn primary() -> SdkTarget {
    SdkTarget {
        target_triple: "x86_64-unknown-aros".into(),
        abi: BTreeMap::from([("source_cpu".into(), SdkAbiValue::Text("x86_64".into()))]),
    }
}

#[test]
fn companion_roles_validate_explicit_i386_and_other_supported_32_bit_architectures() {
    let i386 = role("i386", "i386-unknown-aros", 3, "lib32");
    assert!(i386.validate().is_ok());
    assert!(validate_companion_roles(&[i386], &primary(), &profile("i386")).is_ok());

    let mut arm = role("arm", "arm-unknown-aros", 40, "arm32");
    arm.required_runtime_library = "runtime-arm".into();
    assert!(validate_companion_roles(&[arm], &primary(), &profile("arm")).is_ok());

    // The source selector token for RISC-V 32-bit is `riscv`, while the
    // compiler target triple uses the architecture spelling `riscv32`.
    let riscv = role("riscv", "riscv32-unknown-aros", 243, "riscv32");
    assert!(validate_companion_roles(&[riscv], &primary(), &profile("riscv")).is_ok());

    let ppc = role("ppc", "ppc-unknown-aros", 20, "ppc32");
    assert!(validate_companion_roles(&[ppc], &primary(), &profile("ppc")).is_ok());
}

#[test]
fn companion_role_requires_explicit_cpu_abi_and_triple_agreement() {
    let mut invalid = role("i386", "i386-unknown-aros", 3, "lib32");
    invalid
        .target
        .abi
        .insert("pointer_width".into(), SdkAbiValue::Unsigned(64));
    assert!(invalid.validate().is_err());

    let mut invalid = role("i386", "i386-unknown-aros", 3, "lib32");
    invalid
        .target
        .abi
        .insert("elf_class".into(), SdkAbiValue::Unsigned(2));
    assert!(invalid.validate().is_err());

    let mut invalid = role("i386", "i386-unknown-aros", 3, "lib32");
    invalid
        .target
        .abi
        .insert("endian".into(), SdkAbiValue::Text("big".into()));
    assert!(invalid.validate().is_err());

    let invalid = role("i386", "i386-unknown-aros", 62, "lib32");
    assert!(invalid.validate().is_err());

    let invalid = role("arm", "i386-unknown-aros", 40, "arm32");
    assert!(invalid.validate().is_err());
}

#[test]
fn companion_role_requires_one_matching_argv_target_and_rejects_conflicts() {
    let mut invalid = role("i386", "i386-unknown-aros", 3, "lib32");
    invalid.compiler_options = vec!["--target=x86_64-unknown-aros".into()];
    assert!(invalid.validate().is_err());

    let mut invalid = role("i386", "i386-unknown-aros", 3, "lib32");
    invalid.compiler_options.push("-m64".into());
    assert!(invalid.validate().is_err());

    let mut invalid = role("i386", "i386-unknown-aros", 3, "lib32");
    invalid.compiler_options.push("-march=i686".into());
    assert!(invalid.validate().is_err());

    let mut valid = role("i386", "i386-unknown-aros", 3, "lib32");
    valid.compiler_options.push("-m32".into());
    assert!(valid.validate().is_ok());

    let mut invalid = role("i386", "i386-unknown-aros", 3, "lib32");
    invalid.compiler_options.push("-mbig-endian".into());
    assert!(invalid.validate().is_err());

    let mut invalid = role("i386", "i386-unknown-aros", 3, "lib32");
    invalid
        .compiler_options
        .push("-march=i486; echo bad".into());
    assert!(invalid.validate().is_err());

    let mut invalid = role("i386", "i386-unknown-aros", 3, "lib32");
    invalid
        .compiler_options
        .push("--gcc-toolchain=/opt/other".into());
    assert!(invalid.validate().is_err());

    let mut invalid = role("i386", "i386-unknown-aros", 3, "lib32");
    invalid.compiler_options = vec!["--target=".into()];
    assert!(invalid.validate().is_err());
}

#[test]
fn companion_roles_bind_source_selector_and_reject_collisions() {
    let i386 = role("i386", "i386-unknown-aros", 3, "lib32");
    assert!(
        validate_companion_roles(std::slice::from_ref(&i386), &primary(), &profile("")).is_err()
    );
    assert!(
        validate_companion_roles(std::slice::from_ref(&i386), &primary(), &profile("arm")).is_err()
    );

    let mut primary_cpu_collision = i386;
    primary_cpu_collision.selector_cpu = "x86_64".into();
    primary_cpu_collision.target.target_triple = "x86_64-unknown-aros".into();
    assert!(
        validate_companion_roles(&[primary_cpu_collision], &primary(), &profile("x86_64")).is_err()
    );

    let primary_root = role("i386", "i386-unknown-aros", 3, "lib");
    assert!(validate_companion_roles(&[primary_root], &primary(), &profile("i386")).is_err());

    let second_role = role("arm", "arm-unknown-aros", 40, "arm32");
    assert!(validate_companion_roles(
        &[role("i386", "i386-unknown-aros", 3, "lib32"), second_role],
        &primary(),
        &profile("i386")
    )
    .is_err());
}

#[test]
fn companion_role_paths_and_runtime_names_are_closed_data() {
    for path in [
        "/lib32",
        "lib/32",
        "lib32/extra",
        "../lib32",
        "lib\\32",
        ".",
        "..",
        "_lib32",
        "-lib32",
        "",
    ] {
        let invalid = role("i386", "i386-unknown-aros", 3, path);
        assert!(invalid.validate().is_err(), "accepted library_dir {path:?}");
    }
    for library in ["-lfoo", "dir/libfoo.a", "foo bar", ""] {
        let mut invalid = role("i386", "i386-unknown-aros", 3, "lib32");
        invalid.required_runtime_library = library.into();
        assert!(invalid.validate().is_err(), "accepted library {library:?}");
    }
}
