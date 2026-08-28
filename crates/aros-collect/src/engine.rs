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

#[must_use]
pub fn is_driver_invocation(argument_zero: Option<&OsStr>) -> bool {
    argument_zero
        .and_then(|argument| Path::new(argument).file_stem())
        .and_then(OsStr::to_str)
        .is_some_and(|name| DRIVER_NAMES.contains(&name))
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
        println!(
            "{name}: AROS linker collector\n\
             usage: {name} [collector observability options] \
             [linker arguments including --sysroot=DIR and -o FILE]\n\
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
        println!("{name} {}", env!("CARGO_PKG_VERSION"));
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
    let bin = executable.parent().ok_or_else(|| {
        failure(
            DiagnosticCode::CollectorToolResolution,
            DiagnosticStage::ToolResolution,
            "the collector executable has no parent directory",
            DiagnosticContext::default(),
        )
    })?;
    let linker = require_sibling(bin, "ld.lld").map_err(|error| {
        failure(
            DiagnosticCode::CollectorToolResolution,
            DiagnosticStage::ToolResolution,
            format!("{error:#}"),
            DiagnosticContext {
                tool: Some(bin.join("ld.lld").display().to_string()),
                ..DiagnosticContext::default()
            },
        )
    })?;
    let strip = require_sibling(bin, "llvm-strip").map_err(|error| {
        failure(
            DiagnosticCode::CollectorToolResolution,
            DiagnosticStage::ToolResolution,
            format!("{error:#}"),
            DiagnosticContext {
                tool: Some(bin.join("llvm-strip").display().to_string()),
                ..DiagnosticContext::default()
            },
        )
    })?;
    let args = expand_response_files(&raw, 0).map_err(|error| {
        failure(
            DiagnosticCode::CollectorResponseFile,
            DiagnosticStage::ResponseExpansion,
            format!("{error:#}"),
            DiagnosticContext::default(),
        )
    })?;
    let request = parse(name, linker, strip, args).map_err(|error| {
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

fn require_sibling(bin: &Path, name: &str) -> Result<PathBuf> {
    let path = bin.join(name);
    if !path.is_file() {
        bail!(
            "required sibling tool {} is missing; the released collector never searches PATH or COMPILER_PATH",
            path.display()
        );
    }
    Ok(path)
}

fn parse(
    name: String,
    linker: PathBuf,
    strip: PathBuf,
    mut args: Vec<OsString>,
) -> Result<EngineRequest> {
    let mut output = None;
    let mut sysroot = None;
    let mut mode = LinkMode::Final;
    let mut strip_output = false;
    let mut ignore_undefined = false;
    let mut index = 0;
    while index < args.len() {
        let text = args[index].to_string_lossy();
        if text == "-o" {
            output = Some(PathBuf::from(
                args.get(index + 1)
                    .context("linker command line ends after -o")?,
            ));
            index += 2;
            continue;
        }
        if let Some(value) = text.strip_prefix("-o").filter(|value| !value.is_empty()) {
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
                "the released collector does not permit a linker override; it requires its sibling ld.lld"
            );
        }
        index += 1;
    }

    let output = output.context("linker command line has no -o FILE")?;
    Ok(EngineRequest {
        name,
        linker,
        strip: Some(strip),
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

fn validate_sysroot(request: &EngineRequest) -> Result<()> {
    if let Some(root) = &request.sysroot {
        if !root.is_absolute() {
            bail!("--sysroot must be absolute, got {}", root.display());
        }
        let library_dir = root.join(if request.name == "collect-aros32" {
            "lib32"
        } else {
            "lib"
        });
        if !library_dir.is_dir() {
            bail!(
                "AROS sysroot library directory is missing: {}",
                library_dir.display()
            );
        }
    }
    Ok(())
}

fn run(
    request: &EngineRequest,
    logger: &Logger,
    diagnostics: &mut Vec<Diagnostic>,
) -> CollectorResult<()> {
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
    let status = run_tool(&request.linker, &first).map_err(|error| {
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
    let mut second = vec![
        OsString::from("-r"),
        OsString::from("-o"),
        final_staged.clone().into_os_string(),
        staged.into_os_string(),
    ];
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
    let status = run_tool(&request.linker, &second).map_err(|error| {
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
        let status = run_tool(
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
        let text = replaced[index].to_string_lossy();
        if text == "-o" {
            let slot = replaced
                .get_mut(index + 1)
                .context("linker command line ends after -o")?;
            output.as_os_str().clone_into(slot);
            return Ok(replaced);
        }
        if text.starts_with("-o") && text.len() > 2 {
            let mut joined = OsString::from("-o");
            joined.push(output);
            replaced[index] = joined;
            return Ok(replaced);
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
    fn write_linker_that_fails_second_pass(linker: &Path, fixture: &Path, counter: &Path) {
        let body = format!(
            "#!/bin/sh\n\
             count=0\n\
             if [ -f \"{counter}\" ]; then count=$(sed -n '1p' \"{counter}\"); fi\n\
             count=$((count + 1))\n\
             printf '%s\\n' \"$count\" > \"{counter}\"\n\
             out=\n\
             while [ $# -gt 0 ]; do\n\
               case $1 in\n\
                 -o) shift; out=$1 ;;\n\
                 -o*) out=${{1#-o}} ;;\n\
               esac\n\
               shift\n\
             done\n\
             if [ \"$count\" -eq 1 ]; then cp \"{fixture}\" \"$out\"; exit 0; fi\n\
             printf 'incomplete second pass' > \"$out\"\n\
             exit 23\n",
            counter = counter.display(),
            fixture = fixture.display(),
        );
        fs::write(linker, body).unwrap();
        fs::set_permissions(linker, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn only_expected_aliases_select_driver_mode() {
        assert!(is_driver_invocation(Some(OsStr::new("/tmp/collect-aros"))));
        assert!(is_driver_invocation(Some(OsStr::new("collect-aros32"))));
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
    fn missing_sibling_is_reported_without_a_path_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let error = require_sibling(directory.path(), "ld.lld").unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("required sibling tool"));
        assert!(message.contains("never searches PATH or COMPILER_PATH"));
    }

    #[test]
    fn collect_aros32_requires_the_multilib_sysroot_directory() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("lib32")).unwrap();
        let args = strings(&[
            "--sysroot",
            directory.path().to_str().unwrap(),
            "-o",
            "output.o",
        ]);
        let multilib = parse(
            "collect-aros32".into(),
            "ld.lld".into(),
            "llvm-strip".into(),
            args.clone(),
        )
        .unwrap();
        assert!(validate_sysroot(&multilib).is_ok());
        let native = parse(
            "collect-aros".into(),
            "ld.lld".into(),
            "llvm-strip".into(),
            args,
        )
        .unwrap();
        assert!(validate_sysroot(&native).is_err());
    }

    #[test]
    fn a_library_free_compiler_probe_does_not_require_a_sysroot() {
        let request = parse(
            "collect-aros".into(),
            "ld.lld".into(),
            "llvm-strip".into(),
            strings(&["-Llib", "probe.o", "-o", "conftest"]),
        )
        .unwrap();

        assert!(request.sysroot.is_none());
    }

    #[test]
    fn a_discovered_target_input_still_requires_a_sysroot() {
        let request = parse(
            "collect-aros".into(),
            "ld.lld".into(),
            "llvm-strip".into(),
            strings(&["-o", "output.o"]),
        )
        .unwrap();

        let error = require_sysroot_library(&request, "libpthread.a").unwrap_err();
        assert!(format!("{error:#}").contains("pass an absolute AROS Developer sysroot"));
    }

    #[cfg(unix)]
    #[test]
    fn a_bad_first_link_never_replaces_the_existing_output() {
        let directory = tempfile::tempdir().unwrap();
        let linker = directory.path().join("ld.lld");
        fs::write(
            &linker,
            b"#!/bin/sh\nout=\nwhile [ $# -gt 0 ]; do\n  case $1 in\n    -o) shift; out=$1 ;;\n    -o*) out=${1#-o} ;;\n  esac\n  shift\ndone\nprintf 'not an ELF' > \"$out\"\n",
        )
        .unwrap();
        fs::set_permissions(&linker, fs::Permissions::from_mode(0o755)).unwrap();
        let sysroot = directory.path().join("sysroot");
        fs::create_dir_all(sysroot.join("lib")).unwrap();
        let output = directory.path().join("output.o");
        fs::write(&output, b"previous good output").unwrap();
        let request = parse(
            "collect-aros".into(),
            linker.clone(),
            linker,
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
        assert!(run(&request, &logger, &mut Vec::new()).is_err());
        assert_eq!(fs::read(output).unwrap(), b"previous good output");
    }

    #[cfg(unix)]
    #[test]
    fn direct_frontend_skips_an_empty_second_pass() {
        let directory = tempfile::tempdir().unwrap();
        let linker = directory.path().join("ld.lld");
        let fixture = directory.path().join("fixture.o");
        let counter = directory.path().join("calls");
        let output = directory.path().join("output.o");
        fs::write(&fixture, elf64_with_section(".text")).unwrap();
        fs::write(&output, b"previous output").unwrap();
        write_linker_that_fails_second_pass(&linker, &fixture, &counter);
        let logger = Logger::open(
            &crate::observability::RuntimeOptions::default(),
            "aros-collect",
        )
        .unwrap();

        run_direct(
            linker,
            vec![
                OsString::from("-r"),
                OsString::from("-o"),
                output.clone().into_os_string(),
                OsString::from("input.o"),
            ],
            output.clone(),
            None,
            None,
            &logger,
            &mut Vec::new(),
        )
        .unwrap();

        assert_eq!(fs::read(&output).unwrap(), fs::read(&fixture).unwrap());
        assert_eq!(fs::read_to_string(counter).unwrap().trim(), "1");
        assert!(!adjacent(&output, ".collect-pre").exists());
        assert!(!adjacent(&output, ".collect-final").exists());
        assert!(!adjacent(&output, ".collect-sets.ld").exists());
    }

    #[cfg(unix)]
    #[test]
    fn direct_second_pass_failure_is_atomic_and_keeps_an_explicit_script() {
        let directory = tempfile::tempdir().unwrap();
        let linker = directory.path().join("ld.lld");
        let fixture = directory.path().join("fixture.o");
        let counter = directory.path().join("calls");
        let output = directory.path().join("output.o");
        let script = directory.path().join("sets.ld");
        fs::write(&fixture, elf64_with_section(".aros.set.INITLIB.10")).unwrap();
        fs::write(&output, b"previous good output").unwrap();
        write_linker_that_fails_second_pass(&linker, &fixture, &counter);
        let logger = Logger::open(
            &crate::observability::RuntimeOptions::default(),
            "aros-collect",
        )
        .unwrap();

        let error = run_direct(
            linker,
            vec![
                OsString::from("-r"),
                OsString::from("-o"),
                output.clone().into_os_string(),
                OsString::from("input.o"),
            ],
            output.clone(),
            None,
            Some(script.clone()),
            &logger,
            &mut Vec::new(),
        )
        .unwrap_err();

        assert_eq!(error.diagnostic().code, DiagnosticCode::CollectorSecondLink);
        assert_eq!(
            error.diagnostic().context.as_ref().unwrap().exit_code,
            Some(23)
        );
        assert_eq!(fs::read(&output).unwrap(), b"previous good output");
        assert_eq!(fs::read_to_string(counter).unwrap().trim(), "2");
        assert!(fs::read_to_string(script)
            .unwrap()
            .contains("__INITLIB_LIST__"));
        assert!(!adjacent(&output, ".collect-pre").exists());
        assert!(!adjacent(&output, ".collect-final").exists());
    }
}
