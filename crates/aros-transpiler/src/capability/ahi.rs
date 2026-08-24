//! The AHI subsystem's `%build_with_configure` declaration.
//!
//! One declaration, one target directory. The downstream `aros_build_ahi`
//! helper owns the source closure, the products and the tool contract; what is
//! decided here is whether the legacy macro is the exact audited shape and
//! which of the three supported profiles it is being read for.

use super::{file_has_sha256, require_exact_macro_arguments};
use crate::ast::AhiBuildDecl;
use crate::parser::{macro_arg, Invocation, TargetContext};
use crate::pins::pin;
use std::path::Path;

const DIRECTORY: &str = "workbench/devs/AHI";

/// Parses the one current AHI subsystem declaration without turning the
/// legacy `%build_with_configure` macro into a general command runner.
///
/// The AHI helper owns its fixed local source closure, complete products and
/// tool contract.  The transpiler only accepts the exact audited mmakefile
/// and macro shape, selects a supported target profile, and passes the two
/// already-established host-tool variables by name.
pub(crate) fn parse(
    root: &Path,
    invocation: &Invocation,
    relative_dir: &Path,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<AhiBuildDecl>, String> {
    if relative_dir != Path::new(DIRECTORY) {
        return Ok(None);
    }
    let Some(mmake) = macro_arg(&invocation.args, "mmake") else {
        return Ok(None);
    };
    if mmake != "workbench-devs-AHI-subsystem" {
        return Ok(None);
    }

    if !file_has_sha256(
        root,
        "workbench/devs/AHI/mmakefile.src",
        pin("ahi-mmakefile"),
    ) {
        return Err("AHI subsystem mmakefile differs from the audited capability".to_owned());
    }

    let Some(profile) = target else {
        return Err("AHI subsystem capability requires a concrete target profile".to_owned());
    };
    let profile_key = (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    );
    let mode = match profile_key {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some("")) => "x86_64",
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard")) => "arm",
        (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("")) => "aarch64",
        _ => {
            return Err(format!(
                "AHI subsystem capability only supports x86_64-pc, arm-raspi and aarch64-raspi LLVM profiles (cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={})",
                profile.cpu.as_deref().unwrap_or("<unset>"),
                profile.platform.as_deref().unwrap_or("<unset>"),
                profile.toolchain.as_deref().unwrap_or("<unset>"),
                profile.cpu32.as_deref().unwrap_or("<unset>"),
                profile.use_mmu.as_deref().unwrap_or("<unset>"),
                profile.float_abi.as_deref().unwrap_or("<unset>")
            ));
        }
    };

    require_exact_macro_arguments(
        invocation,
        &[
            ("mmake", "workbench-devs-AHI-subsystem"),
            ("prefix", "$(EXEDIR)"),
            ("extraoptions", "$(AHI_OPTIONS)"),
            ("usecppflags", "no"),
            ("gnuflags", "no"),
            (
                "config_env_extra",
                "OBJCOPY=$(OBJCOPY) STRIP=$(STRIP_PLAIN)",
            ),
        ],
    )?;

    Ok(Some(AhiBuildDecl {
        mmake_name: mmake,
        mode: mode.to_owned(),
        binary_dir: format!("${{AROS_BUILD_DIR}}/gen/configure/workbench/devs/AHI/{mode}"),
        install_prefix: "${AROS_BUILD_DIR}/SYS".to_owned(),
        host_sfdc: "${AROS_HOST_SFDC}".to_owned(),
        host_perl: "${AROS_HOST_PERL}".to_owned(),
        dir_path: relative_dir.to_path_buf(),
    }))
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::make_vars::collect_vars_impl;
    use crate::parser::{join_continuations, macro_arg, select_target_invocations, Invocation};
    use crate::testing::{root, target_context, TempTree};
    use aros_common::read_source;
    use std::fs;
    use std::path::Path;

    fn parsed_ahi_capability() -> (Invocation, String) {
        let root = root();
        let relative_dir = Path::new("workbench/devs/AHI");
        let content = read_source(&root.join(relative_dir).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let profile = target_context("x86_64", "pc", "");
        let (_, states) = collect_vars_impl(&joined, Some(&profile));
        let mut skipped = Vec::new();
        let invocation =
            select_target_invocations(&joined, Some(&states), relative_dir, &mut skipped)
                .into_iter()
                .find(|invocation| {
                    invocation.name == "build_with_configure"
                        && macro_arg(&invocation.args, "mmake").as_deref()
                            == Some("workbench-devs-AHI-subsystem")
                })
                .unwrap();
        assert!(skipped.is_empty(), "{skipped:#?}");
        (invocation, content)
    }

    #[test]
    fn ahi_capability_rejects_macro_profile_and_mmakefile_drift() {
        let root = root();
        let relative_dir = Path::new("workbench/devs/AHI");
        let profile = target_context("x86_64", "pc", "");
        let (invocation, content) = parsed_ahi_capability();

        assert!(parse(&root, &invocation, relative_dir, Some(&profile))
            .unwrap()
            .is_some());

        let mut changed = invocation.clone();
        changed.args = changed.args.replace("gnuflags=no", "gnuflags=yes");
        assert!(parse(&root, &changed, relative_dir, Some(&profile))
            .unwrap_err()
            .contains("gnuflags uses"));

        let unsupported_profile = target_context("arm", "raspi", "soft");
        assert!(
            parse(&root, &invocation, relative_dir, Some(&unsupported_profile))
                .unwrap_err()
                .contains("AHI subsystem capability only supports")
        );

        let tree = TempTree::new();
        let drifted = tree.0.join(relative_dir).join("mmakefile.src");
        fs::create_dir_all(drifted.parent().unwrap()).unwrap();
        fs::write(&drifted, format!("{content}\n# audited-input drift\n")).unwrap();
        assert!(parse(&tree.0, &invocation, relative_dir, Some(&profile))
            .unwrap_err()
            .contains("AHI subsystem mmakefile differs"));
    }
}
