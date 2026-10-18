use super::{
    BuiltinSelectors, MesaConfigurationIdentity, MesaConfigurationWire, ValidatedMesaConfiguration,
    CONFIGURATION_FILE, MAX_CONFIGURATION_BYTES, MAX_TARGET_ISA_FLAGS,
    MESA_COMPILER_OPTION_VARIABLES,
};
use crate::{ArosCompilerIdentity, ArosError, Result, Sha256Digest};
use serde_json::{json, Value};

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::parse(&byte.to_string().repeat(64)).unwrap()
}

fn identity(profile: &str, triple: &str) -> MesaConfigurationIdentity {
    MesaConfigurationIdentity {
        target_profile: profile.into(),
        target_triple: triple.into(),
        compiler: ArosCompilerIdentity::Llvm {
            version: "18.1.0".into(),
        },
        compiler_tree_sha256: digest('a'),
        source_inputs_sha256: digest('b'),
        configure_invocation_sha256: digest('c'),
    }
}

fn selectors(value: bool) -> BuiltinSelectors {
    let value = value.into();
    BuiltinSelectors {
        ffs: value,
        ffsll: value,
        bswap32: value,
        bswap64: value,
        clz: value,
        clzll: value,
        ctz: value,
        expect: value,
        popcount: value,
        popcountll: value,
        unreachable: value,
    }
}

fn wire(
    identity: &MesaConfigurationIdentity,
    target_isa_flags: Vec<String>,
    builtin_selectors: BuiltinSelectors,
) -> MesaConfigurationWire {
    MesaConfigurationWire {
        version: 1,
        identity: identity.clone(),
        target_isa_flags,
        builtin_selectors,
        compiler_options: MESA_COMPILER_OPTION_VARIABLES
            .iter()
            .map(|name| ((*name).into(), Vec::new()))
            .collect(),
        mesa_compile_options: Vec::new(),
    }
}

fn encoded(wire: &MesaConfigurationWire) -> Vec<u8> {
    serde_json::to_vec(wire).unwrap()
}

fn configuration_error(result: Result<ValidatedMesaConfiguration>) -> String {
    match result {
        Err(ArosError::Configuration { file, message }) => {
            assert_eq!(file, CONFIGURATION_FILE);
            message
        }
        Err(error) => panic!("expected a configuration error, got {error}"),
        Ok(_) => panic!("expected configuration parsing to fail"),
    }
}

#[test]
fn accepts_ordered_duplicate_arm_atoms_and_empty_aarch64_flags() {
    let arm = identity("armv7", "armv7-unknown-aros");
    let arm_bytes = encoded(&wire(
        &arm,
        vec!["-Dvalue=1".into(), "-Uvalue".into(), "-Dvalue=1".into()],
        selectors(false),
    ));
    let parsed = ValidatedMesaConfiguration::parse_bound(&arm_bytes, &arm).unwrap();
    assert_eq!(
        parsed
            .target_isa_flags()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["-Dvalue=1", "-Uvalue", "-Dvalue=1"]
    );
    assert_eq!(
        parsed.make_variables().get("TARGET_ISA_CFLAGS").unwrap(),
        "-Dvalue=1 -Uvalue -Dvalue=1"
    );

    let aarch64 = identity("aarch64", "aarch64-unknown-aros");
    let empty_bytes = encoded(&wire(&aarch64, vec![], selectors(false)));
    let empty = ValidatedMesaConfiguration::parse_bound(&empty_bytes, &aarch64).unwrap();
    assert!(empty.target_isa_flags().is_empty());
    assert_eq!(empty.make_variables()["TARGET_ISA_CFLAGS"], "");
}

#[test]
fn maps_all_eleven_boolean_selectors_to_fixed_options_or_empty_values() {
    let expected = identity("aarch64", "aarch64-unknown-aros");
    let enabled = ValidatedMesaConfiguration::parse_bound(
        &encoded(&wire(&expected, vec![], selectors(true))),
        &expected,
    )
    .unwrap();
    let enabled_variables = enabled.make_variables();
    let pairs = [
        ("CFLAGS_NO_BUILTIN_FFS", "-fno-builtin-ffs"),
        ("CFLAGS_NO_BUILTIN_FFSLL", "-fno-builtin-ffsll"),
        ("CFLAGS_NO_BUILTIN_BSWAP32", "-fno-builtin-bswap32"),
        ("CFLAGS_NO_BUILTIN_BSWAP64", "-fno-builtin-bswap64"),
        ("CFLAGS_NO_BUILTIN_CLZ", "-fno-builtin-clz"),
        ("CFLAGS_NO_BUILTIN_CLZLL", "-fno-builtin-clzll"),
        ("CFLAGS_NO_BUILTIN_CTZ", "-fno-builtin-ctz"),
        ("CFLAGS_NO_BUILTIN_EXPECT", "-fno-builtin-expect"),
        ("CFLAGS_NO_BUILTIN_POPCOUNT", "-fno-builtin-popcount"),
        ("CFLAGS_NO_BUILTIN_POPCOUNTLL", "-fno-builtin-popcountll"),
        ("CFLAGS_NO_BUILTIN_UNREACHABLE", "-fno-builtin-unreachable"),
    ];
    assert_eq!(
        enabled_variables.len(),
        13 + MESA_COMPILER_OPTION_VARIABLES.len()
    );
    for (name, option) in pairs {
        assert_eq!(enabled_variables[name], option, "selector {name}");
    }

    let disabled = ValidatedMesaConfiguration::parse_bound(
        &encoded(&wire(&expected, vec![], selectors(false))),
        &expected,
    )
    .unwrap();
    let disabled_variables = disabled.make_variables();
    assert_eq!(
        disabled_variables.len(),
        13 + MESA_COMPILER_OPTION_VARIABLES.len()
    );
    for (name, _) in pairs {
        assert_eq!(disabled_variables[name], "", "selector {name}");
    }
}

#[test]
fn rejects_mismatch_in_every_identity_field() {
    let expected = identity("aarch64", "aarch64-unknown-aros");
    let bytes = encoded(&wire(&expected, vec![], selectors(false)));
    let mut mismatches = Vec::new();

    let mut changed = expected.clone();
    changed.target_profile = "aarch64-debug".into();
    mismatches.push(changed);

    let mut changed = expected.clone();
    changed.target_triple = "arm-unknown-aros".into();
    mismatches.push(changed);

    let mut changed = expected.clone();
    changed.compiler = ArosCompilerIdentity::Llvm {
        version: "18.1.1".into(),
    };
    mismatches.push(changed);

    let mut changed = expected.clone();
    changed.compiler_tree_sha256 = digest('d');
    mismatches.push(changed);

    let mut changed = expected.clone();
    changed.source_inputs_sha256 = digest('e');
    mismatches.push(changed);

    let mut changed = expected;
    changed.configure_invocation_sha256 = digest('f');
    mismatches.push(changed);

    assert_eq!(mismatches.len(), 6);
    for mismatched_expected in mismatches {
        let message = configuration_error(ValidatedMesaConfiguration::parse_bound(
            &bytes,
            &mismatched_expected,
        ));
        assert!(
            message.contains("does not match"),
            "wrong rejection reason: {message}"
        );
    }
}

#[test]
fn rejects_unsupported_version_and_missing_required_fields() {
    let expected = identity("aarch64", "aarch64-unknown-aros");
    let valid = wire(&expected, vec![], selectors(false));

    let mut unsupported = valid.clone();
    unsupported.version = 2;
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        &encoded(&unsupported),
        &expected,
    ))
    .contains("unsupported Mesa configuration version 2"));

    let mut missing_isa: Value = serde_json::from_slice(&encoded(&valid)).unwrap();
    missing_isa
        .as_object_mut()
        .unwrap()
        .remove("target_isa_flags");
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        &serde_json::to_vec(&missing_isa).unwrap(),
        &expected,
    ))
    .contains("missing field `target_isa_flags`"));

    let all_selectors = serde_json::to_value(&valid.builtin_selectors).unwrap();
    for name in all_selectors.as_object().unwrap().keys() {
        let mut missing_selector: Value = serde_json::from_slice(&encoded(&valid)).unwrap();
        missing_selector["builtin_selectors"]
            .as_object_mut()
            .unwrap()
            .remove(name);
        let message = configuration_error(ValidatedMesaConfiguration::parse_bound(
            &serde_json::to_vec(&missing_selector).unwrap(),
            &expected,
        ));
        assert!(
            message.contains(&format!("missing field `{name}`")),
            "{message}"
        );
    }
}

#[test]
fn rejects_extra_and_duplicate_json_fields() {
    let expected = identity("aarch64", "aarch64-unknown-aros");
    let valid = encoded(&wire(&expected, vec![], selectors(false)));

    let mut extra: Value = serde_json::from_slice(&valid).unwrap();
    extra["unexpected"] = json!(true);
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        &serde_json::to_vec(&extra).unwrap(),
        &expected,
    ))
    .contains("unknown field `unexpected`"));

    let text = String::from_utf8(valid).unwrap();
    let duplicated = text.replacen("\"version\":1,", "\"version\":1,\"version\":1,", 1);
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        duplicated.as_bytes(),
        &expected,
    ))
    .contains("duplicate field `version`"));

    let duplicate_selector = text.replacen(
        "\"CFLAGS_NO_BUILTIN_FFS\":false",
        "\"CFLAGS_NO_BUILTIN_FFS\":false,\"CFLAGS_NO_BUILTIN_FFS\":true",
        1,
    );
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        duplicate_selector.as_bytes(),
        &expected,
    ))
    .contains("duplicate field `CFLAGS_NO_BUILTIN_FFS`"));

    let wrong_type = text.replacen(
        "\"CFLAGS_NO_BUILTIN_FFS\":false",
        "\"CFLAGS_NO_BUILTIN_FFS\":\"false\"",
        1,
    );
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        wrong_type.as_bytes(),
        &expected,
    ))
    .contains("expected a boolean"));
}

#[test]
fn rejects_invalid_expected_identity_and_preserves_gnu_target_validation() {
    let mut invalid = identity("aarch64", "aarch64-unknown-aros");
    invalid.target_profile.clear();
    let valid_bytes = encoded(&wire(
        &identity("aarch64", "aarch64-unknown-aros"),
        vec![],
        selectors(false),
    ));
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        &valid_bytes,
        &invalid,
    ))
    .contains("target_profile must be nonempty"));

    let mut invalid_profile_length = identity("aarch64", "aarch64-unknown-aros");
    invalid_profile_length.target_profile = "p".repeat(65);
    let message = configuration_error(ValidatedMesaConfiguration::parse_bound(
        &valid_bytes,
        &invalid_profile_length,
    ));
    assert!(
        message.contains("target_profile must be nonempty ASCII identifier text"),
        "wrong profile validation error: {message}"
    );

    let mut invalid_profile_character = identity("aarch64", "aarch64-unknown-aros");
    invalid_profile_character.target_profile = "bad/name".into();
    let message = configuration_error(ValidatedMesaConfiguration::parse_bound(
        &valid_bytes,
        &invalid_profile_character,
    ));
    assert!(
        message.contains("target_profile must be nonempty ASCII identifier text"),
        "wrong profile validation error: {message}"
    );

    let mut invalid_triple_length = identity("aarch64", "aarch64-unknown-aros");
    invalid_triple_length.target_triple = "t".repeat(129);
    let message = configuration_error(ValidatedMesaConfiguration::parse_bound(
        &valid_bytes,
        &invalid_triple_length,
    ));
    assert!(
        message.contains("target_triple must be nonempty ASCII identifier text"),
        "wrong triple validation error: {message}"
    );

    let mut invalid_triple_character = identity("aarch64", "aarch64-unknown-aros");
    invalid_triple_character.target_triple = "aarch64-unknown-aros\n".into();
    let message = configuration_error(ValidatedMesaConfiguration::parse_bound(
        &valid_bytes,
        &invalid_triple_character,
    ));
    assert!(
        message.contains("target_triple must be nonempty ASCII identifier text"),
        "wrong triple validation error: {message}"
    );

    let mut invalid_gnu = identity("riscv64", "riscv-unknown-aros");
    let target = serde_json::from_value(json!({
        "schema": "aros-riscv-target-v1",
        "isa": "rva22u64",
        "abi": "lp64d",
        "code_model": "medany",
        "architecture": "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
        "unaligned_access": false,
        "atomic_abi": 0,
        "x3_reg_usage": 0
    }))
    .unwrap();
    invalid_gnu.compiler = ArosCompilerIdentity::Gnu {
        gcc_version: "16.2.0".into(),
        binutils_version: "2.47".into(),
        target,
    };
    let message = configuration_error(ValidatedMesaConfiguration::parse_bound(
        &valid_bytes,
        &invalid_gnu,
    ));
    assert!(
        message.contains("CPU differs from the target ABI width"),
        "wrong GNU target validation error: {message}"
    );
}

#[test]
fn rejects_unsafe_empty_and_overlong_isa_atoms() {
    let expected = identity("aarch64", "aarch64-unknown-aros");
    for unsafe_atom in [
        "-DNAME=has space",
        "-DNAME=x;touch",
        "`command`",
        "$VALUE",
        "@file",
    ] {
        let bytes = encoded(&wire(&expected, vec![unsafe_atom.into()], selectors(false)));
        let message =
            configuration_error(ValidatedMesaConfiguration::parse_bound(&bytes, &expected));
        assert!(
            message.contains("unsafe syntax"),
            "wrong rejection for atom {unsafe_atom:?}: {message}"
        );
    }

    for unsafe_atom in ["\"quoted\"", "-DNAME=x\n", ";"] {
        let bytes = encoded(&wire(&expected, vec![unsafe_atom.into()], selectors(false)));
        let message =
            configuration_error(ValidatedMesaConfiguration::parse_bound(&bytes, &expected));
        assert!(
            message.contains("unsafe syntax"),
            "wrong rejection for atom {unsafe_atom:?}: {message}"
        );
    }

    let empty = encoded(&wire(&expected, vec![String::new()], selectors(false)));
    assert!(
        configuration_error(ValidatedMesaConfiguration::parse_bound(&empty, &expected,))
            .contains("atom 0 is empty")
    );

    let overlong = encoded(&wire(&expected, vec!["a".repeat(257)], selectors(false)));
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        &overlong, &expected,
    ))
    .contains("atom 0 is empty, too long"));
}

#[test]
fn rejects_oversized_documents_and_too_many_flag_atoms() {
    let expected = identity("aarch64", "aarch64-unknown-aros");
    let oversized = vec![b' '; MAX_CONFIGURATION_BYTES + 1];
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        &oversized, &expected,
    ))
    .contains("input exceeds the 32768-byte limit"));

    let too_many_flags = vec!["-D1".into(); MAX_TARGET_ISA_FLAGS + 1];
    let bytes = encoded(&wire(&expected, too_many_flags, selectors(false)));
    let message = configuration_error(ValidatedMesaConfiguration::parse_bound(&bytes, &expected));
    assert!(
        message.contains("exceeds its 64-atom limit"),
        "wrong count rejection reason: {message}"
    );
}

#[test]
fn compiler_options_preserve_order_and_require_every_known_empty_value() {
    let expected = identity("arm-raspi", "arm-unknown-aros");
    let mut configured = wire(&expected, vec![], selectors(false));
    let mut absent: Value = serde_json::from_slice(&encoded(&configured)).unwrap();
    absent.as_object_mut().unwrap().remove("compiler_options");
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        &serde_json::to_vec(&absent).unwrap(),
        &expected
    ))
    .contains("missing field `compiler_options`"));
    configured.compiler_options.insert(
        "CFLAGS_GNU11".into(),
        vec![
            "-std=gnu11".into(),
            "-DORDER=1".into(),
            "-UORDER".into(),
            "-DORDER=1".into(),
        ],
    );
    let parsed = ValidatedMesaConfiguration::parse_bound(&encoded(&configured), &expected).unwrap();
    assert_eq!(
        parsed.make_variables()["CFLAGS_GNU11"],
        "-std=gnu11 -DORDER=1 -UORDER -DORDER=1"
    );
    assert_eq!(parsed.make_variables()["CXXFLAGS_LIFETIME_DSE1"], "");
    for name in MESA_COMPILER_OPTION_VARIABLES {
        let mut missing = configured.clone();
        missing.compiler_options.remove(*name);
        assert!(
            configuration_error(ValidatedMesaConfiguration::parse_bound(
                &encoded(&missing),
                &expected
            ))
            .contains("every reviewed Mesa compiler variable"),
            "missing {name}"
        );
    }
    let mut extra = configured.clone();
    extra.compiler_options.insert("UNREVIEWED".into(), vec![]);
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        &encoded(&extra),
        &expected
    ))
    .contains("and no others"));
    let text = String::from_utf8(encoded(&configured)).unwrap();
    let duplicate = text.replace(
        "\"compiler_options\":{",
        "\"compiler_options\":{\"CFLAGS_GNU11\":[],",
    );
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        duplicate.as_bytes(),
        &expected
    ))
    .contains("duplicate compiler-option variable CFLAGS_GNU11"));
    configured
        .compiler_options
        .insert("CFLAGS_GNU11".into(), vec!["@untrusted".into()]);
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        &encoded(&configured),
        &expected
    ))
    .contains("unsafe syntax"));
}

#[test]
fn optional_make_bundle_is_explicitly_measured_not_implicitly_empty() {
    let expected = identity("arm-raspi", "arm-unknown-aros");
    let mut configured = wire(&expected, vec![], selectors(false));
    let empty = ValidatedMesaConfiguration::parse_bound(&encoded(&configured), &expected).unwrap();
    assert_eq!(empty.make_variables()["MESA_COMPILEOPTIONS"], "");
    configured.mesa_compile_options =
        vec!["-DVALUE=1".into(), "-UVALUE".into(), "-DVALUE=1".into()];
    let nonempty =
        ValidatedMesaConfiguration::parse_bound(&encoded(&configured), &expected).unwrap();
    assert_eq!(
        nonempty.make_variables()["MESA_COMPILEOPTIONS"],
        "-DVALUE=1 -UVALUE -DVALUE=1"
    );
    let mut missing: Value = serde_json::from_slice(&encoded(&configured)).unwrap();
    missing
        .as_object_mut()
        .unwrap()
        .remove("mesa_compile_options");
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        &serde_json::to_vec(&missing).unwrap(),
        &expected
    ))
    .contains("missing field `mesa_compile_options`"));
    configured.mesa_compile_options = vec!["@untrusted".into()];
    assert!(configuration_error(ValidatedMesaConfiguration::parse_bound(
        &encoded(&configured),
        &expected
    ))
    .contains("unsafe syntax"));
}
