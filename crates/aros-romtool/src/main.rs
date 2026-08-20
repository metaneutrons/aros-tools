//! ROM and distribution packaging tool for AROS-NG.
//!
//! Currently implements the kickstart package (`PKG`) container consumed by the
//! 32-bit bootstrap. See [`pkg`] for the format description.

mod pkg;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "ROM and distribution packaging tool for AROS",
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Build, inspect, or unpack a kickstart package (PKG container).
    Pkg {
        #[command(subcommand)]
        action: PkgAction,
    },
}

#[derive(Subcommand, Debug)]
enum PkgAction {
    /// Pack ELF modules into a kickstart package.
    Create {
        /// Destination package file.
        #[arg(short, long)]
        output: PathBuf,

        /// Record only basenames instead of the paths as given. The bootstrap
        /// strips directories anyway, so this keeps packages reproducible
        /// across build directories.
        #[arg(long)]
        basename: bool,

        /// Do not fail when a member is not an ELF object. The bootstrap
        /// silently ignores such members.
        #[arg(long)]
        allow_non_elf: bool,

        /// Modules to pack, in load order.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },

    /// List the members of a package.
    List {
        /// Package file to inspect.
        package: PathBuf,
    },

    /// Unpack a package into a directory.
    Extract {
        /// Package file to unpack.
        package: PathBuf,

        /// Destination directory.
        #[arg(short = 'C', long, default_value = ".")]
        directory: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Pkg { action } => match action {
            PkgAction::Create {
                output,
                basename,
                allow_non_elf,
                files,
            } => create(&output, &files, basename, allow_non_elf),
            PkgAction::List { package } => list(&package),
            PkgAction::Extract { package, directory } => extract(&package, &directory),
        },
    }
}

fn create(output: &Path, files: &[PathBuf], basename: bool, allow_non_elf: bool) -> Result<()> {
    let mode = if basename {
        pkg::PathMode::Basename
    } else {
        pkg::PathMode::Reference
    };

    let entries = pkg::create(output, files, mode)?;

    let non_elf: Vec<&pkg::Entry> = entries.iter().filter(|e| !e.is_elf()).collect();
    if !non_elf.is_empty() && !allow_non_elf {
        let names: Vec<&str> = non_elf.iter().map(|e| e.module_name()).collect();
        bail!(
            "these members are not ELF objects and would be ignored by the bootstrap: {}\n\
             pass --allow-non-elf to package them anyway",
            names.join(", ")
        );
    }

    let total: usize = entries.iter().map(|e| e.data.len()).sum();
    println!(
        "📦 {} — {} module(s), {} bytes of payload",
        output.display(),
        entries.len(),
        total
    );
    for (i, entry) in entries.iter().enumerate() {
        println!(
            "   {:>3}. {:<28} {:>9} bytes{}",
            i + 1,
            entry.module_name(),
            entry.data.len(),
            if entry.is_elf() {
                ""
            } else {
                "  (not ELF, will be ignored)"
            }
        );
    }

    Ok(())
}

fn list(package: &Path) -> Result<()> {
    let bytes =
        fs::read(package).with_context(|| format!("cannot read package '{}'", package.display()))?;
    let entries = pkg::parse(&bytes)?;

    println!("📦 {} — {} member(s)", package.display(), entries.len());
    for (i, entry) in entries.iter().enumerate() {
        println!(
            "   {:>3}. {:<28} {:>9} bytes  {}",
            i + 1,
            entry.module_name(),
            entry.data.len(),
            if entry.is_elf() {
                "ELF"
            } else {
                "non-ELF (ignored at boot)"
            }
        );
        if entry.path != entry.module_name() {
            println!("        path: {}", entry.path);
        }
    }

    Ok(())
}

fn extract(package: &Path, directory: &Path) -> Result<()> {
    let bytes =
        fs::read(package).with_context(|| format!("cannot read package '{}'", package.display()))?;
    let entries = pkg::parse(&bytes)?;

    fs::create_dir_all(directory)
        .with_context(|| format!("cannot create '{}'", directory.display()))?;

    for entry in &entries {
        // Only ever write the basename: a container path is untrusted input and
        // must not be able to escape the destination directory.
        let target = directory.join(entry.module_name());
        fs::write(&target, &entry.data)
            .with_context(|| format!("cannot write '{}'", target.display()))?;
        println!("   {} ({} bytes)", target.display(), entry.data.len());
    }

    println!("📦 extracted {} member(s)", entries.len());
    Ok(())
}
