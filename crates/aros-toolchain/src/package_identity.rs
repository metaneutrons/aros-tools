//! Compiler-family package identities bound to parsed producer inputs.
//!
//! A packaging identity records selected inputs; it does not prove a fresh
//! compiler build, executable compatibility or runtime execution.

use aros_common::ArosCompilerIdentity;

use crate::package::canonical_asset_name;
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

pub fn require_gnu_recipe_binding(
    recipe: &Recipe,
    source_lock: &SourceLock,
    profile: &Profile,
) -> Result<(), ContractError> {
    if source_lock.family() == CompilerFamily::Gnu
        && (source_lock.sha256() != recipe.source_lock_sha256()
            || profile.document_sha256() != recipe.profiles_sha256())
    {
        return Err(ContractError::package(
            "GNU package inputs differ from the exact recipe-bound document bytes",
        ));
    }
    source_lock.verify_recipe_patches(recipe)
}

pub fn asset_name(
    source_lock: &SourceLock,
    profile: &Profile,
    host: &str,
) -> Result<String, ContractError> {
    let identity = compiler_identity(source_lock, profile)?;
    match identity {
        ArosCompilerIdentity::Llvm { version } => {
            canonical_asset_name(&version, host, profile.name())
        }
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
