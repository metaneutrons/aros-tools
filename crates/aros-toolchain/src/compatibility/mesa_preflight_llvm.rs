//! Capture configured LLVM Make values without conflating empty and undefined.

use crate::ContractError;
use aros_common::mesa_configuration::{MesaLlvmConfiguration, MesaMakeVersionValue};
use std::collections::BTreeMap;

pub(super) const LLVM_MEASUREMENT_KEYS: &[&str] = &[
    "MESA_MEASURE_OPT_LLVM_STATE",
    "MESA_MEASURE_OPT_LLVM_VALUE",
    "MESA_MEASURE_TARGET_LLVM_VER_STATE",
    "MESA_MEASURE_TARGET_LLVM_VER_VALUE",
];

pub(super) const LLVM_MEASUREMENT_ASSIGNMENTS: &str = "\
MESA_MEASURE_OPT_LLVM_STATE := $(if $(filter undefined,$(origin OPT_LLVM)),undefined,defined)\n\
MESA_MEASURE_OPT_LLVM_VALUE := |$(OPT_LLVM)|\n\
MESA_MEASURE_TARGET_LLVM_VER_STATE := $(if $(filter undefined,$(origin TARGET_LLVM_VER)),undefined,defined)\n\
MESA_MEASURE_TARGET_LLVM_VER_VALUE := |$(TARGET_LLVM_VER)|\n";

pub(super) fn from_measurements(
    values: &BTreeMap<String, String>,
) -> Result<MesaLlvmConfiguration, ContractError> {
    let result = MesaLlvmConfiguration {
        opt_llvm: captured_value(values, "MESA_MEASURE_OPT_LLVM")?,
        target_llvm_ver: captured_value(values, "MESA_MEASURE_TARGET_LLVM_VER")?,
    };
    result
        .validate()
        .map_err(|_| failure("invalid measured Mesa LLVM configuration"))?;
    Ok(result)
}

fn captured_value(
    values: &BTreeMap<String, String>,
    prefix: &str,
) -> Result<MesaMakeVersionValue, ContractError> {
    let state = values
        .get(&format!("{prefix}_STATE"))
        .ok_or_else(|| failure("missing measured Mesa LLVM state"))?;
    // Sentinels keep leading/trailing whitespace inside the captured value,
    // rather than letting the general measurement parser trim it into validity.
    let value = values
        .get(&format!("{prefix}_VALUE"))
        .and_then(|value| value.strip_prefix('|'))
        .and_then(|value| value.strip_suffix('|'))
        .ok_or_else(|| failure("missing or unframed measured Mesa LLVM value"))?;
    match state.as_str() {
        "undefined" if value.is_empty() => Ok(MesaMakeVersionValue::Undefined),
        "defined" => Ok(MesaMakeVersionValue::Defined(value.into())),
        _ => Err(failure("inconsistent measured Mesa LLVM state/value")),
    }
}

fn failure(message: &str) -> ContractError {
    ContractError::compatibility(message)
}

#[cfg(test)]
#[path = "mesa_preflight_llvm_tests.rs"]
mod tests;
