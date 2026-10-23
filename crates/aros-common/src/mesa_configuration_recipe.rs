//! Data-only transport for the reviewed Mesa configure recipe values.
//!
//! This module validates the shape and safe spelling of transported values.
//! It does not identify a producer, prove that configure ran, or establish
//! parity with another build system.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A safety flag list measured as either globally undefined or explicitly
/// defined. An explicitly defined empty list remains distinct from undefined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MesaRecipeFlagValue {
    Undefined,
    Defined(Vec<String>),
}

/// Whether the reviewed target recipe enables link-time optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MesaTargetLto {
    Yes,
    No,
}

/// Nominal GNU Make coordinates, distinct from productive engine layout.
/// These are measured data, not authenticated producer or path ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaMakeCoordinates {
    pub source_root: String,
    pub build_root: String,
    pub ports_root: String,
    pub generated_root: String,
}

impl MesaMakeCoordinates {
    /// Validate lexical coordinates only; no filesystem or producer claim.
    ///
    /// # Errors
    /// Rejects unsafe paths and output roots outside the nominal build.
    pub fn validate(&self) -> crate::Result<()> {
        for (name, path) in [
            ("source_root", &self.source_root),
            ("build_root", &self.build_root),
            ("ports_root", &self.ports_root),
            ("generated_root", &self.generated_root),
        ] {
            validate_absolute_path(name, path)?;
            if path.contains(['<', '>', '"', '\\']) {
                return Err(super::configuration_error("unsafe Mesa Make coordinate"));
            }
        }
        for path in [&self.ports_root, &self.generated_root] {
            if !std::path::Path::new(path).starts_with(&self.build_root) || path == &self.build_root
            {
                return Err(super::configuration_error(
                    "Mesa Make output coordinate leaves its build",
                ));
            }
        }
        if std::path::Path::new(&self.ports_root).starts_with(&self.generated_root)
            || std::path::Path::new(&self.generated_root).starts_with(&self.ports_root)
        {
            return Err(super::configuration_error(
                "overlapping nominal Mesa output roots",
            ));
        }
        Ok(())
    }

    /// Map an independently selected physical original into the measured Make
    /// layout. This does not select a proxy candidate or prove file ownership.
    ///
    /// # Errors
    /// Rejects unknown, unsafe, ambiguous or escaping productive coordinates.
    pub fn nominal_original(
        &self,
        original: &std::path::Path,
        productive_build: &std::path::Path,
        productive_ports: &std::path::Path,
        productive_generated: &std::path::Path,
    ) -> crate::Result<std::path::PathBuf> {
        self.validate()?;
        for path in [
            original,
            productive_build,
            productive_ports,
            productive_generated,
        ] {
            validate_absolute_path(
                "productive Mesa coordinate",
                path.to_str()
                    .ok_or_else(|| super::configuration_error("non-UTF8 Mesa coordinate"))?,
            )?;
        }
        if productive_ports == productive_generated
            || productive_ports.starts_with(productive_generated)
            || productive_generated.starts_with(productive_ports)
            || [productive_ports, productive_generated]
                .iter()
                .any(|path| !path.starts_with(productive_build) || *path == productive_build)
        {
            return Err(super::configuration_error(
                "ambiguous or escaping productive Mesa roots",
            ));
        }
        // The productive build may lie inside SRCDIR. Its explicit output
        // lanes must win before source-root matching; their nominal Make
        // location can be outside SRCDIR in the private configure owner.
        for (physical, nominal) in [
            (productive_ports, self.ports_root.as_str()),
            (productive_generated, self.generated_root.as_str()),
            (productive_build, self.build_root.as_str()),
            (
                std::path::Path::new(&self.source_root),
                self.source_root.as_str(),
            ),
        ] {
            if let Ok(relative) = original.strip_prefix(physical) {
                if relative.as_os_str().is_empty() {
                    return Err(super::configuration_error(
                        "Mesa original cannot be a mapping root",
                    ));
                }
                return Ok(std::path::Path::new(nominal).join(relative));
            }
        }
        Err(super::configuration_error(
            "Mesa original is outside all selected coordinate roots",
        ))
    }
}

/// Configure-measured values for the reviewed clang-based Mesa target recipe.
///
/// Every field is required on the wire. Repeated and empty flag lists are
/// preserved exactly; this type does not infer defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaRecipeConfiguration {
    pub make_coordinates: MesaMakeCoordinates,
    pub safety_cflags: MesaRecipeFlagValue,
    pub target_lto: MesaTargetLto,
    pub lto_cflags: Vec<String>,
    pub cflags_iquote: Vec<String>,
    pub cflags_iquote_end: Vec<String>,
    pub target_cc: String,
    pub target_cxx: String,
    pub target_sysroot: Vec<String>,
}

impl MesaRecipeConfiguration {
    /// Validate only the bounded, closed data contract for the reviewed
    /// clang recipe. No filesystem lookup or producer claim is performed.
    ///
    /// # Errors
    /// Returns a configuration error for unsafe or unsupported flag atoms,
    /// quote flag layouts, driver paths, or sysroot arguments.
    pub fn validate(&self) -> crate::Result<()> {
        self.make_coordinates.validate()?;
        if let MesaRecipeFlagValue::Defined(flags) = &self.safety_cflags {
            super::validate_flag_atoms("safety_cflags", flags)?;
        }
        super::validate_flag_atoms("lto_cflags", &self.lto_cflags)?;

        if self.cflags_iquote.len() != 1 || self.cflags_iquote[0] != "-iquote" {
            return Err(super::configuration_error(
                "cflags_iquote must exactly match the reviewed [-iquote] recipe",
            ));
        }
        if !self.cflags_iquote_end.is_empty() {
            return Err(super::configuration_error(
                "cflags_iquote_end must be empty for the reviewed recipe",
            ));
        }

        validate_driver("target_cc", &self.target_cc, &["clang", "clang-11"])?;
        validate_driver("target_cxx", &self.target_cxx, &["clang++", "clang++-11"])?;
        validate_sysroot(&self.target_sysroot)?;

        Ok(())
    }

    /// Project the validated values to the Make variable names consumed by
    /// the reviewed recipe. Undefined safety flags are omitted; defined empty
    /// flags produce an explicit empty assignment.
    #[must_use]
    pub(crate) fn make_variables(&self) -> BTreeMap<String, String> {
        let mut variables = BTreeMap::from([
            (
                "TARGET_LTO".into(),
                match self.target_lto {
                    MesaTargetLto::Yes => "yes",
                    MesaTargetLto::No => "no",
                }
                .into(),
            ),
            ("LTO_CFLAGS".into(), self.lto_cflags.join(" ")),
            ("CFLAGS_IQUOTE".into(), self.cflags_iquote.join(" ")),
            ("CFLAGS_IQUOTE_END".into(), self.cflags_iquote_end.join(" ")),
            ("TARGET_CC".into(), self.target_cc.clone()),
            ("AROS_CXX".into(), self.target_cxx.clone()),
            ("TARGET_SYSROOT".into(), self.target_sysroot.join(" ")),
        ]);
        if let MesaRecipeFlagValue::Defined(flags) = &self.safety_cflags {
            variables.insert("SAFETY_CFLAGS".into(), flags.join(" "));
        }
        variables
    }

    /// Return only variables explicitly asserted undefined by this recipe.
    #[must_use]
    pub fn undefined_variables(&self) -> Vec<&'static str> {
        if matches!(&self.safety_cflags, MesaRecipeFlagValue::Undefined) {
            vec!["SAFETY_CFLAGS"]
        } else {
            Vec::new()
        }
    }
}

fn validate_driver(field: &str, value: &str, accepted_basenames: &[&str]) -> crate::Result<()> {
    validate_absolute_path(field, value)?;
    let basename = value.rsplit('/').next().unwrap_or_default();
    if !accepted_basenames.contains(&basename) {
        return Err(super::configuration_error(format!(
            "{field} must end in one of the reviewed clang driver names"
        )));
    }
    Ok(())
}

fn validate_sysroot(arguments: &[String]) -> crate::Result<()> {
    match arguments {
        [] => Ok(()),
        [option, path] if option == "--sysroot" => validate_absolute_path("target_sysroot", path),
        [argument] if argument.starts_with("--sysroot=") => {
            validate_absolute_path("target_sysroot", &argument["--sysroot=".len()..])
        }
        _ => Err(super::configuration_error(
            "target_sysroot must be empty or one exact --sysroot absolute-path option",
        )),
    }
}

/// Check canonical absolute path spelling lexically without consulting the
/// filesystem. Spaces are accepted as part of the single path string.
fn validate_absolute_path(field: &str, value: &str) -> crate::Result<()> {
    let invalid_syntax = value
        .chars()
        .any(|character| character.is_control() || matches!(character, '$' | ';' | '`'));
    let invalid_components = if value == "/" {
        false
    } else if !value.starts_with('/') || value.len() == 1 {
        true
    } else {
        value[1..]
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    };

    if invalid_syntax || invalid_components {
        return Err(super::configuration_error(format!(
            "{field} must be a normalized absolute path without control characters, dollar signs, semicolons, backticks, or dot components"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "mesa_configuration_recipe_tests.rs"]
mod tests;
