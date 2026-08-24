//! Capabilities: declarations the transpiler models exactly.
//!
//! Most of the tree is read generically -- a `%build_module` is a module, a
//! `%build_linklib` an archive. A handful of declarations cannot be: they hand
//! a legacy macro an open-ended configure environment, a Python driver or a
//! source manifest, and reproducing them means reproducing decisions that live
//! in the mmakefile rather than in the macro. Each of those is modelled here,
//! and each is gated on a digest of the inputs that were audited, so a changed
//! input means the capability is not recognised and the declaration lands in
//! the unmodelled report rather than being guessed at. The digests are data:
//! `pinned-digests.pins`, read through `crate::pins`.
//!
//! One module per capability family. They were all in `parser.rs`, which is
//! what the decomposition is taking apart; this module holds what more than one
//! family needs.

pub mod ahi;
pub mod configure;
pub mod grub2;
pub mod mesa;
pub mod nouveau;

use crate::parser::{
    collect_vars, join_continuations, macro_arg, macro_argument_names, Invocation,
};
use aros_common::read_source;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub(crate) fn file_has_sha256(root: &Path, relative: &str, expected: &str) -> bool {
    fs::read(root.join(relative))
        .ok()
        .is_some_and(|bytes| format!("{:x}", Sha256::digest(bytes)) == expected)
}

pub(crate) fn require_exact_macro_arguments(
    invocation: &Invocation,
    expected: &[(&str, &str)],
) -> std::result::Result<(), String> {
    let names = macro_argument_names(&invocation.args);
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    if unique.len() != names.len() {
        return Err("duplicate macro argument".to_owned());
    }
    let mut expected_names = expected
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect::<Vec<_>>();
    expected_names.sort();
    if unique != expected_names {
        return Err(format!(
            "argument set [{}] does not match audited capability [{}]",
            unique.join(", "),
            expected_names.join(", ")
        ));
    }
    for (name, expected_value) in expected {
        // MetaMake accepts a literal empty `key=` argument. `macro_arg`
        // intentionally returns None for it, because ordinary consumers use
        // absence and emptiness equivalently. Closed capabilities sometimes
        // need to distinguish it, however: the name-set check above proves
        // the key exists, and this branch proves it has no value.
        if expected_value.is_empty() {
            if let Some(actual) = macro_arg(&invocation.args, name) {
                return Err(format!(
                    "{name} uses `{actual}`, expected an audited empty argument"
                ));
            }
            continue;
        }
        let actual = macro_arg(&invocation.args, name)
            .ok_or_else(|| format!("missing required {name}= argument"))?;
        if actual != *expected_value {
            return Err(format!(
                "{name} uses `{actual}`, expected audited form `{expected_value}`"
            ));
        }
    }
    Ok(())
}

/// One inventory variable of a Make manifest, as a safe list of file names.
///
/// Named for Mesa while it sat in parser.rs, but four of its seven call sites
/// are Nouveau: a capability whose sources are a manifest reads them with this
/// rather than globbing, and the checks are the same wherever it is used --
/// non-empty, no unexpanded variable, no absolute path, no `..`.
/// Exact, version-pinned source lanes for the remaining Mesa 20.0.8 private
/// archives.  The adjacent manifests contain only literal upstream-relative
/// inventories; generated products are kept in separate variables so they can
/// acquire real build owners before CMake resolves the source lanes.
pub(crate) fn manifest_inventory(
    root: &Path,
    relative: &str,
    variable: &str,
) -> std::result::Result<Vec<String>, String> {
    let path = root.join(relative);
    let content =
        read_source(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let joined = join_continuations(&content);
    let scope = collect_vars(&joined);
    let values = scope
        .snapshot(usize::MAX)
        .remove(variable)
        .ok_or_else(|| format!("{relative} does not define {variable}"))?;
    if values.is_empty()
        || values.iter().any(|value| {
            value.contains("$(")
                || value.contains("${")
                || value.starts_with('/')
                || value.split('/').any(|part| part == "..")
        })
    {
        return Err(format!(
            "{relative} contains an empty or unsafe {variable} inventory"
        ));
    }
    Ok(values)
}
