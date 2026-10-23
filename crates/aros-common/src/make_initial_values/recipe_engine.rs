//! Separate opt-in observation of GNU's immutable CLI-propagation scalar.
//!
//! This does not admit general automatic variables or replace legacy packet
//! memberships. Values are private measurement data, not execution authority.
use super::{MakeInitialValueObservation, MakeInitialValuesObservation, ValueSelection};
use crate::make_namespace::ValidatedMakeNamespace;
use std::collections::BTreeMap;

pub const MAKE_RECIPE_ENGINE_VALUES_METADATA_KEY: &str = "AROS_MAKE_RECIPE_ENGINE_VALUES";
pub const MAKE_CLI_PROPAGATION_VARIABLE: &str = "-*-command-variables-*-";

/// Measured raw CLI propagation, including GNU's original escaping/order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedMakeRecipeEngineValues {
    entries: BTreeMap<String, MakeInitialValueObservation>,
}

impl ValidatedMakeRecipeEngineValues {
    /// Validate exact automatic/simple membership and existing codec bounds.
    /// # Errors
    /// Refuses wrong schema, missing/extra/duplicate names, origins or flavor.
    pub fn validate(
        raw: MakeInitialValuesObservation,
        namespace: &ValidatedMakeNamespace,
    ) -> crate::Result<Self> {
        Ok(Self {
            entries: super::validate_entries(raw, namespace, ValueSelection::RecipeEngine)?,
        })
    }

    /// Parse the separate packet, without adopting other automatic variables.
    /// # Errors
    /// Refuses malformed framing, membership and bounded-value violations.
    pub fn from_measurement(
        value: &str,
        namespace: &ValidatedMakeNamespace,
    ) -> crate::Result<Self> {
        Ok(Self {
            entries: super::parse_measurement(value, namespace, ValueSelection::RecipeEngine)?,
        })
    }

    /// Select one exact private metadata packet from bounded GNU output.
    /// # Errors
    /// Refuses missing/duplicate keys and every invalid measurement above.
    pub fn from_make_stdout(
        bytes: &[u8],
        namespace: &ValidatedMakeNamespace,
    ) -> crate::Result<Self> {
        Self::from_measurement(
            super::metadata_measurement(bytes, MAKE_RECIPE_ENGINE_VALUES_METADATA_KEY)?,
            namespace,
        )
    }

    /// Iterate validated raw observations; their Debug hides private values.
    pub fn entries(&self) -> impl Iterator<Item = &MakeInitialValueObservation> {
        self.entries.values()
    }
}

#[cfg(test)]
#[path = "recipe_engine_tests.rs"]
mod tests;
