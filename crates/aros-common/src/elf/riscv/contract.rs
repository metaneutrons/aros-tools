//! Explicit target expectations, separate from measured ELF attributes.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::elf::{Class, AROS_ABI_VERSION, OS_ABI_AROS};

/// A source-bound compiler expectation. The caller must bind its bytes to the
/// recipe; parsing a declaration alone establishes no compiler provenance.
///
/// Untrusted standalone documents must use [`Self::parse`] for the 16 KiB
/// byte cap. Embedded Serde callers must bound their enclosing document before
/// decoding: `Deserialize` validates semantics and selector lengths, but cannot
/// enforce a byte-stream cap before Serde materializes fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetContract {
    schema: String,
    isa: String,
    abi: String,
    code_model: String,
    architecture: String,
    unaligned_access: bool,
    atomic_abi: u64,
    x3_reg_usage: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: String,
    isa: String,
    abi: String,
    code_model: String,
    architecture: String,
    unaligned_access: bool,
    atomic_abi: u64,
    x3_reg_usage: u64,
}

impl<'de> Deserialize<'de> for TargetContract {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let record = Record::deserialize(deserializer)?;
        let contract = Self {
            schema: record.schema,
            isa: record.isa,
            abi: record.abi,
            code_model: record.code_model,
            architecture: record.architecture,
            unaligned_access: record.unaligned_access,
            atomic_abi: record.atomic_abi,
            x3_reg_usage: record.x3_reg_usage,
        };
        contract.validate().map_err(serde::de::Error::custom)?;
        Ok(contract)
    }
}

/// The link stage being verified, not an inference from the output filename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactRole {
    /// Compiler/assembler output or an intermediate relocatable link.
    CompilationUnit,
    /// An AROS application linked as a relocatable ELF with AROS ABI marking.
    ArosRelocatable,
}

impl TargetContract {
    /// Parse a closed, bounded RISC-V compiler target document.
    ///
    /// `isa` and `code_model` select compiler arguments. The `architecture`
    /// field is the exact independently measured canonical ELF attribute for
    /// that selection, not a fuzzy alias or an ISA-superset promise.
    ///
    /// # Errors
    /// Rejects unknown/duplicate fields, unsupported ABI/code model, unsafe
    /// arguments and inconsistent attribute width or floating-point features.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= 16 * 1024,
            "RISC-V target contract exceeds 16 KiB"
        );
        let contract: Self =
            serde_json::from_slice(bytes).context("invalid closed RISC-V target contract")?;
        contract.validate()?;
        Ok(contract)
    }

    /// Exact compiler ISA selector, never inferred from a board name.
    #[must_use]
    pub fn isa(&self) -> &str {
        &self.isa
    }

    /// Exact compiler calling-convention selector.
    #[must_use]
    pub fn abi(&self) -> &str {
        &self.abi
    }

    /// Exact compiler code-model selector. ELF attributes do not verify it;
    /// real source-bound compile/link probes must establish that behavior.
    #[must_use]
    pub fn code_model(&self) -> &str {
        &self.code_model
    }

    /// Canonical architecture attribute required on measured output.
    #[must_use]
    pub fn architecture(&self) -> &str {
        &self.architecture
    }

    fn semantics(&self) -> Result<(Class, u32)> {
        match self.abi.as_str() {
            "ilp32" => Ok((Class::Elf32, 0)),
            "ilp32f" => Ok((Class::Elf32, 2)),
            "ilp32d" => Ok((Class::Elf32, 4)),
            "lp64" => Ok((Class::Elf64, 0)),
            "lp64f" => Ok((Class::Elf64, 2)),
            "lp64d" => Ok((Class::Elf64, 4)),
            _ => anyhow::bail!("unsupported RISC-V calling convention"),
        }
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == "aros-riscv-target-v1",
            "unsupported RISC-V target schema"
        );
        let (class, float_flags) = self.semantics()?;
        for argument in [&self.isa, &self.architecture] {
            ensure!(
                !argument.is_empty()
                    && argument.len() <= 4096
                    && argument.bytes().all(|byte| byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'_'),
                "unsafe RISC-V ISA or architecture selector"
            );
        }
        ensure!(
            matches!(self.code_model.as_str(), "medlow" | "medany"),
            "unsupported RISC-V code model"
        );
        ensure!(
            self.atomic_abi <= 3 && self.x3_reg_usage <= 3,
            "unsupported RISC-V atomic or x3 ABI declaration"
        );
        let width = if class == Class::Elf32 {
            "rv32"
        } else {
            "rv64"
        };
        ensure!(
            self.architecture.starts_with(width),
            "RISC-V architecture width differs from ABI"
        );
        let base = format!("{width}i");
        let base_version = self
            .architecture
            .split('_')
            .next()
            .and_then(|token| token.strip_prefix(&base));
        ensure!(
            base_version.is_some_and(|version| {
                let mut parts = version.split('p');
                let numeric =
                    |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
                parts.next().is_some_and(numeric)
                    && parts.next().is_some_and(numeric)
                    && parts.next().is_none()
            }),
            "selected RISC-V ABI requires a versioned I base, not E or another base ISA"
        );
        ensure!(
            self.isa.starts_with(&base)
                || (class == Class::Elf64
                    && self.isa.starts_with("rva")
                    && self.isa.ends_with("u64")),
            "RISC-V ISA width differs from ABI"
        );
        // A canonical attribute lists each base extension as a versioned
        // underscore-separated token. This is not a complete ISA parser.
        let has = |extension: &str| {
            self.architecture.split('_').any(|token| {
                token
                    .strip_prefix(extension)
                    .is_some_and(|suffix| suffix.as_bytes().first().is_some_and(u8::is_ascii_digit))
            })
        };
        ensure!(
            float_flags != 2 || has("f"),
            "single-float ABI requires measured F extension"
        );
        ensure!(
            float_flags != 4 || (has("f") && has("d")),
            "double-float ABI requires measured F/D extensions"
        );
        Ok(())
    }

    /// Check the actual selected bytes against this complete target contract.
    ///
    /// AROS application links are `REL`, not `EXEC`. This check does not prove
    /// runtime execution, hardware readiness or a complete Developer sysroot.
    ///
    /// # Errors
    /// Rejects wrong machine/class/type, float or embedded ABI flags, unknown
    /// flags, missing/altered architecture attributes and missing AROS marking.
    pub fn verify(&self, bytes: &[u8], role: ArtifactRole) -> Result<()> {
        self.validate()?;
        let object = crate::elf::read(bytes)?;
        let (class, float_flags) = self.semantics()?;
        ensure!(
            object.machine == super::MACHINE,
            "RISC-V target machine mismatch"
        );
        ensure!(object.class == class, "RISC-V target ELF width mismatch");
        ensure!(object.kind == 1, "RISC-V artifact is not a relocatable ELF");
        ensure!(
            object.flags & super::FLOAT_ABI_MASK == float_flags,
            "RISC-V floating-point ABI mismatch"
        );
        // RVC (bit 0) may be absent when a unit contains no compressed code;
        // TSO (bit 4) is an ISA property. RVE and all unknown flags are not in
        // these six supported calling conventions and must not be ignored.
        ensure!(
            object.flags & !0x17 == 0,
            "unsupported RISC-V ELF ABI flags"
        );
        let attributes = super::read(bytes)?;
        ensure!(
            attributes.architecture == self.architecture,
            "RISC-V architecture attribute mismatch"
        );
        ensure!(
            attributes
                .values
                .get(&4)
                .is_none_or(|value| *value == super::Value::Integer(16)),
            "RISC-V stack alignment differs from the selected calling convention"
        );
        for (tag, expected) in [
            (6, u64::from(self.unaligned_access)),
            (14, self.atomic_abi),
            (16, self.x3_reg_usage),
        ] {
            let actual = match attributes.values.get(&tag) {
                None => 0,
                Some(super::Value::Integer(value)) => *value,
                Some(super::Value::String(_)) => anyhow::bail!("invalid RISC-V ABI attribute type"),
            };
            ensure!(actual == expected, "RISC-V ABI attribute {tag} mismatch");
        }
        let has = |extension: &str| {
            self.architecture.split('_').any(|token| {
                token
                    .strip_prefix(extension)
                    .is_some_and(|suffix| suffix.as_bytes().first().is_some_and(u8::is_ascii_digit))
            })
        };
        ensure!(
            object.flags & 1 == 0 || has("c") || has("zca"),
            "RVC flag is not covered by the measured architecture"
        );
        ensure!(
            object.flags & 0x10 == 0 || has("ztso"),
            "TSO flag is not covered by the measured architecture"
        );
        if role == ArtifactRole::ArosRelocatable {
            ensure!(
                object.os_abi == OS_ABI_AROS && object.abi_version == AROS_ABI_VERSION,
                "missing AROS ELF ABI marking"
            );
        } else {
            ensure!(
                (object.os_abi == 0 && object.abi_version == 0)
                    || (object.os_abi == OS_ABI_AROS && object.abi_version == AROS_ABI_VERSION),
                "unsupported compilation-unit ELF ABI marking"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contract(class: u8) -> serde_json::Value {
        json!({"schema": "aros-riscv-target-v1", "isa": if class == 1 {"rv32i"} else {"rv64i"},
            "abi": if class == 1 {"ilp32"} else {"lp64"}, "code_model": "medany",
            "architecture": if class == 1 {"rv32i2p1"} else {"rv64i2p1"},
            "unaligned_access": false, "atomic_abi": 0, "x3_reg_usage": 0})
    }

    #[test]
    fn verifies_independent_widths_and_aros_link_stage() {
        for class in [1, 2] {
            let selected =
                TargetContract::parse(&serde_json::to_vec(&contract(class)).unwrap()).unwrap();
            let mut bytes = super::super::tests::object(class);
            bytes[0x10..0x12].copy_from_slice(&1_u16.to_le_bytes());
            selected
                .verify(&bytes, ArtifactRole::CompilationUnit)
                .unwrap();
            assert!(selected
                .verify(&bytes, ArtifactRole::ArosRelocatable)
                .is_err());
            bytes[7] = OS_ABI_AROS;
            bytes[8] = AROS_ABI_VERSION;
            selected
                .verify(&bytes, ArtifactRole::ArosRelocatable)
                .unwrap();
            for (offset, value, diagnostic) in [
                (0x12, 62_u8, "machine"),
                (0x10, 2, "relocatable"),
                (if class == 1 { 0x24 } else { 0x30 }, 4, "floating-point"),
                (if class == 1 { 0x24 } else { 0x30 }, 8, "ABI flags"),
                (if class == 1 { 0x24 } else { 0x30 }, 1, "RVC flag"),
                (if class == 1 { 0x24 } else { 0x30 }, 16, "TSO flag"),
            ] {
                let mut invalid = bytes.clone();
                invalid[offset] = value;
                assert!(selected
                    .verify(&invalid, ArtifactRole::ArosRelocatable)
                    .unwrap_err()
                    .to_string()
                    .contains(diagnostic));
            }
            let other = TargetContract::parse(
                &serde_json::to_vec(&contract(if class == 1 { 2 } else { 1 })).unwrap(),
            )
            .unwrap();
            assert!(other
                .verify(&bytes, ArtifactRole::ArosRelocatable)
                .unwrap_err()
                .to_string()
                .contains("width"));
            let mut different = contract(class);
            different["architecture"] = json!(if class == 1 { "rv32i2p0" } else { "rv64i2p0" });
            assert!(
                TargetContract::parse(&serde_json::to_vec(&different).unwrap())
                    .unwrap()
                    .verify(&bytes, ArtifactRole::ArosRelocatable)
                    .unwrap_err()
                    .to_string()
                    .contains("attribute mismatch")
            );
            let mut bad_stack = bytes.clone();
            // The fixture has one stack-align tag 4 followed by value 16.
            let attribute = crate::elf::read(&bad_stack)
                .unwrap()
                .sections
                .into_iter()
                .find(|section| section.name == ".riscv.attributes")
                .unwrap();
            let offset = usize::try_from(attribute.offset).unwrap();
            bad_stack[offset + 17] = 8;
            assert!(selected
                .verify(&bad_stack, ArtifactRole::ArosRelocatable)
                .unwrap_err()
                .to_string()
                .contains("stack alignment"));
            for field in ["atomic_abi", "x3_reg_usage", "unaligned_access"] {
                let mut changed = contract(class);
                changed[field] = if field == "unaligned_access" {
                    json!(true)
                } else {
                    json!(1)
                };
                let changed =
                    TargetContract::parse(&serde_json::to_vec(&changed).unwrap()).unwrap();
                assert!(changed
                    .verify(&bytes, ArtifactRole::ArosRelocatable)
                    .unwrap_err()
                    .to_string()
                    .contains("ABI attribute"));
            }
        }
    }

    #[test]
    fn rejects_unsafe_or_inconsistent_contracts() {
        for (field, value) in [
            ("schema", "aros-riscv-target-v2"),
            ("isa", "--march=rv32i"),
            ("abi", "lp64"),
            ("code_model", "anything"),
            ("architecture", "rv64i2p1"),
            ("abi", "ilp32f"),
            ("abi", "ilp32d"),
        ] {
            let mut invalid = contract(1);
            invalid[field] = json!(value);
            assert!(
                TargetContract::parse(&serde_json::to_vec(&invalid).unwrap()).is_err(),
                "{field}:{value}"
            );
        }
        let mut extra = contract(1);
        extra["ignored"] = json!(true);
        assert!(TargetContract::parse(&serde_json::to_vec(&extra).unwrap()).is_err());
        let duplicate = br#"{"schema":"aros-riscv-target-v1","isa":"rv32i","isa":"rv64i","abi":"ilp32","code_model":"medany","architecture":"rv32i2p1"}"#;
        assert!(TargetContract::parse(duplicate).is_err());
        assert!(TargetContract::parse(&vec![b' '; 16 * 1024 + 1]).is_err());
        let mut bypass = contract(1);
        bypass["isa"] = json!("--bad-argument");
        assert!(
            serde_json::from_value::<TargetContract>(bypass).is_err(),
            "embedded deserialization cannot bypass validation"
        );
        for architecture in ["rv32e2p0", "rv32i", "rv32i2", "rv32i2p", "rv32i2p1p0"] {
            let mut invalid = contract(1);
            invalid["architecture"] = json!(architecture);
            assert!(
                TargetContract::parse(&serde_json::to_vec(&invalid).unwrap()).is_err(),
                "{architecture}"
            );
        }
        let mut wrong_base = contract(1);
        wrong_base["isa"] = json!("rv32e");
        assert!(TargetContract::parse(&serde_json::to_vec(&wrong_base).unwrap()).is_err());
    }
}
