//! Private Rust bridge for source-owned MetaMake `%fetch` invocations.
//!
//! This command is deliberately hidden from the public CLI. The native
//! lifecycle passes it to `make` through the controlled `FETCH` variable. It
//! authorizes one lock-selected cache object, durably records that use, then
//! invokes the unchanged source-owned fetch helper with the original argument
//! vector. It never downloads, expands a suffix fallback or invokes Python.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use aros_toolchain::metamake_fetch::{MetaMakeFetchInvocation, SourceUseLedger};
use aros_toolchain::source_lock::SourceLock;
use clap::Args;

use crate::observability;

/// Arguments forwarded verbatim by the source-owned MetaMake rule.
#[derive(Args)]
pub struct MetaMakeFetchArgs {
    /// Opaque source-owned `fetch.sh` arguments.
    #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
    pub arguments: Vec<OsString>,
}

/// Execute one internal verified MetaMake fetch request.
pub fn run(args: &MetaMakeFetchArgs) -> miette::Result<()> {
    let invocation = MetaMakeFetchInvocation::parse(&args.arguments)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let lock = SourceLock::parse(&read_environment_file("AROS_TOOLCHAIN_FETCH_LOCK")?)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let cache = environment_path("AROS_TOOLCHAIN_FETCH_CACHE")?;
    let ledger = SourceUseLedger::open(&environment_path("AROS_TOOLCHAIN_FETCH_LEDGER")?)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let source = invocation
        .resolve(&lock, &cache)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    ledger
        .record(&source)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let upstream = environment_path("AROS_TOOLCHAIN_FETCH_UPSTREAM")?;
    let metadata = fs::symlink_metadata(&upstream).map_err(|_| {
        miette::miette!("native MetaMake fetch bridge: source fetch helper is unavailable")
    })?;
    if !metadata.is_file() {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: source fetch helper is not a regular file"
        ));
    }
    let mut command = Command::new("/bin/bash");
    command.arg(upstream).args(source.arguments());
    observability::run_command(&mut command, "native MetaMake source fetch helper")?;
    Ok(())
}

fn environment_path(name: &str) -> miette::Result<PathBuf> {
    let value = env::var_os(name)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            miette::miette!(
                "native MetaMake fetch bridge: required controlled environment is absent"
            )
        })?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: controlled path is not absolute"
        ));
    }
    Ok(path)
}

fn read_environment_file(name: &str) -> miette::Result<Vec<u8>> {
    let path = environment_path(name)?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| {
        miette::miette!("native MetaMake fetch bridge: controlled file is unavailable")
    })?;
    if !metadata.is_file() {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: controlled file is not regular"
        ));
    }
    fs::read(path)
        .map_err(|_| miette::miette!("native MetaMake fetch bridge: cannot read controlled file"))
}
