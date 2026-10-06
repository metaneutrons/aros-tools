//! Thin CLI frontend for the controlled local native producer.

use std::fmt::Write as _;
use std::path::PathBuf;

use aros_common::CancellationToken;
use aros_toolchain::executor::{BuildRequest, ResumePhase};
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

/// Explicit inputs for one local native build.
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
    /// Fresh work root; with --resume-from it must be the exact retained root.
    #[arg(long)]
    pub work_dir: PathBuf,
    /// Fresh output root; with --resume-from it must be the exact retained root.
    #[arg(long)]
    pub output_dir: PathBuf,
    /// Existing prepared source cache; it is never populated by this command.
    #[arg(long)]
    pub cache_dir: PathBuf,
    /// Managed local cache for host C/C++ compilation; off keeps qualification cold.
    #[arg(long, value_enum, default_value = "off")]
    pub compiler_cache: crate::build_cache::BuildCompilerCache,
    /// Prepared compiler-cache namespace; requires an explicit sccache or ccache backend.
    #[arg(long)]
    pub compiler_cache_dir: Option<PathBuf>,
    /// Positive producer parallelism.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub jobs: u64,
    /// Whole-operation deadline in seconds.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout_seconds: u64,
    /// Explicit local candidate identifier; this does not publish or tag.
    #[arg(long)]
    pub release_id: String,
    /// Re-enter one verified native phase boundary. Only `compiler` is currently safe.
    #[arg(long, value_parser = parse_resume_phase)]
    pub resume_from: Option<ResumePhase>,
    /// Result representation on stdout.
    #[arg(long, value_enum, default_value = "human")]
    pub format: ResultFormat,
}

fn parse_resume_phase(value: &str) -> Result<ResumePhase, String> {
    match value {
        "compiler" => Ok(ResumePhase::Compiler),
        _ => Err("expected compiler; incomplete compiler trees are never resumable".into()),
    }
}

/// Run the native lifecycle on a blocking thread and propagate Ctrl-C cooperatively.
pub async fn run(args: BuildArgs) -> miette::Result<()> {
    let fetch_bridge = std::env::current_exe().map_err(|_| {
        miette::miette!("cannot resolve the running aros executable for the native fetch bridge")
    })?;
    let request = BuildRequest {
        preset: args.preset,
        recipe: args.recipe,
        source_dir: args.source_dir,
        producer_dir: args.producer_dir,
        tools_dir: args.tools_dir,
        work_dir: args.work_dir,
        output_dir: args.output_dir,
        cache_dir: args.cache_dir,
        compiler_cache: match args.compiler_cache {
            crate::build_cache::BuildCompilerCache::Auto => aros_cache::CompilerBackendChoice::Auto,
            crate::build_cache::BuildCompilerCache::Off => aros_cache::CompilerBackendChoice::Off,
            crate::build_cache::BuildCompilerCache::Sccache => {
                aros_cache::CompilerBackendChoice::Sccache
            }
            crate::build_cache::BuildCompilerCache::Ccache => {
                aros_cache::CompilerBackendChoice::Ccache
            }
        },
        compiler_cache_dir: args.compiler_cache_dir,
        jobs: args.jobs,
        timeout_seconds: args.timeout_seconds,
        release_id: args.release_id,
        fetch_bridge: Some(fetch_bridge),
        resume_from: args.resume_from,
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct BuildParser {
        #[command(flatten)]
        args: BuildArgs,
    }

    #[test]
    fn producer_compiler_cache_defaults_off_and_accepts_explicit_backends() {
        let arguments = [
            "build",
            "--preset",
            "fixture",
            "--recipe",
            "/recipe",
            "--source-dir",
            "/source",
            "--producer-dir",
            "/producer",
            "--tools-dir",
            "/tools",
            "--work-dir",
            "/work",
            "--output-dir",
            "/output",
            "--cache-dir",
            "/sources",
            "--jobs",
            "1",
            "--timeout-seconds",
            "60",
            "--release-id",
            "local",
        ];
        let parsed = BuildParser::try_parse_from(arguments).unwrap();
        assert!(matches!(
            parsed.args.compiler_cache,
            crate::build_cache::BuildCompilerCache::Off
        ));
        assert!(parsed.args.compiler_cache_dir.is_none());
        for backend in ["auto", "off", "sccache", "ccache"] {
            let mut explicit = arguments.to_vec();
            explicit.extend(["--compiler-cache", backend]);
            assert!(BuildParser::try_parse_from(explicit).is_ok());
        }
        let mut invalid = arguments.to_vec();
        invalid.extend(["--compiler-cache", "remote"]);
        assert!(BuildParser::try_parse_from(invalid).is_err());
    }
}
