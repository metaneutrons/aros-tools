//! Resolve the complete MetaMake selector context for source qualification.
//!
//! New target profiles own this data directly. The CMake bridge exists only
//! for already-published AROS-NX checkouts and is deliberately fail-closed: it
//! accepts explicit preset values plus a reviewed set of CMake defaults, and
//! asks for a tools update if that source contract changes.

use aros_common::TargetProfile;
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use serde_json::{Map, Value};
use std::fs;
use std::path::Path;

const CPU32_DEFAULT: &str =
    "if(AROS_TARGET_CPUSTREQUAL\"x86_64\")set(AROS_TARGET_CPU32\"i386\")else()set(AROS_TARGET_CPU32\"\")endif()";
const FAMILY_DEFAULT: &str = "set(AROS_TARGET_FAMILY\"\"CACHESTRING";
const VARIANT_DEFAULT: &str = "set(AROS_TARGET_VARIANT\"\"CACHESTRING";
const MMU_DEFAULT: &str = "option(AROS_ENABLE_MMU\"IncludetheMetaMakeMMUkernelsources\"ON)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedContext {
    pub family: String,
    pub variant: String,
    pub toolchain: String,
    pub cpu32: String,
    pub use_mmu: String,
    pub float_abi: String,
}

pub fn resolve(checkout: &Path, profile: &TargetProfile) -> Result<ResolvedContext> {
    if let Some(context) = &profile.transpiler {
        let explicit = ResolvedContext {
            family: context.family.clone(),
            variant: context.variant.clone(),
            toolchain: context.toolchain.clone(),
            cpu32: context.cpu32.clone(),
            use_mmu: if context.use_mmu { "1" } else { "0" }.to_owned(),
            float_abi: profile.float_abi.clone().unwrap_or_default(),
        };
        if checkout.join("CMakePresets.json").is_file() {
            let configured =
                resolve_legacy_cmake_bridge(checkout, profile).wrap_err_with(|| {
                    format!(
                    "could not prove that target profile '{}' matches its same-named CMake preset",
                    profile.name
                )
                })?;
            if configured != explicit {
                bail!(
                    "target profile '{}' [targets.transpiler] context differs from its same-named CMake preset/default context",
                    profile.name
                );
            }
        }
        return Ok(explicit);
    }

    resolve_legacy_cmake_bridge(checkout, profile).wrap_err_with(|| {
        format!(
            "target profile '{}' has no explicit [targets.transpiler] contract and its legacy CMake context could not be proven; add the five explicit selector fields or update aros-tools for the reviewed source change",
            profile.name
        )
    })
}

fn resolve_legacy_cmake_bridge(
    checkout: &Path,
    profile: &TargetProfile,
) -> Result<ResolvedContext> {
    let preset_path = checkout.join("CMakePresets.json");
    let document: Value = serde_json::from_slice(
        &fs::read(&preset_path)
            .into_diagnostic()
            .wrap_err_with(|| format!("could not read {}", preset_path.display()))?,
    )
    .into_diagnostic()
    .wrap_err_with(|| format!("could not parse {}", preset_path.display()))?;
    let presets = document
        .get("configurePresets")
        .and_then(Value::as_array)
        .ok_or_else(|| miette::miette!("CMakePresets.json has no configurePresets array"))?;
    let matches = presets
        .iter()
        .filter(|preset| preset.get("name").and_then(Value::as_str) == Some(&profile.name))
        .collect::<Vec<_>>();
    let [preset] = matches.as_slice() else {
        bail!(
            "CMakePresets.json must contain exactly one configure preset named '{}' (found {})",
            profile.name,
            matches.len()
        );
    };
    let cache = preset
        .get("cacheVariables")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            miette::miette!(
                "configure preset '{}' has no cacheVariables object",
                profile.name
            )
        })?;
    require_equal(cache, "AROS_TARGET_CPU", &profile.arch.to_string())?;
    require_equal(cache, "AROS_TARGET_PLATFORM", &profile.platform)?;
    let toolchain = required_string(cache, "AROS_TOOLCHAIN")?;
    if !portable_token(&toolchain) {
        bail!(
            "configure preset '{}' has an unsafe or empty AROS_TOOLCHAIN",
            profile.name
        );
    }

    let cmake_path = checkout.join("CMakeLists.txt");
    let cmake = fs::read_to_string(&cmake_path)
        .into_diagnostic()
        .wrap_err_with(|| format!("could not read {}", cmake_path.display()))?;
    let compact = cmake
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();

    let cpu32 = optional_string(cache, "AROS_TARGET_CPU32")?.unwrap_or_else(|| {
        if profile.arch.to_string() == "x86_64" {
            "i386".to_owned()
        } else {
            String::new()
        }
    });
    if !cache.contains_key("AROS_TARGET_CPU32") {
        require_default(&compact, CPU32_DEFAULT, "AROS_TARGET_CPU32")?;
    }
    let family = optional_string(cache, "AROS_TARGET_FAMILY")?.unwrap_or_default();
    if !cache.contains_key("AROS_TARGET_FAMILY") {
        require_default(&compact, FAMILY_DEFAULT, "AROS_TARGET_FAMILY")?;
    }
    let variant = optional_string(cache, "AROS_TARGET_VARIANT")?.unwrap_or_default();
    if !cache.contains_key("AROS_TARGET_VARIANT") {
        require_default(&compact, VARIANT_DEFAULT, "AROS_TARGET_VARIANT")?;
    }
    let use_mmu = if let Some(value) = optional_string(cache, "AROS_ENABLE_MMU")? {
        parse_bool(&value, "AROS_ENABLE_MMU")?
    } else {
        require_default(&compact, MMU_DEFAULT, "AROS_ENABLE_MMU")?;
        true
    };
    let float_abi = optional_string(cache, "GCC_CONFIG_FLOAT_ABI")?.unwrap_or_default();
    if float_abi != profile.float_abi.as_deref().unwrap_or_default() {
        bail!(
            "configure preset '{}' GCC_CONFIG_FLOAT_ABI does not match aros-targets.toml",
            profile.name
        );
    }
    for (name, value) in [
        ("AROS_TARGET_CPU32", &cpu32),
        ("AROS_TARGET_FAMILY", &family),
        ("AROS_TARGET_VARIANT", &variant),
    ] {
        if !value.is_empty() && !portable_token(value) {
            bail!(
                "configure preset '{}' has an unsafe {name} value",
                profile.name
            );
        }
    }

    Ok(ResolvedContext {
        family,
        variant,
        toolchain,
        cpu32,
        use_mmu: if use_mmu { "1" } else { "0" }.to_owned(),
        float_abi,
    })
}

fn require_default(compact: &str, contract: &str, name: &str) -> Result<()> {
    if !compact.contains(contract) {
        bail!(
            "the reviewed legacy CMake default for {name} changed; declare [targets.transpiler] explicitly or update aros-tools"
        );
    }
    Ok(())
}

fn require_equal(cache: &Map<String, Value>, name: &str, expected: &str) -> Result<()> {
    let actual = required_string(cache, name)?;
    if actual != expected {
        bail!("configure preset {name}={actual:?}; target profile requires {expected:?}");
    }
    Ok(())
}

fn required_string(cache: &Map<String, Value>, name: &str) -> Result<String> {
    optional_string(cache, name)?.ok_or_else(|| miette::miette!("configure preset omits {name}"))
}

fn optional_string(cache: &Map<String, Value>, name: &str) -> Result<Option<String>> {
    let Some(value) = cache.get(name) else {
        return Ok(None);
    };
    let value = match value {
        Value::String(value) => value,
        Value::Object(object) => object.get("value").and_then(Value::as_str).ok_or_else(|| {
            miette::miette!("configure preset {name} must have one literal string value")
        })?,
        _ => bail!("configure preset {name} must be a literal string"),
    };
    Ok(Some(value.to_owned()))
}

fn parse_bool(value: &str, name: &str) -> Result<bool> {
    match value.to_ascii_uppercase().as_str() {
        "1" | "ON" | "TRUE" | "YES" => Ok(true),
        "0" | "OFF" | "FALSE" | "NO" => Ok(false),
        _ => bail!("configure preset {name} must be an explicit CMake boolean"),
    }
}

fn portable_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aros_common::{Architecture, TranspilerProfile};

    fn profile(transpiler: Option<TranspilerProfile>) -> TargetProfile {
        TargetProfile {
            name: "pc-x86_64".into(),
            arch: Architecture::X86_64,
            platform: "pc".into(),
            bsp: "generic".into(),
            features: Vec::new(),
            float_abi: None,
            transpiler,
        }
    }

    #[test]
    fn explicit_context_needs_no_cmake_bridge() {
        let context = resolve(
            Path::new("/does/not/exist"),
            &profile(Some(TranspilerProfile {
                family: String::new(),
                variant: String::new(),
                toolchain: "llvm".into(),
                cpu32: "i386".into(),
                use_mmu: true,
            })),
        )
        .unwrap();
        assert_eq!(context.toolchain, "llvm");
        assert_eq!(context.cpu32, "i386");
        assert_eq!(context.use_mmu, "1");
    }

    #[test]
    fn legacy_bridge_requires_preset_and_reviewed_defaults() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("CMakePresets.json"),
            r#"{"configurePresets":[{"name":"pc-x86_64","cacheVariables":{"AROS_TARGET_CPU":"x86_64","AROS_TARGET_PLATFORM":"pc","AROS_TOOLCHAIN":"llvm"}}]}"#,
        )
        .unwrap();
        fs::write(
            directory.path().join("CMakeLists.txt"),
            "if(AROS_TARGET_CPU STREQUAL \"x86_64\")\nset(AROS_TARGET_CPU32 \"i386\")\nelse()\nset(AROS_TARGET_CPU32 \"\")\nendif()\nset(AROS_TARGET_FAMILY \"\" CACHE STRING \"family\")\nset(AROS_TARGET_VARIANT \"\" CACHE STRING \"variant\")\noption(AROS_ENABLE_MMU \"Include the MetaMake MMU kernel sources\" ON)\n",
        )
        .unwrap();
        let context = resolve(directory.path(), &profile(None)).unwrap();
        assert_eq!(context.cpu32, "i386");
        assert_eq!(context.toolchain, "llvm");
        assert_eq!(context.use_mmu, "1");

        let drift = resolve(
            directory.path(),
            &profile(Some(TranspilerProfile {
                family: String::new(),
                variant: String::new(),
                toolchain: "gnu".into(),
                cpu32: "i386".into(),
                use_mmu: true,
            })),
        )
        .unwrap_err();
        assert!(drift
            .to_string()
            .contains("differs from its same-named CMake preset"));

        fs::write(directory.path().join("CMakeLists.txt"), "changed\n").unwrap();
        let error = resolve(directory.path(), &profile(None)).unwrap_err();
        assert!(error
            .to_string()
            .contains("legacy CMake context could not be proven"));
    }

    #[test]
    fn qualified_source_profiles_resolve_complete_contexts_when_configured() {
        let Some(root) = std::env::var_os("AROS_TEST_SOURCE_ROOT") else {
            return;
        };
        let root = Path::new(&root);
        let profiles = TargetProfile::load_from_file(&root.join("aros-targets.toml")).unwrap();
        assert!(!profiles.is_empty());
        for profile in profiles {
            let context = resolve(root, &profile).unwrap_or_else(|error| {
                panic!(
                    "profile {} has no complete context: {error:#}",
                    profile.name
                )
            });
            assert!(!context.toolchain.is_empty());
            assert!(matches!(context.use_mmu.as_str(), "0" | "1"));
        }
    }
}
