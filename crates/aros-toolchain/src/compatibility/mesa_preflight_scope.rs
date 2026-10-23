//! Bounded Make observations under selected owner coordinates.
//!
//! The root-prefix reconstruction is not equivalent to MetaMake's inherited
//! CALL environment. The collection adapter refuses that still-unmodeled
//! native transport; synthetic scope data is not native owner qualification.

use super::{
    failure, initial_values_fragment, make_arguments, read_bytes, read_text, run_inheriting, write,
    MesaPreflightRequest, Stage,
};
use crate::ContractError;
use aros_common::make_scope_snapshot::{MakeScopeContext, ValidatedMakeScopeSnapshot};
use aros_common::{sha256_bytes, CancellationToken};
use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

pub(super) fn produce(
    control: (&MesaPreflightRequest<'_>, &CancellationToken, Instant),
    make: &Path,
    context: &MakeScopeContext,
    stages: &mut Vec<Stage>,
) -> Result<(), ContractError> {
    let (request, _, _) = control;
    let parent = stages
        .get(3)
        .filter(|stage| stage.name == "configured-values")
        .ok_or_else(|| failure("owner Make scope lacks its configured invocation"))?;
    context
        .validate()
        .map_err(|_| failure("invalid selected Make scope context"))?;
    let build = request.output_root.join("configure");
    let cwd = Path::new(&context.working_directory);
    if !cwd.starts_with(&build) || cwd == build {
        return Err(failure(
            "selected Make scope directory leaves configure output",
        ));
    }
    std::fs::create_dir_all(cwd)
        .map_err(|_| failure("cannot create selected Make scope directory"))?;
    let stage_name =
        aros_common::mesa_genmodule_collection::mesa_genmodule_scope_stage(&context.owner)
            .map_err(|_| failure("unsafe selected Make scope owner"))?;
    let makefile = read_text(&build.join("Makefile"))?;
    let wrapper = wrapper(&makefile, context)?;
    let path = request.output_root.join(format!("{stage_name}.mk"));
    write(&path, wrapper.as_bytes())?;
    let mut arguments = make_arguments(&path)?;
    let goal = arguments
        .last_mut()
        .ok_or_else(|| failure("missing Make scope goal"))?;
    goal.clone_from(&context.owner);
    arguments.insert(arguments.len() - 1, "--".into());
    arguments.extend(
        context
            .inputs
            .iter()
            .map(|(name, value)| format!("{name}={value}")),
    );
    let stage = run_inheriting(control, &stage_name, make, &arguments, cwd, parent)?;
    if stage.program != parent.program
        || stage.environment != parent.environment
        || stage.invocation_context.environment() != parent.invocation_context.environment()
    {
        return Err(failure(
            "owner Make program/environment differs from configuration",
        ));
    }
    let stdout = read_bytes(&request.output_root.join(format!("{stage_name}.stdout")))?;
    if sha256_bytes(&stdout) != stage.stdout {
        return Err(failure("owner scope stdout changed after execution"));
    }
    ValidatedMakeScopeSnapshot::from_make_stdout(&stdout, context)
        .map_err(|_| failure("native owner Make scope differs from its selected inputs"))?;
    stages.push(stage);
    Ok(())
}

fn wrapper(makefile: &str, context: &MakeScopeContext) -> Result<String, ContractError> {
    let prefix = super::super::mesa_preflight_measurement::configuration_prefix(makefile)?;
    let mut result = prefix.to_owned();
    result.push_str("include $(GENDIR)/config/compiler.cfg\n");
    result.push_str("$(if $(filter undefined,$(origin __AROS_NAMESPACE_ITEM)),,$(error Make namespace iterator collision))\n");
    result.push_str("$(info AROS_MAKE_INITIAL_NAMESPACE=v1:$(words $(sort $(.VARIABLES)));$(foreach __AROS_NAMESPACE_ITEM,$(sort $(.VARIABLES)),$(__AROS_NAMESPACE_ITEM)|$(origin $(__AROS_NAMESPACE_ITEM));)end)\n");
    result.push_str(&initial_values_fragment());
    let identity = context
        .identity_sha256()
        .map_err(|_| failure("cannot identify selected Make scope"))?;
    write!(
        result,
        "$(info AROS_MAKE_SCOPE_CONTEXT_SHA256={identity})\n$(info AROS_MAKE_SCOPE_INPUTS=v1;"
    )
    .map_err(|_| failure("cannot encode selected Make scope"))?;
    for name in context.inputs.keys() {
        let value = format!("$(subst ;,%3B,$(subst |,%7C,$(subst %,%25,$(value {name}))))");
        write!(result, "{name}|$(origin {name})|$(flavor {name})|{value};")
            .map_err(|_| failure("cannot encode selected Make scope input"))?;
    }
    write!(
        result,
        "end)\n.PHONY: {}\n{}:\n",
        context.owner, context.owner
    )
    .map_err(|_| failure("cannot encode selected Make scope goal"))?;
    if result.len() > 64 * 1024 {
        return Err(failure(
            "selected Make scope wrapper exceeds its byte bound",
        ));
    }
    Ok(result)
}
