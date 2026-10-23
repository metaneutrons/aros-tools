//! Closed transport for Mesa's configure-measured LLVM Make variables.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One explicitly defined or globally undefined Mesa Make value.
///
/// `Defined("")` is distinct from `Undefined`: the former projects an explicit
/// empty assignment to Make, while the latter contributes no assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MesaMakeVersionValue {
    Undefined,
    Defined(String),
}

/// Fixed projection of the configure-measured LLVM version variables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaLlvmConfiguration {
    pub opt_llvm: MesaMakeVersionValue,
    pub target_llvm_ver: MesaMakeVersionValue,
}

impl MesaLlvmConfiguration {
    /// Return only explicitly defined values, including explicitly empty ones.
    #[must_use]
    pub fn defined_variables(&self) -> BTreeMap<String, String> {
        let mut variables = BTreeMap::new();
        if let MesaMakeVersionValue::Defined(value) = &self.opt_llvm {
            variables.insert("OPT_LLVM".into(), value.clone());
        }
        if let MesaMakeVersionValue::Defined(value) = &self.target_llvm_ver {
            variables.insert("TARGET_LLVM_VER".into(), value.clone());
        }
        variables
    }

    /// Return the variable names whose configure-measured state is undefined.
    #[must_use]
    pub fn undefined_variables(&self) -> Vec<&'static str> {
        let mut variables = Vec::new();
        if matches!(&self.opt_llvm, MesaMakeVersionValue::Undefined) {
            variables.push("OPT_LLVM");
        }
        if matches!(&self.target_llvm_ver, MesaMakeVersionValue::Undefined) {
            variables.push("TARGET_LLVM_VER");
        }
        variables
    }

    /// Validate each explicitly defined LLVM value against the bounded version
    /// projection accepted by Mesa configuration transport.
    ///
    /// Empty defined values are valid. Nonempty values must have two or three
    /// canonical decimal components in the inclusive range 0 through 255.
    ///
    /// # Errors
    /// Returns a configuration error naming the LLVM variable with an
    /// unsupported value.
    pub fn validate(&self) -> crate::Result<()> {
        for (name, value) in [
            ("OPT_LLVM", &self.opt_llvm),
            ("TARGET_LLVM_VER", &self.target_llvm_ver),
        ] {
            if let MesaMakeVersionValue::Defined(value) = value {
                if !is_supported_version(value) {
                    return Err(crate::ArosError::Configuration {
                        file: super::CONFIGURATION_FILE.into(),
                        message: format!(
                            "{name} must be empty or a canonical two- or three-component decimal version with components from 0 through 255"
                        ),
                    });
                }
            }
        }
        Ok(())
    }
}

fn is_supported_version(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }

    let mut component_count = 0;
    for component in value.split('.') {
        component_count += 1;
        if component.is_empty()
            || (component.len() > 1 && component.starts_with('0'))
            || !component.bytes().all(|byte| byte.is_ascii_digit())
            || !component.parse::<u16>().is_ok_and(|number| number <= 255)
        {
            return false;
        }
    }

    matches!(component_count, 2 | 3)
}

#[cfg(test)]
#[path = "mesa_configuration_llvm_tests.rs"]
mod tests;
