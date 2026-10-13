//! Original-build portable byte readback; never execution authentication.

use std::path::PathBuf;

use aros_common::Sha256Digest;
use aros_toolchain::package_verify::{PackageVerificationRequest, VerifiedPackage};
use clap::Args;
use serde_json::Value;

use super::PackageFormatArg;

#[derive(Args)]
pub(super) struct ReadbackArgs {
    /// Original-owner export; portable readback never rereads its original host paths
    #[arg(long, requires_all = ["package_measurement_sha256", "executor_contract_sha256", "executor_binary_sha256", "package_format"])]
    package_measurement: Option<PathBuf>,
    /// Externally selected raw export digest, not its self-digest or authenticity proof
    #[arg(long, value_parser = super::parse_build_result_digest, requires_all = ["package_measurement", "executor_contract_sha256", "executor_binary_sha256", "package_format"])]
    package_measurement_sha256: Option<Sha256Digest>,
    /// Original executor contract digest independently selected from authenticated evidence
    #[arg(long, value_parser = super::parse_build_result_digest, requires_all = ["package_measurement", "package_measurement_sha256", "executor_binary_sha256", "package_format"])]
    executor_contract_sha256: Option<Sha256Digest>,
    /// Original executor binary digest; do not substitute the collector's own binary
    #[arg(long, value_parser = super::parse_build_result_digest, requires_all = ["package_measurement", "package_measurement_sha256", "executor_contract_sha256", "package_format"])]
    executor_binary_sha256: Option<Sha256Digest>,
}

impl ReadbackArgs {
    pub(super) const fn requested(&self) -> bool {
        self.package_measurement.is_some()
    }

    /// Reject incompatible formats before opening any package or recipe input.
    pub(super) fn preflight(&self, format: Option<PackageFormatArg>) -> miette::Result<()> {
        if self.requested() && !matches!(format, Some(PackageFormatArg::FamilyV2)) {
            return Err(miette::miette!(
                "portable original-build readback requires --package-format family-v2"
            ));
        }
        Ok(())
    }
}

pub(super) fn run(
    package: &PackageVerificationRequest,
    selected: &ReadbackArgs,
) -> miette::Result<(VerifiedPackage, Option<Value>)> {
    #[cfg(unix)]
    {
        use aros_toolchain::native_candidate::{
            readback_finished_package_measurement, PortableFinishedPackageRequest,
        };
        use aros_toolchain::plan::{Executor, Identity};
        use aros_toolchain::release_inputs::ACTIVE_HOSTS;

        let measurement = selected
            .package_measurement
            .as_deref()
            .ok_or_else(|| miette::miette!("portable readback requires --package-measurement"))?;
        let measurement_sha256 = selected
            .package_measurement_sha256
            .as_ref()
            .ok_or_else(|| {
                miette::miette!("portable readback requires --package-measurement-sha256")
            })?;
        let contract_sha256 = selected.executor_contract_sha256.as_ref().ok_or_else(|| {
            miette::miette!("portable readback requires --executor-contract-sha256")
        })?;
        let binary_sha256 = selected.executor_binary_sha256.as_ref().ok_or_else(|| {
            miette::miette!("portable readback requires --executor-binary-sha256")
        })?;
        let host = ACTIVE_HOSTS
            .iter()
            .copied()
            .find(|host| *host == package.host)
            .ok_or_else(|| {
                miette::miette!("portable readback host is outside the active contract")
            })?;
        // Recipe and lane expectations are independent package-context inputs.
        // Executor digests are caller selections, never inferred from the export.
        let identity = Identity {
            recipe_sha256: package.recipe.sha256().clone(),
            source_commit: package.recipe.source().0.clone(),
            producer_commit: package.recipe.producer().0.clone(),
            tools_commit: package.recipe.tools().0.clone(),
            host,
            target_profile: package.profile.name().to_owned(),
            executor: Executor {
                contract_id: Some("aros-toolchain-producer-v1"),
                contract_sha256: Some(contract_sha256.clone()),
                tools_commit: Some(package.recipe.tools().0.clone()),
                binary_sha256: binary_sha256.clone(),
                origin_evidence_sha256: None,
            },
        };
        let proof = readback_finished_package_measurement(&PortableFinishedPackageRequest {
            measurement,
            measurement_sha256,
            identity: &identity,
            package,
        })
        .map_err(|error| super::native_error(&error))?;
        let document = serde_json::json!({
            "schema": "aros-toolchain-portable-package-readback-v2",
            "assurance": "portable-byte-readback-only",
            "measurement_sha256": proof.measurement_sha256(),
            "build_result_sha256": proof.build_result_sha256(),
            "package_tree_sha256": proof.verified_package().manifest.tree_sha256,
            "finished_receipt_sha256": proof.finished_receipt_sha256(),
            "raw_payload_sha256": proof.raw_payload_sha256(),
        });
        Ok((proof.verified_package().clone(), Some(document)))
    }
    #[cfg(not(unix))]
    {
        let _ = (package, selected);
        Err(miette::miette!(
            "portable original-build readback requires a supported Unix host"
        ))
    }
}
