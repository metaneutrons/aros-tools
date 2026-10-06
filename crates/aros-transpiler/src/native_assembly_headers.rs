use super::NativeOwnerProjection;
use crate::assembly_headers::AssemblyHeaderDecl;
use crate::dirs::DirVars;
use std::collections::BTreeSet;
use std::path::Path;

impl NativeOwnerProjection {
    /// Derive architecture flag inputs from sealed source declarations and
    /// verified template semantics, never from generated flag files.
    ///
    /// # Errors
    /// Rejects changed source inputs and malformed physical recipe paths.
    pub fn native_header_context(&self, root: &Path) -> Result<crate::TargetContext, String> {
        use crate::arch_endpoint_effects::{
            collect_source_arch_endpoint_effects, ArchEndpointEffectData,
        };
        self.verify(root)?;
        let mut context = self.architecture_context.clone();
        context.native_arch_include_effects.clear();
        context.native_arch_include_errors.clear();
        context.native_arch_include_catalog_closed = false;
        let mut foreign_flag_writers = Vec::new();
        if let Err(reason) = self.verify_get_archincludes_semantics(root) {
            // This is fatal only when a consumer actually invokes the macro.
            context.native_arch_include_errors.push(reason);
        }
        for recipe in self.effective_inputs().filter(|recipe| {
            Path::new(recipe)
                .extension()
                .is_some_and(|extension| extension == "src")
        }) {
            let path = aros_common::canonical_source_file(root, Path::new(recipe))
                .map_err(|error| error.to_string())?;
            let (_, bytes) = aros_common::measure_regular_file_bounded(&path, 2 * 1024 * 1024)
                .map_err(|error| error.to_string())?
                .ok_or("architecture include recipe is missing")?;
            let digest = self
                .snapshots
                .get(recipe)
                .ok_or("architecture include recipe is not sealed")?;
            if aros_common::sha256_bytes(&bytes).as_str() != digest {
                return Err(format!("architecture include recipe changed: {recipe}"));
            }
            // Legacy mmakefiles carry Latin-1 text (copyright signs); decode
            // them as every other source reader does. The digest above is
            // taken over the same bytes.
            let source = aros_common::decode_source_bytes(bytes);
            if let Some(line) = foreign_include_flag_line(&source) {
                foreign_flag_writers.push(format!("{recipe}:{line}"));
            }
            if !source.contains("%set_archincludes") {
                continue;
            }
            let scan = collect_source_arch_endpoint_effects(
                &source,
                Path::new(recipe),
                root,
                Some(&self.architecture_context),
            )?;
            for rejection in scan
                .rejected
                .iter()
                .filter(|item| item.directive == "%set_archincludes")
            {
                let tags = [
                    context.cpu.clone(),
                    context.platform.clone(),
                    context.family.clone(),
                    context
                        .cpu
                        .as_ref()
                        .zip(context.platform.as_ref())
                        .map(|(cpu, platform)| format!("{platform}-{cpu}")),
                    Some("native".into()),
                ];
                if rejection.endpoint.as_ref().is_none_or(|endpoint| {
                    tags.iter()
                        .flatten()
                        .filter(|tag| !tag.is_empty())
                        .any(|tag| endpoint.ends_with(&format!("-{tag}-set-archincludes")))
                }) {
                    context.native_arch_include_errors.push(format!(
                        "{}:{}: {}",
                        rejection.recipe, rejection.line, rejection.reason
                    ));
                }
            }
            for effect in scan.effects.into_iter().filter(|effect| {
                matches!(effect.data, ArchEndpointEffectData::SetArchIncludes { .. })
                    && effect.applies_to(&self.architecture_context)
            }) {
                match self.verify_arch_endpoint_effects(root, std::slice::from_ref(&effect)) {
                    Ok(()) => context.native_arch_include_effects.push(effect),
                    Err(reason) => context
                        .native_arch_include_errors
                        .push(format!("{}:{}: {reason}", effect.recipe, effect.line)),
                }
            }
        }
        if !foreign_flag_writers.is_empty() {
            context.native_arch_include_errors.push(format!(
                "include flag files are touched outside %set_archincludes/%get_archincludes: {}",
                foreign_flag_writers.join(", ")
            ));
        }
        // The template's two macros are fingerprinted by
        // verify_get_archincludes_semantics and the producer replay; with no
        // error the catalog of applicable flag producers is complete.
        context.native_arch_include_catalog_closed = context.native_arch_include_errors.is_empty();
        self.verify(root)?;
        Ok(context)
    }

    /// Replay closed header producers against sealed recipe bytes and exact
    /// physical MetaMake ownership. No existing generated file supplies proof.
    ///
    /// # Errors
    /// Rejects changed, duplicate, virtual or mismatched source declarations.
    pub fn verify_assembly_headers(
        &self,
        root: &Path,
        headers: &[AssemblyHeaderDecl],
    ) -> Result<(), String> {
        self.verify(root)?;
        let context = self.native_header_context(root)?;
        let configuration_path = Path::new("config/make.cfg.in");
        let configuration_exists = match root.join(configuration_path).symlink_metadata() {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(format!(
                    "cannot inspect assembly header directory configuration: {error}"
                ))
            }
        };
        if configuration_exists {
            let expected = self
                .sealed_configuration_inputs
                .get("config/make.cfg.in")
                .or_else(|| self.snapshots.get("config/make.cfg.in"))
                .ok_or("assembly header directory configuration is not sealed")?;
            let path = aros_common::canonical_source_file(root, configuration_path)
                .map_err(|error| error.to_string())?;
            let (_, bytes) = aros_common::measure_regular_file_bounded(&path, 1024 * 1024)
                .map_err(|error| error.to_string())?
                .ok_or("assembly header directory configuration is missing")?;
            if aros_common::sha256_bytes(&bytes).as_str() != expected {
                return Err("assembly header directory configuration changed".into());
            }
        }
        let mut dirs = DirVars::load(root);
        dirs.bind_native_target_tool_roles();
        let mut outputs = BTreeSet::new();
        let mut owners = BTreeSet::new();
        for header in headers {
            if !outputs.insert(&header.header_output) || !owners.insert(&header.owner) {
                return Err("duplicate assembly header output or owner".into());
            }
            let recipe = Path::new(&header.file);
            let physical_owner = header
                .file
                .strip_suffix(".src")
                .ok_or("assembly header has no source recipe owner")?;
            if self.bound_recipe(physical_owner)? != header.file {
                return Err("assembly header is not bound to its exact source recipe".into());
            }
            let path = aros_common::canonical_source_file(root, recipe)
                .map_err(|error| error.to_string())?;
            let (_, bytes) = aros_common::measure_regular_file_bounded(&path, 1024 * 1024)
                .map_err(|error| error.to_string())?
                .ok_or("assembly header recipe is missing")?;
            let digest = self
                .snapshots
                .get(&header.file)
                .ok_or("assembly header recipe is not sealed")?;
            if aros_common::sha256_bytes(&bytes).as_str() != digest {
                return Err("assembly header recipe bytes changed".into());
            }
            let text = aros_common::decode_source_bytes(bytes);
            let configuration =
                crate::local_make_includes::inline_native_make_configuration_with_templates(
                    &text,
                    root,
                    recipe,
                    crate::local_make_includes::LocalMakeIncludeLimits::default(),
                    &context.make_include_bindings,
                    &context.generated_make_templates,
                );
            for fragment in &configuration.fragments {
                let relative = fragment
                    .path
                    .to_str()
                    .ok_or("non-UTF-8 header configuration path")?;
                let expected = self
                    .sealed_configuration_inputs
                    .get(relative)
                    .or_else(|| self.snapshots.get(relative))
                    .ok_or("header configuration input is not sealed")?;
                let path = aros_common::canonical_source_file(root, &fragment.path)
                    .map_err(|error| error.to_string())?;
                let (_, bytes) = aros_common::measure_regular_file_bounded(&path, 2 * 1024 * 1024)
                    .map_err(|error| error.to_string())?
                    .ok_or("header configuration input is missing")?;
                if aros_common::sha256_bytes(&bytes).as_str() != expected {
                    return Err(format!("header configuration input changed: {relative}"));
                }
            }
            let (replay, _) = crate::assembly_headers::collect_from_snapshot(
                &text,
                &context,
                &dirs,
                root,
                recipe.parent().ok_or("recipe has no directory")?,
            );
            if replay
                .iter()
                .filter(|candidate| candidate.owner == header.owner)
                .collect::<Vec<_>>()
                != [header]
            {
                return Err(
                    "assembly header does not exactly replay from its sealed recipe".into(),
                );
            }
            for owner in [&header.owner, &header.aggregate_owner] {
                let files = self
                    .graph
                    .owner_files(owner)
                    .ok_or_else(|| format!("assembly header owner is absent: {owner}"))?;
                if files.len() != 1 || !files.contains(physical_owner) {
                    return Err(format!(
                        "assembly header owner has ambiguous physical recipe: {owner}"
                    ));
                }
                if !self.graph.declarations().iter().any(|declaration| {
                    declaration.target == *owner
                        && declaration.file == physical_owner
                        && declaration.claims_make_owner
                        && !declaration.virtual_target
                }) {
                    return Err(format!(
                        "assembly header owner is not a concrete source declaration: {owner}"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// First line (1-based) that mentions an include flag file outside a
/// `%set_archincludes`/`%get_archincludes` invocation, if any.
fn foreign_include_flag_line(source: &str) -> Option<usize> {
    let joined = crate::parser::join_continuations(source);
    let (_, origins) = crate::parser::join_continuations_with_origins(source);
    joined.lines().zip(origins).find_map(|(line, origin)| {
        let trimmed = line.trim_start();
        (line.contains(".includeflag.")
            && !trimmed.starts_with("%set_archincludes")
            && !trimmed.starts_with("%get_archincludes"))
        .then_some(origin + 1)
    })
}

#[cfg(test)]
mod include_flag_writer_tests {
    use super::foreign_include_flag_line;

    #[test]
    fn only_the_two_macros_may_name_include_flag_files() {
        let macros = "%set_archincludes mainmmake=m maindir=rom/m \\\n  modname=m pri=5 arch=x includes=\"-Ia\"\n%get_archincludes modname=m maindir=rom/m\n";
        assert_eq!(foreign_include_flag_line(macros), None);
        let handwritten = "X := 1\n\n\techo -Ib > $(GENDIR)/rom/m/m/include/.m.includeflag.1.x\n";
        assert_eq!(foreign_include_flag_line(handwritten), Some(3));
    }
}
