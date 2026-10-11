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
    validate_target(&target64()).unwrap();
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
                REQUIRED_COLLECTOR_SYMBOL,
            ),
            &target,
        )
        .unwrap();
    }

    let target = target32();
    let wrong_class = elf_fixture(32, 243, 1, REQUIRED_COLLECTOR_SYMBOL);
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
        verify_application_elf(&elf_fixture(32, 244, 1, REQUIRED_COLLECTOR_SYMBOL), &target),
        "LLVM SDK application ELF class or machine differs from the source target",
    );
    assert_rejection(
        verify_application_elf(&elf_fixture(32, 243, 0, REQUIRED_COLLECTOR_SYMBOL), &target),
        "LLVM SDK application ELF flags differ from the source mask/value",
    );

    let mut wrong_osabi = elf_fixture(32, 243, 1, REQUIRED_COLLECTOR_SYMBOL);
    wrong_osabi[7] = 0;
    assert_rejection(
        verify_application_elf(&wrong_osabi, &target),
        "LLVM SDK application ELF lacks the selected AROS ABI marking",
    );

    let mut wrong_version = elf_fixture(32, 243, 1, REQUIRED_COLLECTOR_SYMBOL);
    wrong_version[8] = AROS_ABI_VERSION + 1;
    assert_rejection(
        verify_application_elf(&wrong_version, &target),
        "LLVM SDK application ELF lacks the selected AROS ABI marking",
    );

    let mut wrong_type = elf_fixture(32, 243, 1, REQUIRED_COLLECTOR_SYMBOL);
    wrong_type[16..18].copy_from_slice(&2_u16.to_le_bytes());
    assert_rejection(
        verify_application_elf(&wrong_type, &target),
        "LLVM SDK application output is not an ET_REL object",
    );

    assert_rejection(
        verify_application_elf(&elf_fixture(32, 243, 1, "OTHER_SYMBOL"), &target),
        "LLVM SDK application ELF lacks its required defined collector symbol",
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
