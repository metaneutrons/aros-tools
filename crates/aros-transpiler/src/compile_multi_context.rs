//! Independently proven neighboring GenMF context for compile-multi.
//!
//! `%build_archspecific` emits target-specific compiler variables and
//! conditional source-search rules. Admit it only when its exact template
//! closure is verified, its outputs are disjoint from the compile-multi
//! invocation, and the selected target proves those conditional rules inactive.

use crate::make_vars::ConditionalTruth;
use crate::parser::{Invocation, TargetContext};
use crate::source_rule_ownership::CompileMultiTemplateContract;
use crate::{dirs::DirVars, literal_objects::LiteralObjectDecl, make_vars::VarScope};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Default)]
pub(super) struct ProvenAdjacentMacros {
    /// Effective, continuation-joined lines whose exact source macro effects
    /// were independently verified.
    pub(super) effective_lines: BTreeSet<usize>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prove(
    calls: &[Invocation],
    origins: &[usize],
    positions: &[Option<usize>],
    states: &[ConditionalTruth],
    compile_call: &Invocation,
    owner: &str,
    namespace: &str,
    objects: &[LiteralObjectDecl],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    directory: &Path,
    target_context: Option<&TargetContext>,
    contract: &CompileMultiTemplateContract,
) -> Result<ProvenAdjacentMacros, String> {
    let compile_position = effective_position(compile_call, origins, positions)?;
    if states.get(compile_position) != Some(&ConditionalTruth::True) {
        return Err("compile-multi context has unresolved Make control".into());
    }

    let build_calls = calls
        .iter()
        .filter(|call| call.name == "build_archspecific")
        .collect::<Vec<_>>();
    let common_calls = calls
        .iter()
        .filter(|call| call.name == "common")
        .collect::<Vec<_>>();
    if build_calls.is_empty() && common_calls.is_empty() {
        return Ok(ProvenAdjacentMacros::default());
    }
    if common_calls.len() != 1 {
        return Err("compile-multi needs one exact adjacent %common invocation".into());
    }
    let common = common_calls[0];
    let common_position = effective_position(common, origins, positions)?;
    if states.get(common_position) != Some(&ConditionalTruth::True)
        || !common.args.trim().is_empty()
        || common_position <= compile_position
    {
        return Err("compile-multi %common context is conditional, malformed or misplaced".into());
    }
    contract.prove_common_namespace_disjoint(root, namespace, !build_calls.is_empty())?;

    let mut proof = ProvenAdjacentMacros::default();
    proof.effective_lines.insert(common_position);
    if build_calls.is_empty() {
        return Ok(proof);
    }
    if build_calls.len() != 1 {
        return Err("compile-multi has competing build_archspecific contexts".into());
    }
    let build = build_calls[0];
    let build_position = effective_position(build, origins, positions)?;
    if states.get(build_position) != Some(&ConditionalTruth::True)
        || build_position >= compile_position
    {
        return Err("build_archspecific context is conditional or follows compile-multi".into());
    }
    let active_compile_calls = calls
        .iter()
        .filter(|call| call.name == "rule_compile_multi")
        .filter_map(|call| {
            effective_position(call, origins, positions)
                .ok()
                .filter(|position| states.get(*position) == Some(&ConditionalTruth::True))
        })
        .collect::<Vec<_>>();
    if active_compile_calls.len() != 1 || active_compile_calls[0] != compile_position {
        return Err("build_archspecific context has a competing compile-multi invocation".into());
    }

    let arguments = contract.build_archspecific_arguments(&build.args)?;
    let values = [
        "mainmmake",
        "maindir",
        "modname",
        "arch",
        "files",
        "cxxfiles",
        "asmfiles",
        "linklibfiles",
        "linklibobjs",
        "compiler",
        "objdir",
    ]
    .into_iter()
    .map(|name| {
        super::expand(
            &arguments[name],
            scope,
            dirs,
            root,
            directory,
            build_position,
        )
        .map(|value| (name, value))
    })
    .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;

    if values["compiler"] != "target"
        || !values["objdir"].is_empty()
        || values["modname"].is_empty()
        || !values["cxxfiles"].is_empty()
        || !values["asmfiles"].is_empty()
        || !values["linklibfiles"].is_empty()
        || !values["linklibobjs"].is_empty()
    {
        return Err("build_archspecific uses an unsupported compiler or output form".into());
    }
    let mainmmake = super::safe_owner(&values["mainmmake"])
        .then_some(values["mainmmake"].as_str())
        .ok_or("build_archspecific mainmmake is not a safe selector")?;
    let arch = super::safe_owner(&values["arch"])
        .then_some(values["arch"].as_str())
        .ok_or("build_archspecific arch is not a safe selector")?;
    let maindir = super::safe_relative(&values["maindir"])?;
    let modname = super::safe_relative(&values["modname"])?;
    if Path::new(&modname).components().count() != 1 {
        return Err("build_archspecific module name is not one path component".into());
    }
    let files = safe_source_list(&values["files"])?;
    if files.is_empty() {
        return Err("build_archspecific needs a finite nonempty C file list".into());
    }

    if target_context
        .and_then(|target| target.value_of("TARGET"))
        .is_some_and(|selected| selected != owner)
    {
        return Err("target context TARGET differs from the physical compile-multi owner".into());
    }
    for position in [build_position, compile_position] {
        if scope.raw_at("TARGET", position).is_some()
            && super::expand("$(TARGET)", scope, dirs, root, directory, position)? != owner
        {
            return Err(
                "source TARGET binding differs from the physical compile-multi owner".into(),
            );
        }
    }
    let architecture_selector = format!("{mainmmake}-{arch}");
    if [
        architecture_selector.clone(),
        format!("{architecture_selector}-quick"),
        format!("{architecture_selector}-linklib"),
    ]
    .iter()
    .any(|selector| selector == owner)
    {
        return Err("compile-multi owner activates the build_archspecific target selector".into());
    }

    let object_directory = format!("{}/{}/{}/arch", super::GENERATED, maindir, modname);
    let mut arch_outputs = BTreeSet::new();
    for file in files {
        let basename = Path::new(&file)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("build_archspecific source basename has no filename")?;
        if Path::new(basename).extension() == Some(std::ffi::OsStr::new("c")) {
            return Err("build_archspecific file list contains a source extension".into());
        }
        for extension in [".o", ".d"] {
            let output = format!("{object_directory}/{basename}{extension}");
            if !arch_outputs.insert(output.to_ascii_lowercase()) {
                return Err("build_archspecific flattened outputs collide".into());
            }
        }
    }
    let compile_outputs = objects
        .iter()
        .flat_map(|object| {
            std::iter::once(object.output.clone()).chain(
                object
                    .dependency
                    .iter()
                    .map(|dependency| dependency.output.clone()),
            )
        })
        .map(|output| output.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    if !arch_outputs.is_disjoint(&compile_outputs) {
        return Err("build_archspecific outputs overlap compile-multi outputs".into());
    }

    proof.effective_lines.insert(build_position);
    Ok(proof)
}

fn effective_position(
    call: &Invocation,
    origins: &[usize],
    positions: &[Option<usize>],
) -> Result<usize, String> {
    let physical = origins
        .get(call.line)
        .copied()
        .ok_or("adjacent GenMF macro has no physical line mapping")?;
    positions
        .get(physical)
        .copied()
        .flatten()
        .ok_or_else(|| "adjacent GenMF macro has no unique Make scope anchor".into())
}

fn safe_source_list(raw: &str) -> Result<Vec<String>, String> {
    let mut names = BTreeSet::new();
    let mut files = Vec::new();
    for item in raw.split_whitespace() {
        let safe = super::safe_relative(item)?;
        if !names.insert(safe.to_ascii_lowercase()) {
            return Err("build_archspecific source list contains duplicate files".into());
        }
        files.push(safe);
    }
    Ok(files)
}
