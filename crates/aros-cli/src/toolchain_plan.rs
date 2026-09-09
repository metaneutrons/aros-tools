//! Thin producer planning frontend; all input inspection stays in the library.

use aros_toolchain::plan::PlanRequest;
use clap::{Args, ValueEnum};
use std::fmt::Write as _;
use std::path::PathBuf;

use crate::observability;

#[derive(Clone, Copy, ValueEnum)]
pub enum ResultFormat {
    Human,
    Json,
}

#[derive(Args)]
pub struct PlanArgs {
    /// Producer-owned target profile
    #[arg(long)]
    preset: String,
    /// Explicit recipe-v2 JSON regular file
    #[arg(long)]
    recipe: PathBuf,
    /// Exact AROS source checkout root
    #[arg(long)]
    source_dir: PathBuf,
    /// Exact recipe/producer checkout root
    #[arg(long)]
    producer_dir: PathBuf,
    /// Exact recipe-selected tools/collector checkout root
    #[arg(long)]
    tools_dir: PathBuf,
    /// Proposed work root; not created or reserved
    #[arg(long)]
    work_dir: Option<PathBuf>,
    /// Proposed candidate root; not created or reserved
    #[arg(long)]
    output_dir: Option<PathBuf>,
    /// Proposed cache root; not created, scanned or modified
    #[arg(long)]
    cache_dir: Option<PathBuf>,
    /// Explicit future parallel job budget
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    jobs: Option<u64>,
    /// Explicit future whole-build deadline in seconds
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    timeout_seconds: Option<u64>,
    /// Record offline build policy; planning itself never fetches
    #[arg(long, env = "AROS_OFFLINE")]
    offline: bool,
    /// Result representation on stdout, independent of diagnostic-format
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

impl PlanArgs {
    pub(crate) fn preset(&self) -> &str {
        &self.preset
    }
}

pub fn run(args: PlanArgs) -> miette::Result<()> {
    let request = PlanRequest {
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
    };
    let plan = aros_toolchain::plan::inspect(&request).map_err(|error|
        // ContractError owns exactly one structured shared diagnostic.
        observability::native_diagnostic(error.diagnostics().diagnostics[0].clone()))?;
    let output = match args.format {
        ResultFormat::Json => serde_json::to_string_pretty(&plan)
            .map_err(|_| miette::miette!("cannot serialize the validated producer plan"))?,
        ResultFormat::Human => {
            let mut result = format!(
                "Experimental toolchain inspection: {}\nHost/profile: {} / {}\nRecipe: {}\nSource: {}\nProducer: {}\nTools/collector: {}\nNo build, download, cache scan or directory reservation was performed.\n",
                plan.readiness, plan.identity.host, plan.identity.target_profile,
                plan.identity.recipe_sha256, plan.paths.source.display(),
                plan.paths.producer.display(), plan.paths.tools.display()
            );
            for finding in &plan.findings {
                let _ = writeln!(result, "\n{}: {}", finding.code, finding.message);
                if let Some(hint) = &finding.hint {
                    let _ = writeln!(result, "  {hint}");
                }
            }
            result.trim_end().to_owned()
        }
    };
    aros_common::outputln!("{output}");
    Ok(())
}
