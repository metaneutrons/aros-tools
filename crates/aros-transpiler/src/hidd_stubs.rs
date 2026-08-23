//! `%make_hidd_stubs`: the six declarations that fill `libhiddstubs.a`.
//!
//! A HIDD's public API is a set of hand-written stubs that turn a call into an
//! `OOP_DoMethod`. `config/make.tmpl:3551` compiles each declaration's
//! `$(STUBS)` into `$(GENDIR)/lib/hidd/`, and
//! `compiler/libhiddstubs/mmakefile.src` archives whatever is in that directory
//! into `libhiddstubs.a`:
//!
//! ```text
//! HIDD_STUBS_OBJ := $(strip $(call WILDCARD, $(GENDIR)/lib/hidd/*.o))
//! $(HIDD_LIB) : $(HIDD_STUBS_OBJ)
//!         %mklib_q from=$^
//! ```
//!
//! Nothing modelled the macro, so 61 declarations that state
//! `uselibs=hiddstubs` had no archive to link -- reported all along in
//! `generated_targets.unresolved-uselibs.txt`. The visible consequence was one
//! module: `serialmouse.hidd` kept `HIDD_Serial_NewUnit` undefined, and the ELF
//! loader refuses a whole boot over one unresolved symbol, so no package could
//! be passed to the kickstart at all.
//!
//! The wildcard needs no modelling. Its inputs are exactly the `$(STUBS)` of
//! every `%make_hidd_stubs` in the tree, which is known once the tree is parsed,
//! so the archive becomes one link library with those sources. The reference
//! compiles them with `$(CFLAGS) $(CPPFLAGS)` and *not* `$(USER_INCLUDES)` --
//! `%make_hidd_stubs` calls `%compile_q` directly rather than going through the
//! `%(mmake)_CFLAGS` lanes of `make.tmpl:1681` -- so the three declarations that
//! set `USER_INCLUDES` do not get it here either, and none of the six stub
//! sources includes a local header.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::parser::VarScope;

/// One `%make_hidd_stubs` declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiddStubsDecl {
    /// The `hidd=` name, which names the MetaMake target but not the source.
    pub hidd: String,
    /// Directory of the declaring mmakefile, relative to the source root.
    pub directory: String,
    /// Source stems from `$(STUBS)`, relative to the source root.
    pub sources: Vec<String>,
}

/// Collects the `%make_hidd_stubs` declarations of one mmakefile.
///
/// Returns the declarations and, for reporting, the ones whose sources could not
/// be resolved. Nothing is dropped silently.
pub fn collect_hidd_stubs(
    content: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    rel_dir: &Path,
) -> (Vec<HiddStubsDecl>, Vec<String>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let directory = rel_dir.to_string_lossy().replace('\\', "/");

    for body in crate::includes::directive_bodies_pub(content, "%make_hidd_stubs") {
        let Some(hidd) = crate::includes::arg_value(&body, "hidd") else {
            skipped.push(format!(
                "{directory}: %make_hidd_stubs without hidd=, which the macro \
                 requires (make.tmpl:3551 marks it /A)"
            ));
            continue;
        };
        let expressions = MakeExprContext::new(scope, dirs, usize::MAX, root, rel_dir);
        // `STUBS` is the macro's only source lane, and it is a file variable
        // rather than a macro argument.
        let stubs = match evaluate_make_expr("$(STUBS)", &expressions) {
            Ok(value) => value,
            Err(error) => {
                skipped.push(format!(
                    "{directory}: %make_hidd_stubs hidd={hidd} cannot resolve \
                     $(STUBS): {error}"
                ));
                continue;
            }
        };
        let mut sources = Vec::new();
        let mut unresolved = false;
        for stem in stubs.split_whitespace() {
            if stem.contains(['$', '*', ';']) {
                skipped.push(format!(
                    "{directory}: %make_hidd_stubs hidd={hidd} source `{stem}` \
                     is not a plain stem"
                ));
                unresolved = true;
                continue;
            }
            sources.push(if directory.is_empty() {
                stem.to_owned()
            } else {
                format!("{directory}/{stem}")
            });
        }
        if unresolved {
            continue;
        }
        if sources.is_empty() {
            skipped.push(format!(
                "{directory}: %make_hidd_stubs hidd={hidd} has an empty $(STUBS)"
            ));
            continue;
        }
        out.push(HiddStubsDecl {
            hidd,
            directory: directory.clone(),
            sources,
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

    fn collect(rel: &str) -> (Vec<HiddStubsDecl>, Vec<String>) {
        let root = root();
        let rel = PathBuf::from(rel);
        let content = aros_common::read_source(&root.join(&rel).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&root);
        collect_hidd_stubs(&joined, &scope, &dirs, &root, &rel)
    }

    #[test]
    fn the_serial_stubs_resolve_through_modname() {
        let (decls, skipped) = collect("workbench/hidds/serial");
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(decls.len(), 1, "{decls:#?}");
        assert_eq!(decls[0].hidd, "serial");
        assert_eq!(decls[0].sources, ["workbench/hidds/serial/serial_stubs"]);
    }

    #[test]
    fn the_hidd_name_is_not_the_source_name() {
        // `%make_hidd_stubs hidd=mstorage` with `STUBS := storage_stubs`, so a
        // model that built the file name from hidd= would miss this one.
        let (decls, skipped) = collect("workbench/devs/USB/classes/MassStorage");
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(decls[0].hidd, "mstorage");
        assert_eq!(
            decls[0].sources,
            ["workbench/devs/USB/classes/MassStorage/storage_stubs"]
        );
    }

    fn collect_fixture(content: &str) -> (Vec<HiddStubsDecl>, Vec<String>) {
        let rel = PathBuf::from("fixture");
        let joined = join_continuations(content);
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(&root());
        collect_hidd_stubs(&joined, &scope, &dirs, &root(), &rel)
    }

    #[test]
    fn a_declaration_with_no_stubs_variable_is_reported() {
        let (decls, skipped) = collect_fixture("%make_hidd_stubs hidd=nothing\n");
        assert!(decls.is_empty(), "{decls:#?}");
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].contains("cannot resolve $(STUBS)"), "{skipped:?}");
    }

    #[test]
    fn a_declaration_with_an_empty_stubs_variable_is_reported() {
        let (decls, skipped) =
            collect_fixture("STUBS :=\n%make_hidd_stubs hidd=nothing\n");
        assert!(decls.is_empty(), "{decls:#?}");
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].contains("empty $(STUBS)"), "{skipped:?}");
    }
}
