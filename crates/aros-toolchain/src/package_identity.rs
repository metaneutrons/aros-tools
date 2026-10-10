//! Compiler-family package identities bound to parsed producer inputs.
//!
//! A packaging identity records selected inputs; it does not prove a fresh
//! compiler build, executable compatibility or runtime execution.

use aros_common::ArosCompilerIdentity;

use crate::package::{canonical_asset_name, PackageFormat};
use crate::profiles::Profile;
use crate::source_lock::{CompilerFamily, SourceLock};
use crate::{ContractError, Recipe};

pub fn compiler_identity(
    source_lock: &SourceLock,
    profile: &Profile,
) -> Result<ArosCompilerIdentity, ContractError> {
    if source_lock.family() != profile.family() {
        return Err(ContractError::package(
            "package source lock and profile select different compiler families",
        ));
    }
    let identity = match source_lock.family() {
        CompilerFamily::Llvm => ArosCompilerIdentity::Llvm {
            version: source_lock.version().to_owned(),
        },
        CompilerFamily::Gnu => {
            let binutils = source_lock
                .source_components()
                .find(|source| source.component() == "binutils")
                .ok_or_else(|| ContractError::package("GNU source lock omits binutils"))?;
            ArosCompilerIdentity::Gnu {
                gcc_version: source_lock.version().to_owned(),
                binutils_version: binutils.version().to_owned(),
                target: profile.target().cloned().ok_or_else(|| {
                    ContractError::package("GNU profile omits its measured target contract")
                })?,
            }
        }
    };
    // LLVM uses the unchanged legacy v1 version/name grammar below. The new
    // bounded v2 identity grammar must not retroactively narrow that contract.
    if source_lock.family() == CompilerFamily::Gnu {
        identity
            .validate_for_target(profile.target_triple())
            .map_err(ContractError::package)?;
    }
    Ok(identity)
}

pub fn compiler_identity_for_format(
    source_lock: &SourceLock,
    profile: &Profile,
    format: PackageFormat,
) -> Result<ArosCompilerIdentity, ContractError> {
    validate_format_family(source_lock.family(), format)?;
    let identity = compiler_identity(source_lock, profile)?;
    if format == PackageFormat::CompilerFamilyV2 {
        identity
            .validate_for_target(profile.target_triple())
            .map_err(ContractError::package)?;
        if source_lock.family() == CompilerFamily::Llvm {
            validate_family_llvm_profile(profile)?;
        }
    }
    Ok(identity)
}

fn validate_family_llvm_profile(profile: &Profile) -> Result<(), ContractError> {
    let triple = profile.target_triple().split('-').collect::<Vec<_>>();
    if triple.len() < 3
        || triple.iter().any(|component| component.is_empty())
        || triple.first().copied() != Some(profile.cpu())
        || triple.last().copied() != Some("aros")
    {
        return Err(ContractError::package(
            "compiler-family-v2 LLVM profile CPU must match the first target-triple component and the triple must end in -aros",
        ));
    }
    Ok(())
}

fn validate_format_family(
    family: CompilerFamily,
    format: PackageFormat,
) -> Result<(), ContractError> {
    if format == PackageFormat::LegacyLlvmV1 && family != CompilerFamily::Llvm {
        return Err(ContractError::package(
            "legacy-v1 package format is only supported for LLVM",
        ));
    }
    Ok(())
}

pub fn require_recipe_binding(
    recipe: &Recipe,
    source_lock: &SourceLock,
    profile: &Profile,
    format: PackageFormat,
) -> Result<(), ContractError> {
    validate_format_family(source_lock.family(), format)?;
    if (source_lock.family() == CompilerFamily::Gnu || format == PackageFormat::CompilerFamilyV2)
        && (source_lock.sha256() != recipe.source_lock_sha256()
            || profile.document_sha256() != recipe.profiles_sha256())
    {
        return Err(ContractError::package(
            if source_lock.family() == CompilerFamily::Gnu {
                "GNU package inputs differ from the exact recipe-bound document bytes"
            } else {
                "package inputs differ from the exact recipe-bound document bytes"
            },
        ));
    }
    source_lock.verify_recipe_patches(recipe)
}

pub fn asset_name_for_format(
    source_lock: &SourceLock,
    profile: &Profile,
    host: &str,
    format: PackageFormat,
) -> Result<String, ContractError> {
    let identity = compiler_identity_for_format(source_lock, profile, format)?;
    match identity {
        ArosCompilerIdentity::Llvm { version } => match format {
            PackageFormat::LegacyLlvmV1 => canonical_asset_name(&version, host, profile.name()),
            PackageFormat::CompilerFamilyV2 => {
                canonical_family_llvm_asset_name(&version, host, profile.name())
            }
        },
        ArosCompilerIdentity::Gnu {
            gcc_version,
            binutils_version,
            ..
        } => {
            if !matches!(host, "linux-x86_64" | "linux-aarch64" | "macos-aarch64") {
                return Err(ContractError::package("unsupported GNU package host"));
            }
            // The profile ID is producer-owned data, not a Rust board enum.
            // Its parser has already rejected unsafe and ambiguous selectors.
            Ok(format!(
                "aros-toolchain-v2-gcc{gcc_version}-binutils{binutils_version}-{host}-{}.tar.xz",
                profile.name()
            ))
        }
    }
}

fn canonical_family_llvm_asset_name(
    version: &str,
    host: &str,
    target_profile: &str,
) -> Result<String, ContractError> {
    if !matches!(host, "linux-x86_64" | "linux-aarch64" | "macos-aarch64") {
        return Err(ContractError::package(
            "compiler-family-v2 package identity has an unsupported LLVM host selector",
        ));
    }
    Ok(format!(
        "aros-toolchain-v2-llvm{version}-{host}-{target_profile}.tar.xz"
    ))
}
