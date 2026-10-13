use super::*;
use aros_common::sdk_manifest::SdkAbiValue;
use std::collections::BTreeMap;

fn target_with_facts(
    triple: &str,
    pointer_width: Option<SdkAbiValue>,
    elf_machine: Option<SdkAbiValue>,
    elf_flags_mask: Option<SdkAbiValue>,
    elf_flags_value: Option<SdkAbiValue>,
    float_abi: Option<SdkAbiValue>,
) -> SdkTarget {
    let mut abi = BTreeMap::new();
    for (key, value) in [
        ("pointer_width", pointer_width),
        ("elf_machine", elf_machine),
        ("elf_flags_mask", elf_flags_mask),
        ("elf_flags_value", elf_flags_value),
        ("float_abi", float_abi),
    ] {
        if let Some(value) = value {
            abi.insert(key.to_owned(), value);
        }
    }
    SdkTarget {
        target_triple: triple.to_owned(),
        abi,
    }
}

fn unsigned(value: u64) -> SdkAbiValue {
    SdkAbiValue::Unsigned(value)
}

fn target32() -> SdkTarget {
    target_with_facts(
        "riscv32-vendor-aros",
        Some(unsigned(32)),
        Some(unsigned(243)),
        Some(unsigned(0x3)),
        Some(unsigned(0x1)),
        Some(SdkAbiValue::Text("softfp".to_owned())),
    )
}

fn target64() -> SdkTarget {
    target_with_facts(
        "futurecpu-vendor-aros",
        Some(unsigned(64)),
        Some(unsigned(0xbeef)),
        Some(unsigned(0x7)),
        Some(unsigned(0x2)),
        None,
    )
}

fn with_compiler_facts(
    mut target: SdkTarget,
    cpu_fact: Option<SdkAbiValue>,
    floating_point_fact: Option<SdkAbiValue>,
) -> SdkTarget {
    for (key, value) in [
        ("compiler_cpu", cpu_fact),
        ("compiler_fpu", floating_point_fact),
    ] {
        if let Some(value) = value {
            target.abi.insert(key.to_owned(), value);
        }
    }
    target
}

fn with_compiler_arch(mut target: SdkTarget, architecture: SdkAbiValue) -> SdkTarget {
    target.abi.insert("compiler_arch".to_owned(), architecture);
    target
}

fn with_compiler_code_model(mut target: SdkTarget, code_model: SdkAbiValue) -> SdkTarget {
    target
        .abi
        .insert("compiler_code_model".to_owned(), code_model);
    target
}

#[test]
fn source_boolean_codegen_switches_are_typed_and_reach_all_four_links_once() {
    for enabled in [false, true] {
        let mut target = target64();
        for fact in ["compiler_red_zone", "compiler_pic", "compiler_pie"] {
            target
                .abi
                .insert(fact.to_owned(), SdkAbiValue::Boolean(enabled));
        }
        for (language, label, suffix) in [
            ("c", "original", "c"),
            ("c", "relocated", "c"),
            ("cxx", "original", "cpp"),
            ("cxx", "relocated", "cpp"),
        ] {
            let fixture = format!("/source/app.{suffix}");
            let command = application_command(
                Path::new("/prefix/bin/clang"),
                Path::new(&fixture),
                &probe(&format!("app.{suffix}"), &[]),
                &target,
                Path::new("/sdk"),
                Path::new("/proof"),
                (language, label),
            )
            .unwrap();
            for (yes, no) in [
                ("-mred-zone", "-mno-red-zone"),
                ("-fPIC", "-fno-pic"),
                ("-fPIE", "-fno-pie"),
            ] {
                let selected = if enabled { yes } else { no };
                assert_eq!(
                    command
                        .arguments
                        .iter()
                        .filter(|flag| flag.as_str() == selected)
                        .count(),
                    1
                );
                assert!(!command
                    .arguments
                    .iter()
                    .any(|flag| flag == if enabled { no } else { yes }));
            }
        }
    }
    let absent = target64();
    assert!(compiler_switch_facts(&absent).unwrap().is_empty());
    for fact in ["compiler_red_zone", "compiler_pic", "compiler_pie"] {
        for value in [SdkAbiValue::Text("false".into()), SdkAbiValue::Unsigned(0)] {
            let mut target = target64();
            target.abi.insert(fact.to_owned(), value);
            assert_rejection(
                validate_target(&target),
                &format!("SDK target {fact} must be boolean"),
            );
        }
    }
}

fn probe(source: &str, libraries: &[&str]) -> NativeSdkLinkProbe {
    NativeSdkLinkProbe {
        source: source.to_owned(),
        libraries: libraries
            .iter()
            .map(|library| (*library).to_owned())
            .collect(),
    }
}

fn assert_rejection<T: std::fmt::Debug>(result: Result<T, ContractError>, expected: &str) {
    let error = result.expect_err("fixture must be rejected");
    let diagnostics = error.diagnostics();
    assert_eq!(diagnostics.diagnostics.len(), 1);
    assert_eq!(diagnostics.diagnostics[0].message, expected);
}

fn elf_fixture(pointer_width: u64, machine: u16, flags: u32, symbol: &str) -> Vec<u8> {
    let mut bytes = match pointer_width {
        32 => super::super::fixture_elf32(symbol),
        64 => super::super::fixture_elf64(symbol),
        _ => panic!("fixture pointer width must be supported"),
    };
    bytes[16..18].copy_from_slice(&1_u16.to_le_bytes()); // ET_REL
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    let flags_offset = if pointer_width == 32 { 36 } else { 48 };
    bytes[flags_offset..flags_offset + 4].copy_from_slice(&flags.to_le_bytes());
    bytes
}

fn arm_target() -> SdkTarget {
    let mut target = target_with_facts(
        "arm-unknown-aros",
        Some(unsigned(32)),
        Some(unsigned(40)),
        Some(unsigned(0xff00_0000)),
        Some(unsigned(0x0500_0000)),
        Some(SdkAbiValue::Text("hard".into())),
    );
    for (name, value) in [
        ("arm_cpu_arch", 10),
        ("arm_cpu_profile", 65),
        ("arm_fp_arch", 5),
        ("arm_simd_arch", 2),
        ("arm_vfp_args", 1),
        ("arm_arm_isa", 1),
        ("arm_thumb_isa", 2),
    ] {
        target.abi.insert(name.into(), unsigned(value));
    }
    target
}

// Add an independently encoded aeabi section to the ordinary collector fixture.
// No CPU name or object flag determines the attribute bytes.
fn arm_elf(attributes: &[(u8, u8)]) -> Vec<u8> {
    let mut bytes = elf_fixture(32, 40, 0x0500_0000, REQUIRED_STARTUP_SYMBOL);
    let prior_headers = bytes[52..172].to_vec();
    let old_names_offset = u32::from_le_bytes(bytes[108..112].try_into().unwrap()) as usize;
    let old_names_size = u32::from_le_bytes(bytes[112..116].try_into().unwrap()) as usize;
    let mut names = bytes[old_names_offset..old_names_offset + old_names_size].to_vec();
    let attribute_name = names.len() as u32;
    names.extend_from_slice(b".ARM.attributes\0");
    let names_offset = bytes.len() as u32;
    bytes.extend_from_slice(&names);
    let attribute_offset = bytes.len() as u32;
    let mut file = vec![1];
    file.extend_from_slice(&(5 + 2 * attributes.len() as u32).to_le_bytes());
    for (tag, value) in attributes {
        file.extend_from_slice(&[*tag, *value]);
    }
    let mut encoded = vec![b'A'];
    encoded.extend_from_slice(&(10 + file.len() as u32).to_le_bytes());
    encoded.extend_from_slice(b"aeabi\0");
    encoded.extend_from_slice(&file);
    bytes.extend_from_slice(&encoded);
    let headers_offset = bytes.len() as u32;
    let mut headers = prior_headers;
    headers[56..60].copy_from_slice(&names_offset.to_le_bytes());
    headers[60..64].copy_from_slice(&(names.len() as u32).to_le_bytes());
    let mut section = [0_u8; 40];
    section[..4].copy_from_slice(&attribute_name.to_le_bytes());
    section[4..8].copy_from_slice(&elf::arm::SHT_ATTRIBUTES.to_le_bytes());
    section[16..20].copy_from_slice(&attribute_offset.to_le_bytes());
    section[20..24].copy_from_slice(&(encoded.len() as u32).to_le_bytes());
    section[32..36].copy_from_slice(&1_u32.to_le_bytes());
    headers.extend_from_slice(&section);
    bytes.extend_from_slice(&headers);
    bytes[32..36].copy_from_slice(&headers_offset.to_le_bytes());
    bytes[48..50].copy_from_slice(&4_u16.to_le_bytes());
    bytes[50..52].copy_from_slice(&1_u16.to_le_bytes());
    bytes
}

#[test]
fn arm_attributes_are_measured_instead_of_inferred_from_identical_elf_flags() {
    let target = arm_target();
    let values = [(6, 10), (7, 65), (8, 1), (9, 2), (10, 5), (12, 2), (28, 1)];
    let valid = arm_elf(&values);
    validate_target(&target).unwrap();
    verify_application_elf(&valid, &target).unwrap();
    for index in 0..values.len() {
        let mut wrong = values;
        wrong[index].1 = 0;
        let changed = arm_elf(&wrong);
        assert_eq!(&changed[36..40], &valid[36..40]);
        assert_rejection(
            verify_application_elf(&changed, &target),
            "LLVM SDK application ARM attributes differ from the source target",
        );
    }
    assert_rejection(
        verify_application_elf(
            &elf_fixture(32, 40, 0x0500_0000, REQUIRED_STARTUP_SYMBOL),
            &target,
        ),
        "LLVM SDK application ARM attributes are absent or invalid",
    );
}

#[test]
fn arm_target_facts_are_explicit_typed_and_cannot_leak_to_other_machines() {
    let target = arm_target();
    for name in [
        "arm_cpu_arch",
        "arm_cpu_profile",
        "arm_fp_arch",
        "arm_simd_arch",
        "arm_vfp_args",
    ] {
        let mut missing = target.clone();
        missing.abi.remove(name);
        assert_rejection(
            validate_target(&missing),
            &format!("SDK target is missing required unsigned ABI fact {name}"),
        );
        for value in [SdkAbiValue::Text("1".into()), SdkAbiValue::Boolean(true)] {
            let mut wrong = target.clone();
            wrong.abi.insert(name.into(), value);
            assert_rejection(
                validate_target(&wrong),
                &format!("SDK target ABI fact {name} must be an unsigned integer"),
            );
        }
    }
    let mut wrong = target.clone();
    wrong.abi.insert("arm_future_unknown".into(), unsigned(1));
    assert_rejection(
        validate_target(&wrong),
        "SDK target declares an unsupported ARM attribute fact",
    );
    wrong = target.clone();
    wrong.abi.insert("pointer_width".into(), unsigned(64));
    assert_rejection(
        validate_target(&wrong),
        "ARM SDK attribute facts require ELF32",
    );
    wrong = target.clone();
    wrong.abi.remove("float_abi");
    assert_rejection(
        validate_target(&wrong),
        "ARM SDK requires an explicit float ABI",
    );
    for value in [0, 2, 4] {
        wrong = target.clone();
        wrong.abi.insert("arm_vfp_args".into(), unsigned(value));
        assert_rejection(
            validate_target(&wrong),
            "ARM VFP argument convention contradicts the selected float ABI",
        );
    }
    wrong = target64();
    wrong.abi.insert("arm_cpu_arch".into(), unsigned(10));
    assert_rejection(
        validate_target(&wrong),
        "non-ARM SDK cannot declare ARM attribute facts",
    );
}

#[test]
fn arm_soft_float_uses_only_standardized_omitted_zero_attributes() {
    let mut target = arm_target();
    target
        .abi
        .insert("float_abi".into(), SdkAbiValue::Text("soft".into()));
    for name in ["arm_fp_arch", "arm_simd_arch", "arm_vfp_args"] {
        target.abi.insert(name.into(), unsigned(0));
    }
    let valid = arm_elf(&[(6, 10), (7, 65), (8, 1), (9, 2)]);
    verify_application_elf(&valid, &target).unwrap();
    let mut undefined = vec![(6, 10), (7, 65), (8, 1), (9, 2)];
    undefined.push((64, 0));
    assert_rejection(
        verify_application_elf(&arm_elf(&undefined), &target),
        "LLVM SDK application ARM attributes are absent or invalid",
    );
}

#[test]
fn commands_preserve_normal_defaults_target_float_fact_and_ordered_libraries() {
    let command = application_command(
        Path::new("/prefix/bin/clang++"),
        Path::new("/source/src/application.cpp"),
        &probe("src/application.cpp", &["first", "second+rt", "third_2"]),
        &target32(),
        Path::new("/selected/sdk"),
        Path::new("/proof"),
        ("cxx", "relocated"),
    )
    .unwrap();

    assert_eq!(command.program, Path::new("/prefix/bin/clang++"));
    let expected = [
        "--target=riscv32-vendor-aros",
        "--sysroot=/selected/sdk",
        "-O2",
        "-fno-common",
        "-g0",
        "-mfloat-abi=softfp",
        "-std=c++17",
        "/source/src/application.cpp",
        "-Wl,-Map=/proof/relocated-cxx.map,--cref,--trace",
        "-lfirst",
        "-lsecond+rt",
        "-lthird_2",
        "-o",
        "/proof/relocated-cxx.elf",
    ]
    .map(str::to_owned)
    .to_vec();
    assert_eq!(command.arguments, expected);
    assert!(command.arguments.len() <= MAX_APPLICATION_ARGUMENTS);
    for forbidden in [
        "-c",
        "-r",
        "-nostdlib",
        "-nostartfiles",
        "-nodefaultlibs",
        "-ffreestanding",
        "-march=",
        "-mabi=",
        "-mcmodel=",
    ] {
        assert!(
            !command
                .arguments
                .iter()
                .any(|argument| argument == forbidden),
            "{forbidden}"
        );
    }

    let boundary_libraries = (0..MAX_ADDITIONAL_LIBRARIES)
        .map(|index| format!("library-{index}"))
        .collect::<Vec<_>>();
    let boundary_probe = NativeSdkLinkProbe {
        source: "src/application.cpp".into(),
        libraries: boundary_libraries,
    };
    let boundary = application_command(
        Path::new("/prefix/bin/clang++"),
        Path::new("/source/src/application.cpp"),
        &boundary_probe,
        &target32(),
        Path::new("/selected/sdk"),
        Path::new("/proof"),
        ("cxx", "original"),
    )
    .unwrap();
    assert_eq!(
        boundary
            .arguments
            .iter()
            .filter(|argument| argument.starts_with("-l"))
            .count(),
        MAX_ADDITIONAL_LIBRARIES
    );
    assert!(boundary.arguments.len() <= MAX_APPLICATION_ARGUMENTS);
}

#[test]
fn c_commands_do_not_infer_float_mode_or_cpp_mode() {
    let command = application_command(
        Path::new("/prefix/bin/clang"),
        Path::new("/source/app.c"),
        &probe("app.c", &[]),
        &target64(),
        Path::new("/selected/sdk"),
        Path::new("/proof"),
        ("c", "original"),
    )
    .unwrap();
    assert!(command
        .arguments
        .contains(&"--target=futurecpu-vendor-aros".into()));
    assert!(!command
        .arguments
        .iter()
        .any(|argument| argument.starts_with("-mfloat-abi=")));
    assert!(!command
        .arguments
        .iter()
        .any(|argument| argument == "-std=c++17"));
    assert!(command
        .arguments
        .contains(&"-Wl,-Map=/proof/original-c.map,--cref,--trace".into()));
    assert!(command.arguments.contains(&"/proof/original-c.elf".into()));
    assert_eq!(
        command.arguments,
        [
            "--target=futurecpu-vendor-aros",
            "--sysroot=/selected/sdk",
            "-O2",
            "-fno-common",
            "-g0",
            "/source/app.c",
            "-Wl,-Map=/proof/original-c.map,--cref,--trace",
            "-o",
            "/proof/original-c.elf",
        ]
        .map(str::to_owned)
        .to_vec()
    );
    validate_target(&target64()).unwrap();
}

#[test]
fn compiler_cpu_and_fpu_facts_apply_for_generic_targets_and_both_languages() {
    let mut target = with_compiler_facts(
        target64(),
        Some(SdkAbiValue::Text("future-cpu+rev2".into())),
        Some(SdkAbiValue::Text("future-fpu.v4".into())),
    );
    target
        .abi
        .insert("float_abi".into(), SdkAbiValue::Text("hard".into()));
    validate_target(&target).unwrap();

    for (language, label, driver, fixture, source) in [
        (
            "c",
            "original",
            "/prefix/bin/clang",
            "/source/app.c",
            "app.c",
        ),
        (
            "cxx",
            "relocated",
            "/prefix/bin/clang++",
            "/source/app.cpp",
            "app.cpp",
        ),
    ] {
        let command = application_command(
            Path::new(driver),
            Path::new(fixture),
            &probe(source, &[]),
            &target,
            Path::new("/selected/sdk"),
            Path::new("/proof"),
            (language, label),
        )
        .unwrap();
        let float_index = command
            .arguments
            .iter()
            .position(|argument| argument == "-mfloat-abi=hard")
            .unwrap();
        let cpu_index = command
            .arguments
            .iter()
            .position(|argument| argument == "-mcpu=future-cpu+rev2")
            .unwrap();
        let fpu_index = command
            .arguments
            .iter()
            .position(|argument| argument == "-mfpu=future-fpu.v4")
            .unwrap();
        assert!(float_index < cpu_index && cpu_index < fpu_index);
        if language == "cxx" {
            let standard_index = command
                .arguments
                .iter()
                .position(|argument| argument == "-std=c++17")
                .unwrap();
            assert!(fpu_index < standard_index);
        }
        assert!(command.arguments.contains(&format!(
            "-Wl,-Map=/proof/{label}-{language}.map,--cref,--trace"
        )));
    }
}

#[test]
fn compiler_cpu_and_fpu_facts_are_independent_and_optional() {
    let command = |target: &SdkTarget| {
        application_command(
            Path::new("/prefix/bin/clang"),
            Path::new("/source/app.c"),
            &probe("app.c", &[]),
            target,
            Path::new("/selected/sdk"),
            Path::new("/proof"),
            ("c", "original"),
        )
        .unwrap()
    };

    let cpu_only = command(&with_compiler_facts(
        target64(),
        Some(SdkAbiValue::Text("future-cpu".into())),
        None,
    ));
    assert!(cpu_only.arguments.contains(&"-mcpu=future-cpu".into()));
    assert!(!cpu_only
        .arguments
        .iter()
        .any(|argument| argument.starts_with("-mfpu=")));

    let fpu_only = command(&with_compiler_facts(
        target64(),
        None,
        Some(SdkAbiValue::Text("future-fpu".into())),
    ));
    assert!(!fpu_only
        .arguments
        .iter()
        .any(|argument| argument.starts_with("-mcpu=")));
    assert!(fpu_only.arguments.contains(&"-mfpu=future-fpu".into()));
}

#[test]
fn compiler_arch_fact_applies_to_c_and_cxx_for_original_and_relocated_sources() {
    let mut target = with_compiler_arch(target64(), SdkAbiValue::Text("armv7-a+fp".to_owned()));
    target
        .abi
        .insert("float_abi".into(), SdkAbiValue::Text("hard".into()));
    target.abi.insert(
        "compiler_fpu".into(),
        SdkAbiValue::Text("future-fpu.v4".into()),
    );
    validate_target(&target).unwrap();

    for (language, label, driver, fixture, source) in [
        (
            "c",
            "original",
            "/prefix/bin/clang",
            "/source/app.c",
            "app.c",
        ),
        (
            "c",
            "relocated",
            "/prefix/bin/clang",
            "/source/relocated/app.c",
            "app.c",
        ),
        (
            "cxx",
            "original",
            "/prefix/bin/clang++",
            "/source/app.cpp",
            "app.cpp",
        ),
        (
            "cxx",
            "relocated",
            "/prefix/bin/clang++",
            "/source/relocated/app.cpp",
            "app.cpp",
        ),
    ] {
        let command = application_command(
            Path::new(driver),
            Path::new(fixture),
            &probe(source, &[]),
            &target,
            Path::new("/selected/sdk"),
            Path::new("/proof"),
            (language, label),
        )
        .unwrap();
        assert_eq!(
            command
                .arguments
                .iter()
                .filter(|argument| argument.starts_with("-march="))
                .count(),
            1
        );
        assert!(command.arguments.contains(&"-march=armv7-a+fp".into()));
        assert!(!command
            .arguments
            .iter()
            .any(|argument| argument.starts_with("-mcpu=")));
        let float_index = command
            .arguments
            .iter()
            .position(|argument| argument == "-mfloat-abi=hard")
            .unwrap();
        let arch_index = command
            .arguments
            .iter()
            .position(|argument| argument == "-march=armv7-a+fp")
            .unwrap();
        let fpu_index = command
            .arguments
            .iter()
            .position(|argument| argument == "-mfpu=future-fpu.v4")
            .unwrap();
        assert!(float_index < arch_index && arch_index < fpu_index);
        if language == "cxx" {
            let standard_index = command
                .arguments
                .iter()
                .position(|argument| argument == "-std=c++17")
                .unwrap();
            assert!(fpu_index < standard_index);
        }
        assert!(command.arguments.contains(&format!(
            "-Wl,-Map=/proof/{label}-{language}.map,--cref,--trace"
        )));
    }
}

#[test]
fn compiler_arch_is_optional_bounded_and_exclusive_with_compiler_cpu() {
    for value in [SdkAbiValue::Boolean(false), unsigned(1)] {
        let target = with_compiler_arch(target64(), value);
        assert_rejection(
            validate_target(&target),
            "SDK target compiler_arch must be text",
        );
    }

    for value in [
        String::new(),
        "_arch".into(),
        ".arch".into(),
        "+arch".into(),
        "-arch".into(),
        "arm arch".into(),
        "arch=armv7".into(),
        "/armv7".into(),
        "@arch.rsp".into(),
        "arm\\arch".into(),
        "arm\narch".into(),
        "armç".into(),
        "x".repeat(MAX_COMPILER_OPTION_BYTES + 1),
    ] {
        let target = with_compiler_arch(target64(), SdkAbiValue::Text(value));
        assert_rejection(
            validate_target(&target),
            "SDK target compiler_arch must be 1..=64 ASCII characters, start with a letter or digit, and contain only letters, digits, _, ., + or -",
        );
    }

    let maximum_length = with_compiler_arch(
        target64(),
        SdkAbiValue::Text("x".repeat(MAX_COMPILER_OPTION_BYTES)),
    );
    validate_target(&maximum_length).unwrap();

    let with_arch = with_compiler_arch(target64(), SdkAbiValue::Text("armv7-a+fp".to_owned()));
    let with_both = with_compiler_facts(
        with_arch,
        Some(SdkAbiValue::Text("cortex-a7".to_owned())),
        None,
    );
    assert_rejection(
        validate_target(&with_both),
        "SDK target compiler_arch and compiler_cpu are mutually exclusive",
    );
}

#[test]
fn compiler_code_model_applies_to_c_and_cxx_for_original_and_relocated_sources() {
    let target = with_compiler_code_model(target64(), SdkAbiValue::Text("large".into()));
    validate_target(&target).unwrap();

    for (language, label, driver, fixture, source) in [
        (
            "c",
            "original",
            "/prefix/bin/clang",
            "/source/app.c",
            "app.c",
        ),
        (
            "c",
            "relocated",
            "/prefix/bin/clang",
            "/source/relocated/app.c",
            "app.c",
        ),
        (
            "cxx",
            "original",
            "/prefix/bin/clang++",
            "/source/app.cpp",
            "app.cpp",
        ),
        (
            "cxx",
            "relocated",
            "/prefix/bin/clang++",
            "/source/relocated/app.cpp",
            "app.cpp",
        ),
    ] {
        let command = application_command(
            Path::new(driver),
            Path::new(fixture),
            &probe(source, &[]),
            &target,
            Path::new("/selected/sdk"),
            Path::new("/proof"),
            (language, label),
        )
        .unwrap();
        assert_eq!(
            command
                .arguments
                .iter()
                .filter(|argument| argument.starts_with("-mcmodel="))
                .count(),
            1
        );
        let code_model_index = command
            .arguments
            .iter()
            .position(|argument| argument == "-mcmodel=large")
            .unwrap();
        let debug_index = command
            .arguments
            .iter()
            .position(|argument| argument == "-g0")
            .unwrap();
        assert_eq!(code_model_index, debug_index + 1);
        if language == "cxx" {
            let standard_index = command
                .arguments
                .iter()
                .position(|argument| argument == "-std=c++17")
                .unwrap();
            assert!(code_model_index < standard_index);
        }
        assert!(command.arguments.contains(&format!(
            "-Wl,-Map=/proof/{label}-{language}.map,--cref,--trace"
        )));
    }
}

#[test]
fn compiler_code_model_is_optional_and_uses_bounded_generic_token_validation() {
    let absent = target64();
    validate_target(&absent).unwrap();
    let absent_command = application_command(
        Path::new("/prefix/bin/clang"),
        Path::new("/source/app.c"),
        &probe("app.c", &[]),
        &absent,
        Path::new("/selected/sdk"),
        Path::new("/proof"),
        ("c", "original"),
    )
    .unwrap();
    assert!(!absent_command
        .arguments
        .iter()
        .any(|argument| argument.starts_with("-mcmodel=")));

    for value in [SdkAbiValue::Boolean(false), unsigned(1)] {
        let target = with_compiler_code_model(target64(), value);
        assert_rejection(
            validate_target(&target),
            "SDK target compiler_code_model must be text",
        );
    }

    for value in [
        String::new(),
        "_model".into(),
        ".model".into(),
        "+model".into(),
        "-model".into(),
        "large model".into(),
        "model=large".into(),
        "/large".into(),
        "@model.rsp".into(),
        "large\\model".into(),
        "large\nmodel".into(),
        "lárge".into(),
        "x".repeat(MAX_COMPILER_OPTION_BYTES + 1),
    ] {
        let target = with_compiler_code_model(target64(), SdkAbiValue::Text(value));
        assert_rejection(
            validate_target(&target),
            "SDK target compiler_code_model must be 1..=64 ASCII characters, start with a letter or digit, and contain only letters, digits, _, ., + or -",
        );
    }

    let maximum_length = with_compiler_code_model(
        target64(),
        SdkAbiValue::Text("x".repeat(MAX_COMPILER_OPTION_BYTES)),
    );
    validate_target(&maximum_length).unwrap();
}

#[test]
fn validates_required_unsigned_facts_and_ranges() {
    let base = target32();
    for (field, expected) in [
        (
            "pointer_width",
            "SDK target is missing required unsigned ABI fact pointer_width",
        ),
        (
            "elf_machine",
            "SDK target is missing required unsigned ABI fact elf_machine",
        ),
        (
            "elf_flags_mask",
            "SDK target is missing required unsigned ABI fact elf_flags_mask",
        ),
        (
            "elf_flags_value",
            "SDK target is missing required unsigned ABI fact elf_flags_value",
        ),
    ] {
        let mut target = base.clone();
        target.abi.remove(field);
        assert_rejection(validate_target(&target), expected);
    }

    for field in [
        "pointer_width",
        "elf_machine",
        "elf_flags_mask",
        "elf_flags_value",
    ] {
        let mut target = base.clone();
        target
            .abi
            .insert(field.to_owned(), SdkAbiValue::Text("32".to_owned()));
        assert_rejection(
            validate_target(&target),
            &format!("SDK target ABI fact {field} must be an unsigned integer"),
        );
    }

    let mut bad_width = base.clone();
    bad_width
        .abi
        .insert("pointer_width".into(), SdkAbiValue::Unsigned(16));
    assert_rejection(
        validate_target(&bad_width),
        "SDK target pointer_width must be 32 or 64",
    );

    for machine in [0, u64::from(u16::MAX) + 1] {
        let mut target = base.clone();
        target
            .abi
            .insert("elf_machine".into(), SdkAbiValue::Unsigned(machine));
        assert_rejection(
            validate_target(&target),
            "SDK target elf_machine must be in 1..=65535",
        );
    }

    for field in ["elf_flags_mask", "elf_flags_value"] {
        let mut target = base.clone();
        target
            .abi
            .insert(field.into(), SdkAbiValue::Unsigned(u64::from(u32::MAX) + 1));
        assert_rejection(
            validate_target(&target),
            &format!("SDK target ABI fact {field} must not exceed 4294967295"),
        );
    }

    let mut outside_mask = base;
    outside_mask
        .abi
        .insert("elf_flags_mask".into(), SdkAbiValue::Unsigned(0x1));
    outside_mask
        .abi
        .insert("elf_flags_value".into(), SdkAbiValue::Unsigned(0x2));
    assert_rejection(
        validate_target(&outside_mask),
        "SDK target elf_flags_value contains bits outside elf_flags_mask",
    );
}

#[test]
fn validates_optional_float_fact_and_exact_aros_triple_without_cpu_allowlist() {
    for value in ["hard", "soft", "softfp"] {
        let mut target = target32();
        target
            .abi
            .insert("float_abi".into(), SdkAbiValue::Text(value.into()));
        validate_target(&target).unwrap();
    }

    for value in [
        SdkAbiValue::Text("soft-float".into()),
        SdkAbiValue::Boolean(true),
        SdkAbiValue::Unsigned(1),
    ] {
        let mut target = target32();
        target.abi.insert("float_abi".into(), value);
        assert_rejection(
            validate_target(&target),
            "SDK target float_abi must be text hard, soft, or softfp",
        );
    }

    for triple in ["riscv--aros", "riscv-unknown-linux", "riscv-aros-"] {
        let mut target = target32();
        target.target_triple = triple.into();
        assert_rejection(
            validate_target(&target),
            "SDK target triple must have nonempty components and end in an exact -aros component",
        );
    }

    let mut invalid_generic = target32();
    invalid_generic
        .abi
        .insert("board".into(), SdkAbiValue::Text("fixture-board".into()));
    assert_rejection(
        validate_target(&invalid_generic),
        "LLVM SDK source target descriptor is invalid",
    );
    validate_target(&target64()).unwrap();
}

#[test]
fn validates_compiler_cpu_and_fpu_fact_types_bounds_and_portable_tokens() {
    for name in ["compiler_cpu", "compiler_fpu"] {
        for value in [SdkAbiValue::Boolean(false), unsigned(1)] {
            let mut target = target64();
            target.abi.insert(name.into(), value);
            assert_rejection(
                validate_target(&target),
                &format!("SDK target {name} must be text"),
            );
        }

        for value in [
            String::new(),
            "_cpu".into(),
            ".cpu".into(),
            "+cpu".into(),
            "-cpu".into(),
            "cpu name".into(),
            "cpu=name".into(),
            "/cpu".into(),
            "@cpu.rsp".into(),
            "cpu\\name".into(),
            "cpu\nname".into(),
            "cṕu".into(),
            "x".repeat(MAX_COMPILER_OPTION_BYTES + 1),
        ] {
            let mut target = target64();
            target.abi.insert(name.into(), SdkAbiValue::Text(value));
            assert_rejection(
                validate_target(&target),
                &format!("SDK target {name} must be 1..=64 ASCII characters, start with a letter or digit, and contain only letters, digits, _, ., + or -"),
            );
        }
    }

    for name in ["compiler_cpu", "compiler_fpu"] {
        let mut target = target64();
        target.abi.insert(
            name.into(),
            SdkAbiValue::Text("x".repeat(MAX_COMPILER_OPTION_BYTES)),
        );
        validate_target(&target).unwrap();
    }
}

#[test]
fn verifies_source_selected_elf_class_machine_flags_and_aros_identity() {
    for (width, target) in [(32, target32()), (64, target64())] {
        let machine = match target.abi.get("elf_machine") {
            Some(SdkAbiValue::Unsigned(machine)) => u16::try_from(*machine).unwrap(),
            _ => unreachable!(),
        };
        let mask = match target.abi.get("elf_flags_mask") {
            Some(SdkAbiValue::Unsigned(mask)) => u32::try_from(*mask).unwrap(),
            _ => unreachable!(),
        };
        let value = match target.abi.get("elf_flags_value") {
            Some(SdkAbiValue::Unsigned(value)) => u32::try_from(*value).unwrap(),
            _ => unreachable!(),
        };
        verify_application_elf(
            &elf_fixture(
                width,
                machine,
                value | (0x80 & !mask),
                REQUIRED_STARTUP_SYMBOL,
            ),
            &target,
        )
        .unwrap();
    }

    let target = target32();
    let wrong_class = elf_fixture(32, 243, 1, REQUIRED_STARTUP_SYMBOL);
    let target_64 = target_with_facts(
        "futurecpu-vendor-aros",
        Some(unsigned(64)),
        Some(unsigned(243)),
        Some(unsigned(3)),
        Some(unsigned(1)),
        None,
    );
    assert_rejection(
        verify_application_elf(&wrong_class, &target_64),
        "LLVM SDK application ELF class or machine differs from the source target",
    );

    assert_rejection(
        verify_application_elf(&elf_fixture(32, 244, 1, REQUIRED_STARTUP_SYMBOL), &target),
        "LLVM SDK application ELF class or machine differs from the source target",
    );
    assert_rejection(
        verify_application_elf(&elf_fixture(32, 243, 0, REQUIRED_STARTUP_SYMBOL), &target),
        "LLVM SDK application ELF flags differ from the source mask/value",
    );

    let mut wrong_osabi = elf_fixture(32, 243, 1, REQUIRED_STARTUP_SYMBOL);
    wrong_osabi[7] = 0;
    assert_rejection(
        verify_application_elf(&wrong_osabi, &target),
        "LLVM SDK application ELF lacks the selected AROS ABI marking",
    );

    let mut wrong_version = elf_fixture(32, 243, 1, REQUIRED_STARTUP_SYMBOL);
    wrong_version[8] = AROS_ABI_VERSION + 1;
    assert_rejection(
        verify_application_elf(&wrong_version, &target),
        "LLVM SDK application ELF lacks the selected AROS ABI marking",
    );

    let mut wrong_type = elf_fixture(32, 243, 1, REQUIRED_STARTUP_SYMBOL);
    wrong_type[16..18].copy_from_slice(&2_u16.to_le_bytes());
    assert_rejection(
        verify_application_elf(&wrong_type, &target),
        "LLVM SDK application output is not an ET_REL object",
    );

    assert_rejection(
        verify_application_elf(&elf_fixture(32, 243, 1, "OTHER_SYMBOL"), &target),
        "LLVM SDK application ELF lacks its required defined startup list symbol",
    );
}

#[test]
fn requires_a_defined_program_entries_startup_list_not_a_toolchain_list() {
    let target = target32();

    verify_application_elf(&elf_fixture(32, 243, 1, REQUIRED_STARTUP_SYMBOL), &target).unwrap();

    assert_rejection(
        verify_application_elf(&elf_fixture(32, 243, 1, "__TOOLCHAIN_LIST__"), &target),
        "LLVM SDK application ELF lacks its required defined startup list symbol",
    );

    let mut undefined = elf_fixture(32, 243, 1, REQUIRED_STARTUP_SYMBOL);
    // The 32-bit fixture's final word is st_shndx for the defined symbol.
    let symbol_section_offset = undefined.len() - 2;
    undefined[symbol_section_offset..].copy_from_slice(&0_u16.to_le_bytes());
    assert_rejection(
        verify_application_elf(&undefined, &target),
        "LLVM SDK application ELF lacks its required defined startup list symbol",
    );
}

#[test]
fn rejects_invalid_selectors_paths_probe_sources_and_libraries() {
    let good_probe = probe("app.c", &["client", "support+rt"]);
    let target = target32();
    let paths = (
        Path::new("/prefix/bin/clang"),
        Path::new("/source/app.c"),
        Path::new("/selected/sdk"),
        Path::new("/proof"),
    );

    for selection in [("cpp", "original"), ("c", "stale")] {
        let expected = if selection.0 == "cpp" {
            "LLVM SDK application language must be c or cxx"
        } else {
            "LLVM SDK application label must be original or relocated"
        };
        assert_rejection(
            application_command(
                paths.0,
                paths.1,
                &good_probe,
                &target,
                paths.2,
                paths.3,
                selection,
            ),
            expected,
        );
    }

    assert_rejection(
        application_command(
            Path::new("bin/clang"),
            paths.1,
            &good_probe,
            &target,
            paths.2,
            paths.3,
            ("c", "original"),
        ),
        "LLVM SDK driver path must be absolute",
    );
    for (program, fixture, sdk, output, expected) in [
        (
            paths.0,
            Path::new("source/app.c"),
            paths.2,
            paths.3,
            "LLVM SDK fixture path must be absolute",
        ),
        (
            paths.0,
            paths.1,
            Path::new("selected/sdk"),
            paths.3,
            "LLVM SDK sysroot path must be absolute",
        ),
        (
            paths.0,
            paths.1,
            paths.2,
            Path::new("proof"),
            "LLVM SDK output path must be absolute",
        ),
    ] {
        assert_rejection(
            application_command(
                program,
                fixture,
                &good_probe,
                &target,
                sdk,
                output,
                ("c", "original"),
            ),
            expected,
        );
    }
    assert_rejection(
        application_command(
            paths.0,
            paths.1,
            &good_probe,
            &target,
            paths.2,
            Path::new("/proof,ambiguous"),
            ("c", "original"),
        ),
        "LLVM SDK output path is not portable for a driver argument",
    );

    for (source, language, expected) in [
        (
            "../outside.c",
            "c",
            "LLVM SDK probe source must be a bounded portable relative path",
        ),
        (
            "app.cpp",
            "c",
            "LLVM SDK probe source suffix does not match its selected language",
        ),
        (
            "-Wl,--injected.c",
            "c",
            "LLVM SDK probe source must be a bounded portable relative path",
        ),
    ] {
        assert_rejection(
            application_command(
                paths.0,
                Path::new("/source/app.c"),
                &probe(source, &[]),
                &target,
                paths.2,
                paths.3,
                (language, "original"),
            ),
            expected,
        );
    }

    assert_rejection(
        application_command(
            paths.0,
            Path::new("/source/different.c"),
            &good_probe,
            &target,
            paths.2,
            paths.3,
            ("c", "original"),
        ),
        "LLVM SDK fixture basename differs from the sealed probe source",
    );

    for libraries in [
        vec![String::new()],
        vec!["-Wl,--as-needed".to_owned()],
        vec!["a".repeat(MAX_LIBRARY_BYTES + 1)],
        vec!["duplicate".to_owned(), "duplicate".to_owned()],
        (0..=MAX_ADDITIONAL_LIBRARIES)
            .map(|index| format!("library{index}"))
            .collect(),
    ] {
        let expected = if libraries.len() > MAX_ADDITIONAL_LIBRARIES {
            "LLVM SDK probe libraries exceed the 64-item bound"
        } else if libraries.len() > 1 && libraries[0] == libraries[1] {
            "LLVM SDK probe library names must be unique"
        } else {
            "LLVM SDK libraries must be bounded bare identifiers using ASCII letters, digits, _, + or -"
        };
        assert_rejection(
            application_command(
                paths.0,
                paths.1,
                &NativeSdkLinkProbe {
                    source: "app.c".into(),
                    libraries,
                },
                &target,
                paths.2,
                paths.3,
                ("c", "original"),
            ),
            expected,
        );
    }
}
