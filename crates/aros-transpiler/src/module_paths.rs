//! Where a module's output goes, and what MetaMake edges it implies.
//!
//! A declaration says `%build_module mmake=… modname=… modtype=…` and almost
//! never says where the result belongs: the type decides that, and the type's
//! default directory is a table in `config/make.tmpl`. Reproducing those
//! defaults is what lets a declaration stay as short in CMake as it is in Make.
//!
//! The implied MetaMake rules are the other half. A `%build_module` contributes
//! edges its file never spells out, and a transpiler that only reads explicit
//! `#MM` lines loses them, along with every target that depended on being
//! reachable that way.

use crate::ast::MetaTargetRule;
use crate::make_vars::VarScope;
use crate::parser::macro_arg;
use std::path::Path;

/// Whether a full library intentionally delegates all of its sources to
/// genmodule.
///
/// An evaluated expression that happens to be empty is not equivalent: it may
/// be an unresolved source list. Only the literal quoted-empty spelling used
/// by version.library opts into this mode, and no second language lane may be
/// present.
pub(crate) fn is_explicit_genmodule_only(invocation: &str, args: &str, mod_type: &str) -> bool {
    let literal = "files=\"\"";
    let has_literal_empty_files = args.match_indices(literal).any(|(start, _)| {
        let end = start + literal.len();
        (start == 0
            || args[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace))
            && (end == args.len() || args[end..].chars().next().is_some_and(char::is_whitespace))
    });
    invocation == "build_module"
        && mod_type == "library"
        && has_literal_empty_files
        && ["cxxfiles", "objcfiles", "asmfiles"]
            .iter()
            .all(|key| macro_arg(args, key).is_none())
}

pub(crate) fn implicit_module_meta_rules(
    mmake: &str,
    modname: &str,
    include_set: &str,
    use_libs: &[String],
    has_abi: bool,
    has_library: bool,
    emit_archspecific_rules: bool,
) -> Vec<MetaTargetRule> {
    const fn rule(name: String, dependencies: Vec<String>) -> MetaTargetRule {
        MetaTargetRule { name, dependencies }
    }

    let mut rules = Vec::new();
    for suffix in [
        "",
        "-quick",
        "-makefile",
        "-clean",
        "-genmakefile",
        "-genmodfiles",
    ] {
        rules.push(rule(format!("{mmake}{suffix}"), Vec::new()));
    }
    rules.push(rule(
        format!("{mmake}-genmodfiles"),
        vec![format!("{mmake}-genmakefile")],
    ));
    // The quick spelling is an alias for the complete module/linklib target,
    // not merely for its reduced include and architecture prerequisites
    // (make.tmpl:2671).
    rules.push(rule(format!("{mmake}-quick"), vec![mmake.to_owned()]));

    let linklibs: Vec<String> = use_libs
        .iter()
        .map(|name| format!("linklibs-{name}"))
        .collect();
    let includes: Vec<String> = use_libs
        .iter()
        .map(|name| format!("includes-{name}"))
        .collect();

    if has_abi {
        for suffix in [
            "-includes",
            "-includes-quick",
            "-includes-dirs",
            "-fd",
            "-linklib",
            "-set-archincludes",
        ] {
            rules.push(rule(format!("{mmake}{suffix}"), Vec::new()));
        }
        for alias in [
            format!("includes-{modname}"),
            format!("includes-{modname}_rel"),
        ] {
            rules.push(rule(alias, vec![format!("{mmake}-includes")]));
        }
        for alias in [
            format!("linklibs-{modname}"),
            format!("linklibs-{modname}_rel"),
        ] {
            rules.push(rule(alias, vec![format!("{mmake}-linklib")]));
        }
        rules.push(rule(
            include_set.to_owned(),
            vec![format!("{mmake}-includes")],
        ));

        let mut base_dependencies = vec![format!("{mmake}-includes"), "core-linklibs".to_owned()];
        base_dependencies.extend(linklibs.iter().cloned());
        rules.push(rule(mmake.to_owned(), base_dependencies));

        let mut linklib_dependencies = vec![format!("{mmake}-includes")];
        linklib_dependencies.extend(includes.iter().cloned());
        rules.push(rule(format!("{mmake}-linklib"), linklib_dependencies));
        rules.push(rule(
            format!("{mmake}-quick"),
            vec![format!("{mmake}-includes-quick")],
        ));
        rules.push(rule(
            format!("{mmake}-includes"),
            vec![
                format!("{mmake}-makefile"),
                format!("{mmake}-includes-dirs"),
                format!("{mmake}-set-archincludes"),
                "includes-generate-deps".to_owned(),
                format!("{mmake}-fd"),
            ],
        ));
    }

    if has_library {
        let mut kobj_dependencies = vec!["core-linklibs".to_owned()];
        kobj_dependencies.extend(linklibs);
        if has_abi {
            kobj_dependencies.insert(0, format!("{mmake}-includes"));
        }
        rules.push(rule(format!("{mmake}-kobj"), kobj_dependencies));
        rules.push(rule(
            format!("{mmake}-kobj-quick"),
            if has_abi {
                vec![format!("{mmake}-includes-quick")]
            } else {
                Vec::new()
            },
        ));
    }

    if emit_archspecific_rules {
        // `%gen_archspecificrules` is expanded for the ABI/genmodule-only
        // forms.  Sourceful modules deliberately do not receive this CMake
        // translation: MetaMake marks its architecture chain virtual and uses
        // a pre-marked traversal to break its circular return to the concrete
        // module producer.  CMake rejects that strong cycle.  Their ordinary
        // ABI/linklib aliases above remain real dependencies, while explicit
        // source-tree architecture selectors retain their own mappings.
        for suffix in [
            "",
            "-set-archincludes",
            "-linklib",
            "-kobj",
            "-kobj-quick",
            "-quick",
        ] {
            let base = format!("{mmake}{suffix}");
            let cpu = format!("{mmake}-${{AROS_TARGET_CPU}}{suffix}");
            let family = format!("{mmake}-${{AROS_TARGET_FAMILY}}{suffix}");
            let arch = format!("{mmake}-${{AROS_TARGET_PLATFORM}}{suffix}");
            let arch_variant =
                format!("{mmake}-${{AROS_TARGET_PLATFORM}}-${{AROS_TARGET_VARIANT}}{suffix}");
            let arch_cpu =
                format!("{mmake}-${{AROS_TARGET_PLATFORM}}-${{AROS_TARGET_CPU}}{suffix}");
            let arch_cpu_variant = format!(
                "{mmake}-${{AROS_TARGET_PLATFORM}}-${{AROS_TARGET_CPU}}-${{AROS_TARGET_VARIANT}}{suffix}"
            );

            rules.push(rule(base, vec![cpu.clone()]));
            rules.push(rule(cpu, vec![family.clone()]));
            rules.push(rule(family, vec![arch.clone()]));
            rules.push(rule(arch, vec![arch_variant.clone()]));
            rules.push(rule(arch_variant, vec![arch_cpu.clone()]));
            rules.push(rule(arch_cpu, vec![arch_cpu_variant.clone()]));
            rules.push(rule(arch_cpu_variant, Vec::new()));
        }
        rules.push(rule(
            format!("{mmake}-kobj"),
            vec![format!("{mmake}-${{AROS_TARGET_CPU}}")],
        ));
        rules.push(rule(
            format!("{mmake}-kobj-quick"),
            vec![format!("{mmake}-${{AROS_TARGET_CPU}}-quick")],
        ));
    }

    rules
}

/// The relative module directory genmodule chooses for a full module when no
/// `moduledir=` override is present (tools/genmodule/config.c:250-333).
///
/// This is normally left to the CMake module builder. It is needed here only
/// when a declaration explicitly changes `prefix=`, because that prefix and
/// the relative default together determine the complete output directory.
pub(crate) fn default_relative_module_dir(mod_type: &str) -> Option<&'static str> {
    match mod_type {
        "library" => Some("Libs"),
        "class" => Some("Classes"),
        "mcc" | "mui" | "mcp" => Some("Classes/Zune"),
        "device" | "resource" | "hook" => Some("Devs"),
        "gadget" => Some("Classes/Gadgets"),
        "image" => Some("Classes/Images"),
        "datatype" => Some("Classes/DataTypes"),
        "usbclass" => Some("Classes/USB"),
        "btclass" => Some("Classes/Bluetooth"),
        "hidd" => Some("Devs/Drivers"),
        "handler" => Some("L"),
        _ => None,
    }
}

pub(crate) fn rendered_absolute(path: &str) -> bool {
    Path::new(path).is_absolute()
        || path == "${AROS_BUILD_DIR}"
        || path.starts_with("${AROS_BUILD_DIR}/")
}

pub(crate) fn join_module_prefix(prefix: &str, directory: &str) -> String {
    if rendered_absolute(directory) {
        return directory.to_owned();
    }
    let prefix = prefix.trim_end_matches('/');
    let directory = directory.trim_start_matches('/');
    if prefix.is_empty() {
        directory.to_owned()
    } else if directory.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}/{directory}")
    }
}

pub(crate) fn expand_module_arg(
    raw: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    line: usize,
) -> std::result::Result<String, Vec<String>> {
    let local = |name: &str| scope.raw_at(name, line);

    // A whole local variable names the value of its assignment, not a fresh
    // recursive lookup of that name. This matters when a file shadows a
    // configured variable with a simple assignment such as
    // TARGETDIR := $(AROS_TESTS)/Library: AROS_TESTS was derived from the
    // configured TARGETDIR before the local assignment took effect.
    if let Some(name) = raw
        .strip_prefix("$(")
        .and_then(|value| value.strip_suffix(')'))
    {
        if !name.contains(['$', ' ', ')']) {
            if let Some(value) = local(name) {
                return dirs.expand_with(&value, &|nested| {
                    if nested == name {
                        None
                    } else {
                        local(nested)
                    }
                });
            }
        }
    }

    dirs.expand_with(raw, &local)
}

/// Resolves a module's explicit output arguments at the declaration line.
///
/// Local variables shadow the shared `make.cfg.in` directory table. An
/// explicit but unresolved value is an error: treating it like an absent
/// override would silently install the module into its type's default path.
pub(crate) fn resolve_module_target_dir(
    args: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    line: usize,
    mod_type: &str,
    uses_prefix: bool,
    arch_specific: bool,
) -> std::result::Result<Option<String>, String> {
    let module_dir = match macro_arg(args, "moduledir") {
        Some(raw) => Some(
            expand_module_arg(&raw, scope, dirs, line)
                .map_err(|missing| format!("moduledir={raw} references {}", missing.join(", ")))?,
        ),
        None => None,
    };

    if !uses_prefix && !arch_specific {
        return Ok(module_dir);
    }

    let prefix = if uses_prefix {
        match macro_arg(args, "prefix") {
            Some(raw) => Some(
                expand_module_arg(&raw, scope, dirs, line)
                    .map_err(|missing| format!("prefix={raw} references {}", missing.join(", ")))?,
            ),
            None => None,
        }
    } else {
        None
    };

    // An explicit moduledir replaces DEFMODDIR after the archspecific prefix
    // is computed (make.tmpl:2398-2407), so it must never inherit boot/<arch>.
    // CMake supplies the ordinary AROSDIR prefix for an otherwise relative
    // override; only an explicitly changed prefix has to be joined here.
    if let Some(directory) = module_dir {
        if rendered_absolute(&directory) {
            return Ok(Some(directory));
        }
        return Ok(Some(prefix.map_or_else(
            || directory.clone(),
            |prefix| join_module_prefix(&prefix, &directory),
        )));
    }

    if prefix.is_none() && !arch_specific {
        return Ok(None);
    }

    let directory = default_relative_module_dir(mod_type)
        .ok_or_else(|| format!("no known default moduledir for modtype={mod_type}"))?
        .to_owned();
    if rendered_absolute(&directory) {
        return Ok(Some(directory));
    }

    if arch_specific {
        // build_module_core inserts AROS_DIR_BOOTARCH between prefix and the
        // module's relative default (make.tmpl:2400-2407). With the ordinary
        // prefix, use the canonical CMake directory directly. An explicitly
        // changed prefix instead receives the same relative boot path.
        return Ok(Some(prefix.map_or_else(
            || join_module_prefix("${AROS_BOOT_ARCH_DIR}", &directory),
            |prefix| {
                join_module_prefix(
                    &prefix,
                    &format!("boot/${{AROS_TARGET_PLATFORM}}/{directory}"),
                )
            },
        )));
    }

    Ok(prefix.map(|prefix| join_module_prefix(&prefix, &directory)))
}

pub(crate) fn resolve_yes_argument(
    args: &str,
    key: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    line: usize,
) -> std::result::Result<bool, String> {
    let Some(raw) = macro_arg(args, key) else {
        return Ok(false);
    };
    let local = |name: &str| scope.raw_at(name, line);
    dirs.expand_with(&raw, &local)
        .map(|value| value == "yes")
        .map_err(|missing| format!("{key}={raw} references {}", missing.join(", ")))
}

pub(crate) fn resolve_module_suffix(
    args: &str,
    scope: &VarScope,
    dirs: &crate::dirs::DirVars,
    line: usize,
    mod_type: &str,
) -> std::result::Result<Option<String>, String> {
    if let Some(raw) = macro_arg(args, "modsuffix") {
        if raw.is_empty() {
            return Ok(None);
        }
        let local = |name: &str| scope.raw_at(name, line);
        return dirs
            .expand_with(&raw, &local)
            .map(|value| (!value.is_empty()).then_some(value))
            .map_err(|missing| format!("modsuffix={raw} references {}", missing.join(", ")));
    }
    Ok(matches!(mod_type, "usbclass" | "btclass").then(|| "class".to_owned()))
}
