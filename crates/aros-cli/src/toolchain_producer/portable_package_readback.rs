//! Optional portable byte read-back for a finished family-v2 package.
//!
//! The measurement joins retained bytes to independently selected package
//! and executor expectations. It does not authenticate origin, execution, or
//! publication.

use std::path::PathBuf;

use aros_common::Sha256Digest;
#[cfg(unix)]
use aros_toolchain::native_candidate::{
    readback_finished_package_measurement, PortableFinishedPackageRequest,
};
use aros_toolchain::package_verify::{PackageVerificationRequest, VerifiedPackage};
#[cfg(unix)]
use aros_toolchain::plan::{Executor, Identity};
#[cfg(unix)]
use aros_toolchain::release_inputs::ACTIVE_HOSTS;

use super::PackageFormatArg;

/// Independently selected portable finished-package measurement and executor identity.
#[derive(clap::Args, Default)]
pub(super) struct ReadbackArgs {
    /// Portable package measurement exported by the owning build host
    #[arg(
        long = "package-measurement",
        requires_all = [
            "measurement_sha256",
            "executor_contract_sha256",
            "executor_binary_sha256",
            "package_format"
        ]
    )]
    pub(super) measurement: Option<PathBuf>,
    /// SHA-256 of the exact selected portable measurement bytes
    #[arg(
        long = "package-measurement-sha256",
        value_parser = super::parse_build_result_digest,
        requires = "measurement"
    )]
    pub(super) measurement_sha256: Option<Sha256Digest>,
    /// Independently selected native executor contract digest
    #[arg(
        long = "executor-contract-sha256",
        value_parser = super::parse_build_result_digest,
        requires = "measurement"
    )]
    pub(super) executor_contract_sha256: Option<Sha256Digest>,
    /// Independently selected native executor binary digest
    #[arg(
        long = "executor-binary-sha256",
        value_parser = super::parse_build_result_digest,
        requires = "measurement"
    )]
    pub(super) executor_binary_sha256: Option<Sha256Digest>,
}

impl ReadbackArgs {
    /// Whether any portable read-back selector was supplied.
    #[must_use]
    pub(super) const fn enabled(&self) -> bool {
        self.measurement.is_some()
            || self.measurement_sha256.is_some()
            || self.executor_contract_sha256.is_some()
            || self.executor_binary_sha256.is_some()
    }

    /// Validate the cross-field selection before any selected input is opened.
    pub(super) fn validate_format(
        &self,
        package_format: Option<PackageFormatArg>,
    ) -> miette::Result<()> {
        self.validate_selection()?;
        if self.enabled() && !matches!(package_format, Some(PackageFormatArg::FamilyV2)) {
            return Err(miette::miette!(
                "portable package read-back requires an explicit --package-format family-v2"
            ));
        }
        Ok(())
    }

    fn validate_selection(&self) -> miette::Result<()> {
        let any_executor_selector =
            self.executor_contract_sha256.is_some() || self.executor_binary_sha256.is_some();
        if self.measurement.is_some() {
            if self.measurement_sha256.is_none()
                || self.executor_contract_sha256.is_none()
                || self.executor_binary_sha256.is_none()
            {
                return Err(miette::miette!(
                    "portable package read-back requires the measurement and all three selected SHA-256 values"
                ));
            }
        } else if self.measurement_sha256.is_some() || any_executor_selector {
            return Err(miette::miette!(
                "portable package read-back SHA-256 values require --package-measurement"
            ));
        }
        Ok(())
    }
}

/// Read one retained portable measurement against separately selected inputs.
pub(super) fn run(
    request: &PackageVerificationRequest,
    args: &ReadbackArgs,
) -> miette::Result<(VerifiedPackage, serde_json::Value)> {
    #[cfg(not(unix))]
    {
        let _ = (request, args);
        Err(miette::miette!(
            "portable package read-back requires a supported Unix host"
        ))
    }
    #[cfg(unix)]
    {
        args.validate_selection()?;

        let measurement = args.measurement.as_deref().ok_or_else(|| {
            miette::miette!("portable package read-back requires --package-measurement")
        })?;
        let measurement_sha256 = args.measurement_sha256.as_ref().ok_or_else(|| {
            miette::miette!("portable package read-back requires --package-measurement-sha256")
        })?;
        let executor_contract_sha256 = args.executor_contract_sha256.as_ref().ok_or_else(|| {
            miette::miette!("portable package read-back requires --executor-contract-sha256")
        })?;
        let executor_binary_sha256 = args.executor_binary_sha256.as_ref().ok_or_else(|| {
            miette::miette!("portable package read-back requires --executor-binary-sha256")
        })?;
        let host = ACTIVE_HOSTS
            .iter()
            .copied()
            .find(|host| *host == request.host)
            .ok_or_else(|| {
                miette::miette!("portable package read-back host is outside the active contract")
            })?;

        let recipe = &request.recipe;
        let identity = Identity {
            recipe_sha256: recipe.sha256().clone(),
            source_commit: recipe.source().0.clone(),
            producer_commit: recipe.producer().0.clone(),
            tools_commit: recipe.tools().0.clone(),
            host,
            target_profile: request.profile.name().to_owned(),
            executor: Executor {
                contract_id: Some("aros-toolchain-producer-v1"),
                contract_sha256: Some(executor_contract_sha256.clone()),
                tools_commit: Some(recipe.tools().0.clone()),
                binary_sha256: executor_binary_sha256.clone(),
                origin_evidence_sha256: None,
            },
        };
        let readback = readback_finished_package_measurement(&PortableFinishedPackageRequest {
            measurement,
            measurement_sha256,
            identity: &identity,
            package: request,
        })
        .map_err(|error| super::native_error(&error))?;

        let verified_package = readback.verified_package().clone();
        let evidence = serde_json::json!({
            "schema": "aros-toolchain-portable-package-readback-v2",
            "assurance": "portable-byte-readback-only",
            "measurement_sha256": readback.measurement_sha256(),
            "build_result_sha256": readback.build_result_sha256(),
            "receipt_sha256": readback.finished_receipt_sha256(),
            "raw_payload_sha256": readback.raw_payload_sha256(),
            "package_tree_sha256": verified_package.manifest.tree_sha256,
        });
        Ok((verified_package, evidence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest() -> Sha256Digest {
        Sha256Digest::parse(&"0".repeat(64)).expect("canonical digest")
    }

    #[test]
    fn default_selection_is_disabled_and_valid_without_format() {
        let args = ReadbackArgs::default();

        assert!(!args.enabled());
        assert!(args.validate_format(None).is_ok());
    }

    #[test]
    fn incomplete_direct_selection_is_rejected() {
        let args = ReadbackArgs {
            measurement: Some(PathBuf::from("measurement.json")),
            measurement_sha256: Some(digest()),
            ..ReadbackArgs::default()
        };

        assert!(args.enabled());
        assert!(args
            .validate_format(Some(PackageFormatArg::FamilyV2))
            .is_err());
    }

    #[test]
    fn complete_selection_requires_explicit_family_v2() {
        let args = ReadbackArgs {
            measurement: Some(PathBuf::from("measurement.json")),
            measurement_sha256: Some(digest()),
            executor_contract_sha256: Some(digest()),
            executor_binary_sha256: Some(digest()),
        };

        assert!(args.validate_format(None).is_err());
        assert!(args
            .validate_format(Some(PackageFormatArg::LegacyV1))
            .is_err());
        assert!(args
            .validate_format(Some(PackageFormatArg::FamilyV2))
            .is_ok());
    }
}
