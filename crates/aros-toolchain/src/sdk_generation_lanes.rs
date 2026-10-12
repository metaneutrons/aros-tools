//! Input-derived SDK generation execution coverage.
//!
//! These lanes declare which SDK generation jobs a selected release requires.
//! They are derived only from validated release inputs and do not constitute
//! generation, package, cross-host consistency, origin or qualification proof.

use crate::release_inputs::ReleaseInputs;

/// One required SDK generation for a bound SDK/compiler-profile/host tuple.
///
/// Instances are projected from [`ReleaseInputs`]; fields remain private so a
/// caller cannot deserialize or construct an arbitrary expected lane list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedSdkGenerationLane {
    sdk_id: String,
    group_id: String,
    profile: String,
    host: String,
}

impl ExpectedSdkGenerationLane {
    /// Source-owned SDK product ID from the validated input collection.
    #[must_use]
    pub fn sdk_id(&self) -> &str {
        &self.sdk_id
    }

    /// Explicitly referenced compiler input group.
    #[must_use]
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// Explicitly referenced compiler profile within that group.
    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// One selected active host from the validated release-input collection.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }
}

impl ReleaseInputs {
    /// Return the canonical SDK/compiler-profile/host generation matrix.
    ///
    /// Every bound SDK is paired with exactly its explicit compiler profile
    /// references and each selected active host. Compiler-only inputs produce
    /// an empty vector. Order follows the validated SDK IDs, reference pairs,
    /// and host order; no product, profile or host is inferred or hard-coded.
    #[must_use]
    pub fn expected_sdk_generation_lanes(&self) -> Vec<ExpectedSdkGenerationLane> {
        self.sdks()
            .iter()
            .flat_map(|sdk| {
                sdk.compiler_profiles().iter().flat_map(move |reference| {
                    self.hosts()
                        .iter()
                        .map(move |host| ExpectedSdkGenerationLane {
                            sdk_id: sdk.id().to_owned(),
                            group_id: reference.group_id().to_owned(),
                            profile: reference.profile().to_owned(),
                            host: host.clone(),
                        })
                })
            })
            .collect()
    }
}
