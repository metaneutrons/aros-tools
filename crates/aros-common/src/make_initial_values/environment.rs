//! Opt-in, bounded observations of GNU Make environment-origin variable values.
//!
//! The caller must provide the complete expected environment-origin name set.
//! This validates supplied observations only; it does not read the process
//! environment or establish who produced the namespace or measurement.

use std::collections::{BTreeMap, BTreeSet};

use crate::make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace};

use super::{MakeInitialValueObservation, MakeInitialValuesObservation, ValueSelection};

const MAX_ENTRIES: usize = 4096;
const MAX_NAME_BYTES: usize = 128;
const RESERVED_HELPERS: [&str; 2] = [
    "__AROS_ENVIRONMENT_VALUES_ITEM",
    "__AROS_ENVIRONMENT_VALUES_NL",
];
const CONFIGURATION_FILE: &str = "Make environment values";

/// Metadata key for explicitly requested environment-origin values.
pub const MAKE_ENVIRONMENT_VALUES_METADATA_KEY: &str = "AROS_MAKE_ENVIRONMENT_VALUES";

/// A validated, deterministic map of all expected environment-origin values.
///
/// Expected names must exactly match the environment-origin entries in the
/// measured namespace. Environment-override origin is refused. This type does
/// not read the process environment, authenticate the observation producer or
/// bind values to an execution receipt.
#[derive(Clone, Eq, PartialEq)]
pub struct ValidatedMakeEnvironmentValues {
    entries: BTreeMap<String, MakeInitialValueObservation>,
}

impl std::fmt::Debug for ValidatedMakeEnvironmentValues {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedMakeEnvironmentValues")
            .field("entry_count", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl ValidatedMakeEnvironmentValues {
    /// Validate raw values against the complete expected environment name set.
    ///
    /// # Errors
    /// Rejects incomplete or extra membership, unsafe names, refused origins,
    /// invalid raw values and protocol-bound violations.
    pub fn validate(
        raw: MakeInitialValuesObservation,
        namespace: &ValidatedMakeNamespace,
        expected: &BTreeSet<String>,
    ) -> crate::Result<Self> {
        validate_expected_names(namespace, expected)?;
        let entries = super::validate_entries(raw, namespace, ValueSelection::Environment)?;
        validate_entry_membership(&entries, expected)?;
        Ok(Self { entries })
    }

    /// Parse the bounded `v1;NAME|environment|flavor|value;...;end` form.
    ///
    /// Values use the existing initial-value percent encoding. The complete
    /// expected name set is checked against the namespace before parsing.
    ///
    /// # Errors
    /// Rejects inconsistent membership, malformed framing, unsafe names,
    /// refused origins/controls, invalid escapes and measurements over bounds.
    pub fn from_measurement(
        measurement: &str,
        namespace: &ValidatedMakeNamespace,
        expected: &BTreeSet<String>,
    ) -> crate::Result<Self> {
        validate_expected_names(namespace, expected)?;
        let entries =
            super::parse_measurement(measurement, namespace, ValueSelection::Environment)?;
        validate_entry_membership(&entries, expected)?;
        Ok(Self { entries })
    }

    /// Select and parse exactly one environment-values metadata line.
    ///
    /// # Errors
    /// Rejects oversized or invalid UTF-8 stdout, missing or duplicate metadata,
    /// and every malformed or refused measurement described by
    /// [`Self::from_measurement`].
    pub fn from_make_stdout(
        bytes: &[u8],
        namespace: &ValidatedMakeNamespace,
        expected: &BTreeSet<String>,
    ) -> crate::Result<Self> {
        validate_expected_names(namespace, expected)?;
        let measurement = super::metadata_measurement(bytes, MAKE_ENVIRONMENT_VALUES_METADATA_KEY)?;
        let entries =
            super::parse_measurement(measurement, namespace, ValueSelection::Environment)?;
        validate_entry_membership(&entries, expected)?;
        Ok(Self { entries })
    }

    /// Return a stable, name-sorted raw representation. Explicit field access and
    /// serialization include raw values; keep that data private rather than logging it.
    #[must_use]
    pub fn to_observation(&self) -> MakeInitialValuesObservation {
        MakeInitialValuesObservation {
            version: 1,
            entries: self.entries.values().cloned().collect(),
        }
    }

    /// Iterate over validated entries in name order.
    pub fn entries(&self) -> impl Iterator<Item = &MakeInitialValueObservation> {
        self.entries.values()
    }
}

fn validate_expected_names(
    namespace: &ValidatedMakeNamespace,
    expected: &BTreeSet<String>,
) -> crate::Result<()> {
    validate_environment_names(expected)?;

    let mut namespace_names = BTreeSet::new();
    for entry in namespace.to_observation().entries {
        match entry.origin {
            MakeVariableOrigin::Environment => {
                validate_environment_name(&entry.name)?;
                namespace_names.insert(entry.name);
            }
            MakeVariableOrigin::EnvironmentOverride => {
                return Err(environment_values_error(
                    "environment-override origin is unsupported",
                ));
            }
            _ => {}
        }
    }
    if namespace_names != *expected {
        return Err(environment_values_error(
            "expected names do not match the complete environment namespace",
        ));
    }

    Ok(())
}

/// Validate a complete set of names for safe observer-fragment interpolation.
pub fn validate_environment_names(expected: &BTreeSet<String>) -> crate::Result<()> {
    if expected.len() > MAX_ENTRIES {
        return Err(environment_values_error(
            "expected name count exceeds its limit",
        ));
    }
    for name in expected {
        validate_environment_name(name)?;
    }

    Ok(())
}

fn validate_environment_name(name: &str) -> crate::Result<()> {
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || RESERVED_HELPERS.contains(&name)
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(environment_values_error(
            "environment namespace contains an unsupported name",
        ));
    }
    Ok(())
}

fn validate_entry_membership(
    entries: &BTreeMap<String, MakeInitialValueObservation>,
    expected: &BTreeSet<String>,
) -> crate::Result<()> {
    if entries.len() != expected.len() || entries.keys().any(|name| !expected.contains(name)) {
        return Err(environment_values_error(
            "measurement does not contain every expected environment name",
        ));
    }
    Ok(())
}

fn environment_values_error(message: &str) -> crate::ArosError {
    crate::ArosError::Configuration {
        file: CONFIGURATION_FILE.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation};

    fn namespace(entries: &[(&str, MakeVariableOrigin)]) -> ValidatedMakeNamespace {
        let mut all = vec![
            MakeNamespaceEntry {
                name: ".VARIABLES".into(),
                origin: MakeVariableOrigin::Default,
            },
            MakeNamespaceEntry {
                name: "MAKE_VERSION".into(),
                origin: MakeVariableOrigin::Default,
            },
        ];
        all.extend(entries.iter().map(|(name, origin)| MakeNamespaceEntry {
            name: (*name).into(),
            origin: *origin,
        }));
        ValidatedMakeNamespace::validate(MakeNamespaceObservation {
            version: 1,
            entries: all,
        })
        .unwrap()
    }

    fn expected(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn entry(
        name: &str,
        value: &str,
        flavor: super::super::MakeInitialValueFlavor,
        origin: MakeVariableOrigin,
    ) -> MakeInitialValueObservation {
        MakeInitialValueObservation {
            name: name.into(),
            value: value.into(),
            flavor,
            origin,
        }
    }

    fn observation(entries: Vec<MakeInitialValueObservation>) -> MakeInitialValuesObservation {
        MakeInitialValuesObservation {
            version: 1,
            entries,
        }
    }

    #[test]
    fn measurement_preserves_raw_values_flavors_and_redacts_debug() {
        let namespace = namespace(&[
            ("SYNTHETIC_ALPHA", MakeVariableOrigin::Environment),
            ("1SYNTHETIC-B", MakeVariableOrigin::Environment),
        ]);
        let values = ValidatedMakeEnvironmentValues::from_measurement(
            "v1;1SYNTHETIC-B|environment|simple|literal%25; SYNTHETIC_ALPHA|environment|recursive|  $$(NEXT)%7C%3B%25%0A%09  ;end",
            &namespace,
            &expected(&["SYNTHETIC_ALPHA", "1SYNTHETIC-B"]),
        )
        .unwrap();
        let entries = values.to_observation().entries;
        assert_eq!(entries[0].name, "1SYNTHETIC-B");
        assert_eq!(entries[0].value, "literal%");
        assert_eq!(
            entries[0].flavor,
            super::super::MakeInitialValueFlavor::Simple
        );
        assert_eq!(entries[1].name, "SYNTHETIC_ALPHA");
        assert_eq!(entries[1].value, "  $$(NEXT)|;%\n\t  ");
        assert_eq!(
            entries[1].flavor,
            super::super::MakeInitialValueFlavor::Recursive
        );
        assert!(entries
            .iter()
            .all(|value| value.origin == MakeVariableOrigin::Environment));

        let debug = format!("{values:?}");
        assert!(!debug.contains("NEXT"));
        assert!(!debug.contains("literal%"));
        let raw_debug = format!("{:?}", values.to_observation());
        assert!(!raw_debug.contains("NEXT"));
        assert!(!raw_debug.contains("literal%"));
    }

    #[test]
    fn expected_names_and_measurement_must_match_complete_environment_membership() {
        let namespace = namespace(&[
            ("SYNTHETIC_A", MakeVariableOrigin::Environment),
            ("SYNTHETIC_B", MakeVariableOrigin::Environment),
        ]);
        let both = expected(&["SYNTHETIC_A", "SYNTHETIC_B"]);

        assert!(ValidatedMakeEnvironmentValues::from_measurement(
            "v1;SYNTHETIC_A|environment|simple|a;end",
            &namespace,
            &both,
        )
        .is_err());
        assert!(ValidatedMakeEnvironmentValues::from_measurement(
            "v1;SYNTHETIC_A|environment|simple|a;SYNTHETIC_B|environment|simple|b;end",
            &namespace,
            &expected(&["SYNTHETIC_A"]),
        )
        .is_err());
        assert!(ValidatedMakeEnvironmentValues::from_measurement(
            "v1;SYNTHETIC_A|environment|simple|a;SYNTHETIC_B|file|simple|b;end",
            &namespace,
            &both,
        )
        .is_err());
        assert!(ValidatedMakeEnvironmentValues::from_measurement(
            "v1;SYNTHETIC_A|environment|simple|a;SYNTHETIC_B|environment|simple|b;end",
            &namespace,
            &expected(&["SYNTHETIC_A", "SYNTHETIC_B", "SYNTHETIC_EXTRA"]),
        )
        .is_err());
    }

    #[test]
    fn refuses_environment_override_and_legacy_file_default_origins() {
        let override_namespace = namespace(&[(
            "SYNTHETIC_OVERRIDE",
            MakeVariableOrigin::EnvironmentOverride,
        )]);
        assert!(ValidatedMakeEnvironmentValues::from_measurement(
            "v1;end",
            &override_namespace,
            &BTreeSet::new(),
        )
        .is_err());

        let file_namespace = namespace(&[("SYNTHETIC_FILE", MakeVariableOrigin::File)]);
        assert!(ValidatedMakeEnvironmentValues::from_measurement(
            "v1;SYNTHETIC_FILE|file|simple|synthetic-file-value;end",
            &file_namespace,
            &BTreeSet::new(),
        )
        .is_err());

        let environment_namespace =
            namespace(&[("SYNTHETIC_ENV", MakeVariableOrigin::Environment)]);
        assert!(super::super::ValidatedMakeInitialValues::from_measurement(
            "v1;SYNTHETIC_ENV|environment|simple|synthetic-env-value;end",
            &environment_namespace,
        )
        .is_err());
    }

    #[test]
    fn refuses_unsafe_and_reserved_names_before_reading_measurement_values() {
        for name in [
            "SYNTHETIC.BAD",
            ".SYNTHETIC_HIDDEN",
            "__AROS_ENVIRONMENT_VALUES_ITEM",
            "__AROS_ENVIRONMENT_VALUES_NL",
        ] {
            let namespace = namespace(&[(name, MakeVariableOrigin::Environment)]);
            let error = ValidatedMakeEnvironmentValues::from_measurement(
                "not-a-measurement-containing-synthetic-secret",
                &namespace,
                &expected(&[name]),
            )
            .unwrap_err()
            .to_string();
            assert!(!error.contains("synthetic-secret"));
        }

        let too_many = (0..=MAX_ENTRIES)
            .map(|index| format!("SYNTHETIC_{index}"))
            .collect::<BTreeSet<_>>();
        assert!(validate_environment_names(&too_many).is_err());
        assert!(
            validate_environment_names(&BTreeSet::from(["N".repeat(MAX_NAME_BYTES + 1)])).is_err()
        );
    }

    #[test]
    fn stdout_selects_one_metadata_line_and_refuses_duplicate_or_oversized_data() {
        let namespace = namespace(&[("SYNTHETIC_A", MakeVariableOrigin::Environment)]);
        let names = expected(&["SYNTHETIC_A"]);
        let key = MAKE_ENVIRONMENT_VALUES_METADATA_KEY;
        assert!(ValidatedMakeEnvironmentValues::from_make_stdout(
            format!("OTHER=ignored\n{key}=v1;SYNTHETIC_A|environment|simple|value;end\n")
                .as_bytes(),
            &namespace,
            &names,
        )
        .is_ok());
        assert!(ValidatedMakeEnvironmentValues::from_make_stdout(
            format!("{key}=v1;SYNTHETIC_A|environment|simple|x;end\n{key}=v1;end\n").as_bytes(),
            &namespace,
            &names,
        )
        .is_err());
        assert!(ValidatedMakeEnvironmentValues::from_make_stdout(
            b"AROS_MAKE_INITIAL_VALUES=v1;end\n",
            &namespace,
            &names,
        )
        .is_err());

        let oversized = format!(
            "v1;SYNTHETIC_A|environment|simple|{};end",
            "x".repeat(super::super::MAX_RAW_VALUE_BYTES + 1)
        );
        assert!(
            ValidatedMakeEnvironmentValues::from_measurement(&oversized, &namespace, &names,)
                .is_err()
        );
        assert!(ValidatedMakeEnvironmentValues::from_measurement(
            &"x".repeat(super::super::MAX_MEASUREMENT_BYTES + 1),
            &namespace,
            &names,
        )
        .is_err());
        assert!(ValidatedMakeEnvironmentValues::from_make_stdout(
            &vec![b'x'; super::super::MAX_STDOUT_BYTES + 1],
            &namespace,
            &names,
        )
        .is_err());
    }

    #[test]
    fn raw_validation_checks_origin_duplicates_controls_and_combined_bounds() {
        let namespace = namespace(&[
            ("SYNTHETIC_A", MakeVariableOrigin::Environment),
            ("SYNTHETIC_B", MakeVariableOrigin::Environment),
        ]);
        let names = expected(&["SYNTHETIC_A", "SYNTHETIC_B"]);
        let valid = vec![
            entry(
                "SYNTHETIC_A",
                "line\nwith\ttab",
                super::super::MakeInitialValueFlavor::Recursive,
                MakeVariableOrigin::Environment,
            ),
            entry(
                "SYNTHETIC_B",
                "percent%|;value",
                super::super::MakeInitialValueFlavor::Simple,
                MakeVariableOrigin::Environment,
            ),
        ];
        assert!(ValidatedMakeEnvironmentValues::validate(
            observation(valid.clone()),
            &namespace,
            &names,
        )
        .is_ok());

        let mut wrong_origin = valid.clone();
        wrong_origin[0].origin = MakeVariableOrigin::EnvironmentOverride;
        assert!(ValidatedMakeEnvironmentValues::validate(
            observation(wrong_origin),
            &namespace,
            &names,
        )
        .is_err());

        let mut duplicate = valid.clone();
        duplicate.push(valid[0].clone());
        assert!(ValidatedMakeEnvironmentValues::validate(
            observation(duplicate),
            &namespace,
            &names,
        )
        .is_err());

        let mut control = valid;
        control[0].value.push('\0');
        assert!(
            ValidatedMakeEnvironmentValues::validate(observation(control), &namespace, &names,)
                .is_err()
        );

        let long_value = "x".repeat(64 * 1024);
        let over_total = vec![
            entry(
                "SYNTHETIC_A",
                &long_value,
                super::super::MakeInitialValueFlavor::Simple,
                MakeVariableOrigin::Environment,
            ),
            entry(
                "SYNTHETIC_B",
                &long_value,
                super::super::MakeInitialValueFlavor::Simple,
                MakeVariableOrigin::Environment,
            ),
        ];
        assert!(ValidatedMakeEnvironmentValues::validate(
            observation(over_total),
            &namespace,
            &names,
        )
        .is_err());
    }
}
