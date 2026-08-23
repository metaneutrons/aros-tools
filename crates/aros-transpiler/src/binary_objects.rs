//! `%rule_link_binary`: a flat binary wrapped as a relocatable object.
//!
//! `config/make.tmpl:1552` links the given objects at a fixed text address with
//! no ELF wrapper, then re-links that raw image with `ld -r --format binary`,
//! which gives the caller `_binary_<name>_start`, `_binary_<name>_end` and
//! `_binary_<name>_size`. Four declarations use it, and nothing modelled or even
//! reported it, so `kernel.resource` carried `_binary_smpbootstrap_start` and
//! `_binary_smpbootstrap_size` as dangling externals: the SMP trampoline the
//! kernel copies to low memory to start the other cores.
//!
//! The `cd` in the reference recipe is load-bearing. `ld --format binary`
//! derives the symbol names from the input path as written, so the second link
//! runs in the directory holding the image and names it bare.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::parser::VarScope;

/// One resolved `%rule_link_binary`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryObjectDecl {
    /// Name of the flat image, which decides the `_binary_<name>_*` symbols.
    pub name: String,
    /// The wrapped object this produces.
    pub output: String,
    /// Directory of the declaring mmakefile, relative to the source root.
    pub directory: String,
    /// Source base names to compile or assemble into the flat image.
    pub sources: Vec<String>,
    /// `start=`: entry point and text address of the flat image.
    pub start: String,
    /// `ldflags=`, already split into tokens.
    pub ldflags: Vec<String>,
    /// The target that must link the wrapped object, or empty when the
    /// declaration does not say and nothing in the file implies it.
    pub consumer: String,
    /// Architecture tag gating the consumer, empty when unconditional.
    pub arch_tag: String,
}

fn arg(body: &str, key: &str) -> Option<String> {
    crate::includes::arg_value_quoted(body, key).or_else(|| crate::includes::arg_value(body, key))
}

/// Maps an object path produced by a sibling `%rule_assemble_multi` back to the
/// source base name in the declaring directory.
///
/// The reference builds `$(OBJDIR)/smpbootstrap.o` with
/// `%rule_assemble_multi basenames=smpbootstrap targetdir=$(OBJDIR)`, whose
/// default suffix is `.s`. Only the stem is needed here; CMake resolves the
/// extension the same way it does for every other source.
fn object_to_source(object: &str) -> Option<String> {
    let stem = Path::new(object).file_stem()?.to_str()?;
    if stem.is_empty() || stem.contains(['$', '*', ';']) {
        return None;
    }
    Some(stem.to_owned())
}

/// Collects the `%rule_link_binary` declarations of one mmakefile.
///
/// `arch_roots` maps a `%build_archspecific` object root to its
/// `(mainmmake, arch)`, which is how the reference attaches these objects: the
/// output lands in the module's arch object directory and
/// `config/make.tmpl:2921` collects it with `$(wildcard $(OBJDIR)/arch/*.o)`.
///
/// Returns the declarations and, for reporting, the ones that could not be
/// resolved. Nothing is dropped silently.
pub fn collect_binary_objects(
    content: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    rel_dir: &Path,
    known_targets: &[String],
    arch_roots: &[(String, String, String)],
) -> (Vec<BinaryObjectDecl>, Vec<String>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let directory = rel_dir.to_string_lossy().replace('\\', "/");

    for body in crate::includes::directive_bodies_pub(content, "%rule_link_binary") {
        let expressions = MakeExprContext::new(scope, dirs, usize::MAX, root, rel_dir);
        let resolve = |raw: &str| -> Result<String, String> {
            evaluate_make_expr(raw, &expressions).map_err(|error| error.to_string())
        };

        let Some(raw_file) = arg(&body, "file") else {
            skipped.push(format!("{directory}: %rule_link_binary without file="));
            continue;
        };
        let Some(raw_name) = arg(&body, "name") else {
            skipped.push(format!("{directory}: %rule_link_binary without name="));
            continue;
        };
        let output = match resolve(&raw_file) {
            Ok(value) => value.trim().to_owned(),
            Err(error) => {
                skipped.push(format!(
                    "{directory}: %rule_link_binary file={raw_file} cannot be evaluated: {error}"
                ));
                continue;
            }
        };
        let name = match resolve(&raw_name) {
            Ok(value) => value.trim().to_owned(),
            Err(error) => {
                skipped.push(format!(
                    "{directory}: %rule_link_binary name={raw_name} cannot be evaluated: {error}"
                ));
                continue;
            }
        };
        if name.is_empty() || name.contains(['/', '$', ';', ' ']) {
            skipped.push(format!(
                "{directory}: %rule_link_binary name={name} is not a plain image name"
            ));
            continue;
        }

        let mut sources: Vec<String> = Vec::new();
        let mut unresolved = false;
        for key in ["files", "asmfiles"] {
            if let Some(raw) = arg(&body, key) {
                match resolve(&raw) {
                    Ok(value) => {
                        sources.extend(value.split_whitespace().filter_map(object_to_source));
                    }
                    Err(error) => {
                        skipped.push(format!(
                            "{directory}: %rule_link_binary {key}={raw} cannot be evaluated: {error}"
                        ));
                        unresolved = true;
                    }
                }
            }
        }
        if let Some(raw) = arg(&body, "objs") {
            match resolve(&raw) {
                Ok(value) => {
                    for object in value.split_whitespace() {
                        if let Some(stem) = object_to_source(object) {
                            sources.push(stem);
                        } else {
                            skipped.push(format!(
                                "{directory}: %rule_link_binary objs={object} is not a plain object path"
                            ));
                            unresolved = true;
                        }
                    }
                }
                Err(error) => {
                    skipped.push(format!(
                        "{directory}: %rule_link_binary objs={raw} cannot be evaluated: {error}"
                    ));
                    unresolved = true;
                }
            }
        }
        if unresolved {
            continue;
        }
        sources.dedup();
        if sources.is_empty() {
            skipped.push(format!(
                "{directory}: %rule_link_binary name={name} has no resolvable inputs"
            ));
            continue;
        }

        let start = arg(&body, "start")
            .and_then(|raw| resolve(&raw).ok())
            .map_or_else(|| "0".to_owned(), |value| value.trim().to_owned());
        let ldflags: Vec<String> = arg(&body, "ldflags")
            .and_then(|raw| resolve(&raw).ok())
            .map(|value| {
                value
                    .split_whitespace()
                    .filter(|token| !token.contains(['$', ';', '"']))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        // The consumer, in the two ways the reference states it: an explicit
        // mmake= naming a target in this file, or the output landing in a
        // %build_archspecific object root.
        let mut consumer = String::new();
        let mut arch_tag = String::new();
        if let Some(raw) = arg(&body, "mmake") {
            if let Ok(value) = resolve(&raw) {
                let value = crate::parser::sanitize_ident(value.trim());
                if known_targets.contains(&value) {
                    consumer = value;
                }
            }
        }
        if consumer.is_empty() {
            let output_dir = Path::new(&output)
                .parent()
                .map(|dir| dir.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if let Some((_, mainmmake, tag)) = arch_roots
                .iter()
                .find(|(arch_root, _, _)| *arch_root == output_dir)
            {
                consumer.clone_from(mainmmake);
                arch_tag.clone_from(tag);
            }
        }
        if consumer.is_empty() {
            skipped.push(format!(
                "{directory}: %rule_link_binary name={name} has no target that links {output}"
            ));
            continue;
        }

        out.push(BinaryObjectDecl {
            name,
            output,
            directory: directory.clone(),
            sources,
            start,
            ldflags,
            consumer,
            arch_tag,
        });
    }

    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::DirVars;
    use crate::parser::{collect_vars, join_continuations};
    use std::path::PathBuf;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../..")
            .canonicalize()
            .unwrap()
    }

    #[test]
    fn the_smp_trampoline_resolves_to_the_kernels_arch_object_root() {
        let root = root();
        let rel = PathBuf::from("arch/x86_64-pc/kernel");
        let content = aros_common::read_source(&root.join(&rel).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&root);
        // What a %build_archspecific maindir=rom/kernel modname=kernel implies.
        let arch_roots = vec![(
            "${AROS_BUILD_DIR}/gen/rom/kernel/kernel/arch".to_owned(),
            "kernel-kernel".to_owned(),
            "pc-x86_64".to_owned(),
        )];

        let (decls, skipped) = collect_binary_objects(
            &joined,
            &scope,
            &dirs,
            &root,
            &rel,
            &["kernel-kernel".to_owned()],
            &arch_roots,
        );

        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(decls.len(), 1, "{decls:#?}");
        let decl = &decls[0];
        assert_eq!(decl.name, "smpbootstrap");
        assert_eq!(decl.sources, ["smpbootstrap"]);
        assert_eq!(decl.consumer, "kernel-kernel");
        assert_eq!(decl.arch_tag, "pc-x86_64");
        assert_eq!(decl.start, "0");
        assert_eq!(
            decl.output,
            "${AROS_BUILD_DIR}/gen/rom/kernel/kernel/arch/smpboot.bin.o"
        );
    }

    #[test]
    fn an_explicit_mmake_names_the_consumer_directly() {
        let root = root();
        let rel = PathBuf::from("arch/all-pc/bootstrap");
        let content = aros_common::read_source(&root.join(&rel).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&root);

        let (decls, skipped) = collect_binary_objects(
            &joined,
            &scope,
            &dirs,
            &root,
            &rel,
            &["kernel-bootstrap-pc".to_owned()],
            &[],
        );

        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(decls.len(), 1, "{decls:#?}");
        assert_eq!(decls[0].name, "vesa");
        assert_eq!(decls[0].consumer, "kernel-bootstrap-pc");
        assert_eq!(decls[0].start, "0x1000");
        assert_eq!(decls[0].sources, ["vesa"]);
        assert!(decls[0].arch_tag.is_empty());
    }

    #[test]
    fn a_declaration_with_no_consumer_is_reported_not_dropped() {
        let root = root();
        let rel = PathBuf::from("arch/x86_64-pc/kernel");
        let content = aros_common::read_source(&root.join(&rel).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&root);

        let (decls, skipped) =
            collect_binary_objects(&joined, &scope, &dirs, &root, &rel, &[], &[]);
        assert!(decls.is_empty());
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].contains("no target that links"), "{skipped:?}");
    }
}
