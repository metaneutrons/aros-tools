//! Closed native-executor declaration and material binding.
//!
//! A declaration names the contract and committed producer inputs. It is a
//! selection document, not executable-origin evidence and not permission to
//! start a compiler. Keeping the parse/binding logic in Rust avoids a second
//! shell or workflow interpretation once `aros-toolchains` publishes it.

use aros_common::{sha256_bytes, Sha256Digest};
use serde::Deserialize;

use crate::profiles::{Profile, Profiles};
use crate::recipe::{safe_relative_path, GitObjectId};
use crate::source_lock::SourceLock;
use crate::{ContractError, Recipe};

const SCHEMA_VERSION: u32 = 1;
const CONTRACT_ID: &str = "aros-toolchain-producer-v1";
const CONTRACT_PATH: &str = "contracts/toolchain-producer-v1.toml";

/// Validated native-executor declaration, before it is bound to file bytes.
#[derive(Debug, Clone)]
pub struct NativeExecutorDeclaration(Record);

/// Validated declaration plus the exact recipe-selected source/profile data.
#[derive(Debug, Clone)]
pub struct NativeInputContract {
    declaration: NativeExecutorDeclaration,
    source_lock: SourceLock,
    profiles: Profiles,
    selected_profile: Profile,
}

impl NativeExecutorDeclaration {
    /// Parse a bounded, closed native declaration without touching the filesystem.
    ///
    /// # Errors
    ///
    /// Returns AX0101 for unknown/duplicate/unsupported fields or unsafe
    /// relative input paths. A caller must bind the parsed declaration to
    /// committed bytes through [`Self::bind`].
    pub fn parse(input: &[u8]) -> Result<Self, ContractError> {
        if input.len() > crate::canonical::MAX_DOCUMENT_BYTES {
            return Err(ContractError::invalid(
                "native executor declaration exceeds 1 MiB",
            ));
        }
        let record: Record = toml::from_str(
            std::str::from_utf8(input)
                .map_err(|_| ContractError::invalid("native executor declaration is not UTF-8"))?,
        )
        .map_err(|_| {
            ContractError::invalid("invalid or unsupported native executor declaration")
        })?;
        validate(&record)?;
        Ok(Self(record))
    }

    /// Producer-relative source-lock path; it is not an arbitrary local path.
    #[must_use]
    pub fn source_lock_path(&self) -> &str {
        &self.0.source_lock
    }

    /// Tools-relative contract path whose committed bytes are bound below.
    #[must_use]
    pub fn contract_path(&self) -> &str {
        &self.0.contract_path
    }

    /// Declared digest of the selected tools contract document.
    #[must_use]
    pub const fn contract_sha256(&self) -> &Sha256Digest {
        &self.0.contract_sha256
    }

    /// Stable identifier of this closed contract format.
    #[must_use]
    pub fn contract_id(&self) -> &str {
        &self.0.contract_id
    }

    /// Producer-relative profiles path; it is not an arbitrary local path.
    #[must_use]
    pub fn profiles_path(&self) -> &str {
        &self.0.profiles
    }

    /// Tools source identity required by this native contract.
    #[must_use]
    pub const fn tools_commit(&self) -> &GitObjectId {
        &self.0.tools_commit
    }

    /// Bind the declaration to the exact recipe and committed raw documents.
    ///
    /// This is deliberately pure: snapshot creation, cache verification,
    /// external prerequisite discovery and executable-origin proof remain
    /// separate M2/M3 gates.
    ///
    /// # Errors
    ///
    /// Returns AX0102 for identity/digest disagreement and AX0101 for invalid
    /// source-lock or profiles documents. It never executes a source script.
    pub fn bind(
        &self,
        recipe: &Recipe,
        contract_bytes: &[u8],
        source_lock_bytes: &[u8],
        profiles_bytes: &[u8],
        preset: &str,
    ) -> Result<NativeInputContract, ContractError> {
        if &self.0.tools_commit != recipe.tools().0 {
            return Err(ContractError::identity(
                "native executor declaration tools commit differs from the selected recipe",
            ));
        }
        if sha256_bytes(contract_bytes) != self.0.contract_sha256 {
            return Err(ContractError::identity(
                "native executor declaration contract digest differs from the selected contract bytes",
            ));
        }
        if sha256_bytes(source_lock_bytes) != *recipe.source_lock_sha256() {
            return Err(ContractError::identity(
                "native executor declaration source lock differs from the selected recipe digest",
            ));
        }
        if sha256_bytes(profiles_bytes) != *recipe.profiles_sha256() {
            return Err(ContractError::identity(
                "native executor declaration profiles differ from the selected recipe digest",
            ));
        }
        let source_lock = SourceLock::parse(source_lock_bytes)?;
        source_lock.verify_recipe_patches(recipe)?;
        let profiles = Profiles::parse(profiles_bytes)?;
        let selected_profile = profiles.select(preset)?.clone();
        Ok(NativeInputContract {
            declaration: self.clone(),
            source_lock,
            profiles,
            selected_profile,
        })
    }
}

impl NativeInputContract {
    /// Immutable declaration that selected this contract.
    #[must_use]
    pub const fn declaration(&self) -> &NativeExecutorDeclaration {
        &self.declaration
    }

    /// Validated source payload closure.
    #[must_use]
    pub const fn source_lock(&self) -> &SourceLock {
        &self.source_lock
    }

    /// Validated producer profile matrix.
    #[must_use]
    pub const fn profiles(&self) -> &Profiles {
        &self.profiles
    }

    /// Exact selected profile with its capabilities.
    #[must_use]
    pub const fn selected_profile(&self) -> &Profile {
        &self.selected_profile
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema_version: u32,
    contract_id: String,
    contract_path: String,
    contract_sha256: Sha256Digest,
    tools_commit: GitObjectId,
    source_lock: String,
    profiles: String,
}

fn validate(record: &Record) -> Result<(), ContractError> {
    if record.schema_version != SCHEMA_VERSION
        || record.contract_id != CONTRACT_ID
        || record.contract_path != CONTRACT_PATH
        || !producer_input_path(&record.source_lock, ".sources.json")
        || !producer_input_path(&record.profiles, ".json")
    {
        return Err(ContractError::invalid(
            "native executor declaration has unsupported identity or input paths",
        ));
    }
    Ok(())
}

fn producer_input_path(path: &str, suffix: &str) -> bool {
    safe_relative_path(path) && path.starts_with("toolchains/") && path.ends_with(suffix)
}

#[cfg(test)]
mod tests {
    use aros_common::sha256_bytes;
    use serde_json::json;

    use super::NativeExecutorDeclaration;
    use crate::{canonical, Recipe};

    fn source_lock() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
            "sources": [{
                "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
                "patch": "tools/crosstools/llvm/llvm-11.0.0.src-aros.diff",
                "filename": "llvm-11.0.0.src.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
                "sha256": "a".repeat(64), "size": 1
            }],
            "host_python_packages": [
                {"name": "mako", "version": "1.3.10", "filename": "mako.tar.gz", "url": "https://example.invalid/mako.tar.gz", "sha256": "b".repeat(64), "size": 1, "source_root": "mako", "python_path": "."},
                {"name": "markupsafe", "version": "3.0.2", "filename": "markupsafe.tar.gz", "url": "https://example.invalid/markupsafe.tar.gz", "sha256": "c".repeat(64), "size": 1, "source_root": "markupsafe", "python_path": "."}
            ]
        }))
        .unwrap()
    }

    fn profiles() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v1", "upstream_commit": "d".repeat(40),
            "profiles": [{
                "name": "pc-x86_64", "configure_target": "pc-x86_64", "upstream_output_target": "pc-x86_64",
                "target_triple": "x86_64-unknown-aros", "cpu": "x86_64", "platform": "pc", "float_abi": "",
                "capabilities": ["c", "cxx", "standalone-collector"]
            }]
        }))
        .unwrap()
    }

    fn recipe(lock: &[u8], profiles: &[u8]) -> Recipe {
        let mut value = json!({
            "schema": "aros-toolchain-recipe-v2",
            "source_commit": "1".repeat(40), "source_tree": "2".repeat(40),
            "producer_commit": "3".repeat(40), "producer_tree": "4".repeat(40),
            "tools_commit": "5".repeat(40), "tools_tree": "6".repeat(40),
            "source_date_epoch": 0,
            "source_lock_sha256": sha256_bytes(lock), "profiles_sha256": sha256_bytes(profiles),
            "patches": [{"path": "tools/crosstools/llvm/llvm-11.0.0.src-aros.diff", "sha256": "e".repeat(64)}]
        });
        value["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&value).unwrap()));
        Recipe::parse(&serde_json::to_vec(&value).unwrap()).unwrap()
    }

    #[test]
    fn binds_one_native_declaration_to_exact_recipe_material() {
        let lock = source_lock();
        let profiles = profiles();
        let recipe = recipe(&lock, &profiles);
        let contract = b"[contract]\nid = 'aros-toolchain-producer-v1'\n";
        let declaration = format!(
            "schema_version = 1\ncontract_id = \"aros-toolchain-producer-v1\"\ncontract_path = \"contracts/toolchain-producer-v1.toml\"\ncontract_sha256 = \"{}\"\ntools_commit = \"{}\"\nsource_lock = \"toolchains/llvm-11.0.0.sources.json\"\nprofiles = \"toolchains/profiles-v1.json\"\n",
            sha256_bytes(contract), recipe.tools().0.as_str()
        );
        let declaration = NativeExecutorDeclaration::parse(declaration.as_bytes()).unwrap();
        let bound = declaration
            .bind(&recipe, contract, &lock, &profiles, "pc-x86_64")
            .unwrap();
        assert_eq!(
            bound.selected_profile().target_triple(),
            "x86_64-unknown-aros"
        );
        assert_eq!(bound.source_lock().sources().len(), 1);
    }

    #[test]
    fn rejects_unknown_paths_and_changed_material() {
        let invalid = b"schema_version = 1\ncontract_id = \"aros-toolchain-producer-v1\"\ncontract_path = \"contracts/toolchain-producer-v1.toml\"\ncontract_sha256 = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\ntools_commit = \"5555555555555555555555555555555555555555\"\nsource_lock = \"../escape.sources.json\"\nprofiles = \"toolchains/profiles-v1.json\"\n";
        assert!(NativeExecutorDeclaration::parse(invalid).is_err());

        let lock = source_lock();
        let profiles = profiles();
        let recipe = recipe(&lock, &profiles);
        let contract = b"contract";
        let declaration = format!(
            "schema_version = 1\ncontract_id = \"aros-toolchain-producer-v1\"\ncontract_path = \"contracts/toolchain-producer-v1.toml\"\ncontract_sha256 = \"{}\"\ntools_commit = \"{}\"\nsource_lock = \"toolchains/llvm-11.0.0.sources.json\"\nprofiles = \"toolchains/profiles-v1.json\"\n",
            sha256_bytes(contract), recipe.tools().0.as_str()
        );
        let declaration = NativeExecutorDeclaration::parse(declaration.as_bytes()).unwrap();
        assert!(declaration
            .bind(&recipe, b"changed", &lock, &profiles, "pc-x86_64")
            .is_err());
    }

    #[test]
    fn rejects_a_declaration_for_a_different_executor_revision() {
        let lock = source_lock();
        let profiles = profiles();
        let recipe = recipe(&lock, &profiles);
        let contract = b"contract";
        let declaration = format!(
            "schema_version = 1\ncontract_id = \"aros-toolchain-producer-v1\"\ncontract_path = \"contracts/toolchain-producer-v1.toml\"\ncontract_sha256 = \"{}\"\ntools_commit = \"{}\"\nsource_lock = \"toolchains/llvm-11.0.0.sources.json\"\nprofiles = \"toolchains/profiles-v1.json\"\n",
            sha256_bytes(contract),
            "6".repeat(40),
        );
        let declaration = NativeExecutorDeclaration::parse(declaration.as_bytes()).unwrap();
        let error = declaration
            .bind(&recipe, contract, &lock, &profiles, "pc-x86_64")
            .unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            aros_common::DiagnosticCode::ProducerIdentity
        );
    }
}
