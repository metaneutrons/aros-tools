//! Thin CLI frontend for the experimental local producer adapter.

use std::fmt::Write as _;
use std::path::PathBuf;

use aros_common::CancellationToken;
use aros_toolchain::executor::BuildRequest;
use aros_toolchain::plan::Backend;
use clap::{Args, ValueEnum};

use crate::observability;

/// Result representation for a local candidate.
#[derive(Clone, Copy, ValueEnum)]
pub enum ResultFormat {
    /// Human-readable summary and measured output names.
    Human,
    /// Complete `aros-toolchain-result-v1` JSON document.
    Json,
}

/// Explicit inputs for one local native or legacy-preview build.
#[derive(Args)]
pub struct BuildArgs {
    /// Producer-owned target profile.
    #[arg(long)]
    pub preset: String,
    /// Explicit recipe-v2 JSON regular file.
    #[arg(long)]
    pub recipe: PathBuf,
    /// Exact AROS source checkout root.
    #[arg(long)]
    pub source_dir: PathBuf,
    /// Exact producer checkout root.
    #[arg(long)]
    pub producer_dir: PathBuf,
    /// Exact tools/collector checkout root.
    #[arg(long)]
    pub tools_dir: PathBuf,
    /// Fresh work root; this final path must not already exist.
    #[arg(long)]
    pub work_dir: PathBuf,
    /// Fresh output root; this final path must not already exist.
    #[arg(long)]
    pub output_dir: PathBuf,
    /// Existing prepared source cache; it is never populated by this command.
    #[arg(long)]
    pub cache_dir: PathBuf,
    /// Explicit lifecycle backend; native is the default and never falls back.
    #[arg(long, default_value = "native", value_parser = parse_backend)]
    pub backend: Backend,
    /// Positive producer parallelism.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub jobs: u64,
    /// Whole-operation deadline in seconds.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout_seconds: u64,
    /// Require the prepared offline cache and disable all producer transport.
    #[arg(long, env = "AROS_OFFLINE")]
    pub offline: bool,
    /// Explicit local candidate identifier; this does not publish or tag.
    #[arg(long)]
    pub release_id: String,
    /// Result representation on stdout.
    #[arg(long, value_enum, default_value = "human")]
    pub format: ResultFormat,
}

fn parse_backend(value: &str) -> Result<Backend, String> {
    match value {
        "native" => Ok(Backend::Native),
        "legacy-preview" => Ok(Backend::LegacyPreview),
        _ => {
            Err("expected native or legacy-preview; backends are never selected by fallback".into())
        }
    }
}

/// Run the adapter on a blocking thread and propagate Ctrl-C cooperatively.
pub async fn run(args: BuildArgs) -> miette::Result<()> {
    let fetch_bridge = std::env::current_exe().map_err(|_| {
        miette::miette!("cannot resolve the running aros executable for the native fetch bridge")
    })?;
    let request = BuildRequest {
        backend: args.backend,
        preset: args.preset,
        recipe: args.recipe,
        source_dir: args.source_dir,
        producer_dir: args.producer_dir,
        tools_dir: args.tools_dir,
        work_dir: args.work_dir,
        output_dir: args.output_dir,
        cache_dir: args.cache_dir,
        jobs: args.jobs,
        timeout_seconds: args.timeout_seconds,
        offline: args.offline,
        release_id: args.release_id,
        fetch_bridge: Some(fetch_bridge),
    };
    let cancellation = CancellationToken::default();
    let worker_token = cancellation.clone();
    let mut worker =
        tokio::task::spawn_blocking(move || aros_toolchain::executor::run(&request, &worker_token));
    let result = tokio::select! {
        result = &mut worker => result.map_err(|_| miette::miette!("toolchain build worker terminated unexpectedly"))?,
        signal = tokio::signal::ctrl_c() => {
            if signal.is_ok() {
                cancellation.cancel();
            }
            // The bounded child runner observes this token and reaps its process group.
            (&mut worker).await.map_err(|_| miette::miette!("toolchain build worker terminated unexpectedly"))?
        }
    };
    let result = result.map_err(|error| {
        observability::native_diagnostic(error.diagnostics().diagnostics[0].clone())
    })?;
    let output = match args.format {
        ResultFormat::Json => serde_json::to_string_pretty(&result)
            .map_err(|_| miette::miette!("cannot serialize the toolchain result"))?,
        ResultFormat::Human => {
            let mut text = format!(
                "Local toolchain candidate: {}\nProfile/host: {}/{}\nOutput root: {}\nQualification: {}\n",
                result.operation,
                result.identity.target_profile,
                result.identity.host,
                result.output_root.display(),
                result.qualification,
            );
            for file in &result.outputs {
                let _ = writeln!(text, "  {} {} ({})", file.sha256, file.size, file.path);
            }
            text.trim_end().to_owned()
        }
    };
    aros_common::outputln!("{output}");
    Ok(())
}
