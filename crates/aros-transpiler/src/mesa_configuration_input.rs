//! Consistency-only child data channel, not an assertion that configure ran.
//!
//! The parent must retain and revalidate its private completed producer and
//! bind that execution into owning-job evidence. This loader neither creates
//! that authority. Productive graph projection is a separate transaction and
//! still does not prove actual compilation, execution authentication or parity.

use std::path::PathBuf;

use aros_common::mesa_configuration_input::{
    load_mesa_configuration_input, LoadedMesaConfigurationInput,
};
use aros_common::mesa_source::MesaSourceSnapshot;
use aros_common::{ArosCompilerIdentity, ArosError, Result};

use crate::cli_args::Args;
use crate::publication::Publication;
use crate::reports::resolved_publication_path;

pub struct Binding {
    input: LoadedMesaConfigurationInput,
    source: PathBuf,
    source_tree: MesaSourceSnapshot,
    inventories: Vec<aros_transpiler::capability::mesa::configured_inventory::InventoryObservation>,
}

impl Binding {
    /// Derive the complete selected graph projection without creating producer
    /// authority. Publication still revalidates both input and source snapshots.
    pub fn project_graph(
        &mut self,
        args: &Args,
        dirs: &aros_transpiler::dirs::DirVars,
        target: Option<&aros_transpiler::TargetContext>,
        graph: &mut aros_transpiler::DependencyGraph,
    ) -> Result<()> {
        let target = target.ok_or_else(|| error("configured Mesa requires target context"))?;
        if !aros_transpiler::capability::mesa::configured_graph::required(graph, target)
            .map_err(|message| error(&message))?
        {
            // A consistency-only or inventory graph without Mesa compilation
            // does not acquire compile metadata merely by carrying this input.
            return Ok(());
        }
        if self.input.native_make().is_some() {
            return Err(error(
                "native Mesa Source-program graph projection is not yet complete",
            ));
        }
        let build = args
            .build_dir
            .as_ref()
            .ok_or_else(|| error("configured Mesa projection requires explicit --build-dir"))?;
        let ports = args
            .ports_dir
            .as_ref()
            .ok_or_else(|| error("configured Mesa projection requires explicit --ports-dir"))?;
        self.input.revalidate()?;
        self.inventories = aros_transpiler::capability::mesa::configured_graph::project(
            graph,
            &self.source,
            dirs,
            target,
            self.input.configuration(),
            build,
            ports,
        )
        .map_err(|message| error(&message))?;
        Ok(())
    }
}

pub fn load(args: &Args) -> Result<Option<Binding>> {
    let (Some(path), Some(digest)) = (
        &args.mesa_configuration_input,
        &args.mesa_configuration_input_sha256,
    ) else {
        if args.mesa_configuration_input.is_some() || args.mesa_configuration_input_sha256.is_some()
        {
            return Err(error(
                "Mesa configuration input requires both path and digest",
            ));
        }
        return Ok(None);
    };
    let digest = digest
        .parse()
        .map_err(|_| error("Mesa configuration input digest is invalid"))?;
    let input = load_mesa_configuration_input(path, &digest)?;
    let identity = input.configuration().identity();
    let cpu = identity.target_triple.split_once('-').map(|(cpu, _)| cpu);
    if args.mesa_version.as_deref() != Some("26.0.0")
        || args.toolchain.as_deref() != Some("llvm")
        || args.cpu.as_deref() != cpu
        || !matches!(identity.compiler, ArosCompilerIdentity::Llvm { .. })
    {
        return Err(error(
            "Mesa configured input disagrees with the selected target/compiler family",
        ));
    }
    let source = std::fs::canonicalize(&args.source_dir)
        .map_err(|_| error("cannot canonicalize configured Mesa source"))?;
    if std::path::Path::new(
        &input
            .configuration()
            .recipe_configuration()
            .make_coordinates
            .source_root,
    ) != source
    {
        return Err(error(
            "measured Mesa Make source root differs from selected source",
        ));
    }
    let source_tree = MesaSourceSnapshot::capture(
        &source,
        &identity.source_inputs_sha256,
        input.local_source(),
    )
    .map_err(|_| error("Mesa configuration input selected different source bytes"))?;
    let output = resolved_publication_path(&args.output)?;
    let build = args
        .build_dir
        .as_ref()
        .map(|build| resolved_publication_path(build))
        .transpose()?;
    let generated_paths = std::iter::once(&output).chain(build.iter());
    if generated_paths
        .clone()
        .any(|path| path.starts_with(input.root()) || input.root().starts_with(path))
    {
        return Err(error(
            "transpiler output/build overlaps the measured Mesa input",
        ));
    }
    if generated_paths
        .into_iter()
        .any(|path| !source_tree.permits_generated_path(&source, path))
    {
        return Err(error(
            "configured Mesa output/build must be source-distinct or inside its exact selected local build subtree",
        ));
    }
    Ok(Some(Binding {
        input,
        source,
        source_tree,
        inventories: Vec::new(),
    }))
}

pub fn stage(
    binding: Option<&Binding>,
    publication: &mut Publication,
    output: &std::path::Path,
) -> Result<()> {
    let sidecar = output.with_extension("mesa-configuration-binding.json");
    let Some(binding) = binding else {
        publication.absent(sidecar);
        return Ok(());
    };
    binding.input.revalidate()?;
    for inventory in &binding.inventories {
        inventory.verify().map_err(|message| error(&message))?;
    }
    binding
        .source_tree
        .verify(&binding.source)
        .map_err(|_| error("configured Mesa source changed during transpilation"))?;
    let mut report = serde_json::json!({
        "schema":if binding.input.local_source().is_some() { "aros-mesa-transpiler-binding-v2" } else { "aros-mesa-transpiler-binding-v1" },
        "qualification":"data-consistency-only",
        "target_selection":"not-established-by-this-data-channel",
        "configuration_input_sha256":binding.input.input_sha256(),
        "preflight_receipt_sha256":binding.input.receipt_sha256(),
        "identity":binding.input.configuration().identity(),
        "limits":"does not prove producer execution, complete target/compiler selection, actual compilation or Make ABI parity"
    });
    if let Some(local) = binding.input.local_source() {
        report["local_source"] = serde_json::to_value(local)
            .map_err(|_| error("cannot serialize local Mesa source binding"))?;
    }
    let bytes = serde_json::to_vec_pretty(&report)
        .map_err(|_| error("cannot serialize Mesa configuration binding"))?;
    publication.present(sidecar, bytes);
    Ok(())
}

fn error(message: &str) -> ArosError {
    ArosError::Configuration {
        file: "Mesa configuration input".into(),
        message: message.into(),
    }
}

#[cfg(test)]
#[path = "mesa_configuration_input_tests.rs"]
mod tests;
