//! The part of `collect-aros` this build needs: symbol-set collection.
//!
//! For an AROS target the linker named by the compiler spec is not `ld` but
//! `collect-aros` (`config/elf-specs.in` `*linker:` -> `scripts/aros-ld.in:5`),
//! and `TARGET_LD` is the same wrapper (`configure:18209`, `use_ld_wrapper` is
//! unconditionally `yes`). It links twice: first `ld -r` over the inputs, then,
//! having read the section names out of that result, `ld -r -T <generated
//! script> <first result>` (`tools/collect-aros/collect-aros.c:650`). The
//! second pass is what turns the `.aros.set.*` sections into the arrays the
//! code reads.
//!
//! Which of its modes matters: `-r` and `-i` make it stop after the first pass,
//! `-Ur` makes it do both (`collect-aros.c:184` and `:188`). A module link uses
//! neither and gets both passes; a kickstart member and the kickstart itself
//! use `-Ur` and get both. Our link rule was a plain `ld.lld -r`
//! (`cmake/AROS.cmake:244`), which is exactly the one mode that skips the
//! collection, so every symbol set in this build was the empty weak stub.
//!
//! Two of collect-aros's jobs live here: the symbol sets (`sets`) and the
//! library-version markers (`libreq`). Both are emitted into one generated
//! script and one second pass, as `collect-aros.c:390` does.
//!
//! The released `collect-aros` aliases additionally implement `collect_extra`
//! (`backend-generic.c:117`). They obtain `static-cxx-cxa-pure-virtual.o` and
//! `libpthread.a` from an explicit Developer sysroot instead of embedding the
//! build machine's `OBJLIBDIR`.

mod driver;
mod extra;
mod libreq;
mod sets;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{bail, Context, Result};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Link an AROS relocatable object and collect its symbol sets",
    propagate_version = true
)]
struct Cli {
    /// The real linker to drive.
    #[arg(long)]
    ld: PathBuf,

    /// Keep the generated linker script at this path instead of removing it.
    #[arg(long)]
    keep_script: Option<PathBuf>,

    /// Report set sections that could not be laid out to this file. Written
    /// only when there is something to report, and removed when there is not.
    #[arg(long)]
    report: Option<PathBuf>,

    /// The linker command line, including its `-o <output>`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<OsString>,
}

/// Where `-o` names the output, and what it names.
///
/// Both spellings the reference accepts (`collect-aros.c:181`): `-o path` and
/// `-opath`.
fn output_argument(args: &[OsString]) -> Result<(usize, bool, PathBuf)> {
    for (index, argument) in args.iter().enumerate() {
        let text = argument.to_string_lossy();
        if text == "-o" {
            let value = args
                .get(index + 1)
                .context("the linker command line ends after -o")?;
            return Ok((index + 1, false, PathBuf::from(value)));
        }
        if let Some(rest) = text.strip_prefix("-o") {
            if !rest.is_empty() {
                return Ok((index, true, PathBuf::from(rest)));
            }
        }
    }
    bail!("the linker command line has no -o <output>");
}

fn run(ld: &Path, args: &[OsString]) -> Result<bool> {
    let status = Command::new(ld)
        .args(args)
        .status()
        .with_context(|| format!("could not run {}", ld.display()))?;
    Ok(status.success())
}

fn write_report(path: Option<&PathBuf>, lines: &[String]) -> Result<()> {
    let Some(path) = path else { return Ok(()) };
    if lines.is_empty() {
        // Absent means clean, the same convention the transpiler's reports use.
        let _ = std::fs::remove_file(path);
        return Ok(());
    }
    let mut body = lines.join("\n");
    body.push('\n');
    std::fs::write(path, body).with_context(|| format!("could not write {}", path.display()))
}

fn main() -> ExitCode {
    if driver::is_driver_invocation(std::env::args_os().next().as_deref()) {
        return driver::main(std::env::args_os());
    }
    let cli = Cli::parse();
    match collect(&cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("aros-collect: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn collect(cli: &Cli) -> Result<bool> {
    if cli.args.is_empty() {
        bail!("no linker command line was given");
    }
    let (index, joined, output) = output_argument(&cli.args)?;

    // The first pass writes beside the real output, so it lands on the same
    // filesystem and a failed link leaves the evidence next to the target it
    // was for.
    let mut staged = output.clone().into_os_string();
    staged.push(".collect-pre");
    let staged = PathBuf::from(staged);

    let mut first: Vec<OsString> = cli.args.clone();
    first[index] = if joined {
        let mut joined_argument = OsString::from("-o");
        joined_argument.push(&staged);
        joined_argument
    } else {
        staged.clone().into_os_string()
    };
    if !run(&cli.ld, &first)? {
        return Ok(false);
    }

    let bytes = std::fs::read(&staged)
        .with_context(|| format!("the linker wrote no {}", staged.display()))?;
    let object = aros_common::elf::read(&bytes)
        .with_context(|| format!("could not read the sections of {}", staged.display()))?;
    let section_names = object.section_names();
    let (found, mut skipped) = sets::discover(&section_names);
    let (requirements, libreq_skipped) = libreq::discover(&object.symbols);
    skipped.extend(libreq_skipped);
    // Printed as well as written: a report file nobody aggregates is easy to
    // miss, and a section that looks like a set and is not laid out, or a
    // version requirement that is dropped, changes what the module does at
    // runtime.
    for line in &skipped {
        eprintln!("aros-collect: {}: {line}", output.display());
    }
    write_report(cli.report.as_ref(), &skipped)?;

    if found.is_empty() && requirements.is_empty() {
        // Nothing to lay out, so the first pass is already the answer. The
        // reference runs its second pass regardless; skipping it here saves one
        // linker invocation on the majority of targets and cannot change the
        // result, because an empty script contributes nothing.
        std::fs::rename(&staged, &output).with_context(|| {
            format!(
                "could not move {} to {}",
                staged.display(),
                output.display()
            )
        })?;
        return Ok(true);
    }

    let script_path = cli.keep_script.clone().unwrap_or_else(|| {
        let mut path = output.clone().into_os_string();
        path.push(".collect-sets.ld");
        PathBuf::from(path)
    });
    let script = sets::script(&found, object.class, &libreq::script(&requirements));
    std::fs::write(&script_path, &script)
        .with_context(|| format!("could not write {}", script_path.display()))?;

    // `ld -r -o <output> <first pass> -T <script>`, as collect-aros.c:676
    // builds it. No other flag from the first pass is repeated -- the machine
    // comes from the input, and the inputs are already resolved into one
    // object.
    let second: Vec<OsString> = vec![
        OsString::from("-r"),
        OsString::from("-o"),
        output.into_os_string(),
        staged.clone().into_os_string(),
        OsString::from("-T"),
        script_path.clone().into_os_string(),
    ];
    let ok = run(&cli.ld, &second)?;

    if cli.keep_script.is_none() {
        let _ = std::fs::remove_file(&script_path);
    }
    if ok {
        let _ = std::fs::remove_file(&staged);
    }
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_separate_output_argument_is_found() {
        let (index, joined, path) = output_argument(&args(&["-r", "a.o", "-o", "out.o"])).unwrap();
        assert_eq!(index, 3);
        assert!(!joined);
        assert_eq!(path, PathBuf::from("out.o"));
    }

    #[test]
    fn the_joined_output_argument_is_found() {
        let (index, joined, path) = output_argument(&args(&["-r", "-oout.o", "a.o"])).unwrap();
        assert_eq!(index, 1);
        assert!(joined);
        assert_eq!(path, PathBuf::from("out.o"));
    }

    #[test]
    fn a_command_line_without_an_output_is_refused() {
        assert!(output_argument(&args(&["-r", "a.o"])).is_err());
    }
}
