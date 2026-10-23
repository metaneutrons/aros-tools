//! GNU Make recipe controls, separate from ordinary default values and parser controls.

use super::{MakeInitialValueObservation, MakeInitialValuesObservation, ValueSelection};
use crate::make_namespace::ValidatedMakeNamespace;
use std::collections::BTreeMap;

/// Distinct versioned packet for the measured `.SHELLFLAGS` value.
pub const MAKE_RECIPE_CONTROLS_METADATA_KEY: &str = "AROS_MAKE_RECIPE_CONTROLS";

/// Measured file/default recipe controls. This establishes consistency only,
/// not native invocation authentication, and never guesses a default value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedMakeRecipeControls {
    entries: BTreeMap<String, MakeInitialValueObservation>,
}

impl ValidatedMakeRecipeControls {
    /// Validate the complete eligible `.SHELLFLAGS` membership and raw value.
    ///
    /// # Errors
    /// Refuses missing/extra/duplicate records, wrong origins, invalid versions,
    /// malformed values and the shared initial-value protocol bounds.
    pub fn validate(
        raw: MakeInitialValuesObservation,
        namespace: &ValidatedMakeNamespace,
    ) -> crate::Result<Self> {
        Ok(Self {
            entries: super::validate_entries(raw, namespace, ValueSelection::RecipeControls)?,
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
            entries: super::parse_measurement(value, namespace, ValueSelection::RecipeControls)?,
        })
    }

    /// Select exactly one recipe-control packet from bounded native stdout.
    ///
    /// # Errors
    /// Refuses missing/duplicate metadata and every invalid measurement above.
    pub fn from_make_stdout(
        bytes: &[u8],
        namespace: &ValidatedMakeNamespace,
    ) -> crate::Result<Self> {
        Self::from_measurement(
            super::metadata_measurement(bytes, MAKE_RECIPE_CONTROLS_METADATA_KEY)?,
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
                name: ".SHELLFLAGS".into(),
                origin,
            });
        }
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

    fn entry(value: &str, origin: MakeVariableOrigin) -> MakeInitialValueObservation {
        MakeInitialValueObservation {
            name: ".SHELLFLAGS".into(),
            value: value.into(),
            flavor: super::super::MakeInitialValueFlavor::Recursive,
            origin,
        }
    }

    #[test]
    fn recipe_controls_preserve_file_and_default_values_through_the_shared_codec() {
        for (origin, label) in [
            (MakeVariableOrigin::File, "file"),
            (MakeVariableOrigin::Default, "default"),
        ] {
            let ns = namespace(Some(origin));
            let raw_value = " private\t|;%$(error SHOULD_NOT_EXPAND)";
            let raw = observation(vec![entry(raw_value, origin)]);
            let measured = ValidatedMakeRecipeControls::validate(raw, &ns).unwrap();
            assert_eq!(measured.entries().next().unwrap().value, raw_value);
            assert_eq!(
                measured.entries().next().unwrap().flavor,
                super::super::MakeInitialValueFlavor::Recursive
            );
            assert!(!format!("{measured:?}").contains("SHOULD_NOT_EXPAND"));

            let encoded = " private%09%7C%3B%25$(error SHOULD_NOT_EXPAND)";
            let packet = format!("v1;.SHELLFLAGS|{label}|recursive|{encoded};end");
            let parsed = ValidatedMakeRecipeControls::from_measurement(&packet, &ns).unwrap();
            assert_eq!(parsed.entries().next().unwrap().value, raw_value);
        }
    }

    #[test]
    fn validation_rejects_wrong_version_membership_origins_and_malformed_packets() {
        let ns = namespace(Some(MakeVariableOrigin::Default));
        let good = entry("-c", MakeVariableOrigin::Default);

        let mut wrong_version = observation(vec![good.clone()]);
        wrong_version.version = 2;
        assert!(ValidatedMakeRecipeControls::validate(wrong_version, &ns).is_err());
        assert!(ValidatedMakeRecipeControls::validate(observation(Vec::new()), &ns).is_err());

        let duplicate = vec![good.clone(), good.clone()];
        assert!(ValidatedMakeRecipeControls::validate(observation(duplicate), &ns).is_err());

        let extra = vec![
            good,
            MakeInitialValueObservation {
                name: ".OTHER".into(),
                value: "x".into(),
                flavor: super::super::MakeInitialValueFlavor::Simple,
                origin: MakeVariableOrigin::File,
            },
        ];
        assert!(ValidatedMakeRecipeControls::validate(observation(extra), &ns).is_err());

        assert!(ValidatedMakeRecipeControls::validate(
            observation(vec![entry("-c", MakeVariableOrigin::File)]),
            &ns
        )
        .is_err());

        for malformed in [
            "v2;.SHELLFLAGS|default|recursive|-c;end",
            "v1;end",
            "v1;.OTHER|default|simple|x;end",
            "v1;.SHELLFLAGS|environment|simple|-c;end",
            "v1;.SHELLFLAGS|default|unknown|-c;end",
            "v1;.SHELLFLAGS|default|simple|bad%GG;end",
            "v1;.SHELLFLAGS|default|simple|bad%;end",
            "v1;.SHELLFLAGS|default|simple;end",
        ] {
            assert!(ValidatedMakeRecipeControls::from_measurement(malformed, &ns).is_err());
        }

        assert!(ValidatedMakeRecipeControls::from_make_stdout(b"", &ns).is_err());
        let packet =
            format!("{MAKE_RECIPE_CONTROLS_METADATA_KEY}=v1;.SHELLFLAGS|default|simple|-c;end\n");
        assert!(ValidatedMakeRecipeControls::from_make_stdout(
            format!("{packet}{packet}").as_bytes(),
            &ns
        )
        .is_err());
    }

    #[test]
    fn absent_and_ineligible_origins_have_empty_packets_without_fabricated_values() {
        for origin in [
            None,
            Some(MakeVariableOrigin::Environment),
            Some(MakeVariableOrigin::CommandLine),
        ] {
            let ns = namespace(origin);
            assert_eq!(
                ValidatedMakeRecipeControls::from_measurement("v1;end", &ns)
                    .unwrap()
                    .entries()
                    .count(),
                0
            );
            if let Some(origin) = origin {
                assert!(ValidatedMakeRecipeControls::validate(
                    observation(vec![entry("-c", origin)]),
                    &ns
                )
                .is_err());
            }
        }
    }

    #[test]
    fn recipe_controls_do_not_change_legacy_default_or_parser_control_membership() {
        let ns = namespace(Some(MakeVariableOrigin::Default));
        let packet = "v1;.SHELLFLAGS|default|simple|-c;end";
        assert!(super::super::ValidatedMakeInitialValues::from_measurement(packet, &ns).is_err());
        assert!(super::super::ValidatedMakeParserControls::from_measurement(packet, &ns).is_err());
    }
}
