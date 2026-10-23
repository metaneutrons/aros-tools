//! Bounded configured Mesa target-driver data for generated build rules.
//!
//! An empty `target_sysroot` is an explicit no-sysroot selection. It does not
//! request or infer an SDK path. A configured sysroot, when present, must be
//! kept outside Mesa's measured source and generated output roots.

use aros_common::mesa_configuration::{MesaRecipeConfiguration, ValidatedMesaConfiguration};
use std::path::Path;

/// The reviewed target compiler recipe and configured target triple.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MesaConfiguredDriver {
    /// Configure-measured recipe values, including the explicit sysroot mode.
    pub recipe: MesaRecipeConfiguration,
    /// Bounded target triple associated with the validated configuration.
    pub target_triple: String,
}

impl MesaConfiguredDriver {
    /// Project the target driver from already-validated configure data.
    ///
    /// # Errors
    /// Rejects an invalid recipe, target triple, or sysroot placement.
    pub fn project(configuration: &ValidatedMesaConfiguration) -> Result<Self, String> {
        let driver = Self {
            recipe: configuration.recipe_configuration().clone(),
            target_triple: configuration.identity().target_triple.clone(),
        };
        driver.validate()?;
        Ok(driver)
    }

    /// Revalidate public driver data before the generator publishes it.
    ///
    /// # Errors
    /// Rejects invalid recipe atoms, unsafe target-triple text, or a sysroot
    /// outside the permitted nominal Make build area.
    pub fn validate(&self) -> Result<(), String> {
        self.recipe
            .validate()
            .map_err(|error| format!("invalid configured Mesa recipe: {error}"))?;
        validate_target_triple(&self.target_triple)?;
        validate_sysroot_placement(&self.recipe)?;
        Ok(())
    }
}

fn validate_target_triple(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
    {
        return Err(
            "configured Mesa target triple must be nonempty ASCII identifier text of at most 128 bytes"
                .into(),
        );
    }
    Ok(())
}

fn validate_sysroot_placement(recipe: &MesaRecipeConfiguration) -> Result<(), String> {
    let Some(sysroot) = sysroot_path(&recipe.target_sysroot) else {
        // An explicitly empty recipe is the supported no-sysroot mode.
        return Ok(());
    };

    let sysroot = Path::new(sysroot);
    let coordinates = &recipe.make_coordinates;
    let build_root = Path::new(&coordinates.build_root);
    if !sysroot.starts_with(build_root) || sysroot == build_root {
        return Err(
            "configured Mesa target sysroot must be a strict descendant of the measured Make build root"
                .into(),
        );
    }
    if [&coordinates.generated_root, &coordinates.ports_root]
        .iter()
        .any(|root| sysroot.starts_with(root) || Path::new(root).starts_with(sysroot))
    {
        return Err(
            "configured Mesa target sysroot must be outside the measured generated and ports roots"
                .into(),
        );
    }
    Ok(())
}

fn sysroot_path(arguments: &[String]) -> Option<&str> {
    match arguments {
        [option, path] if option == "--sysroot" => Some(path),
        [argument] => argument.strip_prefix("--sysroot="),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aros_common::mesa_configuration::{
        MesaMakeCoordinates, MesaRecipeFlagValue, MesaTargetLto,
    };

    fn fixture() -> MesaConfiguredDriver {
        MesaConfiguredDriver {
            target_triple: "arm-unknown-aros".into(),
            recipe: MesaRecipeConfiguration {
                make_coordinates: MesaMakeCoordinates {
                    source_root: "/source".into(),
                    build_root: "/build".into(),
                    generated_root: "/build/bin/target/gen".into(),
                    ports_root: "/build/bin/target/Ports".into(),
                },
                safety_cflags: MesaRecipeFlagValue::Undefined,
                target_lto: MesaTargetLto::No,
                lto_cflags: vec![],
                cflags_iquote: vec!["-iquote".into()],
                cflags_iquote_end: vec![],
                target_cc: "/compiler/bin/clang".into(),
                target_cxx: "/compiler/bin/clang++".into(),
                target_sysroot: vec![
                    "--sysroot".into(),
                    "/build/bin/target/AROS/Developer".into(),
                ],
            },
        }
    }

    #[test]
    fn compiler_role_preserves_sysroot_shape_and_rejects_ambiguous_roots() {
        let original = fixture();
        original.validate().unwrap();
        let mut empty = original.clone();
        empty.recipe.target_sysroot.clear();
        empty.validate().unwrap();
        let mut equals = original.clone();
        equals.recipe.target_sysroot = vec!["--sysroot=/build/sdk/Developer".into()];
        equals.validate().unwrap();
        for path in [
            "/outside/sdk",
            "/build",
            "/build/bin/target/gen/sdk",
            "/build/bin/target/Ports/sdk",
            "/build/bin/target",
        ] {
            let mut invalid = original.clone();
            invalid.recipe.target_sysroot[1] = path.into();
            assert!(invalid.validate().is_err(), "{path}");
        }
        for target in ["", "arm;other", "arm\nother"] {
            let mut invalid = original.clone();
            invalid.target_triple = target.into();
            assert!(invalid.validate().is_err());
        }
    }
}
