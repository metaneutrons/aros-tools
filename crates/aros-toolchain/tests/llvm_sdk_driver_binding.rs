//! Opt-in measurement of an explicitly selected installed LLVM payload.
//!
//! This checks real driver queries, not package authenticity, ordinary SDK
//! linking, target ABI completeness, host coverage or release admission.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use aros_common::sdk_manifest::{SdkAbiValue, SdkTarget};
use aros_common::{ArosCompilerIdentity, CancellationToken};
use aros_toolchain::compatibility::{inspect_llvm_sdk_drivers, LlvmSdkDriverRequest};

#[test]
#[ignore = "requires an explicitly selected installed LLVM payload; never discovers host tools"]
fn selected_installed_llvm_drivers_report_exact_version_and_triple() {
    let compiler_root = PathBuf::from(
        std::env::var_os("AROS_TEST_LLVM_DRIVER_ROOT")
            .expect("explicit AROS_TEST_LLVM_DRIVER_ROOT is required"),
    );
    let version = std::env::var("AROS_TEST_LLVM_DRIVER_VERSION")
        .expect("explicit AROS_TEST_LLVM_DRIVER_VERSION is required");
    let target_triple = std::env::var("AROS_TEST_LLVM_DRIVER_TRIPLE")
        .expect("explicit AROS_TEST_LLVM_DRIVER_TRIPLE is required");
    let source_cpu = target_triple.split('-').next().unwrap().to_owned();
    let request = LlvmSdkDriverRequest {
        compiler_root,
        compiler: ArosCompilerIdentity::Llvm { version },
        target: SdkTarget {
            target_triple,
            abi: BTreeMap::from([("source_cpu".into(), SdkAbiValue::Text(source_cpu))]),
        },
        timeout: Duration::from_secs(30),
    };
    let binding = inspect_llvm_sdk_drivers(&request, &CancellationToken::default())
        .expect("both actual installed drivers must match the explicit selection");
    assert_eq!(binding.compiler(), &request.compiler);
    assert_eq!(binding.target(), &request.target);
    let root = request.compiler_root.canonicalize().unwrap();
    assert_eq!(binding.clang_path(), root.join("bin/clang"));
    assert_eq!(binding.clangxx_path(), root.join("bin/clang++"));
    for role in [binding.clang_identity(), binding.clangxx_identity()] {
        assert!(role.resolved_executable().starts_with(&root));
        assert!(role.file_size() > 0);
        assert!(role.version_query().size() > 0);
        assert!(role.target_query().size() > 0);
        assert_eq!(
            role.target_query().stdout_bytes(),
            format!("{}\n", request.target.target_triple).as_bytes()
        );
        for query in [role.version_query(), role.target_query()] {
            assert_eq!(query.size(), query.stdout_bytes().len() as u64);
            assert_eq!(
                query.sha256(),
                &aros_common::sha256_bytes(query.stdout_bytes())
            );
        }
    }
    binding
        .revalidate(&CancellationToken::default())
        .expect("the compiler tree must remain unchanged after its four driver queries");
}
