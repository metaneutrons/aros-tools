//! Compiler-family arguments at the source-owned native build boundary.
//!
//! These selections come from the bound lock/profile, never a board name or
//! executable basename. LLVM retains its existing configure contract; GNU
//! selects both independently locked compiler components.

use crate::profiles::Profile;
use crate::source_lock::{CompilerFamily, SourceLock};
use crate::ContractError;

/// Select source-owned configure arguments for the bound compiler family.
///
/// # Errors
///
/// Rejects a family mismatch or a GNU lock without its binutils component.
pub fn configure_arguments(
    lock: &SourceLock,
    profile: &Profile,
) -> Result<Vec<String>, ContractError> {
    if lock.family() != profile.family() {
        return Err(ContractError::identity(
            "native configure source lock and profile select different compiler families",
        ));
    }
    match lock.family() {
        CompilerFamily::Llvm => Ok(vec![
            "--with-toolchain=llvm".to_owned(),
            format!("--with-llvm-version={}", lock.version()),
        ]),
        CompilerFamily::Gnu => {
            let binutils = lock
                .source_components()
                .find(|source| source.component() == "binutils")
                .ok_or_else(|| ContractError::invalid("native GNU input omits binutils"))?;
            Ok(vec![
                "--with-toolchain=gnu".to_owned(),
                format!("--with-gcc-version={}", lock.version()),
                format!("--with-binutils-version={}", binutils.version()),
            ])
        }
    }
}

/// Paths modified by collector installation; receipts measure all of them.
#[must_use]
pub fn collector_outputs(profile: &Profile) -> Vec<String> {
    match profile.family() {
        CompilerFamily::Llvm => vec!["bin/aros-collect".to_owned()],
        CompilerFamily::Gnu => {
            let triple = profile.target_triple();
            vec![
                format!("{triple}/bin/collect-aros"),
                format!("{triple}-collect-aros"),
                format!("{triple}/bin/aros-collector-tools.json"),
                "aros-collector-tools.json".to_owned(),
                "toolchain-tools.json".to_owned(),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::Profiles;
    use serde_json::json;

    fn input(family: &str) -> (SourceLock, Profile) {
        let mut lock = json!({
            "schema": "aros-toolchain-source-lock-v3", "family": family,
            "version": "11.0.0",
            "sources": [{"component": "llvm", "version": "11.0.0",
                "purpose": "toolchain-component", "filename": "llvm.tar.xz",
                "url": "https://example.invalid/llvm.tar.xz", "size": 1,
                "sha256": "a".repeat(64)}],
            "host_python_packages": [{"name": "mako", "version": "1.3.10",
                "filename": "mako.tar.gz", "url": "https://example.invalid/mako.tar.gz",
                "sha256": "b".repeat(64), "size": 1,
                "source_root": "mako-1.3.10", "python_path": "."}]
        });
        let mut profile = json!({
            "name": "test", "configure_target": "pc-x86_64",
            "upstream_output_target": "pc-x86_64", "target_triple": "x86_64-unknown-aros",
            "cpu": "x86_64", "platform": "pc", "float_abi": "", "capabilities": ["c"]
        });
        if family == "gnu" {
            lock["version"] = json!("16.2.0");
            lock["sources"] = json!([
                {"component":"gcc", "version":"16.2.0", "purpose":"toolchain-component",
                 "filename":"gcc.tar.xz", "url":"https://example.invalid/gcc.tar.xz",
                 "sha256":"a".repeat(64), "size":1},
                {"component":"binutils", "version":"2.47", "purpose":"toolchain-component",
                 "filename":"binutils.tar.bz2", "url":"https://example.invalid/binutils.tar.bz2",
                 "sha256":"b".repeat(64), "size":1}
            ]);
            profile["configure_target"] = json!("opensbi-riscv64");
            profile["upstream_output_target"] = json!("opensbi-riscv64");
            profile["target_triple"] = json!("riscv64-aros");
            profile["cpu"] = json!("riscv64");
            profile["platform"] = json!("opensbi");
            profile["float_abi"] = json!("lp64d");
            profile["capabilities"] = json!(["c", "libgcc", "standalone-collector"]);
            profile["target"] = json!({
                "schema":"aros-riscv-target-v1", "isa":"rva22u64", "abi":"lp64d",
                "code_model":"medany", "architecture":"rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
                "unaligned_access":false, "atomic_abi":0, "x3_reg_usage":0
            });
        }
        let lock = SourceLock::parse(&serde_json::to_vec(&lock).unwrap()).unwrap();
        let profiles = Profiles::parse(
            &serde_json::to_vec(&json!({
                "schema":"aros-toolchain-profiles-v2", "family":family,
                "upstream_commit":"a".repeat(40), "profiles":[profile]
            }))
            .unwrap(),
        )
        .unwrap();
        (lock, profiles.select("test").unwrap().clone())
    }

    #[test]
    fn arguments_use_locked_components_without_changing_llvm() {
        let (lock, profile) = input("llvm");
        assert_eq!(
            configure_arguments(&lock, &profile).unwrap(),
            ["--with-toolchain=llvm", "--with-llvm-version=11.0.0"]
        );
        assert_eq!(collector_outputs(&profile), ["bin/aros-collect"]);
        let (lock, profile) = input("gnu");
        assert_eq!(
            configure_arguments(&lock, &profile).unwrap(),
            [
                "--with-toolchain=gnu",
                "--with-gcc-version=16.2.0",
                "--with-binutils-version=2.47"
            ]
        );
        assert_eq!(collector_outputs(&profile).len(), 5);
        assert_eq!(
            collector_outputs(&profile)[0],
            "riscv64-aros/bin/collect-aros"
        );
    }

    #[test]
    fn mismatched_families_never_emit_configure_arguments() {
        let (llvm, _) = input("llvm");
        let (_, gnu_profile) = input("gnu");
        assert!(configure_arguments(&llvm, &gnu_profile)
            .unwrap_err()
            .to_string()
            .contains("AX0102"));
    }
}
