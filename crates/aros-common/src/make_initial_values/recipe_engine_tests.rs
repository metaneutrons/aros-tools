//! Tests for GNU Make's hidden CLI-propagation recipe engine value.

use super::super::{
    MakeInitialValueFlavor, ValidatedMakeInitialValues, ValidatedMakeParserControls,
    ValidatedMakeRecipeControls, MAKE_INITIAL_VALUES_METADATA_KEY,
};
use super::*;
use crate::make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin};

fn make_namespace(origin: Option<MakeVariableOrigin>) -> ValidatedMakeNamespace {
    let mut entries = [".VARIABLES", "MAKE_VERSION"]
        .map(|name| MakeNamespaceEntry {
            name: name.into(),
            origin: MakeVariableOrigin::Default,
        })
        .to_vec();
    if let Some(origin) = origin {
        entries.push(MakeNamespaceEntry {
            name: MAKE_CLI_PROPAGATION_VARIABLE.into(),
            origin,
        });
    }
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries,
    })
    .unwrap()
}

fn namespace_with_legacy_controls() -> ValidatedMakeNamespace {
    let mut entries = [
        (".VARIABLES", MakeVariableOrigin::Default),
        ("MAKE_VERSION", MakeVariableOrigin::Default),
        (MAKE_CLI_PROPAGATION_VARIABLE, MakeVariableOrigin::Automatic),
        (".SHELLFLAGS", MakeVariableOrigin::Default),
        (".RECIPEPREFIX", MakeVariableOrigin::Default),
        ("ORDINARY_DEFAULT", MakeVariableOrigin::Default),
    ]
    .map(|(name, origin)| MakeNamespaceEntry {
        name: name.into(),
        origin,
    })
    .to_vec();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries,
    })
    .unwrap()
}

fn observation(entries: Vec<MakeInitialValueObservation>) -> MakeInitialValuesObservation {
    MakeInitialValuesObservation {
        version: 1,
        entries,
    }
}

fn entry(
    value: &str,
    origin: MakeVariableOrigin,
    flavor: MakeInitialValueFlavor,
) -> MakeInitialValueObservation {
    MakeInitialValueObservation {
        name: MAKE_CLI_PROPAGATION_VARIABLE.into(),
        value: value.into(),
        flavor,
        origin,
    }
}

fn valid_engine_entry(value: &str) -> MakeInitialValueObservation {
    entry(
        value,
        MakeVariableOrigin::Automatic,
        MakeInitialValueFlavor::Simple,
    )
}

fn engine_packet(value: &str) -> String {
    format!("v1;{};end", engine_record(value))
}

fn engine_record(value: &str) -> String {
    format!("{MAKE_CLI_PROPAGATION_VARIABLE}|automatic|simple|{value}")
}

fn stdout_with_engine_packet(packet: &str) -> Vec<u8> {
    format!("unrelated=metadata\n{MAKE_RECIPE_ENGINE_VALUES_METADATA_KEY}={packet}\n").into_bytes()
}

#[test]
fn exact_hidden_engine_entry_round_trips_raw_measurement_and_stdout() {
    assert_eq!(
        MAKE_RECIPE_ENGINE_VALUES_METADATA_KEY,
        "AROS_MAKE_RECIPE_ENGINE_VALUES"
    );
    assert_eq!(MAKE_CLI_PROPAGATION_VARIABLE, "-*-command-variables-*-");

    let value = "private propagation	with|delimiters;percent%\nnext line";
    let namespace = make_namespace(Some(MakeVariableOrigin::Automatic));
    let raw = observation(vec![valid_engine_entry(value)]);

    let validated = ValidatedMakeRecipeEngineValues::validate(raw, &namespace).unwrap();
    let measured_entry = validated.entries().next().unwrap();
    assert_eq!(measured_entry.name, MAKE_CLI_PROPAGATION_VARIABLE);
    assert_eq!(measured_entry.value, value);
    assert_eq!(measured_entry.origin, MakeVariableOrigin::Automatic);
    assert_eq!(measured_entry.flavor, MakeInitialValueFlavor::Simple);
    assert!(!format!("{validated:?}").contains(value));
    assert!(!format!("{validated:?}").contains("private propagation"));

    let packet = engine_packet("private propagation%09with%7Cdelimiters%3Bpercent%25%0Anext line");
    let from_measurement =
        ValidatedMakeRecipeEngineValues::from_measurement(&packet, &namespace).unwrap();
    assert_eq!(from_measurement.entries().next().unwrap().value, value);
    assert!(!format!("{from_measurement:?}").contains("private propagation"));

    let from_stdout = ValidatedMakeRecipeEngineValues::from_make_stdout(
        &stdout_with_engine_packet(&packet),
        &namespace,
    )
    .unwrap();
    assert_eq!(from_stdout.entries().next().unwrap().value, value);
    assert!(!format!("{from_stdout:?}").contains("private propagation"));

    let legacy_key = format!("{MAKE_INITIAL_VALUES_METADATA_KEY}={packet}\n");
    assert!(
        ValidatedMakeRecipeEngineValues::from_make_stdout(legacy_key.as_bytes(), &namespace)
            .is_err()
    );
}

#[test]
fn empty_packet_is_valid_only_when_no_automatic_engine_value_is_eligible() {
    let absent = make_namespace(None);
    assert_eq!(
        ValidatedMakeRecipeEngineValues::validate(observation(Vec::new()), &absent)
            .unwrap()
            .entries()
            .count(),
        0
    );
    assert_eq!(
        ValidatedMakeRecipeEngineValues::from_measurement("v1;end", &absent)
            .unwrap()
            .entries()
            .count(),
        0
    );
    assert_eq!(
        ValidatedMakeRecipeEngineValues::from_make_stdout(
            &stdout_with_engine_packet("v1;end"),
            &absent,
        )
        .unwrap()
        .entries()
        .count(),
        0
    );
    assert!(ValidatedMakeRecipeEngineValues::validate(
        observation(vec![valid_engine_entry("unexpected")]),
        &absent
    )
    .is_err());

    let wrong_namespace_origin = make_namespace(Some(MakeVariableOrigin::File));
    assert!(ValidatedMakeRecipeEngineValues::validate(
        observation(vec![valid_engine_entry("not automatic")]),
        &wrong_namespace_origin
    )
    .is_err());
    assert!(
        ValidatedMakeRecipeEngineValues::from_measurement("v1;end", &wrong_namespace_origin)
            .is_err()
    );
}

#[test]
fn validation_requires_exact_membership_version_automatic_origin_and_simple_flavor() {
    let namespace = make_namespace(Some(MakeVariableOrigin::Automatic));
    let valid = valid_engine_entry("opaque cli propagation");

    let mut wrong_version = observation(vec![valid.clone()]);
    wrong_version.version = 2;
    assert!(ValidatedMakeRecipeEngineValues::validate(wrong_version, &namespace).is_err());
    assert!(
        ValidatedMakeRecipeEngineValues::validate(observation(Vec::new()), &namespace).is_err()
    );
    assert!(ValidatedMakeRecipeEngineValues::from_measurement("v1;end", &namespace).is_err());

    assert!(ValidatedMakeRecipeEngineValues::validate(
        observation(vec![valid.clone(), valid.clone()]),
        &namespace,
    )
    .is_err());
    assert!(ValidatedMakeRecipeEngineValues::from_measurement(
        &format!(
            "v1;{};{};end",
            engine_record("first"),
            engine_record("second")
        ),
        &namespace,
    )
    .is_err());

    let extra = MakeInitialValueObservation {
        name: "UNEXPECTED_ENGINE_VALUE".into(),
        value: "x".into(),
        flavor: MakeInitialValueFlavor::Simple,
        origin: MakeVariableOrigin::Automatic,
    };
    assert!(
        ValidatedMakeRecipeEngineValues::validate(observation(vec![valid, extra]), &namespace,)
            .is_err()
    );
    assert!(ValidatedMakeRecipeEngineValues::from_measurement(
        &format!(
            "v1;{};UNEXPECTED_ENGINE_VALUE|automatic|simple|x;end",
            engine_record("x")
        ),
        &namespace,
    )
    .is_err());

    assert!(ValidatedMakeRecipeEngineValues::validate(
        observation(vec![entry(
            "wrong origin",
            MakeVariableOrigin::Default,
            MakeInitialValueFlavor::Simple,
        )]),
        &namespace,
    )
    .is_err());
    assert!(ValidatedMakeRecipeEngineValues::from_measurement(
        &format!("v1;{MAKE_CLI_PROPAGATION_VARIABLE}|default|simple|x;end"),
        &namespace,
    )
    .is_err());

    assert!(ValidatedMakeRecipeEngineValues::validate(
        observation(vec![entry(
            "recursive is not admitted",
            MakeVariableOrigin::Automatic,
            MakeInitialValueFlavor::Recursive,
        )]),
        &namespace,
    )
    .is_err());
    assert!(ValidatedMakeRecipeEngineValues::from_measurement(
        &format!("v1;{MAKE_CLI_PROPAGATION_VARIABLE}|automatic|recursive|x;end"),
        &namespace,
    )
    .is_err());
}

#[test]
fn malformed_metadata_encoding_and_size_limits_are_refused() {
    let namespace = make_namespace(Some(MakeVariableOrigin::Automatic));
    for malformed in [
        "v2;end",
        "v1;end;trailing",
        "v1;OTHER|automatic|simple|x;end",
        "v1;-*-command-variables-*-|automatic|recursive|x;end",
        "v1;-*-command-variables-*-|environment|simple|x;end",
        "v1;-*-command-variables-*-|automatic|simple|bad%GG;end",
        "v1;-*-command-variables-*-|automatic|simple|bad%;end",
        "v1;-*-command-variables-*-|automatic|simple|bad%7c;end",
        "v1;-*-command-variables-*-|automatic|simple;end",
    ] {
        assert!(
            ValidatedMakeRecipeEngineValues::from_measurement(malformed, &namespace).is_err(),
            "malformed engine packet should be rejected: {malformed}"
        );
    }

    let oversized_raw = "x".repeat(64 * 1024 + 1);
    assert!(ValidatedMakeRecipeEngineValues::validate(
        observation(vec![valid_engine_entry(&oversized_raw)]),
        &namespace,
    )
    .is_err());

    let oversized_measurement = format!(
        "v1;{}|automatic|simple|{};end",
        MAKE_CLI_PROPAGATION_VARIABLE,
        "x".repeat(128 * 1024),
    );
    assert!(
        ValidatedMakeRecipeEngineValues::from_measurement(&oversized_measurement, &namespace)
            .is_err()
    );

    assert!(ValidatedMakeRecipeEngineValues::from_make_stdout(b"", &namespace).is_err());
    assert!(ValidatedMakeRecipeEngineValues::from_make_stdout(
        b"AROS_MAKE_RECIPE_ENGINE_VALUES=v1;end\nAROS_MAKE_RECIPE_ENGINE_VALUES=v1;end\n",
        &namespace,
    )
    .is_err());
    assert!(ValidatedMakeRecipeEngineValues::from_make_stdout(
        &vec![b'x'; 256 * 1024 + 1],
        &namespace,
    )
    .is_err());
    assert!(ValidatedMakeRecipeEngineValues::from_make_stdout(
        b"AROS_MAKE_RECIPE_ENGINE_VALUES=v1;-*-command-variables-*-|automatic|simple|%GG;end\n",
        &namespace,
    )
    .is_err());
}

#[test]
fn hidden_automatic_value_does_not_expand_legacy_packet_memberships() {
    let namespace = make_namespace(Some(MakeVariableOrigin::Automatic));
    let packet = engine_packet("opaque");

    assert!(ValidatedMakeInitialValues::from_measurement(&packet, &namespace).is_err());
    assert!(ValidatedMakeParserControls::from_measurement(&packet, &namespace).is_err());
    assert!(ValidatedMakeRecipeControls::from_measurement(&packet, &namespace).is_err());

    assert_eq!(
        ValidatedMakeInitialValues::from_measurement(
            "v1;MAKE_VERSION|default|simple|4.4;end",
            &namespace
        )
        .unwrap()
        .entries()
        .count(),
        1
    );
    assert_eq!(
        ValidatedMakeParserControls::from_measurement("v1;end", &namespace)
            .unwrap()
            .entries()
            .count(),
        0
    );
    assert_eq!(
        ValidatedMakeRecipeControls::from_measurement("v1;end", &namespace)
            .unwrap()
            .entries()
            .count(),
        0
    );

    let compatible = namespace_with_legacy_controls();
    assert_eq!(
        ValidatedMakeInitialValues::from_measurement(
            "v1;MAKE_VERSION|default|simple|4.4;ORDINARY_DEFAULT|default|simple|ordinary;end",
            &compatible,
        )
        .unwrap()
        .entries()
        .count(),
        2
    );
    assert_eq!(
        ValidatedMakeParserControls::from_measurement(
            "v1;.RECIPEPREFIX|default|simple|:;end",
            &compatible,
        )
        .unwrap()
        .entries()
        .count(),
        1
    );
    assert_eq!(
        ValidatedMakeRecipeControls::from_measurement(
            "v1;.SHELLFLAGS|default|recursive|-c;end",
            &compatible,
        )
        .unwrap()
        .entries()
        .count(),
        1
    );
}
