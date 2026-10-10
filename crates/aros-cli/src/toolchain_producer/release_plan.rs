//! Read-only matrix projection from the validated compiler-family collection.
//! Runner labels and A/B scheduling are producer policy, not target semantics.

use std::path::PathBuf;

use aros_common::Sha256Digest;
use aros_toolchain::release_inputs::ReleaseInputs;
use clap::Args;

use super::{native_error, print_json, ResultFormat};

#[derive(Args)]
pub(super) struct ReleasePlanArgs {
    /// Directory containing toolchain-release-inputs-v2.json and its exact referenced documents
    #[arg(long)]
    directory: PathBuf,
    /// Independently selected raw collection digest; not signature verification
    #[arg(long)]
    inputs_sha256: Sha256Digest,
    /// Complete deterministic lane projection; never build or release admission
    #[arg(long, value_enum, default_value = "human")]
    format: ResultFormat,
}

pub(super) fn run(args: &ReleasePlanArgs) -> miette::Result<()> {
    let inputs = ReleaseInputs::load(&args.directory).map_err(|error| native_error(&error))?;
    if inputs.collection_sha256() != &args.inputs_sha256 {
        return Err(miette::miette!(
            "release inputs differ from the independently selected digest"
        ));
    }
    let groups = inputs
        .groups()
        .iter()
        .map(|group| {
            serde_json::json!({
                "group_id": group.id(),
                "source_commit": group.recipe().source().0,
                "source_tree": group.recipe().source().1,
                "upstream_commit": group.profiles().upstream_commit(),
                "recipe": {
                    "file": group.recipe_reference().file(),
                    "sha256": group.recipe_reference().sha256(),
                },
                "source_lock": {
                    "file": group.source_lock_reference().file(),
                    "sha256": group.source_lock_reference().sha256(),
                },
                "profiles": {
                    "file": group.profiles_reference().file(),
                    "sha256": group.profiles_reference().sha256(),
                },
            })
        })
        .collect::<Vec<_>>();
    let lanes = inputs
        .expected_lanes()
        .iter()
        .map(|lane| {
            serde_json::json!({
                "group_id": lane.group_id(),
                "host": lane.host(),
                "target_profile": lane.profile(),
            })
        })
        .collect::<Vec<_>>();
    let document = serde_json::json!({
        "schema": "aros-toolchain-producer-stage-v2",
        "operation": "release-plan",
        "release_format": "family-v2",
        "assurance": "input-binding-only",
        "inputs_sha256": inputs.collection_sha256(),
        "producer_commit": inputs.producer_commit(),
        "tools_commit": inputs.tools_commit(),
        "hosts": inputs.hosts(),
        "groups": groups,
        "lane_count": lanes.len(),
        "lanes": lanes,
    });
    // Reacquire the closed document set before exposing any matrix. This is
    // not a lock or an atomic snapshot against concurrent owner writes.
    let rechecked = ReleaseInputs::load(&args.directory).map_err(|error| native_error(&error))?;
    if rechecked.collection_sha256() != inputs.collection_sha256() {
        return Err(miette::miette!(
            "release inputs changed while projecting the matrix"
        ));
    }
    match args.format {
        ResultFormat::Json => print_json(&document),
        ResultFormat::Human => {
            aros_common::outputln!(
                "Family-v2 release plan: {} groups, {} hosts, {} lanes\nInputs SHA-256: {}\nNo compiler execution, provenance or publication is qualified.",
                inputs.groups().len(),
                inputs.hosts().len(),
                inputs.expected_lanes().len(),
                inputs.collection_sha256()
            );
            Ok(())
        }
    }
}
