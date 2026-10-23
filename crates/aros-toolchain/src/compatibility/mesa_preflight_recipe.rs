//! Required source Make recipe observations, separate from compiler features.

use crate::ContractError;
use aros_common::mesa_configuration::MesaRecipeConfiguration;
use std::collections::BTreeMap;

pub(super) const RECIPE_MEASUREMENT_KEYS: &[&str] = &[
    "MESA_MEASURE_SAFETY_STATE",
    "MESA_MEASURE_SAFETY_VALUE",
    "TARGET_LTO",
    "LTO_CFLAGS",
    "CFLAGS_IQUOTE",
    "CFLAGS_IQUOTE_END",
    "TARGET_CC",
    "AROS_CXX",
    "TARGET_SYSROOT",
    "MESA_MEASURE_TOP",
    "MESA_MEASURE_SRCDIR",
    "MESA_MEASURE_PORTSDIR",
    "MESA_MEASURE_GENDIR",
];
pub(super) const RECIPE_MEASUREMENT_ASSIGNMENTS: &str = "\
MESA_MEASURE_SAFETY_STATE := $(if $(filter undefined,$(origin SAFETY_CFLAGS)),undefined,defined)\n\
MESA_MEASURE_SAFETY_VALUE := |$(SAFETY_CFLAGS)|\n\
MESA_MEASURE_TOP := $(TOP)\n\
MESA_MEASURE_SRCDIR := $(SRCDIR)\n\
MESA_MEASURE_PORTSDIR := $(PORTSDIR)\n\
MESA_MEASURE_GENDIR := $(GENDIR)\n";

pub(super) fn from_measurements(
    values: &BTreeMap<String, String>,
) -> Result<MesaRecipeConfiguration, ContractError> {
    let value = |name: &str| {
        values
            .get(name)
            .ok_or_else(|| failure("missing required Mesa recipe observation"))
    };
    let words = |name: &str| -> Result<Vec<&str>, ContractError> {
        Ok(value(name)?.split_whitespace().collect())
    };
    let state = value("MESA_MEASURE_SAFETY_STATE")?;
    let safety = value("MESA_MEASURE_SAFETY_VALUE")?
        .strip_prefix('|')
        .and_then(|raw| raw.strip_suffix('|'))
        .ok_or_else(|| failure("unframed Mesa safety flags"))?;
    let safety = match state.as_str() {
        "undefined" if safety.is_empty() => serde_json::json!({"state":"undefined"}),
        "defined" if safety.trim() == safety && !safety.contains(['\n', '\r', '\t']) => {
            serde_json::json!({"state":"defined","value":safety.split_whitespace().collect::<Vec<_>>()})
        }
        _ => return Err(failure("inconsistent Mesa safety flag state/value")),
    };
    let configured: MesaRecipeConfiguration = serde_json::from_value(serde_json::json!({
        "make_coordinates": {
            "source_root":value("MESA_MEASURE_SRCDIR")?,
            "build_root":value("MESA_MEASURE_TOP")?,
            "ports_root":value("MESA_MEASURE_PORTSDIR")?,
            "generated_root":value("MESA_MEASURE_GENDIR")?,
        },
        "safety_cflags":safety, "target_lto":value("TARGET_LTO")?,
        "lto_cflags":words("LTO_CFLAGS")?, "cflags_iquote":words("CFLAGS_IQUOTE")?,
        "cflags_iquote_end":words("CFLAGS_IQUOTE_END")?,
        "target_cc":value("TARGET_CC")?, "target_cxx":value("AROS_CXX")?,
        "target_sysroot":words("TARGET_SYSROOT")?,
    }))
    .map_err(|_| failure("invalid Mesa recipe observations"))?;
    configured
        .validate()
        .map_err(|_| failure("unsafe or unsupported Mesa recipe observations"))?;
    Ok(configured)
}

fn failure(message: &str) -> ContractError {
    ContractError::compatibility(message)
}
