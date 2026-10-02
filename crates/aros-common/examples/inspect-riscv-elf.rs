//! Local compiler-qualification aid; this is not an AROS runtime/boot test.

use anyhow::{ensure, Context, Result};
use aros_common::{elf, measure_regular_file_bounded, sha256_bytes};

fn main() -> Result<()> {
    let mut paths = std::env::args_os().skip(1).collect::<Vec<_>>();
    let expectation = if paths.first().is_some_and(|value| value == "--contract") {
        ensure!(
            paths.len() >= 5 && paths[2] == "--role",
            "usage: --contract FILE --role unit|aros ELF..."
        );
        let role = match paths[3].to_str() {
            Some("unit") => elf::riscv::ArtifactRole::CompilationUnit,
            Some("aros") => elf::riscv::ArtifactRole::ArosRelocatable,
            _ => anyhow::bail!("explicit role must be unit or aros"),
        };
        let (_, bytes) = measure_regular_file_bounded(paths[1].as_ref(), 16 * 1024)?
            .context("selected target contract does not exist")?;
        let target = elf::riscv::TargetContract::parse(&bytes)?;
        let identity = sha256_bytes(&bytes);
        paths.drain(..4);
        Some((target, role, identity))
    } else {
        None
    };
    ensure!(!paths.is_empty(), "supply explicitly selected ELF paths");
    for path in paths {
        let (_, bytes) = measure_regular_file_bounded(path.as_ref(), 64 * 1024 * 1024)?
            .context("selected ELF does not exist")?;
        let object = elf::read(&bytes)?;
        let attributes = elf::riscv::read(&bytes)?;
        if let Some((target, role, _)) = &expectation {
            target.verify(&bytes, *role)?;
        }
        let evidence = serde_json::json!({
            "path": path.to_string_lossy(),
            "sha256": sha256_bytes(&bytes).as_str(),
            "size": bytes.len(),
            "elf_class": object.class.pointer_bytes() * 8,
            "elf_type": object.kind,
            "elf_machine": object.machine,
            "elf_flags": object.flags,
            "os_abi": object.os_abi,
            "abi_version": object.abi_version,
            "architecture": attributes.architecture,
            "attributes": attributes.values,
            "target_contract_sha256": expectation.as_ref().map(|(_, _, digest)| digest.as_str()),
        });
        aros_common::outputln!("{}", serde_json::to_string(&evidence)?);
    }
    Ok(())
}
