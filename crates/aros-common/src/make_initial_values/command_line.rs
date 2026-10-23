//! Opt-in, bounded observations of GNU Make command-line variable values.
//!
//! Raw command-line values can contain sensitive data. Callers must provide
//! the complete expected name set explicitly; this module checks it against
//! the measured namespace before parsing or returning any raw values.

use std::collections::{BTreeMap, BTreeSet};

use crate::make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace};

use super::{MakeInitialValueObservation, MakeInitialValuesObservation, ValueSelection};

const MAX_ENTRIES: usize = 4096;
const MAX_NAME_BYTES: usize = 128;
const RESERVED_HELPERS: [&str; 2] = ["__AROS_COMMAND_VALUES_ITEM", "__AROS_COMMAND_VALUES_NL"];
const CONFIGURATION_FILE: &str = "Make command-line values";

/// Metadata key for explicitly requested, non-environment command-line values.
pub const MAKE_COMMAND_LINE_VALUES_METADATA_KEY: &str = "AROS_MAKE_COMMAND_LINE_VALUES";

/// A validated, deterministic map of all expected command-line-origin values.
///
/// The supplied names must match the complete set of ordinary command-line
/// origin names in the exact measured namespace. This type does not establish
/// who produced the namespace or bind it to an execution receipt.
#[derive(Clone, Eq, PartialEq)]
pub struct ValidatedMakeCommandLineValues {
    entries: BTreeMap<String, MakeInitialValueObservation>,
}

impl std::fmt::Debug for ValidatedMakeCommandLineValues {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedMakeCommandLineValues")
            .field("entry_count", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl ValidatedMakeCommandLineValues {
    /// Validate values against the complete expected command-line name set.
    ///
    /// # Errors
    /// Rejects unsupported versions, incomplete or extra membership, unsafe
    /// names, origin mismatches, refused controls and protocol-bound violations.
    pub fn validate(
        raw: MakeInitialValuesObservation,
        namespace: &ValidatedMakeNamespace,
        expected: &BTreeSet<String>,
    ) -> crate::Result<Self> {
        validate_expected_names(namespace, expected)?;
        let entries = super::validate_entries(raw, namespace, ValueSelection::CommandLine)?;
        validate_entry_membership(&entries, expected)?;
        Ok(Self { entries })
    }

    /// Parse the bounded `v1;NAME|command line|flavor|value;...;end` form.
    ///
    /// Values use the existing initial-value percent encoding. The complete
    /// expected name set is checked against the namespace before the packet is
    /// parsed.
    ///
    /// # Errors
    /// Rejects inconsistent membership, malformed framing, unsafe names, refused
    /// origins/controls, invalid escapes and measurements exceeding the bounds.
    pub fn from_measurement(
        measurement: &str,
        namespace: &ValidatedMakeNamespace,
        expected: &BTreeSet<String>,
    ) -> crate::Result<Self> {
        validate_expected_names(namespace, expected)?;
        let entries =
            super::parse_measurement(measurement, namespace, ValueSelection::CommandLine)?;
        validate_entry_membership(&entries, expected)?;
        Ok(Self { entries })
    }

    /// Select and parse exactly one command-line-values metadata line.
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
        let measurement =
            super::metadata_measurement(bytes, MAKE_COMMAND_LINE_VALUES_METADATA_KEY)?;
        let entries =
            super::parse_measurement(measurement, namespace, ValueSelection::CommandLine)?;
        validate_entry_membership(&entries, expected)?;
        Ok(Self { entries })
    }

    /// Return a stable, name-sorted raw representation.
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
    validate_command_line_names(expected)?;

    let mut namespace_names = BTreeSet::new();
    for entry in namespace.to_observation().entries {
        if entry.origin == MakeVariableOrigin::CommandLine {
            validate_cli_name(&entry.name)?;
            namespace_names.insert(entry.name);
        }
    }
    if namespace_names != *expected {
        return Err(command_line_values_error(
            "expected names do not match the complete command-line namespace",
        ));
    }
    Ok(())
}

pub fn validate_command_line_names(expected: &BTreeSet<String>) -> crate::Result<()> {
    if expected.len() > MAX_ENTRIES {
        return Err(command_line_values_error(
            "expected name count exceeds its limit",
        ));
    }
    for name in expected {
        validate_cli_name(name)?;
    }

    Ok(())
}

fn validate_entry_membership(
    entries: &BTreeMap<String, MakeInitialValueObservation>,
    expected: &BTreeSet<String>,
) -> crate::Result<()> {
    if entries.len() != expected.len() || entries.keys().any(|name| !expected.contains(name)) {
        return Err(command_line_values_error(
            "measurement does not contain every expected command-line name",
        ));
    }
    Ok(())
}

fn validate_cli_name(name: &str) -> crate::Result<()> {
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || RESERVED_HELPERS.contains(&name)
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(command_line_values_error(
            "command-line namespace contains an unsupported name",
        ));
    }
    Ok(())
}

fn command_line_values_error(message: &str) -> crate::ArosError {
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

    #[test]
    fn measurement_preserves_raw_values_and_flavors_with_complete_membership() {
        let namespace = namespace(&[
            ("CLI_A", MakeVariableOrigin::CommandLine),
            ("1CLI-B", MakeVariableOrigin::CommandLine),
        ]);
        let values = ValidatedMakeCommandLineValues::from_measurement(
            "v1; 1CLI-B|command line|simple|literal%25; CLI_A|command line|recursive|  $$(LATER)%0A%09%7C%3B  ;end",
            &namespace,
            &expected(&["CLI_A", "1CLI-B"]),
        )
        .unwrap();
        let entries = values.to_observation().entries;
        let debug = format!("{values:?}");
        assert!(!debug.contains("literal%"));
        assert!(!debug.contains("LATER"));
        assert_eq!(entries[0].name, "1CLI-B");
        assert_eq!(entries[0].value, "literal%");
        assert_eq!(
            entries[0].flavor,
            super::super::MakeInitialValueFlavor::Simple
        );
        assert_eq!(entries[1].name, "CLI_A");
        assert_eq!(entries[1].value, "  $$(LATER)\n\t|;  ");
        assert_eq!(
            entries[1].flavor,
            super::super::MakeInitialValueFlavor::Recursive
        );
        assert!(entries
            .iter()
            .all(|value| value.origin == MakeVariableOrigin::CommandLine));
    }

    #[test]
    fn expected_names_must_match_namespace_and_measurement_exactly() {
        let namespace = namespace(&[
            ("A", MakeVariableOrigin::CommandLine),
            ("B", MakeVariableOrigin::CommandLine),
        ]);
        let both = expected(&["A", "B"]);
        assert!(ValidatedMakeCommandLineValues::from_measurement(
            "v1;A|command line|simple|a;end",
            &namespace,
            &both,
        )
        .is_err());
        assert!(ValidatedMakeCommandLineValues::from_measurement(
            "v1;A|command line|simple|a;B|file|simple|b;end",
            &namespace,
            &both,
        )
        .is_err());
        assert!(ValidatedMakeCommandLineValues::from_measurement(
            "v1;A|command line|simple|a;B|command line|simple|b;end",
            &namespace,
            &expected(&["A"]),
        )
        .is_err());
        assert!(ValidatedMakeCommandLineValues::from_measurement(
            "v1;A|command line|simple|a;B|command line|simple|b;end",
            &namespace,
            &expected(&["A", "B", "EXTRA"]),
        )
        .is_err());
    }

    #[test]
    fn refuses_unsupported_dot_and_helper_names_before_parsing_values() {
        for name in ["BAD.NAME", ".HIDDEN", "__AROS_COMMAND_VALUES_ITEM"] {
            let namespace = namespace(&[(name, MakeVariableOrigin::CommandLine)]);
            let error = ValidatedMakeCommandLineValues::from_measurement(
                "not-a-measurement-containing-secret",
                &namespace,
                &expected(&[name]),
            )
            .unwrap_err()
            .to_string();
            assert!(!error.contains("secret"));
        }
    }

    #[test]
    fn stdout_requires_one_metadata_line_and_rejects_oversized_raw_values() {
        let namespace = namespace(&[("A", MakeVariableOrigin::CommandLine)]);
        let names = expected(&["A"]);
        let key = MAKE_COMMAND_LINE_VALUES_METADATA_KEY;
        assert!(ValidatedMakeCommandLineValues::from_make_stdout(
            format!("{key}=v1;A|command line|simple|x;end\n").as_bytes(),
            &namespace,
            &names,
        )
        .is_ok());
        assert!(ValidatedMakeCommandLineValues::from_make_stdout(
            format!("{key}=v1;A|command line|simple|x;end\n{key}=v1;end\n").as_bytes(),
            &namespace,
            &names,
        )
        .is_err());

        let oversized = format!("v1;A|command line|simple|{};end", "x".repeat(64 * 1024 + 1));
        assert!(
            ValidatedMakeCommandLineValues::from_measurement(&oversized, &namespace, &names,)
                .is_err()
        );
    }

    #[test]
    fn existing_file_default_parser_still_refuses_command_line_values() {
        let namespace = namespace(&[("A", MakeVariableOrigin::CommandLine)]);
        assert!(super::super::ValidatedMakeInitialValues::from_measurement(
            "v1;A|command line|simple|synthetic-secret;end",
            &namespace,
        )
        .is_err());
    }

    #[test]
    fn raw_validation_requires_every_expected_command_line_entry() {
        let namespace = namespace(&[("A", MakeVariableOrigin::CommandLine)]);
        let raw = MakeInitialValuesObservation {
            version: 1,
            entries: vec![entry(
                "A",
                "value",
                super::super::MakeInitialValueFlavor::Simple,
                MakeVariableOrigin::CommandLine,
            )],
        };
        assert!(
            ValidatedMakeCommandLineValues::validate(raw, &namespace, &expected(&["A"]),).is_ok()
        );
    }
}
