//! Closed native-executor declaration and material binding.
//!
//! A declaration names the contract and committed producer inputs. It is a
//! selection document, not executable-origin evidence and not permission to
//! start a compiler. Keeping the parse/binding logic in Rust avoids a second
//! shell or workflow interpretation once `aros-toolchains` publishes it.

use std::collections::BTreeSet;

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

/// Selected raw inputs, shared by committed inspection and isolated execution.
pub(crate) struct SelectedNativeInputs {
    pub(crate) declaration: NativeExecutorDeclaration,
    pub(crate) source_lock: Vec<u8>,
    pub(crate) profiles: Vec<u8>,
}

/// Validated declaration plus the exact recipe-selected source/profile data.
#[derive(Debug, Clone)]
pub struct NativeInputContract {
    declaration: NativeExecutorDeclaration,
    source_lock: SourceLock,
    profiles: Profiles,
    selected_profile: Profile,
}

impl NativeExecutorDeclaration {
    /// Resolve a v1 input pair or a closed v2 input-group collection.
    ///
    /// The stable pathname denotes the native execution protocol, not this
    /// selection document's schema. V2 does not change receipt or execution
    /// protocol identities. All groups are read and validated before selecting
    /// the unique recipe-digest/profile match; malformed v2 never falls back to
    /// v1. The reader must provide committed or isolated snapshot bytes.
    pub(crate) fn select(
        input: &[u8],
        recipe: &Recipe,
        preset: &str,
        mut read: impl FnMut(&str) -> Result<Vec<u8>, ContractError>,
    ) -> Result<SelectedNativeInputs, ContractError> {
        if input.len() > crate::canonical::MAX_DOCUMENT_BYTES {
            return Err(ContractError::invalid(
                "native executor declaration exceeds 1 MiB",
            ));
        }
        let text = std::str::from_utf8(input)
            .map_err(|_| ContractError::invalid("native executor declaration is not UTF-8"))?;
        let header: SchemaHeader = toml::from_str(text)
            .map_err(|_| ContractError::invalid("invalid native executor declaration schema"))?;
        match header.schema_version {
            1 => {
                let declaration = Self::parse(input)?;
                Ok(SelectedNativeInputs {
                    source_lock: read(declaration.source_lock_path())?,
                    profiles: read(declaration.profiles_path())?,
                    declaration,
                })
            }
            2 => {
                let record: GroupRecord = toml::from_str(text).map_err(|_| {
                    ContractError::invalid("invalid closed native executor input groups")
                })?;
                record.select(recipe, preset, read)
            }
            _ => Err(ContractError::invalid(
                "unsupported native executor declaration schema",
            )),
        }
    }

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
        if profiles.family() != source_lock.family() {
            return Err(ContractError::identity(
                "native source lock and profiles select different compiler families",
            ));
        }
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

#[derive(Deserialize)]
struct SchemaHeader {
    schema_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupRecord {
    schema_version: u32,
    contract_id: String,
    contract_path: String,
    contract_sha256: Sha256Digest,
    tools_commit: GitObjectId,
    groups: Vec<InputGroup>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputGroup {
    id: String,
    source_lock: String,
    source_lock_sha256: Sha256Digest,
    profiles: String,
    profiles_sha256: Sha256Digest,
}

impl GroupRecord {
    fn select(
        self,
        recipe: &Recipe,
        preset: &str,
        mut read: impl FnMut(&str) -> Result<Vec<u8>, ContractError>,
    ) -> Result<SelectedNativeInputs, ContractError> {
        if self.schema_version != 2
            || self.contract_id != CONTRACT_ID
            || self.contract_path != CONTRACT_PATH
            || self.groups.is_empty()
            || self.groups.len() > 32
        {
            return Err(ContractError::invalid(
                "native executor input groups have unsupported identity or count",
            ));
        }
        let mut previous_id: Option<&str> = None;
        let mut paths = BTreeSet::new();
        for group in &self.groups {
            if !group_id(&group.id)
                || previous_id.is_some_and(|previous| previous >= group.id.as_str())
                || !producer_input_path(&group.source_lock, ".sources.json")
                || !producer_input_path(&group.profiles, ".json")
                || !paths.insert(group.source_lock.as_str())
                || !paths.insert(group.profiles.as_str())
            {
                return Err(ContractError::invalid(
                    "native executor input groups require sorted unique IDs and distinct safe paths",
                ));
            }
            previous_id = Some(&group.id);
        }
        let mut profile_names = BTreeSet::new();
        let mut selected = None;
        for group in &self.groups {
            let source_lock = read(&group.source_lock)?;
            let profiles = read(&group.profiles)?;
            if sha256_bytes(&source_lock) != group.source_lock_sha256
                || sha256_bytes(&profiles) != group.profiles_sha256
            {
                return Err(ContractError::identity(
                    "native executor input-group bytes differ from their declared digests",
                ));
            }
            let lock = SourceLock::parse(&source_lock)?;
            let matrix = Profiles::parse(&profiles)?;
            if lock.family() != matrix.family() {
                return Err(ContractError::identity(
                    "native executor input group mixes compiler families",
                ));
            }
            for profile in matrix.entries() {
                if !profile_names.insert(profile.name().to_owned()) {
                    return Err(ContractError::invalid(
                        "native executor profiles must be unique across input groups",
                    ));
                }
            }
            if group.source_lock_sha256 == *recipe.source_lock_sha256()
                && group.profiles_sha256 == *recipe.profiles_sha256()
                && matrix
                    .entries()
                    .iter()
                    .any(|profile| profile.name() == preset)
            {
                if selected.is_some() {
                    return Err(ContractError::invalid(
                        "native executor recipe selects ambiguous input groups",
                    ));
                }
                selected = Some(SelectedNativeInputs {
                    declaration: NativeExecutorDeclaration(Record {
                        schema_version: SCHEMA_VERSION,
                        contract_id: self.contract_id.clone(),
                        contract_path: self.contract_path.clone(),
                        contract_sha256: self.contract_sha256.clone(),
                        tools_commit: self.tools_commit.clone(),
                        source_lock: group.source_lock.clone(),
                        profiles: group.profiles.clone(),
                    }),
                    source_lock,
                    profiles,
                });
            }
        }
        selected.ok_or_else(|| {
            ContractError::identity(
                "native executor recipe and preset do not select one declared input group",
            )
        })
    }
}

fn group_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
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
    use crate::source_lock::SourceLock;
    use aros_common::sha256_bytes;
    use serde_json::json;
    use std::collections::BTreeMap;

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
            "patches": SourceLock::parse(lock).unwrap().source_patch_paths().map(|path| {
                json!({"path": path, "sha256": "e".repeat(64)})
            }).collect::<Vec<_>>()
        });
        value["recipe_sha256"] = json!(sha256_bytes(&canonical::bytes(&value).unwrap()));
        Recipe::parse(&serde_json::to_vec(&value).unwrap()).unwrap()
    }

    struct GroupFixture {
        declaration: Vec<u8>,
        documents: BTreeMap<String, Vec<u8>>,
        llvm_recipe: Recipe,
        gnu_recipe: Recipe,
    }

    impl GroupFixture {
        fn new() -> Self {
            let llvm_lock = source_lock();
            let llvm_profiles = profiles();
            let gnu_lock = include_bytes!("../tests/fixtures/gnu-source-lock-v3.json").to_vec();
            let gnu_profiles = serde_json::to_vec(&json!({
                "schema":"aros-toolchain-profiles-v2", "family":"gnu",
                "upstream_commit":"d".repeat(40), "profiles":[{
                    "name":"rv32-esp32p4", "configure_target":"esp32p4-riscv",
                    "upstream_output_target":"esp32p4-riscv", "target_triple":"riscv-aros",
                    "cpu":"riscv", "platform":"esp32p4", "float_abi":"ilp32f",
                    "capabilities":["c", "libgcc", "standalone-collector"],
                    "target":{"schema":"aros-riscv-target-v1", "isa":"rv32imafc", "abi":"ilp32f",
                        "code_model":"medany", "architecture":"rv32i2p1_m2p0_a2p1_f2p2_c2p0",
                        "unaligned_access":false, "atomic_abi":0, "x3_reg_usage":0}
                }]
            }))
            .unwrap();
            let declaration = format!(
                "schema_version = 2\ncontract_id = 'aros-toolchain-producer-v1'\ncontract_path = 'contracts/toolchain-producer-v1.toml'\ncontract_sha256 = '{}'\ntools_commit = '{}'\n\
                [[groups]]\nid = 'gnu-rv32'\nsource_lock = 'toolchains/gnu.sources.json'\nsource_lock_sha256 = '{}'\nprofiles = 'toolchains/gnu-profiles.json'\nprofiles_sha256 = '{}'\n\
                [[groups]]\nid = 'llvm'\nsource_lock = 'toolchains/llvm.sources.json'\nsource_lock_sha256 = '{}'\nprofiles = 'toolchains/llvm-profiles.json'\nprofiles_sha256 = '{}'\n",
                sha256_bytes(b"contract"), "5".repeat(40),
                sha256_bytes(&gnu_lock), sha256_bytes(&gnu_profiles),
                sha256_bytes(&llvm_lock), sha256_bytes(&llvm_profiles),
            ).into_bytes();
            Self {
                declaration,
                llvm_recipe: recipe(&llvm_lock, &llvm_profiles),
                gnu_recipe: recipe(&gnu_lock, &gnu_profiles),
                documents: BTreeMap::from([
                    ("toolchains/gnu.sources.json".into(), gnu_lock),
                    ("toolchains/gnu-profiles.json".into(), gnu_profiles),
                    ("toolchains/llvm.sources.json".into(), llvm_lock),
                    ("toolchains/llvm-profiles.json".into(), llvm_profiles),
                ]),
            }
        }

        fn select(
            &self,
            recipe: &Recipe,
            preset: &str,
        ) -> Result<super::SelectedNativeInputs, crate::ContractError> {
            NativeExecutorDeclaration::select(&self.declaration, recipe, preset, |path| {
                self.documents
                    .get(path)
                    .cloned()
                    .ok_or_else(|| crate::ContractError::invalid("fixture input missing"))
            })
        }

        fn replace(&mut self, from: &str, to: &str) {
            self.declaration = std::str::from_utf8(&self.declaration)
                .unwrap()
                .replace(from, to)
                .into_bytes();
        }
    }

    #[test]
    fn selects_both_compiler_families_from_one_closed_native_declaration() {
        let fixture = GroupFixture::new();
        for (recipe, preset, path, family) in [
            (
                &fixture.llvm_recipe,
                "pc-x86_64",
                "toolchains/llvm.sources.json",
                crate::source_lock::CompilerFamily::Llvm,
            ),
            (
                &fixture.gnu_recipe,
                "rv32-esp32p4",
                "toolchains/gnu.sources.json",
                crate::source_lock::CompilerFamily::Gnu,
            ),
        ] {
            let inputs = fixture.select(recipe, preset).unwrap();
            assert_eq!(inputs.declaration.source_lock_path(), path);
            let bound = inputs
                .declaration
                .bind(
                    recipe,
                    b"contract",
                    &inputs.source_lock,
                    &inputs.profiles,
                    preset,
                )
                .unwrap();
            assert_eq!(bound.source_lock().family(), family);
            assert_eq!(bound.selected_profile().name(), preset);
        }
    }

    #[test]
    fn rejects_cross_group_recipe_and_preset_selection() {
        let fixture = GroupFixture::new();
        assert!(fixture
            .select(&fixture.llvm_recipe, "rv32-esp32p4")
            .is_err());
        assert!(fixture.select(&fixture.gnu_recipe, "pc-x86_64").is_err());
        assert!(fixture.select(&fixture.gnu_recipe, "unknown").is_err());
    }

    #[test]
    fn rejects_changed_unselected_group_instead_of_ignoring_it() {
        let mut fixture = GroupFixture::new();
        fixture
            .documents
            .get_mut("toolchains/llvm.sources.json")
            .unwrap()
            .push(b' ');
        let error = fixture
            .select(&fixture.gnu_recipe, "rv32-esp32p4")
            .err()
            .unwrap();
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            aros_common::DiagnosticCode::ProducerIdentity
        );
        assert!(error.to_string().contains("declared digests"));
    }

    #[test]
    fn rejects_ambiguous_profile_names_even_when_raw_digests_match() {
        let mut fixture = GroupFixture::new();
        let original = fixture.documents["toolchains/gnu-profiles.json"].clone();
        let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
        value["profiles"][0]["name"] = json!("pc-x86_64");
        let changed = serde_json::to_vec(&value).unwrap();
        fixture.replace(
            sha256_bytes(&original).as_str(),
            sha256_bytes(&changed).as_str(),
        );
        fixture
            .documents
            .insert("toolchains/gnu-profiles.json".into(), changed);
        let error = fixture
            .select(&fixture.llvm_recipe, "pc-x86_64")
            .err()
            .unwrap();
        assert!(error.to_string().contains("unique across input groups"));
    }

    #[test]
    fn rejects_malformed_group_collections_without_v1_fallback() {
        for (from, to) in [
            ("schema_version = 2", "schema_version = 3"),
            ("id = 'gnu-rv32'", "id = 'LLVM'"),
            ("id = 'gnu-rv32'", "id = 'llvm'"),
            ("id = 'llvm'", "id = 'aaa'"),
            ("toolchains/gnu.sources.json", "../gnu.sources.json"),
            (
                "toolchains/llvm-profiles.json",
                "toolchains/gnu-profiles.json",
            ),
            ("[[groups]]", "unexpected = true\n[[groups]]"),
            (
                "schema_version = 2",
                "schema_version = 2\nsource_lock = 'toolchains/gnu.sources.json'",
            ),
        ] {
            let mut fixture = GroupFixture::new();
            fixture.replace(from, to);
            assert!(
                fixture.select(&fixture.gnu_recipe, "rv32-esp32p4").is_err(),
                "{from} -> {to}"
            );
        }
    }

    #[test]
    fn bounds_group_count_and_declaration_size_before_reading_any_inputs() {
        let fixture = GroupFixture::new();
        let record: toml::Value = toml::from_slice(&fixture.declaration).unwrap();
        for groups in [vec![], vec![record["groups"][0].clone(); 33]] {
            let mut changed = record.clone();
            changed["groups"] = toml::Value::Array(groups);
            let bytes = toml::to_string(&changed).unwrap().into_bytes();
            let error = NativeExecutorDeclaration::select(
                &bytes,
                &fixture.gnu_recipe,
                "rv32-esp32p4",
                |_| panic!("invalid group count must not read inputs"),
            )
            .err()
            .unwrap();
            assert!(error.to_string().contains("count"));
        }
        let oversized = vec![b' '; crate::canonical::MAX_DOCUMENT_BYTES + 1];
        let error = NativeExecutorDeclaration::select(
            &oversized,
            &fixture.gnu_recipe,
            "rv32-esp32p4",
            |_| panic!("oversized declaration must not read inputs"),
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("1 MiB"));
    }

    #[test]
    fn selected_group_still_requires_exact_tools_and_contract_binding() {
        for (from, to, expected) in [
            ("5".repeat(40), "6".repeat(40), "tools commit"),
            (
                sha256_bytes(b"contract").as_str().to_owned(),
                "a".repeat(64),
                "contract digest",
            ),
        ] {
            let mut fixture = GroupFixture::new();
            fixture.replace(&from, &to);
            let inputs = fixture.select(&fixture.gnu_recipe, "rv32-esp32p4").unwrap();
            let error = inputs
                .declaration
                .bind(
                    &fixture.gnu_recipe,
                    b"contract",
                    &inputs.source_lock,
                    &inputs.profiles,
                    "rv32-esp32p4",
                )
                .unwrap_err();
            assert_eq!(
                error.diagnostics().diagnostics[0].code,
                aros_common::DiagnosticCode::ProducerIdentity
            );
            assert!(error.to_string().contains(expected));
        }
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
    fn binds_gnu_without_substituting_llvm_identity_or_runtime() {
        let lock = include_bytes!("../tests/fixtures/gnu-source-lock-v3.json");
        let profiles = serde_json::to_vec(&json!({
            "schema":"aros-toolchain-profiles-v2", "family":"gnu",
            "upstream_commit":"d".repeat(40), "profiles":[{
                "name":"rv64-reference", "configure_target":"opensbi-riscv64",
                "upstream_output_target":"opensbi-riscv64", "target_triple":"riscv64-aros",
                "cpu":"riscv64", "platform":"opensbi", "float_abi":"lp64d",
                "capabilities":["c", "libgcc", "standalone-collector"],
                "target":{"schema":"aros-riscv-target-v1", "isa":"rva22u64", "abi":"lp64d",
                    "code_model":"medany", "architecture":"rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
                    "unaligned_access":false, "atomic_abi":0, "x3_reg_usage":0}
            }]
        }))
        .unwrap();
        let recipe = recipe(lock, &profiles);
        let contract = b"contract";
        let declaration = format!(
            "schema_version = 1\ncontract_id = \"aros-toolchain-producer-v1\"\ncontract_path = \"contracts/toolchain-producer-v1.toml\"\ncontract_sha256 = \"{}\"\ntools_commit = \"{}\"\nsource_lock = \"toolchains/gnu.sources.json\"\nprofiles = \"toolchains/profiles-v2.json\"\n",
            sha256_bytes(contract), recipe.tools().0.as_str()
        );
        let bound = NativeExecutorDeclaration::parse(declaration.as_bytes())
            .unwrap()
            .bind(&recipe, contract, lock, &profiles, "rv64-reference")
            .unwrap();
        assert_eq!(
            bound.source_lock().family(),
            crate::source_lock::CompilerFamily::Gnu
        );
        assert_eq!(bound.selected_profile().target_triple(), "riscv64-aros");
        assert_eq!(bound.selected_profile().target().unwrap().abi(), "lp64d");
        assert!(!bound
            .selected_profile()
            .capabilities()
            .iter()
            .any(|value| value == "libcxx"));
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
