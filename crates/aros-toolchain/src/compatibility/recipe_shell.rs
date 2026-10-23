//! Bind captured recipe-shell controls to an independently selected host role.
//!
//! The caller's explicit `sh` role is the POSIX command-language trust policy;
//! neither its basename, a digest nor an executable header proves semantics.
//! The literal recorder keeps its single-command grammar. The separate Mesa26
//! preparation entry point requires a Source/frame-bound exception token.
//! Source invocation spelling is preserved; no shell is substituted.

use super::mesa_preflight::invocation::InvocationContext;
use super::{checked_executable, measure_executable, HostToolClosure, HostToolIdentity};
use crate::ContractError;
use aros_common::open_regular_file_nofollow;
use std::{
    collections::BTreeMap,
    io::Read as _,
    path::{Path, PathBuf},
};

// The consuming native-owner phase uses the same pinned invocation type as
// run_prepared_observe. This prepares no new owner, clock or output permission.
pub(super) fn prepare_mesa26_original_recipe_invocation(
    recipe: &aros_transpiler::native_owner_projection::Mesa26OriginalRecipeBinding<'_>,
    selected_shell: &Path,
    environment: &super::CompatibilityEnvironment,
    inherited: &super::environment::ResolvedProbeEnvironment,
    original: &aros_common::make_native_invocation::NativeMakeInvocation,
) -> Result<
    (
        InvocationContext,
        aros_transpiler::make_vars::MakeRecipePrefixes,
    ),
    ContractError,
> {
    recipe
        .revalidate()
        .map_err(|_| refusal("Mesa26 original recipe inputs changed before command preparation"))?;
    let super::CompatibilityEnvironment::SealedHostTools { host_tools, .. } = environment else {
        return Err(refusal(
            "Mesa26 original recipe requires selected host tools",
        ));
    };
    // A live native owner adds deterministic preparation variables (such as
    // LC_ALL) before capture. Retain that held context, not a fresh request-only
    // reconstruction. Tool identities and search path must still be selected.
    let selected = super::environment::resolve(environment)?;
    if inherited.host_tools != selected.host_tools
        || inherited.variables.get("PATH") != selected.variables.get("PATH")
    {
        return Err(refusal("original recipe inherited unselected host tools"));
    }
    let exports = recipe
        .recipe()
        .serial_native_environment(original)
        .map_err(|_| refusal("Mesa26 recipe lacks a bound serial native environment"))?;
    let shell = BoundRecipeShell::bind(
        selected_shell,
        recipe.shell(),
        recipe.flags(),
        host_tools,
        &inherited.variables,
        &exports,
    )?;
    let (prefixes, body) = recipe
        .recipe()
        .original_shell_text()
        .map_err(|_| refusal("Mesa26 original recipe exceeds shell text bounds"))?;
    let invocation = original_invocation(
        &shell,
        recipe.working_directory(),
        body,
        inherited.clone(),
        &exports,
    )?;
    shell.revalidate(
        recipe.shell(),
        recipe.flags(),
        host_tools,
        &inherited.variables,
        &exports,
    )?;
    recipe
        .revalidate()
        .map_err(|_| refusal("Mesa26 original recipe inputs changed during command preparation"))?;
    invocation.revalidate()?;
    Ok((invocation, prefixes))
}

fn original_invocation(
    shell: &BoundRecipeShell,
    directory: &Path,
    body: &str,
    inherited: super::environment::ResolvedProbeEnvironment,
    exports: &BTreeMap<String, String>,
) -> Result<InvocationContext, ContractError> {
    if body.contains('\0') {
        return Err(refusal("original recipe contains a NUL command operand"));
    }
    let inherited = captured_recipe_environment(inherited, exports)?;
    InvocationContext::new(
        &shell.spelling,
        &[shell.flags.clone(), body.to_owned()],
        directory,
        inherited,
    )
}

// Shared by the shell exception and fixed data producers; no shell admission.
pub(super) fn captured_recipe_environment(
    mut inherited: super::environment::ResolvedProbeEnvironment,
    exports: &BTreeMap<String, String>,
) -> Result<super::environment::ResolvedProbeEnvironment, ContractError> {
    validate_environment(&inherited.variables, exports)?;
    if exports.get("PATH") != inherited.variables.get("PATH") {
        return Err(refusal(
            "original recipe exports a path outside selected host tools",
        ));
    }
    // The Source projection includes measured inherited exports and applies
    // explicit unexport directives. Extending the preparation environment
    // would silently resurrect variables the original recipe removed.
    inherited.variables = exports.clone();
    Ok(inherited)
}

pub(super) struct BoundRecipeShell {
    spelling: PathBuf,
    identity: HostToolIdentity,
    flags: String,
}

impl BoundRecipeShell {
    pub(super) fn bind(
        selected_spelling: &Path,
        shell: &str,
        flags: &str,
        host_tools: &HostToolClosure,
        inherited: &BTreeMap<String, String>,
        exports: &BTreeMap<String, String>,
    ) -> Result<Self, ContractError> {
        // Do not split arbitrary shell options or infer an absent default.
        // -ec is Make's POSIX-mode form. Preserve the selected flags exactly:
        // neither the literal recorder nor the Mesa26 bridge adds/removes -e.
        // Both forms retain a command operand.
        if !matches!(flags, "-c" | "-ec") {
            return Err(refusal(
                "recipe shell flags are outside the admitted command forms",
            ));
        }
        validate_environment(inherited, exports)?;
        host_tools.revalidate()?;
        let identity = host_tools
            .tools
            .get("sh")
            .ok_or_else(|| refusal("recipe shell has no independently selected sh role"))?;
        let spelling = Path::new(shell);
        if spelling.as_os_str() != selected_spelling.as_os_str()
            || !spelling.is_absolute()
            || spelling.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            || checked_executable(spelling)? != identity.program
            || measure_executable(&identity.program)? != (identity.sha256.clone(), identity.size)
            || identity.invocation_name.is_some()
        {
            return Err(refusal(
                "Source recipe shell differs from its selected host role",
            ));
        }
        // A shell script adds an unbound interpreter dependency. A native
        // header excludes that case; it does NOT authenticate a shell dialect.
        let mut header = [0; 4];
        open_regular_file_nofollow(&identity.program)
            .and_then(|mut file| file.read_exact(&mut header))
            .map_err(|_| refusal("cannot inspect selected recipe shell image"))?;
        if !native_image(header) {
            return Err(refusal(
                "recipe shell must be a selected native executable, not a script",
            ));
        }
        Ok(Self {
            spelling: spelling.into(),
            identity: identity.clone(),
            flags: flags.into(),
        })
    }

    pub(super) fn revalidate(
        &self,
        shell: &str,
        flags: &str,
        host_tools: &HostToolClosure,
        inherited: &BTreeMap<String, String>,
        exports: &BTreeMap<String, String>,
    ) -> Result<(), ContractError> {
        if Path::new(shell).as_os_str() != self.spelling.as_os_str() || flags != self.flags {
            return Err(refusal("captured recipe shell controls changed"));
        }
        let current = Self::bind(&self.spelling, shell, flags, host_tools, inherited, exports)?;
        if current.identity != self.identity {
            return Err(refusal("selected recipe shell executable changed"));
        }
        Ok(())
    }
}

pub(super) const fn native_image(header: [u8; 4]) -> bool {
    matches!(
        header,
        [0x7f, b'E', b'L', b'F']
            | [0xfe, 0xed, 0xfa, 0xce | 0xcf]
            | [0xce | 0xcf, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe | 0xbf]
            | [0xbe | 0xbf, 0xba, 0xfe, 0xca]
    )
}

fn validate_environment(
    inherited: &BTreeMap<String, String>,
    exports: &BTreeMap<String, String>,
) -> Result<(), ContractError> {
    aros_common::make_observer::validate_posix_shell_environment(inherited, exports).map_err(|_| {
        refusal("recipe shell has unmodeled startup, option, function or loader controls")
    })
}

fn refusal(message: &'static str) -> ContractError {
    ContractError::compatibility(message)
}

#[cfg(test)]
mod tests;
