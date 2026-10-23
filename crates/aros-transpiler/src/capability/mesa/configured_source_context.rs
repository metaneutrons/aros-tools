//! Source-owned Make coordinates for one configured Mesa declaration.
//!
//! Expand the actual GenMF macro chain rather than copying its nested default
//! arguments into the transpiler. This is text/Make evaluation, not execution.

use crate::dirs::DirVars;
use crate::genmf_projection::{expand_text, Limits};
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::collect_vars_impl;
use crate::parser::TargetContext;
use aros_common::mesa_configuration::MesaMakeCoordinates;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

pub(super) fn verify_templates(root: &Path) -> Result<(), String> {
    super::configured_scope::verify_recipe_templates(
        root,
        &[
            (
                "rule_compile",
                "5addbe8a161235cabdb8dee678c1fff794ea9f1ce424c03214d2136f7f7f6fd6",
            ),
            (
                "rule_compile_cxx",
                "f966deb2ccccf466fdd8f28674e5c5d30cf14683bb722055338df80cda94f939",
            ),
            (
                "rule_compile_cxx_multi",
                "c6b1b2538e4d727df88ae9d8a5408f4654baef44135ab7cc89d2094661012fe2",
            ),
            (
                "build_module",
                "bb87fbc5a507c90528dfc661c3614b51db69a6cd54f482c3f5508525ca0e2664",
            ),
            (
                "build_module_library",
                "6be80222daf9d9478d759afb7892af3196b0febb220399837627a11af8b6fc7b",
            ),
            (
                "build_module_core",
                "4142ebf69e26c5ead567f74634a3f66142f09815c8fccf0e4fe564c2410df482",
            ),
        ],
    )
}

/// Exact nominal context, independent of the engine's rehomed output paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MesaSourceCompileContext {
    /// Make's selected object directory, including explicit declaration overrides.
    pub object_directory: PathBuf,
    /// The extra quote root passed by the actual expanded C/C++ compile macros.
    pub quote_extra: PathBuf,
    /// Make's source-owned build directory for generated Python inputs, when defined.
    pub python_build_directory: Option<PathBuf>,
}

pub(super) struct ContextInput {
    /// Complete continuation-joined source/configuration assembly, including
    /// statements after the selected declaration. These are assembly positions,
    /// not yet physical origins across recursively inserted Make includes.
    pub source_program: String,
    /// Verified physical origins for joined assembly lines; synthetic setup
    /// statements have no physical location. Paths are absolute source paths.
    pub source_locations: Vec<Option<crate::genmf_projection::SourceLineLocation>>,
    pub declaration_line: usize,
    pub invocation: String,
    pub selected: TargetContext,
}

impl ContextInput {
    pub(super) fn prefix(&self) -> Result<String, String> {
        if self.source_locations.len() != self.source_program.lines().count() {
            return Err("Mesa source origin map does not cover its complete program".into());
        }
        if self.declaration_line >= self.source_program.lines().count() {
            return Err("Mesa source declaration position is outside its complete program".into());
        }
        Ok(self
            .source_program
            .lines()
            .take(self.declaration_line)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

pub(super) fn project(
    root: &Path,
    relative: &Path,
    owner: &str,
    dirs: &DirVars,
    coordinates: &MesaMakeCoordinates,
    input: &ContextInput,
) -> Result<MesaSourceCompileContext, String> {
    let expanded = expand_text(
        &input.invocation,
        &root.join(relative).join("mmakefile.src"),
        &root.join("config/make.tmpl"),
        Limits::default(),
    )
    .map_err(|error| format!("cannot expand Mesa source-context recipe: {error}"))?;
    // This global immediate assignment is part of generated make.cfg, which
    // the bounded source reader intentionally does not import. Bind its exact
    // source formula rather than letting a missing seed pick a macro fallback.
    let make_cfg = aros_common::decode_source_bytes(super::configured_scope::read_regular_bounded(
        &root.join("config/make.cfg.in"),
        512 * 1024,
        "Mesa OBJDIR configuration",
    )?);
    let definitions: Vec<_> = make_cfg
        .lines()
        .filter(|line| line.trim_start().starts_with("OBJDIR "))
        .collect();
    if definitions.len() != 1
        || definitions[0].split_whitespace().collect::<Vec<_>>()
            != ["OBJDIR", ":=", "$(GENDIR)/$(CURDIR)"]
    {
        return Err("Mesa OBJDIR source formula is not the reviewed immediate definition".into());
    }
    let mut selected = input.selected.clone();
    for (name, value) in [
        ("TOP", coordinates.build_root.as_str()),
        ("SRCDIR", coordinates.source_root.as_str()),
        ("GENDIR", coordinates.generated_root.as_str()),
        ("PORTSDIR", coordinates.ports_root.as_str()),
    ] {
        selected.make_variables.insert(name.into(), value.into());
    }
    selected.make_variables.insert(
        "CURDIR".into(),
        relative
            .to_str()
            .ok_or("Mesa relative directory is not UTF-8")?
            .into(),
    );
    let text = crate::parser::join_continuations(&format!(
        "{}\n{}\n{}",
        definitions[0],
        input.prefix()?,
        expanded.text
    ));
    let (scope, _) = collect_vars_impl(&text, Some(&selected));
    let lookup = |name: &str| match name {
        "TOP" => Some(coordinates.build_root.clone()),
        "SRCDIR" => Some(coordinates.source_root.clone()),
        "GENDIR" => Some(coordinates.generated_root.clone()),
        "PORTSDIR" => Some(coordinates.ports_root.clone()),
        "CURDIR" => relative.to_str().map(str::to_owned),
        _ => None,
    };
    let context = MakeExprContext::new(&scope, dirs, usize::MAX, root, relative)
        .with_lookup(&lookup)
        .without_filesystem();
    let object_directory = evaluate_make_expr(&format!("$({owner}_OBJDIR)"), &context)
        .map_err(|error| format!("cannot resolve Mesa source object directory: {error}"))?;
    let object_directory = materialize_nominal(&object_directory, coordinates)?;
    if !object_directory.starts_with(&coordinates.generated_root)
        || object_directory == Path::new(&coordinates.generated_root)
    {
        return Err("Mesa object directory must be beneath measured GENDIR".into());
    }
    let mut quote_roots = BTreeSet::new();
    for origin in &expanded.provenance {
        for frame in &origin.macro_stack {
            if frame.name != "compile_q" {
                continue;
            }
            let expression = frame
                .resolved_arguments
                .get("incextra")
                .ok_or("expanded Mesa compile_q has no extra quote root")?;
            let value = evaluate_make_expr(expression, &context)
                .map_err(|error| format!("cannot resolve Mesa extra quote root: {error}"))?;
            quote_roots.insert(materialize_nominal(&value, coordinates)?);
        }
    }
    if quote_roots.len() != 1 {
        return Err(format!(
            "Mesa source recipe requires one C/C++ extra quote root, found {}",
            quote_roots.len()
        ));
    }
    let quote_extra = quote_roots
        .into_iter()
        .next()
        .ok_or("missing Mesa quote root")?;
    if !quote_extra.starts_with(&coordinates.build_root)
        && !quote_extra.starts_with(&coordinates.source_root)
    {
        return Err("Mesa extra quote root leaves measured source/build roots".into());
    }
    let python_build_directory = if scope.raw_at("top_builddir", usize::MAX).is_some() {
        let value = evaluate_make_expr("$(top_builddir)", &context)
            .map_err(|error| format!("cannot resolve Mesa Python build directory: {error}"))?;
        let directory = materialize_nominal(&value, coordinates)?;
        if !directory.starts_with(&coordinates.generated_root)
            || directory == Path::new(&coordinates.generated_root)
        {
            return Err("Mesa Python build directory must be beneath measured GENDIR".into());
        }
        Some(directory)
    } else {
        None
    };
    Ok(MesaSourceCompileContext {
        object_directory,
        quote_extra,
        python_build_directory,
    })
}

fn materialize_nominal(value: &str, coordinates: &MesaMakeCoordinates) -> Result<PathBuf, String> {
    // MakeExpr deliberately represents built-in TOP/SRCDIR as deferred CMake
    // roots before consulting caller lookups. Bind only those two known tokens
    // to measured nominal roots here, not to the productive engine layout.
    let value = value
        .replace("${AROS_BUILD_DIR}", &coordinates.build_root)
        .replace("${AROS_SOURCE_DIR}", &coordinates.source_root);
    absolute_path(&value)
}

fn absolute_path(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if !path.is_absolute()
        || value
            .chars()
            .any(|c| c.is_control() || ";$<>\"\\".contains(c))
    {
        return Err(format!(
            "Mesa source context requires an absolute inert path: {:?}",
            value.chars().take(160).collect::<String>()
        ));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err("Mesa source path escapes its root".into());
                }
            }
            Component::CurDir => {}
            _ => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

#[cfg(test)]
#[path = "configured_source_context_tests.rs"]
mod tests;
