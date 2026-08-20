//! OOP interface header generation (`include/interface/<Name>.h`).
//!
//! A `.conf` file may declare one or more `##begin interface` sections. Each
//! carries an id, a name, a method stub prefix and the base names for its
//! method and attribute ids, plus an attribute list and a method list. From
//! these the build needs an `interface/<Name>.h` providing the attribute and
//! method id enums and an inline stub per method.
//!
//! The output format follows `tools/genmodule/writeincinterfaces.c`; the code
//! that consumes these headers depends on the exact macro and enum names.
//!
//! Numbering matches the reference parser: attribute and method LVOs both
//! start at 0, a blank line advances the counter by one (leaving a gap), and
//! `.skip n` advances it by n. Comment lines do not advance it.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

/// One entry of an attribute or method list.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Attribute or method name.
    pub name: String,
    /// Return type for methods, value type for attributes.
    pub ty: String,
    /// Argument declarations, verbatim, methods only.
    pub args: Vec<String>,
    /// Library vector offset within this interface.
    pub lvo: u32,
    /// Trailing `#` comment, if any.
    pub comment: Option<String>,
}

/// A parsed `##begin interface` section.
///
/// Field names deliberately mirror the `.conf` keys (`interfaceid`,
/// `interfacename`), which is worth more here than avoiding the repetition.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, Default)]
pub struct Interface {
    pub interface_id: String,
    pub interface_name: String,
    pub method_stub: String,
    pub method_base: String,
    pub attribute_base: String,
    pub attributes: Vec<Entry>,
    pub methods: Vec<Entry>,
}

/// Splits a prototype or attribute line into type and name.
///
/// Mirrors the reference parser: the name is the trailing identifier before the
/// argument list (or before end of line for attributes), and the scan backwards
/// stops at whitespace *or* at `*`, so `OOP_Object *AddDriver` yields the type
/// `OOP_Object *` and the name `AddDriver`.
fn split_type_and_name(head: &str) -> Option<(String, String)> {
    let head = head.trim_end();
    let bytes = head.as_bytes();
    let mut i = head.len();
    while i > 0 {
        let c = bytes[i - 1];
        if c.is_ascii_whitespace() || c == b'*' {
            break;
        }
        i -= 1;
    }
    if i == 0 || i == head.len() {
        return None;
    }
    let name = head[i..].to_owned();
    let ty = head[..i].trim_end().to_owned();
    if ty.is_empty() || name.is_empty() {
        return None;
    }
    Some((ty, name))
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

/// Extracts the parameter name from a single argument declaration.
///
/// Handles function-pointer parameters, where the name sits inside the first
/// parenthesised group, e.g. `void (*callback)(APTR)` yields `callback`.
fn arg_name(arg: &str) -> Option<String> {
    let arg = arg.trim();
    let bytes = arg.as_bytes();
    let mut i = arg.len();
    while i > 0 && bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    let end = i;
    while i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
        i -= 1;
    }

    if i < end {
        return Some(arg[i..end].to_owned());
    }

    // Nothing trailing: a function pointer, take the identifier after the
    // first '(' and any leading '*'.
    let open = arg.find('(')?;
    let rest = &arg[open + 1..];
    let start = rest.find(|c: char| c.is_ascii_alphanumeric() || c == '_')?;
    let tail = &rest[start..];
    let stop = tail
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(tail.len());
    Some(tail[..stop].to_owned())
}

/// Parses one attribute or method list, honouring LVO gaps.
fn parse_entry_list(lines: &[&str], is_attribute: bool) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut lvo = 0u32;

    for raw in lines {
        let line = raw.trim_end();
        let trimmed = line.trim();

        // A blank line leaves a gap in the numbering.
        if trimmed.is_empty() {
            lvo += 1;
            continue;
        }
        // Comment lines do not advance the counter.
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(".skip") {
            if let Ok(n) = rest.split_whitespace().next().unwrap_or("").parse::<u32>() {
                lvo += n;
            }
            continue;
        }
        // Other directives (.alias, .function, .interface, ...) modify the
        // preceding entry and never introduce a new LVO.
        if trimmed.starts_with('.') {
            continue;
        }

        // Strip a trailing comment.
        let (code, comment) = trimmed.find('#').map_or((trimmed, None), |p| {
            let c = trimmed[p + 1..].trim();
            let comment = if c.is_empty() {
                None
            } else {
                Some(c.to_owned())
            };
            (trimmed[..p].trim_end(), comment)
        });
        if code.is_empty() {
            continue;
        }

        let (head, args) = if is_attribute {
            (code, Vec::new())
        } else {
            let Some(open) = code.find('(') else { continue };
            // Take the argument list up to the matching close paren.
            let after = &code[open + 1..];
            let mut depth = 1i32;
            let mut end = after.len();
            for (i, ch) in after.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            (&code[..open], split_args(&after[..end]))
        };

        let Some((ty, name)) = split_type_and_name(head) else {
            continue;
        };

        out.push(Entry {
            name,
            ty,
            args,
            lvo,
            comment,
        });
        lvo += 1;
    }

    out
}

/// Matches a section marker such as `##begin interface` or `##end  interface`.
///
/// The reference parser skips whitespace after `##` and after the keyword, and
/// the tree relies on that: `rom/hidds/pci/pci.conf` writes `##end  interface`
/// with two spaces. An exact comparison silently swallows the rest of the file
/// into that section.
fn is_section(line: &str, keyword: &str, name: &str) -> bool {
    let Some(rest) = line.trim().strip_prefix("##") else {
        return false;
    };
    let rest = rest.trim_start();
    let Some(rest) = rest.strip_prefix(keyword) else {
        return false;
    };
    rest.trim() == name
}

/// Parses every `##begin interface` section of a `.conf` file.
///
/// Section nesting matters: `##begin config` and `##begin methodlist` also
/// appear at the top level and inside `##begin class`, and only the ones inside
/// an interface section describe an interface.
#[must_use]
pub fn parse_interfaces(content: &str) -> Vec<Interface> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        if !is_section(lines[i], "begin", "interface") {
            i += 1;
            continue;
        }
        i += 1;

        let mut iface = Interface::default();
        while i < lines.len() {
            let t = lines[i].trim();
            if is_section(t, "end", "interface") {
                break;
            }
            if is_section(t, "begin", "config") {
                i += 1;
                while i < lines.len() && !is_section(lines[i], "end", "config") {
                    let l = lines[i].trim();
                    let mut it = l.split_whitespace();
                    if let (Some(key), Some(val)) = (it.next(), it.next()) {
                        match key {
                            "interfaceid" => val.clone_into(&mut iface.interface_id),
                            "interfacename" => val.clone_into(&mut iface.interface_name),
                            "methodstub" => val.clone_into(&mut iface.method_stub),
                            "methodbase" => val.clone_into(&mut iface.method_base),
                            "attributebase" => val.clone_into(&mut iface.attribute_base),
                            _ => {}
                        }
                    }
                    i += 1;
                }
            } else if is_section(t, "begin", "attributelist")
                || is_section(t, "begin", "methodlist")
            {
                let is_attr = is_section(t, "begin", "attributelist");
                let terminator = if is_attr {
                    "attributelist"
                } else {
                    "methodlist"
                };
                i += 1;
                let start = i;
                while i < lines.len() {
                    if is_section(lines[i], "end", terminator) {
                        break;
                    }
                    i += 1;
                }
                let body = &lines[start..i.min(lines.len())];
                if is_attr {
                    iface.attributes = parse_entry_list(body, true);
                } else {
                    iface.methods = parse_entry_list(body, false);
                }
            }
            i += 1;
        }

        if !iface.interface_name.is_empty() && !iface.interface_id.is_empty() {
            // Defaults per config.c:2303. Without them the generated header
            // emits `#if !defined()`, which is not valid preprocessor input.
            let name = iface.interface_name.clone();
            if iface.method_stub.is_empty() {
                name.clone_into(&mut iface.method_stub);
            }
            if iface.method_base.is_empty() {
                iface.method_base = format!("{name}Base");
            }
            if iface.attribute_base.is_empty() {
                iface.attribute_base = format!("{name}AttrBase");
            }
            out.push(iface);
        }
        i += 1;
    }

    out
}

/// Renders `interface/<Name>.h` for one interface.
///
/// Layout follows `tools/genmodule/writeincinterfaces.c`; consumers depend on
/// the exact macro and enum names, and on the column padding of the `IID_`,
/// attribute-base and attribute-id defines.
#[must_use]
pub fn render(iface: &Interface) -> String {
    let name = &iface.interface_name;
    let mb = &iface.method_base;
    let ab = &iface.attribute_base;
    let mut out = String::with_capacity(8192);

    let _ = write!(
        out,
        "#ifndef INTERFACE_{name}_H\n\
         #define INTERFACE_{name}_H\n\
         \n\
         /* Auto-generated by AROS-NG genmodule v0.1.0 */\n\
         \n\
         /*\n\
         \x20   Desc: interface inlines for {name}\n\
         */\n\
         \n\
         #include <exec/types.h>\n\
         #include <proto/oop.h>\n\
         \n"
    );
    out.push_str(&format!(
        "#define IID_{:<32} \"{}\"\n\n",
        name, iface.interface_id
    ));

    // The method base is resolved lazily through OOP on first use.
    out.push_str(&format!(
        "#if !defined({mb}) && !defined(__OOP_NOMETHODBASES__) && !defined(__{name}_NOMETHODBASE__)\n\
         #define {mb} {name}_GetMethodBase(__obj)\n\
         \n\
         static inline OOP_MethodID {name}_GetMethodBase(OOP_Object *obj)\n\
         {{\n\
         \x20   static OOP_MethodID {name}_mid;\n\
         \x20   if (!{name}_mid) {{\n\
         \x20       struct Library *OOPBase = (struct Library *)OOP_OCLASS(obj)->OOPBasePtr;\n\
         \x20       {name}_mid = OOP_GetMethodID(IID_{name}, 0);\n\
         \x20   }}\n\
         \x20   return {name}_mid;\n\
         }}\n\
         #endif\n\
         \n"
    ));

    if !iface.attributes.is_empty() {
        out.push_str(&format!("#define {ab:<32} __I{name}\n\n"));
        out.push_str(&format!(
            "#if !defined(__OOP_NOATTRBASES__) && !defined(__{name}_NOATTRBASE__)\n\
             extern OOP_AttrBase {ab};\n\
             #endif\n\
             \n\
             enum\n\
             {{\n"
        ));

        let mut max_lvo: i64 = -1;
        for a in &iface.attributes {
            out.push_str(&format!("    ao{name}_{} = {},", a.name, a.lvo));
            if let Some(c) = &a.comment {
                out.push_str(&format!("  /* {c} */"));
            }
            out.push('\n');
            max_lvo = max_lvo.max(i64::from(a.lvo));
        }
        if max_lvo >= 0 {
            out.push_str(&format!("    num_{name}_Attrs = {},\n}};\n\n", max_lvo + 1));
        }

        for a in &iface.attributes {
            out.push_str(&format!(
                "#define a{name}_{:<32} ({ab} + ao{name}_{})\n",
                a.name, a.name
            ));
        }
    }

    // Emitted unconditionally by the reference generator, attributes or not.
    out.push_str(&format!(
        "\n#define {name}_Switch(attr, idx) \\\n\
         if (((idx) = (attr) - {ab}) < num_{name}_Attrs) \\\n\
         switch (idx)\n\
         \n"
    ));

    if iface.methods.is_empty() {
        out.push_str(&format!("#endif /* INTERFACE_{name}_H */\n"));
        return out;
    }

    // Method ids.
    out.push_str("\nenum {\n");
    let mut max_lvo: i64 = -1;
    for m in &iface.methods {
        out.push_str(&format!("    mo{name}_{} = {},\n", m.name, m.lvo));
        max_lvo = max_lvo.max(i64::from(m.lvo));
    }
    out.push_str(&format!(
        "    num_{name}_Methods = {}\n}};\n\n",
        max_lvo + 1
    ));

    // Per-method message struct and inline stub.
    for m in &iface.methods {
        out.push_str(&format!(
            "struct p{name}_{}\n{{\n    OOP_MethodID mID;\n",
            m.name
        ));
        for arg in &m.args {
            out.push_str(&format!("    {arg};\n"));
        }
        out.push_str("};\n\n");

        if let Some(c) = &m.comment {
            out.push_str(&format!("/* {c} */\n"));
        }

        let stub = &iface.method_stub;
        out.push_str(&format!(
            "#define {stub}_{mname}(obj, args...) \\\n\
             \x20   ({{OOP_Object *__obj = obj;\\\n\
             \x20     {stub}_{mname}_({mb}, __obj ,##args); }})\n\
             \n",
            mname = m.name
        ));

        out.push_str(&format!(
            "static inline {} {stub}_{}_(OOP_MethodID __{mb}, OOP_Object *__obj",
            m.ty, m.name
        ));
        for arg in &m.args {
            out.push_str(&format!(", {arg}"));
        }
        out.push_str(&format!(
            ")\n{{\n    struct p{name}_{mname} p;\n    p.mID = __{mb} + mo{name}_{mname};\n",
            mname = m.name
        ));
        for arg in &m.args {
            if let Some(an) = arg_name(arg) {
                out.push_str(&format!("    p.{an} = {an};\n"));
            }
        }
        let ret = if m.ty.eq_ignore_ascii_case("void") {
            String::new()
        } else {
            format!("return ({})", m.ty)
        };
        out.push_str(&format!("    {ret}OOP_DoMethod(__obj, &p.mID);\n}}\n\n"));
    }

    out.push_str(&format!("#endif /* INTERFACE_{name}_H */\n"));
    out
}

/// Writes `interface/<Name>.h` into `out_inc`.
pub fn write_interface(iface: &Interface, out_inc: &Path) -> std::io::Result<()> {
    let dir = out_inc.join("interface");
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join(format!("{}.h", iface.interface_name)),
        render(iface),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const HIDD_HW: &str = r"
##begin interface
##begin config
interfaceid I_Hw
interfacename HW
methodstub    HW
methodbase    HWBase
attributebase HWAttrBase
##end config

##begin attributelist
BOOL            InUse     # [..G] Subsystem in use or not
CONST_STRPTR    ClassName # [I.G] Human-readable description
OOP_Object      *Device   # [..G] Query the Hardware Device Object
##end attributelist

##begin methodlist
OOP_Object *AddDriver(OOP_Class *driverClass, struct TagItem *tags)
BOOL RemoveDriver(OOP_Object *driverObject)
VOID EnumDrivers(struct Hook *callback, APTR hookMsg)
##end methodlist
##end interface
";

    #[test]
    fn parses_config_attributes_and_methods() {
        let ifs = parse_interfaces(HIDD_HW);
        assert_eq!(ifs.len(), 1);
        let i = &ifs[0];
        assert_eq!(i.interface_id, "I_Hw");
        assert_eq!(i.interface_name, "HW");
        assert_eq!(i.method_stub, "HW");
        assert_eq!(i.method_base, "HWBase");
        assert_eq!(i.attribute_base, "HWAttrBase");
        assert_eq!(i.attributes.len(), 3);
        assert_eq!(i.methods.len(), 3);
    }

    #[test]
    fn attribute_types_names_and_lvos() {
        let i = &parse_interfaces(HIDD_HW)[0];
        assert_eq!(i.attributes[0].ty, "BOOL");
        assert_eq!(i.attributes[0].name, "InUse");
        assert_eq!(i.attributes[0].lvo, 0);
        assert_eq!(i.attributes[1].name, "ClassName");
        assert_eq!(i.attributes[1].lvo, 1);
        // The backward scan stops at '*', so the star belongs to the type.
        assert_eq!(i.attributes[2].ty, "OOP_Object      *");
        assert_eq!(i.attributes[2].name, "Device");
        assert_eq!(i.attributes[2].lvo, 2);
        assert_eq!(
            i.attributes[0].comment.as_deref(),
            Some("[..G] Subsystem in use or not")
        );
    }

    #[test]
    fn method_signature_is_split_correctly() {
        let i = &parse_interfaces(HIDD_HW)[0];
        let m = &i.methods[0];
        assert_eq!(m.ty, "OOP_Object *");
        assert_eq!(m.name, "AddDriver");
        assert_eq!(
            m.args,
            vec!["OOP_Class *driverClass", "struct TagItem *tags"]
        );
        assert_eq!(m.lvo, 0);
        assert_eq!(i.methods[2].name, "EnumDrivers");
        assert_eq!(i.methods[2].lvo, 2);
    }

    #[test]
    fn blank_line_leaves_a_gap_in_numbering() {
        let src = "\
##begin interface
##begin config
interfaceid I_X
interfacename X
methodstub    X
methodbase    XBase
attributebase XAttrBase
##end config
##begin attributelist
ULONG A

ULONG B
##end attributelist
##end interface
";
        let i = &parse_interfaces(src)[0];
        assert_eq!(i.attributes[0].lvo, 0);
        assert_eq!(i.attributes[1].lvo, 2, "blank line must consume one LVO");
    }

    #[test]
    fn skip_directive_advances_numbering() {
        let src = "\
##begin interface
##begin config
interfaceid I_X
interfacename X
methodstub    X
methodbase    XBase
attributebase XAttrBase
##end config
##begin methodlist
void First()
.skip 3
void Second()
##end methodlist
##end interface
";
        let i = &parse_interfaces(src)[0];
        assert_eq!(i.methods[0].lvo, 0);
        assert_eq!(i.methods[1].lvo, 4);
    }

    #[test]
    fn comment_lines_do_not_consume_an_lvo() {
        let src = "\
##begin interface
##begin config
interfaceid I_X
interfacename X
methodstub    X
methodbase    XBase
attributebase XAttrBase
##end config
##begin attributelist
ULONG A
# just a note
ULONG B
##end attributelist
##end interface
";
        let i = &parse_interfaces(src)[0];
        assert_eq!(i.attributes[1].lvo, 1);
    }

    #[test]
    fn nested_class_sections_are_not_treated_as_interfaces() {
        // hiddclass.conf has a top-level methodlist and class sections whose
        // methodlists must not be picked up.
        let src = "\
##begin interface
##begin config
interfaceid I_Hidd
interfacename Hidd
methodstub    HIDD
methodbase    HiddBase
attributebase HiddAttrBase
##end config
##begin attributelist
UWORD Type
##end attributelist
##begin methodlist
##end   methodlist
##end interface

##begin methodlist
.interface Root
New
Dispose
##end methodlist

##begin class
##begin config
basename HW
##end config
##begin methodlist
.interface Root
New
##end methodlist
##end class
";
        let ifs = parse_interfaces(src);
        assert_eq!(ifs.len(), 1);
        assert_eq!(ifs[0].interface_name, "Hidd");
        assert_eq!(ifs[0].attributes.len(), 1);
        assert!(ifs[0].methods.is_empty(), "empty methodlist stays empty");
    }

    #[test]
    fn function_pointer_argument_name_is_found() {
        assert_eq!(
            arg_name("struct Hook *callback").as_deref(),
            Some("callback")
        );
        assert_eq!(arg_name("void (*fn)(APTR)").as_deref(), Some("fn"));
        assert_eq!(arg_name("APTR hookMsg").as_deref(), Some("hookMsg"));
    }

    #[test]
    fn rendered_header_has_the_expected_shape() {
        let i = &parse_interfaces(HIDD_HW)[0];
        let h = render(i);

        assert!(h.contains("#ifndef INTERFACE_HW_H"));
        assert!(h.contains("#define IID_HW                               \"I_Hw\""));
        assert!(h.contains("static inline OOP_MethodID HW_GetMethodBase(OOP_Object *obj)"));
        assert!(h.contains("extern OOP_AttrBase HWAttrBase;"));
        assert!(h.contains("    aoHW_InUse = 0,"));
        assert!(h.contains("    num_HW_Attrs = 3,"));
        assert!(h.contains("(HWAttrBase + aoHW_InUse)"));
        assert!(h.contains("#define HW_Switch(attr, idx)"));
        assert!(h.contains("    moHW_AddDriver = 0,"));
        assert!(h.contains("    num_HW_Methods = 3"));
        assert!(h.contains("struct pHW_AddDriver\n{\n    OOP_MethodID mID;\n    OOP_Class *driverClass;\n    struct TagItem *tags;\n};"));
        assert!(h.contains("static inline OOP_Object * HW_AddDriver_(OOP_MethodID __HWBase, OOP_Object *__obj, OOP_Class *driverClass, struct TagItem *tags)"));
        assert!(h.contains("    p.mID = __HWBase + moHW_AddDriver;"));
        assert!(h.contains("    p.driverClass = driverClass;"));
        assert!(h.contains("    return (OOP_Object *)OOP_DoMethod(__obj, &p.mID);"));
        assert!(h.ends_with("#endif /* INTERFACE_HW_H */\n"));
    }

    #[test]
    fn void_method_does_not_return_a_value() {
        let i = &parse_interfaces(HIDD_HW)[0];
        let h = render(i);
        // VOID EnumDrivers(...) -> reference treats VOID case-insensitively.
        let pos = h.find("EnumDrivers_(").expect("stub present");
        let tail = &h[pos..];
        assert!(tail.contains("    OOP_DoMethod(__obj, &p.mID);"));
    }

    #[test]
    fn tolerates_extra_whitespace_in_section_markers() {
        // rom/hidds/pci/pci.conf uses "##end  interface" with two spaces.
        let src = "\
##begin interface
##begin config
interfaceid   hidd.pci.driver
interfacename Hidd_PCIDriver
methodstub    HIDD_PCIDriver
methodbase    HiddPCIDriverBase
attributebase HiddPCIDriverAttrBase
##end config
##begin attributelist
ULONG A
##end  attributelist
##begin methodlist
BOOL AddInterrupt(OOP_Object *device)
##end  methodlist
##end  interface

##begin interface
##begin config
interfaceid   hidd.pci.device
interfacename Hidd_PCIDevice
##end config
##begin attributelist
ULONG B
##end attributelist
##end interface
";
        let ifs = parse_interfaces(src);
        assert_eq!(ifs.len(), 2, "both interfaces must be found");
        assert_eq!(ifs[0].interface_name, "Hidd_PCIDriver");
        assert_eq!(ifs[0].methods.len(), 1);
        assert_eq!(ifs[1].interface_name, "Hidd_PCIDevice");
    }

    #[test]
    fn missing_bases_fall_back_to_interface_name() {
        // rom/devs/ata/ata.conf declares Hidd_ATAUnit without methodbase.
        let src = "\
##begin interface
##begin config
interfaceid   hidd.ata.unit
interfacename Hidd_ATAUnit
attributebase HiddATAUnitAB
##end config
##begin attributelist
ULONG XferModes
##end attributelist
##end interface
";
        let i = &parse_interfaces(src)[0];
        assert_eq!(i.method_stub, "Hidd_ATAUnit");
        assert_eq!(i.method_base, "Hidd_ATAUnitBase");
        // An explicit attributebase is kept.
        assert_eq!(i.attribute_base, "HiddATAUnitAB");

        let h = render(i);
        assert!(
            !h.contains("!defined()"),
            "empty macro name would be invalid preprocessor input"
        );
        assert!(h.contains("#if !defined(Hidd_ATAUnitBase)"));
    }
}
