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

/// Bounds on the configured build tree's text. The ESP32-P4 graph's
/// `build.ninja` alone is 55 MiB, so the per-file bound is several times that
/// and the total allows the few other included manifests beside it.
const MAX_CONFIGURATION_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CONFIGURATION_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

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
            let (_, bytes) = aros_common::measure_regular_file_bounded(
                &build.join(&name),
                MAX_CONFIGURATION_FILE_BYTES,
            )
            .into_diagnostic()?
            .ok_or_else(|| miette::miette!("local native configuration file is missing: {name}"))?;
            total_bytes += bytes.len() as u64;
            if total_bytes > MAX_CONFIGURATION_TOTAL_BYTES {
                miette::bail!("local native configuration exceeds 256 MiB");
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
                                _ => {
                                    miette::bail!(
                                        "configured board input path has an unsafe component"
                                    );
                                }
                            }
                        }
                        return Ok((resolved, false));
                    }
                    Err(error) => return Err(error).into_diagnostic(),
                }
            }
            std::path::Component::CurDir => {}
            _ => {
                miette::bail!("configured board input path has an unsafe component");
            }
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
                    miette::bail!(
                        "local native configuration requires literal Ninja include paths"
                    );
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
#[path = "local_identity_tests.rs"]
mod tests;
