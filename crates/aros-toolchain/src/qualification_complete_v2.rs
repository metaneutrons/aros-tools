//! Acquire complete compiler and optional SDK selections; no authentication.

use crate::native_candidate::{
    readback_release_compatibility_v2, ReleaseCompatibilityReadbackRequestV2,
    ReleaseCompatibilityReadbackV2,
};
use crate::sdk_generation_matrix::{
    readback_sdk_generation_matrix, IndexedSdkGenerationReadback,
    SdkGenerationMatrixReadbackRequest,
};
use crate::ContractError;

/// Acquire SDK-bearing evidence through the complete matrix factory itself.
/// The nested matrix must use this exact compiler selection, not a second set
/// of roots whose proof could be paired with unrelated compiler observations.
pub fn acquire_complete(
    complete: &ReleaseCompatibilityReadbackRequestV2<'_>,
    sdk: Option<&SdkGenerationMatrixReadbackRequest<'_, '_>>,
) -> Result<
    (
        ReleaseCompatibilityReadbackV2,
        Vec<IndexedSdkGenerationReadback>,
    ),
    ContractError,
> {
    if complete.builds.packages.inputs.sdks().is_empty() {
        if sdk.is_some() {
            return Err(error(
                "compiler-only qualification cannot select an SDK matrix",
            ));
        }
        return Ok((readback_release_compatibility_v2(complete)?, Vec::new()));
    }
    let sdk = sdk.ok_or_else(|| {
        error("SDK qualification requires the complete input-derived generation matrix")
    })?;
    if !std::ptr::eq(complete, sdk.complete) {
        return Err(error(
            "SDK qualification matrix must use the exact compiler selection",
        ));
    }
    Ok(readback_sdk_generation_matrix(sdk)?.into_parts())
}

fn error(message: &str) -> ContractError {
    ContractError::recovery(message)
}
