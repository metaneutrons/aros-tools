//! Varargs convenience stubs for `defines/<module>.h`.
//!
//! AROS APIs come in pairs: a tag-list entry point that takes a
//! `struct TagItem *`, and a variadic convenience form. Only the former is
//! written by hand. `exec.conf` declares
//! `struct Task *NewCreateTaskA(struct TagItem *tags)`, while callers such as
//! `rom/devs/ahci/ahci_aros.h` use `NewCreateTask(TAG, value, ..., TAG_DONE)`.
//!
//! The variadic form is not source in the tree; it is generated. The reference
//! implementation is `tools/genmodule/writeincdefines.c`, which derives the
//! name from the tag-list function and emits a macro that collects the varargs
//! into an `IPTR` array before calling through.
//!
//! Naming rules, taken from that file:
//!
//! | tag-list function      | condition                          | variadic name |
//! |------------------------|------------------------------------|---------------|
//! | `FooA`                 | trailing `A`                       | `Foo`         |
//! | `FooTagList`           | trailing `TagList`                 | `FooTags`     |
//! | `FooArgs`              | last parameter named args/arglist  | `Foo`         |
//! | `Foo`                  | last parameter `struct TagItem *`  | `FooTags`     |
//!
//! Only this tag-list family (the reference's `varargtype == 1`) is generated
//! here. The `va_list` and `RAWARG` variants exist but are rare; they are
//! counted and reported rather than guessed at.

use std::fmt::Write as _;

/// One parameter of a function declaration.
#[derive(Debug, Clone)]
pub struct Arg {
    /// Declaration as written, e.g. `struct TagItem *tags`.
    pub decl: String,
    /// Type portion, e.g. `struct TagItem *`.
    pub ty: String,
    /// Parameter name, e.g. `tags`.
    pub name: String,
    /// The register this argument arrives in, from the declaration's second
    /// parenthesised group. `D0`, or `D0/D1` for a value that occupies two.
    ///
    /// Transcribed, never derived: the AROS_LCA macros take the register as a
    /// token, so passing through what the .conf says cannot introduce a mapping
    /// error. None means the declaration had no register group at all, which is
    /// what makes the function stack-call.
    pub reg: Option<String>,
}

/// A function from a `##begin functionlist` section.
#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub ret_type: String,
    pub args: Vec<Arg>,
    /// Marked `.private`; no public stub is generated.
    pub private: bool,
    /// Marked `.novararg`; the module opts out explicitly.
    pub novararg: bool,
    /// Library vector offset. Assigned sequentially from the module type's
    /// first LVO; a blank line or `.skip n` advances the counter without
    /// producing an entry.
    pub lvo: u32,
    /// True when the declaration carries no register specification.
    ///
    /// `tools/genmodule/config.c:2047` gives a function with registers the
    /// section's default calling convention and leaves one without them at the
    /// initial STACK. writestubs.c then emits a separate object file per
    /// stack-call function and one shared file for the rest, so this decides
    /// which of the two a function belongs to.
    pub stack_call: bool,
    /// `.version n` if the declaration carries one.
    ///
    /// Resolved to a concrete number later: if any function in the list states
    /// a version the default for the others is 0, otherwise it is the module's
    /// major version (config.c:415-432).
    pub declared_version: Option<u32>,
    /// `.alias name` entries, in declaration order.
    pub aliases: Vec<String>,
}

/// The first library vector offset for a module type.
///
/// Per `tools/genmodule/config.c:494`. The vectors below it are the standard
/// Open/Close/Expunge/ExtFunc entries, and a device adds BeginIO/AbortIO.
///
/// `options noresident` overrides this to 1 (config.c:1023). exec.library sets
/// it, and getting this wrong shifts every vector by four: the kernel resource
/// refers to AROS_SLIB_ENTRY(AllocMem, Kernel, LVOAllocMem) and expects
/// Kernel_33_AllocMem, not Kernel_37_AllocMem.
#[must_use]
pub const fn first_lvo(mod_type: &str, noresident: bool) -> u32 {
    if noresident {
        return 1;
    }
    match mod_type.as_bytes() {
        b"handler" | b"resource" => 1,
        // MCC belongs with MUI and MCP (config.c:521). Leaving it out put 20
        // Zune classes, all declared `modtype=mcc`, on the library fallback of 5
        // instead of 6, so their bases were sized one vector short.
        b"datatype" | b"mcc" | b"mui" | b"mcp" => 6,
        b"device" => 7,
        // library, usbclass, btclass, hidd, class, gadget, image
        _ => 5,
    }
}

/// Renders `defines/<name>_LVO.h`.
///
/// Follows `tools/genmodule/writeincdefines.c`. Private functions still consume
/// a vector but are not listed.
#[must_use]
pub fn render_lvo(include_name: &str, functions: &[Function]) -> String {
    let upper = include_name.to_uppercase();
    let mut out = String::with_capacity(1024);
    let _ = write!(
        out,
        "/* Auto-generated by AROS-NG genmodule v0.1.0 */\n\
         #ifndef DEFINES_LVO_{upper}_H\n\
         #define DEFINES_LVO_{upper}_H\n\
         \n\
         /*\n\
         \x20   Desc: Function LVO's for {include_name}\n\
         */\n\
         \n"
    );
    for f in functions {
        if f.private {
            continue;
        }
        let _ = writeln!(out, "#define LVO{:<20} {}", f.name, f.lvo);
    }
    let _ = write!(out, "\n#endif /* DEFINES_LVO_{upper}_H */\n");
    out
}

impl Function {
    /// The declaration as it appears in `clib/<mod>_protos.h`.
    #[must_use]
    pub fn signature(&self) -> String {
        let args = if self.args.is_empty() {
            "void".to_owned()
        } else {
            self.args
                .iter()
                .map(|a| a.decl.clone())
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!("{}{}({args})", self.ret_type, self.name)
    }
}

/// Which variadic family a function belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarargKind {
    /// Tag list collected into an IPTR array. The only kind generated here.
    TagList,
    /// `V`-prefixed function taking a `va_list`.
    VaList,
    /// `RAWARG` based.
    RawArg,
}

/// Derives the variadic name and kind for a function, if it has one.
#[must_use]
pub fn vararg_form(f: &Function) -> Option<(String, VarargKind)> {
    if f.private || f.novararg || f.args.is_empty() {
        return None;
    }
    let last = f.args.last()?;
    let name = f.name.as_str();

    // RAWARG cases first: they share the name shapes below but are a different
    // calling convention, and must not be mistaken for a tag list.
    let last_is_rawarg = last.decl.trim_start().starts_with("RAWARG");

    if let Some(stem) = name.strip_suffix('A') {
        if stem.is_empty() {
            return None;
        }
        let kind = if last_is_rawarg {
            VarargKind::RawArg
        } else {
            VarargKind::TagList
        };
        return Some((stem.to_owned(), kind));
    }

    if let Some(stem) = name.strip_suffix("TagList") {
        return Some((format!("{stem}Tags"), VarargKind::TagList));
    }

    if let Some(stem) = name.strip_suffix("Args") {
        let ln = last.name.to_ascii_lowercase();
        if ln == "args" || ln == "arglist" {
            return Some((stem.to_owned(), VarargKind::TagList));
        }
    }

    if let Some(stem) = name.strip_prefix('V') {
        if last.decl.trim_start().starts_with("va_list") {
            return Some((stem.to_owned(), VarargKind::VaList));
        }
        if last_is_rawarg {
            return Some((stem.to_owned(), VarargKind::RawArg));
        }
    }

    // Fall-through: a trailing `struct TagItem *`, optionally const-qualified.
    let t = last.ty.trim_start();
    let t = t.strip_prefix("const").map_or(t, str::trim_start);
    if let Some(rest) = t.strip_prefix("struct") {
        let rest = rest.trim_start();
        if let Some(rest) = rest.strip_prefix("TagItem") {
            if rest.trim_start().starts_with('*') {
                return Some((format!("{name}Tags"), VarargKind::TagList));
            }
        }
    }

    None
}

/// Splits a declaration into type and name.
///
/// The backward scan stops at whitespace or `*`, so `struct TagItem *tags`
/// yields the type `struct TagItem *` and the name `tags`.
fn split_decl(decl: &str) -> Option<(String, String)> {
    let d = decl.trim_end();
    let bytes = d.as_bytes();
    let mut i = d.len();
    while i > 0 {
        let c = bytes[i - 1];
        if c.is_ascii_whitespace() || c == b'*' {
            break;
        }
        i -= 1;
    }
    if i == 0 || i == d.len() {
        // No name, e.g. `void` or a bare type.
        return None;
    }
    Some((d[..i].to_owned(), d[i..].to_owned()))
}

/// Splits an argument list on commas at nesting depth zero.
fn split_args(list: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in list.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => {
                let t = cur.trim();
                if !t.is_empty() {
                    out.push(t.to_owned());
                }
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_owned());
    }
    out
}

/// Parses one `##begin functionlist` entry.
///
/// Accepted shapes, per `config.c`:
///
/// ```text
/// type name(argproto, ...)
/// type name(argproto, ...) (reg, ...)
/// ```
///
/// The register specification is parsed only far enough to be ignored: on the
/// targets built here arguments are passed on the stack. Matching the closing
/// parenthesis of the *argument* list matters, though; taking the last `(` on
/// the line silently truncates the prototype of any function declared without
/// a register specification.
#[must_use]
pub fn parse_function_line(line: &str) -> Option<Function> {
    let code = line.split('#').next()?.trim();
    if code.is_empty() || code.starts_with('.') {
        return None;
    }

    let open = code.find('(')?;
    let after = &code[open + 1..];
    let mut depth = 1i32;
    let mut close = None;
    for (i, ch) in after.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;

    let (ret_type, name) = split_decl(&code[..open])?;
    if name.is_empty() {
        return None;
    }

    let arg_text = after[..close].trim();
    let mut args = Vec::new();
    if !arg_text.is_empty() && arg_text != "void" {
        for decl in split_args(arg_text) {
            // A parameter may be unnamed; keep it, it just cannot be referenced.
            let (ty, nm) = split_decl(&decl).unwrap_or_else(|| (decl.clone(), String::new()));
            args.push(Arg {
                decl,
                ty,
                name: nm,
                reg: None,
            });
        }
    }

    // A register specification is a second parenthesised group after the
    // argument list. Its absence is what makes the function stack-call.
    let tail = after[close + 1..].trim_start();
    let stack_call = !tail.starts_with('(');
    if !stack_call {
        // Positional: the reference rejects a count mismatch outright
        // (config.c:2043), so a short list means the declaration is malformed
        // and the extra arguments keep None rather than borrowing a neighbour's
        // register.
        let inner = tail
            .strip_prefix('(')
            .and_then(|t| t.rfind(')').map(|e| &t[..e]))
            .unwrap_or("");
        for (arg, reg) in args.iter_mut().zip(inner.split(',')) {
            let reg = reg.trim();
            if !reg.is_empty() {
                arg.reg = Some(reg.to_owned());
            }
        }
    }

    Some(Function {
        name,
        ret_type,
        args,
        private: false,
        novararg: false,
        // Assigned by the caller, which tracks the running counter.
        lvo: 0,
        stack_call,
        declared_version: None,
        aliases: Vec::new(),
    })
}

/// Renders the tag-list stub for one function.
///
/// Shape follows `writedefinevararg` for `varargtype == 1`: the fixed
/// parameters are passed through, and the variadic tail is collected into an
/// `IPTR` array that is cast to the tag-list parameter's type.
fn render_taglist_stub(f: &Function, vararg_name: &str, mod_upper: &str) -> String {
    let mut out = String::with_capacity(512);
    let _ = writeln!(
        out,
        "\n#if !defined(NO_INLINE_STDARG) && !defined({mod_upper}_NO_INLINE_STDARG)"
    );

    let fixed = &f.args[..f.args.len() - 1];
    let last = f.args.last().expect("checked by caller");

    let params: Vec<String> = (1..=fixed.len()).map(|i| format!("arg{i}, ")).collect();
    let _ = writeln!(out, "#define {vararg_name}({}...) \\", params.concat());
    let _ = writeln!(out, "({{ \\");
    let _ = writeln!(
        out,
        "    const IPTR {}_args[] = {{ AROS_PP_VARIADIC_CAST2IPTR(__VA_ARGS__) }};\\",
        f.name
    );

    let mut call: Vec<String> = (1..=fixed.len()).map(|i| format!("(arg{i})")).collect();
    call.push(format!("({})({}_args)", last.ty.trim_end(), f.name));
    let _ = writeln!(out, "    {}({}); \\", f.name, call.join(", "));
    let _ = writeln!(out, "}})");
    let _ = writeln!(out, "#endif /* !NO_INLINE_STDARG */");
    out
}

/// Result of rendering a module's `defines/<mod>.h`.
#[derive(Debug, Default)]
pub struct DefinesOutput {
    pub text: String,
    /// Functions whose variadic form is a kind we do not generate.
    pub unsupported: Vec<String>,
}

/// What the register-call defines need beyond the function list.
pub struct DefinesContext<'a> {
    pub include_name: &'a str,
    pub lib_base: &'a str,
    /// `libbasetypeptrextern`, e.g. `struct ExecBase *`.
    pub lib_base_type_extern: &'a str,
    /// `basename`, the last AROS_LC argument, e.g. `Exec`.
    pub basename: &'a str,
    /// Vectors below this are the module skeleton's own.
    pub first_lvo: u32,
    /// Default `.version` when no function declares one.
    pub major_version: u32,
}

/// The per-function library-call defines, per `writeincdefines.c:235`.
///
/// This is what makes `AllocMem(a, b)` compile to a direct call through the
/// library base instead of a reference to an external symbol. Without it every
/// consumer emits a real call and leaves it undefined, and because every link
/// here is `ld.lld -r` nothing complains: `ninja symbol-audit` measured 25006
/// such references, and the commonest -- CloseLibrary, OpenLibrary, AllocVec,
/// FreeVec, FindTask -- are exec functions that arrive exactly this way.
///
/// exec declares no `linklibname`, so it has no link library and no stubs. For
/// it, these defines are the only mechanism.
///
/// Emitted for a function that is not private, not stack-call, and at or above
/// the module's first LVO, which is the reference's filter at
/// writeincdefines.c:117.
fn render_register_defines(cx: &DefinesContext<'_>, functions: &[Function]) -> String {
    let upper = cx.include_name.to_uppercase();
    // config.c:415-432: one declared version makes 0 the default for the rest.
    let any_declared = functions.iter().any(|f| f.declared_version.is_some());
    let mut out = String::new();

    for f in functions {
        if f.private {
            continue;
        }
        let _ = writeln!(out, "#define LVO{}          {}", f.name, f.lvo);
    }

    for f in functions {
        if f.private || f.stack_call || f.lvo < cx.first_lvo {
            continue;
        }
        let version = f
            .declared_version
            .unwrap_or(if any_declared { 0 } else { cx.major_version });
        let ret = f.ret_type.trim();
        let is_void = matches!(ret, "void" | "VOID");

        let _ = write!(
            out,
            "\n#if !defined(__{upper}_LIBAPI__) || ({version} <= __{upper}_LIBAPI__)\n"
        );

        // The write-back form takes the base as its first argument, so the
        // plain define below can pass the module's configured libbase while a
        // caller with its own base can use __<name>_WB directly.
        let _ = write!(out, "\n#define __{}_WB(__{}", f.name, cx.lib_base);
        for i in 1..=f.args.len() {
            let _ = write!(out, ", __arg{i}");
        }
        out.push_str(") ({\\\n");
        let _ = writeln!(out, "        AROS_LIBREQ({},{version})\\", cx.lib_base);
        let _ = writeln!(
            out,
            "        AROS_LC{}{}({}, {},\\",
            lc_suffix(f),
            if is_void { "NR" } else { "" },
            ret,
            f.name
        );
        for (i, a) in f.args.iter().enumerate() {
            let n = i + 1;
            let ty = a.ty.trim();
            let reg = a.reg.as_deref().unwrap_or("");
            if let Some((first, second)) = reg.split_once('/') {
                let first = if first.len() > 2 { &first[..2] } else { first };
                let _ = writeln!(
                    out,
                    "         AROS_LCA2({ty}, (__arg{n}), {first}, {second}), \\"
                );
            } else {
                let reg = if reg.len() > 2 { &reg[..2] } else { reg };
                let _ = writeln!(out, "         AROS_LCA({ty}, (__arg{n}), {reg}), \\");
            }
        }
        let _ = write!(
            out,
            "        {}, (__{}), {}, {});\\\n}})\n\n",
            cx.lib_base_type_extern, cx.lib_base, f.lvo, cx.basename
        );

        let _ = write!(out, "#define {}(", f.name);
        for i in 1..=f.args.len() {
            if i > 1 {
                out.push_str(", ");
            }
            let _ = write!(out, "arg{i}");
        }
        let _ = write!(out, ") \\\n    __{}_WB(__{upper}_LIBBASE", f.name);
        for i in 1..=f.args.len() {
            let _ = write!(out, ", (arg{i})");
        }
        out.push_str(")\n");

        for alias in &f.aliases {
            let _ = writeln!(out, "#define {alias} {}", f.name);
        }

        let _ = write!(
            out,
            "\n#endif /* !defined(__{upper}_LIBAPI__) || ({version} <= __{upper}_LIBAPI__) */\n"
        );
    }
    out
}

/// The `AROS_LC` variant suffix for one argument list.
///
/// `writeutils.c:3`: runs of like arguments become `<n>`, `QUAD<n>` or
/// `DOUBLE<n>`, concatenated, so a pair followed by two plain arguments gives
/// `QUAD12`. The kind comes from the register spec: a pair means the value
/// occupies two registers, and `double` separates DOUBLE from QUAD.
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

/// Renders `defines/<module>.h`: the library-call defines and the varargs stubs.
#[must_use]
pub fn render_defines(cx: &DefinesContext<'_>, functions: &[Function]) -> DefinesOutput {
    let include_name = cx.include_name;
    let lib_base = cx.lib_base;
    let upper = include_name.to_uppercase();
    let mut out = DefinesOutput::default();
    let t = &mut out.text;

    let _ = write!(
        t,
        "/* Auto-generated by AROS-NG genmodule v0.1.0 */\n\
         #ifndef DEFINES_{upper}_H\n\
         #define DEFINES_{upper}_H\n\
         \n\
         #include <aros/libcall.h>\n\
         #include <exec/types.h>\n\
         #include <aros/symbolsets.h>\n\
         #include <aros/preprocessor/variadic/cast2iptr.hpp>\n\
         \n\
         #if !defined(__{upper}_LIBBASE)\n\
         #    define __{upper}_LIBBASE {lib_base}\n\
         #endif\n\
         \n\
         __BEGIN_DECLS\n"
    );

    t.push_str(&render_register_defines(cx, functions));

    for f in functions {
        let Some((vararg_name, kind)) = vararg_form(f) else {
            continue;
        };
        match kind {
            VarargKind::TagList => {
                t.push_str(&render_taglist_stub(f, &vararg_name, &upper));
            }
            VarargKind::VaList | VarargKind::RawArg => {
                out.unsupported
                    .push(format!("{include_name}: {} ({kind:?})", f.name));
            }
        }
    }

    let _ = write!(t, "\n__END_DECLS\n\n#endif /* DEFINES_{upper}_H */\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A context for the tests that only exercise the varargs half.
    fn cx<'a>(include_name: &'a str, lib_base: &'a str) -> DefinesContext<'a> {
        DefinesContext {
            include_name,
            lib_base,
            lib_base_type_extern: "struct Library *",
            basename: "Test",
            first_lvo: 5,
            major_version: 1,
        }
    }

    fn arg(decl: &str, ty: &str, name: &str) -> Arg {
        Arg {
            decl: decl.to_owned(),
            ty: ty.to_owned(),
            name: name.to_owned(),
            reg: None,
        }
    }

    fn func(ret: &str, name: &str, args: Vec<Arg>) -> Function {
        Function {
            name: name.to_owned(),
            ret_type: ret.to_owned(),
            args,
            private: false,
            novararg: false,
            lvo: 0,
            stack_call: true,
            declared_version: None,
            aliases: Vec::new(),
        }
    }

    #[test]
    fn trailing_a_drops_the_a() {
        // exec.conf: struct Task *NewCreateTaskA(struct TagItem *tags)
        let f = func(
            "struct Task *",
            "NewCreateTaskA",
            vec![arg("struct TagItem *tags", "struct TagItem *", "tags")],
        );
        assert_eq!(
            vararg_form(&f),
            Some(("NewCreateTask".to_owned(), VarargKind::TagList))
        );
    }

    #[test]
    fn taglist_suffix_becomes_tags() {
        let f = func(
            "APTR",
            "OpenWindowTagList",
            vec![
                arg("APTR win", "APTR", "win"),
                arg("struct TagItem *tags", "struct TagItem *", "tags"),
            ],
        );
        assert_eq!(
            vararg_form(&f),
            Some(("OpenWindowTags".to_owned(), VarargKind::TagList))
        );
    }

    #[test]
    fn args_suffix_needs_a_matching_parameter_name() {
        let matching = func(
            "void",
            "DoStuffArgs",
            vec![arg("IPTR *args", "IPTR *", "args")],
        );
        assert_eq!(
            vararg_form(&matching),
            Some(("DoStuff".to_owned(), VarargKind::TagList))
        );

        // Same suffix but an unrelated parameter name: no stub.
        let other = func(
            "void",
            "DoStuffArgs",
            vec![arg("LONG count", "LONG", "count")],
        );
        assert_eq!(vararg_form(&other), None);
    }

    #[test]
    fn trailing_tagitem_pointer_gains_a_tags_form() {
        let f = func(
            "APTR",
            "MakeThing",
            vec![arg("struct TagItem *tags", "struct TagItem *", "tags")],
        );
        assert_eq!(
            vararg_form(&f),
            Some(("MakeThingTags".to_owned(), VarargKind::TagList))
        );
    }

    #[test]
    fn const_qualified_tagitem_is_recognised() {
        let f = func(
            "APTR",
            "MakeThing",
            vec![arg(
                "const struct TagItem *tags",
                "const struct TagItem *",
                "tags",
            )],
        );
        assert_eq!(
            vararg_form(&f),
            Some(("MakeThingTags".to_owned(), VarargKind::TagList))
        );
    }

    #[test]
    fn rawarg_is_classified_separately() {
        let f = func(
            "void",
            "VPrintfA",
            vec![arg("RAWARG args", "RAWARG", "args")],
        );
        assert_eq!(
            vararg_form(&f),
            Some(("VPrintf".to_owned(), VarargKind::RawArg))
        );
    }

    #[test]
    fn v_prefix_with_valist() {
        let f = func(
            "void",
            "VFPrintf",
            vec![
                arg("BPTR fh", "BPTR", "fh"),
                arg("va_list ap", "va_list", "ap"),
            ],
        );
        assert_eq!(
            vararg_form(&f),
            Some(("FPrintf".to_owned(), VarargKind::VaList))
        );
    }

    #[test]
    fn private_and_novararg_are_skipped() {
        let mut f = func(
            "void",
            "SecretA",
            vec![arg("struct TagItem *t", "struct TagItem *", "t")],
        );
        f.private = true;
        assert_eq!(vararg_form(&f), None);

        f.private = false;
        f.novararg = true;
        assert_eq!(vararg_form(&f), None);
    }

    #[test]
    fn plain_function_gets_no_stub() {
        let f = func("void", "Forbid", vec![]);
        assert_eq!(vararg_form(&f), None);
        let f = func("void", "Signal", vec![arg("LONG n", "LONG", "n")]);
        assert_eq!(vararg_form(&f), None);
    }

    #[test]
    fn rendered_stub_matches_the_reference_shape() {
        let f = func(
            "struct Task *",
            "NewCreateTaskA",
            vec![arg("struct TagItem *tags", "struct TagItem *", "tags")],
        );
        let out = render_defines(&cx("exec", "SysBase"), std::slice::from_ref(&f));
        let t = &out.text;

        assert!(t.contains("#include <aros/preprocessor/variadic/cast2iptr.hpp>"));
        assert!(t.contains("#if !defined(NO_INLINE_STDARG) && !defined(EXEC_NO_INLINE_STDARG)"));
        assert!(t.contains("#define NewCreateTask(...) \\"));
        assert!(t.contains(
            "    const IPTR NewCreateTaskA_args[] = { AROS_PP_VARIADIC_CAST2IPTR(__VA_ARGS__) };\\"
        ));
        assert!(t.contains("    NewCreateTaskA((struct TagItem *)(NewCreateTaskA_args)); \\"));
        assert!(t.contains("#endif /* !NO_INLINE_STDARG */"));
        assert!(out.unsupported.is_empty());
    }

    #[test]
    fn fixed_parameters_are_passed_through() {
        let f = func(
            "APTR",
            "OpenWindowTagList",
            vec![
                arg("APTR parent", "APTR", "parent"),
                arg("struct TagItem *tags", "struct TagItem *", "tags"),
            ],
        );
        let out = render_defines(&cx("intuition", "IntuitionBase"), std::slice::from_ref(&f));
        assert!(out.text.contains("#define OpenWindowTags(arg1, ...) \\"));
        assert!(out.text.contains(
            "    OpenWindowTagList((arg1), (struct TagItem *)(OpenWindowTagList_args)); \\"
        ));
    }

    #[test]
    fn unsupported_kinds_are_reported_not_emitted() {
        let f = func(
            "void",
            "VFPrintf",
            vec![
                arg("BPTR fh", "BPTR", "fh"),
                arg("va_list ap", "va_list", "ap"),
            ],
        );
        let out = render_defines(&cx("dos", "DOSBase"), std::slice::from_ref(&f));
        assert!(!out.text.contains("#define FPrintf"));
        assert_eq!(out.unsupported.len(), 1);
        assert!(out.unsupported[0].contains("VFPrintf"));
    }

    #[test]
    fn parses_a_line_with_a_register_specification() {
        // exec.conf
        let f =
            parse_function_line("struct Task *NewCreateTaskA(struct TagItem *tags) (A0)").unwrap();
        assert_eq!(f.name, "NewCreateTaskA");
        assert_eq!(f.ret_type, "struct Task *");
        assert_eq!(f.args.len(), 1);
        assert_eq!(f.args[0].decl, "struct TagItem *tags");
        assert_eq!(f.args[0].ty, "struct TagItem *");
        assert_eq!(f.args[0].name, "tags");
        assert_eq!(
            f.signature(),
            "struct Task *NewCreateTaskA(struct TagItem *tags)"
        );
    }

    #[test]
    fn parses_a_line_without_a_register_specification() {
        // Taking the last '(' on the line would truncate this prototype.
        let f = parse_function_line("APTR AllocMem(IPTR byteSize, ULONG requirements)").unwrap();
        assert_eq!(f.name, "AllocMem");
        assert_eq!(f.args.len(), 2);
        assert_eq!(
            f.signature(),
            "APTR AllocMem(IPTR byteSize, ULONG requirements)"
        );
    }

    #[test]
    fn parses_an_empty_argument_list() {
        let f = parse_function_line("void Forbid()").unwrap();
        assert_eq!(f.name, "Forbid");
        assert!(f.args.is_empty());
        assert_eq!(f.signature(), "void Forbid(void)");
    }

    #[test]
    fn handles_a_function_pointer_parameter() {
        let f = parse_function_line("void EnumDrivers(void (*cb)(APTR, LONG), APTR msg) (A0, A1)")
            .unwrap();
        assert_eq!(f.name, "EnumDrivers");
        assert_eq!(f.args.len(), 2, "args: {:?}", f.args);
        assert_eq!(f.args[1].name, "msg");
    }

    #[test]
    fn strips_a_trailing_comment() {
        let f = parse_function_line("void Signal(LONG n) (D0) # sends a signal").unwrap();
        assert_eq!(f.name, "Signal");
        assert_eq!(f.args.len(), 1);
    }

    #[test]
    fn rejects_directives_and_blank_lines() {
        assert!(parse_function_line(".private").is_none());
        assert!(parse_function_line("   ").is_none());
        assert!(parse_function_line("# comment").is_none());
    }

    #[test]
    fn first_lvo_follows_the_module_type() {
        assert_eq!(first_lvo("resource", false), 1);
        assert_eq!(first_lvo("handler", false), 1);
        assert_eq!(first_lvo("library", false), 5);
        assert_eq!(first_lvo("hidd", false), 5);
        assert_eq!(first_lvo("datatype", false), 6);
        assert_eq!(first_lvo("mcc", false), 6);
        assert_eq!(first_lvo("mcp", false), 6);
        assert_eq!(first_lvo("mui", false), 6);
        assert_eq!(first_lvo("device", false), 7);
        // An undescribed module gets the library form.
        assert_eq!(first_lvo("", false), 5);
    }

    #[test]
    fn noresident_starts_the_vectors_at_one() {
        // exec.conf declares "options noresident, noautoinit, noautolib".
        // Without this, LVOAllocMem comes out as 37 instead of 33.
        assert_eq!(first_lvo("library", true), 1);
        assert_eq!(first_lvo("device", true), 1);
    }

    #[test]
    fn lvo_header_lists_public_functions_with_their_vector() {
        let mut a = func("void", "Forbid", vec![]);
        a.lvo = 5;
        let mut b = func("void", "Secret", vec![]);
        b.lvo = 6;
        b.private = true;
        let mut c = func("void", "Permit", vec![]);
        c.lvo = 7;

        let h = render_lvo("exec", &[a, b, c]);
        assert!(h.contains("#ifndef DEFINES_LVO_EXEC_H"));
        assert!(h.contains("#define LVOForbid"));
        assert!(h.contains(" 5\n"));
        assert!(h.contains(" 7\n"));
        assert!(!h.contains("LVOSecret"), "private functions are not listed");
        assert!(h.ends_with("#endif /* DEFINES_LVO_EXEC_H */\n"));
    }
}
