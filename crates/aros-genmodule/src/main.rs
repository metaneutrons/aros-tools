mod interfaces;
mod linklib;
mod varargs;

use aros_common::read_source;
use clap::Parser;
use rayon::prelude::*;
use std::fmt::Write as _;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Writes generated output only when its bytes have actually changed.
///
/// Configure-time SDK generation runs before every CMake regeneration. Keeping
/// the timestamp of identical headers prevents all of their consumers from
/// being rebuilt even though neither the source nor the generated ABI changed.
pub(crate) fn write_if_changed(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<bool> {
    let path = path.as_ref();
    let contents = contents.as_ref();
    match fs::read(path) {
        Ok(existing) if existing == contents => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::write(path, contents)?;
    Ok(true)
}

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

    /// Where to write the list of library bases this tree declares, one per
    /// line, sorted.
    ///
    /// A relocatable AROS module leaves its library bases undefined on purpose:
    /// the loader sets them. `ninja symbol-audit` needs to know which names
    /// those are, or its count conflates "the loader will fill this in" with
    /// "nothing provides this". SysBase alone accounted for 611 of 9268
    /// references.
    #[arg(long)]
    output_libbases: Option<PathBuf>,

    /// Root for the module link library sources (e.g. build/pc-x86_64/linklib).
    ///
    /// One directory per module, holding one C file per stack-call stub plus
    /// the autoinit and getlibbase files. These are what makes a cross-library
    /// call resolve: without them a consumer carries the call as a dangling
    /// external, and because every link here is `ld.lld -r` nothing complains.
    /// `ninja symbol-audit` is what measures the difference.
    #[arg(long)]
    output_linklib: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct ConfModuleOptions(u8);

impl ConfModuleOptions {
    const EXPLICIT_LIB_BASE: u8 = 1 << 0;
    const AUTO_INIT: u8 = 1 << 1;
    const NO_RESIDENT: u8 = 1 << 2;
    const NO_INCLUDES: u8 = 1 << 3;

    const fn contains(&self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    const fn insert(&mut self, flag: u8) {
        self.0 |= flag;
    }
}

#[derive(Debug, Default)]
struct ConfModule {
    name: String,
    /// `basename` from the config, or the module name with its first letter
    /// capitalised (tools/genmodule/config.c:1333).
    ///
    /// This, not the module name, is what every generated symbol is named
    /// after: `GM_UNIQUENAME` expands to `<basename>_ ## n`
    /// (tools/genmodule/writeinclibdefs.c:82), so `kernel_init.c` referring to
    /// GM_UNIQUENAME(FuncTable) means Kernel_FuncTable. Using the module name
    /// here produced `kernel_FuncTable`, which was self-consistent only as long
    /// as nothing else generated the definition.
    base_name: String,
    lib_base: String,
    lib_base_type: String,
    /// `libbasetypeextern` if the config states one; the type is otherwise
    /// derived from the module type by extern_base_type().
    explicit_base_type_extern: Option<String>,
    options: ConfModuleOptions,
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
    /// `forcebase` entries. The autoinit file imports __aros_getbase_ for each,
    /// which is what makes a parent open those libraries.
    force_bases: Vec<String>,
    /// `##begin startup` lines, written verbatim ahead of the autoinit banner.
    startup_lines: String,
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
    // The UnixIO HIDD is hosted-only code, but its public interface is also
    // included by the native PC and Sam440 serial/parallel drivers. MetaMake's
    // universal includes-generate target publishes it for every architecture;
    // retain that exact API config without admitting unrelated foreign module
    // namespaces that could overwrite the active architecture's headers.
    if rel.starts_with("arch/all-unix/hidd/unixio") {
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

/// The exact `modname`/`modtype` invocation associated with one `.conf`.
///
/// `genmodule` receives both values on its command line. They cannot in
/// general be reconstructed from the config file stem: Wanderer's private
/// `icon.conf`, for example, is invoked as `Icon mui`, whereas icon.library is
/// invoked as `icon library`.
#[derive(Debug)]
struct ModuleDeclaration {
    name: String,
    mod_type: String,
}

fn module_macro_value(block: &str, key: &str) -> Option<String> {
    block
        .split_whitespace()
        .find_map(|token| token.strip_prefix(key))
        .map(|value| value.trim_matches(['"', '\'']).to_owned())
}

/// Reads the genmodule invocation associated with a `.conf` from its sibling
/// mmakefile.src. The module type and default include name both come from that
/// invocation, not from the config file name.
fn read_module_declaration(conf_path: &Path, stem: &str) -> Option<ModuleDeclaration> {
    let mmakefile = conf_path.parent()?.join("mmakefile.src");
    let content = read_source(&mmakefile).ok()?;
    // Directives span continuation lines, so flatten before matching.
    let flat = content.replace("\\\n", " ");
    let config_name = conf_path.file_name()?.to_string_lossy();
    let mut fallback_type: Option<String> = None;
    let mut default_declaration: Option<ModuleDeclaration> = None;
    for block in flat.split("%build_module").skip(1) {
        // The next macro begins with '%'; values of interest are all in this
        // one declaration, so never let a later build_module donate its args.
        let head = block.split('%').next().unwrap_or(block);
        let name = module_macro_value(head, "modname=");
        let mod_type = module_macro_value(head, "modtype=");
        let conffile = module_macro_value(head, "conffile=");
        if fallback_type.is_none() {
            fallback_type.clone_from(&mod_type);
        }

        let matches_explicit_config = conffile.as_deref().is_some_and(|config| {
            Path::new(config)
                .file_name()
                .is_some_and(|file| file == config_name.as_ref())
        });
        let matches_default_config = conffile.is_none()
            && name
                .as_deref()
                .is_some_and(|module_name| module_name == stem);
        // An explicit `conffile=` binds this config even if an earlier
        // declaration would select it by the default `<modname>.conf` rule.
        // Wanderer's `Icon mui conffile=icon.conf` is the important real
        // example: it must not inherit icon.library's declaration merely
        // because that one appears first in the same make fragment.
        if matches_explicit_config {
            if let (Some(name), Some(mod_type)) = (name.as_ref(), mod_type.as_ref()) {
                return Some(ModuleDeclaration {
                    name: name.clone(),
                    mod_type: mod_type.clone(),
                });
            }
        }
        if matches_default_config && default_declaration.is_none() {
            if let (Some(name), Some(mod_type)) = (name, mod_type) {
                default_declaration = Some(ModuleDeclaration { name, mod_type });
            }
        }
    }
    default_declaration.or_else(|| {
        fallback_type.map(|mod_type| ModuleDeclaration {
            name: stem.to_owned(),
            mod_type,
        })
    })
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
    format!("{}{}{}", module.name, sep, suffix)
}

/// The basename used by all public genmodule headers.
///
/// `includename` defaults to the command-line module name in the reference
/// generator.  It is deliberately not normalised: `Icon` and `icon` are
/// distinct header paths on a case-sensitive build host.
fn public_include_name(module: &ConfModule) -> &str {
    if module.include_name.is_empty() {
        &module.name
    } else {
        &module.include_name
    }
}

/// Turns a public header basename into the reference's include-guard token.
///
/// `config.c` uppercases the name and maps every non-alphanumeric byte to an
/// underscore.  Header paths themselves retain the original spelling.
fn header_guard_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
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
    if module.options.contains(ConfModuleOptions::AUTO_INIT) {
        flags.push("RTF_AUTOINIT");
    }
    if flags.is_empty() {
        "0".to_owned()
    } else {
        flags.join("|")
    }
}

/// The number of jump-table vectors a module's base needs, following
/// `tools/genmodule/writeinclibdefs.c:20`.
///
/// This is the *highest LVO*, not the number of functions. A `.skip N` line in
/// the function list reserves N vectors without declaring a function, so a list
/// of 59 functions with 12 reserved LVOs occupies 71 slots. `FUNCTIONS_COUNT` is
/// what sizes the allocation -- the generated start code computes
/// `vecsize = FUNCTIONS_COUNT * LIB_VECTSIZE` (`writestart.c:1055`), and a
/// hand-written base allocator does the same -- while `MakeFunctions` walks the
/// function table to its `-1` terminator, which covers every reserved LVO.
/// Counting functions therefore under-allocates by exactly the reserved vectors,
/// and `MakeFunctions` writes below the allocation.
///
/// That was not hypothetical: kernel.resource has 59 functions and 12 reserved
/// LVOs, so its base was allocated 0x60 bytes short, and `MakeFunctions` wrote
/// its lowest 12 vectors over the ROM MemHeader that `krnCreateROMHeader` had
/// just linked into `SysBase->MemList`. `FindMem` then walked into the wreckage.
/// OPEN-POINTS 27g. 48 conf files in the tree use `.skip`.
///
/// The reference takes the last entry of a list it built in LVO order; taking
/// the maximum is the same value there and does not depend on that ordering.
/// An empty list falls back to `firstlvo - 1`, as the reference does.
fn functions_count(module: &ConfModule) -> u32 {
    module
        .functions
        .iter()
        .map(|f| f.lvo)
        .max()
        .unwrap_or_else(|| {
            varargs::first_lvo(
                &module.mod_type,
                module.options.contains(ConfModuleOptions::NO_RESIDENT),
            )
            .saturating_sub(1)
        })
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
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

fn parse_conf(path: &Path, root: &Path) -> Option<ConfModule> {
    let content = read_source(path).ok()?;
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let declaration = read_module_declaration(path, &stem);
    let module_name = declaration
        .as_ref()
        .map_or_else(|| stem.clone(), |declaration| declaration.name.clone());

    let mut module = ConfModule {
        name: module_name.clone(),
        base_name: default_basename(&module_name),
        lib_base: format!("{}Base", default_basename(&module_name)),
        // Left empty on purpose: the default depends on libbasetypeextern,
        // which may be read later in the config section. Resolved below.
        lib_base_type: String::new(),
        mod_type: declaration.map_or_else(String::new, |declaration| declaration.mod_type),
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
    // Running `.version` value; see the branch below.
    let mut pending_version: Option<u32> = None;
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
        } else if trimmed == "##begin startup" {
            section = "startup";
        } else if trimmed == "##end startup" {
            section = "";
        } else if trimmed == "##begin cdefprivate" {
            section = "cdefprivate";
        } else if trimmed == "##end cdefprivate" {
            section = "";
        } else if trimmed == "##begin functionlist" {
            section = "functions";
            if !lvo_ready {
                lvo = varargs::first_lvo(
                    &module.mod_type,
                    module.options.contains(ConfModuleOptions::NO_RESIDENT),
                );
                lvo_ready = true;
            }
        } else if trimmed == "##end functionlist" {
            section = "";
        } else if section == "config" {
            let Some((key, val)) = conf_key_value(trimmed) else {
                continue;
            };
            match key {
                "forcebase" => module
                    .force_bases
                    .extend(val.split_whitespace().map(str::to_owned)),
                // The reference derives libbase from basename and lets an
                // explicit `libbase` line override it, in either order
                // (tools/genmodule/config.c:1336).
                "basename" => {
                    module.base_name = val.to_string();
                    if !module
                        .options
                        .contains(ConfModuleOptions::EXPLICIT_LIB_BASE)
                    {
                        module.lib_base = format!("{val}Base");
                    }
                }
                "libbase" => {
                    module.lib_base = val.to_string();
                    module.options.insert(ConfModuleOptions::EXPLICIT_LIB_BASE);
                }
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
                            "autoinit" => module.options.insert(ConfModuleOptions::AUTO_INIT),
                            "noresident" => {
                                module.options.insert(ConfModuleOptions::NO_RESIDENT);
                            }
                            "noincludes" => {
                                module.options.insert(ConfModuleOptions::NO_INCLUDES);
                            }
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
        } else if section == "startup" {
            module.startup_lines.push_str(line);
            module.startup_lines.push('\n');
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
            } else if let Some(rest) = code_line.strip_prefix(".version") {
                // Marks the version a caller of the *following* functions has
                // to open, not of the preceding one: config.c:1811 says so and
                // :1919 applies the running value to each new declaration.
                // writestubs.c turns it into AROS_LIBREQ.
                if let Ok(v) = rest.trim().parse::<u32>() {
                    pending_version = Some(v);
                }
            } else if let Some(rest) = code_line.strip_prefix(".alias") {
                let alias = rest.trim();
                if !alias.is_empty() {
                    if let Some(f) = module.functions.last_mut() {
                        f.aliases.push(alias.to_owned());
                    }
                }
            } else if !code_line.is_empty()
                && !code_line.starts_with('.')
                && !code_line.starts_with('#')
            {
                if let Some(mut f) = varargs::parse_function_line(code_line) {
                    f.lvo = lvo;
                    f.declared_version = pending_version;
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
    // config.c:1413-1416 defaults `includename` to the command-line module
    // name. Preserve its spelling: header paths are case-sensitive on some
    // supported host filesystems, while case-insensitive hosts are handled by
    // the collision barrier below.
    if module.include_name.is_empty() {
        module.include_name = module.name.clone();
    }

    Some(module)
}

fn generate_sdk_headers(
    module: &ConfModule,
    out_inc: &Path,
    out_gen: Option<&Path>,
    publish_public_headers: bool,
) -> std::io::Result<()> {
    let include_name = public_include_name(module);
    let include_upper = header_guard_name(include_name);
    let mod_upper = header_guard_name(&module.name);
    let public = publish_public_headers && exports_public_headers(module);

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
         #ifndef PROTO_{include_upper}_H\n\
         #define PROTO_{include_upper}_H\n\n\
         #include <exec/types.h>\n\
         #include <aros/system.h>\n\
         #include <clib/{include_name}_protos.h>\n\
         #include <defines/{include_name}.h>\n\n\
         #if !defined(__NOLIBBASE__) && !defined(__{include_upper}_NOLIBBASE__)\n\
         \x20#if !defined({base})\n"
    );
    if ptr == "struct Library *" {
        let _ = writeln!(proto_content, "  extern {ptr}{base};");
    } else {
        // A non-Library base can still be requested as a plain Library.
        let _ = write!(
            proto_content,
            "  #ifdef __{include_upper}_STDLIBBASE__\n\
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
         #endif /* PROTO_{include_upper}_H */\n"
    );
    if public {
        write_if_changed(proto_dir.join(format!("{include_name}.h")), proto_content)?;
    }

    // 2. clib/<mod>_protos.h
    let mut protos = format!(
        "/* Auto-generated by AROS-NG genmodule v0.1.0 */\n\
         #ifndef CLIB_{include_upper}_PROTOS_H\n\
         #define CLIB_{include_upper}_PROTOS_H\n\n\
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
    protos.push_str(&include_upper);
    protos.push_str("_PROTOS_H */\n");

    if public {
        write_if_changed(clib_dir.join(format!("{include_name}_protos.h")), protos)?;
    }

    // 3. defines/<mod>.h: the library-call defines and the varargs stubs.
    let defines_cx = varargs::DefinesContext {
        include_name,
        lib_base: &module.lib_base,
        lib_base_type_extern: &extern_base_type(module),
        basename: &module.base_name,
        first_lvo: varargs::first_lvo(
            &module.mod_type,
            module.options.contains(ConfModuleOptions::NO_RESIDENT),
        ),
        major_version: module.major_version,
    };
    let defines = varargs::render_defines(&defines_cx, &module.functions);
    if public {
        write_if_changed(defines_dir.join(format!("{include_name}.h")), &defines.text)?;
        // Function LVOs, a separate header in the reference too.
        write_if_changed(
            defines_dir.join(format!("{include_name}_LVO.h")),
            varargs::render_lvo(include_name, &module.functions),
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
        format!("#define GM_UNIQUENAME(n) {}_ ## n", module.base_name),
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
        format!("#define FUNCTIONS_COUNT  {}", functions_count(module)),
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
    write_if_changed(
        libdefs_dir.join(format!("{}_libdefs.h", module.name)),
        libdefs,
    )?;

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

/// Names whose `proto/` and `clib/` paths have more than one potential owner.
///
/// The reference build runs genmodule through a concrete `<mmake>-includes`
/// target.  That target's dependency closure determines which declaration owns
/// a shared SDK path at that moment.  This broad bootstrap scan has no such
/// closure, so writing both owners would make the resulting headers depend on
/// traversal and thread scheduling.  Keep those public paths absent until the
/// concrete CMake producer materialises the selected declaration.
fn colliding_public_include_names(modules: &[ConfModule]) -> std::collections::HashSet<String> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for module in modules {
        if exports_public_headers(module) {
            // Treat the output namespace as case-insensitive here. The
            // bootstrap can run on case-insensitive APFS, while the project
            // still supports case-sensitive hosts where `Icon` and `icon`
            // are distinct. Withholding both is safer than a host-dependent
            // race; a concrete includes target later owns the exact spelling.
            *counts
                .entry(public_include_name(module).to_ascii_lowercase())
                .or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect()
}

/// Removes bootstrap products whose shared name is ambiguous.
///
/// `BootstrapSDK.cmake` reruns this scanner during every configuration.  A
/// stale header from a previous non-conflicting configuration would otherwise
/// look usable to Ninja even though there is no longer an unambiguous owner.
fn remove_colliding_public_headers(module: &ConfModule, out_inc: &Path) -> std::io::Result<()> {
    let include_name = public_include_name(module);
    for path in [
        out_inc.join("proto").join(format!("{include_name}.h")),
        out_inc
            .join("clib")
            .join(format!("{include_name}_protos.h")),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    for path in [
        out_inc.join("defines").join(format!("{include_name}.h")),
        out_inc
            .join("defines")
            .join(format!("{include_name}_LVO.h")),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
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
    let mut modules: Vec<ConfModule> = conf_files
        .par_iter()
        .filter_map(|p| parse_conf(p, &args.scan_dir))
        .collect();
    modules.sort_by(|left, right| {
        left.rel_dir
            .cmp(&right.rel_dir)
            .then_with(|| left.name.cmp(&right.name))
    });

    let colliding_names = colliding_public_include_names(&modules);

    // Only modules that export public headers can collide there. Module-private
    // headers are written per module directory and cannot.
    let mut by_name: std::collections::HashMap<String, Vec<&ConfModule>> =
        std::collections::HashMap::new();
    for m in &modules {
        if exports_public_headers(m) {
            by_name
                .entry(public_include_name(m).to_ascii_lowercase())
                .or_default()
                .push(m);
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

    let report = args.output_inc.join("conf-name-collisions.txt");
    if clashes.is_empty() {
        // Removed rather than left behind: a report that outlives its cause
        // keeps naming collisions that no longer exist.
        let _ = fs::remove_file(&report);
    } else {
        clashes.sort_unstable();
        let _ = fs::create_dir_all(&args.output_inc);
        let _ = write_if_changed(&report, format!("{}\n", clashes.join("\n")));
        println!(
            "⚠️  {} module name(s) have conflicting public headers; bootstrap output is withheld until a concrete includes target owns it -> {}",
            clashes.len(),
            report.display()
        );
    }

    let mut linklib_files = 0usize;
    for module in &modules {
        let conflicting =
            colliding_names.contains(&public_include_name(module).to_ascii_lowercase());
        if conflicting {
            if let Err(error) = remove_colliding_public_headers(module, &args.output_inc) {
                eprintln!(
                    "aros-genmodule: failed to remove ambiguous public headers for {}: {error}",
                    module.rel_dir.display()
                );
            }
        }
        if let Err(error) = generate_sdk_headers(
            module,
            &args.output_inc,
            args.output_gen.as_deref(),
            !conflicting,
        ) {
            eprintln!(
                "aros-genmodule: failed to generate headers for {}: {error}",
                module.rel_dir.display()
            );
        }
        if let Some(root) = args.output_linklib.as_deref() {
            match generate_linklib_sources(module, root) {
                Ok(n) => linklib_files += n,
                Err(error) => eprintln!(
                    "aros-genmodule: failed to generate link library sources for {}: {error}",
                    module.rel_dir.display()
                ),
            }
        }
    }

    println!(
        "⚡ aros-genmodule: Processed {} .conf files -> SDK includes in {}",
        conf_files.len(),
        args.output_inc.display()
    );
    if let Some(path) = args.output_libbases.as_deref() {
        // Every declared base, including modules whose headers are withheld for
        // a name collision: the base is still real and still left to the loader.
        let mut bases: Vec<&str> = modules.iter().map(|m| m.lib_base.as_str()).collect();
        bases.sort_unstable();
        bases.dedup();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match write_if_changed(path, &(bases.join("\n") + "\n")) {
            Ok(_) => println!(
                "🏷️  aros-genmodule: {} library base(s) -> {}",
                bases.len(),
                path.display()
            ),
            Err(error) => eprintln!("aros-genmodule: failed to write library bases: {error}"),
        }
    }
    if let Some(root) = args.output_linklib.as_deref() {
        println!(
            "🔗 aros-genmodule: {linklib_files} link library source(s) in {}",
            root.display()
        );
    }
}

/// Writes one module's link library sources under `<root>/<rel_dir>/<mod>/`.
///
/// The relative directory is kept because 26 .conf stems occur more than once
/// in the tree; a flat layout would make them overwrite each other, the same
/// reason `<mod>_libdefs.h` is placed per module.
///
/// Both the plain and the `rel` flavour are written. The reference generates
/// them from the same config with `is_rel` toggled, and a module built into a
/// relative-library archive needs the second set.
fn generate_linklib_sources(module: &ConfModule, root: &Path) -> std::io::Result<usize> {
    // A module with no public functions has nothing to stub, and one that
    // exports no headers has no proto/ header for the stubs to include.
    if !exports_public_headers(module) {
        return Ok(0);
    }

    let suffix = match module.mod_type.as_str() {
        // config.c:305 and :315 -- both of these are named "<x>.class".
        "usbclass" | "btclass" => "class",
        other => other,
    };
    let facts = linklib::ModuleFacts {
        name: &module.name,
        include_name: &module.include_name,
        lib_base: &module.lib_base,
        lib_base_type_extern: &extern_base_type(module),
        basename: &module.base_name,
        suffix,
        no_includes: module.options.contains(ConfModuleOptions::NO_INCLUDES),
        cdef_private: &module.cdef_private,
        major_version: module.major_version,
        force_bases: &module.force_bases,
        startup_lines: &module.startup_lines,
    };

    let dir = root.join(&module.rel_dir).join(&module.name);
    fs::create_dir_all(&dir)?;
    let mut written = 0usize;
    for is_rel in [false, true] {
        for (name, body) in linklib::sources(&facts, &module.functions, is_rel) {
            write_if_changed(dir.join(name), &body)?;
            written += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn generated_output_is_written_only_when_its_bytes_change() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "aros-genmodule-write-if-changed-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create test directory");
        let path = dir.join("generated.h");

        assert!(write_if_changed(&path, b"first\n").expect("initial write"));
        assert!(!write_if_changed(&path, b"first\n").expect("unchanged write"));
        assert!(write_if_changed(&path, b"second\n").expect("changed write"));
        assert_eq!(fs::read(&path).expect("read generated output"), b"second\n");

        fs::remove_dir_all(dir).expect("remove test directory");
    }

    fn module(mod_type: &str, funcs: usize, cdef: &str) -> ConfModule {
        // LVOs run consecutively from the module type's first, which is what a
        // conf without `.skip` produces. A fixture that left them at 0 would
        // hide the difference between counting functions and taking the
        // highest LVO.
        let first = varargs::first_lvo(mod_type, false);
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
                    lvo: first + u32::try_from(i).expect("fixture function count fits u32"),
                    stack_call: true,
                    declared_version: None,
                    aliases: Vec::new(),
                })
                .collect(),
            ..ConfModule::default()
        }
    }

    /// `.skip` reserves vectors without declaring a function, so the vector
    /// count is the highest LVO and not the function count. kernel.resource is
    /// the measured case: 59 functions, 12 reserved LVOs, 71 slots. Sizing the
    /// base from 59 let MakeFunctions write 0x60 bytes below the allocation,
    /// over the ROM MemHeader. OPEN-POINTS 27g.
    #[test]
    fn functions_count_is_the_highest_lvo_not_the_function_count() {
        // A resource starts at LVO 1, so consecutive LVOs make the two equal.
        let dense = module("resource", 59, "");
        assert_eq!(functions_count(&dense), 59);

        // Reserving 12 LVOs across the list leaves 59 functions on 71 slots.
        let mut sparse = module("resource", 59, "");
        sparse.functions[58].lvo = 71;
        assert_eq!(functions_count(&sparse), 71);
        assert_eq!(sparse.functions.len(), 59);

        // A library's own vectors start at 5, so even a dense list is offset.
        let library = module("library", 3, "");
        assert_eq!(functions_count(&library), 7);
    }

    /// An empty function list still reserves the module type's own vectors:
    /// `firstlvo - 1`, as `writeinclibdefs.c:21` has it.
    #[test]
    fn functions_count_without_functions_reserves_the_type_vectors() {
        assert_eq!(functions_count(&module("library", 0, "")), 4);
        assert_eq!(functions_count(&module("device", 0, "")), 6);
        assert_eq!(functions_count(&module("resource", 0, "")), 0);
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
        assert!(arch_dir_applies(
            Path::new("arch/all-unix/hidd/unixio"),
            &dirs
        ));
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

    #[test]
    fn explicit_conffile_invocation_beats_the_default_module_name_match() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "aros-genmodule-invocation-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create test directory");
        let conf = dir.join("icon.conf");
        fs::write(&conf, "##begin config\n##end config\n").expect("write config");
        fs::write(
            dir.join("mmakefile.src"),
            "%build_module mmake=workbench-libs-icon modname=icon modtype=library files=icon.c\n\
             %build_module mmake=wanderer-classes-icon modname=Icon modtype=mui conffile=icon.conf files=icon.c\n",
        )
        .expect("write make fragment");

        let declaration = read_module_declaration(&conf, "icon").expect("read declaration");
        assert_eq!(declaration.name, "Icon");
        assert_eq!(declaration.mod_type, "mui");

        fs::remove_dir_all(dir).expect("remove test directory");
    }

    #[test]
    fn public_headers_keep_the_exact_include_name_spelling() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "aros-genmodule-include-name-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create test directory");
        let module = ConfModule {
            name: "Icon".to_owned(),
            include_name: "Icon".to_owned(),
            lib_base: "IconBase".to_owned(),
            lib_base_type: "struct Library".to_owned(),
            mod_type: "mui".to_owned(),
            // A MUI module with a cdef block owns public headers in the
            // reference generator too.
            cdef: "typedef int IconPublic;\n".to_owned(),
            ..ConfModule::default()
        };

        generate_sdk_headers(&module, &dir, Some(&dir.join("gen")), true)
            .expect("generate headers");
        let proto = dir.join("proto/Icon.h");
        assert!(proto.exists());
        let names: Vec<String> = fs::read_dir(dir.join("proto"))
            .expect("read proto directory")
            .map(|entry| {
                entry
                    .expect("read proto entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(names.iter().any(|name| name == "Icon.h"));
        assert!(!names.iter().any(|name| name == "icon.h"));
        let contents = fs::read_to_string(proto).expect("read proto header");
        assert!(contents.contains("#include <clib/Icon_protos.h>"));
        assert!(contents.contains("#include <defines/Icon.h>"));

        fs::remove_dir_all(dir).expect("remove test directory");
    }

    #[test]
    fn bootstrap_withholds_case_only_public_header_collisions() {
        let mut library = module("library", 0, "");
        library.name = "icon".to_owned();
        library.include_name = "icon".to_owned();
        let mut mui = module("mui", 0, "typedef int IconPublic;");
        mui.name = "Icon".to_owned();
        mui.include_name = "Icon".to_owned();

        let collisions = colliding_public_include_names(&[library, mui]);
        assert_eq!(collisions.len(), 1);
        assert!(collisions.contains("icon"));
    }

    #[test]
    fn gm_uniquename_is_the_basename_not_the_module_name() {
        let root =
            std::env::temp_dir().join(format!("aros-genmodule-basename-{}", std::process::id()));
        let dir = root.join("rom/kernel");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&dir).expect("module dir");
        std::fs::write(
            dir.join("kernel.conf"),
            "##begin config\nlibbase KernelBase\nlibbasetype struct KernelBase\n##end config\n",
        )
        .expect("write conf");
        let module = parse_conf(&dir.join("kernel.conf"), &root).expect("parse");

        // tools/genmodule/config.c:1333 capitalises the module name when the
        // config states no basename, and writeinclibdefs.c:82 names every
        // generated symbol after it. rom/kernel/kernel_init.c:62 declares
        // GM_UNIQUENAME(FuncTable), so that has to be Kernel_FuncTable.
        assert_eq!(module.base_name, "Kernel");
        assert_eq!(module.lib_base, "KernelBase");

        // An explicit basename wins, and does not overwrite an explicit libbase.
        std::fs::write(
            dir.join("kernel.conf"),
            "##begin config\nlibbase KernelBase\nbasename Kern\n##end config\n",
        )
        .expect("write conf");
        let module = parse_conf(&dir.join("kernel.conf"), &root).expect("parse");
        assert_eq!(module.base_name, "Kern");
        assert_eq!(module.lib_base, "KernelBase");

        // Without an explicit libbase, basename derives it.
        std::fs::write(
            dir.join("kernel.conf"),
            "##begin config\nbasename Kern\n##end config\n",
        )
        .expect("write conf");
        let module = parse_conf(&dir.join("kernel.conf"), &root).expect("parse");
        assert_eq!(module.base_name, "Kern");
        assert_eq!(module.lib_base, "KernBase");

        let _ = std::fs::remove_dir_all(&root);
    }
}
