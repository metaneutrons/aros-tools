//! Closed LLVM 11 target-side MCJIT capability, distinct from host cross tools.
//!
//! No Make shell predicate or wildcard patch command is executed. The audited
//! release layout uses the ordinary fetch engine and local AROS patch tracking.

use super::require_file_fingerprint;
use crate::{
    ast::ExternalCMakeDecl, fetch::FetchDecl, fingerprints::fingerprint, parser::TargetContext,
};
use std::path::{Path, PathBuf};

pub(crate) const DIRECTORY: &str = "workbench/libs/llvm";
pub(crate) const PREFIX: &str = "${AROS_BUILD_DIR}/gen/external-install/llvm11";

pub(crate) fn admit(
    root: &Path,
    relative: &Path,
    target: Option<&TargetContext>,
) -> Result<Option<(FetchDecl, ExternalCMakeDecl)>, String> {
    if relative != Path::new(DIRECTORY) {
        return Ok(None);
    }
    let Some(target) = target else {
        return Ok(None);
    };
    if target.cpu.as_deref() != Some("x86_64")
        || target.platform.as_deref() != Some("pc")
        || target.toolchain.as_deref() != Some("llvm")
        || target.cpu32.as_deref() != Some("i386")
        || target.use_mmu.as_deref() != Some("1")
    {
        return Ok(None);
    }
    if target
        .target_llvm_ver
        .as_deref()
        .is_some_and(|version| !version.is_empty() && version != "11.0.0")
    {
        return Err(format!(
            "native PC Target-LLVM capability requires 11.0.0, not {}; review the requested source/component contract",
            target.target_llvm_ver.as_deref().unwrap_or_default()
        ));
    }
    require_file_fingerprint(
        root,
        "workbench/libs/llvm/mmakefile.src",
        fingerprint("llvm11-target-recipe")?,
        "LLVM11 static target MCJIT",
    )?;
    let patch = root.join("tools/crosstools/llvm/llvm-11.0.0.src-aros.diff");
    let metadata = std::fs::symlink_metadata(&patch)
        .map_err(|error| format!("missing LLVM target patch: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err("LLVM target patch must be a regular source file".to_owned());
    }
    let components = include_str!("../../llvm11-x86-static.components")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut unique = std::collections::HashSet::new();
    if components.len() != 44
        || components.iter().any(|name| {
            !name.starts_with("LLVM")
                || !name.bytes().all(|byte| byte.is_ascii_alphanumeric())
                || !unique.insert(name)
        })
    {
        return Err("invalid closed LLVM11 static component inventory".to_owned());
    }
    let fetch = FetchDecl {
        name: "workbench-libs-llvm-fetch".to_owned(),
        archive: "llvm-11.0.0.src".to_owned(), suffixes: "tar.xz".to_owned(),
        origins: "https://github.com/llvm/llvm-project/releases/download/llvmorg-11.0.0".to_owned(),
        checksums: "llvm-11.0.0.src.tar.xz=sha256:913f68c898dfb4a03b397c5e11c6a2f39d0f22ed7665c9cefa87a34423a72469".to_owned(),
        normalization: String::new(), normalized_size: String::new(),
        location: "${AROS_PORTS_SOURCE_DIR}".to_owned(), destination: "${AROS_PORTS_DIR}/llvm".to_owned(),
        base: String::new(), patch_origins: "${AROS_SOURCE_DIR}/tools/crosstools/llvm".to_owned(),
        patches: "llvm-11.0.0.src-aros.diff:llvm-11.0.0.src:-p1".to_owned(), dir: DIRECTORY.to_owned(),
    };
    let options = [
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
        "-DCMAKE_BUILD_TYPE=Release",
        "-DLLVM_TARGETS_TO_BUILD=X86",
        "-DLLVM_DEFAULT_TARGET_TRIPLE=x86_64-unknown-aros",
        "-DLLVM_HOST_TRIPLE=x86_64-unknown-aros",
        "-DLLVM_ENABLE_PROJECTS=",
        "-DLLVM_ENABLE_RUNTIMES=",
        "-DLLVM_ENABLE_ASSERTIONS=OFF",
        "-DLLVM_ENABLE_BINDINGS=OFF",
        "-DLLVM_ENABLE_DOXYGEN=OFF",
        "-DLLVM_ENABLE_RTTI=ON",
        "-DLLVM_BUILD_LLVM_DYLIB=OFF",
        "-DLLVM_LINK_LLVM_DYLIB=OFF",
        "-DLLVM_INCLUDE_DOCS=OFF",
        "-DLLVM_INCLUDE_TESTS=OFF",
        "-DLLVM_INCLUDE_EXAMPLES=OFF",
        "-DLLVM_INCLUDE_BENCHMARKS=OFF",
        "-DLLVM_ENABLE_TERMINFO=OFF",
        "-DLLVM_ENABLE_LIBXML2=OFF",
        "-DLLVM_ENABLE_ZLIB=OFF",
        "-DLLVM_ENABLE_ZSTD=OFF",
        "-DLLVM_ENABLE_CURL=OFF",
        "-DLLVM_NO_DEAD_STRIP=ON",
        "-DBUILD_SHARED_LIBS=OFF",
        "-DHAVE_LIBDL:INTERNAL=0",
        "-DHAVE_LIBRT:INTERNAL=0",
        "-DUNIX=ON",
        "-DCMAKE_POSITION_INDEPENDENT_CODE=OFF",
        "-DLLVM_ENABLE_PIC=OFF",
        "-DLLVM_BUILD_TOOLS=OFF",
        "-DLLVM_INCLUDE_TOOLS=OFF",
        "-DLLVM_INCLUDE_UTILS=OFF",
        "-DLLVM_BUILD_UTILS=OFF",
    ]
    .map(str::to_owned)
    .to_vec();
    let declaration = ExternalCMakeDecl {
        mmake_name: "workbench-libs-llvm".to_owned(),
        source_dir: "${AROS_PORTS_DIR}/llvm/llvm-11.0.0.src".to_owned(),
        binary_dir: "${AROS_BUILD_DIR}/gen/external-cmake/workbench/libs/llvm/target".to_owned(),
        install_prefix: PREFIX.to_owned(),
        fetch_target: fetch.name.clone(),
        local_patch_files: vec![
            "${AROS_SOURCE_DIR}/tools/crosstools/llvm/llvm-11.0.0.src-aros.diff".to_owned(),
        ],
        provided_library: "LLVM".to_owned(),
        provider_target: "workbench-libs-llvm-external-LLVM".to_owned(),
        library_products: components
            .iter()
            .map(|name| format!("{PREFIX}/lib/lib{name}.a"))
            .collect(),
        header_products: [
            "llvm/Config/llvm-config.h",
            "llvm/Config/abi-breaking.h",
            "llvm/ExecutionEngine/ObjectCache.h",
            "llvm/ExecutionEngine/RTDyldMemoryManager.h",
            "llvm-c/Core.h",
            "llvm-c/ExecutionEngine.h",
            "llvm-c/Target.h",
            "llvm-c/Analysis.h",
        ]
        .map(|header| format!("{PREFIX}/include/{header}"))
        .to_vec(),
        auxiliary_products: Vec::new(),
        public_include_dirs: vec![format!("{PREFIX}/include")],
        options,
        build_targets: components,
        install_components: vec!["llvm-headers".to_owned()],
        library_group: true,
        host_tools: vec!["LLVM_TABLEGEN=llvm-tblgen@11.0.0".to_owned()],
        compile_defines: vec!["_GNU_SOURCE".to_owned()],
        dir_path: PathBuf::from(DIRECTORY),
    };
    Ok(Some((fetch, declaration)))
}
