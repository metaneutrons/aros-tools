//! The three local `%build_with_configure` declarations.
//!
//! ADFlib twice, host and target, and wpa_supplicant once. The macro they use
//! hands a build an open-ended configure environment; the downstream
//! `aros_build_configure` re-checks the same product and path shape
//! independently, and the runner it generates is a closed contract rather than
//! shell text. What is decided here is which of the three audited capabilities
//! a declaration is, and it fails closed for anything else.

use super::require_exact_macro_arguments;
use crate::ast::ConfigureBuildDecl;
use crate::parser::{macro_arg, Invocation, TargetContext};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path};

const ADFLIB_CONFIGURE_DIR: &str = "tools/ADFlib";
const ADFLIB_CONFIGURE_MANIFEST: &str = "tools/ADFlib/adflib-configure.inputs";
const WIRELESS_CONFIGURE_DIR: &str = "workbench/network/WirelessManager/wpa_supplicant";
const WIRELESS_CONFIGURE_SOURCE_ROOT: &str = "workbench/network/WirelessManager";
const WIRELESS_CONFIGURE_MANIFEST: &str =
    "workbench/network/WirelessManager/wirelessmanager-configure.inputs";

const ADFLIB_PUBLIC_HEADERS: &[&str] = &[
    "adf_defs.h",
    "adf_blk.h",
    "adf_err.h",
    "adf_str.h",
    "adflib.h",
    "adf_bitm.h",
    "adf_cache.h",
    "adf_dir.h",
    "adf_disk.h",
    "adf_dump.h",
    "adf_env.h",
    "adf_file.h",
    "adf_hd.h",
    "adf_link.h",
    "adf_raw.h",
    "adf_salv.h",
    "adf_util.h",
    "defendian.h",
    "hd_blk.h",
    "prefix.h",
    "adf_nativ.h",
];

/// Verifies the complete source allowlist before a configure-style capability
/// is admitted. Content is deliberately not pinned: CMake snapshots live
/// hashes into the runner contract and watches every listed file for changes.
fn validate_configure_input_manifest(
    root: &Path,
    source_dir: &str,
    manifest: &str,
) -> std::result::Result<(), String> {
    let manifest_path = root.join(manifest);
    let bytes = fs::read(&manifest_path)
        .map_err(|reason| format!("cannot read input manifest {manifest}: {reason}"))?;
    let body = std::str::from_utf8(&bytes)
        .map_err(|_| format!("input manifest {manifest} is not UTF-8"))?;
    let source_root = root.join(source_dir);
    let mut paths = HashSet::new();
    let mut count = 0usize;
    for (index, line) in body.lines().enumerate() {
        let relative = line.trim();
        if relative != line || relative.is_empty() {
            return Err(format!(
                "input manifest {manifest}:{} is not one canonical relative path",
                index + 1
            ));
        }
        let path = Path::new(relative);
        if relative.is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(format!(
                "input manifest {manifest}:{} has an unsafe path `{relative}`",
                index + 1
            ));
        }
        if !paths.insert(relative.to_owned()) {
            return Err(format!(
                "input manifest {manifest}:{} repeats `{relative}`",
                index + 1
            ));
        }
        let input = source_root.join(path);
        let metadata = fs::metadata(&input).map_err(|reason| {
            format!(
                "input manifest {manifest}:{} cannot read {relative}: {reason}",
                index + 1
            )
        })?;
        if !metadata.is_file() {
            return Err(format!(
                "input manifest {manifest}:{} does not name a regular file: {relative}",
                index + 1
            ));
        }
        count += 1;
    }
    if count == 0 {
        return Err(format!("input manifest {manifest} is empty"));
    }
    Ok(())
}

fn configure_profile_is_supported(
    target: Option<&TargetContext>,
) -> std::result::Result<(), String> {
    let Some(profile) = target else {
        return Err("configure-style capability requires a concrete target profile".to_owned());
    };
    let key = (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    );
    match key {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some(""))
        | (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard"))
        | (
            Some("aarch64"),
            Some("raspi"),
            Some("llvm"),
            Some(""),
            Some("1"),
            Some(""),
        ) => Ok(()),
        _ => Err(format!(
            "configure-style capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
            profile.cpu.as_deref().unwrap_or("<unset>"),
            profile.platform.as_deref().unwrap_or("<unset>"),
            profile.toolchain.as_deref().unwrap_or("<unset>"),
            profile.cpu32.as_deref().unwrap_or("<unset>"),
            profile.use_mmu.as_deref().unwrap_or("<unset>"),
            profile.float_abi.as_deref().unwrap_or("<unset>")
        )),
    }
}

/// Parses the deliberately small, local-source subset of
/// `%build_with_configure`.  No legacy command/environment text is forwarded:
/// a target is admitted only when its identity, argument set, profile and full
/// source manifest match one of these closed contracts.
pub(crate) fn parse(
    root: &Path,
    invocation: &Invocation,
    relative_dir: &Path,
    target: Option<&TargetContext>,
) -> std::result::Result<ConfigureBuildDecl, String> {
    configure_profile_is_supported(target)?;
    let mmake = macro_arg(&invocation.args, "mmake")
        .ok_or_else(|| "missing required mmake= argument".to_owned())?;

    if relative_dir == Path::new(ADFLIB_CONFIGURE_DIR) && mmake == "host-adflib" {
        require_exact_macro_arguments(
            invocation,
            &[
                ("mmake", "host-adflib"),
                ("compiler", "host"),
                ("prefix", "$(CROSSTOOLSDIR)"),
            ],
        )?;
        validate_configure_input_manifest(root, ADFLIB_CONFIGURE_DIR, ADFLIB_CONFIGURE_MANIFEST)?;
        let binary_dir = "${AROS_BUILD_DIR}/gen/configure/tools/ADFlib/host".to_owned();
        let install_prefix = "${AROS_BUILD_DIR}/hosttools".to_owned();
        let mut install_products = vec![format!("{install_prefix}/lib/libadf.a")];
        install_products.extend(
            ADFLIB_PUBLIC_HEADERS
                .iter()
                .map(|header| format!("{install_prefix}/include/{header}")),
        );
        install_products.push(format!("{install_prefix}/lib/pkgconfig/adflib.pc"));
        return Ok(ConfigureBuildDecl {
            mmake_name: mmake,
            mode: "adflib-host".to_owned(),
            source_dir: "${CMAKE_SOURCE_DIR}/tools/ADFlib".to_owned(),
            binary_dir: binary_dir.clone(),
            install_prefix,
            input_manifest: "${CMAKE_SOURCE_DIR}/tools/ADFlib/adflib-configure.inputs".to_owned(),
            private_products: vec![format!("{binary_dir}/build/libadf.a")],
            install_products,
            dependency_targets: Vec::new(),
            provided_library: None,
            provider_target: None,
            dir_path: relative_dir.to_path_buf(),
        });
    }

    if relative_dir == Path::new(ADFLIB_CONFIGURE_DIR) && mmake == "linklib-adflib" {
        require_exact_macro_arguments(
            invocation,
            &[
                ("mmake", "linklib-adflib"),
                ("prefix", "$(AROS_DEVELOPER)"),
                ("extraoptions", "$(AROSADFLIB_OPTS)"),
                ("config_env_extra", "$(AROSADFLIB_ENV)"),
                ("use_build_env", "yes"),
                ("nlsflag", "no"),
                ("xflag", "no"),
            ],
        )?;
        validate_configure_input_manifest(root, ADFLIB_CONFIGURE_DIR, ADFLIB_CONFIGURE_MANIFEST)?;
        let binary_dir = "${AROS_BUILD_DIR}/gen/configure/tools/ADFlib/target".to_owned();
        let install_prefix = "${AROS_BUILD_DIR}/SYS/Developer".to_owned();
        let mut install_products = vec![format!("{install_prefix}/lib/libadf.a")];
        install_products.extend(
            ADFLIB_PUBLIC_HEADERS
                .iter()
                .map(|header| format!("{install_prefix}/include/{header}")),
        );
        install_products.push(format!("{install_prefix}/lib/pkgconfig/adflib.pc"));
        return Ok(ConfigureBuildDecl {
            mmake_name: mmake,
            mode: "adflib-target".to_owned(),
            source_dir: "${CMAKE_SOURCE_DIR}/tools/ADFlib".to_owned(),
            binary_dir: binary_dir.clone(),
            install_prefix,
            input_manifest: "${CMAKE_SOURCE_DIR}/tools/ADFlib/adflib-configure.inputs".to_owned(),
            private_products: vec![format!("{binary_dir}/build/libadf.a")],
            install_products,
            dependency_targets: Vec::new(),
            provided_library: Some("adf".to_owned()),
            provider_target: Some("linklib-adflib-configure-adf".to_owned()),
            dir_path: relative_dir.to_path_buf(),
        });
    }

    if relative_dir == Path::new(WIRELESS_CONFIGURE_DIR)
        && mmake == "workbench-network-wirelessmanager"
    {
        require_exact_macro_arguments(
            invocation,
            &[
                ("mmake", "workbench-network-wirelessmanager"),
                ("install_env", "BINDIR=$(AROS_C)"),
                ("use_build_env", "yes"),
            ],
        )?;
        validate_configure_input_manifest(
            root,
            WIRELESS_CONFIGURE_SOURCE_ROOT,
            WIRELESS_CONFIGURE_MANIFEST,
        )?;
        let binary_dir =
            "${AROS_BUILD_DIR}/gen/configure/workbench/network/WirelessManager".to_owned();
        let private_root = format!("{binary_dir}/source/wpa_supplicant");
        return Ok(ConfigureBuildDecl {
            mmake_name: mmake,
            mode: "wirelessmanager".to_owned(),
            source_dir:
                "${CMAKE_SOURCE_DIR}/workbench/network/WirelessManager".to_owned(),
            binary_dir,
            install_prefix: "${AROS_BUILD_DIR}/SYS".to_owned(),
            input_manifest: "${CMAKE_SOURCE_DIR}/workbench/network/WirelessManager/wirelessmanager-configure.inputs".to_owned(),
            private_products: ["wpa_supplicant", "wpa_passphrase", "wpa_cli"]
                .into_iter()
                .map(|product| format!("{private_root}/{product}"))
                .collect(),
            install_products: vec!["${AROS_BUILD_DIR}/SYS/C/WirelessManager".to_owned()],
            dependency_targets: vec!["linklibs-mui".to_owned()],
            provided_library: None,
            provider_target: None,
            dir_path: relative_dir.to_path_buf(),
        });
    }

    Err("unsupported configure-style capability (modelled: tools/ADFlib mmake=host-adflib,linklib-adflib; workbench/network/WirelessManager/wpa_supplicant mmake=workbench-network-wirelessmanager)".to_owned())
}
