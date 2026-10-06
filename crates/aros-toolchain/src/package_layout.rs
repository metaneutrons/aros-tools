//! GNU executable-layout checks shared by package production and read-back.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Component, Path};

use aros_common::toolchain_layout::ToolchainToolLayout;
use aros_common::{ArosCompilerIdentity, ArosToolchainManifestEntry};

/// Load, bind, and resolve a candidate GNU layout before packaging mutates any
/// output location.
pub fn validate_root(
    root: &Path,
    compiler: &ArosCompilerIdentity,
    target_triple: &str,
) -> Result<(), String> {
    let layout = ToolchainToolLayout::load(root)?;
    layout.validate_binding(compiler, target_triple)?;
    layout.resolve_tools(root)?;
    Ok(())
}

/// Validate a GNU layout against the exact entries read from a package tar.
///
/// The archive is never extracted for this check. Symlinks are interpreted in
/// the inventory and every resolved ancestor must be a declared directory.
pub fn validate_archive(
    bytes: &[u8],
    compiler: &ArosCompilerIdentity,
    target_triple: &str,
    entries: &[ArosToolchainManifestEntry],
) -> Result<(), String> {
    let layout = ToolchainToolLayout::parse(bytes)?;
    layout.validate_binding(compiler, target_triple)?;
    let inventory = entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    for (role, path) in layout.tools().entries() {
        resolve_inventory_path(&inventory, path)
            .map_err(|error| format!("invalid {role} executable in package inventory: {error}"))?;
    }
    Ok(())
}

#[derive(Debug)]
enum PendingComponent {
    Normal(String),
    Current,
    Parent,
}

fn resolve_inventory_path<'a>(
    inventory: &BTreeMap<&str, &'a ArosToolchainManifestEntry>,
    declared_path: &str,
) -> Result<&'a ArosToolchainManifestEntry, String> {
    let mut pending = declared_path
        .split('/')
        .map(|segment| PendingComponent::Normal(segment.to_owned()))
        .collect::<VecDeque<_>>();
    let mut resolved = Vec::<String>::new();
    let mut symlink_hops = 0_u8;

    while let Some(component) = pending.pop_front() {
        let segment = match component {
            PendingComponent::Normal(segment) => segment,
            PendingComponent::Current => continue,
            PendingComponent::Parent if !resolved.is_empty() => {
                resolved.pop();
                continue;
            }
            PendingComponent::Parent => {
                return Err("symlink resolution escapes the toolchain root".into());
            }
        };
        resolved.push(segment);
        let path = resolved.join("/");
        let entry = inventory
            .get(path.as_str())
            .copied()
            .ok_or_else(|| format!("path '{path}' is absent from the package inventory"))?;
        let terminal = pending.is_empty();

        if entry.kind == "symlink" {
            symlink_hops = symlink_hops.saturating_add(1);
            if symlink_hops > 40 {
                return Err("symlink resolution exceeds 40 links".into());
            }
            let target = entry
                .target
                .as_deref()
                .ok_or_else(|| format!("symlink '{path}' has no target"))?;
            let mut replacement = VecDeque::new();
            for component in Path::new(target).components() {
                match component {
                    Component::Normal(value) => replacement.push_back(
                        value
                            .to_str()
                            .map(|value| PendingComponent::Normal(value.to_owned()))
                            .ok_or_else(|| format!("symlink '{path}' target is not UTF-8"))?,
                    ),
                    Component::CurDir => replacement.push_back(PendingComponent::Current),
                    Component::ParentDir => replacement.push_back(PendingComponent::Parent),
                    Component::RootDir | Component::Prefix(_) => {
                        return Err(format!("symlink '{path}' escapes the toolchain root"));
                    }
                }
            }
            replacement.append(&mut pending);
            pending = replacement;
            resolved.pop();
            continue;
        }

        if terminal {
            if entry.kind != "file" || entry.mode != "0755" {
                return Err(format!(
                    "terminal path '{path}' is not an executable regular file"
                ));
            }
            return Ok(entry);
        }
        if entry.kind != "directory" {
            return Err(format!("ancestor path '{path}' is not a directory"));
        }
    }

    Err("declared executable path resolves to the toolchain root".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn compiler() -> ArosCompilerIdentity {
        serde_json::from_value(json!({
            "family": "gnu",
            "gcc_version": "16.2.0",
            "binutils_version": "2.47",
            "target": {
                "schema": "aros-riscv-target-v1",
                "isa": "rva22u64",
                "abi": "lp64d",
                "code_model": "medany",
                "architecture": "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
                "unaligned_access": false,
                "atomic_abi": 0,
                "x3_reg_usage": 0
            }
        }))
        .unwrap()
    }

    fn contract(ranlib: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema": "aros-toolchain-tools-v1",
            "compiler": compiler(),
            "target_triple": "riscv64-aros",
            "tools": {
                "c": "share/tool",
                "cxx": "share/tool",
                "assembler": "share/tool",
                "linker": "share/tool",
                "archive": "share/tool",
                "ranlib": ranlib,
                "strip": "share/tool",
                "collector": "share/tool"
            }
        }))
        .unwrap()
    }

    fn entry(
        path: &str,
        kind: &str,
        mode: &str,
        target: Option<&str>,
    ) -> ArosToolchainManifestEntry {
        ArosToolchainManifestEntry {
            path: path.into(),
            mode: mode.into(),
            kind: kind.into(),
            sha256: None,
            size: None,
            target: target.map(str::to_owned),
        }
    }

    fn base_entries() -> Vec<ArosToolchainManifestEntry> {
        vec![
            entry("bin", "directory", "0755", None),
            entry("share", "directory", "0755", None),
            entry("share/tool", "file", "0755", None),
        ]
    }

    #[test]
    fn archive_resolution_follows_nested_symlinks_before_parent_components() {
        let mut entries = base_entries();
        entries.extend([
            entry("bin/role", "symlink", "0777", Some("hop/../tool")),
            entry("bin/hop", "symlink", "0777", Some("../share/nested")),
            entry("share/nested", "directory", "0755", None),
        ]);

        validate_archive(&contract("bin/role"), &compiler(), "riscv64-aros", &entries).unwrap();
    }

    #[test]
    fn archive_resolution_rejects_symlink_loops_and_physical_root_escape() {
        let mut loop_entries = base_entries();
        loop_entries.push(entry("bin/role", "symlink", "0777", Some("role")));
        assert!(validate_archive(
            &contract("bin/role"),
            &compiler(),
            "riscv64-aros",
            &loop_entries
        )
        .is_err());

        let mut escape_entries = base_entries();
        escape_entries.extend([
            entry(
                "bin/role",
                "symlink",
                "0777",
                Some("hop/../../outside-tool"),
            ),
            entry("bin/hop", "symlink", "0777", Some("../share")),
            entry("outside-tool", "file", "0755", None),
        ]);
        assert!(validate_archive(
            &contract("bin/role"),
            &compiler(),
            "riscv64-aros",
            &escape_entries
        )
        .is_err());
    }
}
