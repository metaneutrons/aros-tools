//! Command-line surface of the dispatcher controller.
//!
//! The commands are inactive building blocks: no workflow calls them yet, and a
//! zero exit status is never native qualification or release authority.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use serde_json::Value;

use super::files;
use super::json::{canonical_line, parse};
use super::provider::{GhProvider, Provider};
use super::{dispatch, select, verify_binding, verify_claim, MAX_BUNDLE, MAX_RECORD};
use crate::ReleaseResult;

#[derive(Debug, Subcommand)]
pub enum DispatcherCommand {
    /// Select the bounded start archive bytes for the observed start (read-only)
    ///
    /// Reads the runner event and GitHub metadata with the job token and
    /// writes the record, signature bundle and a selection report as three
    /// exclusive private files. It does not verify the start signature and
    /// grants no upload, dispatch or publication authority.
    Select(SelectArgs),
    /// Verify the signed start while exactly one selected claim asset exists
    ///
    /// Authenticates the signed start and the exact numeric claim readback in
    /// the claim phase. It is not native or dispatch authority.
    VerifyClaim(VerifyClaimArgs),
    /// The single native dispatch at the immutable tag ref (aros-toolchains#115)
    ///
    /// Validates the closed input contract, refuses when a native dispatch
    /// already exists for the tag, sends exactly one `workflow_dispatch`, reads
    /// the named run back and writes the canonical binding subject for the
    /// dispatcher to attest. Never retries. Durable once-only consumption is
    /// not established by this command.
    Dispatch(DispatchArgs),
    /// Authenticate the original dispatcher's signature of a native-run binding
    ///
    /// Does not authenticate the actual native run, consume the claim or
    /// qualify a release.
    VerifyBinding(VerifyBindingArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RuntimeHost {
    #[value(name = "linux-x86_64")]
    LinuxX86_64,
}

impl RuntimeHost {
    const fn name(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "linux-x86_64",
        }
    }
}

#[derive(Debug, Clone, Args)]
pub struct SelectArgs {
    /// Existing caller-owned mode-0700 directory for the three exclusive outputs
    #[arg(long)]
    pub output_dir: PathBuf,
}

/// Independently selected contexts shared by claim, dispatch and binding.
#[derive(Debug, Clone, Args)]
pub struct ContextFiles {
    /// JSON file with the independently selected start source context
    #[arg(long)]
    pub source_context: PathBuf,
    /// JSON file with the dispatcher context
    #[arg(long)]
    pub dispatcher_context: PathBuf,
    /// JSON file with the dispatcher job `{id, check_run_id, name}`
    #[arg(long)]
    pub dispatcher_job: PathBuf,
}

#[derive(Debug, Clone, Args)]
pub struct VerifyClaimArgs {
    #[arg(long)]
    pub record: PathBuf,
    #[arg(long)]
    pub bundle: PathBuf,
    #[arg(long)]
    pub claim: PathBuf,
    /// Numeric ID of the sole claim asset
    #[arg(long)]
    pub claim_asset_id: u64,
    #[command(flatten)]
    pub contexts: ContextFiles,
    #[arg(long)]
    pub expected_record_sha256: String,
    #[arg(long)]
    pub expected_bundle_sha256: String,
    #[arg(long)]
    pub expected_claim_sha256: String,
    #[arg(long)]
    pub runtime_lock: PathBuf,
    #[arg(long)]
    pub runtime_directory: PathBuf,
    #[arg(long)]
    pub runtime_interface: PathBuf,
    #[arg(long, value_enum)]
    pub runtime_host: RuntimeHost,
    /// Exclusive canonical receipt output (parent must be mode 0700)
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Debug, Clone, Args)]
pub struct DispatchArgs {
    #[arg(long)]
    pub record: PathBuf,
    #[arg(long)]
    pub bundle: PathBuf,
    #[arg(long)]
    pub claim: PathBuf,
    /// JSON file with the uploaded claim asset `{id, name, size, sha256}`
    #[arg(long)]
    pub claim_asset: PathBuf,
    /// SHA-256 of the dispatcher's claim signature bundle
    #[arg(long)]
    pub claim_bundle_sha256: String,
    #[command(flatten)]
    pub contexts: ContextFiles,
    /// Exclusive canonical binding subject output (parent must be mode 0700)
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Debug, Clone, Args)]
pub struct VerifyBindingArgs {
    #[arg(long)]
    pub binding: PathBuf,
    #[arg(long)]
    pub signature: PathBuf,
    #[arg(long)]
    pub expected_binding_sha256: String,
    #[arg(long)]
    pub expected_signature_sha256: String,
    #[arg(long)]
    pub record: PathBuf,
    #[arg(long)]
    pub bundle: PathBuf,
    #[arg(long)]
    pub claim: PathBuf,
    /// JSON file with the uploaded claim asset `{id, name, size, sha256}`
    #[arg(long)]
    pub claim_asset: PathBuf,
    #[arg(long)]
    pub claim_bundle_sha256: String,
    #[command(flatten)]
    pub contexts: ContextFiles,
    /// JSON file with the independently observed actual native context
    #[arg(long)]
    pub native_context: PathBuf,
    /// Actual native event; must be `workflow_dispatch`
    #[arg(long)]
    pub native_event: String,
    /// Exclusive canonical receipt output (parent must be mode 0700)
    #[arg(long)]
    pub output: PathBuf,
}

/// Stable command label for diagnostics.
#[must_use]
pub const fn command_name(command: &DispatcherCommand) -> &'static str {
    match command {
        DispatcherCommand::Select(_) => "select",
        DispatcherCommand::VerifyClaim(_) => "verify-claim",
        DispatcherCommand::Dispatch(_) => "dispatch",
        DispatcherCommand::VerifyBinding(_) => "verify-binding",
    }
}

fn json_file(path: &Path, label: &str) -> super::Result<Value> {
    let raw = files::read_regular(&files::absolute(path)?, label, MAX_RECORD)?;
    parse(&raw, label, MAX_RECORD)
}

fn bytes(path: &Path, label: &str, limit: usize) -> super::Result<Vec<u8>> {
    files::read_regular(&files::absolute(path)?, label, limit)
}

/// Run one dispatcher command against the live provider.
///
/// # Errors
///
/// Returns the structured `AP01xx`/`AP02xx`/`AP04xx` failure of the command.
pub fn run(command: &DispatcherCommand) -> ReleaseResult<()> {
    run_with(command, &GhProvider)
}

/// Run one dispatcher command against any provider (offline tests).
///
/// # Errors
///
/// Returns the structured failure of the command.
pub fn run_with(command: &DispatcherCommand, provider: &dyn Provider) -> ReleaseResult<()> {
    match command {
        DispatcherCommand::Select(args) => run_select(args, provider),
        DispatcherCommand::VerifyClaim(args) => run_verify_claim(args, provider),
        DispatcherCommand::Dispatch(args) => run_dispatch(args, provider),
        DispatcherCommand::VerifyBinding(args) => run_verify_binding(args, provider),
    }
    .map_err(Into::into)
}

fn run_select(args: &SelectArgs, provider: &dyn Provider) -> super::Result<()> {
    let directory = files::absolute(&args.output_dir)?;
    for path in select::output_paths(&directory) {
        files::require_absent(&path)?;
        files::require_private_parent(&path)?;
    }
    let selected = select::select_start_transport(provider)?;
    select::retain(&directory, &selected)
}

fn run_verify_claim(args: &VerifyClaimArgs, provider: &dyn Provider) -> super::Result<()> {
    let output = files::absolute(&args.output)?;
    files::require_absent(&output)?;
    files::require_private_parent(&output)?;
    let source = json_file(
        &args.contexts.source_context,
        "independently selected source context",
    )?;
    let dispatcher = json_file(&args.contexts.dispatcher_context, "dispatcher context")?;
    let job = json_file(&args.contexts.dispatcher_job, "dispatcher job")?;
    let (record, bundle, claim) = (
        files::absolute(&args.record)?,
        files::absolute(&args.bundle)?,
        files::absolute(&args.claim)?,
    );
    let request = verify_claim::Request {
        record_path: &record,
        bundle_path: &bundle,
        claim_path: &claim,
        claim_asset_id: args.claim_asset_id,
        source_context: &source,
        dispatcher_context: &dispatcher,
        dispatcher_job: &job,
        expected_record_sha256: &args.expected_record_sha256,
        expected_bundle_sha256: &args.expected_bundle_sha256,
        expected_claim_sha256: &args.expected_claim_sha256,
        runtime_lock: &files::absolute(&args.runtime_lock)?,
        runtime_directory: &files::absolute(&args.runtime_directory)?,
        runtime_interface: &files::absolute(&args.runtime_interface)?,
        runtime_host: args.runtime_host.name(),
    };
    let verified = verify_claim::verify_claimed_start(provider, &request)?;
    files::write_exclusive(
        &output,
        &verify_claim::report(&verified, &request),
        "verification receipt",
    )
}

fn run_dispatch(args: &DispatchArgs, provider: &dyn Provider) -> super::Result<()> {
    // Output path checks come before the mutation.
    let output = files::absolute(&args.output)?;
    files::require_absent(&output)?;
    files::require_private_parent(&output)?;
    let source = json_file(
        &args.contexts.source_context,
        "independently selected source context",
    )?;
    let dispatcher = json_file(&args.contexts.dispatcher_context, "dispatcher context")?;
    let job = json_file(&args.contexts.dispatcher_job, "dispatcher job")?;
    let asset = json_file(&args.claim_asset, "claim asset")?;
    let claim = bytes(&args.claim, "selected claim", MAX_RECORD)?;
    let record = bytes(&args.record, "selected start record", MAX_RECORD)?;
    let bundle = bytes(&args.bundle, "selected start bundle", MAX_BUNDLE)?;
    let subject = dispatch::dispatch(
        provider,
        &dispatch::Request {
            claim_raw: &claim,
            record_bytes: &record,
            start_bundle_bytes: &bundle,
            source_context: &source,
            dispatcher_context: &dispatcher,
            dispatcher_job: &job,
            claim_asset: &asset,
            claim_bundle_sha256: &args.claim_bundle_sha256,
        },
    )?;
    files::write_exclusive(&output, &subject, "dispatch binding subject")
}

fn run_verify_binding(args: &VerifyBindingArgs, provider: &dyn Provider) -> super::Result<()> {
    let output = files::absolute(&args.output)?;
    files::require_absent(&output)?;
    files::require_private_parent(&output)?;
    let source = json_file(
        &args.contexts.source_context,
        "independently selected source context",
    )?;
    let dispatcher = json_file(&args.contexts.dispatcher_context, "dispatcher context")?;
    let job = json_file(&args.contexts.dispatcher_job, "dispatcher job")?;
    let asset = json_file(&args.claim_asset, "claim asset")?;
    let native = json_file(
        &args.native_context,
        "independently observed native context",
    )?;
    let claim = bytes(&args.claim, "selected claim", MAX_RECORD)?;
    let record = bytes(&args.record, "selected start record", MAX_RECORD)?;
    let bundle = bytes(&args.bundle, "selected start bundle", MAX_BUNDLE)?;
    let (binding_path, signature_path) = (
        files::absolute(&args.binding)?,
        files::absolute(&args.signature)?,
    );
    let request = verify_binding::Request {
        binding_path: &binding_path,
        signature_path: &signature_path,
        expected_binding_sha256: &args.expected_binding_sha256,
        expected_signature_sha256: &args.expected_signature_sha256,
        claim_raw: &claim,
        record_bytes: &record,
        start_bundle_bytes: &bundle,
        source_context: &source,
        dispatcher_context: &dispatcher,
        dispatcher_job: &job,
        claim_asset: &asset,
        expected_claim_bundle_sha256: &args.claim_bundle_sha256,
        actual_native_context: &native,
        actual_native_event: &args.native_event,
    };
    let verified = verify_binding::verify_binding_signature(provider, &request)?;
    files::write_exclusive(
        &output,
        &canonical_line(&verified),
        "authenticated binding receipt",
    )
}
