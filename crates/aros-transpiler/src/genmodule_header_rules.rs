//! Closed model for a hand-written `GENMODULE` include-stamp recipe.
//!
//! This is deliberately separate from the generated module ABI machinery:
//! ordinary `#MM` targets can own a small Make rule which invokes the legacy
//! host `genmodule` tool, and Ninja needs the concrete headers declared before
//! the build starts.  Only the bounded four-invocation shape below is modeled;
//! anything close to it but not representable is returned with its owner so
//! the graph layer can fail closed when that target is selected.

use crate::make_vars::ConditionalTruth;
use crate::{evaluate_make_expr, MakeExprContext};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const STAMP: &str = "$(GENDIR)/$(CURDIR)/.includes-generated";
const PRIVATE_EXPR: &str = "$(GENDIR)/$(CURDIR)/include";
const GENINC_EXPR: &str = "$(GENDIR)/include";
const SDK_EXPR: &str = "$(AROS_INCLUDES)";
const CONFIG_KEYS: &[&str] = &[
    "basename",
    "libbase",
    "libbasetype",
    "libbasetypeextern",
    "version",
    "date",
    "copyright",
    "libcall",
    "forcebase",
    "superclass",
    "superclass_field",
    "residentpri",
    "options",
    "sysbase_field",
    "seglist_field",
    "rootbase_field",
    "classptr_field",
    "classptr_var",
    "classid",
    "classdatatype",
    "beginio_func",
    "abortio_func",
    "dispatcher",
    "initpri",
    "type",
    "addromtag",
    "oopbase_field",
    "rellib",
    "interfaceid",
    "interfacename",
    "methodstub",
    "methodbase",
    "attributebase",
    "handler_func",
    "includename",
];
const OPTIONS: &[&str] = &[
    "noautolib",
    "noexpunge",
    "noresident",
    "peropenerbase",
    "pertaskbase",
    "includes",
    "noincludes",
    "nostubs",
    "autoinit",
    "noautoinit",
    "resautoinit",
    "noinittable",
    "noresstruct",
    "nofunctable",
    "noopenclose",
    "selfinit",
    "rellinklib",
    "noclassquery",
];

/// A `genmodule` module type accepted by the reference tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenmoduleType {
    Class,
    Library,
    Mcc,
    Mui,
    Mcp,
    Device,
    Resource,
    Gadget,
    Image,
    Datatype,
    Usbclass,
    Btclass,
    Hidd,
    Handler,
    Hook,
}

impl GenmoduleType {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "class" => Self::Class,
            "library" => Self::Library,
            "mcc" => Self::Mcc,
            "mui" => Self::Mui,
            "mcp" => Self::Mcp,
            "device" => Self::Device,
            "resource" => Self::Resource,
            "gadget" => Self::Gadget,
            "image" => Self::Image,
            "datatype" => Self::Datatype,
            "usbclass" => Self::Usbclass,
            "btclass" => Self::Btclass,
            "hidd" => Self::Hidd,
            "handler" => Self::Handler,
            "hook" => Self::Hook,
            _ => return None,
        })
    }

    /// The literal module-type spelling accepted by `genmodule`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Library => "library",
            Self::Mcc => "mcc",
            Self::Mui => "mui",
            Self::Mcp => "mcp",
            Self::Device => "device",
            Self::Resource => "resource",
            Self::Gadget => "gadget",
            Self::Image => "image",
            Self::Datatype => "datatype",
            Self::Usbclass => "usbclass",
            Self::Btclass => "btclass",
            Self::Hidd => "hidd",
            Self::Handler => "handler",
            Self::Hook => "hook",
        }
    }
}

/// The bounded destination and action layout for a handwritten stamp recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenmoduleHeaderLayout {
    /// Private `writeincludes` + `writelibdefs`, then GENINCDIR and SDK headers.
    Full,
    /// Private `writeincludes` + `writelibdefs` only.
    PrivateOnly,
    /// GENINCDIR + SDK `writeincludes` only, after a checked local mkdir rule.
    PublicOnly,
}

impl GenmoduleHeaderLayout {
    /// The CMake helper's literal layout spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "Full",
            Self::PrivateOnly => "PrivateOnly",
            Self::PublicOnly => "PublicOnly",
        }
    }
}

/// One concrete output declared by the closed header-stamp rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenmoduleHeaderOutput {
    /// Which configured include root receives this output.
    pub destination: GenmoduleHeaderDestination,
    /// Header path below that root.
    pub relative_path: String,
}

/// The build-tree include root that receives one header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenmoduleHeaderDestination {
    /// `$(GENDIR)/$(CURDIR)/include`, private to this source directory.
    Private,
    /// `$(GENDIR)/include`, mapped to `${AROS_GENINC_DIR}` by the engine.
    Geninc,
    /// `$(AROS_INCLUDES)`, mapped to `${AROS_SDK_INCLUDE_DIR}` by the engine.
    Sdk,
}

/// One safely represented hand-written genmodule header rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenmoduleHeaderRuleDecl {
    /// The ordinary named Make target which depends on the stamp.
    pub owner: String,
    /// Source-root-relative mmakefile path.
    pub file: String,
    /// One-based line of the stamp rule.
    pub line: usize,
    /// Source-root-relative directory represented by `$(CURDIR)`.
    pub declaring_dir: String,
    /// Source-root-relative `.conf` path.
    pub config: String,
    /// Literal `genmodule` module name.
    pub module: String,
    /// Literal `genmodule` module type.
    pub modtype: GenmoduleType,
    /// Bounded producer destinations and actions.
    pub layout: GenmoduleHeaderLayout,
    /// The configured include-name, or the module name when omitted.
    pub include_name: String,
    /// Exact generated files across the three bounded output roots.
    pub outputs: Vec<GenmoduleHeaderOutput>,
}

/// A hand-written genmodule header rule that cannot be represented safely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenmoduleHeaderRuleRejection {
    /// Best available named owner; `<unknown-owner>` when none can be proven.
    pub owner: String,
    /// Source-root-relative mmakefile path.
    pub file: String,
    /// One-based stamp-rule line where the candidate was found.
    pub line: usize,
    /// Why the candidate is outside the closed contract.
    pub reason: String,
}

/// Results from one mmakefile scan.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenmoduleHeaderRuleScan {
    pub declarations: Vec<GenmoduleHeaderRuleDecl>,
    pub rejected: Vec<GenmoduleHeaderRuleRejection>,
}

#[derive(Debug, Clone)]
pub(crate) struct LogicalLine {
    text: String,
    source_line: usize,
    continued: bool,
    state: ConditionalTruth,
    conditional_syntax: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct RecipeLine {
    pub(crate) text: String,
    pub(crate) source_line: usize,
    pub(crate) continued: bool,
    pub(crate) state: ConditionalTruth,
    pub(crate) conditional_syntax: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Rule {
    pub(crate) target: String,
    pub(crate) prerequisites: String,
    pub(crate) line: usize,
    pub(crate) continued: bool,
    pub(crate) state: ConditionalTruth,
    pub(crate) conditional_syntax: bool,
    pub(crate) recipes: Vec<RecipeLine>,
}

/// Scans a single mmakefile without Make conditional information.
///
/// For parser-pipeline use, call the crate-private line-state form so false
/// branches are excluded and unknown owner/recipe lines reject the whole
/// candidate instead of being silently omitted.
#[must_use]
pub fn collect_genmodule_header_rules(
    content: &str,
    source_root: &Path,
    rel_dir: &Path,
) -> GenmoduleHeaderRuleScan {
    let (scope, _) = crate::make_vars::collect_vars_impl(content, None);
    let dirs = crate::dirs::DirVars::load(source_root);
    collect_genmodule_header_rules_with_context(content, source_root, rel_dir, &scope, &dirs, None)
}

/// Line-state-aware scanner used by tests. State positions are zero-based
/// physical source lines from the joined mmakefile text.
#[cfg(test)]
#[must_use]
pub(crate) fn collect_genmodule_header_rules_with_line_states(
    content: &str,
    source_root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
) -> GenmoduleHeaderRuleScan {
    let (scope, _) = crate::make_vars::collect_vars_impl(content, None);
    let dirs = crate::dirs::DirVars::load(source_root);
    collect_genmodule_header_rules_with_context(
        content,
        source_root,
        rel_dir,
        &scope,
        &dirs,
        line_states,
    )
}

/// Context-aware scanner used by the parser pipeline to expand source-local
/// path aliases at the exact recipe/prerequisite line where Make sees them.
#[must_use]
pub(crate) fn collect_genmodule_header_rules_with_context(
    content: &str,
    source_root: &Path,
    rel_dir: &Path,
    scope: &crate::make_vars::VarScope,
    dirs: &crate::dirs::DirVars,
    line_states: Option<&[ConditionalTruth]>,
) -> GenmoduleHeaderRuleScan {
    let rel_dir_text = match safe_relative_directory(rel_dir) {
        Ok(value) => value,
        Err(reason) => {
            let mut scan = GenmoduleHeaderRuleScan::default();
            if content.contains(".includes-generated") {
                scan.rejected.push(GenmoduleHeaderRuleRejection {
                    owner: owner_hint(content),
                    file: rel_dir.to_string_lossy().replace('\\', "/"),
                    line: 1,
                    reason,
                });
            }
            return scan;
        }
    };
    let file = if rel_dir_text.is_empty() {
        "mmakefile.src".to_owned()
    } else {
        format!("{rel_dir_text}/mmakefile.src")
    };
    let lines = logical_lines(content, line_states);
    let rules = parse_rules(lines);
    let mut scan = GenmoduleHeaderRuleScan::default();

    for (stamp_index, stamp_rule) in rules.iter().enumerate() {
        // Other legitimate GENMODULE uses (writefiles, writefd, and similar)
        // are deliberately outside this capability and must not become gaps.
        let stamp_candidate = stamp_rule.target.contains(".includes-generated");
        if !stamp_candidate || stamp_rule.state == ConditionalTruth::False {
            continue;
        }

        let possible_owners: Vec<&Rule> = rules
            .iter()
            .filter(|rule| {
                rule.state != ConditionalTruth::False
                    && rule.target.trim() != stamp_rule.target.trim()
                    && rule
                        .prerequisites
                        .split_whitespace()
                        .any(|word| word.contains(".includes-generated"))
            })
            .collect();
        let owner = match possible_owners.as_slice() {
            [rule] if safe_target_name(rule.target.trim()) => rule.target.trim().to_owned(),
            [rule] => rule.target.trim().to_owned(),
            [] => "<unknown-owner>".to_owned(),
            owners => owners
                .iter()
                .map(|rule| rule.target.trim())
                .collect::<Vec<_>>()
                .join(","),
        };
        let reject = |reason: String| GenmoduleHeaderRuleRejection {
            owner: owner.clone(),
            file: file.clone(),
            line: stamp_rule.line + 1,
            reason,
        };

        if stamp_rule.state == ConditionalTruth::Unknown
            || stamp_rule.conditional_syntax
            || stamp_rule.recipes.iter().any(|recipe| {
                recipe.state == ConditionalTruth::Unknown || recipe.conditional_syntax
            })
            || possible_owners
                .iter()
                .any(|rule| rule.state == ConditionalTruth::Unknown || rule.conditional_syntax)
        {
            scan.rejected.push(reject(
                "owner, stamp, or recipe is affected by an unresolved Make conditional".into(),
            ));
            continue;
        }
        if !stamp_rule.target.trim().eq(STAMP) {
            scan.rejected.push(reject(format!(
                "stamp target `{}` is not the bounded `{STAMP}` output",
                stamp_rule.target.trim()
            )));
            continue;
        }
        if possible_owners.len() != 1 {
            scan.rejected.push(reject(format!(
                "stamp has {} possible named owners; exactly one is required",
                possible_owners.len()
            )));
            continue;
        }
        let owner_rule = possible_owners[0];
        if !safe_target_name(owner_rule.target.trim()) {
            scan.rejected.push(reject(format!(
                "owner target `{}` is not one safe named target",
                owner_rule.target.trim()
            )));
            continue;
        }
        if owner_rule.prerequisites.trim() != STAMP || !owner_rule.recipes.is_empty() {
            scan.rejected.push(reject(
                "owner must depend only on the stamp and have no recipe".into(),
            ));
            continue;
        }
        if owner_rule.continued || stamp_rule.continued {
            scan.rejected.push(reject(
                "continued owner or stamp rule is not represented".into(),
            ));
            continue;
        }
        let Some(parsed) = parse_recipe(&stamp_rule.recipes) else {
            scan.rejected.push(reject(format!(
                "stamp recipe has {} commands or a command outside the closed GENMODULE/ECHO/TOUCH form",
                stamp_rule.recipes.len()
            )));
            continue;
        };
        if parsed.commands.iter().any(|command| {
            command.module != parsed.commands[0].module
                || command.modtype != parsed.commands[0].modtype
        }) {
            scan.rejected.push(reject(
                "GENMODULE calls do not use one module and module type".into(),
            ));
            continue;
        }
        let mut config_relative = None;
        let mut config_error = None;
        for command in &parsed.commands {
            let config_value = match evaluate_at_line(
                &command.config,
                command.source_line,
                scope,
                dirs,
                source_root,
                rel_dir,
            ) {
                Ok(value) => value,
                Err(reason) => {
                    config_error = Some(reason);
                    break;
                }
            };
            let relative = match source_local_config_path(&config_value, &rel_dir_text) {
                Ok(value) => value,
                Err(reason) => {
                    config_error = Some(reason);
                    break;
                }
            };
            if config_relative
                .as_ref()
                .is_some_and(|previous| previous != &relative)
            {
                config_error =
                    Some("GENMODULE calls resolve to different source-tree config files".into());
                break;
            }
            config_relative = Some(relative);
        }
        let Some(config_relative) = config_relative else {
            scan.rejected.push(reject(
                config_error.unwrap_or_else(|| "GENMODULE recipe has no config file".into()),
            ));
            continue;
        };
        if let Some(reason) = config_error {
            scan.rejected.push(reject(reason));
            continue;
        }
        let config_path = match confined_source_file(source_root, &config_relative) {
            Ok(path) => path,
            Err(reason) => {
                scan.rejected.push(reject(reason));
                continue;
            }
        };
        let module = parsed.commands[0].module.clone();
        if !safe_basename(&module, "") {
            scan.rejected.push(reject(format!(
                "GENMODULE module `{module}` is not a safe basename"
            )));
            continue;
        }
        let Some(modtype) = GenmoduleType::parse(&parsed.commands[0].modtype) else {
            scan.rejected.push(reject(format!(
                "GENMODULE module type `{}` is not supported by the reference tool",
                parsed.commands[0].modtype
            )));
            continue;
        };
        let roots = match canonical_output_roots(stamp_rule.line, scope, dirs, source_root, rel_dir)
        {
            Ok(roots) => roots,
            Err(reason) => {
                scan.rejected.push(reject(reason));
                continue;
            }
        };
        let layout =
            match classify_layout(&parsed.commands, &roots, scope, dirs, source_root, rel_dir) {
                Ok(layout) => layout,
                Err(reason) => {
                    scan.rejected.push(reject(reason));
                    continue;
                }
            };
        if let Err(reason) = validate_stamp_prerequisites(
            stamp_rule,
            StampPrerequisiteContext {
                config_relative: &config_relative,
                layout,
                roots: &roots,
                scope,
                dirs,
                source_root,
                rel_dir,
            },
        ) {
            scan.rejected.push(reject(reason));
            continue;
        }
        if layout == GenmoduleHeaderLayout::PublicOnly
            && !has_bounded_order_only_directory_producer(&rules)
        {
            scan.rejected.push(reject(
                "PublicOnly requires one safe local order-only directory producer (`%mkdir_q dir=\"$@\"`)".into(),
            ));
            continue;
        }

        let include_name = match include_name_from_config(&config_path, &module, modtype) {
            Ok(value) => value,
            Err(reason) => {
                scan.rejected.push(reject(reason));
                continue;
            }
        };
        let mut outputs = Vec::new();
        let header_rel = [
            format!("clib/{include_name}_protos.h"),
            format!("inline/{include_name}.h"),
            format!("defines/{include_name}.h"),
            format!("defines/{include_name}_LVO.h"),
            format!("proto/{include_name}.h"),
        ];
        match layout {
            GenmoduleHeaderLayout::Full | GenmoduleHeaderLayout::PrivateOnly => {
                outputs.extend(header_rel.iter().cloned().map(|relative_path| {
                    GenmoduleHeaderOutput {
                        destination: GenmoduleHeaderDestination::Private,
                        relative_path,
                    }
                }));
                outputs.push(GenmoduleHeaderOutput {
                    destination: GenmoduleHeaderDestination::Private,
                    relative_path: format!("{module}_libdefs.h"),
                });
            }
            GenmoduleHeaderLayout::PublicOnly => {}
        }
        if matches!(
            layout,
            GenmoduleHeaderLayout::Full | GenmoduleHeaderLayout::PublicOnly
        ) {
            for destination in [
                GenmoduleHeaderDestination::Geninc,
                GenmoduleHeaderDestination::Sdk,
            ] {
                outputs.extend(header_rel.iter().cloned().map(|relative_path| {
                    GenmoduleHeaderOutput {
                        destination,
                        relative_path,
                    }
                }));
            }
        }

        // A repeated stamp rule is a separate producer even if Make would
        // merge its prerequisites. Do not choose one recipe by source order.
        let repeated_stamp = rules.iter().enumerate().any(|(index, rule)| {
            index != stamp_index
                && rule.state != ConditionalTruth::False
                && rule.target.trim() == STAMP
        });
        if repeated_stamp {
            scan.rejected
                .push(reject("stamp target has multiple rule blocks".into()));
            continue;
        }

        scan.declarations.push(GenmoduleHeaderRuleDecl {
            owner: owner_rule.target.trim().to_owned(),
            file: file.clone(),
            line: stamp_rule.line + 1,
            declaring_dir: rel_dir_text.clone(),
            config: config_relative,
            module,
            modtype,
            layout,
            include_name,
            outputs,
        });
    }

    // Keep a broken dependency visible even when its stamp producer has been
    // removed or renamed. Do not inspect unrelated GENMODULE commands here.
    for owner_rule in rules.iter().filter(|rule| {
        rule.state != ConditionalTruth::False
            && rule
                .prerequisites
                .split_whitespace()
                .any(|word| word.contains(".includes-generated"))
    }) {
        let has_local_producer = rules.iter().any(|candidate| {
            candidate.state != ConditionalTruth::False
                && candidate.target.trim().contains(".includes-generated")
                && owner_rule
                    .prerequisites
                    .split_whitespace()
                    .any(|word| word == candidate.target.trim())
        });
        if !has_local_producer
            && !scan.rejected.iter().any(|rejection| {
                rejection.owner == owner_rule.target.trim() && rejection.line == owner_rule.line + 1
            })
        {
            scan.rejected.push(GenmoduleHeaderRuleRejection {
                owner: owner_rule.target.trim().to_owned(),
                file: file.clone(),
                line: owner_rule.line + 1,
                reason: "owner depends on an includes-generated stamp with no local producer"
                    .into(),
            });
        }
    }

    scan
}

#[derive(Debug, Clone)]
struct GenmoduleCommand {
    config: String,
    dest: String,
    action: String,
    module: String,
    modtype: String,
    source_line: usize,
}

#[derive(Debug)]
struct ParsedRecipe {
    commands: Vec<GenmoduleCommand>,
}

#[derive(Debug)]
struct CanonicalRoots {
    private_include: String,
    geninc: String,
    sdk: String,
    module_directory: String,
}

#[derive(Clone, Copy)]
struct StampPrerequisiteContext<'a> {
    config_relative: &'a str,
    layout: GenmoduleHeaderLayout,
    roots: &'a CanonicalRoots,
    scope: &'a crate::make_vars::VarScope,
    dirs: &'a crate::dirs::DirVars,
    source_root: &'a Path,
    rel_dir: &'a Path,
}

pub(crate) fn evaluate_at_line(
    raw: &str,
    line: usize,
    scope: &crate::make_vars::VarScope,
    dirs: &crate::dirs::DirVars,
    source_root: &Path,
    rel_dir: &Path,
) -> Result<String, String> {
    let context = MakeExprContext::new(scope, dirs, line, source_root, rel_dir);
    evaluate_make_expr(raw, &context)
        .map_err(|error| format!("cannot resolve Make expression `{raw}`: {error}"))
}

fn canonical_output_roots(
    line: usize,
    scope: &crate::make_vars::VarScope,
    dirs: &crate::dirs::DirVars,
    source_root: &Path,
    rel_dir: &Path,
) -> Result<CanonicalRoots, String> {
    Ok(CanonicalRoots {
        private_include: evaluate_at_line(PRIVATE_EXPR, line, scope, dirs, source_root, rel_dir)?,
        geninc: evaluate_at_line(GENINC_EXPR, line, scope, dirs, source_root, rel_dir)?,
        sdk: evaluate_at_line(SDK_EXPR, line, scope, dirs, source_root, rel_dir)?,
        module_directory: evaluate_at_line(
            "$(GENDIR)/$(CURDIR)",
            line,
            scope,
            dirs,
            source_root,
            rel_dir,
        )?,
    })
}

fn classify_layout(
    commands: &[GenmoduleCommand],
    roots: &CanonicalRoots,
    scope: &crate::make_vars::VarScope,
    dirs: &crate::dirs::DirVars,
    source_root: &Path,
    rel_dir: &Path,
) -> Result<GenmoduleHeaderLayout, String> {
    let destinations = commands
        .iter()
        .map(|command| {
            evaluate_at_line(
                &command.dest,
                command.source_line,
                scope,
                dirs,
                source_root,
                rel_dir,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let actions: Vec<_> = commands
        .iter()
        .map(|command| command.action.as_str())
        .collect();

    if actions
        == [
            "writeincludes",
            "writelibdefs",
            "writeincludes",
            "writeincludes",
        ]
        && destinations
            == vec![
                roots.private_include.clone(),
                roots.private_include.clone(),
                roots.geninc.clone(),
                roots.sdk.clone(),
            ]
    {
        return Ok(GenmoduleHeaderLayout::Full);
    }
    if actions == ["writeincludes", "writelibdefs"]
        && destinations == vec![roots.private_include.clone(), roots.private_include.clone()]
    {
        return Ok(GenmoduleHeaderLayout::PrivateOnly);
    }
    if actions == ["writeincludes", "writeincludes"]
        && destinations == vec![roots.geninc.clone(), roots.sdk.clone()]
    {
        return Ok(GenmoduleHeaderLayout::PublicOnly);
    }
    Err(format!(
        "GENMODULE action/destination sequence is outside Full, PrivateOnly, and PublicOnly layouts: actions={actions:?}, destinations={destinations:?}"
    ))
}

fn validate_stamp_prerequisites(
    stamp_rule: &Rule,
    context: StampPrerequisiteContext<'_>,
) -> Result<(), String> {
    let mut sections = stamp_rule.prerequisites.split('|');
    let normal: Vec<_> = sections
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let order_only: Vec<_> = sections
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    if sections.next().is_some() {
        return Err("stamp prerequisite list has multiple order-only separators".into());
    }
    if normal
        .iter()
        .filter(|prerequisite| **prerequisite == "$(GENMODULE)")
        .count()
        != 1
    {
        return Err("stamp must have exactly one normal `$(GENMODULE)` prerequisite".into());
    }
    let config_prerequisites: Vec<_> = normal
        .iter()
        .copied()
        .filter(|prerequisite| *prerequisite != "$(GENMODULE)")
        .collect();
    if config_prerequisites.len() > 1 {
        return Err("stamp has more than one source config prerequisite".into());
    }
    for prerequisite in config_prerequisites {
        let resolved = evaluate_at_line(
            prerequisite,
            stamp_rule.line,
            context.scope,
            context.dirs,
            context.source_root,
            context.rel_dir,
        )?;
        if source_local_config_path(&resolved, &stamp_rule_directory(context.rel_dir)?)?
            != context.config_relative
        {
            return Err(format!(
                "source config prerequisite `{prerequisite}` does not resolve to recipe config `{}`",
                context.config_relative
            ));
        }
    }
    match context.layout {
        GenmoduleHeaderLayout::PublicOnly => {
            if order_only.len() != 1 {
                return Err(
                    "PublicOnly stamp requires exactly one order-only local output directory prerequisite".into(),
                );
            }
            let resolved = evaluate_at_line(
                order_only[0],
                stamp_rule.line,
                context.scope,
                context.dirs,
                context.source_root,
                context.rel_dir,
            )?;
            if resolved != context.roots.module_directory {
                return Err(format!(
                    "PublicOnly order-only prerequisite `{}` does not resolve to the local output directory",
                    order_only[0]
                ));
            }
        }
        GenmoduleHeaderLayout::Full | GenmoduleHeaderLayout::PrivateOnly => {
            if !order_only.is_empty() {
                return Err("Full/PrivateOnly stamp may not have order-only prerequisites".into());
            }
        }
    }
    Ok(())
}

fn stamp_rule_directory(rel_dir: &Path) -> Result<String, String> {
    safe_relative_directory(rel_dir)
}

fn has_bounded_order_only_directory_producer(rules: &[Rule]) -> bool {
    let producers: Vec<_> = rules
        .iter()
        .filter(|rule| {
            rule.target.trim() == "$(GENDIR)/$(CURDIR)" && rule.state != ConditionalTruth::False
        })
        .collect();
    matches!(producers.as_slice(), [rule]
        if rule.prerequisites.trim().is_empty()
            && !rule.continued
            && !rule.conditional_syntax
            && rule.state == ConditionalTruth::True
            && rule.recipes.len() == 1
            && rule.recipes[0].text.trim() == "%mkdir_q dir=\"$@\""
            && !rule.recipes[0].continued
            && !rule.recipes[0].conditional_syntax
            && rule.recipes[0].state == ConditionalTruth::True)
}

fn parse_recipe(recipes: &[RecipeLine]) -> Option<ParsedRecipe> {
    if !matches!(recipes.len(), 4 | 6)
        || recipes.iter().any(|recipe| recipe.continued)
        || !safe_echo(&recipes[0].text)
        || recipes.last()?.text.trim() != "@$(TOUCH) $@"
    {
        return None;
    }
    let mut commands = Vec::new();
    for recipe in &recipes[1..recipes.len() - 1] {
        let words: Vec<&str> = recipe.text.split_whitespace().collect();
        if words.len() != 8 || words[0] != "@$(GENMODULE)" || words[1] != "-c" || words[3] != "-d" {
            return None;
        }
        if words[4..]
            .iter()
            .any(|word| word.contains([';', '&', '|', '<', '>', '`', '\\', '"', '\'']))
        {
            return None;
        }
        commands.push(GenmoduleCommand {
            config: words[2].to_owned(),
            dest: words[4].to_owned(),
            action: words[5].to_owned(),
            module: words[6].to_owned(),
            modtype: words[7].to_owned(),
            source_line: recipe.source_line,
        });
    }
    Some(ParsedRecipe { commands })
}

pub(crate) fn safe_echo(line: &str) -> bool {
    let Some(message) = line
        .trim()
        .strip_prefix("@$(ECHO) \"")
        .and_then(|value| value.strip_suffix('"'))
    else {
        return false;
    };
    !message.is_empty()
        && message
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || " .,:!_-()".contains(character))
}

pub(crate) fn logical_lines(
    content: &str,
    line_states: Option<&[ConditionalTruth]>,
) -> Vec<LogicalLine> {
    let physical: Vec<&str> = content.lines().collect();
    let mut result = Vec::new();
    let mut index = 0usize;
    let mut conditional_depth = 0usize;
    while index < physical.len() {
        let first = index;
        let mut text = String::new();
        let mut physical_lines = Vec::new();
        let mut continued = false;
        loop {
            let line = physical[index].trim_end_matches('\r');
            physical_lines.push(index);
            let trimmed = line.trim_end();
            if let Some(prefix) = trimmed.strip_suffix('\\') {
                text.push_str(prefix.trim_end());
                text.push(' ');
                continued = true;
                index += 1;
                if index == physical.len() {
                    break;
                }
                continue;
            }
            if continued {
                text.push_str(line.trim());
            } else {
                text.push_str(line);
            }
            index += 1;
            break;
        }
        let line_state = line_states.map_or_else(
            || ConditionalTruth::True,
            |states| {
                let mut seen_true = false;
                let mut seen_false = false;
                let mut seen_unknown = false;
                for source_line in &physical_lines {
                    match states
                        .get(*source_line)
                        .copied()
                        .unwrap_or(ConditionalTruth::Unknown)
                    {
                        ConditionalTruth::True => seen_true = true,
                        ConditionalTruth::False => seen_false = true,
                        ConditionalTruth::Unknown => seen_unknown = true,
                    }
                }
                if seen_unknown || (seen_true && seen_false) {
                    ConditionalTruth::Unknown
                } else if seen_false {
                    ConditionalTruth::False
                } else {
                    ConditionalTruth::True
                }
            },
        );
        let directive = conditional_word(text.trim()).map(str::to_owned);
        let conditional_syntax = line_states.is_none() && conditional_depth > 0;
        result.push(LogicalLine {
            text,
            source_line: first,
            continued,
            state: line_state,
            conditional_syntax,
        });
        if line_states.is_none() {
            match directive.as_deref() {
                Some("ifeq" | "ifneq" | "ifdef" | "ifndef") => conditional_depth += 1,
                Some("endif") => conditional_depth = conditional_depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    result
}

fn is_conditional_directive(line: &str) -> bool {
    matches!(
        conditional_word(line),
        Some("ifeq" | "ifneq" | "ifdef" | "ifndef" | "else" | "endif")
    )
}

fn conditional_word(line: &str) -> Option<&str> {
    line.split_whitespace().next()
}

pub(crate) fn parse_rules(lines: Vec<LogicalLine>) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut current: Option<Rule> = None;
    for line in lines {
        let text = line.text.trim_end_matches('\r');
        if text.starts_with('\t') {
            if let Some(rule) = current.as_mut() {
                if rule.state == ConditionalTruth::False || line.state == ConditionalTruth::False {
                    continue;
                }
                rule.recipes.push(RecipeLine {
                    text: text.trim().to_owned(),
                    source_line: line.source_line,
                    continued: line.continued,
                    state: line.state,
                    conditional_syntax: line.conditional_syntax,
                });
                rule.conditional_syntax |= line.conditional_syntax;
            }
            continue;
        }
        let header = text.trim();
        if header.is_empty() || header.starts_with('#') {
            continue;
        }
        if is_conditional_directive(header) {
            if current.as_ref().is_some_and(|rule| rule.recipes.is_empty()) {
                rules.push(current.take().expect("current rule was just inspected"));
            } else if let Some(rule) = current.as_mut() {
                rule.conditional_syntax |=
                    line.conditional_syntax || line.state == ConditionalTruth::Unknown;
            }
            continue;
        }
        if let Some(rule) = current.take() {
            rules.push(rule);
        }
        let Some((target, prerequisites)) = header.split_once(':') else {
            continue;
        };
        let target = target.trim();
        if target.is_empty() || prerequisites.starts_with([':', '=']) {
            continue;
        }
        current = Some(Rule {
            target: target.to_owned(),
            prerequisites: prerequisites.trim().to_owned(),
            line: line.source_line,
            continued: line.continued,
            state: line.state,
            conditional_syntax: line.conditional_syntax,
            recipes: Vec::new(),
        });
    }
    if let Some(rule) = current {
        rules.push(rule);
    }
    rules
}

fn owner_hint(content: &str) -> String {
    for line in content.lines() {
        let line = line.trim();
        if line.contains(".includes-generated") {
            if let Some((owner, _)) = line.split_once(':') {
                let owner = owner.trim();
                if safe_target_name(owner) {
                    return owner.to_owned();
                }
            }
        }
    }
    "<unknown-owner>".to_owned()
}

pub(crate) fn safe_target_name(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('.')
        && value != ".."
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.+-".contains(character))
}

pub(crate) fn safe_basename(value: &str, extension: &str) -> bool {
    if extension.is_empty() {
        return value == value.trim()
            && !value.is_empty()
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "_-".contains(ch));
    }
    let Some(stem) = value.strip_suffix(extension) else {
        return false;
    };
    !stem.is_empty()
        && stem != "."
        && stem != ".."
        && stem
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_-".contains(ch))
}

pub(crate) fn safe_relative_directory(rel_dir: &Path) -> Result<String, String> {
    let raw = rel_dir
        .to_str()
        .ok_or_else(|| "declaring directory is not valid UTF-8".to_owned())?
        .replace('\\', "/");
    if raw.is_empty() || raw == "." {
        return Ok(String::new());
    }
    if raw.starts_with('/') || raw.contains([';', '$', '\n', '\r']) {
        return Err(format!(
            "declaring directory `{raw}` is not source-relative"
        ));
    }
    let mut components = Vec::new();
    for component in raw.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(format!("declaring directory `{raw}` contains traversal"));
        }
        if !component
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.+-".contains(character))
        {
            return Err(format!(
                "declaring directory `{raw}` has an unsafe component"
            ));
        }
        components.push(component);
    }
    Ok(components.join("/"))
}

pub(crate) fn source_local_config_path(value: &str, declaring_dir: &str) -> Result<String, String> {
    let Some(relative) = value.strip_prefix("${AROS_SOURCE_DIR}/") else {
        return Err(format!(
            "GENMODULE config `{value}` does not resolve under AROS_SOURCE_DIR"
        ));
    };
    if !safe_source_relative_file(relative) {
        return Err(format!(
            "GENMODULE config `{value}` is not one safe source-relative .conf path"
        ));
    }
    let parent = Path::new(relative)
        .parent()
        .and_then(Path::to_str)
        .unwrap_or("");
    if parent != declaring_dir {
        return Err(format!(
            "GENMODULE config `{value}` is not local to declaring directory `{declaring_dir}`"
        ));
    }
    Ok(relative.to_owned())
}

fn safe_source_relative_file(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains([
            '\\', ';', '$', '\n', '\r', ':', '<', '>', '|', '"', '\'', '`',
        ])
        && value.split('/').all(|component| {
            !component.is_empty()
                && component != "."
                && component != ".."
                && component.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "_.+-".contains(character)
                })
        })
        && Path::new(value)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| safe_basename(name, ".conf"))
}

pub(crate) fn confined_source_file(source_root: &Path, relative: &str) -> Result<PathBuf, String> {
    if !safe_source_relative_file(relative) {
        return Err(format!(
            "source config `{relative}` is not one safe source-relative .conf file"
        ));
    }
    let root = source_root
        .canonicalize()
        .map_err(|error| format!("source root cannot be resolved: {error}"))?;
    if !root.is_dir() {
        return Err("source root is not a directory".into());
    }
    let mut candidate = root.clone();
    let components: Vec<_> = relative.split('/').collect();
    for (index, component) in components.iter().enumerate() {
        candidate.push(component);
        if index + 1 < components.len() {
            let metadata = fs::symlink_metadata(&candidate).map_err(|error| {
                format!(
                    "source config parent {} cannot be read: {error}",
                    candidate.display()
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "source config parent {} is not a real in-tree directory",
                    candidate.display()
                ));
            }
        } else {
            let metadata = fs::symlink_metadata(&candidate).map_err(|error| {
                format!(
                    "source config {} cannot be read: {error}",
                    candidate.display()
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(format!(
                    "source config {} is not a regular non-symlink file",
                    candidate.display()
                ));
            }
        }
    }
    let physical = candidate
        .canonicalize()
        .map_err(|error| format!("source config cannot be resolved: {error}"))?;
    if !physical.starts_with(&root) {
        return Err("source config resolves outside the source tree".into());
    }
    Ok(physical)
}

fn include_name_from_config(
    path: &Path,
    module: &str,
    modtype: GenmoduleType,
) -> Result<String, String> {
    let source = fs::read_to_string(path).map_err(|error| {
        format!(
            "genmodule config {} cannot be read: {error}",
            path.display()
        )
    })?;
    let mut section: Option<&str> = None;
    let mut config_seen = false;
    let mut include_name = None;
    let mut saw_includes = false;
    let mut saw_noincludes = false;

    for (line_number, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if let Some(opened) = line.strip_prefix("##begin ") {
            if section.is_some() {
                return Err(format!(
                    "config line {} nests unsupported sections",
                    line_number + 1
                ));
            }
            let name = opened.split_whitespace().next().unwrap_or_default();
            match name {
                "config" | "cdef" | "cdefprivate" | "functionlist" | "cfunctionlist" => {}
                "interface" | "class" => {
                    return Err(format!(
                        "config line {} declares `{name}` output-bearing sections this rule does not model",
                        line_number + 1
                    ));
                }
                _ => {
                    return Err(format!(
                        "config line {} opens unsupported section `{name}`",
                        line_number + 1
                    ));
                }
            }
            if name == "config" {
                if config_seen {
                    return Err("genmodule config has multiple config sections".into());
                }
                config_seen = true;
            }
            section = Some(name);
            continue;
        }
        if let Some(closed) = line.strip_prefix("##end ") {
            let name = closed.split_whitespace().next().unwrap_or_default();
            if section != Some(name) {
                return Err(format!(
                    "config line {} has a mismatched section end",
                    line_number + 1
                ));
            }
            section = None;
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match section {
            Some("config") => {
                let (key, value) = line
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| format!("config line {} has no value", line_number + 1))?;
                let value = value.trim();
                if !CONFIG_KEYS.contains(&key) {
                    return Err(format!(
                        "config line {} uses unsupported config option `{key}`",
                        line_number + 1
                    ));
                }
                if value.is_empty() {
                    return Err(format!(
                        "config line {} has an empty `{key}` value",
                        line_number + 1
                    ));
                }
                if key == "includename" {
                    if include_name.is_some() || !safe_basename(value, "") {
                        return Err(format!(
                            "config line {} has duplicate or unsafe includename `{value}`",
                            line_number + 1
                        ));
                    }
                    include_name = Some(value.to_owned());
                } else if key == "options" {
                    let tokens: Vec<&str> = value
                        .split(|character: char| character == ',' || character.is_whitespace())
                        .filter(|token| !token.is_empty())
                        .collect();
                    if tokens.is_empty() || tokens.iter().any(|token| !OPTIONS.contains(token)) {
                        return Err(format!(
                            "config line {} has unsupported `options` values",
                            line_number + 1
                        ));
                    }
                    let has_includes = tokens.contains(&"includes");
                    let has_noincludes = tokens.contains(&"noincludes");
                    if has_includes && has_noincludes {
                        return Err(format!(
                            "config line {} combines includes and noincludes",
                            line_number + 1
                        ));
                    }
                    saw_includes |= has_includes;
                    saw_noincludes |= has_noincludes;
                    if saw_includes && saw_noincludes {
                        return Err(format!(
                            "config line {} combines includes and noincludes",
                            line_number + 1
                        ));
                    }
                }
            }
            Some("cdef" | "functionlist" | "cfunctionlist") => {}
            Some(other) => {
                return Err(format!(
                    "config line {} is in unsupported section `{other}`",
                    line_number + 1
                ));
            }
            None => {
                return Err(format!(
                    "config line {} contains content outside a recognized section",
                    line_number + 1
                ));
            }
        }
    }
    if section.is_some() || !config_seen {
        return Err("genmodule config is unclosed or lacks a config section".into());
    }
    let include_name = include_name.unwrap_or_else(|| module.to_owned());
    if !safe_basename(&include_name, "") {
        return Err(format!("effective includename `{include_name}` is unsafe"));
    }

    let includes = if saw_includes {
        true
    } else if saw_noincludes {
        false
    } else if matches!(modtype, GenmoduleType::Library | GenmoduleType::Resource) {
        true
    } else {
        return Err(
            "non-library/resource config needs explicit `options includes` to prove the writeincludes output set"
                .into(),
        );
    };
    if !includes {
        return Err("config does not enable public includes for writeincludes".into());
    }
    Ok(include_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{root, TempTree};

    const RECIPE: &str = concat!(
        "owner: $(GENDIR)/$(CURDIR)/.includes-generated\n",
        "$(GENDIR)/$(CURDIR)/.includes-generated : $(GENMODULE)\n",
        "\t@$(ECHO) \"Generating API headers...\"\n",
        "\t@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/module.conf -d $(GENDIR)/$(CURDIR)/include writeincludes module resource\n",
        "\t@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/module.conf -d $(GENDIR)/$(CURDIR)/include writelibdefs module resource\n",
        "\t@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/module.conf -d $(GENDIR)/include writeincludes module resource\n",
        "\t@$(GENMODULE) -c $(SRCDIR)/$(CURDIR)/module.conf -d $(AROS_INCLUDES) writeincludes module resource\n",
        "\t@$(TOUCH) $@\n",
    );

    fn temp_source() -> TempTree {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("unit")).unwrap();
        fs::create_dir_all(tree.0.join("config")).unwrap();
        fs::write(
            tree.0.join("config/make.cfg.in"),
            "AROS_INCLUDES := ${AROS_BUILD_DIR}/SDK/include\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("unit/module.conf"),
            "##begin config\nversion 1.0\nlibbase ModuleBase\n##end config\n\n##begin functionlist\nvoid F(void) (D0)\n##end functionlist\n",
        )
        .unwrap();
        tree
    }

    #[test]
    fn actual_p4_clocksource_stamp_is_recognized_with_all_outputs() {
        let source = root();
        let directory = Path::new("rom/kernel");
        let content = fs::read_to_string(source.join(directory).join("mmakefile.src")).unwrap();
        let scan = collect_genmodule_header_rules(&content, &source, directory);
        assert!(scan.rejected.is_empty(), "{:#?}", scan.rejected);
        assert_eq!(scan.declarations.len(), 1, "{:#?}", scan.declarations);
        let declaration = &scan.declarations[0];
        assert_eq!(declaration.owner, "kernel-clocksource-gen-includes");
        assert_eq!(declaration.config, "rom/kernel/clocksource.conf");
        assert_eq!(declaration.include_name, "clocksource");
        assert_eq!(declaration.outputs.len(), 16);
        assert_eq!(
            declaration
                .outputs
                .iter()
                .filter(|output| output.destination == GenmoduleHeaderDestination::Private)
                .count(),
            6
        );
    }

    #[test]
    fn all_five_actual_stamp_layouts_resolve_aliases_and_configs() {
        let source = root();
        let fixtures = [
            (
                "rom/kernel",
                "kernel-clocksource-gen-includes",
                GenmoduleHeaderLayout::Full,
                16,
                "rom/kernel/clocksource.conf",
            ),
            (
                "workbench/libs/gl",
                "workbench-libs-gl-gen-includes",
                GenmoduleHeaderLayout::Full,
                16,
                "workbench/libs/gl/gl.conf",
            ),
            (
                "workbench/tools/SysExplorer",
                "workbench-tools-sysexplorer-gen-includes",
                GenmoduleHeaderLayout::PrivateOnly,
                6,
                "workbench/tools/SysExplorer/sysexp.conf",
            ),
            (
                "workbench/prefs/network",
                "workbench-prefs-network-gen-includes",
                GenmoduleHeaderLayout::PrivateOnly,
                6,
                "workbench/prefs/network/netprefs.conf",
            ),
            (
                "workbench/libs/vhi",
                "workbench-libs-vhi-includes",
                GenmoduleHeaderLayout::PublicOnly,
                10,
                "workbench/libs/vhi/vhi.conf",
            ),
        ];
        for (directory, owner, layout, output_count, config) in fixtures {
            let rel_dir = Path::new(directory);
            let content = fs::read_to_string(source.join(rel_dir).join("mmakefile.src")).unwrap();
            let scan = collect_genmodule_header_rules(&content, &source, rel_dir);
            assert!(
                scan.rejected.is_empty(),
                "{directory}: {:#?}",
                scan.rejected
            );
            assert_eq!(scan.declarations.len(), 1, "{directory}");
            let declaration = &scan.declarations[0];
            assert_eq!(declaration.owner, owner, "{directory}");
            assert_eq!(declaration.layout, layout, "{directory}");
            assert_eq!(declaration.config, config, "{directory}");
            assert_eq!(declaration.outputs.len(), output_count, "{directory}");
        }
    }

    #[test]
    fn missing_or_unsafe_stamp_directory_producers_keep_the_owner() {
        let source = root();
        let directory = Path::new("workbench/libs/vhi");
        let content = fs::read_to_string(source.join(directory).join("mmakefile.src")).unwrap();
        let missing = content.replace("$(GENDIR)/$(CURDIR):\n\t%mkdir_q dir=\"$@\"\n", "");
        let scan = collect_genmodule_header_rules(&missing, &source, directory);
        assert!(scan.declarations.is_empty());
        assert_eq!(scan.rejected.len(), 1);
        assert_eq!(scan.rejected[0].owner, "workbench-libs-vhi-includes");
        assert!(scan.rejected[0]
            .reason
            .contains("order-only directory producer"));

        let unsafe_recipe = content.replace("%mkdir_q dir=\"$@\"", "mkdir -p \"$@\"");
        let scan = collect_genmodule_header_rules(&unsafe_recipe, &source, directory);
        assert!(scan.declarations.is_empty());
        assert_eq!(scan.rejected.len(), 1);
        assert_eq!(scan.rejected[0].owner, "workbench-libs-vhi-includes");
    }

    #[test]
    fn missing_stamp_producer_is_a_named_failure() {
        let tree = temp_source();
        let content = "broken-header-owner: $(GENDIR)/$(CURDIR)/.includes-generated\n";
        let scan = collect_genmodule_header_rules(content, &tree.0, Path::new("unit"));
        assert!(scan.declarations.is_empty());
        assert_eq!(scan.rejected.len(), 1);
        assert_eq!(scan.rejected[0].owner, "broken-header-owner");
        assert!(scan.rejected[0].reason.contains("no local producer"));
    }

    #[test]
    fn generic_owner_and_configured_includename_are_not_hardcoded() {
        let tree = temp_source();
        let config = tree.0.join("unit/module.conf");
        let source = fs::read_to_string(&config)
            .unwrap()
            .replace("version 1.0", "version 1.0\nincludename custom_api");
        fs::write(config, source).unwrap();
        let content = RECIPE
            .replace("owner", "arbitrary-resource-api")
            .replace("module", "arbmod")
            .replace("module.conf", "arbmod.conf");
        fs::rename(
            tree.0.join("unit/module.conf"),
            tree.0.join("unit/arbmod.conf"),
        )
        .unwrap();
        let scan = collect_genmodule_header_rules(&content, &tree.0, Path::new("unit"));
        assert!(scan.rejected.is_empty(), "{:#?}", scan.rejected);
        assert_eq!(scan.declarations[0].owner, "arbitrary-resource-api");
        assert_eq!(scan.declarations[0].include_name, "custom_api");
        assert!(scan.declarations[0]
            .outputs
            .iter()
            .any(|output| output.relative_path == "proto/custom_api.h"));
        assert!(scan.declarations[0]
            .outputs
            .iter()
            .any(|output| output.relative_path == "arbmod_libdefs.h"));
    }

    #[test]
    fn mixed_commands_and_shell_syntax_are_owner_bearing_rejections() {
        let tree = temp_source();
        let mut mixed = RECIPE.to_owned();
        mixed = mixed.replace(
            "writeincludes module resource\n",
            "writeincludes module resource && touch /tmp/escape\n",
        );
        let scan = collect_genmodule_header_rules(&mixed, &tree.0, Path::new("unit"));
        assert!(scan.declarations.is_empty());
        assert_eq!(scan.rejected.len(), 1);
        assert_eq!(scan.rejected[0].owner, "owner");
    }

    #[test]
    fn traversal_config_path_is_rejected() {
        let tree = temp_source();
        let content = RECIPE.replace(
            "$(SRCDIR)/$(CURDIR)/module.conf",
            "$(SRCDIR)/$(CURDIR)/../module.conf",
        );
        let scan = collect_genmodule_header_rules(&content, &tree.0, Path::new("unit"));
        assert!(scan.declarations.is_empty());
        assert_eq!(scan.rejected.len(), 1);
        assert!(scan.rejected[0].reason.contains("safe source-relative"));
    }

    #[test]
    fn unsupported_includename_and_output_bearing_sections_are_rejected() {
        let tree = temp_source();
        fs::write(
            tree.0.join("unit/module.conf"),
            "##begin config\nversion 1.0\nincludename ../escape\n##end config\n",
        )
        .unwrap();
        let scan = collect_genmodule_header_rules(RECIPE, &tree.0, Path::new("unit"));
        assert!(scan.declarations.is_empty());
        assert!(scan.rejected[0].reason.contains("includename"));

        fs::write(
            tree.0.join("unit/module.conf"),
            "##begin config\nversion 1.0\n##end config\n##begin interface Foo\n##end interface\n",
        )
        .unwrap();
        let scan = collect_genmodule_header_rules(RECIPE, &tree.0, Path::new("unit"));
        assert!(scan.declarations.is_empty());
        assert!(scan.rejected[0].reason.contains("output-bearing"));
    }

    #[test]
    fn unknown_conditional_recipe_line_rejects_the_owner() {
        let tree = temp_source();
        let states = vec![ConditionalTruth::True; RECIPE.lines().count()];
        let mut states = states;
        let recipe_line = RECIPE
            .lines()
            .position(|line| line.contains("writeincludes module resource"))
            .unwrap();
        states[recipe_line] = ConditionalTruth::Unknown;
        let scan = collect_genmodule_header_rules_with_line_states(
            RECIPE,
            &tree.0,
            Path::new("unit"),
            Some(&states),
        );
        assert!(scan.declarations.is_empty());
        assert_eq!(scan.rejected.len(), 1);
        assert_eq!(scan.rejected[0].owner, "owner");
        assert!(scan.rejected[0].reason.contains("conditional"));
    }

    #[test]
    fn inactive_candidate_is_not_reported() {
        let tree = temp_source();
        let states = vec![ConditionalTruth::False; RECIPE.lines().count()];
        let scan = collect_genmodule_header_rules_with_line_states(
            RECIPE,
            &tree.0,
            Path::new("unit"),
            Some(&states),
        );
        assert!(scan.declarations.is_empty());
        assert!(scan.rejected.is_empty());
    }
}
