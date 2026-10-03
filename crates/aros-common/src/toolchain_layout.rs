//! Data-declared paths to the executable roles in an installed GNU toolchain.
//!
//! This contract is separate from LLVM's historical executable layout. It
//! records relative paths in the extracted payload and resolves them only
//! against that payload root; it never searches `PATH` or invokes a tool.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::toolchain_manifest::ArosCompilerIdentity;
use crate::{sha256_bytes, Sha256Digest};

/// File embedded at the root of a GNU toolchain payload tree.
pub const TOOLCHAIN_TOOLS_FILE: &str = "toolchain-tools.json";

const MAX_TOOLCHAIN_TOOLS_BYTES: u64 = 16 * 1024;
const MAX_TOOL_PATH_LENGTH: usize = 1024;
const MAX_TOOL_PATH_SEGMENTS: usize = 32;

/// Closed set of executable roles required by GNU toolchains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRoles {
    c: String,
    cxx: String,
    assembler: String,
    linker: String,
    archive: String,
    ranlib: String,
    strip: String,
    collector: String,
    nm: Option<String>,
    objcopy: Option<String>,
}

impl ToolRoles {
    /// Return every declared role and its payload-relative executable path.
    pub fn entries(&self) -> impl Iterator<Item = (&'static str, &str)> + '_ {
        [
            ("c", self.c.as_str()),
            ("cxx", self.cxx.as_str()),
            ("assembler", self.assembler.as_str()),
            ("linker", self.linker.as_str()),
            ("archive", self.archive.as_str()),
            ("ranlib", self.ranlib.as_str()),
            ("strip", self.strip.as_str()),
            ("collector", self.collector.as_str()),
        ]
        .into_iter()
        .chain(self.nm.as_deref().map(|path| ("nm", path)))
        .chain(self.objcopy.as_deref().map(|path| ("objcopy", path)))
    }
}

/// Declared executable layout bound to one source-pinned GNU compiler and
/// exact target triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainToolLayout {
    schema_version: ToolchainToolsSchemaVersion,
    compiler: ArosCompilerIdentity,
    target_triple: String,
    tools: ToolRoles,
    sha256: Sha256Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolchainToolsSchemaVersion {
    V1,
    V2,
}

#[derive(Deserialize)]
#[serde(tag = "schema", deny_unknown_fields)]
enum ToolchainToolLayoutRecord {
    #[serde(rename = "aros-toolchain-tools-v1")]
    V1 {
        compiler: ArosCompilerIdentity,
        target_triple: String,
        tools: ToolRolesV1Record,
    },
    #[serde(rename = "aros-toolchain-tools-v2")]
    V2 {
        compiler: ArosCompilerIdentity,
        target_triple: String,
        tools: ToolRolesV2Record,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolRolesV1Record {
    c: String,
    cxx: String,
    assembler: String,
    linker: String,
    archive: String,
    ranlib: String,
    strip: String,
    collector: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolRolesV2Record {
    c: String,
    cxx: String,
    assembler: String,
    linker: String,
    archive: String,
    ranlib: String,
    strip: String,
    collector: String,
    nm: String,
    objcopy: String,
}

impl ToolchainToolLayout {
    /// Parse a closed, bounded GNU executable-layout document.
    ///
    /// # Errors
    /// Rejects oversized JSON, unknown or duplicate fields, unsupported
    /// schemas, LLVM identities, invalid target bindings, and non-portable
    /// executable paths.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() as u64 > MAX_TOOLCHAIN_TOOLS_BYTES {
            return Err("toolchain tools contract exceeds 16 KiB".into());
        }
        let record: ToolchainToolLayoutRecord = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid toolchain tools contract: {error}"))?;
        let (schema_version, compiler, target_triple, tools) = match record {
            ToolchainToolLayoutRecord::V1 {
                compiler,
                target_triple,
                tools,
            } => (
                ToolchainToolsSchemaVersion::V1,
                compiler,
                target_triple,
                ToolRoles {
                    c: tools.c,
                    cxx: tools.cxx,
                    assembler: tools.assembler,
                    linker: tools.linker,
                    archive: tools.archive,
                    ranlib: tools.ranlib,
                    strip: tools.strip,
                    collector: tools.collector,
                    nm: None,
                    objcopy: None,
                },
            ),
            ToolchainToolLayoutRecord::V2 {
                compiler,
                target_triple,
                tools,
            } => (
                ToolchainToolsSchemaVersion::V2,
                compiler,
                target_triple,
                ToolRoles {
                    c: tools.c,
                    cxx: tools.cxx,
                    assembler: tools.assembler,
                    linker: tools.linker,
                    archive: tools.archive,
                    ranlib: tools.ranlib,
                    strip: tools.strip,
                    collector: tools.collector,
                    nm: Some(tools.nm),
                    objcopy: Some(tools.objcopy),
                },
            ),
        };
        if !matches!(compiler, ArosCompilerIdentity::Gnu { .. }) {
            return Err("toolchain tools contract requires a GNU compiler identity".into());
        }
        compiler
            .validate_for_target(&target_triple)
            .map_err(|error| format!("invalid GNU compiler target binding: {error}"))?;

        for (role, path) in tools.entries() {
            validate_tool_path(path)
                .map_err(|error| format!("invalid {role} executable path: {error}"))?;
        }

        Ok(Self {
            schema_version,
            compiler,
            target_triple,
            tools,
            sha256: sha256_bytes(bytes),
        })
    }

    /// Load and parse the companion contract from a payload root without
    /// following a symlink in the root path or contract pathname.
    ///
    /// # Errors
    /// Returns an error if the root is not a real directory, the contract is
    /// absent, non-regular, linked, unreadable, oversized, or invalid.
    pub fn load(root: &Path) -> Result<Self, String> {
        let root = validate_payload_root(root)?;
        let contract_path = root.join(TOOLCHAIN_TOOLS_FILE);
        let mut file = crate::open_regular_file_nofollow(&contract_path).map_err(|error| {
            format!(
                "cannot open toolchain tools contract '{}': {error}",
                contract_path.display()
            )
        })?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_TOOLCHAIN_TOOLS_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read toolchain tools contract: {error}"))?;
        if bytes.len() as u64 > MAX_TOOLCHAIN_TOOLS_BYTES {
            return Err("toolchain tools contract exceeds 16 KiB".into());
        }
        Self::parse(&bytes)
    }

    /// Require an exact compiler-identity and target-triple match.
    ///
    /// # Errors
    /// Rejects LLVM identities, mismatched identity/triple values, or a GNU
    /// identity that is invalid for the supplied target triple.
    pub fn validate_binding(
        &self,
        compiler: &ArosCompilerIdentity,
        target_triple: &str,
    ) -> Result<(), String> {
        if !matches!(compiler, ArosCompilerIdentity::Gnu { .. }) {
            return Err("toolchain tools contract can only bind a GNU compiler".into());
        }
        compiler
            .validate_for_target(target_triple)
            .map_err(|error| format!("invalid GNU compiler target binding: {error}"))?;
        if self.compiler != *compiler {
            return Err("toolchain tools compiler identity does not match".into());
        }
        if self.target_triple != target_triple {
            return Err("toolchain tools target triple does not match".into());
        }
        Ok(())
    }

    /// The declared executable roles.
    #[must_use]
    pub const fn tools(&self) -> &ToolRoles {
        &self.tools
    }

    /// Whether the contract declares the v2 `nm` and `objcopy` roles.
    #[must_use]
    pub const fn has_native_utilities(&self) -> bool {
        matches!(self.schema_version, ToolchainToolsSchemaVersion::V2)
    }

    /// SHA-256 digest of the exact parsed contract bytes.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }

    /// Resolve each declared path inside `root`, requiring a regular,
    /// executable file whose canonical target remains below that root.
    /// In-root compiler symlinks are permitted.
    ///
    /// # Errors
    /// Rejects an invalid root, missing path, directory or non-executable
    /// target, and any symlink that resolves outside the payload tree.
    pub fn resolve_tools(&self, root: &Path) -> Result<Vec<(&'static str, PathBuf)>, String> {
        let root = validate_payload_root(root)?;
        let canonical_root = root.canonicalize().map_err(|error| {
            format!(
                "cannot resolve toolchain root '{}': {error}",
                root.display()
            )
        })?;
        let mut resolved = Vec::with_capacity(if self.has_native_utilities() { 10 } else { 8 });
        for (role, relative) in self.tools.entries() {
            let declared_path = root.join(relative);
            let target = declared_path.canonicalize().map_err(|error| {
                format!(
                    "cannot resolve {role} executable '{}': {error}",
                    declared_path.display()
                )
            })?;
            if !target.starts_with(&canonical_root) {
                return Err(format!(
                    "{role} executable resolves outside the toolchain root: '{}'",
                    declared_path.display()
                ));
            }
            let metadata = fs::metadata(&target).map_err(|error| {
                format!(
                    "cannot inspect {role} executable '{}': {error}",
                    target.display()
                )
            })?;
            if !metadata.is_file() {
                return Err(format!(
                    "{role} executable is not a regular file: '{}'",
                    declared_path.display()
                ));
            }
            if !is_executable(&metadata) {
                return Err(format!(
                    "{role} executable has no executable permission: '{}'",
                    declared_path.display()
                ));
            }
            // Preserve the declared invocation path: GCC driver and collector
            // behavior can depend on the basename passed as argv[0].
            resolved.push((role, declared_path));
        }
        Ok(resolved)
    }
}

fn validate_tool_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > MAX_TOOL_PATH_LENGTH {
        return Err("path must contain 1 to 1024 characters".into());
    }
    let segments = path.split('/').collect::<Vec<_>>();
    if segments.len() > MAX_TOOL_PATH_SEGMENTS {
        return Err("path exceeds 32 segments".into());
    }
    for segment in segments {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err("path contains an empty or traversal segment".into());
        }
        if segment.starts_with('-') {
            return Err("path segment must not begin with '-'".into());
        }
        if !segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
        {
            return Err("path contains a non-portable character".into());
        }
    }
    let parsed = Path::new(path);
    if parsed.is_absolute()
        || parsed
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("path must be a canonical relative path".into());
    }
    Ok(())
}

fn validate_payload_root(root: &Path) -> Result<PathBuf, String> {
    let absolute = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot determine current directory: {error}"))?
            .join(root)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => {
                return Err("toolchain root must be an absolute normalized path".into());
            }
        }
    }
    crate::validate_existing_directory_prefix_nofollow(&normalized)
        .map_err(|error| format!("invalid toolchain root '{}': {error}", normalized.display()))?;
    let metadata = fs::symlink_metadata(&normalized).map_err(|error| {
        format!(
            "cannot inspect toolchain root '{}': {error}",
            normalized.display()
        )
    })?;
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "toolchain root is not a real directory: '{}'",
            normalized.display()
        ));
    }
    Ok(normalized)
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn compiler(abi: &str) -> ArosCompilerIdentity {
        let (isa, architecture) = if abi.starts_with("lp64") {
            ("rv64imafdc", "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0")
        } else {
            ("rv32imafdc", "rv32i2p1_m2p0_a2p1_f2p2_d2p2_c2p0")
        };
        let target = serde_json::from_value(json!({
            "schema": "aros-riscv-target-v1",
            "isa": isa,
            "abi": abi,
            "code_model": "medany",
            "architecture": architecture,
            "unaligned_access": false,
            "atomic_abi": 0,
            "x3_reg_usage": 0
        }))
        .unwrap();
        ArosCompilerIdentity::Gnu {
            gcc_version: "16.2.0".into(),
            binutils_version: "2.47".into(),
            target,
        }
    }

    fn paths(layout: &str) -> Value {
        paths_for(layout, "riscv64")
    }

    fn paths_for(layout: &str, arch: &str) -> Value {
        if layout == "flat" {
            json!({
                "c": format!("{arch}-aros-gcc"),
                "cxx": format!("{arch}-aros-g++"),
                "assembler": format!("{arch}-aros-as"),
                "linker": format!("{arch}-aros-ld"),
                "archive": format!("{arch}-aros-ar"),
                "ranlib": format!("{arch}-aros-ranlib"),
                "strip": format!("{arch}-aros-strip"),
                "collector": format!("{arch}-aros/bin/collect-aros")
            })
        } else {
            json!({
                "c": format!("bin/{arch}-aros-gcc"),
                "cxx": format!("bin/{arch}-aros-g++"),
                "assembler": format!("bin/{arch}-aros-as"),
                "linker": format!("bin/{arch}-aros-ld"),
                "archive": format!("bin/{arch}-aros-ar"),
                "ranlib": format!("bin/{arch}-aros-ranlib"),
                "strip": format!("bin/{arch}-aros-strip"),
                "collector": format!("{arch}-aros/bin/collect-aros")
            })
        }
    }

    fn paths_v2(layout: &str, arch: &str) -> Value {
        let mut tools = paths_for(layout, arch);
        tools["nm"] = json!(format!("{arch}-aros-nm"));
        tools["objcopy"] = json!(format!("{arch}-aros-objcopy"));
        tools
    }

    fn document(compiler: &ArosCompilerIdentity, triple: &str, tools: &Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema": "aros-toolchain-tools-v1",
            "compiler": compiler,
            "target_triple": triple,
            "tools": tools
        }))
        .unwrap()
    }

    fn document_v2(compiler: &ArosCompilerIdentity, triple: &str, tools: &Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema": "aros-toolchain-tools-v2",
            "compiler": compiler,
            "target_triple": triple,
            "tools": tools
        }))
        .unwrap()
    }

    fn parse_valid(layout: &str, abi: &str, triple: &str) -> ToolchainToolLayout {
        let identity = compiler(abi);
        ToolchainToolLayout::parse(&document(&identity, triple, &paths(layout))).unwrap()
    }

    #[test]
    fn parses_rv32_and_rv64_layouts_without_board_specific_fields() {
        let rv32_identity = compiler("ilp32d");
        let rv32 = ToolchainToolLayout::parse(&document(
            &rv32_identity,
            "riscv-unknown-aros",
            &json!({
                "c": "riscv-aros-gcc",
                "cxx": "riscv-aros-g++",
                "assembler": "riscv-aros-as",
                "linker": "riscv-aros-ld",
                "archive": "riscv-aros-ar",
                "ranlib": "riscv-aros-ranlib",
                "strip": "riscv-aros-strip",
                "collector": "riscv-aros/bin/collect-aros"
            }),
        ))
        .unwrap();
        rv32.validate_binding(&rv32_identity, "riscv-unknown-aros")
            .unwrap();
        assert!(!rv32.has_native_utilities());
        assert_eq!(
            rv32.tools().entries().collect::<Vec<_>>(),
            vec![
                ("c", "riscv-aros-gcc"),
                ("cxx", "riscv-aros-g++"),
                ("assembler", "riscv-aros-as"),
                ("linker", "riscv-aros-ld"),
                ("archive", "riscv-aros-ar"),
                ("ranlib", "riscv-aros-ranlib"),
                ("strip", "riscv-aros-strip"),
                ("collector", "riscv-aros/bin/collect-aros"),
            ]
        );
        let rv64 = parse_valid("bin", "lp64d", "riscv64-unknown-aros");
        rv64.validate_binding(&compiler("lp64d"), "riscv64-unknown-aros")
            .unwrap();
        assert!(!rv64.has_native_utilities());
        assert_eq!(rv64.tools().entries().count(), 8);
    }

    #[test]
    fn parses_v2_rv32_and_rv64_layouts_with_native_utilities() {
        for (abi, arch, triple) in [
            ("ilp32d", "riscv32", "riscv-unknown-aros"),
            ("lp64d", "riscv64", "riscv64-unknown-aros"),
        ] {
            let identity = compiler(abi);
            let layout = ToolchainToolLayout::parse(&document_v2(
                &identity,
                triple,
                &paths_v2("flat", arch),
            ))
            .unwrap();
            layout.validate_binding(&identity, triple).unwrap();
            assert!(layout.has_native_utilities());
            let entries = layout
                .tools()
                .entries()
                .map(|(role, path)| (role.to_owned(), path.to_owned()))
                .collect::<Vec<_>>();
            let expected = [
                ("c", format!("{arch}-aros-gcc")),
                ("cxx", format!("{arch}-aros-g++")),
                ("assembler", format!("{arch}-aros-as")),
                ("linker", format!("{arch}-aros-ld")),
                ("archive", format!("{arch}-aros-ar")),
                ("ranlib", format!("{arch}-aros-ranlib")),
                ("strip", format!("{arch}-aros-strip")),
                ("collector", format!("{arch}-aros/bin/collect-aros")),
                ("nm", format!("{arch}-aros-nm")),
                ("objcopy", format!("{arch}-aros-objcopy")),
            ]
            .map(|(role, path)| (role.to_owned(), path));
            assert_eq!(entries, expected);
        }
    }

    #[test]
    fn rejects_unknown_duplicate_and_unsupported_schema_fields() {
        let identity = compiler("lp64d");
        let valid = document(&identity, "riscv64-unknown-aros", &paths("flat"));
        let mut value: Value = serde_json::from_slice(&valid).unwrap();
        value["boards"] = json!(["rpi3", "rpi5"]);
        assert!(ToolchainToolLayout::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        value = serde_json::from_slice(&valid).unwrap();
        value["tools"]["nm"] = json!("riscv64-aros-nm");
        assert!(ToolchainToolLayout::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        value["tools"].as_object_mut().unwrap().remove("nm");
        value["tools"]["objcopy"] = json!("riscv64-aros-objcopy");
        assert!(ToolchainToolLayout::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        value = serde_json::from_slice(&valid).unwrap();
        value["schema"] = json!("aros-toolchain-tools-v3");
        assert!(ToolchainToolLayout::parse(&serde_json::to_vec(&value).unwrap()).is_err());

        let duplicate_schema = String::from_utf8(valid.clone()).unwrap().replacen(
            "\"schema\":\"aros-toolchain-tools-v1\"",
            "\"schema\":\"aros-toolchain-tools-v1\",\"schema\":\"aros-toolchain-tools-v1\"",
            1,
        );
        assert!(ToolchainToolLayout::parse(duplicate_schema.as_bytes()).is_err());
        let duplicate_compiler_version = String::from_utf8(valid.clone()).unwrap().replacen(
            "\"gcc_version\":\"16.2.0\"",
            "\"gcc_version\":\"16.2.0\",\"gcc_version\":\"16.2.0\"",
            1,
        );
        assert!(ToolchainToolLayout::parse(duplicate_compiler_version.as_bytes()).is_err());

        value = serde_json::from_slice(&valid).unwrap();
        value["tools"]["strip"] = Value::Null;
        assert!(ToolchainToolLayout::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        value = serde_json::from_slice(&valid).unwrap();
        value["tools"].as_object_mut().unwrap().remove("strip");
        assert!(ToolchainToolLayout::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn rejects_missing_null_duplicate_unknown_and_unsafe_v2_native_roles() {
        let identity = compiler("lp64d");
        let valid = document_v2(
            &identity,
            "riscv64-unknown-aros",
            &paths_v2("flat", "riscv64"),
        );
        for role in ["nm", "objcopy"] {
            let mut value: Value = serde_json::from_slice(&valid).unwrap();
            value["tools"].as_object_mut().unwrap().remove(role);
            assert!(
                ToolchainToolLayout::parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "accepted missing v2 role {role}"
            );

            value = serde_json::from_slice(&valid).unwrap();
            value["tools"][role] = Value::Null;
            assert!(
                ToolchainToolLayout::parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "accepted null v2 role {role}"
            );
        }

        let valid_text = String::from_utf8(valid.clone()).unwrap();
        for role in ["nm", "objcopy"] {
            let path = format!("riscv64-aros-{role}");
            let duplicate = valid_text.replacen(
                &format!("\"{role}\":\"{path}\""),
                &format!("\"{role}\":\"{path}\",\"{role}\":\"{path}\""),
                1,
            );
            assert!(
                ToolchainToolLayout::parse(duplicate.as_bytes()).is_err(),
                "accepted duplicate v2 role {role}"
            );

            for unsafe_path in ["../tool", "/usr/bin/tool", "bin/-tool"] {
                let mut tool_paths = paths_v2("flat", "riscv64");
                tool_paths[role] = json!(unsafe_path);
                assert!(
                    ToolchainToolLayout::parse(&document_v2(
                        &identity,
                        "riscv64-unknown-aros",
                        &tool_paths,
                    ))
                    .is_err(),
                    "accepted unsafe {role} path {unsafe_path:?}"
                );
            }
        }

        let mut value: Value = serde_json::from_slice(&valid).unwrap();
        value["tools"]["objdump"] = json!("riscv64-aros-objdump");
        assert!(ToolchainToolLayout::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn rejects_malformed_paths_and_oversized_documents() {
        let identity = compiler("lp64d");
        for invalid in [
            "",
            "/usr/bin/gcc",
            "../gcc",
            "bin/../gcc",
            "./gcc",
            "bin//gcc",
            "-gcc",
            "bin/-gcc",
            "bin\\gcc",
            "bin:gcc",
            "bin/é-gcc",
        ] {
            let mut tool_paths = paths("flat");
            tool_paths["c"] = json!(invalid);
            assert!(
                ToolchainToolLayout::parse(&document(
                    &identity,
                    "riscv64-unknown-aros",
                    &tool_paths
                ))
                .is_err(),
                "accepted invalid path {invalid:?}"
            );
        }
        let too_many_segments = std::iter::repeat_n("a", MAX_TOOL_PATH_SEGMENTS + 1)
            .collect::<Vec<_>>()
            .join("/");
        let mut tool_paths = paths("flat");
        tool_paths["c"] = json!(too_many_segments);
        assert!(ToolchainToolLayout::parse(&document(
            &identity,
            "riscv64-unknown-aros",
            &tool_paths
        ))
        .is_err());
        tool_paths = paths("flat");
        tool_paths["c"] = json!(format!("{}gcc", "a".repeat(MAX_TOOL_PATH_LENGTH)));
        assert!(ToolchainToolLayout::parse(&document(
            &identity,
            "riscv64-unknown-aros",
            &tool_paths
        ))
        .is_err());

        assert!(
            ToolchainToolLayout::parse(&vec![b' '; MAX_TOOLCHAIN_TOOLS_BYTES as usize + 1])
                .is_err()
        );
    }

    #[test]
    fn validates_exact_identity_and_target_bindings() {
        let identity = compiler("lp64d");
        let layout = ToolchainToolLayout::parse(&document(
            &identity,
            "riscv64-unknown-aros",
            &paths("flat"),
        ))
        .unwrap();
        layout
            .validate_binding(&identity, "riscv64-unknown-aros")
            .unwrap();
        assert!(layout
            .validate_binding(&identity, "riscv64-vendor-aros")
            .is_err());
        let mut changed_identity = compiler("lp64d");
        if let ArosCompilerIdentity::Gnu { gcc_version, .. } = &mut changed_identity {
            *gcc_version = "16.2.1".into();
        }
        assert!(layout
            .validate_binding(&changed_identity, "riscv64-unknown-aros")
            .is_err());
        assert!(layout
            .validate_binding(&compiler("lp64f"), "riscv64-unknown-aros")
            .is_err());
        assert!(ToolchainToolLayout::parse(&document(
            &identity,
            "riscv-unknown-aros",
            &paths("flat"),
        ))
        .is_err());
        let llvm = ArosCompilerIdentity::Llvm {
            version: "20.1.0".into(),
        };
        assert!(layout
            .validate_binding(&llvm, "riscv64-unknown-aros")
            .is_err());
    }

    #[test]
    fn raw_document_digest_survives_clone_and_distinguishes_substitution_and_whitespace() {
        let identity = compiler("lp64d");
        let bytes = document_v2(
            &identity,
            "riscv64-unknown-aros",
            &paths_v2("flat", "riscv64"),
        );
        let layout = ToolchainToolLayout::parse(&bytes).unwrap();
        let legacy_bytes = document(&identity, "riscv64-unknown-aros", &paths("flat"));
        for raw in [&legacy_bytes, &bytes] {
            let parsed = ToolchainToolLayout::parse(raw).unwrap();
            assert_eq!(parsed.sha256(), &sha256_bytes(raw));
            assert_eq!(parsed.clone().sha256(), parsed.sha256());
            let mut whitespace_variant = b" \n".to_vec();
            whitespace_variant.extend_from_slice(raw);
            let reformatted = ToolchainToolLayout::parse(&whitespace_variant).unwrap();
            assert_ne!(reformatted.sha256(), parsed.sha256());
        }

        let mut substituted_paths = paths_v2("flat", "riscv64");
        substituted_paths["nm"] = json!("riscv64-aros-nm-substituted");
        let substituted = ToolchainToolLayout::parse(&document_v2(
            &identity,
            "riscv64-unknown-aros",
            &substituted_paths,
        ))
        .unwrap();
        assert_ne!(substituted.sha256(), layout.sha256());
    }

    fn create_executable(root: &Path, path: &str) {
        let target = root.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&target, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn create_tools(root: &Path, roles: &ToolRoles) {
        let mut created = Vec::new();
        for (_, path) in roles.entries() {
            if !created.contains(&path) {
                create_executable(root, path);
                created.push(path);
            }
        }
    }

    #[test]
    fn resolves_in_root_compiler_symlinks_and_rejects_escapes_and_nonexecutables() {
        let layout = parse_valid("flat", "lp64d", "riscv64-unknown-aros");
        let root = tempfile::tempdir().unwrap();
        create_tools(root.path(), layout.tools());
        let c = root.path().join("riscv64-aros-gcc");
        fs::remove_file(&c).unwrap();
        let real_compiler = root.path().join("libexec/gcc-driver");
        fs::create_dir_all(real_compiler.parent().unwrap()).unwrap();
        fs::write(&real_compiler, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&real_compiler, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&real_compiler, &c).unwrap();

        let cxx = root.path().join("riscv64-aros-g++");
        fs::remove_file(&cxx).unwrap();
        let real_cxx = root.path().join("libexec/cxx-driver");
        fs::write(&real_cxx, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&real_cxx, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&real_cxx, &cxx).unwrap();

        let collector = root.path().join("riscv64-aros/bin/collect-aros");
        fs::remove_file(&collector).unwrap();
        let real_collector = root.path().join("libexec/collector-driver");
        fs::write(&real_collector, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&real_collector, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&real_collector, &collector).unwrap();

        let resolved = layout.resolve_tools(root.path()).unwrap();
        assert_eq!(resolved.len(), 8);
        assert_eq!(resolved[0].0, "c");
        assert_eq!(resolved[0].1, c);
        assert_eq!(resolved[1].0, "cxx");
        assert_eq!(resolved[1].1, cxx);
        assert_eq!(resolved[7].0, "collector");
        assert_eq!(resolved[7].1, collector);

        let outside = tempfile::tempdir().unwrap();
        let outside_tool = outside.path().join("gcc");
        create_executable(outside.path(), "gcc");
        fs::remove_file(&c).unwrap();
        symlink(&outside_tool, &c).unwrap();
        assert!(layout.resolve_tools(root.path()).is_err());

        fs::remove_file(&c).unwrap();
        create_executable(root.path(), "riscv64-aros-gcc");
        fs::set_permissions(
            root.path().join("riscv64-aros-gcc"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(layout.resolve_tools(root.path()).is_err());
    }

    #[test]
    fn resolves_v2_native_utility_aliases_and_rejects_unsafe_targets() {
        let identity = compiler("lp64d");
        let layout = ToolchainToolLayout::parse(&document_v2(
            &identity,
            "riscv64-unknown-aros",
            &paths_v2("flat", "riscv64"),
        ))
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        create_tools(root.path(), layout.tools());

        let nm = root.path().join("riscv64-aros-nm");
        fs::remove_file(&nm).unwrap();
        let nm_target = root.path().join("libexec/nm-driver");
        create_executable(root.path(), "libexec/nm-driver");
        symlink(&nm_target, &nm).unwrap();

        let objcopy = root.path().join("riscv64-aros-objcopy");
        fs::remove_file(&objcopy).unwrap();
        let objcopy_target = root.path().join("libexec/objcopy-driver");
        create_executable(root.path(), "libexec/objcopy-driver");
        symlink(&objcopy_target, &objcopy).unwrap();

        let resolved = layout.resolve_tools(root.path()).unwrap();
        assert_eq!(resolved.len(), 10);
        assert_eq!(resolved[8], ("nm", nm.clone()));
        assert_eq!(resolved[9], ("objcopy", objcopy.clone()));

        let outside = tempfile::tempdir().unwrap();
        create_executable(outside.path(), "nm-driver");
        fs::remove_file(&nm).unwrap();
        symlink(outside.path().join("nm-driver"), &nm).unwrap();
        assert!(layout.resolve_tools(root.path()).is_err());

        fs::remove_file(&nm).unwrap();
        create_executable(root.path(), "riscv64-aros-nm");
        fs::remove_file(&objcopy).unwrap();
        let non_executable = root.path().join("libexec/objcopy-noexec");
        fs::write(&non_executable, b"not executable").unwrap();
        fs::set_permissions(&non_executable, fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&non_executable, &objcopy).unwrap();
        assert!(layout.resolve_tools(root.path()).is_err());
    }

    #[test]
    fn loads_only_regular_no_follow_contract_files() {
        let layout = parse_valid("flat", "lp64d", "riscv64-unknown-aros");
        let root = tempfile::tempdir().unwrap();
        let contract = document(&compiler("lp64d"), "riscv64-unknown-aros", &paths("flat"));
        let external = tempfile::NamedTempFile::new().unwrap();
        fs::write(external.path(), &contract).unwrap();
        symlink(external.path(), root.path().join(TOOLCHAIN_TOOLS_FILE)).unwrap();
        assert!(ToolchainToolLayout::load(root.path()).is_err());

        fs::remove_file(root.path().join(TOOLCHAIN_TOOLS_FILE)).unwrap();
        fs::write(root.path().join(TOOLCHAIN_TOOLS_FILE), &contract).unwrap();
        let loaded = ToolchainToolLayout::load(root.path()).unwrap();
        assert_eq!(&loaded, &layout);
    }
}
