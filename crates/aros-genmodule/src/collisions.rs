//! Case-folded public include names that more than one module claims.

use super::{
    public_include_name, publishes_include_files, ConfModule, FileTransaction, Path, PathBuf,
};
use aros_common::PortableOutputName;

/// Modules that export public headers, grouped by the case-folded include name.
///
/// The output namespace is treated as case-insensitive here. The bootstrap can
/// run on case-insensitive APFS, while the project still supports
/// case-sensitive hosts where `Icon` and `icon` are distinct. Withholding every
/// claimant of a shared name is safer than a host-dependent race, unless the
/// conf files themselves name exactly one owner (see [`yielded_public_modules`]).
pub fn public_include_groups(
    modules: &[ConfModule],
) -> std::collections::HashMap<String, Vec<&ConfModule>> {
    let mut groups: std::collections::HashMap<String, Vec<&ConfModule>> =
        std::collections::HashMap::new();
    for module in modules {
        if publishes_include_files(module) {
            groups
                .entry(public_include_name(module).to_ascii_lowercase())
                .or_default()
                .push(module);
        }
    }
    groups
}

/// A `modsuffix=service` module is Envoy's "xxx.service" stub: Envoy loads it
/// by path and name, and applications never open it through `proto/`. It is
/// therefore only a nominal claimant of a shared include name. When it collides
/// with exactly one module that is not such a stub, that module is the single
/// owner of the spelling and the stubs yield. Every other group (no owner, or
/// two or more owners) stays ambiguous and is withheld as before. The rule
/// applies only inside a collision: a service module without a competitor keeps
/// publishing its headers.
pub fn is_service_stub(module: &ConfModule) -> bool {
    module.mod_suffix == "service"
}

pub fn group_owner<'a>(members: &[&'a ConfModule]) -> Option<&'a ConfModule> {
    let mut owners = members
        .iter()
        .copied()
        .filter(|module| !is_service_stub(module));
    match (owners.next(), owners.next()) {
        (Some(owner), None) => Some(owner),
        _ => None,
    }
}

pub fn colliding_public_include_names(modules: &[ConfModule]) -> std::collections::HashSet<String> {
    public_include_groups(modules)
        .into_iter()
        .filter(|(_, members)| members.len() > 1 && group_owner(members).is_none())
        .map(|(name, _)| name)
        .collect()
}

/// Modules that give up the public headers of a resolved case collision, keyed
/// by module directory and name. They publish nothing, and nothing is staged
/// for removal for them: their spelling may differ from the owner's only in
/// case, and removing it would be a second mutation of the owner's output
/// path.
pub fn yielded_public_modules(
    modules: &[ConfModule],
) -> std::collections::HashSet<(PathBuf, String)> {
    let mut yielded = std::collections::HashSet::new();
    for members in public_include_groups(modules).values() {
        if members.len() < 2 {
            continue;
        }
        if let Some(owner) = group_owner(members) {
            for member in members {
                if !std::ptr::eq(*member, owner) {
                    yielded.insert((member.rel_dir.clone(), member.name.clone()));
                }
            }
        }
    }
    yielded
}

/// Gives a service stub that has lost a case-folded name its `basename` as
/// the public include name, so its API stays visible in the SDK.
///
/// Only a yielding stub with an explicit `basename` is renamed, and only when
/// the new name is portable and, case-folded, equals no other exporter's public
/// name and no other replacement. Otherwise the stub stays withheld, as before.
/// Returns the number of renamed stubs.
pub fn rename_yielding_stubs(modules: &mut [ConfModule]) -> usize {
    let yielded = yielded_public_modules(modules);
    if yielded.is_empty() {
        return 0;
    }
    let mut taken: std::collections::HashSet<String> = modules
        .iter()
        .filter(|module| publishes_include_files(module))
        .map(|module| public_include_name(module).to_ascii_lowercase())
        .collect();
    let mut renames = Vec::new();
    for (index, module) in modules.iter().enumerate() {
        if !yielded.contains(&(module.rel_dir.clone(), module.name.clone()))
            || !module.explicit_base_name
            || PortableOutputName::new(&format!("{}.h", module.base_name)).is_err()
        {
            continue;
        }
        let folded = module.base_name.to_ascii_lowercase();
        if taken.insert(folded) {
            renames.push((index, module.base_name.clone()));
        }
    }
    let count = renames.len();
    for (index, name) in renames {
        modules[index].include_name = name;
    }
    count
}

/// Removes bootstrap products whose shared name is ambiguous.
///
/// `BootstrapSDK.cmake` reruns this scanner during every configuration.  A
/// stale header from a previous non-conflicting configuration would otherwise
/// look usable to Ninja even though there is no longer an unambiguous owner.
pub fn remove_colliding_public_headers(
    module: &ConfModule,
    out_inc: &Path,
    transaction: &mut FileTransaction,
) -> std::io::Result<()> {
    let include_name = public_include_name(module);
    for path in [
        out_inc.join("proto").join(format!("{include_name}.h")),
        out_inc
            .join("clib")
            .join(format!("{include_name}_protos.h")),
    ] {
        transaction.stage_remove(&path)?;
    }

    for path in [
        out_inc.join("defines").join(format!("{include_name}.h")),
        out_inc
            .join("defines")
            .join(format!("{include_name}_LVO.h")),
    ] {
        transaction.stage_remove(&path)?;
    }

    Ok(())
}
