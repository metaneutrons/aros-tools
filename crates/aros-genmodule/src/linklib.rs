//! The module link library sources: stubs, autoinit, getlibbase.
//!
//! Why these matter more than their size suggests
//! ----------------------------------------------
//!
//! A module that calls another library does not resolve the call by ELF symbol
//! name. It calls a stub, and the stub reaches the callee through its library
//! base. genmodule writes one such stub per stack-call function into the
//! module's link library, and a consumer links the ones it uses.
//!
//! Without them every consumer carries the call as a dangling external, and
//! because every link here is `ld.lld -r` nothing complains. `ninja
//! symbol-audit` measured the result: 998 of 1011 built artefacts have
//! undefined symbols, 25006 references in total, and the commonest are exactly
//! the names these stubs would define -- CloseLibrary 431, OpenLibrary 422,
//! FreeVec 281, AllocVec 275.
//!
//! Faithful to `tools/genmodule/writestubs.c`, `writeautoinit.c` and
//! `writegetlibbase.c`. One deliberate omission is recorded at
//! `regcall_stubs_unsupported`.

use crate::varargs::Function;
use std::fmt::Write as _;

/// What a writer needs to know about the module.
///
/// Deliberately a separate borrow rather than the whole config: these three
/// writers touch a small, stable subset, and passing it explicitly keeps the
/// generated text checkable in a unit test.
pub struct ModuleFacts<'a> {
    /// `modulename`, the .conf stem.
    pub name: &'a str,
    /// `includename`, the SDK header base; defaults to the module name.
    pub include_name: &'a str,
    /// `libbase`, e.g. `SysBase`.
    pub lib_base: &'a str,
    /// `libbasetypeptrextern`, e.g. `struct Library *`. Includes the trailing
    /// space and star that the reference's field carries, so it concatenates
    /// directly onto an identifier.
    pub lib_base_type_extern: &'a str,
    /// `basename`, e.g. `Sys` for SysBase. The last argument of AROS_LC.
    pub basename: &'a str,
    /// The module suffix used in the `AROS_LIBSET` name, e.g. `library`.
    pub suffix: &'a str,
    /// Whether the config declared `options noincludes`.
    pub no_includes: bool,
    /// `##begin cdefprivate` lines, needed only in the noincludes path.
    pub cdef_private: &'a str,
    /// The module's major version, the default `.version` when no function
    /// states one.
    pub major_version: u32,
    /// `forcebase` entries, brought in so a parent opens those libraries.
    pub force_bases: &'a [String],
    /// `##begin startup` lines, written verbatim ahead of the autoinit banner.
    pub startup_lines: &'a str,
}

impl ModuleFacts<'_> {
    fn include_upper(&self) -> String {
        self.include_name.to_uppercase()
    }
}

/// The banner every generated file opens with.
///
/// The reference stamps no date here, and neither does this: a date would make
/// the output differ between builds for no gain.
fn banner(m: &ModuleFacts<'_>) -> String {
    format!(
        "/*\n    *** Automatically generated from '{}.conf'. Edits will be lost. ***\n*/\n",
        m.name
    )
}

/// Resolves `.version` to a number for every function.
///
/// config.c:415-432: if any function states a version the default for the rest
/// is 0; otherwise it is the module's major version. Getting this backwards
/// would make every stub demand the module's current version.
#[must_use]
pub fn resolved_version(m: &ModuleFacts<'_>, funcs: &[Function], f: &Function) -> u32 {
    if let Some(v) = f.declared_version {
        return v;
    }
    if funcs.iter().any(|g| g.declared_version.is_some()) {
        0
    } else {
        m.major_version
    }
}

/// The header block shared by every stub file.
fn stub_header(m: &ModuleFacts<'_>, is_rel: bool) -> String {
    let up = m.include_upper();
    let mut out = banner(m);
    out.push_str(
        "#ifdef  NOLIBINLINE\n#undef  NOLIBINLINE\n#endif  /* NOLIBINLINE */\n\
         #ifdef  NOLIBDEFINES\n#undef  NOLIBDEFINES\n#endif  /* NOLIBDEFINES */\n\
         #define NOLIBINLINE\n#define NOLIBDEFINES\n",
    );
    if is_rel {
        let _ = write!(
            out,
            "char *__aros_getoffsettable(void);\n\
             #ifndef __{up}_NOLIBBASE__\n\
             /* Do not include the libbase */\n\
             #define __{up}_NOLIBBASE__\n\
             #endif\n"
        );
    } else {
        let _ = write!(
            out,
            "/* Be sure that the libbases are included in the stubs file */\n\
             #ifdef  __NOLIBBASE__\n#undef  __NOLIBBASE__\n#endif  /* __NOLIBBASE__ */\n\
             #ifdef  __{up}_NOLIBBASE__\n#undef  __{up}_NOLIBBASE__\n\
             #endif  /* __{up}_NOLIBBASE__ */\n"
        );
    }

    if m.no_includes {
        out.push_str("#include <exec/types.h>\n#include <aros/system.h>\n\n");
        if !m.cdef_private.trim().is_empty() {
            out.push_str(m.cdef_private.trim_end());
            out.push('\n');
        }
        let _ = write!(
            out,
            "\n{}__aros_getbase_{}(void);\n\n",
            m.lib_base_type_extern, m.lib_base
        );
    } else {
        if is_rel {
            let _ = writeln!(out, "#define __{up}_RELLIBBASE__");
        }
        let _ = writeln!(out, "#include <proto/{}.h>", m.include_name);
    }

    out.push_str(
        "#include <stddef.h>\n\n#include <aros/cpu.h>\n#include <aros/genmodule.h>\n\
         #include <aros/libcall.h>\n#include <aros/symbolsets.h>\n\n",
    );
    out
}

/// One stack-call function's stub file.
///
/// The library-version requirement is emitted per function on purpose: each
/// stack-call stub is its own compilation unit, so a consumer that uses one
/// function does not inherit the highest version any other function needs.
#[must_use]
pub fn stack_stub(m: &ModuleFacts<'_>, funcs: &[Function], f: &Function, is_rel: bool) -> String {
    let mut out = stub_header(m, is_rel);
    let version = resolved_version(m, funcs, f);
    let _ = writeln!(
        out,
        "void __{}_{}_libreq(){{ AROS_LIBREQ({},{}); }}",
        f.name, m.lib_base, m.lib_base, version
    );
    let _ = writeln!(
        out,
        "AROS_GM_{}LIBFUNCSTUB({}, {}, {})",
        if is_rel { "REL" } else { "" },
        f.name,
        m.lib_base,
        f.lvo
    );
    for alias in &f.aliases {
        let _ = writeln!(out, "AROS_GM_LIBFUNCALIAS({}, {alias})", f.name);
    }
    out
}

/// The `AROS_LC` variant suffix for one function's argument list.
///
/// `writeutils.c:3`: runs of arguments of the same kind become `<n>`, `QUAD<n>`
/// or `DOUBLE<n>`, concatenated without a separator, so one quad followed by two
/// normal arguments gives `QUAD12`. An empty list gives `0`.
///
/// The kind comes from the register spec, not from the C type alone: a register
/// pair means the value occupies two, and `double` distinguishes DOUBLE from
/// QUAD (writestubs.c:196-204).
fn lc_suffix(f: &Function) -> String {
    if f.args.is_empty() {
        return "0".to_owned();
    }
    #[derive(PartialEq, Clone, Copy)]
    enum Kind {
        Normal,
        Quad,
        Double,
    }
    let kinds: Vec<Kind> = f
        .args
        .iter()
        .map(|a| {
            if a.reg.as_deref().is_some_and(|r| r.contains('/')) {
                if a.ty.trim() == "double" {
                    Kind::Double
                } else {
                    Kind::Quad
                }
            } else {
                Kind::Normal
            }
        })
        .collect();

    let mut out = String::new();
    let mut current = kinds[0];
    let mut run = 0usize;
    let mut flush = |kind: Kind, run: usize, out: &mut String| match kind {
        Kind::Double => {
            let _ = write!(out, "DOUBLE{run}");
        }
        Kind::Quad => {
            let _ = write!(out, "QUAD{run}");
        }
        Kind::Normal => {
            let _ = write!(out, "{run}");
        }
    };
    for k in kinds {
        if k == current {
            run += 1;
        } else {
            flush(current, run, &mut out);
            current = k;
            run = 1;
        }
    }
    flush(current, run, &mut out);
    out
}

/// One register-call function, as a full C function body.
fn regcall_stub(m: &ModuleFacts<'_>, f: &Function) -> String {
    // Types are trimmed at every use. The parser keeps a declaration as
    // written, so `APTR ` would otherwise reach the generated C as
    // `APTR  AllocMem` and `AROS_LC2(APTR , ...)`.
    let ret = f.ret_type.trim();
    let is_void = matches!(ret, "void" | "VOID");
    let mut out = String::new();
    let decls: Vec<&str> = f.args.iter().map(|a| a.decl.trim()).collect();
    let _ = write!(out, "\n{} {}({})\n{{\n", ret, f.name, decls.join(", "));
    let _ = writeln!(
        out,
        "    {}AROS_LC{}{}({}, {},\\",
        if is_void { "" } else { "return " },
        lc_suffix(f),
        if is_void { "NR" } else { "" },
        ret,
        f.name
    );
    for a in &f.args {
        let reg = a.reg.as_deref().unwrap_or("");
        let ty = a.ty.trim();
        if let Some((first, second)) = reg.split_once('/') {
            // The reference truncates the first register to two characters.
            let first = if first.len() > 2 { &first[..2] } else { first };
            let _ = writeln!(
                out,
                "         AROS_LCA2({ty}, {}, {first}, {second}), \\",
                a.name
            );
        } else {
            let reg = if reg.len() > 2 { &reg[..2] } else { reg };
            let _ = writeln!(out, "         AROS_LCA({ty}, {}, {reg}), \\", a.name);
        }
    }
    let _ = write!(
        out,
        "                    {}, __aros_getbase_{}(), {}, {});\n}}\n",
        m.lib_base_type_extern, m.lib_base, f.lvo, m.basename
    );
    for alias in &f.aliases {
        let _ = writeln!(out, "AROS_GM_LIBFUNCALIAS({}, {alias})", f.name);
    }
    out
}

/// The shared `<mod>_regcall_stubs.c`.
///
/// All register-call stubs share one object file, unlike the stack-call ones.
/// writestubs.c:11-18 explains why: the linker pulls a whole object out of an
/// archive, and a register-call function is normally reached through an inline
/// or a define rather than the linklib, so the aggregation costs little.
///
/// This is what covers exec: all 153 of its functions carry register specs, so
/// exec produces no stack-call stubs at all, and the symbols the audit reports
/// most -- CloseLibrary, OpenLibrary, AllocVec, FreeVec, FindTask -- are all
/// here rather than in the per-function files.
#[must_use]
pub fn regcall_stubs(m: &ModuleFacts<'_>, funcs: &[Function], is_rel: bool) -> String {
    let mut out = stub_header(m, is_rel);
    for f in funcs {
        if f.stack_call || f.private {
            continue;
        }
        out.push_str(&regcall_stub(m, f));
    }
    out
}

/// Historical note kept for the record.
///
/// writestubs.c also writes one shared `<mod>_regcall_stubs.c` holding a full C
/// function per register-call entry, expanded through `AROS_LC<n>` with an
/// `AROS_LCA` per argument. That needs the per-argument register names, which
/// this parser reads only far enough to detect their presence, and the
/// argument-type grouping that picks the `AROS_LC` variant.
///
/// It is left out rather than approximated because a wrong register mapping
/// produces a stub that links and then corrupts arguments at runtime, which is
/// far worse than a missing symbol the audit can see. Stack-call functions are
/// the large majority and are what the audit reports as missing.
pub const REGCALL_STUBS_UNSUPPORTED: &str =
    "register-call stubs are not generated; see linklib::REGCALL_STUBS_UNSUPPORTED";

/// `<mod>_autoinit.c`, per `writeautoinit.c`.
#[must_use]
pub fn autoinit(m: &ModuleFacts<'_>, is_rel: bool) -> String {
    let mut out = String::new();
    // The startup block goes ahead of the banner, as in the reference.
    if !is_rel && !m.startup_lines.trim().is_empty() {
        out.push_str(m.startup_lines.trim_end());
        out.push('\n');
    }
    out.push_str(&banner(m));
    if !m.no_includes {
        let _ = writeln!(out, "#include <proto/{}.h>", m.include_name);
    }
    out.push_str("#include <aros/symbolsets.h>\n\n");
    let _ = writeln!(
        out,
        "AROS_{}LIBSET(\"{}.{}\", {}, {})",
        if is_rel { "REL" } else { "" },
        m.include_name,
        m.suffix,
        m.lib_base_type_extern,
        m.lib_base
    );
    let _ = writeln!(
        out,
        "AROS_IMPORT_ASM_SYM(int, dummy, __include{}librarieshandling);",
        if is_rel { "rel" } else { "" }
    );
    if !m.force_bases.is_empty() {
        out.push('\n');
        for base in m.force_bases {
            // Bringing in __aros_getbase_XXXBase() makes the parent open it.
            let _ = writeln!(
                out,
                "AROS_IMPORT_ASM_SYM(void *, __dummy_{base}, __aros_getbase_{base});"
            );
        }
    }
    out
}

/// `<mod>_getlibbase.c`, per `writegetlibbase.c`.
#[must_use]
pub fn getlibbase(m: &ModuleFacts<'_>, is_rel: bool) -> String {
    let mut out = banner(m);
    if is_rel {
        let _ = write!(
            out,
            "#include <exec/types.h>\n\
             char *__aros_getoffsettable(void);\n\
             extern IPTR __aros_rellib_offset_{base};\n\
             \n\
             {ty}__aros_getbase_{base}(void)\n{{\n\
             \x20   return *(({ty}*)(__aros_getoffsettable()+__aros_rellib_offset_{base}));\n}}\n",
            base = m.lib_base,
            ty = m.lib_base_type_extern
        );
    } else {
        let _ = write!(
            out,
            "extern {ty}{base};\n\
             \n\
             {ty}__aros_getbase_{base}(void)\n{{\n\
             \x20   return {base};\n}}\n",
            base = m.lib_base,
            ty = m.lib_base_type_extern
        );
    }
    out
}

/// Every link-library source for one module, as (filename, contents).
///
/// The stack-call stubs get one file each, which is the whole point: the linker
/// pulls a complete object out of an archive, so putting every stub in one file
/// would drag all of them into any consumer that used one.
#[must_use]
pub fn sources(m: &ModuleFacts<'_>, funcs: &[Function], is_rel: bool) -> Vec<(String, String)> {
    let rel = if is_rel { "rel" } else { "" };
    let mut out = Vec::new();
    for f in funcs {
        if !f.stack_call || f.private {
            continue;
        }
        out.push((
            format!("{}_{}_{rel}stub.c", m.name, f.name),
            stack_stub(m, funcs, f, is_rel),
        ));
    }
    if funcs.iter().any(|f| !f.stack_call && !f.private) {
        out.push((
            format!("{}_regcall_{rel}stubs.c", m.name),
            regcall_stubs(m, funcs, is_rel),
        ));
    }
    out.push((format!("{}_{rel}autoinit.c", m.name), autoinit(m, is_rel)));
    out.push((
        format!("{}_{rel}getlibbase.c", m.name),
        getlibbase(m, is_rel),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::{autoinit, getlibbase, resolved_version, sources, stack_stub, ModuleFacts};
    use crate::varargs::Function;

    fn facts() -> ModuleFacts<'static> {
        ModuleFacts {
            name: "exec",
            include_name: "exec",
            lib_base: "SysBase",
            lib_base_type_extern: "struct ExecBase *",
            basename: "Exec",
            suffix: "library",
            no_includes: false,
            cdef_private: "",
            major_version: 41,
            force_bases: &[],
            startup_lines: "",
        }
    }

    fn func(name: &str, lvo: u32) -> Function {
        Function {
            name: name.to_owned(),
            ret_type: "APTR".to_owned(),
            args: Vec::new(),
            private: false,
            novararg: false,
            lvo,
            stack_call: true,
            declared_version: None,
            aliases: Vec::new(),
        }
    }

    #[test]
    fn a_stack_stub_carries_the_macro_the_reference_emits() {
        let m = facts();
        let f = func("AllocMem", 33);
        let s = stack_stub(&m, std::slice::from_ref(&f), &f, false);
        assert!(
            s.contains("AROS_GM_LIBFUNCSTUB(AllocMem, SysBase, 33)"),
            "{s}"
        );
        assert!(
            s.contains("void __AllocMem_SysBase_libreq(){ AROS_LIBREQ(SysBase,41); }"),
            "{s}"
        );
        assert!(s.contains("#include <proto/exec.h>"), "{s}");
        // The non-rel header must undefine the nolibbase guards, or the stub
        // compiles without a library base and calls through nothing.
        assert!(s.contains("#undef  __EXEC_NOLIBBASE__"), "{s}");
    }

    #[test]
    fn the_rel_variant_uses_the_offset_table() {
        let m = facts();
        let f = func("AllocMem", 33);
        let s = stack_stub(&m, std::slice::from_ref(&f), &f, true);
        assert!(
            s.contains("AROS_GM_RELLIBFUNCSTUB(AllocMem, SysBase, 33)"),
            "{s}"
        );
        assert!(s.contains("char *__aros_getoffsettable(void);"), "{s}");
        assert!(s.contains("#define __EXEC_NOLIBBASE__"), "{s}");
    }

    #[test]
    fn aliases_follow_the_stub() {
        let m = facts();
        let mut f = func("AllocMem", 33);
        f.aliases = vec!["AllocMemAlias".to_owned()];
        let s = stack_stub(&m, std::slice::from_ref(&f), &f, false);
        assert!(
            s.contains("AROS_GM_LIBFUNCALIAS(AllocMem, AllocMemAlias)"),
            "{s}"
        );
    }

    #[test]
    fn one_declared_version_makes_zero_the_default_for_the_others() {
        // config.c:415-432. Reading it the other way round would make every
        // stub demand the module's current version.
        let m = facts();
        let mut a = func("Old", 5);
        let mut b = func("New", 6);
        b.declared_version = Some(45);
        let funcs = vec![a.clone(), b.clone()];
        assert_eq!(resolved_version(&m, &funcs, &funcs[0]), 0);
        assert_eq!(resolved_version(&m, &funcs, &funcs[1]), 45);
        // With no declaration anywhere the module's major version applies.
        a.declared_version = None;
        b.declared_version = None;
        let plain = vec![a, b];
        assert_eq!(resolved_version(&m, &plain, &plain[0]), 41);
    }

    #[test]
    fn autoinit_names_the_library_and_its_type() {
        let s = autoinit(&facts(), false);
        assert!(
            s.contains("AROS_LIBSET(\"exec.library\", struct ExecBase *, SysBase)"),
            "{s}"
        );
        assert!(
            s.contains("AROS_IMPORT_ASM_SYM(int, dummy, __includelibrarieshandling);"),
            "{s}"
        );
    }

    #[test]
    fn autoinit_forces_the_bases_a_parent_must_open() {
        let bases = vec!["DOSBase".to_owned()];
        let m = ModuleFacts {
            force_bases: &bases,
            ..facts()
        };
        let s = autoinit(&m, false);
        assert!(
            s.contains("AROS_IMPORT_ASM_SYM(void *, __dummy_DOSBase, __aros_getbase_DOSBase);"),
            "{s}"
        );
    }

    #[test]
    fn getlibbase_returns_the_base_directly_and_via_the_table() {
        let m = facts();
        let plain = getlibbase(&m, false);
        assert!(
            plain.contains("extern struct ExecBase *SysBase;"),
            "{plain}"
        );
        assert!(plain.contains("return SysBase;"), "{plain}");
        let rel = getlibbase(&m, true);
        assert!(rel.contains("__aros_rellib_offset_SysBase"), "{rel}");
        assert!(rel.contains("__aros_getoffsettable()"), "{rel}");
    }

    fn reg_func(name: &str, lvo: u32, ret: &str, args: &[(&str, &str, &str)]) -> Function {
        Function {
            name: name.to_owned(),
            ret_type: ret.to_owned(),
            args: args
                .iter()
                .map(|(ty, nm, reg)| crate::varargs::Arg {
                    decl: format!("{ty} {nm}"),
                    ty: (*ty).to_owned(),
                    name: (*nm).to_owned(),
                    reg: Some((*reg).to_owned()),
                })
                .collect(),
            private: false,
            novararg: false,
            lvo,
            stack_call: false,
            declared_version: None,
            aliases: Vec::new(),
        }
    }

    #[test]
    fn a_register_call_stub_matches_the_reference_shape() {
        // exec's AllocMem: two normal arguments in D0 and D1.
        let m = facts();
        let f = reg_func(
            "AllocMem",
            33,
            "APTR",
            &[("IPTR", "byteSize", "D0"), ("ULONG", "requirements", "D1")],
        );
        let s = super::regcall_stubs(&m, std::slice::from_ref(&f), false);
        assert!(
            s.contains("APTR AllocMem(IPTR byteSize, ULONG requirements)"),
            "{s}"
        );
        assert!(s.contains("return AROS_LC2(APTR, AllocMem,"), "{s}");
        assert!(s.contains("AROS_LCA(IPTR, byteSize, D0), \\"), "{s}");
        assert!(
            s.contains("struct ExecBase *, __aros_getbase_SysBase(), 33, Exec);"),
            "{s}"
        );
    }

    #[test]
    fn a_void_return_uses_the_nr_variant() {
        let m = facts();
        let f = reg_func("FreeMem", 35, "void", &[("APTR", "memoryBlock", "A1")]);
        let s = super::regcall_stubs(&m, std::slice::from_ref(&f), false);
        assert!(s.contains("    AROS_LC1NR(void, FreeMem,"), "{s}");
        assert!(!s.contains("return AROS_LC"), "{s}");
    }

    #[test]
    fn a_register_pair_becomes_lca2_and_names_the_run() {
        // writeutils.c:3 concatenates runs, so a quad followed by two normal
        // arguments gives QUAD12, not 3 and not QUAD1_2.
        let m = facts();
        let f = reg_func(
            "Seek64",
            40,
            "LONG",
            &[
                ("QUAD", "pos", "D0/D1"),
                ("LONG", "mode", "D2"),
                ("LONG", "flags", "D3"),
            ],
        );
        let s = super::regcall_stubs(&m, std::slice::from_ref(&f), false);
        assert!(s.contains("AROS_LCQUAD12(LONG, Seek64,"), "{s}");
        assert!(s.contains("AROS_LCA2(QUAD, pos, D0, D1), \\"), "{s}");
    }

    #[test]
    fn a_double_in_a_register_pair_is_named_double() {
        let m = facts();
        let f = reg_func("SPFix", 41, "LONG", &[("double", "x", "D0/D1")]);
        let s = super::regcall_stubs(&m, std::slice::from_ref(&f), false);
        assert!(s.contains("AROS_LCDOUBLE1(LONG, SPFix,"), "{s}");
    }

    #[test]
    fn a_register_call_module_gets_one_shared_file() {
        let m = facts();
        let funcs = vec![
            reg_func("AllocMem", 33, "APTR", &[("IPTR", "n", "D0")]),
            reg_func("FreeMem", 34, "void", &[("APTR", "p", "A1")]),
        ];
        let files: Vec<String> = sources(&m, &funcs, false)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(
            files.contains(&"exec_regcall_stubs.c".to_owned()),
            "{files:?}"
        );
        // No per-function file for a register-call entry.
        assert!(!files.iter().any(|f| f.contains("AllocMem")), "{files:?}");
        assert_eq!(files.len(), 3);
    }

    #[test]
    fn one_file_per_stack_stub_and_none_for_register_calls() {
        let m = facts();
        let mut reg = func("RegOnly", 40);
        reg.stack_call = false;
        let mut priv_fn = func("Hidden", 41);
        priv_fn.private = true;
        let funcs = vec![func("AllocMem", 33), func("FreeMem", 34), reg, priv_fn];
        let files: Vec<String> = sources(&m, &funcs, false)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(
            files.contains(&"exec_AllocMem_stub.c".to_owned()),
            "{files:?}"
        );
        assert!(
            files.contains(&"exec_FreeMem_stub.c".to_owned()),
            "{files:?}"
        );
        assert!(files.contains(&"exec_autoinit.c".to_owned()), "{files:?}");
        assert!(files.contains(&"exec_getlibbase.c".to_owned()), "{files:?}");
        // A register-call function gets no separate file, and a private one no
        // public stub at all.
        assert!(!files.iter().any(|f| f.contains("RegOnly")), "{files:?}");
        assert!(!files.iter().any(|f| f.contains("Hidden")), "{files:?}");
        // The register-call entry lands in the one shared file instead.
        assert!(
            files.contains(&"exec_regcall_stubs.c".to_owned()),
            "{files:?}"
        );
        assert_eq!(files.len(), 5);
    }
}
