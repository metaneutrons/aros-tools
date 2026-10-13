//! Explicit, data-only secondary target roles in a source-owned SDK contract.
//!
//! A source selector such as `cpu32` does not declare an ABI or a compiler
//! invocation. This contract records those facts without selecting a compiler
//! executable or claiming that a compiler package actually supports them.

use crate::arch::Architecture;
use crate::sdk_manifest::{SdkAbiValue, SdkTarget};
use crate::TargetProfile;
use std::collections::BTreeSet;

/// Maximum number of explicit companion roles in one SDK consumer contract.
///
/// Current profile format has one `transpiler.cpu32` selector, so it can admit
/// one companion role. Keep the vector shape for an explicit future schema.
pub const MAX_SDK_COMPANION_ROLES: usize = 1;
const MAX_COMPILER_OPTIONS: usize = 32;

/// One explicit compiler/ABI role which writes to a separate Developer library.
///
/// `selector_cpu` must equal the selected source profile's `transpiler.cpu32`.
/// `compiler_options` are argv tokens for the already-admitted target compiler,
/// not a command line or compiler selection; exactly one
/// `--target=<target.target_triple>` is required. The role binds no compiler
/// executable or path, and its data alone is not package ABI proof.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkCompanionRole {
    pub selector_cpu: String,
    pub target: SdkTarget,
    pub compiler_options: Vec<String>,
    /// One direct child directory below the SDK's Developer root, e.g. `lib32`.
    pub library_dir: String,
    /// Required linker library name without `-l` or a filesystem path.
    pub required_runtime_library: String,
}

/// Validate a closed list of source-declared companion roles.
///
/// The current profile format is a closed one-role capability: its singular
/// `cpu32` selector admits only one explicitly described matching role. It
/// never synthesizes that role. Unknown CPUs and ABI facts fail closed.
///
/// ABI validation models both endian values from the architecture mapping.
/// The current CMake object adapter supports little-endian companions only and
/// rejects big-endian roles explicitly.
///
/// # Errors
/// Rejects contradictory selectors, colliding output roots or invalid ABI roles.
pub fn validate_companion_roles(
    roles: &[SdkCompanionRole],
    primary: &SdkTarget,
    profile: &TargetProfile,
) -> Result<(), String> {
    if roles.len() > MAX_SDK_COMPANION_ROLES {
        return Err(format!(
            "SDK companion role list exceeds its limit of {MAX_SDK_COMPANION_ROLES}"
        ));
    }
    if roles.is_empty() {
        return Ok(());
    }

    let selectors = profile
        .transpiler
        .as_ref()
        .map(|transpiler| transpiler.cpu32.as_str())
        .ok_or("SDK companion roles require an explicit `transpiler.cpu32` selector")?;
    let mut cpus = BTreeSet::new();
    let mut triples = BTreeSet::new();
    let mut roots = BTreeSet::new();
    for role in roles {
        role.validate()?;
        if selectors.is_empty() || role.selector_cpu.as_str() != selectors {
            return Err(format!(
                "companion selector CPU `{}` differs from the source profile's `transpiler.cpu32` selector",
                role.selector_cpu
            ));
        }
        let role_architecture = architecture_for_source_cpu(&role.selector_cpu)
            .ok_or_else(|| format!("unsupported companion selector CPU `{}`", role.selector_cpu))?;
        if role_architecture == profile.arch {
            return Err("companion selector CPU collides with the primary SDK target".into());
        }
        let triple = role.target.target_triple.as_str();
        if triple.eq_ignore_ascii_case(&primary.target_triple) {
            return Err("companion target triple collides with the primary SDK target".into());
        }
        let root = role.library_dir.to_ascii_lowercase();
        if root == "lib" || root.starts_with("lib/") {
            return Err(
                "companion library directory collides with the primary Developer lib root".into(),
            );
        }
        if !cpus.insert(role.selector_cpu.to_ascii_lowercase()) {
            return Err("SDK companion roles contain duplicate selector CPUs".into());
        }
        if !triples.insert(triple.to_ascii_lowercase()) {
            return Err("SDK companion roles contain duplicate target triples".into());
        }
        if roots.iter().any(|existing: &String| {
            root.as_str() == existing.as_str()
                || root
                    .strip_prefix(existing.as_str())
                    .is_some_and(|suffix| suffix.starts_with('/'))
                || existing
                    .as_str()
                    .strip_prefix(root.as_str())
                    .is_some_and(|suffix| suffix.starts_with('/'))
        }) {
            return Err("SDK companion roles contain overlapping library directories".into());
        }
        roots.insert(root);
    }
    Ok(())
}

impl SdkCompanionRole {
    /// Validate one role's target facts, argv-safe options, and direct Developer child.
    /// # Errors
    /// Rejects unsafe compiler options, output roots and contradictory ABI facts.
    pub fn validate(&self) -> Result<(), String> {
        self.target.validate()?;
        let architecture = architecture_for_source_cpu(&self.selector_cpu)
            .ok_or_else(|| format!("unsupported companion selector CPU `{}`", self.selector_cpu))?;
        if architecture.pointer_width() != 32 {
            return Err("companion selector CPU must identify a 32-bit architecture".into());
        }
        require_fact(
            &self.target,
            "source_cpu",
            &SdkAbiValue::Text(self.selector_cpu.clone()),
        )?;
        require_fact(
            &self.target,
            "pointer_width",
            &SdkAbiValue::Unsigned(architecture.pointer_width() as u64),
        )?;
        require_fact(&self.target, "elf_class", &SdkAbiValue::Unsigned(1))?;
        require_fact(
            &self.target,
            "elf_machine",
            &SdkAbiValue::Unsigned(elf_machine(architecture)),
        )?;
        require_fact(
            &self.target,
            "endian",
            &SdkAbiValue::Text(endian(architecture).into()),
        )?;

        let source_cpu = architecture.source_cpu();
        let architecture_name = architecture.to_string();
        let components = self.target.target_triple.split('-').collect::<Vec<_>>();
        let triple_cpu = components.first().copied().unwrap_or_default();
        if components.len() < 2
            || components.iter().any(|component| component.is_empty())
            || (triple_cpu != source_cpu.as_str() && triple_cpu != architecture_name.as_str())
            || components.last() != Some(&"aros")
        {
            return Err(
                "companion target triple must start with its selector CPU and end in `aros`".into(),
            );
        }

        validate_compiler_options(self)?;
        validate_library_dir(&self.library_dir)?;
        validate_runtime_library(&self.required_runtime_library)?;
        Ok(())
    }
}

fn architecture_for_source_cpu(cpu: &str) -> Option<Architecture> {
    [
        Architecture::X86_64,
        Architecture::AArch64,
        Architecture::Arm,
        Architecture::M68k,
        Architecture::Riscv32,
        Architecture::Riscv64,
        Architecture::I386,
        Architecture::Ppc,
    ]
    .into_iter()
    .find(|architecture| architecture.source_cpu() == cpu)
}

const fn elf_machine(architecture: Architecture) -> u64 {
    match architecture {
        Architecture::I386 => 3,
        Architecture::M68k => 4,
        Architecture::Arm => 40,
        Architecture::Ppc => 20,
        Architecture::X86_64 => 62,
        Architecture::AArch64 => 183,
        Architecture::Riscv32 | Architecture::Riscv64 => 243,
    }
}

const fn endian(architecture: Architecture) -> &'static str {
    match architecture {
        Architecture::I386
        | Architecture::X86_64
        | Architecture::AArch64
        | Architecture::Arm
        | Architecture::Riscv32
        | Architecture::Riscv64 => "little",
        Architecture::M68k | Architecture::Ppc => "big",
    }
}

fn validate_compiler_options(role: &SdkCompanionRole) -> Result<(), String> {
    if role.compiler_options.is_empty() || role.compiler_options.len() > MAX_COMPILER_OPTIONS {
        return Err(format!(
            "companion compiler options require between one and {MAX_COMPILER_OPTIONS} argv tokens"
        ));
    }
    let mut target_count = 0;
    let mut overrides = BTreeSet::new();
    for option in &role.compiler_options {
        if option.is_empty()
            || option.len() > 256
            || !option.starts_with('-')
            || !option
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-.,=+:/".contains(&byte))
        {
            return Err(
                "companion compiler options must be bounded, shell-free argv tokens".into(),
            );
        }
        if matches!(
            option.as_str(),
            "--target" | "-target" | "-o" | "-c" | "-S" | "-E" | "-I" | "-D" | "-U"
        ) {
            return Err(format!(
                "companion compiler option `{option}` requires an out-of-contract separate argument"
            ));
        }
        if let Some(target) = option.strip_prefix("--target=") {
            target_count += 1;
            if target != role.target.target_triple.as_str() {
                return Err(
                    "companion `--target` option contradicts the declared target triple".into(),
                );
            }
            continue;
        }
        if matches!(option.as_str(), "-m32" | "-m64") {
            let expected =
                if role.target.abi.get("pointer_width") == Some(&SdkAbiValue::Unsigned(32)) {
                    "-m32"
                } else {
                    "-m64"
                };
            if option != expected {
                return Err(format!(
                    "companion compiler option `{option}` contradicts pointer_width"
                ));
            }
            unique_override(&mut overrides, "pointer_width")?;
            continue;
        }
        if matches!(
            option.as_str(),
            "-mbig-endian" | "-mlittle-endian" | "-EB" | "-EL"
        ) {
            let expected = if matches!(option.as_str(), "-mbig-endian" | "-EB") {
                "big"
            } else {
                "little"
            };
            unique_override(&mut overrides, "endian")?;
            if role.target.abi.get("endian") != Some(&SdkAbiValue::Text(expected.into())) {
                return Err(format!(
                    "companion compiler option `{option}` contradicts target ABI fact `endian`"
                ));
            }
            continue;
        }
        if is_external_tool_or_search_path_option(option) {
            return Err(format!(
                "companion compiler option `{option}` selects an external tool or search path"
            ));
        }
        validate_fact_override(&role.target, &mut overrides, option)?;
    }
    if target_count != 1 {
        return Err(
            "companion compiler options require exactly one matching `--target=<triple>`".into(),
        );
    }
    Ok(())
}

fn is_external_tool_or_search_path_option(option: &str) -> bool {
    [
        "-B",
        "--gcc-toolchain",
        "-gcc-toolchain",
        "--sysroot",
        "-isysroot",
        "-isystem",
        "-iquote",
        "-idirafter",
        "-include",
        "-imacros",
        "-iframework",
        "-resource-dir",
        "--ld-path",
        "-fuse-ld",
        "-fplugin",
        "-Xclang",
        "-mllvm",
    ]
    .iter()
    .copied()
    .any(|prefix| {
        option == prefix
            || option
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('=') || suffix.starts_with('/'))
    }) || option.starts_with("-B")
        || option.starts_with("-I")
        || option.starts_with("-L")
        || option.starts_with("-F")
}

fn validate_fact_override(
    target: &SdkTarget,
    overrides: &mut BTreeSet<&'static str>,
    option: &str,
) -> Result<(), String> {
    let (family, fact_key, value) = if let Some(value) = option.strip_prefix("-march=") {
        ("compiler_arch", "compiler_arch", value)
    } else if let Some(value) = option.strip_prefix("-mcpu=") {
        ("compiler_cpu", "compiler_cpu", value)
    } else if let Some(value) = option.strip_prefix("-mfpu=") {
        ("compiler_fpu", "compiler_fpu", value)
    } else if let Some(value) = option.strip_prefix("-mabi=") {
        ("compiler_abi", "compiler_abi", value)
    } else if let Some(value) = option.strip_prefix("-mfloat-abi=") {
        ("float_abi", "float_abi", value)
    } else if let Some(value) = option.strip_prefix("-mcmodel=") {
        ("compiler_code_model", "compiler_code_model", value)
    } else if option == "-mred-zone" {
        return validate_bool_override(target, overrides, "compiler_red_zone", true);
    } else if option == "-mno-red-zone" {
        return validate_bool_override(target, overrides, "compiler_red_zone", false);
    } else if matches!(option, "-fpic" | "-fPIC") {
        return validate_bool_override(target, overrides, "compiler_pic", true);
    } else if matches!(option, "-fno-pic" | "-fno-PIC") {
        return validate_bool_override(target, overrides, "compiler_pic", false);
    } else if matches!(option, "-fpie" | "-fPIE") {
        return validate_bool_override(target, overrides, "compiler_pie", true);
    } else if matches!(option, "-fno-pie" | "-fno-PIE") {
        return validate_bool_override(target, overrides, "compiler_pie", false);
    } else {
        return Ok(());
    };
    unique_override(overrides, family)?;
    match target.abi.get(fact_key) {
        Some(SdkAbiValue::Text(expected)) if expected == value => Ok(()),
        Some(_) => Err(format!(
            "companion compiler option `{option}` contradicts target ABI fact `{fact_key}`"
        )),
        None => Err(format!(
            "companion compiler option `{option}` requires explicit target ABI fact `{fact_key}`"
        )),
    }
}

fn validate_bool_override(
    target: &SdkTarget,
    overrides: &mut BTreeSet<&'static str>,
    fact_key: &'static str,
    value: bool,
) -> Result<(), String> {
    unique_override(overrides, fact_key)?;
    match target.abi.get(fact_key) {
        Some(SdkAbiValue::Boolean(expected)) if *expected == value => Ok(()),
        Some(_) => Err(format!(
            "companion compiler option contradicts target ABI fact `{fact_key}`"
        )),
        None => Err(format!(
            "companion compiler option requires explicit target ABI fact `{fact_key}`"
        )),
    }
}

fn unique_override(
    overrides: &mut BTreeSet<&'static str>,
    family: &'static str,
) -> Result<(), String> {
    if overrides.insert(family) {
        Ok(())
    } else {
        Err(format!(
            "companion compiler options repeat or contradict `{family}`"
        ))
    }
}

fn validate_library_dir(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > 256 || path.contains('/') || path.contains('\\') {
        return Err("companion library_dir must be one bounded Developer child directory".into());
    }
    let mut bytes = path.bytes();
    if !matches!(bytes.next(), Some(byte) if byte.is_ascii_alphanumeric())
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
    {
        return Err("companion library_dir contains an unsafe path component".into());
    }
    Ok(())
}

fn validate_runtime_library(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 128
        || name.starts_with('-')
        || name.contains('/')
        || name.contains('\\')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
    {
        return Err(
            "required_runtime_library must be a simple library name without `-l` or a path".into(),
        );
    }
    Ok(())
}

fn require_fact(target: &SdkTarget, key: &str, expected: &SdkAbiValue) -> Result<(), String> {
    match target.abi.get(key) {
        Some(actual) if actual == expected => Ok(()),
        Some(_) => Err(format!(
            "companion target ABI fact `{key}` contradicts its selector architecture"
        )),
        None => Err(format!(
            "companion target requires explicit ABI fact `{key}`"
        )),
    }
}

#[cfg(test)]
#[path = "sdk_companion_role_tests.rs"]
mod tests;
