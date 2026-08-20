mod interfaces;
mod varargs;

use clap::Parser;
use rayon::prelude::*;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(
    name = "aros-genmodule",
    author = "AROS Development Team & Fabian Schmieder (@metaneutrons)",
    version = "0.1.0",
    about = "Pure Rust replacement for genmodule (.conf parser & SDK proto generator)"
)]
struct Args {
    /// Source directory to scan for .conf files
    #[arg(short, long, default_value = ".")]
    scan_dir: PathBuf,

    /// Target SDK include directory (e.g. build/pc-x86_64/SDK/include)
    #[arg(short, long)]
    output_inc: PathBuf,

    /// Architecture directories that apply to the configured target, e.g.
    /// `x86_64-pc all-pc x86_64-all all-native`.
    ///
    /// Without this every `arch/<cpu>-<platform>/` subtree is scanned, so a
    /// module that exists for several architectures under the same name (audio,
    /// trackdisk, bwfm) overwrites the target's own generated headers.
    #[arg(long, value_delimiter = ' ')]
    arch_dirs: Vec<String>,

    /// Root for per-module generated headers (e.g. build/pc-x86_64/gen).
    ///
    /// `<mod>_libdefs.h` is module-private: it carries LIBBASE, LIBBASETYPE and
    /// the module's `cdefprivate` block. Twenty-six .conf stems occur more than
    /// once in the tree, so a flat SDK layout makes them overwrite each other.
    /// The reference build emits this file into the module's own build
    /// directory; mirroring that keeps them apart.
    #[arg(long)]
    output_gen: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct ConfModule {
    name: String,
    lib_base: String,
    lib_base_type: String,
    /// `libbasetypeextern` if the config states one; the type is otherwise
    /// derived from the module type by extern_base_type().
    explicit_base_type_extern: Option<String>,
    cdef: String,
    /// Contents of `##begin cdefprivate`. The reference generator writes these
    /// lines verbatim into `<mod>_libdefs.h`; they are how a module pulls in the
    /// header that defines its library base struct, so omitting them leaves
    /// `struct <X>Base` incomplete at every use site.
    cdef_private: String,
    functions: Vec<varargs::Function>,
    /// `version <major>.<minor>` from the config section.
    major_version: u32,
    minor_version: u32,
    /// `residentpri`, used for RESIDENTPRI and to derive RESIDENTFLAGS.
    resident_pri: i32,
    /// `date` from the config section. MOD_DATE_STRING is consumed by real code
    /// (rom/exec/taggedopenlibrary.c), so it must be present. Left empty when
    /// the .conf does not state one, rather than stamped with the build date,
    /// which would make output non-reproducible.
    date: String,
    /// Whether the config declared `options autoinit`.
    auto_init: bool,
    /// Whether the config declared `options noresident`, which starts the
    /// library vectors at 1 instead of the module type's default.
    no_resident: bool,
    /// Module type (`library`, `device`, `resource`, `hidd`, `handler`, ...).
    /// Not part of the .conf; taken from the sibling mmakefile.src.
    mod_type: String,
    /// Fields the module init code needs to reach through the library base.
    sysbase_field: Option<String>,
    oopbase_field: Option<String>,
    seglist_field: Option<String>,
    /// Interfaces declared in this .conf.
    interfaces: Vec<interfaces::Interface>,
    /// `includename` from the config section; the SDK header base name, which
    /// defaults to the module name.
    include_name: String,
    /// Directory of the .conf, relative to the scan root. Used to give the
    /// module-private headers a unique location.
    rel_dir: PathBuf,
}

/// Whether a `.conf` below `arch/` applies to the configured target.
///
/// Paths outside `arch/` always apply. Inside it, the directory right below
/// `arch/` must be one of the architecture directories the caller listed; with
/// no list given nothing is filtered, which keeps the tool usable standalone.
fn arch_dir_applies(rel: &Path, arch_dirs: &[String]) -> bool {
    if arch_dirs.is_empty() {
        return true;
    }
    let mut parts = rel.components().map(|c| c.as_os_str().to_string_lossy());
    if parts.next().as_deref() != Some("arch") {
        return true;
    }
    parts
        .next()
        .is_none_or(|dir| arch_dirs.iter().any(|a| *a == dir))
}

/// Whether the module publishes `proto/`, `clib/` and `defines/` headers.
///
/// Mirrors `tools/genmodule/config.c:711`. Modules without a public API do not
/// export headers, which is also what keeps same-named `.conf` files in
/// different subsystems from overwriting each other's SDK headers.
fn exports_public_headers(module: &ConfModule) -> bool {
    let has_funcs = !module.functions.is_empty();
    let has_cdef = !module.cdef.trim().is_empty();
    match module.mod_type.as_str() {
        "library" | "resource" => true,
        "handler" | "mcc" | "mui" | "mcp" | "usbclass" | "btclass" => has_funcs || has_cdef,
        // A device only exports if it has an API or a non-standard base type.
        "device" => has_funcs || has_cdef || extern_base_type(module) != "struct Device *",
        // class/gadget/image/datatype/hidd need a function list; so does an
        // undescribed module, so one the build description does not cover
        // cannot claim the SDK namespace.
        _ => has_funcs,
    }
}

/// The library base pointer type as seen from *outside* the module.
///
/// Per `tools/genmodule/config.c:1375` this follows the module type, not
/// `libbasetype`: that one is the module's private view. `acpica.conf` declares
/// `libbasetype struct ACPICABase` but is a library, so consumers of
/// `<proto/acpica.h>` must see `struct Library *` -- otherwise their own
/// `struct Library *ACPICABase` collides with the declaration.
///
/// An explicit `libbasetypeextern` always wins.
fn extern_base_type(module: &ConfModule) -> String {
    if let Some(t) = &module.explicit_base_type_extern {
        return format!("{t} *");
    }
    match module.mod_type.as_str() {
        "device" => "struct Device *".to_owned(),
        // APTR is already a pointer; the reference stores it without a star.
        "handler" | "resource" => "APTR ".to_owned(),
        // library, class, mui, mcp, mcc, gadget, image, datatype, usbclass,
        // btclass, hidd -- and anything the build description does not name.
        _ => "struct Library *".to_owned(),
    }
}

/// Splits a config line into key and value at the first run of whitespace.
///
/// Config sections use either spaces or tabs as the separator; 46 `.conf` files
/// in the tree use a tab, so matching on `"key "` silently drops their values
/// and the generated header falls back to defaults.
fn conf_key_value(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    let idx = line.find(char::is_whitespace)?;
    let (k, v) = line.split_at(idx);
    let v = v.trim();
    if k.is_empty() || v.is_empty() {
        return None;
    }
    Some((k, v))
}

/// Reads `modtype=` for `modname` from the mmakefile.src next to the .conf.
///
/// The module type lives in the build description, not in the .conf, because
/// one directory can build several modules. Without it the generated
/// MOD_NAME_STRING would call every module a `.library`.
fn read_mod_type(conf_path: &Path, module_name: &str) -> Option<String> {
    let mmakefile = conf_path.parent()?.join("mmakefile.src");
    let content = fs::read_to_string(mmakefile).ok()?;
    // Directives span continuation lines, so flatten before matching.
    let flat = content.replace("\\\n", " ");
    let mut best: Option<String> = None;
    for block in flat.split("%build_module").skip(1) {
        let head: String = block.chars().take(600).collect();
        let name = head
            .split_whitespace()
            .find_map(|t| t.strip_prefix("modname="))
            .map(str::to_owned);
        let ty = head
            .split_whitespace()
            .find_map(|t| t.strip_prefix("modtype="))
            .map(str::to_owned);
        match (name.as_deref(), ty) {
            (Some(n), Some(t)) if n == module_name => return Some(t),
            (_, Some(t)) if best.is_none() => best = Some(t),
            _ => {}
        }
    }
    best
}

/// Suffix and separator used to build the module's run-time name.
fn mod_name_string(module: &ConfModule) -> String {
    let suffix = if module.mod_type.is_empty() {
        "library"
    } else {
        module.mod_type.as_str()
    };
    // Handlers are named `<name>-handler`, everything else `<name>.<type>`.
    let sep = if suffix == "handler" { '-' } else { '.' };
    format!("{}{}{}", module.name.to_lowercase(), sep, suffix)
}

/// RESIDENTFLAGS, following the thresholds in writeinclibdefs.c.
fn resident_flags(module: &ConfModule) -> String {
    let mut flags: Vec<&str> = Vec::new();
    if module.resident_pri >= 105 {
        flags.push("RTF_SINGLETASK");
    } else if module.resident_pri >= -60 {
        flags.push("RTF_COLDSTART");
    } else if module.resident_pri < -120 {
        flags.push("RTF_AFTERDOS");
    }
    if module.auto_init {
        flags.push("RTF_AUTOINIT");
    }
    if flags.is_empty() {
        "0".to_owned()
    } else {
        flags.join("|")
    }
}

/// The reference's default basename: the module name with its first letter
/// capitalised (`tools/genmodule/config.c:1334`).
///
/// This is what makes `LIBBASE` usable inside the module. A library function's
/// last parameter is the typed base, named after the basename -- for layers
/// that is `struct LayersBase *LayersBase`. `LIBBASE` expands to that name, so
/// inside a function it binds to the typed parameter and shadows the
/// `struct Library *` the proto header declares. Lowercasing the first letter
/// breaks the match, and every `LIBBASE->field` access then resolves against
/// `struct Library` instead.
fn default_basename(module_name: &str) -> String {
    let mut chars = module_name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn parse_conf(path: &Path, root: &Path) -> Option<ConfModule> {
    let content = fs::read_to_string(path).ok()?;
    let stem = path.file_stem()?.to_string_lossy().to_string();

    let mut module = ConfModule {
        name: stem.clone(),
        lib_base: format!("{}Base", default_basename(&stem)),
        // Left empty on purpose: the default depends on libbasetypeextern,
        // which may be read later in the config section. Resolved below.
        lib_base_type: String::new(),
        mod_type: read_mod_type(path, &stem).unwrap_or_default(),
        rel_dir: path
            .parent()
            .and_then(|d| d.strip_prefix(root).ok())
            .map_or_else(PathBuf::new, Path::to_path_buf),
        interfaces: interfaces::parse_interfaces(&content),
        ..ConfModule::default()
    };

    // Vector numbering starts below the module's own functions; the standard
    // Open/Close/Expunge vectors occupy the slots before it. Initialised on
    // entering the function list, because the config section that may override
    // the starting point with `options noresident` is read first.
    let mut lvo = 0u32;
    let mut lvo_ready = false;
    let mut section = "";
    // `##begin interface` and `##begin class` nest their own config and
    // methodlist sections; reading those as the module's would let an
    // interface's keys overwrite the module's version, basename and so on.
    let mut nesting = 0usize;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("##begin interface") || trimmed.starts_with("##begin class") {
            nesting += 1;
            continue;
        }
        if trimmed.starts_with("##end interface") || trimmed.starts_with("##end class") {
            nesting = nesting.saturating_sub(1);
            continue;
        }
        if nesting > 0 {
            // Interfaces are parsed separately by the interfaces module.
            continue;
        }
        if trimmed == "##begin config" {
            section = "config";
        } else if trimmed == "##end config" {
            section = "";
        } else if trimmed == "##begin cdef" {
            section = "cdef";
        } else if trimmed == "##end cdef" {
            section = "";
        } else if trimmed == "##begin cdefprivate" {
            section = "cdefprivate";
        } else if trimmed == "##end cdefprivate" {
            section = "";
        } else if trimmed == "##begin functionlist" {
            section = "functions";
            if !lvo_ready {
                lvo = varargs::first_lvo(&module.mod_type, module.no_resident);
                lvo_ready = true;
            }
        } else if trimmed == "##end functionlist" {
            section = "";
        } else if section == "config" {
            let Some((key, val)) = conf_key_value(trimmed) else {
                continue;
            };
            match key {
                "basename" => module.lib_base = format!("{val}Base"),
                "libbase" => module.lib_base = val.to_string(),
                "libbasetypeextern" => {
                    module.explicit_base_type_extern = Some(val.to_string());
                }
                "libbasetype" => module.lib_base_type = val.to_string(),
                "includename" => module.include_name = val.to_string(),
                "date" => module.date = val.to_string(),
                "version" => {
                    let mut it = val.split('.');
                    if let Some(maj) = it.next().and_then(|x| x.trim().parse::<u32>().ok()) {
                        module.major_version = maj;
                        module.minor_version = it
                            .next()
                            .and_then(|x| x.trim().parse::<u32>().ok())
                            .unwrap_or(0);
                    }
                }
                "residentpri" => {
                    if let Ok(v) = val.parse::<i32>() {
                        module.resident_pri = v;
                    }
                }
                "options" => {
                    for opt in val.split([',', ' ']) {
                        match opt.trim() {
                            "autoinit" => module.auto_init = true,
                            "noresident" => module.no_resident = true,
                            _ => {}
                        }
                    }
                }
                "sysbase_field" => module.sysbase_field = Some(val.to_string()),
                "oopbase_field" => module.oopbase_field = Some(val.to_string()),
                "seglist_field" => module.seglist_field = Some(val.to_string()),
                _ => {}
            }
        } else if section == "cdef" {
            module.cdef.push_str(line);
            module.cdef.push('\n');
        } else if section == "cdefprivate" {
            module.cdef_private.push_str(line);
            module.cdef_private.push('\n');
        } else if section == "functions" {
            let code_line = trimmed
                .find('#')
                .map_or(trimmed, |hash_pos| trimmed[..hash_pos].trim());

            // A blank line or `.skip n` advances the vector counter without
            // producing an entry, exactly as the reference parser does. A
            // comment line does not: 867 of them sit inside function lists, and
            // counting them would shift every vector that follows.
            let is_comment = trimmed.starts_with('#');
            if code_line.is_empty() && !is_comment {
                lvo = lvo.saturating_add(1);
            } else if let Some(rest) = code_line.strip_prefix(".skip") {
                if let Ok(n) = rest.split_whitespace().next().unwrap_or("").parse::<u32>() {
                    lvo = lvo.saturating_add(n);
                }
            }

            // `.private` and `.novararg` modify the preceding declaration.
            if code_line == ".private" {
                if let Some(f) = module.functions.last_mut() {
                    f.private = true;
                }
            } else if code_line == ".novararg" {
                if let Some(f) = module.functions.last_mut() {
                    f.novararg = true;
                }
            } else if !code_line.is_empty()
                && !code_line.starts_with('.')
                && !code_line.starts_with('#')
            {
                if let Some(mut f) = varargs::parse_function_line(code_line) {
                    f.lvo = lvo;
                    lvo = lvo.saturating_add(1);
                    // The four standard library vectors are supplied by the
                    // module skeleton, not declared in the public header.
                    let is_standard_vector = matches!(
                        f.name.as_str(),
                        "open"
                            | "close"
                            | "expunge"
                            | "extFunc"
                            | "OpenLib"
                            | "CloseLib"
                            | "ExpungeLib"
                            | "ExtFuncLib"
                    );
                    if !is_standard_vector {
                        module.functions.push(f);
                    }
                }
            }
        }
    }

    // `libbasetype` is the module's private view of its base. The reference
    // takes an explicit declaration first, falls back to libbasetypeextern
    // (config.c:1344), and writes `struct Library` when neither is given
    // (writeinclibdefs.c:13). It is never derived from the module name.
    if module.lib_base_type.is_empty() {
        module.lib_base_type = module
            .explicit_base_type_extern
            .clone()
            .unwrap_or_else(|| "struct Library".to_owned());
    }

    Some(module)
}

fn generate_sdk_headers(
    module: &ConfModule,
    out_inc: &Path,
    out_gen: Option<&Path>,
) -> std::io::Result<()> {
    let mod_upper = module.name.to_uppercase();
    let mod_lower = module.name.to_lowercase();
    let public = exports_public_headers(module);

    let proto_dir = out_inc.join("proto");
    let clib_dir = out_inc.join("clib");
    let defines_dir = out_inc.join("defines");
    if public {
        fs::create_dir_all(&proto_dir)?;
        fs::create_dir_all(&clib_dir)?;
        fs::create_dir_all(&defines_dir)?;
    }

    // 1. proto/<mod>.h, following tools/genmodule/writeincproto.c.
    //
    // The `#if !defined(<libbase>)` guard matters: a module that defines its
    // own libbase as a macro (rom/exec does this for KernelBase, in 126 files)
    // must not also see an extern declaration of a different type.
    let ptr = extern_base_type(module);
    let base = &module.lib_base;
    let mut proto_content = format!(
        "/* Auto-generated by AROS-NG genmodule v0.1.0 */\n\
         #ifndef PROTO_{mod_upper}_H\n\
         #define PROTO_{mod_upper}_H\n\n\
         #include <exec/types.h>\n\
         #include <aros/system.h>\n\
         #include <clib/{mod_lower}_protos.h>\n\
         #include <defines/{mod_lower}.h>\n\n\
         #if !defined(__NOLIBBASE__) && !defined(__{mod_upper}_NOLIBBASE__)\n\
         \x20#if !defined({base})\n"
    );
    if ptr == "struct Library *" {
        let _ = writeln!(proto_content, "  extern {ptr}{base};");
    } else {
        // A non-Library base can still be requested as a plain Library.
        let _ = write!(
            proto_content,
            "  #ifdef __{mod_upper}_STDLIBBASE__\n\
             \x20  extern struct Library *{base};\n\
             \x20 #else\n\
             \x20  extern {ptr}{base};\n\
             \x20 #endif\n"
        );
    }
    let _ = write!(
        proto_content,
        " #endif\n\
         \x20#ifndef __aros_getbase_{base}\n\
         \x20 #define __aros_getbase_{base}() ({base})\n\
         \x20#endif\n\
         #endif\n\n\
         #endif /* PROTO_{mod_upper}_H */\n"
    );
    if public {
        fs::write(proto_dir.join(format!("{mod_lower}.h")), proto_content)?;
    }

    // 2. clib/<mod>_protos.h
    let mut protos = format!(
        "/* Auto-generated by AROS-NG genmodule v0.1.0 */\n\
         #ifndef CLIB_{mod_upper}_PROTOS_H\n\
         #define CLIB_{mod_upper}_PROTOS_H\n\n\
         #include <exec/types.h>\n\
         #include <aros/system.h>\n\n\
         {}\n\n\
         #ifdef __cplusplus\n\
         extern \"C\" {{\n\
         #endif\n\n",
        module.cdef
    );

    for func in &module.functions {
        if func.private {
            continue;
        }
        let _ = writeln!(protos, "{};", func.signature());
    }

    protos.push_str(
        "\n#ifdef __cplusplus\n\
         }\n\
         #endif\n\n\
         #endif /* CLIB_",
    );
    protos.push_str(&mod_upper);
    protos.push_str("_PROTOS_H */\n");

    if public {
        fs::write(clib_dir.join(format!("{mod_lower}_protos.h")), protos)?;
    }

    // 3. defines/<mod>.h, including the varargs convenience stubs.
    let include_name = if module.include_name.is_empty() {
        mod_lower.clone()
    } else {
        module.include_name.to_lowercase()
    };
    let defines = varargs::render_defines(&include_name, &module.lib_base, &module.functions);
    if public {
        fs::write(defines_dir.join(format!("{include_name}.h")), &defines.text)?;
        // Function LVOs, a separate header in the reference too.
        fs::write(
            defines_dir.join(format!("{include_name}_LVO.h")),
            varargs::render_lvo(&include_name, &module.functions),
        )?;
    }

    // 4. <mod>_libdefs.h, following tools/genmodule/writeinclibdefs.c.
    let mod_name = mod_name_string(module);
    let flags = resident_flags(module);
    let (maj, min) = (module.major_version, module.minor_version);
    let mut libdefs = String::with_capacity(1024);
    for line in [
        "/* Auto-generated by AROS-NG genmodule v0.1.0 */".to_owned(),
        format!("#ifndef _{mod_upper}_LIBDEFS_H"),
        format!("#define _{mod_upper}_LIBDEFS_H"),
        String::new(),
        "#include <exec/types.h>".to_owned(),
        "#include <exec/libraries.h>".to_owned(),
        String::new(),
        format!("#define GM_UNIQUENAME(n) {mod_lower}_ ## n"),
        format!("#define LIBBASE          {}", module.lib_base),
        format!("#define LIBBASETYPE      {}", module.lib_base_type),
        format!("#define LIBBASETYPEPTR   {} *", module.lib_base_type),
        format!("#define MOD_NAME_STRING  \"{mod_name}\""),
        format!("#define MOD_DATE_STRING  \"{}\"", module.date),
        format!("#define MOD_VERS_STRING  \"{maj}.{min}\""),
        format!(
            "#define VERSION_STRING   \"$VER: {mod_name} {maj}.{min}{}\\r\\n\"",
            if module.date.is_empty() {
                String::new()
            } else {
                format!(" ({})", module.date)
            }
        ),
        format!("#define VERSION_NUMBER   {maj}"),
        format!("#define MAJOR_VERSION    {maj}"),
        format!("#define REVISION_NUMBER  {min}"),
        format!("#define MINOR_VERSION    {min}"),
        "#define COPYRIGHT_STRING \"\"".to_owned(),
        "#define LIBEND           GM_UNIQUENAME(End)".to_owned(),
        "#define LIBFUNCTABLE     GM_UNIQUENAME(FuncTable)".to_owned(),
        format!("#define RESIDENTPRI      {}", module.resident_pri),
        format!("#define RESIDENTFLAGS    {flags}"),
        format!("#define FUNCTIONS_COUNT  {}", module.functions.len()),
    ] {
        libdefs.push_str(&line);
        libdefs.push('\n');
    }

    // The cdefprivate block, verbatim. This is what supplies the definition of
    // the library base struct (e.g. `#include <timer_intern.h>` for
    // struct TimerBase), so it has to land here.
    if !module.cdef_private.trim().is_empty() {
        libdefs.push('\n');
        libdefs.push_str(&module.cdef_private);
    }

    // Accessors the module init code reaches through the library base.
    if let Some(f) = &module.sysbase_field {
        let _ = writeln!(
            libdefs,
            "#define GM_SYSBASE_FIELD(lh) (((LIBBASETYPEPTR)lh)->{f})"
        );
    }
    if let Some(f) = &module.oopbase_field {
        let _ = writeln!(
            libdefs,
            "#define GM_OOPBASE_FIELD(lh) (((LIBBASETYPEPTR)lh)->{f})"
        );
    }
    if let Some(f) = &module.seglist_field {
        let _ = writeln!(
            libdefs,
            "#define GM_SEGLIST_FIELD(lh) (((LIBBASETYPEPTR)lh)->{f})"
        );
    }

    let _ = writeln!(libdefs, "\n#endif /* _{mod_upper}_LIBDEFS_H */");
    // Module-private: keep it out of the shared SDK so same-named .conf files
    // in different subsystems cannot overwrite each other.
    let libdefs_dir = out_gen.map_or_else(|| out_inc.to_path_buf(), |g| g.join(&module.rel_dir));
    fs::create_dir_all(&libdefs_dir)?;
    fs::write(libdefs_dir.join(format!("{mod_lower}_libdefs.h")), libdefs)?;

    // 5. interface/<Name>.h for every declared OOP interface.
    for iface in &module.interfaces {
        interfaces::write_interface(iface, out_inc)?;
    }

    // 6. Varargs forms we recognised but do not generate (va_list / RAWARG).
    if !defines.unsupported.is_empty() {
        let report = out_inc.join("unsupported-varargs.txt");
        let mut existing = fs::read_to_string(&report).unwrap_or_default();
        for line in &defines.unsupported {
            existing.push_str(line);
            existing.push('\n');
        }
        let _ = fs::write(&report, existing);
    }

    Ok(())
}

fn main() {
    let args = Args::parse();
    // Build trees are skipped: SDK header staging copies source directories
    // wholesale, so scanning build/ would parse copied .conf files again.
    let skip_dirs = ["build", "target", ".git"];
    let conf_files: Vec<PathBuf> = WalkDir::new(&args.scan_dir)
        .into_iter()
        .filter_entry(|e| {
            !e.file_type().is_dir()
                || e.depth() == 0
                || !skip_dirs
                    .iter()
                    .any(|d| e.file_name().to_string_lossy() == *d)
        })
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "conf"))
        .map(walkdir::DirEntry::into_path)
        .filter(|p| {
            p.parent()
                .and_then(|d| d.strip_prefix(&args.scan_dir).ok())
                .is_none_or(|rel| arch_dir_applies(rel, &args.arch_dirs))
        })
        .collect();

    // Parse first, then write: the collision check needs to know which modules
    // actually claim an SDK namespace, and that follows from the parsed config.
    let modules: Vec<ConfModule> = conf_files
        .par_iter()
        .filter_map(|p| parse_conf(p, &args.scan_dir))
        .collect();

    // Only modules that export public headers can collide there. Module-private
    // headers are written per module directory and cannot.
    let mut by_name: std::collections::HashMap<String, Vec<&ConfModule>> =
        std::collections::HashMap::new();
    for m in &modules {
        if exports_public_headers(m) {
            by_name.entry(m.name.to_lowercase()).or_default().push(m);
        }
    }
    let mut clashes: Vec<String> = by_name
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(name, v)| {
            let where_: Vec<String> = v
                .iter()
                .map(|m| {
                    let t = if m.mod_type.is_empty() {
                        "?".to_owned()
                    } else {
                        m.mod_type.clone()
                    };
                    format!("{} ({t})", m.rel_dir.display())
                })
                .collect();
            format!("{name}: {}", where_.join(", "))
        })
        .collect();

    if !clashes.is_empty() {
        clashes.sort_unstable();
        let report = args.output_inc.join("conf-name-collisions.txt");
        let _ = fs::create_dir_all(&args.output_inc);
        let _ = fs::write(&report, format!("{}\n", clashes.join("\n")));
        println!(
            "⚠️  {} module name(s) still share SDK headers (both export a public API) -> {}",
            clashes.len(),
            report.display()
        );
    }

    modules.par_iter().for_each(|module| {
        let _ = generate_sdk_headers(module, &args.output_inc, args.output_gen.as_deref());
    });

    println!(
        "⚡ aros-genmodule: Processed {} .conf files -> SDK includes in {}",
        conf_files.len(),
        args.output_inc.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(mod_type: &str, funcs: usize, cdef: &str) -> ConfModule {
        ConfModule {
            name: "x".to_owned(),
            mod_type: mod_type.to_owned(),
            cdef: cdef.to_owned(),
            functions: (0..funcs)
                .map(|i| varargs::Function {
                    name: format!("F{i}"),
                    ret_type: "void ".to_owned(),
                    args: Vec::new(),
                    private: false,
                    novararg: false,
                    lvo: 0,
                })
                .collect(),
            ..ConfModule::default()
        }
    }

    #[test]
    fn libraries_and_resources_always_export() {
        assert!(exports_public_headers(&module("library", 0, "")));
        assert!(exports_public_headers(&module("resource", 0, "")));
    }

    #[test]
    fn a_hidd_without_functions_claims_no_sdk_namespace() {
        // rom/hidds/pci is a hidd with no functionlist; it must not overwrite
        // the SDK headers of workbench/tools/SysExplorer/Modules/PCI.
        assert!(!exports_public_headers(&module("hidd", 0, "")));
        assert!(exports_public_headers(&module("hidd", 1, "")));
    }

    #[test]
    fn a_device_exports_only_with_an_api_or_a_custom_base() {
        assert!(!exports_public_headers(&module("device", 0, "")));
        assert!(exports_public_headers(&module("device", 2, "")));
        assert!(exports_public_headers(&module(
            "device",
            0,
            "#include <x.h>"
        )));
        let mut custom = module("device", 0, "");
        custom.explicit_base_type_extern = Some("struct MyBase".to_owned());
        assert!(exports_public_headers(&custom));
    }

    #[test]
    fn handlers_export_with_functions_or_a_cdef_block() {
        assert!(!exports_public_headers(&module("handler", 0, "")));
        assert!(exports_public_headers(&module(
            "handler",
            0,
            "#include <y.h>"
        )));
    }

    #[test]
    fn an_undescribed_module_needs_an_api_to_export() {
        // No modtype in the build description: fall back to "has functions".
        assert!(!exports_public_headers(&module("", 0, "")));
        assert!(exports_public_headers(&module("", 1, "")));
    }

    #[test]
    fn arch_filter_keeps_paths_outside_arch() {
        let dirs = vec!["x86_64-pc".to_owned(), "all-pc".to_owned()];
        assert!(arch_dir_applies(Path::new("rom/exec"), &dirs));
        assert!(arch_dir_applies(Path::new("workbench/libs/icon"), &dirs));
    }

    #[test]
    fn arch_filter_selects_matching_architecture_dirs() {
        let dirs = vec![
            "x86_64-pc".to_owned(),
            "all-pc".to_owned(),
            "x86_64-all".to_owned(),
            "all-native".to_owned(),
        ];
        assert!(arch_dir_applies(Path::new("arch/all-pc/exec"), &dirs));
        assert!(arch_dir_applies(Path::new("arch/x86_64-all/kernel"), &dirs));
        assert!(arch_dir_applies(Path::new("arch/all-native/acpica"), &dirs));
        // Foreign architectures are skipped; this is what stops
        // arch/m68k-amiga/devs/audio from clobbering workbench/devs/audio.
        assert!(!arch_dir_applies(
            Path::new("arch/m68k-amiga/devs/audio"),
            &dirs
        ));
        assert!(!arch_dir_applies(Path::new("arch/arm-native/soc"), &dirs));
    }

    #[test]
    fn empty_arch_list_filters_nothing() {
        assert!(arch_dir_applies(
            Path::new("arch/m68k-amiga/devs/audio"),
            &[]
        ));
    }

    #[test]
    fn external_base_type_follows_the_module_type() {
        // acpica declares `libbasetype struct ACPICABase` but is a library, so
        // consumers of <proto/acpica.h> must see `struct Library *`.
        let mut m = module("library", 0, "");
        m.lib_base_type = "struct ACPICABase".to_owned();
        assert_eq!(extern_base_type(&m), "struct Library *");

        assert_eq!(
            extern_base_type(&module("device", 0, "")),
            "struct Device *"
        );
        assert_eq!(extern_base_type(&module("resource", 0, "")), "APTR ");
        assert_eq!(extern_base_type(&module("handler", 0, "")), "APTR ");
        assert_eq!(extern_base_type(&module("hidd", 0, "")), "struct Library *");
        // Unknown module type falls back to the library form.
        assert_eq!(extern_base_type(&module("", 0, "")), "struct Library *");
    }

    #[test]
    fn explicit_libbasetypeextern_wins() {
        let mut m = module("library", 0, "");
        m.explicit_base_type_extern = Some("struct MyOwnBase".to_owned());
        assert_eq!(extern_base_type(&m), "struct MyOwnBase *");
    }

    #[test]
    fn default_basename_capitalises_the_first_letter() {
        // The library base has to be named exactly as the module's own
        // functions name their last parameter, or LIBBASE binds to the
        // untyped global from the proto header instead.
        assert_eq!(default_basename("layers"), "Layers");
        assert_eq!(default_basename("intuition"), "Intuition");
        assert_eq!(default_basename("acpica"), "Acpica");
    }

    #[test]
    fn default_basename_leaves_an_already_capitalised_name_alone() {
        assert_eq!(default_basename("Layers"), "Layers");
    }

    #[test]
    fn default_basename_handles_an_empty_name() {
        assert_eq!(default_basename(""), "");
    }
}
