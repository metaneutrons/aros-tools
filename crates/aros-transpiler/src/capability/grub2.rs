//! The three x86 GRUB host-tool lanes for explicitly audited source versions.
//!
//! The legacy macro carries an open-ended configure environment; the downstream
//! helper owns the source URL, the patch, the cross targets, the host
//! dependencies and the product manifests. What is decided here is whether the
//! declaration is the exact audited input, and which fixed lane it selects.

use super::require_exact_macro_arguments;
use crate::ast::GrubBuildDecl;
use crate::parser::{macro_arg, Invocation, TargetContext};
use aros_common::read_source;
use std::path::Path;

const HOST_DIRECTORY: &str = "arch/all-pc/boot/grub2-host";

/// Parses the three x86 GRUB host-tool lanes without admitting the
/// legacy macro's open-ended configure environment.  The downstream helper
/// owns the source URL, patch, cross targets, host dependencies and complete
/// product manifests; this parser verifies that the legacy declaration is the
/// exact audited input before selecting its fixed lane roots.
pub(crate) fn parse(
    root: &Path,
    invocation: &Invocation,
    relative_dir: &Path,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<GrubBuildDecl>, String> {
    if relative_dir != Path::new(HOST_DIRECTORY) {
        return Ok(None);
    }
    let Some(mmake) = macro_arg(&invocation.args, "mmake") else {
        return Ok(None);
    };
    if !matches!(
        mmake.as_str(),
        "grub2-host" | "grub2-efi-host" | "grub2-efi32-host"
    ) {
        return Ok(None);
    }

    let Some(profile) = target else {
        return Err("GRUB2 host-tool capability requires a concrete target profile".to_owned());
    };
    let profile_key = (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    );
    if profile_key
        != (
            Some("x86_64"),
            Some("pc"),
            Some("llvm"),
            Some("i386"),
            Some("1"),
            Some(""),
        )
    {
        return Err(format!(
            "GRUB2 host-tool capability only supports x86_64-pc LLVM with the i386 companion (cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={})",
            profile.cpu.as_deref().unwrap_or("<unset>"),
            profile.platform.as_deref().unwrap_or("<unset>"),
            profile.toolchain.as_deref().unwrap_or("<unset>"),
            profile.cpu32.as_deref().unwrap_or("<unset>"),
            profile.use_mmu.as_deref().unwrap_or("<unset>"),
            profile.float_abi.as_deref().unwrap_or("<unset>")
        ));
    }
    let version_path = root.join("arch/all-pc/boot/grub2_def");
    let version = read_source(&version_path)
        .map_err(|error| format!("cannot read {}: {error}", version_path.display()))?;
    if !matches!(version.trim(), "2.12" | "2.16") {
        return Err(format!(
            "GRUB2 host-tool capability supports versions 2.12 and 2.16, but arch/all-pc/boot/grub2_def declares {:?}; update the transpiler and its closed GRUB build contract",
            version.trim()
        ));
    }
    let (mode, lane) = match mmake.as_str() {
        "grub2-host" => {
            require_exact_macro_arguments(
                invocation,
                &[
                    ("mmake", "grub2-host"),
                    ("compiler", "host"),
                    ("prefix", "$(DESTDIR)"),
                    ("srcdir", "$(GRUBSRCDIR)"),
                    ("package", "pc"),
                    ("extraoptions", "$(GRUB2_HOST_OPTS) --with-platform=pc"),
                    ("targetisaflags", ""),
                    ("config_env_extra", "$(GRUB2_HOST_ENV)"),
                ],
            )?;
            ("pc", "pc")
        }
        "grub2-efi-host" => {
            require_exact_macro_arguments(
                invocation,
                &[
                    ("mmake", "grub2-efi-host"),
                    ("compiler", "host"),
                    ("prefix", "$(DESTDIR)"),
                    ("srcdir", "$(GRUBSRCDIR)"),
                    ("touch", "no"),
                    ("package", "efi-$(AROS_TARGET_CPU)"),
                    ("extraoptions", "$(GRUB2_HOST_OPTS) --with-platform=efi"),
                    ("targetisaflags", ""),
                    ("config_env_extra", "$(GRUB2_EFI_ENV)"),
                ],
            )?;
            ("efi64", "efi-x86_64")
        }
        "grub2-efi32-host" => {
            require_exact_macro_arguments(
                invocation,
                &[
                    ("mmake", "grub2-efi32-host"),
                    ("compiler", "host"),
                    ("prefix", "$(DESTDIR)"),
                    ("srcdir", "$(GRUBSRCDIR)"),
                    ("touch", "no"),
                    ("package", "efi-$(AROS_TARGET_CPU32)"),
                    ("extraoptions", "$(GRUB2_EFI32_OPTS) --with-platform=efi"),
                    ("targetisaflags", ""),
                    ("config_env_extra", "$(GRUB2_EFI32_ENV)"),
                ],
            )?;
            ("efi32", "efi-i386")
        }
        unsupported => {
            return Err(format!(
                "internal GRUB2 capability invariant failed for identity {unsupported:?}"
            ));
        }
    };

    Ok(Some(GrubBuildDecl {
        mmake_name: mmake,
        version: version.trim().to_owned(),
        mode: mode.to_owned(),
        binary_dir: format!("${{AROS_BUILD_DIR}}/gen/configure/arch/all-pc/boot/grub2-host/{lane}"),
        install_prefix: format!("${{AROS_BUILD_DIR}}/hosttools/grub2/{lane}"),
        dir_path: relative_dir.to_path_buf(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unaudited_grub_version() {
        let source = tempfile::tempdir().unwrap();
        let definition = source.path().join("arch/all-pc/boot/grub2_def");
        std::fs::create_dir_all(definition.parent().unwrap()).unwrap();
        std::fs::write(&definition, "2.17\n").unwrap();
        let invocation = Invocation {
            name: "build_with_configure".to_owned(),
            args: "mmake=grub2-host".to_owned(),
            line: 1,
        };
        let profile = TargetContext {
            cpu: Some("x86_64".to_owned()),
            platform: Some("pc".to_owned()),
            toolchain: Some("llvm".to_owned()),
            cpu32: Some("i386".to_owned()),
            use_mmu: Some("1".to_owned()),
            float_abi: Some(String::new()),
            ..TargetContext::default()
        };
        let error = parse(
            source.path(),
            &invocation,
            Path::new(HOST_DIRECTORY),
            Some(&profile),
        )
        .unwrap_err();
        assert!(error.contains("supports versions 2.12 and 2.16"), "{error}");
    }
}
