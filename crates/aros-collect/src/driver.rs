//! Clang-compatible, relocatable `collect-aros` driver mode.

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

#[derive(Debug)]
struct Driver {
    name: String,
    linker: PathBuf,
    strip: PathBuf,
    args: Vec<OsString>,
    output: PathBuf,
    sysroot: Option<PathBuf>,
    mode: LinkMode,
    strip_output: bool,
    ignore_undefined: bool,
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
    let driver = parse(name, linker, strip, args).map_err(|error| {
        failure(
            DiagnosticCode::CollectorInvocation,
            DiagnosticStage::Invocation,
            format!("{error:#}"),
            DiagnosticContext::default(),
        )
    })?;
    validate_sysroot(&driver).map_err(|error| {
        failure(
            DiagnosticCode::CollectorSysroot,
            DiagnosticStage::SysrootValidation,
            format!("{error:#}"),
            driver_context(&driver),
        )
    })?;
    run(&driver, logger, diagnostics)
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

fn parse(name: String, linker: PathBuf, strip: PathBuf, mut args: Vec<OsString>) -> Result<Driver> {
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
    Ok(Driver {
        name,
        linker,
        strip,
        args,
        output,
        sysroot,
        mode,
        strip_output,
        ignore_undefined,
    })
}

fn validate_sysroot(driver: &Driver) -> Result<()> {
    if let Some(root) = &driver.sysroot {
        if !root.is_absolute() {
            bail!("--sysroot must be absolute, got {}", root.display());
        }
        let library_dir = root.join(if driver.name == "collect-aros32" {
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

fn run(driver: &Driver, logger: &Logger, diagnostics: &mut Vec<Diagnostic>) -> CollectorResult<()> {
    let staged = adjacent(&driver.output, ".collect-pre");
    let final_staged = adjacent(&driver.output, ".collect-final");
    let script = adjacent(&driver.output, ".collect-sets.ld");
    for path in [&staged, &final_staged, &script] {
        remove_if_exists(path).map_err(|error| {
            failure(
                DiagnosticCode::CollectorPublication,
                DiagnosticStage::Publication,
                format!("{error:#}"),
                driver_context(driver),
            )
        })?;
    }
    let cleanup = Cleanup::new([staged.clone(), final_staged.clone(), script.clone()]);

    let mut first = replace_output(&driver.args, &staged).map_err(|error| {
        failure(
            DiagnosticCode::CollectorInvocation,
            DiagnosticStage::Invocation,
            format!("{error:#}"),
            driver_context(driver),
        )
    })?;
    if !first.iter().any(|argument| argument == "-r") {
        first.insert(0, OsString::from("-r"));
    }
    logger.event(
        LogLevel::Debug,
        "link.first.start",
        "starting first relocatable link",
        &driver_context(driver),
    )?;
    let status = run_tool(&driver.linker, &first).map_err(|error| {
        failure(
            DiagnosticCode::CollectorFirstLink,
            DiagnosticStage::FirstLink,
            format!("{error:#}"),
            driver_context(driver),
        )
    })?;
    if !status.success() {
        return Err(process_failure(
            DiagnosticCode::CollectorFirstLink,
            DiagnosticStage::FirstLink,
            "the first relocatable link failed",
            status,
            driver_context(driver),
        ));
    }
    if driver.mode == LinkMode::Incremental {
        set_aros_abi(&staged).map_err(|error| {
            failure(
                DiagnosticCode::CollectorAbi,
                DiagnosticStage::AbiMarking,
                format!("{error:#}"),
                driver_context(driver),
            )
        })?;
        publish(&staged, &driver.output).map_err(|error| {
            failure(
                DiagnosticCode::CollectorPublication,
                DiagnosticStage::Publication,
                format!("{error:#}"),
                driver_context(driver),
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
                ..driver_context(driver)
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
            .with_context(driver_context(driver));
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
            &driver_context(driver),
        )?;
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
            driver_context(driver),
        )
    })?;

    let extras = extra::discover(&object.symbols);
    let mut second = vec![
        OsString::from("-r"),
        OsString::from("-o"),
        final_staged.clone().into_os_string(),
        staged.into_os_string(),
    ];
    if extras.cxx_pure_virtual {
        second.push(
            require_sysroot_library(driver, "static-cxx-cxa-pure-virtual.o")
                .map_err(|error| required_input_failure(driver, &error))?
                .into_os_string(),
        );
    }
    if extras.pthread {
        second.push(
            require_sysroot_library(driver, "libpthread.a")
                .map_err(|error| required_input_failure(driver, &error))?
                .into_os_string(),
        );
    }
    if has_undefined(&object) {
        second.extend(resupplied_libraries(&driver.args));
    }
    second.push(OsString::from("-T"));
    second.push(script.into_os_string());
    logger.event(
        LogLevel::Debug,
        "link.second.start",
        "starting set-collection link",
        &driver_context(driver),
    )?;
    let status = run_tool(&driver.linker, &second).map_err(|error| {
        failure(
            DiagnosticCode::CollectorSecondLink,
            DiagnosticStage::SecondLink,
            format!("{error:#}"),
            driver_context(driver),
        )
    })?;
    if !status.success() {
        return Err(process_failure(
            DiagnosticCode::CollectorSecondLink,
            DiagnosticStage::SecondLink,
            "the set-collection link failed",
            status,
            driver_context(driver),
        ));
    }

    if driver.mode == LinkMode::Final && !driver.ignore_undefined {
        let output = read_object(&final_staged).map_err(|error| {
            failure(
                DiagnosticCode::CollectorObjectInspection,
                DiagnosticStage::ObjectInspection,
                format!("{error:#}"),
                driver_context(driver),
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
                driver_context(driver),
            ));
        }
    }
    if driver.strip_output {
        let status = run_tool(
            &driver.strip,
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
                    tool: Some(driver.strip.display().to_string()),
                    ..driver_context(driver)
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
                    tool: Some(driver.strip.display().to_string()),
                    ..driver_context(driver)
                },
            ));
        }
    }
    set_aros_abi(&final_staged).map_err(|error| {
        failure(
            DiagnosticCode::CollectorAbi,
            DiagnosticStage::AbiMarking,
            format!("{error:#}"),
            driver_context(driver),
        )
    })?;
    #[cfg(unix)]
    fs::set_permissions(&final_staged, fs::Permissions::from_mode(0o766)).map_err(|error| {
        failure(
            DiagnosticCode::CollectorPublication,
            DiagnosticStage::Publication,
            format!(
                "cannot set permissions on {}: {error}",
                final_staged.display()
            ),
            driver_context(driver),
        )
    })?;
    publish(&final_staged, &driver.output).map_err(|error| {
        failure(
            DiagnosticCode::CollectorPublication,
            DiagnosticStage::Publication,
            format!("{error:#}"),
            driver_context(driver),
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
    Command::new(tool)
        .args(args)
        .status()
        .with_context(|| format!("cannot execute required sibling tool {}", tool.display()))
}

fn driver_context(driver: &Driver) -> DiagnosticContext {
    DiagnosticContext {
        tool: Some(driver.linker.display().to_string()),
        mode: Some(
            match driver.mode {
                LinkMode::Final => "final",
                LinkMode::Incremental => "incremental",
                LinkMode::CollectRelocatable => "collect_relocatable",
            }
            .into(),
        ),
        output: Some(driver.output.display().to_string()),
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

fn required_input_failure(driver: &Driver, error: &anyhow::Error) -> CollectorFailure {
    failure(
        DiagnosticCode::CollectorRequiredInput,
        DiagnosticStage::RequiredInput,
        format!("{error:#}"),
        driver_context(driver),
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

fn require_sysroot_library(driver: &Driver, name: &str) -> Result<PathBuf> {
    let root = driver.sysroot.as_ref().with_context(|| {
        format!(
            "the first link requires {name}, but the linker command line has no --sysroot; pass an absolute AROS Developer sysroot"
        )
    })?;
    let directory = root.join(if driver.name == "collect-aros32" {
        "lib32"
    } else {
        "lib"
    });
    require_library(&directory, name)
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
        let driver = parse(
            "collect-aros".into(),
            "ld.lld".into(),
            "llvm-strip".into(),
            strings(&["-Llib", "probe.o", "-o", "conftest"]),
        )
        .unwrap();

        assert!(driver.sysroot.is_none());
    }

    #[test]
    fn a_discovered_target_input_still_requires_a_sysroot() {
        let driver = Driver {
            name: "collect-aros".into(),
            linker: "ld.lld".into(),
            strip: "llvm-strip".into(),
            args: Vec::new(),
            output: "output.o".into(),
            sysroot: None,
            mode: LinkMode::Final,
            strip_output: false,
            ignore_undefined: false,
        };

        let error = require_sysroot_library(&driver, "libpthread.a").unwrap_err();
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
        let driver = Driver {
            name: "collect-aros".into(),
            linker: linker.clone(),
            strip: linker,
            args: vec![
                OsString::from("--sysroot"),
                sysroot.clone().into_os_string(),
                OsString::from("-o"),
                output.clone().into_os_string(),
            ],
            output: output.clone(),
            sysroot: Some(sysroot),
            mode: LinkMode::Final,
            strip_output: false,
            ignore_undefined: false,
        };

        let logger = Logger::open(
            &crate::observability::RuntimeOptions::default(),
            "collect-aros",
        )
        .unwrap();
        assert!(run(&driver, &logger, &mut Vec::new()).is_err());
        assert_eq!(fs::read(output).unwrap(), b"previous good output");
    }
}
