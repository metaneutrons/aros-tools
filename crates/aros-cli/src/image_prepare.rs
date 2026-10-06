//! Public process boundary for controlled, offline external-media preparation.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use aros_common::{CancellationToken, Sha256Digest};
use aros_toolchain::native_media_preparation::{
    bind_native_media_preparation_inputs, prepare_native_media, read_document,
    NativeMediaPreparationRequest,
};
use clap::Args;

#[derive(Args)]
pub struct ImagePrepareArgs {
    /// Exact preset in the explicitly selected source's aros-targets.toml
    #[arg(long)]
    preset: String,
    /// Source root owning the native build and media contracts
    #[arg(long, value_name = "DIR")]
    source_root: PathBuf,
    /// Closed local input selection; every source/tool/runtime byte is pinned
    #[arg(long, value_name = "FILE")]
    inputs: PathBuf,
    /// Expected raw SHA-256 of the input selection; no inferred pin
    #[arg(long, value_name = "SHA256", value_parser = parse_digest)]
    inputs_sha256: Sha256Digest,
    /// Existing private parent for one fresh retained preparation; never adopted
    #[arg(long, value_name = "DIR")]
    work_parent: PathBuf,
    /// Positive external-producer parallelism
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    jobs: u32,
    /// Whole-operation deadline in seconds
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    timeout_seconds: u64,
    /// Result representation on stdout
    #[arg(long, value_enum, default_value = "human")]
    format: crate::toolchain_build::ResultFormat,
}

fn parse_digest(value: &str) -> Result<Sha256Digest, String> {
    Sha256Digest::parse(value).map_err(|error| error.to_string())
}

pub async fn run(args: ImagePrepareArgs) -> miette::Result<()> {
    let started_at = Instant::now();
    let cancellation = CancellationToken::default();
    let worker_token = cancellation.clone();
    let format = args.format;
    let mut worker = tokio::task::spawn_blocking(move || {
        let raw = read_document(&args.inputs)?;
        let inputs = bind_native_media_preparation_inputs(&raw, &args.inputs_sha256)?;
        prepare_native_media(&NativeMediaPreparationRequest {
            inputs: &inputs,
            source_root: &args.source_root,
            preset: &args.preset,
            work_parent: &args.work_parent,
            jobs: args.jobs,
            started_at,
            timeout: Duration::from_secs(args.timeout_seconds),
            cancellation: &worker_token,
        })
    });
    let result = tokio::select! {
        result = &mut worker => result,
        signal = tokio::signal::ctrl_c() => {
            if signal.is_ok() { cancellation.cancel(); }
            (&mut worker).await
        }
    }
    .map_err(|_| miette::miette!("native media preparation worker terminated unexpectedly"))?
    .map_err(|error| {
        let mut diagnostic = aros_common::Diagnostic::error(
            aros_common::DiagnosticCode::CliMediaSafety,
            aros_common::DiagnosticStage::MediaSafety,
            error.diagnostics().diagnostics[0].message.clone(),
        );
        diagnostic.hint = Some("check the explicit regular executable paths, source preset and exact byte-locked inputs; inspect retained failed roots and retry only in a fresh preparation, never by adopting partial output".into());
        crate::observability::native_diagnostic(diagnostic)
    })?;
    let output = match format {
        crate::toolchain_build::ResultFormat::Json => serde_json::to_string_pretty(&result)
            .map_err(|_| miette::miette!("cannot serialize native media preparation"))?,
        crate::toolchain_build::ResultFormat::Human => format!(
            "Prepared external media inputs: {}\nPreset: {}\nBootloader: {}\nPartition table: {}\nNot a complete flash plan; no device written.",
            result.work_root.display(), result.preset, result.bootloader_path.display(),
            result.partition_table_path.display()
        ),
    };
    aros_common::outputln!("{output}");
    Ok(())
}
