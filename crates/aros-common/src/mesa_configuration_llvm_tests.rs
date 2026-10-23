use super::{MesaLlvmConfiguration, MesaMakeVersionValue};
use serde_json::json;

#[test]
fn serializes_undefined_and_defined_empty_as_distinct_states() {
    assert_eq!(
        serde_json::to_value(MesaMakeVersionValue::Undefined).unwrap(),
        json!({ "state": "undefined" })
    );
    assert_eq!(
        serde_json::to_value(MesaMakeVersionValue::Defined(String::new())).unwrap(),
        json!({ "state": "defined", "value": "" })
    );

    let config = MesaLlvmConfiguration {
        opt_llvm: MesaMakeVersionValue::Undefined,
        target_llvm_ver: MesaMakeVersionValue::Defined(String::new()),
    };
    assert_eq!(config.defined_variables()["TARGET_LLVM_VER"], "");
    assert!(!config.defined_variables().contains_key("OPT_LLVM"));
    assert_eq!(config.undefined_variables(), vec!["OPT_LLVM"]);
}

#[test]
fn rejects_invalid_enum_shapes_unknown_fields_and_duplicate_members() {
    for invalid in [
        r#"{"state":"other"}"#,
        r#"{"state":"defined"}"#,
        r#"{"state":"defined","value":null}"#,
        r#"{"state":"defined","value":12}"#,
        r#"{"state":"undefined","value":""}"#,
        r#"{"state":"defined","value":"1.0","extra":true}"#,
        r#"{"state":"defined","state":"undefined","value":"1.0"}"#,
        r#"{"state":"defined","value":"1.0","value":"2.0"}"#,
    ] {
        assert!(
            serde_json::from_str::<MesaMakeVersionValue>(invalid).is_err(),
            "accepted invalid enum JSON: {invalid}"
        );
    }

    for invalid in [
        r#"{"target_llvm_ver":{"state":"undefined"}}"#,
        r#"{"opt_llvm":{"state":"undefined"}}"#,
        r#"{"opt_llvm":{"state":"undefined"},"target_llvm_ver":{"state":"undefined"},"extra":true}"#,
        r#"{"opt_llvm":{"state":"undefined"},"opt_llvm":{"state":"defined","value":"1.0"},"target_llvm_ver":{"state":"undefined"}}"#,
        r#"{"opt_llvm":{"state":"undefined"},"target_llvm_ver":{"state":"undefined"},"target_llvm_ver":{"state":"defined","value":"1.0"}}"#,
    ] {
        assert!(
            serde_json::from_str::<MesaLlvmConfiguration>(invalid).is_err(),
            "accepted invalid struct JSON: {invalid}"
        );
    }
}

#[test]
fn validates_only_empty_or_canonical_bounded_versions() {
    for valid in ["", "0.0", "11.0.0", "255.255.255"] {
        let config = MesaLlvmConfiguration {
            opt_llvm: MesaMakeVersionValue::Defined(valid.into()),
            target_llvm_ver: MesaMakeVersionValue::Undefined,
        };
        config.validate().unwrap();
    }

    for invalid in [
        "0",
        "1.2.3.4",
        ".1",
        "1.",
        "1..2",
        "01.2",
        "1.02.3",
        "256.0",
        "0.256.0",
        "1.2.256",
        " 1.2",
        "1.2\n",
        "1.2;echo",
        "$(touch)",
        "1.2.999999999999999999999999999999999999999999999999999999",
    ] {
        let config = MesaLlvmConfiguration {
            opt_llvm: MesaMakeVersionValue::Undefined,
            target_llvm_ver: MesaMakeVersionValue::Defined(invalid.into()),
        };
        assert!(
            config.validate().is_err(),
            "accepted unsupported version {invalid:?}"
        );
    }
}
