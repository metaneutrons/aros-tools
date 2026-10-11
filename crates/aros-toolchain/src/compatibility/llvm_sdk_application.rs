//! Source-fact-driven LLVM commands and narrow ordinary-application ELF checks.
//!
//! These helpers preserve LLVM's normal link defaults. They validate only the
//! source-selected target facts and one required common collector symbol; they
//! do not authenticate the source, prove every scalar ABI fact, demonstrate
//! C++ initialization, or qualify SDK usability.

use std::collections::BTreeSet;
use std::path::{Component, Path};

use aros_common::elf::{self, Class, Home, AROS_ABI_VERSION, OS_ABI_AROS};
use aros_common::native_consumer_contract::NativeSdkLinkProbe;
use aros_common::sdk_manifest::{SdkAbiValue, SdkTarget};

use super::CompatibilityCommand;
use crate::ContractError;

const MAX_PROBE_SOURCE_BYTES: usize = 1_024;
const MAX_ADDITIONAL_LIBRARIES: usize = 64;
const MAX_LIBRARY_BYTES: usize = 64;
const MAX_APPLICATION_ARGUMENTS: usize = 256;
const REQUIRED_COLLECTOR_SYMBOL: &str = "__TOOLCHAIN_LIST__";

/// Build one ordinary C or C++ LLVM SDK application-link command.
///
/// Driver defaults remain enabled. The complete source-owned target is
/// validated without CPU/board allowlists; only an explicitly declared
/// `float_abi` becomes a driver flag. The caller remains responsible for
/// authenticating the selected source and SDK and for proving the full ABI.
///
/// # Errors
/// Rejects invalid target facts, selectors, paths, probe source or libraries.
pub(super) fn application_command(
    program: &Path,
    fixture: &Path,
    probe: &NativeSdkLinkProbe,
    target: &SdkTarget,
    sdk: &Path,
    output: &Path,
    selection: (&str, &str),
) -> Result<CompatibilityCommand, ContractError> {
    let (language, label) = selection;
    let source_suffix = match language {
        "c" => "c",
        "cxx" => "cpp",
        _ => return Err(error("LLVM SDK application language must be c or cxx")),
    };
    if !matches!(label, "original" | "relocated") {
        return Err(error(
            "LLVM SDK application label must be original or relocated",
        ));
    }

    let facts = target_facts(target)?;
    portable_absolute_path(program, "driver")?;
    let fixture_text = portable_absolute_path(fixture, "fixture")?;
    let sdk_text = portable_absolute_path(sdk, "sysroot")?;
    portable_absolute_path(output, "output")?;
    validate_probe_source(&probe.source, source_suffix)?;
    let sealed_basename = probe.source.rsplit('/').next().unwrap_or_default();
    if fixture.file_name().and_then(|name| name.to_str()) != Some(sealed_basename) {
        return Err(error(
            "LLVM SDK fixture basename differs from the sealed probe source",
        ));
    }
    validate_libraries(&probe.libraries)?;

    let name = format!("{label}-{language}");
    let map_path = output.join(format!("{name}.map"));
    let elf_path = output.join(format!("{name}.elf"));
    let map_text = portable_absolute_path(&map_path, "link map")?;
    let elf_text = portable_absolute_path(&elf_path, "ELF output")?;

    let mut arguments = vec![
        format!("--target={}", target.target_triple),
        format!("--sysroot={sdk_text}"),
        "-O2".to_owned(),
        "-fno-common".to_owned(),
        "-g0".to_owned(),
    ];
    if let Some(float_abi) = facts.float_abi {
        arguments.push(format!("-mfloat-abi={float_abi}"));
    }
    if language == "cxx" {
        arguments.push("-std=c++17".to_owned());
    }
    arguments.extend([fixture_text, format!("-Wl,-Map={map_text},--cref,--trace")]);
    arguments.extend(probe.libraries.iter().map(|library| format!("-l{library}")));
    arguments.extend(["-o".to_owned(), elf_text]);
    if arguments.len() > MAX_APPLICATION_ARGUMENTS {
        return Err(error("LLVM SDK application command exceeds 256 arguments"));
    }

    Ok(CompatibilityCommand {
        program: program.to_owned(),
        arguments,
    })
}

/// Validate the generic AROS target descriptor and required ELF ABI facts.
///
/// No CPU or board allowlist is applied. A valid descriptor is source data,
/// not evidence that those scalar facts are true of a produced binary.
///
/// # Errors
/// Rejects malformed target triples, missing/wrongly typed facts and invalid
/// ranges or mask relationships.
pub(super) fn validate_target(target: &SdkTarget) -> Result<(), ContractError> {
    target_facts(target).map(|_| ())
}

/// Check the measured ordinary-link ELF identity against source facts.
///
/// This requires only the common defined collector list symbol. It does not
/// infer a C++ initialization proof from that symbol or validate ABI facts
/// beyond the selected class, machine, flags mask and AROS marking.
///
/// # Errors
/// Rejects malformed ELF or disagreement with the source-selected facts.
pub(super) fn verify_application_elf(
    bytes: &[u8],
    target: &SdkTarget,
) -> Result<(), ContractError> {
    let facts = target_facts(target)?;
    let object = elf::read(bytes).map_err(|_| {
        error("LLVM SDK application output is not a supported little-endian ELF object")
    })?;
    if object.kind != 1 {
        return Err(error("LLVM SDK application output is not an ET_REL object"));
    }
    if object.class != facts.class || object.machine != facts.machine {
        return Err(error(
            "LLVM SDK application ELF class or machine differs from the source target",
        ));
    }
    if object.os_abi != OS_ABI_AROS || object.abi_version != AROS_ABI_VERSION {
        return Err(error(
            "LLVM SDK application ELF lacks the selected AROS ABI marking",
        ));
    }
    if object.flags & facts.elf_flags_mask != facts.elf_flags_value {
        return Err(error(
            "LLVM SDK application ELF flags differ from the source mask/value",
        ));
    }
    if !object.symbols.iter().any(|symbol| {
        symbol.name == REQUIRED_COLLECTOR_SYMBOL
            && match symbol.home {
                Home::Absolute => true,
                Home::Undefined => false,
                Home::Section(index) => {
                    index != 0 && object.sections.iter().any(|section| section.index == index)
                }
            }
    }) {
        return Err(error(
            "LLVM SDK application ELF lacks its required defined collector symbol",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct TargetFacts {
    class: Class,
    machine: u16,
    elf_flags_mask: u32,
    elf_flags_value: u32,
    float_abi: Option<&'static str>,
}

fn target_facts(target: &SdkTarget) -> Result<TargetFacts, ContractError> {
    target
        .validate()
        .map_err(|_| error("LLVM SDK source target descriptor is invalid"))?;
    validate_aros_triple(&target.target_triple)?;

    let pointer_width = unsigned_fact(target, "pointer_width")?;
    let class = match pointer_width {
        32 => Class::Elf32,
        64 => Class::Elf64,
        _ => return Err(error("SDK target pointer_width must be 32 or 64")),
    };

    let elf_machine = unsigned_fact(target, "elf_machine")?;
    if !(1..=u64::from(u16::MAX)).contains(&elf_machine) {
        return Err(error("SDK target elf_machine must be in 1..=65535"));
    }
    let elf_flags_mask = u32_fact(target, "elf_flags_mask")?;
    let elf_flags_value = u32_fact(target, "elf_flags_value")?;
    if elf_flags_value & !elf_flags_mask != 0 {
        return Err(error(
            "SDK target elf_flags_value contains bits outside elf_flags_mask",
        ));
    }

    let float_abi = match target.abi.get("float_abi") {
        None => None,
        Some(SdkAbiValue::Text(value)) if value == "hard" => Some("hard"),
        Some(SdkAbiValue::Text(value)) if value == "soft" => Some("soft"),
        Some(SdkAbiValue::Text(value)) if value == "softfp" => Some("softfp"),
        Some(_) => {
            return Err(error(
                "SDK target float_abi must be text hard, soft, or softfp",
            ));
        }
    };

    Ok(TargetFacts {
        class,
        machine: u16::try_from(elf_machine)
            .map_err(|_| error("SDK target elf_machine must be in 1..=65535"))?,
        elf_flags_mask,
        elf_flags_value,
        float_abi,
    })
}

fn unsigned_fact(target: &SdkTarget, name: &str) -> Result<u64, ContractError> {
    match target.abi.get(name) {
        None => Err(error(format!(
            "SDK target is missing required unsigned ABI fact {name}"
        ))),
        Some(SdkAbiValue::Unsigned(value)) => Ok(*value),
        Some(_) => Err(error(format!(
            "SDK target ABI fact {name} must be an unsigned integer"
        ))),
    }
}

fn u32_fact(target: &SdkTarget, name: &str) -> Result<u32, ContractError> {
    let value = unsigned_fact(target, name)?;
    u32::try_from(value).map_err(|_| {
        error(format!(
            "SDK target ABI fact {name} must not exceed 4294967295"
        ))
    })
}

fn validate_aros_triple(triple: &str) -> Result<(), ContractError> {
    let components = triple.split('-').collect::<Vec<_>>();
    if components.len() < 2
        || components.iter().any(|component| component.is_empty())
        || components.last() != Some(&"aros")
    {
        return Err(error(
            "SDK target triple must have nonempty components and end in an exact -aros component",
        ));
    }
    Ok(())
}

fn validate_probe_source(source: &str, expected_suffix: &str) -> Result<(), ContractError> {
    if source.is_empty()
        || source.len() > MAX_PROBE_SOURCE_BYTES
        || source.starts_with('/')
        || source.contains(['\\', ':'])
        || source.split('/').any(|component| {
            component.is_empty()
                || matches!(component, "." | "..")
                || !component.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+')
                })
        })
    {
        return Err(error(
            "LLVM SDK probe source must be a bounded portable relative path",
        ));
    }
    if Path::new(source)
        .extension()
        .and_then(|extension| extension.to_str())
        != Some(expected_suffix)
    {
        return Err(error(
            "LLVM SDK probe source suffix does not match its selected language",
        ));
    }
    Ok(())
}

fn validate_libraries(libraries: &[String]) -> Result<(), ContractError> {
    if libraries.len() > MAX_ADDITIONAL_LIBRARIES {
        return Err(error("LLVM SDK probe libraries exceed the 64-item bound"));
    }
    let mut unique = BTreeSet::new();
    for library in libraries {
        if library.is_empty()
            || library.len() > MAX_LIBRARY_BYTES
            || !library
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-'))
        {
            return Err(error(
                "LLVM SDK libraries must be bounded bare identifiers using ASCII letters, digits, _, + or -",
            ));
        }
        if !unique.insert(library.as_str()) {
            return Err(error("LLVM SDK probe library names must be unique"));
        }
    }
    Ok(())
}

fn portable_absolute_path(path: &Path, label: &str) -> Result<String, ContractError> {
    if !path.is_absolute() {
        return Err(error(format!("LLVM SDK {label} path must be absolute")));
    }
    let value = path
        .to_str()
        .ok_or_else(|| error(format!("LLVM SDK {label} path is not UTF-8")))?;
    if value.contains([',', '\\', ':'])
        || value.chars().any(char::is_control)
        || value.contains("//")
        || value
            .split('/')
            .skip(1)
            .any(|component| matches!(component, "." | ".."))
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(error(format!(
            "LLVM SDK {label} path is not portable for a driver argument"
        )));
    }
    Ok(value.to_owned())
}

fn error(message: impl Into<String>) -> ContractError {
    ContractError::compatibility(message)
}

#[cfg(test)]
#[path = "llvm_sdk_application_tests.rs"]
mod tests;
