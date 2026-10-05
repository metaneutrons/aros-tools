//! Pre-build local-native input stamp, not a successful-build/media receipt.

use aros_common::{
    local_source::LocalSourceIdentity,
    local_toolchain::{LocalToolchainDescriptor, LOCAL_TOOLCHAIN_DESCRIPTOR_FILE},
    native_build_contract::load_bound_native_build_contract,
    AtomicFilePolicy, Sha256Digest, TargetProfile, TreeTraversalLimits,
};
use miette::{IntoDiagnostic, Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, process::Command};

const STAMP_NAME: &str = ".aros-local-native-inputs.json";
const MAX_STAMP_BYTES: u64 = 64 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CONFIGURED_BOARD_INPUT_BYTES: u64 = 256 * 1024 * 1024;
const KOBJ_MEMBERS: &[&str] = &["kernel_resource.o", "exec_library.o", "task_resource.o"];
const TOOL_NAMES: &[&str] = &[
    "aros-transpiler",
    "aros-genmodule",
    "aros-romtool",
    "aros-collect",
    "aros-ahi-runner",
    "aros-fetch",
    "aros-verify",
];

pub(super) fn validate_source_namespace(
    profile: &TargetProfile,
    contract: &aros_common::native_build_contract::LoadedNativeBuildContract,
) -> Result<()> {
    if profile
        .native_build_contract
        .as_ref()
        .is_some_and(|path| path.starts_with("build/"))
        || contract
            .contract
            .inputs
            .iter()
            .any(|input| input.path.starts_with("build/"))
    {
        miette::bail!("native source contract must not read the generated build namespace");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LocalNativeInputs {
    schema: String,
    source: LocalSourceIdentity,
    native_contract_sha256: Sha256Digest,
    toolchain_descriptor_sha256: Sha256Digest,
    toolchain_tree_sha256: Sha256Digest,
    engine_payload_sha256: Sha256Digest,
    executable_sha256: BTreeMap<String, Sha256Digest>,
    executor_paths: BTreeMap<String, String>,
    environment_sha256: BTreeMap<String, Option<Sha256Digest>>,
    configure_arguments: Vec<String>,
}

impl LocalNativeInputs {
    pub(super) fn capture(
        root: &Path,
        preset: &str,
        profile: &TargetProfile,
        toolchain_root: &Path,
        engine: &Path,
        tools: &Path,
        configure: &Command,
    ) -> Result<Self> {
        let source =
            LocalSourceIdentity::capture(root, preset).map_err(|error| miette::miette!(error))?;
        let relative = profile.native_build_contract.as_ref().ok_or_else(|| {
            miette::miette!("local native binding requires a source build contract")
        })?;
        let contract = load_bound_native_build_contract(root, Path::new(relative), profile)
            .into_diagnostic()?;
        validate_source_namespace(profile, &contract)?;
        let (_, descriptor_bytes) = aros_common::measure_regular_file_bounded(
            &toolchain_root.join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE),
            16 * 1024 * 1024,
        )
        .into_diagnostic()?
        .ok_or_else(|| miette::miette!("local compiler descriptor disappeared"))?;
        let descriptor = LocalToolchainDescriptor::parse(&descriptor_bytes)
            .map_err(|error| miette::miette!(error))?;
        descriptor
            .verify(toolchain_root)
            .map_err(|error| miette::miette!(error))?;
        if descriptor.target_profile != profile.toolchain_profile() {
            miette::bail!("local native compiler profile differs from source selection");
        }
        let engine_tree = aros_common::measure_tree_content_cas_bounded(
            engine,
            TreeTraversalLimits::new(4096, 64 * 1024 * 1024).into_diagnostic()?,
        )
        .into_diagnostic()
        .wrap_err("cannot bind the actual CMake engine bytes")?;
        if engine_tree.has_symlinks() {
            miette::bail!(
                "local native CMake engine must contain only regular files and directories"
            );
        }
        let mut executable_sha256 = BTreeMap::new();
        executable_sha256.insert(
            "aros".into(),
            executable_digest(&std::env::current_exe().into_diagnostic()?)?,
        );
        for name in TOOL_NAMES {
            executable_sha256.insert((*name).into(), executable_digest(&tools.join(name))?);
        }
        let configure_arguments = configure
            .get_args()
            .map(|argument| {
                argument
                    .to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| miette::miette!("local native configure argument is not UTF-8"))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut executor_paths = BTreeMap::new();
        for (role, path) in std::iter::once((
            "cmake",
            configure.get_program().to_string_lossy().into_owned(),
        ))
        .chain(
            [
                ("ninja", "CMAKE_MAKE_PROGRAM"),
                ("compiler-cache", "AROS_COMPILER_CACHE_EXECUTABLE"),
            ]
            .into_iter()
            .filter_map(|(role, key)| {
                let prefix = format!("-D{key}=");
                configure_arguments
                    .iter()
                    .rev()
                    .find_map(|argument| argument.strip_prefix(&prefix))
                    .filter(|value| !value.is_empty())
                    .map(|value| (role, value.to_owned()))
            }),
        ) {
            // Frontend selects absolute executors. Do not rediscover a different
            // program through PATH after the pre-build stamp was written.
            let path = Path::new(&path);
            if !path.is_absolute() {
                miette::bail!("local native executor {role} is not an absolute path");
            }
            executable_sha256.insert(role.into(), executable_digest(path)?);
            executor_paths.insert(
                role.into(),
                path.to_str()
                    .ok_or_else(|| miette::miette!("executor path is not UTF-8"))?
                    .into(),
            );
        }
        // Source could change while the other input inventories are read.
        source
            .verify(root, preset)
            .map_err(|error| miette::miette!(error))?;
        Ok(Self {
            schema: "aros-local-native-inputs-v1".into(),
            source,
            native_contract_sha256: contract.sha256,
            toolchain_descriptor_sha256: aros_common::sha256_bytes(&descriptor_bytes),
            toolchain_tree_sha256: Sha256Digest::parse(&descriptor.tree_sha256)
                .into_diagnostic()?,
            engine_payload_sha256: engine_tree.payload_digest_excluding(None),
            executable_sha256,
            executor_paths,
            environment_sha256: environment_identity(configure),
            configure_arguments,
        })
    }

    pub(super) fn bind_build_tree(&self, build_dir: &Path) -> Result<()> {
        let stamp_path = build_dir.join(STAMP_NAME);
        if let Some((_, bytes)) =
            aros_common::measure_regular_file_bounded(&stamp_path, MAX_STAMP_BYTES)
                .into_diagnostic()?
        {
            let previous: Self = serde_json::from_slice(&bytes).into_diagnostic().wrap_err(
                "local native input stamp is invalid; preserve this tree for diagnosis",
            )?;
            if previous != *self {
                miette::bail!(
                    "local native inputs changed; use a fresh build tree or explicitly --clean after preserving its evidence"
                );
            }
            return Ok(());
        }
        if build_dir.join("CMakeCache.txt").symlink_metadata().is_ok() {
            miette::bail!(
                "existing local native build has no pre-build input stamp; do not adopt historical outputs"
            );
        }
        let bytes = serde_json::to_vec_pretty(self).into_diagnostic()?;
        if bytes.len() > MAX_STAMP_BYTES as usize {
            miette::bail!("local native input stamp exceeds 64 KiB");
        }
        aros_common::publish_atomic_file(&stamp_path, &bytes, AtomicFilePolicy::NoClobber)
            .into_diagnostic()
            .wrap_err("cannot publish local native pre-build input stamp")?;
        Ok(())
    }

    pub(super) fn require_unchanged(&self, actual: &Self) -> Result<()> {
        if *self != *actual {
            miette::bail!(
                "local native source/compiler/engine/tools/configure inputs changed during execution; outputs are not qualified"
            );
        }
        Ok(())
    }
}

fn environment_identity(command: &Command) -> BTreeMap<String, Option<Sha256Digest>> {
    let mut values = BTreeMap::new();
    for name in [
        "PATH",
        "HOME",
        "CC",
        "CXX",
        "AS",
        "AR",
        "RANLIB",
        "NM",
        "STRIP",
        "OBJCOPY",
        "OBJDUMP",
        "CFLAGS",
        "CXXFLAGS",
        "CPPFLAGS",
        "LDFLAGS",
        "CPATH",
        "C_INCLUDE_PATH",
        "CPLUS_INCLUDE_PATH",
        "LIBRARY_PATH",
        "SDKROOT",
        "MACOSX_DEPLOYMENT_TARGET",
        "CMAKE_PREFIX_PATH",
        "MAKEFLAGS",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
    ] {
        let value = std::env::var_os(name);
        values.insert(
            name.to_owned(),
            value.map(|value| aros_common::sha256_bytes(value.as_encoded_bytes())),
        );
    }
    for (name, value) in command.get_envs() {
        values.insert(
            name.to_string_lossy().into_owned(),
            value.map(|value| aros_common::sha256_bytes(value.as_encoded_bytes())),
        );
    }
    values
}

const CONFIGURATION_STAMP: &str = ".aros-local-native-configuration.json";
const CONFIGURATION_FILES: &[&str] = &["CMakeCache.txt", "build.ninja", "CMakeFiles/rules.ninja"];

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfiguredTree {
    schema: String,
    files: BTreeMap<String, Sha256Digest>,
    programs: BTreeMap<String, ConfiguredProgram>,
    board_file_inputs: BTreeMap<String, ConfiguredExternalFile>,
    board_object_directories: BTreeMap<String, ConfiguredObjectDirectory>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfiguredProgram {
    resolved_path: String,
    sha256: Sha256Digest,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfiguredExternalFile {
    configured_path: String,
    resolved_path: String,
    sha256: Option<Sha256Digest>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfiguredObjectDirectory {
    configured_path: String,
    resolved_path: String,
    is_directory: bool,
    members: BTreeMap<String, ConfiguredObjectMember>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfiguredObjectMember {
    resolved_path: String,
    internal_to_build: bool,
    sha256: Option<Sha256Digest>,
}

impl ConfiguredTree {
    fn capture(build: &Path) -> Result<Self> {
        let mut files = BTreeMap::new();
        let mut programs = BTreeMap::new();
        let mut board_file_inputs = BTreeMap::new();
        let mut board_object_directories = BTreeMap::new();
        let mut pending: Vec<String> = CONFIGURATION_FILES
            .iter()
            .map(|name| (*name).into())
            .collect();
        let mut total_bytes = 0_u64;
        while let Some(name) = pending.pop() {
            validate_configuration_path(&name)?;
            if files.contains_key(&name) {
                continue;
            }
            if files.len() >= 1024 {
                miette::bail!("local native configuration exceeds 1024 files");
            }
            let (_, bytes) =
                aros_common::measure_regular_file_bounded(&build.join(&name), 16 * 1024 * 1024)
                    .into_diagnostic()?
                    .ok_or_else(|| {
                        miette::miette!("local native configuration file is missing: {name}")
                    })?;
            total_bytes += bytes.len() as u64;
            if total_bytes > 64 * 1024 * 1024 {
                miette::bail!("local native configuration exceeds 64 MiB");
            }
            if name == "CMakeCache.txt" {
                programs = configured_programs(build, &bytes)?;
                (board_file_inputs, board_object_directories) =
                    configured_board_inputs(build, &bytes)?;
            }
            // Included manifests need not have a .ninja suffix. CMake's cache
            // is also text and cannot contain standalone Ninja directives.
            let text = std::str::from_utf8(&bytes).into_diagnostic()?;
            for line in text.lines() {
                if let Some(value) = ninja_include_value(line) {
                    if pending.len() >= 4096 {
                        miette::bail!("local native configuration has too many include references");
                    }
                    pending.push(literal_ninja_path(value)?);
                }
            }
            files.insert(name, aros_common::sha256_bytes(&bytes));
        }
        Ok(Self {
            schema: "aros-local-native-configuration-v2".into(),
            files,
            programs,
            board_file_inputs,
            board_object_directories,
        })
    }
}

fn configured_programs(build: &Path, cache: &[u8]) -> Result<BTreeMap<String, ConfiguredProgram>> {
    let text = std::str::from_utf8(cache).into_diagnostic()?;
    let canonical_build = build.canonicalize().into_diagnostic()?;
    let mut programs = BTreeMap::new();
    for line in text.lines().filter(|line| !line.starts_with(['#', '/'])) {
        let Some((declaration, value)) = line.split_once('=') else {
            continue;
        };
        let Some((name, kind)) = declaration.split_once(':') else {
            continue;
        };
        let required = [
            "_EXECUTABLE",
            "_COMPILER",
            "_COMPILER_AR",
            "_COMPILER_RANLIB",
            "_OBJCOPY",
            "_OBJDUMP",
            "_STRIP",
            "_CC",
            "_CXX",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
            || matches!(
                name,
                "CMAKE_AR" | "CMAKE_RANLIB" | "CMAKE_NM" | "CMAKE_LINKER" | "CMAKE_MAKE_PROGRAM"
            );
        if value.is_empty() || value.ends_with("-NOTFOUND") || (!required && kind != "FILEPATH") {
            continue;
        }
        let candidate = if Path::new(value).is_absolute() {
            Some(Path::new(value).to_path_buf())
        } else if required {
            Some(
                which::which(value)
                    .into_diagnostic()
                    .wrap_err_with(|| format!("cannot resolve configured program {name}"))?,
            )
        } else {
            None
        };
        let Some(candidate) = candidate else { continue };
        if candidate
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            miette::bail!("configured external program {name} contains parent traversal");
        }
        let (resolved, exists) = canonicalize_existing_prefix(&candidate)?;
        // Generated host tools are native build products, not external inputs.
        // Resolve first so a build-local symlink cannot hide an external tool.
        if resolved.starts_with(&canonical_build) {
            continue;
        }
        if !exists {
            if required {
                miette::bail!("configured program {name} is not an executable regular file");
            }
            continue;
        }
        let executable = resolved.is_file() && {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                resolved.metadata().into_diagnostic()?.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        };
        if !required && !executable {
            continue;
        }
        if required && !executable {
            miette::bail!("configured program {name} is not an executable regular file");
        }
        if programs.len() >= 128 {
            miette::bail!("local native cache exceeds 128 external programs");
        }
        programs.insert(
            name.into(),
            ConfiguredProgram {
                resolved_path: resolved
                    .to_str()
                    .ok_or_else(|| miette::miette!("configured program path is not UTF-8"))?
                    .into(),
                sha256: executable_digest(&resolved)?,
            },
        );
    }
    Ok(programs)
}

fn configured_board_inputs(
    build: &Path,
    cache: &[u8],
) -> Result<(
    BTreeMap<String, ConfiguredExternalFile>,
    BTreeMap<String, ConfiguredObjectDirectory>,
)> {
    let text = std::str::from_utf8(cache).into_diagnostic()?;
    let canonical_build = build.canonicalize().into_diagnostic()?;
    let mut board_files = BTreeMap::new();
    let mut object_directories = BTreeMap::new();
    let mut total_bytes = 0_u64;

    for line in text.lines().filter(|line| !line.starts_with(['#', '/'])) {
        let Some((declaration, value)) = line.split_once('=') else {
            continue;
        };
        let Some((name, _kind)) = declaration.split_once(':') else {
            continue;
        };
        if value.is_empty() || value.ends_with("-NOTFOUND") {
            continue;
        }

        if name == "AROS_RPI_DTB" {
            let candidate = configured_input_path(value, None)?;
            let (resolved, exists) = canonicalize_existing_prefix(&candidate)?;
            let sha256 = if exists {
                Some(configured_input_digest(&resolved, &mut total_bytes)?)
            } else {
                None
            };
            board_files.insert(
                name.into(),
                ConfiguredExternalFile {
                    configured_path: value.into(),
                    resolved_path: resolved
                        .to_str()
                        .ok_or_else(|| miette::miette!("configured DTB path is not UTF-8"))?
                        .into(),
                    sha256,
                },
            );
            continue;
        }

        if !matches!(
            name,
            "AROS_RPI_CORE_KOBJ_DIR" | "AROS_OPENSBI_CORE_KOBJ_DIR"
        ) {
            continue;
        }

        // The CMake engine resolves these cache PATH entries relative to its
        // binary directory. Board CLI normally supplies canonical absolutes.
        let candidate = configured_input_path(value, Some(&canonical_build))?;
        let (resolved, exists) = canonicalize_existing_prefix(&candidate)?;
        let is_directory = if exists {
            let metadata = std::fs::metadata(&resolved).into_diagnostic()?;
            if !metadata.is_dir() && !metadata.is_file() {
                miette::bail!("configured KOBJ path is a special filesystem object");
            }
            metadata.is_dir()
        } else {
            false
        };

        let mut members = BTreeMap::new();
        for member in KOBJ_MEMBERS {
            let path = resolved.join(member);
            let (member_resolved, member_exists) = if is_directory {
                canonicalize_existing_prefix(&path)?
            } else {
                (path, false)
            };
            let internal_to_build = member_resolved.starts_with(&canonical_build);
            let sha256 = if member_exists {
                Some(configured_input_digest(&member_resolved, &mut total_bytes)?)
            } else {
                None
            };
            members.insert(
                (*member).into(),
                ConfiguredObjectMember {
                    resolved_path: member_resolved
                        .to_str()
                        .ok_or_else(|| miette::miette!("configured KOBJ member path is not UTF-8"))?
                        .into(),
                    internal_to_build,
                    sha256,
                },
            );
        }
        object_directories.insert(
            name.into(),
            ConfiguredObjectDirectory {
                configured_path: value.into(),
                resolved_path: resolved
                    .to_str()
                    .ok_or_else(|| miette::miette!("configured KOBJ directory is not UTF-8"))?
                    .into(),
                is_directory,
                members,
            },
        );
    }

    Ok((board_files, object_directories))
}

fn configured_input_path(value: &str, relative_base: Option<&Path>) -> Result<std::path::PathBuf> {
    if value.contains('\\') || value.chars().any(char::is_control) {
        miette::bail!("configured board input path contains unsafe characters");
    }
    let path = Path::new(value);
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        miette::bail!("configured board input path contains parent traversal");
    }
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    relative_base
        .map(|base| base.join(path))
        .ok_or_else(|| miette::miette!("configured DTB path must be absolute"))
}

// Resolve every existing path prefix, so a missing final file beneath a
// symlinked directory is still associated with the directory the engine will
// use if that file appears. A dangling symlink is rejected, not treated as a
// missing regular input.
fn canonicalize_existing_prefix(path: &Path) -> Result<(std::path::PathBuf, bool)> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        miette::bail!("configured board input path must be absolute and traversal-free");
    }

    let mut resolved = std::path::PathBuf::new();
    let mut components = path.components();
    while let Some(component) = components.next() {
        match component {
            std::path::Component::RootDir => resolved.push(component.as_os_str()),
            std::path::Component::Normal(name) => {
                let next = resolved.join(name);
                match std::fs::symlink_metadata(&next) {
                    Ok(_) => resolved = next.canonicalize().into_diagnostic()?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        resolved.push(name);
                        for remaining in components {
                            match remaining {
                                std::path::Component::Normal(name) => resolved.push(name),
                                std::path::Component::CurDir => {}
                                _ => miette::bail!(
                                    "configured board input path has an unsafe component"
                                ),
                            }
                        }
                        return Ok((resolved, false));
                    }
                    Err(error) => return Err(error).into_diagnostic(),
                }
            }
            std::path::Component::CurDir => {}
            _ => miette::bail!("configured board input path has an unsafe component"),
        }
    }
    Ok((resolved, true))
}

fn configured_input_digest(path: &Path, total_bytes: &mut u64) -> Result<Sha256Digest> {
    let remaining = MAX_CONFIGURED_BOARD_INPUT_BYTES
        .checked_sub(*total_bytes)
        .ok_or_else(|| miette::miette!("configured board inputs exceed 256 MiB"))?;
    let (_, bytes) = aros_common::measure_regular_file_bounded(path, remaining)
        .into_diagnostic()?
        .ok_or_else(|| miette::miette!("configured board input disappeared during capture"))?;
    *total_bytes = total_bytes
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| miette::miette!("configured board input size overflow"))?;
    Ok(aros_common::sha256_bytes(&bytes))
}

fn validate_configuration_path(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
        || name
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        miette::bail!("local native configuration include must be a safe build-relative path");
    }
    Ok(())
}

fn ninja_include_value(line: &str) -> Option<&str> {
    ["include", "subninja"].into_iter().find_map(|keyword| {
        line.strip_prefix(keyword)
            .filter(|value| value.starts_with([' ', '\t']))
            .map(|value| value.trim_start_matches([' ', '\t']))
    })
}

// CMake's Ninja generator emits literal paths. Refuse variable expansion and
// continuations instead of implementing a second, approximate Ninja evaluator.
fn literal_ninja_path(value: &str) -> Result<String> {
    let mut path = String::new();
    let mut characters = value.trim().chars();
    while let Some(character) = characters.next() {
        if character == '$' {
            match characters.next() {
                Some(escaped @ (' ' | ':' | '$')) => path.push(escaped),
                _ => {
                    miette::bail!("local native configuration requires literal Ninja include paths")
                }
            }
        } else {
            path.push(character);
        }
    }
    validate_configuration_path(&path)?;
    Ok(path)
}

pub(super) fn verify_configured_tree(build: &Path, after_configure: bool) -> Result<()> {
    let path = build.join(CONFIGURATION_STAMP);
    let stamp =
        aros_common::measure_regular_file_bounded(&path, MAX_STAMP_BYTES).into_diagnostic()?;
    if let Some((_, bytes)) = stamp {
        let expected: ConfiguredTree = serde_json::from_slice(&bytes).into_diagnostic()?;
        if expected != ConfiguredTree::capture(build)? {
            miette::bail!(
                "local native CMake cache/Ninja rules differ from the recorded configuration; preserve evidence and use a fresh tree"
            );
        }
    } else if after_configure {
        let actual = ConfiguredTree::capture(build)?;
        let bytes = serde_json::to_vec(&actual).into_diagnostic()?;
        if bytes.len() > MAX_STAMP_BYTES as usize {
            miette::bail!("local native configuration stamp exceeds 64 KiB");
        }
        aros_common::publish_atomic_file(&path, &bytes, AtomicFilePolicy::NoClobber)
            .into_diagnostic()?;
    } else if CONFIGURATION_FILES
        .iter()
        .any(|name| build.join(name).symlink_metadata().is_ok())
    {
        miette::bail!(
            "local native build has unbound historical CMake/Ninja configuration; use a fresh tree"
        );
    }
    Ok(())
}

fn executable_digest(path: &Path) -> Result<Sha256Digest> {
    let (_, bytes) = aros_common::measure_regular_file_bounded(path, MAX_EXECUTABLE_BYTES)
        .into_diagnostic()?
        .ok_or_else(|| {
            miette::miette!("native producer executable disappeared: {}", path.display())
        })?;
    Ok(aros_common::sha256_bytes(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_configuration_fixture(build: &Path, cache: &str) {
        fs::create_dir_all(build.join("CMakeFiles")).unwrap();
        fs::write(build.join("CMakeCache.txt"), cache).unwrap();
        fs::write(build.join("build.ninja"), b"fixture build rules").unwrap();
        fs::write(build.join("CMakeFiles/rules.ninja"), b"fixture rules").unwrap();
    }

    fn identity() -> LocalNativeInputs {
        let digest = aros_common::sha256_bytes(b"fixture identity");
        LocalNativeInputs {
            schema: "aros-local-native-inputs-v1".into(),
            source: LocalSourceIdentity {
                schema: "aros-local-source-v1".into(),
                head_baseline: "a".repeat(40),
                submodules_sha256: digest.clone(),
                generated_subtree: "build/fixture".into(),
                content_sha256: digest.clone(),
                entry_count: 1,
                regular_file_bytes: 7,
            },
            native_contract_sha256: digest.clone(),
            toolchain_descriptor_sha256: digest.clone(),
            toolchain_tree_sha256: digest.clone(),
            engine_payload_sha256: digest.clone(),
            executable_sha256: BTreeMap::from([("aros".into(), digest)]),
            executor_paths: BTreeMap::new(),
            environment_sha256: BTreeMap::new(),
            configure_arguments: vec!["-DAROS_TARGET_PROFILE=fixture".into()],
        }
    }

    #[test]
    fn local_native_stamp_is_prebuild_binding_and_refuses_mixed_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let original = identity();
        original.bind_build_tree(temp.path()).unwrap();
        let path = temp.path().join(STAMP_NAME);
        let bytes = fs::read(&path).unwrap();
        original.bind_build_tree(temp.path()).unwrap();
        let mut changed = original.clone();
        changed.source.content_sha256 = aros_common::sha256_bytes(b"dirty source");
        assert!(changed.bind_build_tree(temp.path()).is_err());
        assert!(original.require_unchanged(&changed).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        for field in ["release_id", "build_succeeded"] {
            let mut json = serde_json::to_value(&original).unwrap();
            json[field] = true.into();
            assert!(serde_json::from_value::<LocalNativeInputs>(json).is_err());
        }
    }

    #[test]
    fn local_native_stamp_refuses_unbound_existing_build_and_unsafe_metadata() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("CMakeCache.txt"), b"historical build").unwrap();
        assert!(identity().bind_build_tree(temp.path()).is_err());
        assert!(!temp.path().join(STAMP_NAME).exists());
        fs::remove_file(temp.path().join("CMakeCache.txt")).unwrap();
        let path = temp.path().join(STAMP_NAME);
        for bytes in [
            b"invalid JSON".as_slice(),
            &vec![b'x'; MAX_STAMP_BYTES as usize + 1],
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(identity().bind_build_tree(temp.path()).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(identity().bind_build_tree(temp.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn local_native_stamp_refuses_symlink_without_mutating_target() {
        let temp = tempfile::tempdir().unwrap();
        let sentinel = temp.path().join("sentinel");
        fs::write(&sentinel, b"must remain unchanged").unwrap();
        std::os::unix::fs::symlink(&sentinel, temp.path().join(STAMP_NAME)).unwrap();
        assert!(identity().bind_build_tree(temp.path()).is_err());
        assert_eq!(fs::read(sentinel).unwrap(), b"must remain unchanged");
    }

    #[test]
    fn local_native_configuration_refuses_cache_and_rule_tampering() {
        let temp = tempfile::tempdir().unwrap();
        verify_configured_tree(temp.path(), false).unwrap();
        fs::create_dir(temp.path().join("CMakeFiles")).unwrap();
        for name in CONFIGURATION_FILES {
            fs::write(temp.path().join(name), name.as_bytes()).unwrap();
        }
        assert!(verify_configured_tree(temp.path(), false).is_err());
        verify_configured_tree(temp.path(), true).unwrap();
        verify_configured_tree(temp.path(), false).unwrap();
        let stamp = fs::read(temp.path().join(CONFIGURATION_STAMP)).unwrap();
        for name in CONFIGURATION_FILES {
            fs::write(temp.path().join(name), b"tampered configuration").unwrap();
            assert!(verify_configured_tree(temp.path(), false).is_err());
            assert!(verify_configured_tree(temp.path(), true).is_err());
            assert_eq!(
                fs::read(temp.path().join(CONFIGURATION_STAMP)).unwrap(),
                stamp
            );
            fs::write(temp.path().join(name), name.as_bytes()).unwrap();
        }
    }

    #[test]
    fn local_native_configuration_binds_literal_ninja_include_closure() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("CMakeFiles")).unwrap();
        fs::write(temp.path().join("CMakeCache.txt"), b"fixture cache").unwrap();
        fs::write(temp.path().join("CMakeFiles/rules.ninja"), b"fixture rules").unwrap();
        fs::write(
            temp.path().join("build.ninja"),
            b"include CMakeFiles/impl-Release.ninja\n",
        )
        .unwrap();
        let included = temp.path().join("CMakeFiles/impl-Release.ninja");
        fs::write(&included, b"subninja\tCMakeFiles/no-suffix\n").unwrap();
        fs::write(
            temp.path().join("CMakeFiles/no-suffix"),
            b"include CMakeFiles/rules.ninja\n",
        )
        .unwrap();
        let first = ConfiguredTree::capture(temp.path()).unwrap();
        assert!(first.files.contains_key("CMakeFiles/impl-Release.ninja"));
        assert!(first.files.contains_key("CMakeFiles/no-suffix"));
        verify_configured_tree(temp.path(), true).unwrap();
        fs::write(&included, b"changed command edge").unwrap();
        assert!(verify_configured_tree(temp.path(), false).is_err());
        for text in [
            "include ../outside.ninja\n",
            "include $dynamic\n",
            "include /outside.ninja\n",
        ] {
            fs::write(temp.path().join("build.ninja"), text).unwrap();
            assert!(ConfiguredTree::capture(temp.path()).is_err());
        }
        assert_eq!(
            literal_ninja_path("CMakeFiles/with$ space.ninja").unwrap(),
            "CMakeFiles/with space.ninja"
        );
    }

    #[test]
    fn local_native_configuration_binds_discovered_external_program_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let build = temp.path().join("build");
        fs::create_dir_all(build.join("CMakeFiles")).unwrap();
        for name in CONFIGURATION_FILES {
            fs::write(build.join(name), b"fixture configuration").unwrap();
        }
        let program = temp.path().join("host-compiler");
        fs::write(&program, b"original external compiler").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(
            build.join("CMakeCache.txt"),
            format!("AROS_HOST_CC:STRING={}\n", program.display()),
        )
        .unwrap();
        verify_configured_tree(&build, true).unwrap();
        let first = ConfiguredTree::capture(&build).unwrap();
        assert!(first.programs.contains_key("AROS_HOST_CC"));
        fs::write(&program, b"replaced compiler at identical path").unwrap();
        assert!(verify_configured_tree(&build, false).is_err());
        fs::write(
            build.join("CMakeCache.txt"),
            format!("AROS_HOST_CC:STRING={}/../host-compiler\n", build.display()),
        )
        .unwrap();
        assert!(ConfiguredTree::capture(&build).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn local_native_configuration_resolves_build_program_symlink_before_exclusion() {
        use std::os::unix::fs::{symlink, PermissionsExt as _};

        let temp = tempfile::tempdir().unwrap();
        let build = temp.path().join("build");
        fs::create_dir_all(build.join("CMakeFiles")).unwrap();
        let external_tools = temp.path().join("external-tools");
        fs::create_dir(&external_tools).unwrap();
        let compiler = external_tools.join("compiler");
        fs::write(&compiler, b"external compiler bytes").unwrap();
        fs::set_permissions(&compiler, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&external_tools, build.join("host-tools")).unwrap();
        for name in CONFIGURATION_FILES {
            fs::write(build.join(name), b"fixture configuration").unwrap();
        }
        let candidate = build.join("host-tools/compiler");
        fs::write(
            build.join("CMakeCache.txt"),
            format!("AROS_HOST_CC:FILEPATH={}\n", candidate.display()),
        )
        .unwrap();

        verify_configured_tree(&build, true).unwrap();
        let captured = ConfiguredTree::capture(&build).unwrap();
        assert_eq!(
            captured.programs["AROS_HOST_CC"].resolved_path,
            compiler.canonicalize().unwrap().to_str().unwrap()
        );
        fs::write(&compiler, b"mutated external compiler").unwrap();
        assert!(verify_configured_tree(&build, false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn local_native_configuration_excludes_only_missing_generated_programs() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let build = temp.path().join("build");
        fs::create_dir_all(build.join("CMakeFiles")).unwrap();
        for name in CONFIGURATION_FILES {
            fs::write(build.join(name), b"fixture configuration").unwrap();
        }
        let generated = build.join("generated/compiler");
        fs::write(
            build.join("CMakeCache.txt"),
            format!("CMAKE_C_COMPILER:FILEPATH={}\n", generated.display()),
        )
        .unwrap();
        let captured = ConfiguredTree::capture(&build).unwrap();
        assert!(!captured.programs.contains_key("CMAKE_C_COMPILER"));

        let external = temp.path().join("external-tools");
        fs::create_dir(&external).unwrap();
        symlink(&external, build.join("external-tools")).unwrap();
        fs::write(
            build.join("CMakeCache.txt"),
            format!(
                "CMAKE_C_COMPILER:FILEPATH={}\n",
                build.join("external-tools/compiler").display()
            ),
        )
        .unwrap();
        assert!(ConfiguredTree::capture(&build).is_err());
    }

    #[test]
    fn local_native_configuration_binds_only_configured_board_input_members() {
        let temp = tempfile::tempdir().unwrap();
        let build = temp.path().join("build");
        let external = temp.path().join("board-inputs");
        let objects = external.join("kobjs");
        fs::create_dir_all(&objects).unwrap();
        let dtb = external.join("bcm2711-rpi-4-b.dtb");
        fs::write(&dtb, b"device tree bytes").unwrap();
        for name in KOBJ_MEMBERS {
            fs::write(objects.join(name), format!("{name} contents")).unwrap();
        }
        let unrelated = objects.join("unconsumed.o");
        fs::write(&unrelated, b"not consumed by these CMake edges").unwrap();
        write_configuration_fixture(
            &build,
            &format!(
                "AROS_RPI_DTB:FILEPATH={}\nAROS_RPI_CORE_KOBJ_DIR:PATH={}\nOTHER_SEARCH_PATH:PATH={}\n",
                dtb.display(),
                objects.display(),
                external.display(),
            ),
        );

        let original = ConfiguredTree::capture(&build).unwrap();
        assert!(original.board_file_inputs.contains_key("AROS_RPI_DTB"));
        let kobj = &original.board_object_directories["AROS_RPI_CORE_KOBJ_DIR"];
        assert!(kobj.is_directory);
        assert_eq!(kobj.members.len(), KOBJ_MEMBERS.len());
        assert!(!original
            .board_object_directories
            .contains_key("OTHER_SEARCH_PATH"));

        fs::write(
            &unrelated,
            b"sibling changes do not widen the closed input set",
        )
        .unwrap();
        assert!(original == ConfiguredTree::capture(&build).unwrap());

        fs::write(&dtb, b"changed device tree bytes").unwrap();
        assert!(original != ConfiguredTree::capture(&build).unwrap());
        fs::write(&dtb, b"device tree bytes").unwrap();
        fs::write(objects.join("task_resource.o"), b"changed terminal object").unwrap();
        assert!(original != ConfiguredTree::capture(&build).unwrap());
    }

    #[test]
    fn local_native_configuration_binds_configured_board_inputs_inside_build() {
        let temp = tempfile::tempdir().unwrap();
        let build = temp.path().join("build");
        let dtb = build.join("board-inputs/bcm2711-rpi-4-b.dtb");
        let objects = build.join("board-inputs/kobjs");
        fs::create_dir_all(&objects).unwrap();
        fs::write(&dtb, b"configured in-build device tree").unwrap();
        for name in KOBJ_MEMBERS {
            fs::write(objects.join(name), format!("in-build {name} contents")).unwrap();
        }
        write_configuration_fixture(
            &build,
            &format!(
                "AROS_RPI_DTB:FILEPATH={}\nAROS_RPI_CORE_KOBJ_DIR:PATH={}\n",
                dtb.display(),
                objects.display(),
            ),
        );

        verify_configured_tree(&build, true).unwrap();
        let original = ConfiguredTree::capture(&build).unwrap();
        assert!(original.board_file_inputs.contains_key("AROS_RPI_DTB"));
        assert!(original.board_object_directories["AROS_RPI_CORE_KOBJ_DIR"]
            .members
            .values()
            .all(|member| member.internal_to_build && member.sha256.is_some()));

        fs::write(&dtb, b"mutated in-build device tree").unwrap();
        assert!(verify_configured_tree(&build, false).is_err());
        fs::write(&dtb, b"configured in-build device tree").unwrap();
        verify_configured_tree(&build, false).unwrap();

        fs::write(objects.join("task_resource.o"), b"mutated in-build KOBJ").unwrap();
        assert!(verify_configured_tree(&build, false).is_err());
    }

    #[test]
    #[ignore = "requires explicitly installed CMake and Ninja; configuration admission only"]
    fn local_native_configuration_accepts_actual_cmake_ninja_and_refuses_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let build = temp.path().join("build");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("CMakeLists.txt"), b"cmake_minimum_required(VERSION 3.20)\nproject(ConfigurationIdentity NONE)\nadd_custom_target(probe ALL COMMAND \"${CMAKE_COMMAND}\" -E touch \"${CMAKE_BINARY_DIR}/probe.out\")\n").unwrap();
        let configure = Command::new("cmake")
            .arg("-S")
            .arg(&source)
            .arg("-B")
            .arg(&build)
            .args(["-G", "Ninja"])
            .output()
            .unwrap();
        assert!(
            configure.status.success(),
            "{}",
            String::from_utf8_lossy(&configure.stderr)
        );
        verify_configured_tree(&build, true).unwrap();
        let execute = Command::new("cmake")
            .arg("--build")
            .arg(&build)
            .output()
            .unwrap();
        assert!(
            execute.status.success(),
            "{}",
            String::from_utf8_lossy(&execute.stderr)
        );
        assert!(build.join("probe.out").is_file());
        verify_configured_tree(&build, false).unwrap();
        let rules = build.join("CMakeFiles/rules.ninja");
        let before = fs::read(&rules).unwrap();
        fs::write(&rules, [before.as_slice(), b"\n# changed rules\n"].concat()).unwrap();
        assert!(verify_configured_tree(&build, false).is_err());
    }
}
