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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
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

/// Compiler groups whose SDK generation is required on a subset of the active
/// hosts only.
///
/// The LLVM SDK generation does not finish within the six hour limit of a
/// GitHub-hosted macOS runner (aros-toolchains#119), while the Linux hosts
/// finish in under four hours. Release evidence must come from GitHub-hosted
/// runners, so the macOS host qualifies the LLVM compilers (both copies and
/// their comparison) but does not generate the LLVM SDKs. The SDK packages are
/// host-independent, so no SDK package is lost. What is given up is the proof
/// that the macOS-hosted LLVM compiler generates the same SDK bytes as the
/// Linux-hosted compilers. Groups absent from this table need every selected
/// host.
const SDK_GENERATION_HOST_RESTRICTIONS: &[(&str, &[&str])] =
    &[("llvm", &["linux-aarch64", "linux-x86_64"])];

/// Report whether an SDK generation for `group_id` is required on `host`.
#[must_use]
pub fn sdk_generation_required_on_host(group_id: &str, host: &str) -> bool {
    SDK_GENERATION_HOST_RESTRICTIONS
        .iter()
        .find(|(group, _)| *group == group_id)
        .is_none_or(|(_, hosts)| hosts.contains(&host))
}

impl ReleaseInputs {
    /// Return the canonical SDK/compiler-profile/host generation matrix.
    ///
    /// Every bound SDK is paired with exactly its explicit compiler profile
    /// references and each selected active host on which that compiler group
    /// requires an SDK generation (see [`sdk_generation_required_on_host`]).
    /// Compiler-only inputs produce an empty vector. Order follows the
    /// validated SDK IDs, reference pairs, and host order; no product or
    /// profile is inferred, and the only host restriction is the explicit
    /// table above.
    #[must_use]
    pub fn expected_sdk_generation_lanes(&self) -> Vec<ExpectedSdkGenerationLane> {
        self.sdks()
            .iter()
            .flat_map(|sdk| {
                sdk.compiler_profiles().iter().flat_map(move |reference| {
                    self.hosts()
                        .iter()
                        .filter(move |host| {
                            sdk_generation_required_on_host(reference.group_id(), host)
                        })
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

#[cfg(test)]
mod tests {
    use super::sdk_generation_required_on_host;

    #[test]
    fn llvm_sdk_generation_is_not_required_on_macos() {
        assert!(sdk_generation_required_on_host("llvm", "linux-aarch64"));
        assert!(sdk_generation_required_on_host("llvm", "linux-x86_64"));
        assert!(!sdk_generation_required_on_host("llvm", "macos-aarch64"));
    }

    #[test]
    fn other_groups_require_every_host() {
        for host in ["linux-aarch64", "linux-x86_64", "macos-aarch64"] {
            assert!(sdk_generation_required_on_host("gnu-rv32", host));
            assert!(sdk_generation_required_on_host("any-new-group", host));
        }
    }
}
