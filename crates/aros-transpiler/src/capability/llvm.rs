//! Closed target-side LLVM MCJIT capabilities, distinct from host cross tools.
//!
//! No Make shell predicate or wildcard patch command is executed. The audited
//! release layout uses the ordinary fetch engine and local AROS patch tracking.

use super::require_file_fingerprint_one_of;
use crate::{
    ast::ExternalCMakeDecl, fetch::FetchDecl, fingerprints::fingerprint, parser::TargetContext,
};
use std::path::{Path, PathBuf};

pub(crate) const DIRECTORY: &str = "workbench/libs/llvm";
pub(crate) const LLVM11_PREFIX: &str = "${AROS_BUILD_DIR}/gen/external-install/llvm11";
pub(crate) const LLVM23_PREFIX: &str = "${AROS_BUILD_DIR}/gen/external-install/llvm23";

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
    let (backend, triple, llvm11_manifest, llvm23_manifest) = match (
        target.cpu.as_deref(),
        target.platform.as_deref(),
        target.toolchain.as_deref(),
        target.cpu32.as_deref(),
        target.use_mmu.as_deref(),
        target.float_abi.as_deref(),
    ) {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), _) => (
            "X86",
            "x86_64-unknown-aros",
            include_str!("../../llvm11-x86-static.components"),
            include_str!("../../llvm23-x86-static.components"),
        ),
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard")) => (
            "ARM",
            "arm-unknown-aros",
            include_str!("../../llvm11-arm-static.components"),
            include_str!("../../llvm23-arm-static.components"),
        ),
        (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("")) => (
            "AArch64",
            "aarch64-unknown-aros",
            include_str!("../../llvm11-aarch64-static.components"),
            include_str!("../../llvm23-aarch64-static.components"),
        ),
        _ => return Ok(None),
    };
    let recipe = root.join("workbench/libs/llvm/mmakefile.src");
    let requested_version = target
        .target_llvm_ver
        .as_deref()
        .filter(|version| !version.is_empty())
        .map_or_else(
            || {
                std::fs::read_to_string(&recipe)
                    .ok()
                    .filter(|source| source.contains("OPT_LLVM := 23.1.0"))
                    .map_or_else(|| "11.0.0".to_owned(), |_| "23.1.0".to_owned())
            },
            str::to_owned,
        );

    let (manifest, component_count, prefix, fetch, local_patch_files, host_tool) =
        match requested_version.as_str() {
            "11.0.0" => {
                require_file_fingerprint_one_of(
                    root,
                    "workbench/libs/llvm/mmakefile.src",
                    &[
                        fingerprint("llvm11-target-recipe")?,
                        fingerprint("llvm11-target-recipe-with-precompiled-headers")?,
                        fingerprint("llvm-target-recipe-with-llvm23-default")?,
                    ],
                    "LLVM 11 static target MCJIT",
                )?;
                let patch = "tools/crosstools/llvm/llvm-11.0.0.src-aros.diff";
                let fetch = FetchDecl {
                    name: "workbench-libs-llvm-fetch".to_owned(),
                    archive: "llvm-11.0.0.src".to_owned(),
                    suffixes: "tar.xz".to_owned(),
                    origins: "https://github.com/llvm/llvm-project/releases/download/llvmorg-11.0.0".to_owned(),
                    checksums: "llvm-11.0.0.src.tar.xz=sha256:913f68c898dfb4a03b397c5e11c6a2f39d0f22ed7665c9cefa87a34423a72469".to_owned(),
                    normalization: String::new(), normalized_size: String::new(),
                    location: "${AROS_PORTS_SOURCE_DIR}".to_owned(), destination: "${AROS_PORTS_DIR}/llvm".to_owned(),
                    base: String::new(), patch_origins: "${AROS_SOURCE_DIR}/tools/crosstools/llvm".to_owned(),
                    patches: "llvm-11.0.0.src-aros.diff:llvm-11.0.0.src:-p1".to_owned(), dir: DIRECTORY.to_owned(),
                };
                (
                    llvm11_manifest,
                    if backend == "X86" { 44 } else { 45 },
                    LLVM11_PREFIX,
                    fetch,
                    vec![format!("${{AROS_SOURCE_DIR}}/{patch}")],
                    "LLVM_TABLEGEN=llvm-tblgen@11.0.0",
                )
            }
            "23.1.0" => {
                require_file_fingerprint_one_of(
                    root,
                    "workbench/libs/llvm/mmakefile.src",
                    &[fingerprint("llvm-target-recipe-with-llvm23-default")?],
                    "LLVM 23 static target MCJIT",
                )?;
                let patch = "tools/crosstools/llvm/llvm-23.1.0.src-aros.diff";
                let fetch = FetchDecl {
                    name: "workbench-libs-llvm-project-fetch".to_owned(),
                    archive: "llvm-project-23.1.0.src".to_owned(),
                    suffixes: "tar.xz".to_owned(),
                    origins: "https://github.com/llvm/llvm-project/releases/download/llvmorg-23.1.0".to_owned(),
                    checksums: "llvm-project-23.1.0.src.tar.xz=sha256:ab1f0e3ec52448c33e8782eaf0422504b87c7b016b22514653ee0d8fcee479ff".to_owned(),
                    normalization: String::new(), normalized_size: String::new(),
                    location: "${AROS_PORTS_SOURCE_DIR}".to_owned(), destination: "${AROS_PORTS_DIR}/llvm/llvm-project".to_owned(),
                    base: String::new(), patch_origins: "${AROS_SOURCE_DIR}/tools/crosstools/llvm".to_owned(),
                    patches: "llvm-23.1.0.src-aros.diff:llvm-project-23.1.0.src/llvm:-p2".to_owned(), dir: DIRECTORY.to_owned(),
                };
                (
                    llvm23_manifest,
                    match backend { "X86" => 65, "ARM" => 63, _ => 66 },
                    LLVM23_PREFIX,
                    fetch,
                    vec![format!("${{AROS_SOURCE_DIR}}/{patch}")],
                    "LLVM_TABLEGEN=llvm-tblgen@23.1.0",
                )
            }
            version => return Err(format!(
                "native {backend} Target-LLVM capability supports 11.0.0 and 23.1.0, not {version}; review the requested source/component contract"
            )),
        };

    for relative_patch in &local_patch_files {
        let relative_patch = relative_patch
            .strip_prefix("${AROS_SOURCE_DIR}/")
            .expect("local patch paths are source-relative");
        let metadata = std::fs::symlink_metadata(root.join(relative_patch))
            .map_err(|error| format!("missing LLVM target patch: {error}"))?;
        if !metadata.file_type().is_file() {
            return Err("LLVM target patch must be a regular source file".to_owned());
        }
    }
    let components = manifest
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut unique = std::collections::HashSet::new();
    if components.len() != component_count
        || components.iter().any(|name| {
            !name.starts_with("LLVM")
                || !name.bytes().all(|byte| byte.is_ascii_alphanumeric())
                || !unique.insert(name)
        })
    {
        return Err(format!(
            "invalid closed LLVM {requested_version} static component inventory"
        ));
    }
    let mut options = [
        "-DCMAKE_DISABLE_PRECOMPILE_HEADERS=ON",
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
        "-DCMAKE_BUILD_TYPE=Release",
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
    // LLVM_HOST_TRIPLE names where these libraries run, not the build machine.
    options.splice(
        3..3,
        [
            format!("-DLLVM_TARGETS_TO_BUILD={backend}"),
            format!("-DLLVM_DEFAULT_TARGET_TRIPLE={triple}"),
            format!("-DLLVM_HOST_TRIPLE={triple}"),
        ],
    );
    let declaration = ExternalCMakeDecl {
        mmake_name: "workbench-libs-llvm".to_owned(),
        source_dir: if requested_version == "23.1.0" {
            "${AROS_PORTS_DIR}/llvm/llvm-project/llvm-project-23.1.0.src/llvm".to_owned()
        } else {
            "${AROS_PORTS_DIR}/llvm/llvm-11.0.0.src".to_owned()
        },
        binary_dir: "${AROS_BUILD_DIR}/gen/external-cmake/workbench/libs/llvm/target".to_owned(),
        install_prefix: prefix.to_owned(),
        fetch_target: fetch.name.clone(),
        local_patch_files,
        provided_library: "LLVM".to_owned(),
        provider_target: "workbench-libs-llvm-external-LLVM".to_owned(),
        library_products: components
            .iter()
            .map(|name| format!("{prefix}/lib/lib{name}.a"))
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
        .map(|header| format!("{prefix}/include/{header}"))
        .to_vec(),
        auxiliary_products: Vec::new(),
        public_include_dirs: vec![format!("{prefix}/include")],
        options,
        build_targets: components,
        install_components: vec!["llvm-headers".to_owned()],
        library_group: true,
        host_tools: vec![host_tool.to_owned()],
        compile_defines: vec!["_GNU_SOURCE".to_owned()],
        dir_path: PathBuf::from(DIRECTORY),
    };
    Ok(Some((fetch, declaration)))
}
