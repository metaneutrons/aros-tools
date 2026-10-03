//! Private Rust bridge for source-owned MetaMake `%fetch` invocations.
//!
//! This command is deliberately hidden from the public CLI. The native
//! lifecycle passes it to `make` through the controlled `FETCH` variable. It
//! authorizes one lock-selected cache object, durably records that use, then
//! invokes the unchanged source-owned fetch helper. GNU suffix lists are
//! narrowed to the unique lock-selected archive; no format fallback reaches
//! that helper. The bridge never downloads or invokes Python.

use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::Command;

use aros_common::{open_regular_file_nofollow, sha256_file};
use aros_toolchain::metamake_fetch::{
    MetaMakeFetchInvocation, ResolvedMetaMakeSource, SourceUseLedger,
};
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
    let lock = SourceLock::parse(&read_environment_file("AROS_TOOLCHAIN_FETCH_LOCK")?)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let invocation = MetaMakeFetchInvocation::parse_for_lock(&args.arguments, &lock)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let cache = environment_path("AROS_TOOLCHAIN_FETCH_CACHE")?;
    let ledger = SourceUseLedger::open(&environment_path("AROS_TOOLCHAIN_FETCH_LEDGER")?)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let source = invocation
        .resolve(&lock, &cache)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let private_cache = materialize_private_source(&source, &ledger)?;
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
    command
        .arg(upstream)
        .args(private_arguments(&source, &private_cache)?)
        .env("AROS_FETCH_OFFLINE", "1")
        .env("AROS_FETCH_REQUIRE_CHECKSUMS", "1");
    observability::run_command(&mut command, "native MetaMake source fetch helper")?;
    source
        .revalidate()
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    ledger
        .record(&source)
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    Ok(())
}

fn materialize_private_source(
    source: &ResolvedMetaMakeSource,
    ledger: &SourceUseLedger,
) -> miette::Result<tempfile::TempDir> {
    source
        .revalidate()
        .map_err(|error| miette::miette!("native MetaMake fetch bridge: {error}"))?;
    let parent = ledger.path().parent().ok_or_else(|| {
        miette::miette!("native MetaMake fetch bridge: source ledger has no owned parent")
    })?;
    let directory = tempfile::Builder::new()
        .prefix("metamake-source-")
        .tempdir_in(parent)
        .map_err(|_| {
            miette::miette!("native MetaMake fetch bridge: cannot stage private source")
        })?;
    let destination = directory.path().join(source.filename());
    let mut input = open_regular_file_nofollow(source.snapshot_path())
        .map_err(|_| miette::miette!("native MetaMake fetch bridge: private snapshot is unsafe"))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .open(&destination)
        .map_err(|_| {
            miette::miette!("native MetaMake fetch bridge: cannot create private payload")
        })?;
    io::copy(&mut input, &mut output)
        .and_then(|_| output.sync_all())
        .map_err(|_| {
            miette::miette!("native MetaMake fetch bridge: cannot seal private payload")
        })?;
    let measured = sha256_file(&destination).map_err(|_| {
        miette::miette!("native MetaMake fetch bridge: cannot verify staged source")
    })?;
    if measured.size != source.size() || &measured.digest != source.sha256() {
        return Err(miette::miette!(
            "native MetaMake fetch bridge: staged source differs from the selected lock"
        ));
    }
    Ok(directory)
}

fn private_arguments(
    source: &ResolvedMetaMakeSource,
    directory: &tempfile::TempDir,
) -> miette::Result<Vec<OsString>> {
    let mut arguments = source.arguments().to_vec();
    let location = arguments
        .iter()
        .position(|value| value == "-l")
        .ok_or_else(|| {
            miette::miette!("native MetaMake fetch bridge: verified source has no cache location")
        })?;
    directory
        .path()
        .as_os_str()
        .clone_into(&mut arguments[location + 1]);
    let checksum = format!("{}=sha256:{}", source.filename(), source.sha256());
    if let Some(index) = arguments.iter().position(|value| value == "-cs") {
        let value = arguments.get_mut(index + 1).ok_or_else(|| {
            miette::miette!("native MetaMake fetch bridge: checksum argument is incomplete")
        })?;
        *value = checksum.into();
    } else {
        arguments.extend(["-cs".into(), checksum.into()]);
    }
    Ok(arguments)
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
