//! Closed compiler-driver tool manifest and adjacent executable resolution.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Deserializer};

use super::is_legacy_driver_name;

pub(super) const TOOL_MANIFEST_NAME: &str = "aros-collector-tools.json";
const TOOL_MANIFEST_SCHEMA: &str = "aros-collector-tools-v1";
const TOOL_MANIFEST_LIMIT: usize = 16 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolManifest {
    schema: String,
    family: ToolFamily,
    invocation: String,
    linker: String,
    strip: String,
    #[serde(default, deserialize_with = "deserialize_non_null_optional_string")]
    emulation: Option<String>,
    #[serde(default, deserialize_with = "deserialize_non_null_optional_string")]
    driver_emulation: Option<String>,
}

fn deserialize_non_null_optional_string<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum ToolFamily {
    Llvm,
    Gnu,
}

#[derive(Debug)]
pub(super) struct DriverTools {
    pub(super) linker: PathBuf,
    pub(super) strip: PathBuf,
    pub(super) emulation: Option<String>,
    pub(super) driver_emulation: Option<String>,
}

pub(super) fn resolve_driver_tools(
    bin: &Path,
    invocation_filename: &str,
    invocation_stem: &str,
    invoked_name: &str,
) -> Result<DriverTools> {
    let manifest_path = bin.join(TOOL_MANIFEST_NAME);
    let manifest = read_tool_manifest(&manifest_path, invocation_filename)?;
    let (linker, strip, emulation, driver_emulation) = match manifest {
        Some(manifest) => (
            manifest.linker,
            manifest.strip,
            manifest.emulation,
            manifest.driver_emulation,
        ),
        None
            if is_legacy_driver_name(invoked_name)
                && (is_legacy_driver_name(invocation_stem) || invocation_stem == "aros-collect") =>
        {
            (
                "ld.lld".to_owned(),
                "llvm-strip".to_owned(),
                None,
                None,
            )
        }
        None => bail!(
            "prefixed collector '{invocation_filename}' requires an adjacent {TOOL_MANIFEST_NAME} manifest"
        ),
    };
    let linker = require_sibling(bin, &linker)?;
    let strip = require_sibling(bin, &strip)?;
    Ok(DriverTools {
        linker,
        strip,
        emulation,
        driver_emulation,
    })
}

pub(super) fn read_tool_manifest(
    path: &Path,
    invocation_filename: &str,
) -> Result<Option<ToolManifest>> {
    let Some((_, bytes)) =
        aros_common::measure_regular_file_bounded(path, TOOL_MANIFEST_LIMIT as u64).with_context(
            || {
                format!(
                    "cannot read bounded regular tool manifest {}",
                    path.display()
                )
            },
        )?
    else {
        return Ok(None);
    };
    let manifest: ToolManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("tool manifest {} is invalid JSON", path.display()))?;
    validate_tool_manifest(&manifest, invocation_filename)?;
    Ok(Some(manifest))
}

fn validate_tool_manifest(manifest: &ToolManifest, invocation_filename: &str) -> Result<()> {
    if manifest.schema != TOOL_MANIFEST_SCHEMA {
        bail!(
            "tool manifest schema must be '{TOOL_MANIFEST_SCHEMA}', got '{}'",
            manifest.schema
        );
    }
    if manifest.invocation != invocation_filename {
        bail!(
            "tool manifest invocation '{}' does not match executable filename '{invocation_filename}'",
            manifest.invocation
        );
    }
    validate_tool_basename("linker", &manifest.linker)?;
    validate_tool_basename("strip", &manifest.strip)?;
    if let Some(emulation) = &manifest.emulation {
        validate_emulation(emulation)?;
    }
    if let Some(driver_emulation) = &manifest.driver_emulation {
        validate_emulation(driver_emulation)?;
    }
    match manifest.family {
        ToolFamily::Gnu if manifest.emulation.is_none() => {
            bail!("GNU tool manifests require an emulation value");
        }
        ToolFamily::Llvm if manifest.driver_emulation.is_some() => {
            bail!("LLVM tool manifests must not set driver_emulation");
        }
        _ => {}
    }
    Ok(())
}

fn validate_tool_basename(role: &str, name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || !name.is_ascii()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
    {
        bail!("tool manifest {role} must be a safe single basename, got '{name}'");
    }
    Ok(())
}

pub(super) fn validate_emulation(emulation: &str) -> Result<()> {
    if emulation.is_empty()
        || !emulation
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        bail!("tool manifest emulation must contain only ASCII letters, digits, '_' or '-' and must not be empty");
    }
    Ok(())
}

pub(super) fn require_sibling(bin: &Path, name: &str) -> Result<PathBuf> {
    let path = bin.join(name);
    let canonical_bin = fs::canonicalize(bin).with_context(|| {
        format!(
            "cannot resolve collector executable directory {}",
            bin.display()
        )
    })?;
    let metadata = fs::metadata(&path).with_context(|| {
        format!(
            "required sibling tool {} is unavailable; the released collector never searches PATH or COMPILER_PATH",
            path.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        bail!(
            "required sibling tool {} is not a regular file",
            path.display()
        );
    }
    let resolved = fs::canonicalize(&path)
        .with_context(|| format!("cannot resolve sibling tool {}", path.display()))?;
    if resolved.parent() != Some(canonical_bin.as_path()) {
        bail!(
            "required sibling tool {} resolves outside the collector executable directory",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            bail!("required sibling tool {} is not executable", path.display());
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn missing_sibling_is_reported_without_a_path_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let error = require_sibling(directory.path(), "ld.lld").unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("required sibling tool"));
        assert!(message.contains("never searches PATH or COMPILER_PATH"));
    }

    #[test]
    fn tool_manifests_are_closed_bounded_and_invocation_specific() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(TOOL_MANIFEST_NAME);
        let valid = r#"{"schema":"aros-collector-tools-v1","family":"gnu","invocation":"riscv-aros-collect-aros","linker":"riscv64-aros-ld","strip":"riscv64-aros-strip","emulation":"riscvelf_aros","driver_emulation":"elf32lriscv"}"#;
        fs::write(&path, valid).unwrap();
        assert!(read_tool_manifest(&path, "riscv-aros-collect-aros")
            .unwrap()
            .is_some());

        let invalid_documents = [
            valid.replace('}', ",\"extra\":true}"),
            valid.replace(
                "\"strip\":\"riscv64-aros-strip\"",
                "\"linker\":\"other-ld\",\"strip\":\"riscv64-aros-strip\"",
            ),
            valid.replace("riscvelf_aros", "../ld"),
            valid.replace("riscv-aros-collect-aros", "other-collect-aros"),
            valid.replace("\"family\":\"gnu\"", "\"family\":\"other\""),
            valid.replace(",\"emulation\":\"riscvelf_aros\"", ""),
            valid.replace("aros-collector-tools-v1", "unsupported"),
            valid.replace("riscvelf_aros", "riscv64=elf_aros"),
            valid.replace("\"emulation\":\"riscvelf_aros\"", "\"emulation\":null"),
            valid.replace(
                "\"driver_emulation\":\"elf32lriscv\"",
                "\"driver_emulation\":null",
            ),
            valid.replace(
                "\"driver_emulation\":\"elf32lriscv\"",
                "\"driver_emulation\":\"elf32lriscv\",\"driver_emulation\":\"elf64lriscv\"",
            ),
            valid.replace("\"family\":\"gnu\"", "\"family\":\"llvm\""),
        ];
        for document in invalid_documents {
            fs::write(&path, document).unwrap();
            assert!(read_tool_manifest(&path, "riscv-aros-collect-aros").is_err());
        }

        fs::write(&path, vec![b' '; TOOL_MANIFEST_LIMIT + 1]).unwrap();
        let error = read_tool_manifest(&path, "riscv-aros-collect-aros").unwrap_err();
        assert!(format!("{error:#}").contains("read limit"));
    }

    #[cfg(unix)]
    #[test]
    fn tool_manifest_reader_does_not_follow_a_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = directory.path().join(TOOL_MANIFEST_NAME);
        let external = outside.path().join("manifest.json");
        fs::write(
            &external,
            r#"{"schema":"aros-collector-tools-v1","family":"llvm","invocation":"collect-aros","linker":"ld.lld","strip":"llvm-strip"}"#,
        )
        .unwrap();
        symlink(&external, &path).unwrap();

        let error = read_tool_manifest(&path, "collect-aros").unwrap_err();
        assert!(format!("{error:#}").contains("symbolic"));
    }

    #[cfg(unix)]
    #[test]
    fn sibling_tool_resolution_rejects_non_executables_and_escaping_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let local_tool = directory.path().join("local-ld");
        fs::write(&local_tool, b"tool").unwrap();
        fs::set_permissions(&local_tool, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(require_sibling(directory.path(), "local-ld")
            .unwrap_err()
            .to_string()
            .contains("not executable"));

        let outside_tool = outside.path().join("outside-ld");
        fs::write(&outside_tool, b"tool").unwrap();
        fs::set_permissions(&outside_tool, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&outside_tool, directory.path().join("escaping-ld")).unwrap();
        assert!(require_sibling(directory.path(), "escaping-ld")
            .unwrap_err()
            .to_string()
            .contains("outside the collector executable directory"));
    }
}
