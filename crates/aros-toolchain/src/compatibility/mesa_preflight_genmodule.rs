//! Original source-owned module Make producer, inside the retained preflight.

use super::{failure, path_text, read_bytes, run, write, MesaPreflightRequest, Stage};
use crate::ContractError;
use aros_common::mesa_configuration_input::{
    MesaGenmoduleBinding, MESA_GENMODULE_BASE_CONFIG, MESA_GENMODULE_MAKE_FILE,
    MESA_GENMODULE_OVERRIDE_CONFIG,
};
use aros_common::mesa_genmodule_collection::{
    mesa_genmodule_output_relative_path, MesaGenmoduleCollectionBinding,
    MesaGenmoduleOutputBinding, MESA_GENMODULE_OUTPUTS_DIRECTORY, MESA_GENMODULE_SELECTION_FILE,
};
use aros_common::mesa_genmodule_inventory::ValidatedMesaGenmoduleSelection;
use aros_common::mesa_genmodule_make::MesaGenmoduleMakefile;
use aros_common::{sha256_bytes, CancellationToken};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub(super) fn produce(
    control: (&MesaPreflightRequest<'_>, &CancellationToken, Instant),
    make: &Path,
    configured_wrapper: &Path,
    stages: &mut Vec<Stage>,
) -> Result<MesaGenmoduleBinding, ContractError> {
    let (request, _, _) = control;
    let source = request.source_root;
    let source_directory = source.join("tools/genmodule");
    // Read before execution: linked/missing inputs must not be substituted by
    // an ambient executable or a guessed empty generated include.
    read_bytes(&source_directory.join("Makefile"))?;
    read_bytes(&source_directory.join("Makefile.deps"))?;
    let base = read_bytes(&source.join(MESA_GENMODULE_BASE_CONFIG))?;
    let overrides = read_bytes(&source.join(MESA_GENMODULE_OVERRIDE_CONFIG))?;
    let executable = build_original(control, make, configured_wrapper, stages)?;
    // The configure invocation explicitly selects Mesa 26.0.0. This source
    // generator's versioned declaration must match the child Mesa26 selection;
    // this is not an inferred Mesa20 compatibility path.
    let module = "mesa3dgl26-0";
    let output = request.output_root.join("genmodule-output");
    fs::create_dir(&output).map_err(|_| failure("cannot create fresh genmodule output"))?;
    let generated = run(
        control,
        "genmodule-make",
        &executable,
        &[
            "-c".into(),
            path_text(&source.join(MESA_GENMODULE_BASE_CONFIG))?,
            "-o".into(),
            path_text(&source.join(MESA_GENMODULE_OVERRIDE_CONFIG))?,
            "-d".into(),
            path_text(&output)?,
            "writemakefile".into(),
            module.into(),
            "library".into(),
        ],
        &source_directory,
        &BTreeMap::new(),
    )?;
    let bytes = read_bytes(&output.join(format!("Makefile.{module}library")))?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| failure("genmodule Make output is not UTF-8"))?;
    MesaGenmoduleMakefile::parse(module, text)
        .map_err(|_| failure("genmodule produced unsupported module Make data"))?;
    write(&request.output_root.join(MESA_GENMODULE_MAKE_FILE), &bytes)?;
    let result = MesaGenmoduleBinding {
        module: module.into(),
        makefile_sha256: sha256_bytes(&bytes),
        base_configuration_sha256: sha256_bytes(&base),
        override_configuration_sha256: sha256_bytes(&overrides),
        producer_sha256: generated.program.clone(),
    };
    stages.push(generated);
    Ok(result)
}

pub(super) fn produce_collection(
    control: (&MesaPreflightRequest<'_>, &CancellationToken, Instant),
    make: &Path,
    configured_wrapper: &Path,
    ports_root: &str,
    configuration: &aros_common::mesa_configuration::ValidatedMesaConfiguration,
    configured_stdout_sha256: &aros_common::Sha256Digest,
    stages: &mut Vec<Stage>,
) -> Result<MesaGenmoduleCollectionBinding, ContractError> {
    let (request, _, _) = control;
    // MetaMake's CALL interface may transport source-defined environment
    // assignments. Replaying the root Makefile prefix gives those values file
    // origin instead and can change ?=/override behavior. Until that transport
    // is retained and replayed, refuse rather than issue a false owner scope.
    if configuration
        .initial_values()
        .is_some_and(|values| values.entries().any(|entry| entry.name == "CALL"))
    {
        return Err(failure(
            "source-defined MetaMake environment transport is not yet modeled",
        ));
    }
    let selected = request
        .genmodule_selection
        .ok_or_else(|| failure("multi-producer genmodule execution lost its source selection"))?;
    let inventory_path = request.output_root.join(MESA_GENMODULE_SELECTION_FILE);
    let arguments = inventory_arguments(
        request,
        selected.source_profile,
        selected.scope,
        &inventory_path,
        ports_root,
    )?;
    let inventory_stage = run(
        control,
        "genmodule-inventory",
        selected.transpiler,
        &arguments,
        request.source_root,
        &BTreeMap::new(),
    )?;
    if &inventory_stage.program != selected.transpiler_sha256 {
        return Err(failure(
            "inventory stage executed a changed transpiler helper",
        ));
    }
    stages.push(inventory_stage);
    let inventory_bytes = read_bytes(&inventory_path)?;
    let binding = selected
        .source_profile
        .genmodule_inventory_binding(selected.scope);
    let inventory = ValidatedMesaGenmoduleSelection::parse_bound(&inventory_bytes, &binding)
        .map_err(|_| {
            failure("inventory output differs from the independently selected source scope")
        })?;
    let executable = build_original(control, make, configured_wrapper, stages)?;
    let producer_sha256 = super::super::measure_executable(&executable)?.0;
    fs::create_dir(request.output_root.join(MESA_GENMODULE_OUTPUTS_DIRECTORY))
        .map_err(|_| failure("cannot create fresh genmodule collection directory"))?;
    let mut outputs = Vec::new();
    for producer in inventory.producers() {
        let context = aros_common::mesa_genmodule_collection::mesa_genmodule_scope_context(
            producer,
            configuration.identity(),
            configured_stdout_sha256,
            &configuration.recipe_configuration().make_coordinates,
        )
        .map_err(|_| failure("cannot bind selected owner Make scope"))?;
        super::scope::produce(control, make, &context, stages)?;
        let base = read_bytes(&request.source_root.join(&producer.base_configuration))?;
        let overrides = producer
            .override_configuration
            .as_ref()
            .map(|relative| read_bytes(&request.source_root.join(relative)))
            .transpose()?;
        let relative = mesa_genmodule_output_relative_path(&producer.owner)
            .map_err(|_| failure("unsafe selected genmodule owner"))?;
        let raw_path = request.output_root.join(relative);
        let directory = raw_path
            .parent()
            .ok_or_else(|| failure("missing genmodule output directory"))?;
        fs::create_dir(directory)
            .map_err(|_| failure("cannot create fresh owner-specific genmodule output"))?;
        let mut arguments = vec![
            "-c".into(),
            path_text(&request.source_root.join(&producer.base_configuration))?,
        ];
        if let Some(relative) = &producer.override_configuration {
            arguments.extend(["-o".into(), path_text(&request.source_root.join(relative))?]);
        }
        arguments.extend([
            "-d".into(),
            path_text(directory)?,
            "writemakefile".into(),
            producer.module.clone(),
            producer.module_type.clone(),
        ]);
        let stage = run(
            control,
            &format!("genmodule-make-{}", producer.owner),
            &executable,
            &arguments,
            &request.source_root.join(&producer.declaration_directory),
            &BTreeMap::new(),
        )?;
        if stage.program != producer_sha256 {
            return Err(failure(
                "selected original generator changed between owners",
            ));
        }
        let raw = read_bytes(&directory.join(format!(
            "Makefile.{}{}",
            producer.module, producer.module_type
        )))?;
        let text = std::str::from_utf8(&raw)
            .map_err(|_| failure("owner-specific genmodule output is not UTF-8"))?;
        MesaGenmoduleMakefile::parse(&producer.module, text)
            .map_err(|_| failure("owner-specific genmodule Make output is unsupported"))?;
        write(&raw_path, &raw)?;
        outputs.push(MesaGenmoduleOutputBinding {
            producer: producer.clone(),
            makefile_sha256: sha256_bytes(&raw),
            base_configuration_sha256: sha256_bytes(&base),
            override_configuration_sha256: overrides.as_ref().map(|bytes| sha256_bytes(bytes)),
        });
        stages.push(stage);
    }
    let result = MesaGenmoduleCollectionBinding {
        selection: binding,
        inventory_sha256: sha256_bytes(&inventory_bytes),
        helper_sha256: selected.transpiler_sha256.clone(),
        producer_sha256,
        outputs,
    };
    aros_common::mesa_genmodule_collection::load_genmodule_collection(
        request.output_root,
        request.source_root,
        &result,
    )
    .map_err(|_| {
        failure("completed original genmodule collection failed independent byte readback")
    })?;
    Ok(result)
}

pub(super) fn inventory_arguments(
    request: &MesaPreflightRequest<'_>,
    source_profile: &crate::compatibility::SourceMesaPreflightProfile,
    scope: aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope,
    inventory_path: &Path,
    ports_root: &str,
) -> Result<Vec<String>, ContractError> {
    let mut arguments = source_profile.genmodule_inventory_arguments(scope)?;
    arguments.extend([
        "--source-dir".into(),
        path_text(request.source_root)?,
        "--output".into(),
        path_text(inventory_path)?,
        "--ports-dir".into(),
        ports_root.into(),
    ]);
    if let Some(local) = request.local_source {
        arguments.extend([
            "--build-dir".into(),
            path_text(&request.source_root.join(&local.generated_subtree))?,
        ]);
    }
    Ok(arguments)
}

fn build_original(
    control: (&MesaPreflightRequest<'_>, &CancellationToken, Instant),
    make: &Path,
    configured_wrapper: &Path,
    stages: &mut Vec<Stage>,
) -> Result<PathBuf, ContractError> {
    let (request, _, _) = control;
    let source_directory = request.source_root.join("tools/genmodule");
    read_bytes(&source_directory.join("Makefile"))?;
    read_bytes(&source_directory.join("Makefile.deps"))?;
    let executable = request.output_root.join("genmodule");
    let arguments = vec![
        "--no-builtin-rules".into(),
        "-f".into(),
        path_text(configured_wrapper)?,
        "-f".into(),
        path_text(&source_directory.join("Makefile"))?,
        "CURDIR=tools/genmodule".into(),
        format!("GENMODULE={}", path_text(&executable)?),
        "all".into(),
    ];
    stages.push(run(
        control,
        "genmodule-build",
        make,
        &arguments,
        &source_directory,
        &BTreeMap::new(),
    )?);
    Ok(executable)
}
