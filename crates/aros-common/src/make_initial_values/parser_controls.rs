//! GNU language parser controls, separate from ordinary default values.

use super::{MakeInitialValueObservation, MakeInitialValuesObservation, ValueSelection};
use crate::make_namespace::ValidatedMakeNamespace;
use std::collections::BTreeMap;

/// Distinct versioned packet; the legacy default-value v1 contract is unchanged.
pub const MAKE_PARSER_CONTROLS_METADATA_KEY: &str = "AROS_MAKE_PARSER_CONTROLS";

/// Measured file/default parser controls. This establishes consistency only,
/// not native invocation authentication, and never guesses a default value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedMakeParserControls {
    entries: BTreeMap<String, MakeInitialValueObservation>,
}

impl ValidatedMakeParserControls {
    /// Validate the complete eligible `.RECIPEPREFIX` membership and raw value.
    ///
    /// # Errors
    /// Refuses missing/extra/duplicate records, wrong origins, invalid versions,
    /// malformed values and the shared initial-value protocol bounds.
    pub fn validate(
        raw: MakeInitialValuesObservation,
        namespace: &ValidatedMakeNamespace,
    ) -> crate::Result<Self> {
        Ok(Self {
            entries: super::validate_entries(raw, namespace, ValueSelection::ParserControls)?,
        })
    }

    /// Parse the separate `v1;...;end` packet with the shared percent codec.
    ///
    /// # Errors
    /// Refuses malformed framing/encoding, inconsistent membership and bounds.
    pub fn from_measurement(
        value: &str,
        namespace: &ValidatedMakeNamespace,
    ) -> crate::Result<Self> {
        Ok(Self {
            entries: super::parse_measurement(value, namespace, ValueSelection::ParserControls)?,
        })
    }

    /// Select exactly one parser-control packet from bounded native stdout.
    ///
    /// # Errors
    /// Refuses missing/duplicate metadata and every invalid measurement above.
    pub fn from_make_stdout(
        bytes: &[u8],
        namespace: &ValidatedMakeNamespace,
    ) -> crate::Result<Self> {
        Self::from_measurement(
            super::metadata_measurement(bytes, MAKE_PARSER_CONTROLS_METADATA_KEY)?,
            namespace,
        )
    }

    /// Iterate measured controls; values may be private and Debug remains redacted.
    pub fn entries(&self) -> impl Iterator<Item = &MakeInitialValueObservation> {
        self.entries.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin};

    fn namespace(origin: Option<MakeVariableOrigin>) -> ValidatedMakeNamespace {
        let mut entries = [".VARIABLES", "MAKE_VERSION"]
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: MakeVariableOrigin::Default,
            })
            .to_vec();
        if let Some(origin) = origin {
            entries.push(MakeNamespaceEntry {
                name: ".RECIPEPREFIX".into(),
                origin,
            });
        }
        ValidatedMakeNamespace::validate(MakeNamespaceObservation {
            version: 1,
            entries,
        })
        .unwrap()
    }

    #[test]
    fn controls_are_complete_separate_raw_data_not_assumed_defaults() {
        let ns = namespace(Some(MakeVariableOrigin::Default));
        let good = "v1;.RECIPEPREFIX|default|simple|;end";
        assert_eq!(
            ValidatedMakeParserControls::from_measurement(good, &ns)
                .unwrap()
                .entries()
                .next()
                .unwrap()
                .value,
            ""
        );
        let private = "v1;.RECIPEPREFIX|default|recursive|private%09%7C%3B%25;end";
        let parsed = ValidatedMakeParserControls::from_measurement(private, &ns).unwrap();
        assert_eq!(parsed.entries().next().unwrap().value, "private\t|;%");
        assert!(!format!("{parsed:?}").contains("private"));
        for bad in [
            "v1;end",
            "v2;.RECIPEPREFIX|default|simple|;end",
            "v1;.OTHER|default|simple|;end",
            "v1;.RECIPEPREFIX|environment|simple|;end",
            "v1;.RECIPEPREFIX|default|simple|%GG;end",
            "v1;.RECIPEPREFIX|default|simple|; .RECIPEPREFIX|default|simple|;end",
        ] {
            assert!(ValidatedMakeParserControls::from_measurement(bad, &ns).is_err());
        }
        assert!(ValidatedMakeParserControls::from_make_stdout(b"", &ns).is_err());
        let packet = format!("{MAKE_PARSER_CONTROLS_METADATA_KEY}={good}\n");
        assert!(ValidatedMakeParserControls::from_make_stdout(
            format!("{packet}{packet}").as_bytes(),
            &ns
        )
        .is_err());
        assert!(
            super::super::ValidatedMakeInitialValues::from_measurement(good, &ns).is_err(),
            "legacy v1 must still exclude dot controls"
        );
    }

    #[test]
    fn absent_and_ineligible_origins_have_empty_packets_not_fabricated_records() {
        for origin in [
            None,
            Some(MakeVariableOrigin::Environment),
            Some(MakeVariableOrigin::CommandLine),
        ] {
            let ns = namespace(origin);
            assert_eq!(
                ValidatedMakeParserControls::from_measurement("v1;end", &ns)
                    .unwrap()
                    .entries()
                    .count(),
                0
            );
            assert!(ValidatedMakeParserControls::from_measurement(
                "v1;.RECIPEPREFIX|default|simple|;end",
                &ns
            )
            .is_err());
        }
    }

    #[test]
    fn native_controls_packet_preserves_raw_data_and_refuses_helper_collisions() {
        use std::process::Command;
        let make = if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        };
        let native = |extra: Option<&str>, collision: Option<&str>| {
            let mut command = Command::new(make);
            command
                .env_clear()
                .env("PATH", std::env::var("PATH").unwrap())
                .env("LC_ALL", "C")
                .args(["-r", "-n", "-s", "--no-print-directory"]);
            if let Some(extra) = extra {
                command.args(["--eval", extra]);
            }
            command
                .arg("--eval")
                .arg(crate::make_observer::pre_file_fragment())
                .args([
                    "--eval",
                    "probe:",
                    "-f",
                    "/dev/null",
                    "--old-file=/dev/null",
                    "probe",
                ]);
            if let Some(collision) = collision {
                command.arg(format!("{collision}=collision"));
            }
            command.output().unwrap()
        };
        for (declaration, expected) in [
            (None, ""),
            (
                Some(".RECIPEPREFIX = >raw|;%$(error UNEXPANDED)"),
                ">raw|;%$(error UNEXPANDED)",
            ),
        ] {
            let output = native(declaration, None);
            assert!(
                output.status.success(),
                "synthetic native parser-control query failed"
            );
            let ns = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
            let observed =
                ValidatedMakeParserControls::from_make_stdout(&output.stdout, &ns).unwrap();
            assert_eq!(observed.entries().count(), 1);
            assert_eq!(observed.entries().next().unwrap().value, expected);
            let defaults =
                super::super::ValidatedMakeInitialValues::from_make_stdout(&output.stdout, &ns)
                    .unwrap();
            assert!(!defaults.entries().any(|entry| entry.name.starts_with('.')));
        }
        for helper in ["__AROS_PARSER_CONTROLS_ITEM", "__AROS_PARSER_CONTROLS_NL"] {
            let output = native(None, Some(helper));
            assert!(!output.status.success());
            assert!(
                output.stdout.is_empty(),
                "collision must fail before any metadata emits"
            );
        }
    }
}
