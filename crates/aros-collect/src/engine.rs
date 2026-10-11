//! Shared collection engine for direct links and compiler-driver aliases.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{bail, Context, Result};
use aros_common::elf::{Binding, Home, Object};
use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage};

use crate::observability::{failure, CollectorFailure, CollectorResult, LogLevel, Logger};
use crate::{extra, libreq, sets};

#[cfg(test)]
mod driver_output_tests;
mod driver_tools;

use driver_tools::{resolve_driver_tools, validate_emulation, TOOL_MANIFEST_NAME};

const DRIVER_NAMES: &[&str] = &["collect-aros", "collect-aros32"];
const RESPONSE_DEPTH_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkMode {
    Final,
    Incremental,
    CollectRelocatable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frontend {
    Direct,
    Driver,
}

impl Frontend {
    const fn is_driver(self) -> bool {
        matches!(self, Self::Driver)
    }

    const fn skips_empty_second_pass(self) -> bool {
        matches!(self, Self::Direct)
    }
}

#[derive(Debug)]
struct EngineRequest {
    name: String,
    linker: PathBuf,
    strip: Option<PathBuf>,
    emulation: Option<String>,
    args: Vec<OsString>,
    output: PathBuf,
    sysroot: Option<PathBuf>,
    mode: LinkMode,
    strip_output: bool,
    ignore_undefined: bool,
    report: Option<PathBuf>,
    keep_script: Option<PathBuf>,
    frontend: Frontend,
}

#[derive(Debug)]
struct UserEmulation {
    start: usize,
    end: usize,
    value: String,
}

#[must_use]
pub fn is_driver_invocation(argument_zero: Option<&OsStr>) -> bool {
    argument_zero
        .and_then(|argument| Path::new(argument).file_stem())
        .and_then(OsStr::to_str)
        .is_some_and(is_driver_name)
}

fn is_driver_name(name: &str) -> bool {
    DRIVER_NAMES.contains(&name) || name.ends_with("-collect-aros")
}

fn is_legacy_driver_name(name: &str) -> bool {
    DRIVER_NAMES.contains(&name)
}

pub fn run_entry(
    arguments: impl IntoIterator<Item = OsString>,
    logger: &Logger,
    diagnostics: &mut Vec<Diagnostic>,
) -> CollectorResult<()> {
    let mut arguments = arguments.into_iter();
    let argument_zero = arguments.next().ok_or_else(|| {
        failure(
            DiagnosticCode::CollectorInvocation,
            DiagnosticStage::Invocation,
            "missing collector program name",
            DiagnosticContext::default(),
        )
    })?;
    let name = Path::new(&argument_zero)
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            failure(
                DiagnosticCode::CollectorInvocation,
                DiagnosticStage::Invocation,
                "collector program name is not valid UTF-8",
                DiagnosticContext::default(),
            )
        })?
        .to_owned();
    let raw: Vec<OsString> = arguments.collect();
    if raw
        .iter()
        .any(|argument| argument == "--help" || argument == "-help")
    {
        aros_common::outputln!(
            "{name}: AROS linker collector\n\
             usage: {name} [collector observability options] \
             [linker arguments including --sysroot=DIR and -o FILE]\n\
             configured GNU drivers default to a.out when -o is omitted\n\
             observability:\n  \
             --diagnostic-format human|json\n  \
             --log-level off|error|warn|info|debug|trace\n  \
             --log-format human|jsonl\n  \
             --log-file PATH\n\
             environment: AROS_COLLECT_DIAGNOSTIC_FORMAT, AROS_COLLECT_LOG_LEVEL, \
             AROS_COLLECT_LOG_FORMAT, AROS_COLLECT_LOG_FILE\n\
             logging is off by default and writes only to the selected local file"
        );
        return Ok(());
    }
    if raw.iter().any(|argument| argument == "--version") {
        aros_common::outputln!("{name} {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let executable = std::env::current_exe().map_err(|error| {
        failure(
            DiagnosticCode::CollectorToolResolution,
            DiagnosticStage::ToolResolution,
            format!("cannot locate the running collector: {error}"),
            DiagnosticContext::default(),
        )
    })?;
    let executable = fs::canonicalize(&executable).map_err(|error| {
        failure(
            DiagnosticCode::CollectorToolResolution,
            DiagnosticStage::ToolResolution,
            format!("cannot resolve the running collector executable: {error}"),
            DiagnosticContext::default(),
        )
    })?;
    let bin = executable.parent().ok_or_else(|| {
        failure(
            DiagnosticCode::CollectorToolResolution,
            DiagnosticStage::ToolResolution,
            "the collector executable has no parent directory",
            DiagnosticContext::default(),
        )
    })?;
    let invocation_filename = executable
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            failure(
                DiagnosticCode::CollectorToolResolution,
                DiagnosticStage::ToolResolution,
                "the collector executable filename is not valid UTF-8",
                DiagnosticContext::default(),
            )
        })?;
    let invocation_stem = executable
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            failure(
                DiagnosticCode::CollectorToolResolution,
                DiagnosticStage::ToolResolution,
                "the collector executable name is not valid UTF-8",
                DiagnosticContext::default(),
            )
        })?;
    if raw.is_empty()
        && is_legacy_driver_name(&name)
        && (is_legacy_driver_name(invocation_stem) || invocation_stem == "aros-collect")
        && matches!(
            fs::symlink_metadata(bin.join(TOOL_MANIFEST_NAME)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        )
    {
        return Err(failure(
            DiagnosticCode::CollectorInvocation,
            DiagnosticStage::Invocation,
            "no linker command line was given",
            DiagnosticContext::default(),
        ));
    }
    let tools = resolve_driver_tools(bin, invocation_filename, invocation_stem, &name).map_err(
        |error| {
            failure(
                DiagnosticCode::CollectorToolResolution,
                DiagnosticStage::ToolResolution,
                format!("{error:#}"),
                DiagnosticContext {
                    tool: Some(bin.join(TOOL_MANIFEST_NAME).display().to_string()),
                    ..DiagnosticContext::default()
                },
            )
        },
    )?;
    let args = expand_response_files(&raw, 0).map_err(|error| {
        failure(
            DiagnosticCode::CollectorResponseFile,
            DiagnosticStage::ResponseExpansion,
            format!("{error:#}"),
            DiagnosticContext::default(),
        )
    })?;
    let request = parse_configured(
        name,
        tools.linker,
        tools.strip,
        tools.emulation,
        tools.driver_emulation.as_deref(),
        tools.default_output.as_deref(),
        args,
    )
    .map_err(|error| {
        failure(
            DiagnosticCode::CollectorInvocation,
            DiagnosticStage::Invocation,
            format!("{error:#}"),
            DiagnosticContext::default(),
        )
    })?;
    validate_sysroot(&request).map_err(|error| {
        failure(
            DiagnosticCode::CollectorSysroot,
            DiagnosticStage::SysrootValidation,
            format!("{error:#}"),
            request_context(&request),
        )
    })?;
    run(&request, logger, diagnostics)
}

pub fn run_direct(
    linker: PathBuf,
    args: Vec<OsString>,
    output: PathBuf,
    report: Option<PathBuf>,
    keep_script: Option<PathBuf>,
    logger: &Logger,
    diagnostics: &mut Vec<Diagnostic>,
) -> CollectorResult<()> {
    let request = EngineRequest {
        name: "aros-collect".into(),
        linker,
        strip: None,
        emulation: None,
        args,
        output,
        sysroot: None,
        mode: LinkMode::CollectRelocatable,
        strip_output: false,
        ignore_undefined: true,
        report,
        keep_script,
        frontend: Frontend::Direct,
    };
    run(&request, logger, diagnostics)
}

#[cfg(test)]
fn parse(
    name: String,
    linker: PathBuf,
    strip: PathBuf,
    emulation: Option<String>,
    args: Vec<OsString>,
) -> Result<EngineRequest> {
    parse_configured(name, linker, strip, emulation, None, None, args)
}

fn parse_configured(
    name: String,
    linker: PathBuf,
    strip: PathBuf,
    emulation: Option<String>,
    driver_emulation: Option<&str>,
    default_output: Option<&Path>,
    mut args: Vec<OsString>,
) -> Result<EngineRequest> {
    if let Some(emulation) = &emulation {
        validate_emulation(emulation)?;
        if let Some(driver_emulation) = driver_emulation {
            validate_emulation(driver_emulation)?;
        }
        let found = collect_user_emulations(&args)?;
        if found.iter().any(|selection| {
            selection.value != *emulation
                && driver_emulation.is_none_or(|driver| selection.value != driver)
        }) {
            bail!(
                "conflicting emulation: linker command line selects a value outside the configured linker and driver values"
            );
        }
        let mut normalized = Vec::with_capacity(args.len() + 2);
        let mut selections = found.into_iter().peekable();
        let mut index = 0;
        while index < args.len() {
            if selections
                .peek()
                .is_some_and(|selection| selection.start == index)
            {
                index = selections.next().expect("peeked selection").end;
            } else {
                normalized.push(args[index].clone());
                index += 1;
            }
        }
        normalized.splice(0..0, [OsString::from("-m"), OsString::from(emulation)]);
        args = normalized;
    } else if driver_emulation.is_some() {
        bail!("driver_emulation requires a configured linker emulation");
    }
    let mut output = None;
    let mut sysroot = None;
    let mut mode = LinkMode::Final;
    let mut strip_output = false;
    let mut ignore_undefined = false;
    let mut index = 0;
    while index < args.len() {
        let text = args[index].to_string_lossy();
        if text == "--" {
            break;
        }
        if text == "-o" || text == "--output" {
            if output.is_some() {
                bail!("linker command line specifies output more than once");
            }
            let value = args
                .get(index + 1)
                .with_context(|| format!("linker command line ends after {text}"))?;
            if value.is_empty() {
                bail!("{text} must not be empty");
            }
            output = Some(PathBuf::from(value));
            index += 2;
            continue;
        }
        if let Some(value) = text
            .strip_prefix("--output=")
            .or_else(|| text.strip_prefix("-o").filter(|value| !value.is_empty()))
        {
            if output.is_some() {
                bail!("linker command line specifies output more than once");
            }
            if value.is_empty() {
                bail!("--output must not be empty");
            }
            output = Some(PathBuf::from(value));
        } else if text == "--sysroot" {
            sysroot = Some(PathBuf::from(
                args.get(index + 1)
                    .context("linker command line ends after --sysroot")?,
            ));
            index += 2;
            continue;
        } else if let Some(value) = text.strip_prefix("--sysroot=") {
            if value.is_empty() {
                bail!("--sysroot must not be empty");
            }
            sysroot = Some(PathBuf::from(value));
        } else if text == "-r" || text == "-i" {
            mode = LinkMode::Incremental;
        } else if text == "-Ur" {
            mode = LinkMode::CollectRelocatable;
            args[index] = OsString::from("-r");
        } else if text == "-ius" {
            ignore_undefined = true;
            args[index] = OsString::from("-r");
        } else if text == "-s" {
            strip_output = true;
            args[index] = OsString::from("-r");
        } else if text.starts_with("--ld-path") || text.starts_with("-Wl,--ld-path") {
            bail!(
                "the collector does not permit a linker override; it requires its configured sibling linker"
            );
        } else if text == "-m" || text == "--emulation" {
            // Configured emulation selections have already been normalized at
            // the start of the argument list. Unconfigured legacy invocations
            // still pass their user's selection through to the linker.
            args.get(index + 1)
                .with_context(|| format!("linker command line ends after {text}"))?;
            index += 2;
            continue;
        } else if text.starts_with("--emulation=") {
            // See the separated spelling above; this option has no collector
            // interpretation beyond emulation validation.
        } else if takes_separate_value(&text) {
            args.get(index + 1)
                .with_context(|| format!("linker command line ends after {text}"))?;
            index += 2;
            continue;
        }
        index += 1;
    }

    let output = if let Some(output) = output {
        output
    } else {
        let output = default_output.context("linker command line has no -o FILE")?;
        // Normalize GNU ld's standard default before the operand boundary.
        // Every pass must still use an explicit adjacent staging path.
        args.splice(
            index..index,
            [OsString::from("-o"), output.as_os_str().to_owned()],
        );
        output.to_path_buf()
    };
    Ok(EngineRequest {
        name,
        linker,
        strip: Some(strip),
        emulation,
        args,
        output,
        sysroot,
        mode,
        strip_output,
        ignore_undefined,
        report: None,
        keep_script: None,
        frontend: Frontend::Driver,
    })
}

fn collect_user_emulations(args: &[OsString]) -> Result<Vec<UserEmulation>> {
    let mut found = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let text = args[index].to_string_lossy();
        if text == "--" {
            break;
        }
        if takes_separate_value(&text) {
            if index + 1 >= args.len() {
                bail!("linker command line ends after value-taking option '{text}'");
            }
            index += 2;
            continue;
        }
        if has_attached_value(&text) {
            index += 1;
            continue;
        }
        if text == "-m" || text == "--emulation" {
            let value = args
                .get(index + 1)
                .context("linker command line ends after an emulation option")?
                .to_str()
                .context("linker emulation is not valid UTF-8")?;
            validate_emulation(value)?;
            found.push(UserEmulation {
                start: index,
                end: index + 2,
                value: value.to_owned(),
            });
            index += 2;
            continue;
        }
        if let Some(value) = text.strip_prefix("--emulation=") {
            validate_emulation(value)?;
            found.push(UserEmulation {
                start: index,
                end: index + 1,
                value: value.to_owned(),
            });
        } else if text.starts_with("--emu") {
            bail!("unsupported abbreviated or malformed linker emulation option '{text}'");
        } else if let Some(value) = text.strip_prefix("-m") {
            validate_emulation(value)
                .with_context(|| format!("malformed attached linker emulation option '{text}'"))?;
            found.push(UserEmulation {
                start: index,
                end: index + 1,
                value: value.to_owned(),
            });
        } else if text.starts_with('-')
            && !is_known_flag(&text)
            && args.get(index + 1).is_some_and(|next| {
                next.to_string_lossy().starts_with("-m")
                    || next.to_string_lossy().starts_with("--emu")
            })
        {
            bail!(
                "ambiguous linker option '{text}' precedes a possible emulation operand; refusing to normalize it"
            );
        }
        index += 1;
    }
    Ok(found)
}

fn takes_separate_value(option: &str) -> bool {
    matches!(
        option,
        "-a" | "-A"
            | "--architecture"
            | "-b"
            | "--format"
            | "-c"
            | "--mri-script"
            | "--dependency-file"
            | "-e"
            | "--entry"
            | "-f"
            | "--auxiliary"
            | "-F"
            | "--filter"
            | "-G"
            | "--gpsize"
            | "-h"
            | "-soname"
            | "-I"
            | "--dynamic-linker"
            | "-l"
            | "--library"
            | "-L"
            | "--library-path"
            | "--sysroot"
            | "-o"
            | "--output"
            | "-O"
            | "-plugin"
            | "--plugin"
            | "-plugin-opt"
            | "-R"
            | "--just-symbols"
            | "-rpath"
            | "-rpath-link"
            | "-T"
            | "--script"
            | "--default-script"
            | "-dT"
            | "--oformat"
            | "-u"
            | "--undefined"
            | "--require-defined"
            | "-y"
            | "--trace-symbol"
            | "-Y"
            | "-assert"
            | "--defsym"
            | "-fini"
            | "-init"
            | "--dynamic-list"
            | "--export-dynamic-symbol"
            | "--export-dynamic-symbol-list"
            | "-Map"
            | "--Map"
            | "--section-ordering-file"
            | "--retain-symbols-file"
            | "--image-base"
            | "--section-start"
            | "-Tbss"
            | "-Tdata"
            | "-Ttext"
            | "-Ttext-segment"
            | "-Trodata-segment"
            | "-Tldata-segment"
            | "-P"
            | "--depaudit"
            | "--audit"
            | "-z"
            | "--sort-section"
            | "--spare-dynamic-tags"
            | "--error-handling-script"
            | "--version-script"
            | "--exclude-libs"
            | "--unresolved-symbols"
            | "--out-implib"
            | "--remap-inputs-file"
            | "--remap-inputs"
            | "--orphan-handling"
            | "--task-link"
            | "--wrap"
            | "--ignore-unresolved-symbol"
            | "--version-exports-section"
    )
}

fn has_attached_value(option: &str) -> bool {
    if option.starts_with("--") {
        return option.contains('=') && !option.starts_with("--emu");
    }
    [
        "-o", "-L", "-l", "-T", "-A", "-b", "-e", "-F", "-G", "-h", "-I", "-R", "-u", "-y", "-Y",
        "-a", "-c", "-f", "-dT", "-Map",
    ]
    .iter()
    .any(|prefix| option.starts_with(prefix) && option.len() > prefix.len())
}

fn is_known_flag(option: &str) -> bool {
    matches!(
        option,
        "-r" | "--relocatable"
            | "-i"
            | "-Ur"
            | "-d"
            | "-dc"
            | "-dp"
            | "-E"
            | "--export-dynamic"
            | "--no-export-dynamic"
            | "--force-group-allocation"
            | "--enable-non-contiguous-regions"
            | "--enable-non-contiguous-regions-warnings"
            | "--disable-linker-version"
            | "--enable-linker-version"
            | "-EB"
            | "-EL"
            | "--no-dynamic-linker"
            | "-M"
            | "--print-map"
            | "--plugin-save-temps"
            | "-flto"
            | "--map-whole-files"
            | "--no-map-whole-files"
            | "-Qy"
            | "-shared"
            | "--shared"
            | "-Bshareable"
            | "-Bdynamic"
            | "-Bstatic"
            | "-dn"
            | "-dy"
            | "-call_shared"
            | "-non_shared"
            | "-static"
            | "-pie"
            | "-no-pie"
            | "-n"
            | "-N"
            | "--nmagic"
            | "--omagic"
            | "-q"
            | "--emit-relocs"
            | "-s"
            | "-S"
            | "--strip-all"
            | "--strip-debug"
            | "--strip-discarded"
            | "--no-strip-discarded"
            | "-x"
            | "--discard-all"
            | "-X"
            | "--discard-locals"
            | "-g"
            | "-t"
            | "--trace"
            | "-v"
            | "--version"
            | "-V"
            | "--verbose"
            | "--fatal-warnings"
            | "--no-fatal-warnings"
            | "--no-warnings"
            | "-w"
            | "--warn-common"
            | "--no-warn-common"
            | "--warn-once"
            | "--warn-section-align"
            | "--no-undefined"
            | "--as-needed"
            | "--no-as-needed"
            | "--whole-archive"
            | "--no-whole-archive"
            | "--start-group"
            | "--end-group"
            | "--start-lib"
            | "--end-lib"
            | "--accept-unknown-input-arch"
            | "--no-accept-unknown-input-arch"
            | "--gc-sections"
            | "--no-gc-sections"
            | "--print-gc-sections"
            | "--no-print-gc-sections"
            | "--build-id"
            | "--no-build-id"
            | "--allow-shlib-undefined"
            | "--no-allow-shlib-undefined"
            | "--undefined-version"
            | "--no-undefined-version"
            | "--default-symver"
            | "--default-imported-symver"
            | "--no-warn-mismatch"
            | "--no-warn-search-mismatch"
            | "--noinhibit-exec"
            | "-nostdlib"
            | "--reduce-memory-overheads"
            | "--relax"
            | "--no-relax"
            | "--enable-new-dtags"
            | "--disable-new-dtags"
            | "--no-keep-memory"
            | "--check-sections"
            | "--no-check-sections"
            | "--copy-dt-needed-entries"
            | "--no-copy-dt-needed-entries"
            | "--cref"
            | "--demangle"
            | "--no-demangle"
            | "--disable-multiple-abs-defs"
            | "--embedded-relocs"
            | "--force-exe-suffix"
            | "--no-define-common"
            | "--link-mapless"
            | "--no-link-mapless"
            | "--print-output-format"
            | "--print-sysroot"
            | "-qmagic"
            | "--target-help"
            | "--traditional-format"
            | "--stats"
            | "--no-stats"
            | "--print-memory-usage"
            | "--no-eh-frame-hdr"
            | "--eh-frame-hdr"
            | "--rosegment"
            | "--no-rosegment"
            | "--dynamic-list-data"
            | "--dynamic-list-cpp-new"
            | "--dynamic-list-cpp-typeinfo"
            | "--warn-textrel"
            | "--error-execstack"
            | "--no-error-execstack"
            | "--warn-execstack-objects"
            | "--warn-execstack"
            | "--no-warn-execstack"
            | "--warn-rwx-segments"
            | "--no-warn-rwx-segments"
            | "--error-rwx-segments"
            | "--no-error-rwx-segments"
            | "--warn-multiple-gp"
            | "--warn-alternate-em"
            | "--warn-unresolved-symbols"
            | "--error-unresolved-symbols"
            | "--print-map-discarded"
            | "--no-print-map-discarded"
            | "--print-map-locals"
            | "--no-print-map-locals"
            | "--ctf-variables"
            | "--no-ctf-variables"
            | "-Bsymbolic"
            | "-Bsymbolic-functions"
            | "-Bsymbolic-non-weak"
            | "--allow-multiple-definition"
    )
}

fn validate_sysroot(request: &EngineRequest) -> Result<()> {
    if let Some(root) = &request.sysroot {
        if !root.is_absolute() {
            bail!("--sysroot must be absolute, got {}", root.display());
        }
        // Compiler drivers can suppress default libraries without forwarding
        // those driver flags here. Validate collector-added files only when a
        // discovered requirement actually needs one.
    }
    Ok(())
}

fn run(
    request: &EngineRequest,
    logger: &Logger,
    diagnostics: &mut Vec<Diagnostic>,
) -> CollectorResult<()> {
    run_with(request, logger, diagnostics, run_tool)
}

fn run_with<F>(
    request: &EngineRequest,
    logger: &Logger,
    diagnostics: &mut Vec<Diagnostic>,
    mut execute: F,
) -> CollectorResult<()>
where
    F: FnMut(&Path, &[OsString]) -> Result<ExitStatus>,
{
    let staged = adjacent(&request.output, ".collect-pre");
    let final_staged = adjacent(&request.output, ".collect-final");
    let script = request
        .keep_script
        .clone()
        .unwrap_or_else(|| adjacent(&request.output, ".collect-sets.ld"));
    for path in [&staged, &final_staged] {
        remove_if_exists(path).map_err(|error| {
            failure(
                DiagnosticCode::CollectorPublication,
                DiagnosticStage::Publication,
                format!("{error:#}"),
                request_context(request),
            )
        })?;
    }
    if request.keep_script.is_none() {
        remove_if_exists(&script).map_err(|error| {
            failure(
                DiagnosticCode::CollectorPublication,
                DiagnosticStage::Publication,
                format!("{error:#}"),
                request_context(request),
            )
        })?;
    }
    let mut cleanup_paths = vec![staged.clone(), final_staged.clone()];
    if request.keep_script.is_none() {
        cleanup_paths.push(script.clone());
    }
    let cleanup = Cleanup::new(cleanup_paths);

    let mut first = replace_output(&request.args, &staged).map_err(|error| {
        failure(
            DiagnosticCode::CollectorInvocation,
            DiagnosticStage::Invocation,
            format!("{error:#}"),
            request_context(request),
        )
    })?;
    if request.frontend.is_driver() && !first.iter().any(|argument| argument == "-r") {
        first.insert(0, OsString::from("-r"));
    }
    logger.event(
        LogLevel::Debug,
        "link.first.start",
        "starting first relocatable link",
        &request_context(request),
    )?;
    let status = execute(&request.linker, &first).map_err(|error| {
        failure(
            DiagnosticCode::CollectorFirstLink,
            DiagnosticStage::FirstLink,
            format!("{error:#}"),
            request_context(request),
        )
    })?;
    if !status.success() {
        return Err(process_failure(
            DiagnosticCode::CollectorFirstLink,
            DiagnosticStage::FirstLink,
            "the first relocatable link failed",
            status,
            request_context(request),
        ));
    }
    if request.mode == LinkMode::Incremental {
        if request.frontend.is_driver() {
            set_aros_abi(&staged).map_err(|error| {
                failure(
                    DiagnosticCode::CollectorAbi,
                    DiagnosticStage::AbiMarking,
                    format!("{error:#}"),
                    request_context(request),
                )
            })?;
        }
        publish(&staged, &request.output).map_err(|error| {
            failure(
                DiagnosticCode::CollectorPublication,
                DiagnosticStage::Publication,
                format!("{error:#}"),
                request_context(request),
            )
        })?;
        return Ok(());
    }

    let object = read_object(&staged).map_err(|error| {
        failure(
            DiagnosticCode::CollectorObjectInspection,
            DiagnosticStage::ObjectInspection,
            format!("{error:#}"),
            DiagnosticContext {
                output: Some(staged.display().to_string()),
                ..request_context(request)
            },
        )
    })?;
    let section_names = object.section_names();
    let (found, mut reported) = sets::discover(&section_names);
    let (requirements, libreq_reported) = libreq::discover(&object.symbols);
    reported.extend(libreq_reported);
    if !reported.is_empty() {
        for line in &reported {
            let diagnostic = Diagnostic::warning(
                DiagnosticCode::CollectorSetCollection,
                DiagnosticStage::SetCollection,
                line.clone(),
            )
            .with_context(request_context(request));
            logger.diagnostic(&diagnostic)?;
            diagnostics.push(diagnostic);
        }
        logger.event(
            LogLevel::Warn,
            "collection.skipped",
            &format!(
                "{} set or library requirement entries were skipped",
                reported.len()
            ),
            &request_context(request),
        )?;
    }
    write_report(request.report.as_deref(), &reported).map_err(|error| {
        failure(
            DiagnosticCode::CollectorSetCollection,
            DiagnosticStage::SetCollection,
            format!("{error:#}"),
            DiagnosticContext {
                output: request
                    .report
                    .as_ref()
                    .map(|path| path.display().to_string()),
                ..request_context(request)
            },
        )
    })?;

    if request.frontend.skips_empty_second_pass() && found.is_empty() && requirements.is_empty() {
        publish(&staged, &request.output).map_err(|error| {
            failure(
                DiagnosticCode::CollectorPublication,
                DiagnosticStage::Publication,
                format!("{error:#}"),
                request_context(request),
            )
        })?;
        return Ok(());
    }

    let script_body = sets::script(&found, object.class, &libreq::script(&requirements));
    fs::write(&script, script_body).map_err(|error| {
        failure(
            DiagnosticCode::CollectorSetCollection,
            DiagnosticStage::SetCollection,
            format!(
                "cannot write collector script {}: {error}",
                script.display()
            ),
            request_context(request),
        )
    })?;

    let extras = request
        .frontend
        .is_driver()
        .then(|| extra::discover(&object.symbols));
    let mut second = vec![OsString::from("-r")];
    if let Some(emulation) = &request.emulation {
        second.push(OsString::from("-m"));
        second.push(OsString::from(emulation));
    }
    second.extend([
        OsString::from("-o"),
        final_staged.clone().into_os_string(),
        staged.into_os_string(),
    ]);
    if extras
        .as_ref()
        .is_some_and(|extras| extras.cxx_pure_virtual)
    {
        second.push(
            require_sysroot_library(request, "static-cxx-cxa-pure-virtual.o")
                .map_err(|error| required_input_failure(request, &error))?
                .into_os_string(),
        );
    }
    if extras.as_ref().is_some_and(|extras| extras.pthread) {
        second.push(
            require_sysroot_library(request, "libpthread.a")
                .map_err(|error| required_input_failure(request, &error))?
                .into_os_string(),
        );
    }
    if request.frontend.is_driver() && has_undefined(&object) {
        second.extend(resupplied_libraries(&request.args));
    }
    second.push(OsString::from("-T"));
    second.push(script.into_os_string());
    logger.event(
        LogLevel::Debug,
        "link.second.start",
        "starting set-collection link",
        &request_context(request),
    )?;
    let status = execute(&request.linker, &second).map_err(|error| {
        failure(
            DiagnosticCode::CollectorSecondLink,
            DiagnosticStage::SecondLink,
            format!("{error:#}"),
            request_context(request),
        )
    })?;
    if !status.success() {
        return Err(process_failure(
            DiagnosticCode::CollectorSecondLink,
            DiagnosticStage::SecondLink,
            "the set-collection link failed",
            status,
            request_context(request),
        ));
    }

    if request.frontend.is_driver() && request.mode == LinkMode::Final && !request.ignore_undefined
    {
        let output = read_object(&final_staged).map_err(|error| {
            failure(
                DiagnosticCode::CollectorObjectInspection,
                DiagnosticStage::ObjectInspection,
                format!("{error:#}"),
                request_context(request),
            )
        })?;
        let undefined = undefined_names(&output);
        if !undefined.is_empty() {
            return Err(failure(
                DiagnosticCode::CollectorUndefinedSymbols,
                DiagnosticStage::UndefinedAudit,
                format!(
                    "undefined symbols remain after the final link: {}",
                    undefined.into_iter().collect::<Vec<_>>().join(", ")
                ),
                request_context(request),
            ));
        }
    }
    if request.strip_output {
        let strip = request.strip.as_ref().ok_or_else(|| {
            failure(
                DiagnosticCode::CollectorToolResolution,
                DiagnosticStage::ToolResolution,
                "output stripping was requested without a configured strip tool",
                request_context(request),
            )
        })?;
        let status = execute(
            strip,
            &[
                OsString::from("--strip-unneeded"),
                final_staged.clone().into_os_string(),
            ],
        )
        .map_err(|error| {
            failure(
                DiagnosticCode::CollectorStrip,
                DiagnosticStage::Strip,
                format!("{error:#}"),
                DiagnosticContext {
                    tool: Some(strip.display().to_string()),
                    ..request_context(request)
                },
            )
        })?;
        if !status.success() {
            return Err(process_failure(
                DiagnosticCode::CollectorStrip,
                DiagnosticStage::Strip,
                "stripping the linked object failed",
                status,
                DiagnosticContext {
                    tool: Some(strip.display().to_string()),
                    ..request_context(request)
                },
            ));
        }
    }
    if request.frontend.is_driver() {
        set_aros_abi(&final_staged).map_err(|error| {
            failure(
                DiagnosticCode::CollectorAbi,
                DiagnosticStage::AbiMarking,
                format!("{error:#}"),
                request_context(request),
            )
        })?;
    }
    #[cfg(unix)]
    if request.frontend.is_driver() {
        fs::set_permissions(&final_staged, fs::Permissions::from_mode(0o766)).map_err(|error| {
            failure(
                DiagnosticCode::CollectorPublication,
                DiagnosticStage::Publication,
                format!(
                    "cannot set permissions on {}: {error}",
                    final_staged.display()
                ),
                request_context(request),
            )
        })?;
    }
    publish(&final_staged, &request.output).map_err(|error| {
        failure(
            DiagnosticCode::CollectorPublication,
            DiagnosticStage::Publication,
            format!("{error:#}"),
            request_context(request),
        )
    })?;
    drop(cleanup);
    Ok(())
}

struct Cleanup {
    paths: Vec<PathBuf>,
    keep: bool,
}

impl Cleanup {
    fn new(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            paths: paths.into_iter().collect(),
            keep: std::env::var_os("COLLECT_AROS_DEBUG").is_some(),
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

fn read_object(path: &Path) -> Result<Object> {
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    aros_common::elf::read(&bytes).with_context(|| format!("cannot parse {}", path.display()))
}

fn run_tool(tool: &Path, args: &[OsString]) -> Result<ExitStatus> {
    aros_common::run_status(Command::new(tool).args(args))
        .map(|observed| observed.status)
        .with_context(|| format!("cannot execute required sibling tool {}", tool.display()))
}

fn request_context(request: &EngineRequest) -> DiagnosticContext {
    DiagnosticContext {
        tool: Some(request.linker.display().to_string()),
        mode: Some(
            match request.frontend {
                Frontend::Direct => "direct",
                Frontend::Driver => match request.mode {
                    LinkMode::Final => "final",
                    LinkMode::Incremental => "incremental",
                    LinkMode::CollectRelocatable => "collect_relocatable",
                },
            }
            .into(),
        ),
        output: Some(request.output.display().to_string()),
        ..DiagnosticContext::default()
    }
}

fn process_failure(
    code: DiagnosticCode,
    stage: DiagnosticStage,
    message: impl Into<String>,
    status: ExitStatus,
    mut context: DiagnosticContext,
) -> CollectorFailure {
    context.exit_code = status.code();
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        context.signal = status.signal();
    }
    failure(code, stage, message, context)
}

fn required_input_failure(request: &EngineRequest, error: &anyhow::Error) -> CollectorFailure {
    failure(
        DiagnosticCode::CollectorRequiredInput,
        DiagnosticStage::RequiredInput,
        format!("{error:#}"),
        request_context(request),
    )
}

fn require_library(directory: &Path, name: &str) -> Result<PathBuf> {
    let path = directory.join(name);
    if !path.is_file() {
        bail!(
            "collector-required sysroot input is missing: {}",
            path.display()
        );
    }
    Ok(path)
}

fn require_sysroot_library(request: &EngineRequest, name: &str) -> Result<PathBuf> {
    let root = request.sysroot.as_ref().with_context(|| {
        format!(
            "the first link requires {name}, but the linker command line has no --sysroot; pass an absolute AROS Developer sysroot"
        )
    })?;
    let directory = root.join(if request.name == "collect-aros32" {
        "lib32"
    } else {
        "lib"
    });
    require_library(&directory, name)
}

fn write_report(path: Option<&Path>, lines: &[String]) -> Result<()> {
    let Some(path) = path else { return Ok(()) };
    if lines.is_empty() {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("cannot remove {}", path.display()));
            }
        }
        return Ok(());
    }
    let mut body = lines.join("\n");
    body.push('\n');
    fs::write(path, body).with_context(|| format!("cannot write {}", path.display()))
}

fn has_undefined(object: &Object) -> bool {
    object
        .symbols
        .iter()
        .any(|symbol| symbol.home == Home::Undefined && !symbol.name.is_empty())
}

fn undefined_names(object: &Object) -> BTreeSet<String> {
    object
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.home == Home::Undefined
                && symbol.binding != Binding::Local
                && !symbol.name.is_empty()
        })
        .map(|symbol| symbol.name.clone())
        .collect()
}

fn resupplied_libraries(args: &[OsString]) -> Vec<OsString> {
    let mut supplied = vec![OsString::from("--allow-multiple-definition")];
    let mut index = 0;
    while index < args.len() {
        let text = args[index].to_string_lossy();
        if !text.starts_with('-') && text.ends_with(".a") {
            supplied.push(args[index].clone());
        } else if text == "-L" || text == "-l" {
            if let Some(value) = args.get(index + 1) {
                if text != "-l" || !value.to_string_lossy().starts_with("gcc") {
                    supplied.push(args[index].clone());
                    supplied.push(value.clone());
                }
                index += 1;
            }
        } else if text.starts_with("-L") {
            supplied.push(args[index].clone());
        } else if let Some(name) = text.strip_prefix("-l") {
            if !name.starts_with("gcc") {
                supplied.push(args[index].clone());
            }
        }
        index += 1;
    }
    supplied
}

fn replace_output(args: &[OsString], output: &Path) -> Result<Vec<OsString>> {
    let mut replaced = args.to_vec();
    let mut index = 0;
    while index < replaced.len() {
        let text = replaced[index].to_string_lossy().into_owned();
        if text == "--" {
            break;
        }
        if text == "-o" || text == "--output" {
            let slot = replaced
                .get_mut(index + 1)
                .with_context(|| format!("linker command line ends after {text}"))?;
            output.as_os_str().clone_into(slot);
            return Ok(replaced);
        }
        if text.starts_with("-o") && text.len() > 2 {
            let mut joined = OsString::from("-o");
            joined.push(output);
            replaced[index] = joined;
            return Ok(replaced);
        }
        if text.starts_with("--output=") {
            let mut joined = OsString::from("--output=");
            joined.push(output);
            replaced[index] = joined;
            return Ok(replaced);
        }
        if takes_separate_value(&text) {
            index += 2;
            continue;
        }
        index += 1;
    }
    bail!("linker command line has no -o FILE")
}

fn adjacent(output: &Path, suffix: &str) -> PathBuf {
    let mut value = output.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

fn publish(staged: &Path, output: &Path) -> Result<()> {
    fs::rename(staged, output).with_context(|| {
        format!(
            "cannot publish {} as {}",
            staged.display(),
            output.display()
        )
    })
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("cannot remove {}", path.display())),
    }
}

fn set_aros_abi(path: &Path) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let mut ident = [0_u8; 9];
    file.read_exact(&mut ident)
        .with_context(|| format!("cannot read ELF identity from {}", path.display()))?;
    if ident.get(..4) != Some(b"\x7fELF") {
        bail!(
            "linker output is not a complete ELF file: {}",
            path.display()
        );
    }
    file.seek(SeekFrom::Start(7))
        .with_context(|| format!("cannot seek in {}", path.display()))?;
    file.write_all(&[15, 1])
        .with_context(|| format!("cannot set AROS ABI on {}", path.display()))
}

fn expand_response_files(args: &[OsString], depth: usize) -> Result<Vec<OsString>> {
    if depth >= RESPONSE_DEPTH_LIMIT {
        bail!("response-file nesting exceeds {RESPONSE_DEPTH_LIMIT}");
    }
    let mut expanded = Vec::new();
    for argument in args {
        let text = argument.to_string_lossy();
        let Some(path) = text.strip_prefix('@') else {
            expanded.push(argument.clone());
            continue;
        };
        let body = fs::read_to_string(path)
            .with_context(|| format!("cannot read linker response file {path}"))?;
        let parsed = parse_response(&body)?;
        expanded.extend(expand_response_files(&parsed, depth + 1)?);
    }
    Ok(expanded)
}

fn parse_response(body: &str) -> Result<Vec<OsString>> {
    let mut arguments = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for character in body.chars() {
        if escaped {
            token.push(character);
            escaped = false;
            started = true;
        } else if character == '\\' {
            escaped = true;
            started = true;
        } else if let Some(expected) = quote {
            if character == expected {
                quote = None;
            } else {
                token.push(character);
            }
            started = true;
        } else if character == '\'' || character == '"' {
            quote = Some(character);
            started = true;
        } else if character.is_whitespace() {
            if started {
                arguments.push(OsString::from(std::mem::take(&mut token)));
                started = false;
            }
        } else {
            token.push(character);
            started = true;
        }
    }
    if escaped || quote.is_some() {
        bail!("unterminated escape or quote in linker response file");
    }
    if started {
        arguments.push(OsString::from(token));
    }
    Ok(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// Small ELF64 fixture containing only a section-name table and one named
    /// section. That is sufficient to exercise the real collection engine.
    fn elf64_with_section(section: &str) -> Vec<u8> {
        let mut names = b"\0.shstrtab\0".to_vec();
        let section_name_offset = u32::try_from(names.len()).unwrap();
        names.extend_from_slice(section.as_bytes());
        names.push(0);

        let names_offset = 0x40;
        let section_table_offset = 0x80;
        let mut bytes = vec![0_u8; section_table_offset + 3 * 0x40];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        put_u32(&mut bytes, 0x14, 1);
        put_u16(&mut bytes, 0x34, 64);
        put_u64(&mut bytes, 0x28, section_table_offset as u64);
        put_u16(&mut bytes, 0x3a, 0x40);
        put_u16(&mut bytes, 0x3c, 3);
        put_u16(&mut bytes, 0x3e, 1);
        bytes[names_offset..names_offset + names.len()].copy_from_slice(&names);

        let names_header = section_table_offset + 0x40;
        put_u32(&mut bytes, names_header, 1);
        put_u32(&mut bytes, names_header + 4, 3);
        put_u64(&mut bytes, names_header + 0x18, names_offset as u64);
        put_u64(&mut bytes, names_header + 0x20, names.len() as u64);
        put_u64(&mut bytes, names_header + 0x30, 1);

        let section_header = section_table_offset + 2 * 0x40;
        put_u32(&mut bytes, section_header, section_name_offset);
        put_u32(&mut bytes, section_header + 4, 1);
        put_u64(&mut bytes, section_header + 0x30, 1);
        bytes
    }

    #[cfg(unix)]
    fn test_exit_status(code: i32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;

        ExitStatus::from_raw(code << 8)
    }

    fn output_argument(arguments: &[OsString]) -> PathBuf {
        arguments
            .windows(2)
            .find(|pair| pair[0] == "-o")
            .map(|pair| PathBuf::from(&pair[1]))
            .or_else(|| {
                arguments.iter().find_map(|argument| {
                    argument
                        .to_string_lossy()
                        .strip_prefix("-o")
                        .filter(|value| !value.is_empty())
                        .map(PathBuf::from)
                })
            })
            .expect("linker output argument")
    }

    #[test]
    fn only_expected_aliases_select_driver_mode() {
        assert!(is_driver_invocation(Some(OsStr::new("/tmp/collect-aros"))));
        assert!(is_driver_invocation(Some(OsStr::new("collect-aros32"))));
        assert!(is_driver_invocation(Some(OsStr::new(
            "/opt/cross/bin/riscv-aros-collect-aros"
        ))));
        assert!(!is_driver_invocation(Some(OsStr::new("aros-collect"))));
    }

    #[test]
    fn response_parser_preserves_grouping_and_escapes() {
        let parsed = parse_response("-o 'an output.o' one\\ file.o \"two.o\"").unwrap();
        assert_eq!(
            parsed,
            strings(&["-o", "an output.o", "one file.o", "two.o"])
        );
    }

    #[test]
    fn output_replacement_handles_both_spellings() {
        assert_eq!(
            replace_output(&strings(&["-r", "-o", "old.o"]), Path::new("new.o")).unwrap(),
            strings(&["-r", "-o", "new.o"])
        );
        assert_eq!(
            replace_output(&strings(&["-r", "-oold.o"]), Path::new("new.o")).unwrap(),
            strings(&["-r", "-onew.o"])
        );
    }

    #[test]
    fn library_resupply_omits_compiler_private_archives() {
        let supplied = resupplied_libraries(&strings(&[
            "-L/sysroot/lib",
            "-lfoo",
            "-lgcc",
            "one.a",
            "one.o",
        ]));
        assert_eq!(
            supplied,
            strings(&[
                "--allow-multiple-definition",
                "-L/sysroot/lib",
                "-lfoo",
                "one.a"
            ])
        );
    }

    #[test]
    fn publish_replaces_an_existing_output() {
        let directory = tempfile::tempdir().unwrap();
        let staged = directory.path().join("staged");
        let output = directory.path().join("output");
        fs::write(&staged, b"new").unwrap();
        fs::write(&output, b"old").unwrap();

        publish(&staged, &output).unwrap();

        assert_eq!(fs::read(&output).unwrap(), b"new");
        assert!(!staged.exists());
    }

    #[test]
    fn configured_emulation_is_injected_and_repeated_matching_options_are_safe() {
        let request = parse(
            "riscv-aros-collect-aros".into(),
            "ld.gnu".into(),
            "strip.gnu".into(),
            Some("riscv64elf_aros".into()),
            strings(&[
                "-m",
                "riscv64elf_aros",
                "-mriscv64elf_aros",
                "-o",
                "output.o",
            ]),
        )
        .unwrap();
        assert_eq!(
            collect_user_emulations(&request.args)
                .unwrap()
                .into_iter()
                .map(|selection| selection.value)
                .collect::<Vec<_>>(),
            ["riscv64elf_aros"]
        );
        assert_eq!(
            collect_user_emulations(&strings(&["-m", "riscv64elf_aros", "--", "-m", "other",]))
                .unwrap()
                .into_iter()
                .map(|selection| selection.value)
                .collect::<Vec<_>>(),
            ["riscv64elf_aros"]
        );

        let conflict = parse(
            "riscv-aros-collect-aros".into(),
            "ld.gnu".into(),
            "strip.gnu".into(),
            Some("riscv64elf_aros".into()),
            strings(&["-mriscvelf_aros", "-o", "output.o"]),
        )
        .unwrap_err();
        assert!(format!("{conflict:#}").contains("conflicting emulation"));

        for unsupported in ["--emul=riscvelf_aros", "-mriscv64elf_aros=other"] {
            let error = parse(
                "riscv-aros-collect-aros".into(),
                "ld.gnu".into(),
                "strip.gnu".into(),
                Some("riscv64elf_aros".into()),
                strings(&[unsupported, "-o", "output.o"]),
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains("emulation option"));
        }
    }

    #[test]
    fn declared_driver_emulation_is_normalized_without_an_inferred_mapping() {
        let request = parse_configured(
            "collect-aros".into(),
            "ld".into(),
            "strip".into(),
            Some("riscvelf_aros".into()),
            Some("elf32lriscv"),
            None,
            strings(&["-melf32lriscv", "-m", "riscvelf_aros", "-o", "output.o"]),
        )
        .unwrap();
        assert_eq!(request.emulation.as_deref(), Some("riscvelf_aros"));
        assert_eq!(
            collect_user_emulations(&request.args)
                .unwrap()
                .into_iter()
                .map(|selection| selection.value)
                .collect::<Vec<_>>(),
            ["riscvelf_aros"]
        );

        let inferred = parse_configured(
            "collect-aros".into(),
            "ld".into(),
            "strip".into(),
            Some("riscvelf_aros".into()),
            None,
            None,
            strings(&["-melf32lriscv", "-o", "output.o"]),
        )
        .unwrap_err();
        assert!(format!("{inferred:#}").contains("conflicting emulation"));

        let mismatch = parse_configured(
            "collect-aros".into(),
            "ld".into(),
            "strip".into(),
            Some("riscvelf_aros".into()),
            Some("elf32lriscv"),
            None,
            strings(&["-melf64lriscv", "-o", "output.o"]),
        )
        .unwrap_err();
        assert!(format!("{mismatch:#}").contains("conflicting emulation"));
    }

    #[test]
    fn emulation_scanner_skips_known_operands_and_rejects_ambiguous_options() {
        let parsed = collect_user_emulations(&strings(&[
            "-o",
            "-m-output.o",
            "-T",
            "-m-script.ld",
            "--sysroot",
            "-m-root",
            "--emulation=elf32lriscv",
            "--gc-sections",
        ]))
        .unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].value, "elf32lriscv");

        let ambiguous =
            collect_user_emulations(&strings(&["--unclassified-option", "-melf32lriscv"]))
                .unwrap_err();
        assert!(format!("{ambiguous:#}").contains("ambiguous linker option"));
    }

    #[test]
    fn duplicate_output_spellings_are_rejected_before_a_request_is_built() {
        for args in [
            strings(&["-o", "one.o", "-o", "two.o"]),
            strings(&["-oone.o", "--output", "two.o"]),
            strings(&["--output=one.o", "-otwo.o"]),
        ] {
            let error = parse_configured(
                "collect-aros".into(),
                "ld".into(),
                "strip".into(),
                None,
                None,
                None,
                args,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains("specifies output more than once"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn malformed_non_utf8_attached_emulation_is_rejected() {
        use std::os::unix::ffi::OsStringExt;

        let argument = OsString::from_vec(b"-mriscv64elf_aros\xff".to_vec());
        assert!(collect_user_emulations(&[argument]).is_err());
    }

    #[test]
    fn sysroot_validation_does_not_require_a_library_directory() {
        let directory = tempfile::tempdir().unwrap();
        let sysroot = directory.path().join("absent-sdk");
        let args = strings(&["--sysroot", sysroot.to_str().unwrap(), "-o", "output.o"]);
        let multilib = parse(
            "collect-aros32".into(),
            "ld.lld".into(),
            "llvm-strip".into(),
            None,
            args.clone(),
        )
        .unwrap();
        assert!(validate_sysroot(&multilib).is_ok());
        let native = parse(
            "collect-aros".into(),
            "ld.lld".into(),
            "llvm-strip".into(),
            None,
            args,
        )
        .unwrap();
        assert!(validate_sysroot(&native).is_ok());
    }

    #[test]
    fn a_library_free_compiler_probe_accepts_a_sysroot_without_sdk_files() {
        let directory = tempfile::tempdir().unwrap();
        let sysroot = directory.path().join("absent-sdk");
        let request = parse(
            "collect-aros".into(),
            "ld.lld".into(),
            "llvm-strip".into(),
            None,
            strings(&[
                "--sysroot",
                sysroot.to_str().unwrap(),
                "-Llib",
                "probe.o",
                "-o",
                "conftest",
            ]),
        )
        .unwrap();

        assert_eq!(request.sysroot.as_deref(), Some(sysroot.as_path()));
        assert!(validate_sysroot(&request).is_ok());
    }

    #[test]
    fn sysroot_validation_still_rejects_relative_roots() {
        let request = parse(
            "collect-aros".into(),
            "ld.lld".into(),
            "llvm-strip".into(),
            None,
            strings(&["--sysroot", "relative/sysroot", "-o", "output.o"]),
        )
        .unwrap();

        let error = validate_sysroot(&request).unwrap_err();
        assert!(format!("{error:#}").contains("--sysroot must be absolute"));
    }

    #[test]
    fn a_discovered_target_input_still_requires_a_sysroot() {
        let request = parse(
            "collect-aros".into(),
            "ld.lld".into(),
            "llvm-strip".into(),
            None,
            strings(&["-o", "output.o"]),
        )
        .unwrap();

        let error = require_sysroot_library(&request, "libpthread.a").unwrap_err();
        assert!(format!("{error:#}").contains("pass an absolute AROS Developer sysroot"));
    }

    #[test]
    fn a_discovered_target_input_still_requires_a_regular_file_in_the_multilib_directory() {
        let directory = tempfile::tempdir().unwrap();
        let sysroot = directory.path().join("sysroot");
        fs::create_dir(&sysroot).unwrap();
        let request = parse(
            "collect-aros32".into(),
            "ld.lld".into(),
            "llvm-strip".into(),
            None,
            strings(&["--sysroot", sysroot.to_str().unwrap(), "-o", "output.o"]),
        )
        .unwrap();

        let error = require_sysroot_library(&request, "libpthread.a").unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("collector-required sysroot input is missing"),
            "{message}"
        );
        assert!(message.contains("sysroot/lib32/libpthread.a"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn a_bad_first_link_never_replaces_the_existing_output() {
        let directory = tempfile::tempdir().unwrap();
        let sysroot = directory.path().join("sysroot");
        fs::create_dir_all(sysroot.join("lib")).unwrap();
        let output = directory.path().join("output.o");
        fs::write(&output, b"previous good output").unwrap();
        let request = parse(
            "collect-aros".into(),
            "test-linker".into(),
            "test-strip".into(),
            None,
            vec![
                OsString::from("--sysroot"),
                sysroot.into_os_string(),
                OsString::from("-o"),
                output.clone().into_os_string(),
            ],
        )
        .unwrap();

        let logger = Logger::open(
            &crate::observability::RuntimeOptions::default(),
            "collect-aros",
        )
        .unwrap();
        let execute = |_tool: &Path, arguments: &[OsString]| {
            fs::write(output_argument(arguments), b"not an ELF")?;
            Ok(test_exit_status(0))
        };

        assert!(run_with(&request, &logger, &mut Vec::new(), execute).is_err());
        assert_eq!(fs::read(output).unwrap(), b"previous good output");
    }

    #[cfg(unix)]
    #[test]
    fn direct_frontend_skips_an_empty_second_pass() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = directory.path().join("fixture.o");
        let output = directory.path().join("output.o");
        fs::write(&fixture, elf64_with_section(".text")).unwrap();
        fs::write(&output, b"previous output").unwrap();
        let logger = Logger::open(
            &crate::observability::RuntimeOptions::default(),
            "aros-collect",
        )
        .unwrap();
        let request = EngineRequest {
            name: "aros-collect".into(),
            linker: "test-linker".into(),
            strip: None,
            emulation: None,
            args: vec![
                OsString::from("-r"),
                OsString::from("-o"),
                output.clone().into_os_string(),
                OsString::from("input.o"),
            ],
            output: output.clone(),
            sysroot: None,
            mode: LinkMode::CollectRelocatable,
            strip_output: false,
            ignore_undefined: true,
            report: None,
            keep_script: None,
            frontend: Frontend::Direct,
        };
        let mut calls = 0;
        let mut execute = |_tool: &Path, arguments: &[OsString]| {
            calls += 1;
            fs::copy(&fixture, output_argument(arguments))?;
            Ok(test_exit_status(0))
        };

        run_with(&request, &logger, &mut Vec::new(), &mut execute).unwrap();

        assert_eq!(fs::read(&output).unwrap(), fs::read(&fixture).unwrap());
        assert_eq!(calls, 1);
        assert!(!adjacent(&output, ".collect-pre").exists());
        assert!(!adjacent(&output, ".collect-final").exists());
        assert!(!adjacent(&output, ".collect-sets.ld").exists());
    }

    #[cfg(unix)]
    #[test]
    fn direct_second_pass_failure_is_atomic_and_keeps_an_explicit_script() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = directory.path().join("fixture.o");
        let output = directory.path().join("output.o");
        let script = directory.path().join("sets.ld");
        fs::write(&fixture, elf64_with_section(".aros.set.INITLIB.10")).unwrap();
        fs::write(&output, b"previous good output").unwrap();
        let logger = Logger::open(
            &crate::observability::RuntimeOptions::default(),
            "aros-collect",
        )
        .unwrap();
        let request = EngineRequest {
            name: "aros-collect".into(),
            linker: "test-linker".into(),
            strip: None,
            emulation: None,
            args: vec![
                OsString::from("-r"),
                OsString::from("-o"),
                output.clone().into_os_string(),
                OsString::from("input.o"),
            ],
            output: output.clone(),
            sysroot: None,
            mode: LinkMode::CollectRelocatable,
            strip_output: false,
            ignore_undefined: true,
            report: None,
            keep_script: Some(script.clone()),
            frontend: Frontend::Direct,
        };
        let mut calls = 0;
        let mut execute = |_tool: &Path, arguments: &[OsString]| {
            calls += 1;
            let target = output_argument(arguments);
            if calls == 1 {
                fs::copy(&fixture, target)?;
                Ok(test_exit_status(0))
            } else {
                fs::write(target, b"incomplete second pass")?;
                Ok(test_exit_status(23))
            }
        };

        let error = run_with(&request, &logger, &mut Vec::new(), &mut execute).unwrap_err();

        assert_eq!(error.diagnostic().code, DiagnosticCode::CollectorSecondLink);
        assert_eq!(
            error.diagnostic().context.as_ref().unwrap().exit_code,
            Some(23)
        );
        assert_eq!(fs::read(&output).unwrap(), b"previous good output");
        assert_eq!(calls, 2);
        assert!(fs::read_to_string(script)
            .unwrap()
            .contains("__INITLIB_LIST__"));
        assert!(!adjacent(&output, ".collect-pre").exists());
        assert!(!adjacent(&output, ".collect-final").exists());
    }
}
