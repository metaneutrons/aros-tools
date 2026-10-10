//! Read-only derivation of source-owned native SDK compatibility policy.
//!
//! The result binds one source preset to a pinned, recursively audited Git
//! checkout. It is a policy projection, not source-origin authentication or
//! release admission.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aros_common::{
    native_consumer_contract::load_bound_native_consumer_contract, sha256_bytes, CancellationToken,
    Sha256Digest, TargetProfile,
};
use serde::Serialize;

use crate::compatibility_source::CommittedSourceIdentity;
use crate::inspection::{self, Checkout};
use crate::source_audit::{self, Budget};
use crate::ContractError;

const SOURCE_POLICY_TIMEOUT: Duration = Duration::from_mins(5);
const SOURCE_PROFILE_FILE: &str = "aros-targets.toml";

/// Inputs for one policy selection from an exact source commit and tree.
#[derive(Debug, Clone)]
pub struct SourcePolicyRequest {
    /// AROS source checkout to inspect without writing to it.
    pub source_root: PathBuf,
    /// Source identity independently pinned by the caller.
    pub expected_source: CommittedSourceIdentity,
    /// Exact source-owned target preset name from `aros-targets.toml`.
    pub source_preset: String,
    /// Compiler profile selected by the recipe-bound producer profile matrix.
    pub compiler_profile: String,
}

/// Contract selection, if the chosen source profile declares one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeConsumerPolicy {
    /// Source-relative consumer-contract path.
    pub file: String,
    /// SHA-256 of the raw committed contract bytes.
    pub sha256: Sha256Digest,
    /// Validated native consumer contract schema.
    pub schema: String,
}

/// Source-derived policy for one selected compiler profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeSourcePolicy {
    /// Exact selected source commit object ID.
    pub source_commit: String,
    /// Exact selected source root tree object ID.
    pub source_tree: String,
    /// Source-owned preset name.
    pub source_preset: String,
    /// Compiler/runtime profile selected by the source preset.
    pub compiler_profile: String,
    /// SHA-256 of the raw committed `aros-targets.toml` bytes.
    pub profiles_sha256: Sha256Digest,
    /// Validated source consumer contract, when declared by the preset.
    pub native_consumer: Option<NativeConsumerPolicy>,
    /// True only when the bound consumer contract contains valid v2 SDK probes.
    pub native_sdk_required: bool,
}

/// Derive native SDK policy from a pinned and recursively clean source checkout.
///
/// This reads the selected source profile and every sealed consumer input from
/// raw Git blobs, validates them with the shared source-contract loader, audits
/// the complete checkout and initialized submodules, then rechecks selected
/// bytes and checkout identity. It writes no files and runs no source programs.
///
/// # Errors
/// Rejects a mismatched source identity, dirty checkout, invalid or ambiguous
/// source profile/contract, compiler-profile mismatch, or changed selected data.
pub fn inspect_native_source_policy(
    request: &SourcePolicyRequest,
) -> Result<NativeSourcePolicy, ContractError> {
    let deadline = Instant::now()
        .checked_add(SOURCE_POLICY_TIMEOUT)
        .ok_or_else(|| ContractError::preflight("source policy deadline is not representable"))?;
    let source_root = inspection::directory(&request.source_root)?;
    let (commit, tree) =
        inspection::observed_identity(&source_root, deadline, &CancellationToken::default())?;
    if request.expected_source.commit != commit || request.expected_source.tree != tree {
        return Err(ContractError::identity(
            "source checkout differs from the independently selected commit and tree",
        ));
    }
    let checkout = Checkout::inspect(&source_root, (&commit, &tree), deadline)?;

    let profile_bytes = checkout.required_file(SOURCE_PROFILE_FILE)?;
    let profile_text = std::str::from_utf8(&profile_bytes)
        .map_err(|_| ContractError::invalid("committed source profile is not UTF-8"))?;
    let profiles = TargetProfile::parse_config(profile_text, SOURCE_PROFILE_FILE)
        .map_err(|_| ContractError::invalid("committed source profile is invalid"))?;
    let profile = profiles
        .targets
        .iter()
        .find(|profile| profile.name == request.source_preset)
        .ok_or_else(|| ContractError::invalid("selected source preset is not declared"))?;
    if profile.toolchain_profile() != request.compiler_profile {
        return Err(ContractError::identity(
            "selected source preset maps to a different compiler profile",
        ));
    }

    let mut committed_inputs = Vec::<(String, Vec<u8>)>::new();
    let (native_consumer, native_sdk_required) = if let Some(relative) =
        profile.native_consumer_contract.as_deref()
    {
        let contract_bytes = checkout.required_file(relative)?;
        let loaded =
            load_bound_native_consumer_contract(&source_root, Path::new(relative), profile)
                .map_err(|_| {
                    ContractError::invalid(
                        "selected source consumer contract or its sealed inputs are invalid",
                    )
                })?;
        if loaded.sha256 != sha256_bytes(&contract_bytes) {
            return Err(ContractError::identity(
                "loaded source consumer contract differs from its committed bytes",
            ));
        }

        for input in &loaded.contract.inputs {
            let bytes = checkout.required_file(&input.path)?;
            if sha256_bytes(&bytes) != input.sha256 {
                return Err(ContractError::identity(
                    "sealed source consumer input differs from its committed bytes",
                ));
            }
            committed_inputs.push((input.path.clone(), bytes));
        }

        // The shared loader enforces the v1/v2 schema-to-probe relationship:
        // v1 has no ordinary SDK probes; v2 must carry both sealed probes.
        let native_sdk_required = loaded.contract.native_sdk_link_probes.is_some();
        if native_sdk_required {
            loaded
                .contract
                .require_native_sdk_link_probes()
                .map_err(|_| {
                    ContractError::invalid("source consumer v2 contract lacks required SDK probes")
                })?;
        }
        (
            Some(NativeConsumerPolicy {
                file: relative.to_owned(),
                sha256: loaded.sha256,
                schema: loaded.contract.schema,
            }),
            native_sdk_required,
        )
    } else {
        (None, false)
    };

    // The audit compares every tracked entry to raw Git material, rejects all
    // untracked/ignored entries, and recursively verifies initialized modules.
    source_audit::verify(&checkout, &mut Budget::new(deadline), 0)?;

    // Close the interval between contract loading and return for every selected
    // input; this still is not an atomic snapshot against concurrent writers.
    require_unchanged(&checkout, SOURCE_PROFILE_FILE, &profile_bytes)?;
    if let Some(consumer) = &native_consumer {
        let relative = &consumer.file;
        let contract_bytes = checkout.required_file(relative)?;
        if sha256_bytes(&contract_bytes) != consumer.sha256 {
            return Err(ContractError::identity(
                "source consumer contract changed during policy inspection",
            ));
        }
    }
    for (path, bytes) in &committed_inputs {
        require_unchanged(&checkout, path, bytes)?;
    }
    checkout.recheck()?;

    Ok(NativeSourcePolicy {
        source_commit: commit.as_str().to_owned(),
        source_tree: tree.as_str().to_owned(),
        source_preset: profile.name.clone(),
        compiler_profile: profile.toolchain_profile().to_owned(),
        profiles_sha256: sha256_bytes(&profile_bytes),
        native_consumer,
        native_sdk_required,
    })
}

fn require_unchanged(
    checkout: &Checkout<'_>,
    path: &str,
    expected: &[u8],
) -> Result<(), ContractError> {
    if checkout.required_file(path)? != expected {
        return Err(ContractError::identity(
            "selected source bytes changed during policy inspection",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::GitObjectId;
    use serde_json::{json, Value};
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::process::Command;

    const SOURCE_PRESET: &str = "esp32p4-riscv-gnu";
    const COMPILER_PROFILE: &str = "rv32-esp32p4";
    const CONTRACT_PATH: &str = "contracts/native-consumer.json";

    struct Fixture {
        _temporary: tempfile::TempDir,
        root: PathBuf,
        identity: CommittedSourceIdentity,
    }

    impl Fixture {
        fn new(schema: Option<&str>) -> Self {
            Self::with_options(schema, true)
        }

        fn with_options(schema: Option<&str>, write_contract: bool) -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().join("source");
            fs::create_dir(&root).unwrap();
            git(&root, &["init", "-q"]);

            let contract_declaration = schema.map_or(String::new(), |_| {
                format!("native_consumer_contract = '{CONTRACT_PATH}'\n")
            });
            let profile = format!(
                "[[targets]]\nname = '{SOURCE_PRESET}'\ntoolchain_profile = '{COMPILER_PROFILE}'\narch = 'riscv32'\nplatform = 'esp32p4'\nbsp = 'esp32p4'\nfloat_abi = 'ilp32f'\n{contract_declaration}[targets.transpiler]\nfamily = 'riscv'\nvariant = 'esp32p4'\ntoolchain = 'gnu'\ncpu32 = 'riscv'\nuse_mmu = false\n[targets.bootstrap_abi]\nflavour = 'native'\nplatform_smp = true\n"
            );
            fs::write(root.join(SOURCE_PROFILE_FILE), profile.as_bytes()).unwrap();
            fs::write(root.join("policy.json"), b"{}\n").unwrap();

            if let Some(schema) = schema.filter(|_| write_contract) {
                fs::create_dir_all(root.join("contracts")).unwrap();
                let mut inputs = vec![
                    json!({"path": SOURCE_PROFILE_FILE, "sha256": sha256_bytes(profile.as_bytes())}),
                    json!({"path": "policy.json", "sha256": sha256_bytes(b"{}\n")}),
                ];
                let mut consumer = json!({
                    "schema": schema,
                    "profile": SOURCE_PRESET,
                    "source_baseline": "0123456789abcdef0123456789abcdef01234567",
                    "roots": ["includes", "linklibs"],
                    "inputs": [],
                    "abi": {
                        "source_cpu": "riscv",
                        "target_triple": "riscv-aros",
                        "isa": "rv32imafc_zicsr_zifencei_zaamo_zalrsc",
                        "abi": "ilp32f",
                        "code_model": "medany",
                        "flavour": "native",
                        "platform_smp": true,
                        "use_mmu": false
                    },
                    "metamake_projection": "policy.json"
                });
                if schema == "aros-native-consumer-contract-v2" {
                    fs::write(root.join("probe.c"), b"int main(void) { return 0; }\n").unwrap();
                    fs::write(root.join("probe.cpp"), b"int main() { return 0; }\n").unwrap();
                    inputs.extend([
                        json!({"path": "probe.c", "sha256": sha256_bytes(b"int main(void) { return 0; }\n")}),
                        json!({"path": "probe.cpp", "sha256": sha256_bytes(b"int main() { return 0; }\n")}),
                    ]);
                    consumer["native_sdk_link_probes"] = json!({
                        "c": {"source": "probe.c", "libraries": []},
                        "cxx": {"source": "probe.cpp", "libraries": []}
                    });
                }
                consumer["inputs"] = Value::Array(inputs);
                fs::write(
                    root.join(CONTRACT_PATH),
                    serde_json::to_vec(&consumer).unwrap(),
                )
                .unwrap();
            }

            git(&root, &["add", "."]);
            git(&root, &["commit", "-qm", "fixture"]);
            let commit = GitObjectId::try_from(git(&root, &["rev-parse", "HEAD"])).unwrap();
            let tree = GitObjectId::try_from(git(&root, &["rev-parse", "HEAD:"])).unwrap();
            Self {
                _temporary: temporary,
                root: root.canonicalize().unwrap(),
                identity: CommittedSourceIdentity { commit, tree },
            }
        }

        fn request(&self) -> SourcePolicyRequest {
            SourcePolicyRequest {
                source_root: self.root.clone(),
                expected_source: self.identity.clone(),
                source_preset: SOURCE_PRESET.to_owned(),
                compiler_profile: COMPILER_PROFILE.to_owned(),
            }
        }

        fn recommit(&mut self) {
            git(&self.root, &["add", "."]);
            git(&self.root, &["commit", "-qm", "counterprobe"]);
            self.identity.commit =
                GitObjectId::try_from(git(&self.root, &["rev-parse", "HEAD"])).unwrap();
            self.identity.tree =
                GitObjectId::try_from(git(&self.root, &["rev-parse", "HEAD:"])).unwrap();
        }
    }

    fn git(root: &Path, arguments: &[&str]) -> String {
        let mut command = Command::new("git");
        command
            .current_dir(root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args([
                "-c",
                "user.name=Policy fixture",
                "-c",
                "user.email=policy-fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(arguments);
        let output =
            aros_common::run_output_with_timeout(&mut command, 64 * 1024, Duration::from_secs(10))
                .unwrap();
        assert!(
            output.status.success() && !output.timed_out,
            "git {arguments:?} failed: {output:?}"
        );
        String::from_utf8(output.stdout.exact_bytes().unwrap().to_vec())
            .unwrap()
            .trim()
            .to_owned()
    }

    #[test]
    fn derives_absent_v1_and_v2_policy_from_the_source_preset() {
        let absent = Fixture::new(None);
        let policy = inspect_native_source_policy(&absent.request()).unwrap();
        assert_eq!(policy.source_preset, SOURCE_PRESET);
        assert_eq!(policy.compiler_profile, COMPILER_PROFILE);
        assert!(policy.native_consumer.is_none());
        assert!(!policy.native_sdk_required);

        let v1 = Fixture::new(Some("aros-native-consumer-contract-v1"));
        let policy = inspect_native_source_policy(&v1.request()).unwrap();
        assert_eq!(
            policy.native_consumer.unwrap().schema,
            "aros-native-consumer-contract-v1"
        );
        assert!(!policy.native_sdk_required);

        let v2 = Fixture::new(Some("aros-native-consumer-contract-v2"));
        let policy = inspect_native_source_policy(&v2.request()).unwrap();
        assert_eq!(
            policy.native_consumer.unwrap().schema,
            "aros-native-consumer-contract-v2"
        );
        assert!(policy.native_sdk_required);
    }

    #[test]
    fn rejects_wrong_commit_tree_source_preset_and_compiler_mapping() {
        let fixture = Fixture::new(Some("aros-native-consumer-contract-v2"));
        let mut request = fixture.request();
        request.expected_source.commit = GitObjectId::try_from("a".repeat(40)).unwrap();
        assert!(inspect_native_source_policy(&request).is_err());

        let mut request = fixture.request();
        request.expected_source.tree = GitObjectId::try_from("b".repeat(40)).unwrap();
        assert!(inspect_native_source_policy(&request).is_err());

        let mut request = fixture.request();
        request.source_preset = "missing-preset".into();
        assert!(inspect_native_source_policy(&request).is_err());

        let mut request = fixture.request();
        request.compiler_profile = "different-compiler-profile".into();
        assert!(inspect_native_source_policy(&request).is_err());
    }

    #[test]
    fn rejects_untracked_source_and_changed_sealed_input() {
        let fixture = Fixture::new(None);
        fs::write(fixture.root.join("untracked"), b"not committed\n").unwrap();
        assert!(inspect_native_source_policy(&fixture.request()).is_err());

        let fixture = Fixture::new(Some("aros-native-consumer-contract-v1"));
        fs::write(fixture.root.join("policy.json"), b"changed\n").unwrap();
        assert!(inspect_native_source_policy(&fixture.request()).is_err());
    }

    #[test]
    fn rejects_missing_contract_and_profile_seal_drift() {
        let missing = Fixture::with_options(Some("aros-native-consumer-contract-v1"), false);
        assert!(inspect_native_source_policy(&missing.request()).is_err());

        let mut drifted = Fixture::new(Some("aros-native-consumer-contract-v1"));
        let profile_path = drifted.root.join(SOURCE_PROFILE_FILE);
        let mut profile = fs::read(&profile_path).unwrap();
        profile.extend_from_slice(b"\n# changed after the contract sealed it\n");
        fs::write(&profile_path, profile).unwrap();
        drifted.recommit();
        assert!(inspect_native_source_policy(&drifted.request()).is_err());
    }

    #[test]
    fn rejects_unsealed_v2_probe_unknown_schema_and_symlink_contract() {
        let mut unsealed = Fixture::new(Some("aros-native-consumer-contract-v2"));
        let path = unsealed.root.join(CONTRACT_PATH);
        let mut contract: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        contract["inputs"] = json!(contract["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|input| input["path"] != "probe.c")
            .cloned()
            .collect::<Vec<_>>());
        fs::write(&path, serde_json::to_vec(&contract).unwrap()).unwrap();
        unsealed.recommit();
        assert!(inspect_native_source_policy(&unsealed.request()).is_err());

        let unknown = Fixture::new(Some("aros-native-consumer-contract-v9"));
        assert!(inspect_native_source_policy(&unknown.request()).is_err());

        let mut linked = Fixture::new(Some("aros-native-consumer-contract-v1"));
        let path = linked.root.join(CONTRACT_PATH);
        let real = linked.root.join("contracts/consumer-copy.json");
        fs::copy(&path, &real).unwrap();
        fs::remove_file(&path).unwrap();
        symlink("consumer-copy.json", &path).unwrap();
        linked.recommit();
        assert!(inspect_native_source_policy(&linked.request()).is_err());
    }
}
