//! Explicit same-host finished-build packaging, never release admission.

use aros_toolchain::package::{PackageOutput, PackageRequest};

use super::PackageArgs;

pub(super) fn run(
    request: &PackageRequest,
    args: &PackageArgs,
) -> miette::Result<(PackageOutput, Option<serde_json::Value>)> {
    #[cfg(unix)]
    {
        use aros_toolchain::native_candidate::{
            export_finished_package_measurement, package_finished_candidate,
            readback_finished_build_result, FinishedBuildResultRequest,
        };

        let result = args
            .build_result
            .as_deref()
            .ok_or_else(|| miette::miette!("finished-build packaging requires --build-result"))?;
        let digest = args.build_result_sha256.as_ref().ok_or_else(|| {
            miette::miette!("finished-build packaging requires --build-result-sha256")
        })?;
        let work = args
            .build_work_dir
            .as_deref()
            .ok_or_else(|| miette::miette!("finished-build packaging requires --build-work-dir"))?;
        let output = request.candidate_root.parent().ok_or_else(|| {
            miette::miette!("finished-build packaging requires the original toolchain prefix")
        })?;
        let proof = readback_finished_build_result(&FinishedBuildResultRequest {
            work_dir: work,
            output_dir: output,
            recipe: &request.recipe,
            source_lock: &request.source_lock,
            profile: &request.profile,
            host: &request.host,
            build_result: result,
            build_result_sha256: digest,
        })
        .map_err(|error| super::native_error(&error))?;
        let packaged = package_finished_candidate(request, &proof)
            .map_err(|error| super::native_error(&error))?;
        let measurement = export_finished_package_measurement(&proof, &packaged)
            .map_err(|error| super::native_error(&error))?;
        let measurement_text = std::str::from_utf8(measurement.bytes())
            .map_err(|_| miette::miette!("portable package measurement is not UTF-8"))?;
        let evidence = serde_json::json!({
            "build_result_sha256": digest,
            "receipt_sha256": packaged.candidate_receipt_sha256(),
            "raw_payload_sha256": packaged.candidate_payload_sha256(),
            "package_tree_sha256": packaged.verified().manifest.tree_sha256,
            "portable_measurement": measurement_text,
            "portable_measurement_sha256": measurement.sha256(),
        });
        Ok((packaged.output().clone(), Some(evidence)))
    }
    #[cfg(not(unix))]
    {
        let _ = (request, args);
        Err(miette::miette!(
            "finished-build packaging requires a supported Unix host"
        ))
    }
}
