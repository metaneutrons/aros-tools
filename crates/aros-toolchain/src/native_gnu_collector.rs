//! Install the native collector into a locally built GNU compiler prefix.
//!
//! GNU's legacy collector files are source-owned executable paths referenced
//! by compiler specs. Replace those directory entries atomically, then bind
//! both invocations to their adjacent binutils and publish the closed GNU
//! tool-role contract. All path and role checks happen before staging writes.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use aros_common::toolchain_layout::{ToolchainToolLayout, TOOLCHAIN_TOOLS_FILE};
use aros_common::{
    open_regular_file_nofollow, validate_existing_directory_prefix_nofollow, ArosCompilerIdentity,
};
use serde_json::json;

use crate::package_identity::compiler_identity;
use crate::profiles::Profile;
use crate::source_lock::{CompilerFamily, SourceLock};
use crate::ContractError;

const COLLECTOR_MANIFEST: &str = "aros-collector-tools.json";
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Install and configure the GNU collectors, returning all changed payload paths.
///
/// # Errors
///
/// Fails before writing if the compiler identity, prefix layout, existing
/// collector destinations, or any declared executable role is invalid. Each
/// replacement is staged beside its destination and committed with `rename`,
/// so an existing hard link is never truncated in place.
pub fn install(
    target: &Path,
    prefix: &Path,
    lock: &SourceLock,
    profile: &Profile,
) -> Result<Vec<String>, ContractError> {
    let identity = compiler_identity(lock, profile)?;
    if lock.family() != CompilerFamily::Gnu
        || profile.family() != CompilerFamily::Gnu
        || !matches!(&identity, ArosCompilerIdentity::Gnu { .. })
    {
        return Err(ContractError::collector(
            "native GNU collector installation requires a matching GNU source lock and profile",
        ));
    }

    let (emulation, driver_emulation) = match profile.cpu() {
        "riscv" => ("riscv32elf_aros", "elf32lriscv"),
        "riscv64" => ("riscv64elf_aros", "elf64lriscv"),
        _ => {
            return Err(ContractError::collector(
                "native GNU collector installation supports only RISC-V profile CPUs",
            ));
        }
    };
    let triple = profile.target_triple();
    if !safe_basename(triple) {
        return Err(ContractError::collector(
            "GNU profile target triple is not a safe toolchain path segment",
        ));
    }

    let tuple_directory = PathBuf::from(triple);
    let tuple_bin_directory = tuple_directory.join("bin");
    let tuple_collector_relative = tuple_bin_directory.join("collect-aros");
    let tuple_collector_manifest_relative = tuple_bin_directory.join(COLLECTOR_MANIFEST);
    let root_collector_relative = PathBuf::from(format!("{triple}-collect-aros"));
    let root_collector_manifest_relative = PathBuf::from(COLLECTOR_MANIFEST);

    let tools = json!({
        "c": format!("{triple}-gcc"),
        "cxx": format!("{triple}-g++"),
        "assembler": format!("{triple}-as"),
        "linker": format!("{triple}-ld"),
        "archive": format!("{triple}-ar"),
        "ranlib": format!("{triple}-ranlib"),
        "strip": format!("{triple}-strip"),
        "collector": tuple_collector_relative.to_string_lossy(),
        "nm": format!("{triple}-nm"),
        "objcopy": format!("{triple}-objcopy"),
    });
    let layout_bytes = serde_json::to_vec(&json!({
        "schema": "aros-toolchain-tools-v2",
        "compiler": identity,
        "target_triple": triple,
        "tools": tools,
    }))
    .map_err(|_| ContractError::collector("cannot encode GNU toolchain tools contract"))?;
    let layout = ToolchainToolLayout::parse(&layout_bytes).map_err(|_| {
        ContractError::collector("generated GNU toolchain tools contract is invalid")
    })?;
    layout.validate_binding(&identity, triple).map_err(|_| {
        ContractError::collector("GNU toolchain tools contract identity is invalid")
    })?;

    require_real_directory(prefix, "GNU compiler prefix")?;
    require_real_directory(target, "native collector target directory")?;
    require_real_directory(
        &target.join("release"),
        "native collector release directory",
    )?;
    require_real_directory_chain(prefix, &tuple_directory, "GNU tuple directory")?;
    require_real_directory_chain(prefix, &tuple_bin_directory, "GNU tuple bin directory")?;

    // These are selected as invocation-local linker/strip paths in the
    // adjacent tuple collector manifest, independently of the root-prefixed
    // roles declared in the ten-role tools contract.
    require_regular_executable(
        &prefix.join(&tuple_bin_directory).join("ld"),
        "GNU tuple linker",
    )?;
    require_regular_executable(
        &prefix.join(&tuple_bin_directory).join("strip"),
        "GNU tuple strip tool",
    )?;

    let source_collector = target.join("release/aros-collect");
    require_regular_executable(&source_collector, "native collector build output")?;
    let tuple_collector = prefix.join(&tuple_collector_relative);
    let root_collector = prefix.join(&root_collector_relative);
    require_regular_executable(&tuple_collector, "GNU tuple collector destination")?;
    require_regular_executable(&root_collector, "GNU prefixed collector destination")?;

    let tuple_collector_manifest = prefix.join(&tuple_collector_manifest_relative);
    let root_collector_manifest = prefix.join(&root_collector_manifest_relative);
    let layout_path = prefix.join(TOOLCHAIN_TOOLS_FILE);
    require_absent(&tuple_collector_manifest, "GNU tuple collector manifest")?;
    require_absent(&root_collector_manifest, "GNU prefixed collector manifest")?;
    require_absent(&layout_path, "GNU toolchain tools contract")?;

    // This proves all ten declared roles are present, executable and rooted in
    // the selected prefix before any collector or manifest staging file exists.
    let resolved = layout.resolve_tools(prefix).map_err(|_| {
        ContractError::collector("GNU toolchain is missing a declared executable role")
    })?;
    if resolved.len() != 10 {
        return Err(ContractError::collector(
            "GNU toolchain tools contract does not resolve exactly ten roles",
        ));
    }

    let tuple_manifest_bytes =
        collector_manifest("collect-aros", "ld", "strip", emulation, driver_emulation)?;
    let root_manifest_bytes = collector_manifest(
        root_collector_relative
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ContractError::collector("GNU collector filename is not UTF-8"))?,
        &format!("{triple}-ld"),
        &format!("{triple}-strip"),
        emulation,
        driver_emulation,
    )?;

    let staged = vec![
        stage_copy(&source_collector, &tuple_collector)?,
        stage_copy(&source_collector, &root_collector)?,
        stage_bytes(&tuple_collector_manifest, &tuple_manifest_bytes, 0o644)?,
        stage_bytes(&root_collector_manifest, &root_manifest_bytes, 0o644)?,
        stage_bytes(&layout_path, &layout_bytes, 0o644)?,
    ];
    for file in staged {
        file.publish()?;
    }

    // Read back the persisted contract and check the final executable layout.
    let persisted_layout = fs::read(&layout_path).map_err(|_| {
        ContractError::collector("cannot read persisted GNU toolchain tools contract")
    })?;
    let persisted_layout = ToolchainToolLayout::parse(&persisted_layout).map_err(|_| {
        ContractError::collector("persisted GNU toolchain tools contract is invalid")
    })?;
    persisted_layout
        .validate_binding(&identity, triple)
        .map_err(|_| ContractError::collector("persisted GNU toolchain tools identity changed"))?;
    let resolved = persisted_layout
        .resolve_tools(prefix)
        .map_err(|_| ContractError::collector("persisted GNU toolchain role is unavailable"))?;
    if resolved.len() != 10 {
        return Err(ContractError::collector(
            "persisted GNU toolchain tools contract does not resolve exactly ten roles",
        ));
    }

    Ok(vec![
        tuple_collector_relative.to_string_lossy().into_owned(),
        root_collector_relative.to_string_lossy().into_owned(),
        tuple_collector_manifest_relative
            .to_string_lossy()
            .into_owned(),
        root_collector_manifest_relative
            .to_string_lossy()
            .into_owned(),
        TOOLCHAIN_TOOLS_FILE.to_owned(),
    ])
}

fn collector_manifest(
    invocation: &str,
    linker: &str,
    strip: &str,
    emulation: &str,
    driver_emulation: &str,
) -> Result<Vec<u8>, ContractError> {
    serde_json::to_vec(&json!({
        "schema": "aros-collector-tools-v1",
        "family": "gnu",
        "invocation": invocation,
        "linker": linker,
        "strip": strip,
        "emulation": emulation,
        "driver_emulation": driver_emulation,
    }))
    .map_err(|_| ContractError::collector("cannot encode GNU collector tool manifest"))
}

fn safe_basename(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
}

fn require_real_directory(path: &Path, label: &str) -> Result<(), ContractError> {
    validate_existing_directory_prefix_nofollow(path).map_err(|_| {
        ContractError::collector(format!("{label} has an invalid directory ancestor"))
    })?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::collector(format!("{label} is unavailable")))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ContractError::collector(format!(
            "{label} is not a real directory"
        )));
    }
    Ok(())
}

fn require_real_directory_chain(
    root: &Path,
    relative: &Path,
    label: &str,
) -> Result<(), ContractError> {
    require_real_directory(root, "GNU compiler prefix")?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(ContractError::collector(format!(
                "{label} contains an unsafe path component"
            )));
        };
        current.push(segment);
        require_real_directory(&current, label)?;
    }
    Ok(())
}

fn require_regular_executable(path: &Path, label: &str) -> Result<(), ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::collector(format!("{label} is unavailable")))?;
    if !metadata.file_type().is_file() {
        return Err(ContractError::collector(format!(
            "{label} is not a regular file"
        )));
    }
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(ContractError::collector(format!(
            "{label} is not executable"
        )));
    }
    Ok(())
}

fn require_absent(path: &Path, label: &str) -> Result<(), ContractError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(ContractError::collector(format!("{label} already exists"))),
        Err(_) => Err(ContractError::collector(format!("cannot inspect {label}"))),
    }
}

struct StagedFile {
    temporary: PathBuf,
    destination: PathBuf,
}

impl StagedFile {
    fn publish(self) -> Result<(), ContractError> {
        fs::rename(&self.temporary, &self.destination)
            .map_err(|_| ContractError::collector("cannot atomically publish GNU collector output"))
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        // The path is exclusively ours: it was reserved with create_new and
        // lives below the already validated private prefix.
        let _ = fs::remove_file(&self.temporary);
    }
}

fn create_staging_file(destination: &Path) -> Result<(StagedFile, File), ContractError> {
    let parent = destination
        .parent()
        .ok_or_else(|| ContractError::collector("GNU collector output has no parent directory"))?;
    for _ in 0..128 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".aros-native-gnu-collector-{}-{sequence}.stage",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
        {
            Ok(file) => {
                return Ok((
                    StagedFile {
                        temporary,
                        destination: destination.to_path_buf(),
                    },
                    file,
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => {
                return Err(ContractError::collector(
                    "cannot create private GNU collector staging file",
                ));
            }
        }
    }
    Err(ContractError::collector(
        "cannot reserve a unique GNU collector staging name",
    ))
}

fn stage_copy(source: &Path, destination: &Path) -> Result<StagedFile, ContractError> {
    let (staged, mut output) = create_staging_file(destination)?;
    let mut input = open_regular_file_nofollow(source)
        .map_err(|_| ContractError::collector("cannot reopen native collector build output"))?;
    io::copy(&mut input, &mut output)
        .map_err(|_| ContractError::collector("cannot stage native collector executable"))?;
    output
        .set_permissions(fs::Permissions::from_mode(0o755))
        .map_err(|_| ContractError::collector("cannot set staged collector executable mode"))?;
    output
        .sync_all()
        .map_err(|_| ContractError::collector("cannot synchronize staged collector executable"))?;
    Ok(staged)
}

fn stage_bytes(destination: &Path, bytes: &[u8], mode: u32) -> Result<StagedFile, ContractError> {
    let (staged, mut output) = create_staging_file(destination)?;
    output
        .write_all(bytes)
        .map_err(|_| ContractError::collector("cannot stage GNU collector contract"))?;
    output
        .set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|_| ContractError::collector("cannot set GNU collector contract mode"))?;
    output
        .sync_all()
        .map_err(|_| ContractError::collector("cannot synchronize GNU collector contract"))?;
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::Profiles;
    use serde_json::Value;
    use std::os::unix::fs::{symlink, PermissionsExt};

    const GNU_LOCK: &[u8] = include_bytes!("../tests/fixtures/gnu-source-lock-v3.json");

    struct Fixture {
        temporary: tempfile::TempDir,
        prefix: PathBuf,
        target: PathBuf,
        lock: SourceLock,
        profile: Profile,
        triple: String,
    }

    fn fixture(width: u8) -> Fixture {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let prefix = root.join("prefix");
        let target = root.join("target");
        let lock = SourceLock::parse(GNU_LOCK).unwrap();
        let (cpu, triple, abi, isa, architecture) = if width == 32 {
            (
                "riscv",
                "riscv-aros",
                "ilp32f",
                "rv32imafc",
                "rv32i2p1_m2p0_a2p1_f2p2_c2p0",
            )
        } else {
            (
                "riscv64",
                "riscv64-aros",
                "lp64d",
                "rva22u64",
                "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
            )
        };
        let triple = triple.to_owned();
        let profile_bytes = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v2",
            "family": "gnu",
            "upstream_commit": "d".repeat(40),
            "profiles": [{
                "name": format!("rv{width}-aros"),
                "configure_target": format!("fixture-rv{width}"),
                "upstream_output_target": format!("fixture-rv{width}"),
                "target_triple": triple,
                "cpu": cpu,
                "platform": "fixture",
                "float_abi": abi,
                "capabilities": ["c", "libgcc", "standalone-collector"],
                "target": {
                    "schema": "aros-riscv-target-v1",
                    "isa": isa,
                    "abi": abi,
                    "code_model": "medany",
                    "architecture": architecture,
                    "unaligned_access": false,
                    "atomic_abi": 0,
                    "x3_reg_usage": 0
                }
            }]
        }))
        .unwrap();
        let profiles = Profiles::parse(&profile_bytes).unwrap();
        let profile = profiles.select(&format!("rv{width}-aros")).unwrap().clone();

        fs::create_dir_all(prefix.join(&triple).join("bin")).unwrap();
        fs::create_dir_all(target.join("release")).unwrap();
        for role in [
            "gcc", "g++", "as", "ld", "ar", "ranlib", "strip", "nm", "objcopy",
        ] {
            executable(
                &prefix.join(format!("{triple}-{role}")),
                b"GNU role fixture",
            );
        }
        executable(
            &prefix.join(&triple).join("bin/ld"),
            b"GNU tuple linker fixture",
        );
        executable(
            &prefix.join(&triple).join("bin/strip"),
            b"GNU tuple strip fixture",
        );
        executable(
            &prefix.join(&triple).join("bin/collect-aros"),
            b"legacy tuple collector",
        );
        executable(
            &prefix.join(format!("{triple}-collect-aros")),
            b"legacy root collector",
        );
        executable(
            &target.join("release/aros-collect"),
            b"new native collector",
        );

        Fixture {
            temporary,
            prefix,
            target,
            lock,
            profile,
            triple,
        }
    }

    fn executable(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn manifest(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn assert_no_manifests(fixture: &Fixture) {
        assert!(!fixture.prefix.join(TOOLCHAIN_TOOLS_FILE).exists());
        assert!(!fixture.prefix.join(COLLECTOR_MANIFEST).exists());
        assert!(!fixture
            .prefix
            .join(&fixture.triple)
            .join("bin")
            .join(COLLECTOR_MANIFEST)
            .exists());
    }

    #[test]
    fn installs_valid_rv32_and_rv64_contracts() {
        for width in [32, 64] {
            let fixture = fixture(width);
            let outputs = install(
                &fixture.target,
                &fixture.prefix,
                &fixture.lock,
                &fixture.profile,
            )
            .unwrap();
            assert_eq!(
                outputs,
                [
                    format!("{}/bin/collect-aros", fixture.triple),
                    format!("{}-collect-aros", fixture.triple),
                    format!("{}/bin/{COLLECTOR_MANIFEST}", fixture.triple),
                    COLLECTOR_MANIFEST.to_owned(),
                    TOOLCHAIN_TOOLS_FILE.to_owned(),
                ]
            );
            assert_eq!(
                fs::read(
                    fixture
                        .prefix
                        .join(&fixture.triple)
                        .join("bin/collect-aros")
                )
                .unwrap(),
                b"new native collector"
            );
            assert_eq!(
                fs::read(
                    fixture
                        .prefix
                        .join(format!("{}-collect-aros", fixture.triple))
                )
                .unwrap(),
                b"new native collector"
            );

            let target_manifest = manifest(
                &fixture
                    .prefix
                    .join(&fixture.triple)
                    .join("bin")
                    .join(COLLECTOR_MANIFEST),
            );
            let root_manifest = manifest(&fixture.prefix.join(COLLECTOR_MANIFEST));
            let (emulation, driver_emulation) = if width == 32 {
                ("riscv32elf_aros", "elf32lriscv")
            } else {
                ("riscv64elf_aros", "elf64lriscv")
            };
            for value in [&target_manifest, &root_manifest] {
                assert_eq!(value["schema"], "aros-collector-tools-v1");
                assert_eq!(value["family"], "gnu");
                assert_eq!(value["emulation"], emulation);
                assert_eq!(value["driver_emulation"], driver_emulation);
            }
            assert_eq!(target_manifest["invocation"], "collect-aros");
            assert_eq!(target_manifest["linker"], "ld");
            assert_eq!(target_manifest["strip"], "strip");
            assert_eq!(
                root_manifest["invocation"],
                format!("{}-collect-aros", fixture.triple)
            );
            assert_eq!(root_manifest["linker"], format!("{}-ld", fixture.triple));
            assert_eq!(root_manifest["strip"], format!("{}-strip", fixture.triple));

            let layout = ToolchainToolLayout::load(&fixture.prefix).unwrap();
            layout
                .validate_binding(
                    &compiler_identity(&fixture.lock, &fixture.profile).unwrap(),
                    &fixture.triple,
                )
                .unwrap();
            assert_eq!(layout.resolve_tools(&fixture.prefix).unwrap().len(), 10);
        }
    }

    #[test]
    fn symlinked_tuple_parent_fails_before_replacing_collectors() {
        let fixture = fixture(64);
        let root_collector = fixture
            .prefix
            .join(format!("{}-collect-aros", fixture.triple));
        let external = fixture.temporary.path().join("external-tuple");
        let before = fs::read(&root_collector).unwrap();
        fs::rename(fixture.prefix.join(&fixture.triple), &external).unwrap();
        symlink(&external, fixture.prefix.join(&fixture.triple)).unwrap();

        assert!(install(
            &fixture.target,
            &fixture.prefix,
            &fixture.lock,
            &fixture.profile
        )
        .is_err());
        assert_eq!(fs::read(&root_collector).unwrap(), before);
        assert_eq!(
            fs::read(external.join("bin/collect-aros")).unwrap(),
            b"legacy tuple collector"
        );
        assert_no_manifests(&fixture);
    }

    #[test]
    fn symlinked_collector_destinations_fail_before_any_replacement() {
        for tuple_destination in [false, true] {
            let fixture = fixture(64);
            let root_collector = fixture
                .prefix
                .join(format!("{}-collect-aros", fixture.triple));
            let tuple_collector = fixture
                .prefix
                .join(&fixture.triple)
                .join("bin/collect-aros");
            let external = fixture.temporary.path().join("external-collector");
            executable(&external, b"preserve external collector");
            let unaffected_before = if tuple_destination {
                fs::read(&root_collector).unwrap()
            } else {
                fs::read(&tuple_collector).unwrap()
            };
            let destination = if tuple_destination {
                &tuple_collector
            } else {
                &root_collector
            };
            fs::remove_file(destination).unwrap();
            symlink(&external, destination).unwrap();

            assert!(install(
                &fixture.target,
                &fixture.prefix,
                &fixture.lock,
                &fixture.profile
            )
            .is_err());
            assert_eq!(fs::read(&external).unwrap(), b"preserve external collector");
            let unaffected = if tuple_destination {
                &root_collector
            } else {
                &tuple_collector
            };
            assert_eq!(fs::read(unaffected).unwrap(), unaffected_before);
            assert_no_manifests(&fixture);
        }
    }

    #[test]
    fn missing_or_symlinked_tuple_linker_and_strip_fail_before_replacement() {
        for tool in ["ld", "strip"] {
            for symlinked in [false, true] {
                let fixture = fixture(64);
                let tuple_tool = fixture.prefix.join(&fixture.triple).join("bin").join(tool);
                let tuple_collector = fixture
                    .prefix
                    .join(&fixture.triple)
                    .join("bin/collect-aros");
                let root_collector = fixture
                    .prefix
                    .join(format!("{}-collect-aros", fixture.triple));
                let tuple_before = fs::read(&tuple_collector).unwrap();
                let root_before = fs::read(&root_collector).unwrap();

                fs::remove_file(&tuple_tool).unwrap();
                if symlinked {
                    let external = fixture.temporary.path().join(format!("external-{tool}"));
                    executable(&external, b"preserve external adjacent tool");
                    symlink(&external, &tuple_tool).unwrap();
                }

                assert!(install(
                    &fixture.target,
                    &fixture.prefix,
                    &fixture.lock,
                    &fixture.profile
                )
                .is_err());
                assert_eq!(fs::read(tuple_collector).unwrap(), tuple_before);
                assert_eq!(fs::read(root_collector).unwrap(), root_before);
                assert_no_manifests(&fixture);
            }
        }
    }

    #[test]
    fn missing_role_fails_before_overwriting_existing_collectors() {
        let fixture = fixture(64);
        let tuple_collector = fixture
            .prefix
            .join(&fixture.triple)
            .join("bin/collect-aros");
        let root_collector = fixture
            .prefix
            .join(format!("{}-collect-aros", fixture.triple));
        let tuple_before = fs::read(&tuple_collector).unwrap();
        let root_before = fs::read(&root_collector).unwrap();
        fs::remove_file(fixture.prefix.join(format!("{}-objcopy", fixture.triple))).unwrap();

        assert!(install(
            &fixture.target,
            &fixture.prefix,
            &fixture.lock,
            &fixture.profile
        )
        .is_err());
        assert_eq!(fs::read(tuple_collector).unwrap(), tuple_before);
        assert_eq!(fs::read(root_collector).unwrap(), root_before);
        assert_no_manifests(&fixture);
    }

    #[test]
    fn mismatched_family_fails_without_writes() {
        let fixture = fixture(64);
        let root_collector = fixture
            .prefix
            .join(format!("{}-collect-aros", fixture.triple));
        let before = fs::read(&root_collector).unwrap();
        let llvm_profile = Profiles::parse(
            br#"{"schema":"aros-toolchain-profiles-v1","upstream_commit":"dddddddddddddddddddddddddddddddddddddddd","profiles":[{"name":"llvm-test","configure_target":"pc-x86_64","upstream_output_target":"pc-x86_64","target_triple":"x86_64-unknown-aros","cpu":"x86_64","platform":"pc","float_abi":"","capabilities":["c","standalone-collector"]}]}"#,
        )
        .unwrap()
        .select("llvm-test")
        .unwrap()
        .clone();

        assert!(install(
            &fixture.target,
            &fixture.prefix,
            &fixture.lock,
            &llvm_profile
        )
        .is_err());
        assert_eq!(fs::read(root_collector).unwrap(), before);
        assert_no_manifests(&fixture);
    }

    #[test]
    fn collector_replacement_does_not_truncate_an_existing_hard_link() {
        let fixture = fixture(64);
        let root_collector = fixture
            .prefix
            .join(format!("{}-collect-aros", fixture.triple));
        let external_hard_link = fixture.temporary.path().join("old-collector-hard-link");
        fs::hard_link(&root_collector, &external_hard_link).unwrap();

        install(
            &fixture.target,
            &fixture.prefix,
            &fixture.lock,
            &fixture.profile,
        )
        .unwrap();
        assert_eq!(fs::read(&root_collector).unwrap(), b"new native collector");
        assert_eq!(
            fs::read(external_hard_link).unwrap(),
            b"legacy root collector"
        );
    }
}
