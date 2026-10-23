//! Declaration-time compiler option projection from a configured Mesa source.
//!
//! This is a bounded source-scope evaluator, not a configure runner or an
//! authentication mechanism. It combines the caller's validated configuration
//! with the six reviewed recursive definitions from AROS' source templates,
//! then evaluates the selected MetaMake declaration and (for the VC4 linklib)
//! the source's immediate GCA assignment.

use super::configured_inventory::InventoryObservation;
use super::ordered_contract::{evaluate_ordered_mesa_recipe_options, OrderedMesaCompilerOptions};
use crate::dirs::DirVars;
use crate::local_make_includes::{
    inline_native_make_configuration_with_templates, LocalMakeIncludeLimits,
};
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::{collect_vars_impl, strip_make_comment, ConditionalTruth};
use crate::parser::{
    is_concrete_build_invocation, join_continuations, macro_arg, macro_argument_names,
    macro_invocations, TargetContext,
};
use aros_common::mesa_configuration::{MesaMakeVersionValue, ValidatedMesaConfiguration};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

const MAX_MMAKE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TEMPLATE_BYTES: u64 = 512 * 1024;
const MAX_INVENTORY_BYTES: u64 = 1024 * 1024;
const VC4_MMAKE: &str = "linklibs-gallium_vc4";
const GCA_ASSIGNMENT: &str = "GCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS)";
const AROS_CFG_INCLUDE: &str = "include $(SRCDIR)/config/aros.cfg";
const INVENTORY_PATH: &str = "$(GALLIUM_PATH)/$(CUR_GALLIUMDIR)/Makefile.sources";
const INVENTORY_GUARD: &str =
    "ifneq ($(wildcard $(GALLIUM_PATH)/$(CUR_GALLIUMDIR)/Makefile.sources),)";
const COMPONENT_INVENTORY_PATH: &str = "$(top_srcdir)/$(CUR_MESADIR)/Makefile.sources";
const COMPONENT_INVENTORY_PRESENT: &str =
    "ifneq ($(wildcard $(top_srcdir)/$(CUR_MESADIR)/Makefile.sources),)";
const COMPONENT_INVENTORY_ABSENT: &str =
    "ifeq ($(wildcard $(top_srcdir)/$(CUR_MESADIR)/Makefile.sources),)";

pub(super) struct ProjectedScope {
    pub options: OrderedMesaCompilerOptions,
    pub inventories: Vec<InventoryObservation>,
    pub context_input: super::configured_source_context::ContextInput,
}

const RECURSIVE_DEFINITIONS: &[(&str, &str, &str)] = &[
    (
        "config/target.cfg.in",
        "TARGET_CFLAGS",
        "$(strip $(OPTIMIZATION_CFLAGS) $(CONFIG_CFLAGS) $(USER_CFLAGS))",
    ),
    (
        "config/target.cfg.in",
        "TARGET_CXXFLAGS",
        "$(strip $(OPTIMIZATION_CFLAGS) $(CONFIG_CXXFLAGS) $(USER_CXXFLAGS))",
    ),
    (
        "config/target.cfg.in",
        "TARGET_CPPFLAGS",
        "$(strip $(USER_INCLUDES) $(CONFIG_CPPFLAGS) $(USER_CPPFLAGS))",
    ),
    (
        "config/make.cfg.in",
        "CFLAGS",
        "$(strip $(TARGET_ISA_CFLAGS) $(TARGET_CFLAGS) $(LLVM_NOWARN))",
    ),
    (
        "config/make.cfg.in",
        "CXXFLAGS",
        "$(strip $(TARGET_ISA_CXXFLAGS) $(TARGET_CXXFLAGS))",
    ),
    (
        "config/make.cfg.in",
        "CPPFLAGS",
        "$(strip $(TARGET_CPPFLAGS))",
    ),
];

/// Project a configured Mesa source to its ordered compiler options at one
/// concrete MetaMake declaration.
///
/// `relative_dir` is the source-relative directory containing `mmakefile.src`;
/// `mmake` is matched exactly against the declaration's `mmake=` argument.
/// `build` is the configured build directory and contributes `build/gen` as
/// `GENDIR`. The VC4 linklib additionally requires and freezes the exact
/// immediate GCA assignment; other declarations receive an empty unused GCA
/// value.
pub(super) fn project(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    dirs: &DirVars,
    target: &TargetContext,
    configuration: &ValidatedMesaConfiguration,
    build_and_ports: (&Path, &Path),
) -> Result<ProjectedScope, String> {
    let (build, ports) = build_and_ports;
    let root = canonical_directory(root, "source root")?;
    let relative_dir = validate_relative_directory(relative_dir)?;
    let declaration_dir = root.join(&relative_dir);
    let canonical_declaration_dir = fs::canonicalize(&declaration_dir)
        .map_err(|error| format!("cannot resolve Mesa declaration directory: {error}"))?;
    if !canonical_declaration_dir.starts_with(&root) || !canonical_declaration_dir.is_dir() {
        return Err("Mesa declaration directory leaves the source tree".into());
    }
    let build = canonical_directory(build, "build directory")?;
    let selected_ports = ports;
    let ports = canonical_directory(ports, "ports directory")?;
    let generated = build.join("gen");

    let mut selected = target.clone();
    let internal_prefix = format!("{mmake}_");
    if selected
        .make_variables
        .keys()
        .any(|name| name.starts_with(&internal_prefix))
    {
        return Err("configured Mesa does not admit external owner-internal Make overrides".into());
    }
    apply_configured_llvm(&mut selected, configuration)?;
    let mut variables = selected.make_variables.clone();
    let mut undefined: Vec<&str> = configuration
        .undefined_variables()
        .iter()
        .map(String::as_str)
        .collect();
    undefined.extend(configuration.llvm_configuration().undefined_variables());
    undefined.extend(configuration.recipe_configuration().undefined_variables());
    for name in &undefined {
        if variables.contains_key(*name) {
            return Err(format!("configured Mesa variable {name} was measured undefined but the selected context defines it"));
        }
    }
    for (name, value) in configuration.make_variables() {
        merge_scalar(&mut variables, &name, value)?;
    }
    for (name, value) in [
        ("SRCDIR", path_text(&root, "source root")?),
        (
            "CURDIR",
            relative_dir
                .to_str()
                .ok_or_else(|| "Mesa declaration directory is not UTF-8".to_owned())?
                .to_owned(),
        ),
        ("GENDIR", path_text(&generated, "generated directory")?),
        ("PORTSDIR", path_text(&ports, "ports directory")?),
    ] {
        merge_scalar(&mut variables, name, value)?;
    }
    selected.make_variables = variables;

    let definitions = extract_recursive_definitions(&root)?;
    verify_aros_configuration_wrapper(&root)?;

    let mmake_path = relative_dir.join("mmakefile.src");
    let mmake_absolute = root.join(&mmake_path);
    let mmake_bytes = read_regular_bounded(&mmake_absolute, MAX_MMAKE_BYTES, "mmakefile.src")?;
    let mmake_text = aros_common::decode_source_bytes(mmake_bytes);
    let wrapper_line = mmake_text
        .lines()
        .position(|line| strip_make_comment(line).trim() == AROS_CFG_INCLUDE)
        .map(|index| index + 1);
    let mmake_text = remove_aros_wrapper_include(&mmake_text)?;
    // The AROS wrapper imports generated global configuration. Its relevant
    // values are represented by `configuration.make_variables()` and the six
    // reviewed source formulas above, so keep that generated scope out of the
    // source scan while preserving the local Mesa include at its actual site.
    let scan = inline_native_make_configuration_with_templates(
        &mmake_text,
        &root,
        &mmake_path,
        LocalMakeIncludeLimits::default(),
        &selected.make_include_bindings,
        &selected.generated_make_templates,
    );
    if !scan.issues.is_empty() {
        return Err(format!(
            "Mesa local Make include scope is not safe: {}",
            scan.issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    let mesa_fragments = scan
        .fragments
        .iter()
        .filter(|fragment| {
            fragment.path == Path::new("workbench/libs/mesa/mesa.cfg")
                && !fragment.is_proven_absent()
        })
        .count();
    if mesa_fragments != 1 {
        return Err("source workbench/libs/mesa/mesa.cfg was not inlined exactly once".into());
    }

    // Preserve undefinedness, not a defined-empty seed: Make's later ?=
    // assignments must still take effect. These names were explicitly checked
    // with $(origin ...) by the retained preflight, never inferred here.
    let mut source = String::new();
    for name in &undefined {
        source.push_str("undefine ");
        source.push_str(name);
        source.push('\n');
    }
    source.push_str(&definitions);
    source.push('\n');
    let prefix_lines = join_continuations(&source).lines().count();
    let scan_joined = join_continuations(&scan.expanded);
    let anchors = crate::make_source_origins::literal_object_source_anchors(
        &root,
        &mmake_path,
        &mmake_text,
        &scan,
        &scan_joined,
    )
    .ok_or("Mesa include expansion does not retain verified source origins")?;
    let mut source_locations = vec![None; prefix_lines];
    source_locations.extend(anchors.into_iter().map(|anchor| {
        if anchor
            .as_ref()
            .is_some_and(|anchor| anchor.source == mmake_path && Some(anchor.line) == wrapper_line)
        {
            None
        } else {
            anchor.map(|anchor| crate::genmf_projection::SourceLineLocation {
                path: root.join(anchor.source),
                line: anchor.line,
            })
        }
    }));
    source.push_str(&scan.expanded);
    let joined = join_continuations(&source);
    if source_locations.len() != joined.lines().count() {
        return Err("Mesa assembled source origins do not cover continuation joins".into());
    }
    let path_lookup = BTreeMap::from([
        ("SRCDIR", root.as_path()),
        ("CURDIR", relative_dir.as_path()),
        ("GENDIR", generated.as_path()),
        ("PORTSDIR", ports.as_path()),
    ]);
    let lookup = |name: &str| {
        path_lookup
            .get(name)
            .map(|path| path.to_string_lossy().into_owned())
    };
    let joined = super::configured_llvm::project_version_format(
        &joined,
        &selected,
        dirs,
        &root,
        &relative_dir,
        &lookup,
    )?;
    let (joined, inventories) = project_inventory_guards(
        &joined,
        &selected,
        dirs,
        (&root, selected_ports, &relative_dir),
        &lookup,
    )?;
    // Both projections replace a line in place, never insert or remove it.
    if source_locations.len() != joined.lines().count() {
        return Err("Mesa source projection changed physical origin alignment".into());
    }
    let (scope, states) = collect_vars_impl(&joined, Some(&selected));
    let generator_lines =
        super::configured_generators::reviewed_lines(&joined, &root, &relative_dir, &selected)?;
    reject_unmodeled_evaluation(&joined, &states, &generator_lines)?;

    verify_remaining_includes(
        &joined,
        &states,
        &scope,
        dirs,
        (&root, &ports),
        &relative_dir,
        &lookup,
    )?;

    // Keep source coordinates, but do not mistake recipe text or deferred
    // Make definitions for declarations executed at this source position.
    let mut declaration_source = String::new();
    let mut define_depth = 0usize;
    for text in joined.lines() {
        let visible = strip_make_comment(text).trim();
        if text.starts_with('\t') {
            declaration_source.push('\n');
            continue;
        }
        if defined_macro_name(text).is_some() {
            define_depth += 1;
        } else if visible == "endef" {
            define_depth = define_depth
                .checked_sub(1)
                .ok_or("unmatched Make endef while selecting declarations")?;
        } else if define_depth == 0 {
            declaration_source.push_str(text);
        }
        declaration_source.push('\n');
    }
    if define_depth != 0 {
        return Err("unterminated Make definition while selecting declarations".into());
    }
    let invocations = macro_invocations(&declaration_source);
    let mut selected_lines = Vec::new();
    for invocation in &invocations {
        if !is_concrete_build_invocation(&invocation.name) {
            continue;
        }
        let names = macro_argument_names(&invocation.args);
        if names.iter().filter(|name| name.as_str() == "mmake").count() > 1 {
            return Err(format!(
                "{}:{} has duplicate mmake arguments",
                relative_dir.display(),
                invocation.line + 1
            ));
        }
        if macro_arg(&invocation.args, "mmake").as_deref() != Some(mmake) {
            continue;
        }
        match states
            .get(invocation.line)
            .copied()
            .unwrap_or(ConditionalTruth::Unknown)
        {
            ConditionalTruth::True => selected_lines.push(invocation.line),
            ConditionalTruth::False => {}
            ConditionalTruth::Unknown => {
                return Err(format!(
                    "selected %{} mmake={mmake} is guarded by an unresolved Make conditional",
                    invocation.name
                ));
            }
        }
    }
    if selected_lines.len() != 1 {
        return Err(format!(
            "expected exactly one selected concrete MetaMake declaration for mmake={mmake}, found {}",
            selected_lines.len()
        ));
    }
    let declaration_line = selected_lines[0];
    let selected_invocation = invocations
        .iter()
        .find(|invocation| invocation.line == declaration_line)
        .ok_or("selected Mesa declaration disappeared")?;
    verify_default_recipe_arguments(&selected_invocation.args)?;

    let gca_line = if mmake == VC4_MMAKE {
        Some(find_unique_gca_assignment(&joined)?)
    } else {
        None
    };
    if let Some(line) = gca_line {
        if states.get(line).copied() != Some(ConditionalTruth::True) {
            return Err(
                "VC4 immediate GCA assignment is not unconditionally selected for this target"
                    .into(),
            );
        }
    }
    let declaration = MakeExprContext::new(&scope, dirs, declaration_line, &root, &relative_dir)
        .with_lookup(&lookup);
    let gca_context = gca_line.map(|line| {
        MakeExprContext::new(&scope, dirs, line + 1, &root, &relative_dir).with_lookup(&lookup)
    });
    let mut result = evaluate_ordered_mesa_recipe_options(&declaration, gca_context.as_ref())
        .map_err(|error| format!("cannot resolve configured Mesa compiler options: {error}"))?;
    let mut target_rule_seen = false;
    let mut target_define_depth = 0usize;
    for (line, text) in joined.lines().enumerate() {
        if text.starts_with('\t') {
            continue;
        }
        let visible = strip_make_comment(text).trim();
        if defined_macro_name(text).is_some() {
            target_define_depth += 1;
            continue;
        }
        if visible == "endef" && target_define_depth != 0 {
            target_define_depth -= 1;
            continue;
        }
        if target_define_depth != 0 {
            continue;
        }
        let Some(rule) = super::configured_target_options::parse(visible) else {
            continue;
        };
        if !invocations.iter().any(|invocation| {
            invocation.name == "build_linklib"
                && macro_arg(&invocation.args, "mmake").as_deref() == Some(rule.owner.as_str())
        }) {
            return Err("Mesa target-specific option rule has no local link-library owner".into());
        }
        match states
            .get(line)
            .copied()
            .unwrap_or(ConditionalTruth::Unknown)
        {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                return Err("Mesa target-specific options have unresolved selection".into())
            }
            ConditionalTruth::True => {}
        }
        if rule.owner != mmake {
            continue;
        }
        if line >= declaration_line {
            return Err(
                "Mesa target-specific options after their declaration are not modeled".into(),
            );
        }
        if target_rule_seen {
            return Err("multiple active Mesa target-specific option rules are not modeled".into());
        }
        target_rule_seen = true;
        verify_target_option_templates(&root)?;
        let invocation = invocations
            .iter()
            .find(|invocation| invocation.line == declaration_line)
            .ok_or("Mesa declaration disappeared during target-option projection")?;
        let rule_context =
            MakeExprContext::new(&scope, dirs, line, &root, &relative_dir).with_lookup(&lookup);
        let extra = super::configured_target_options::project_complete_c(
            &rule,
            &rule_context,
            &declaration,
            &invocation.args,
        )?;
        // The reviewed C multi-object recipe appends MC_EXTRA_CPPFLAGS after
        // base CPPFLAGS. C++ and the handwritten VC4 bridge are distinct lanes.
        result.c.extend(extra);
    }
    materialize_build_options(&mut result, &build)?;
    for inventory in &inventories {
        inventory.verify()?;
    }
    Ok(ProjectedScope {
        options: result,
        inventories,
        context_input: super::configured_source_context::ContextInput {
            source_program: joined,
            source_locations,
            declaration_line,
            invocation: format!(
                "%{} {}\n",
                selected_invocation.name, selected_invocation.args
            ),
            selected,
        },
    })
}

fn verify_default_recipe_arguments(arguments: &str) -> Result<(), String> {
    let names = macro_argument_names(arguments);
    for (name, default) in [
        ("cflags", ""),
        ("cxxflags", ""),
        ("dflags", ""),
        ("dxxflags", ""),
        ("includedir", ""),
        ("cppflags", "$(CPPFLAGS)"),
        ("lto", "$(TARGET_LTO)"),
        ("compiler", "target"),
    ] {
        let count = names.iter().filter(|key| key.as_str() == name).count();
        if count > 1 || (count == 1 && macro_arg(arguments, name).as_deref() != Some(default)) {
            return Err(format!(
                "Mesa declaration {name}= override is not part of the measured default recipe"
            ));
        }
    }
    Ok(())
}

/// Directory expansion intentionally uses the engine's SYS/build layout, not
/// the private preflight owner's legacy bin/<platform>-<cpu>/AROS tree. Resolve
/// that one trusted deferred root only after source assignment-time evaluation.
/// This is layout relocation, not a claim of raw legacy path-byte equality.
fn materialize_build_options(
    options: &mut OrderedMesaCompilerOptions,
    build: &Path,
) -> Result<(), String> {
    let build = path_text(build, "build directory")?;
    for lane in [&mut options.c, &mut options.cxx, &mut options.vc4_bridge] {
        for option in lane {
            let concrete = option.replace("${AROS_BUILD_DIR}", &build);
            if concrete.contains('$') {
                return Err(
                    "configured Mesa option retains an unbound directory expression".into(),
                );
            }
            *option = concrete;
        }
    }
    // The GCA input is a separately evaluated assignment-time literal. Do not
    // rewrite it, or infer ABI/hash equivalence from directory relocation.
    Ok(())
}

// Both target-option projection and the complete source context rely on these
// same recipes. Keep their reviewed identities in one place; neither caller
// may silently select a different version of a shared recipe.
const TARGET_OPTION_TEMPLATES: &[(&str, &str)] = &[
    (
        "compile_q",
        "2d19e70a44899e1419ef82fb634709f1b597435397c2aba3a9b498ef7f460f62",
    ),
    (
        "rule_compile_multi",
        "8275dfe5d549d82f434c07caa7a32d64aeaabb97c5a5c464e31b8823c05636ec",
    ),
    (
        "build_linklib",
        "ad1352f1a98d5acdd008377d645916198326c662fdd622ff50da1f41faa38474",
    ),
];

fn verify_target_option_templates(root: &Path) -> Result<(), String> {
    verify_recipe_templates(root, &[])
}

/// Verify the shared target-option recipes plus the caller's additional
/// source-context recipes. Both entry points retain their own boundary check.
pub(super) fn verify_recipe_templates(
    root: &Path,
    additional_templates: &[(&str, &str)],
) -> Result<(), String> {
    let template_path = root.join("config/make.tmpl");
    let canonical_template = template_path
        .canonicalize()
        .map_err(|error| format!("cannot resolve reviewed target-option templates: {error}"))?;
    let templates = || TARGET_OPTION_TEMPLATES.iter().chain(additional_templates);
    let names = templates().map(|(name, _)| *name).collect::<Vec<_>>();
    let origins = crate::genmf_projection::template_definition_origins(&template_path, &names)
        .map_err(|error| format!("cannot load effective target-option templates: {error}"))?;
    let bytes = read_regular_bounded(&template_path, MAX_TEMPLATE_BYTES, "make.tmpl")?;
    let text = aros_common::decode_source_bytes(bytes);
    for &(name, expected) in templates() {
        let start = format!("%define {name} ");
        let mut matches = text.match_indices(&start);
        let (offset, _) = matches
            .next()
            .ok_or_else(|| format!("missing reviewed {name} template"))?;
        if matches.next().is_some() {
            return Err(format!("duplicate {name} template"));
        }
        let origin = &origins[name];
        let expected_line = text[..offset].bytes().filter(|byte| *byte == b'\n').count() + 1;
        if origin.path != canonical_template || origin.line != expected_line {
            return Err(format!("reviewed {name} template is hidden or overridden"));
        }
        let body = &text[offset..];
        let end = body
            .find("\n%end\n")
            .ok_or("unterminated target-option template")?
            + "\n%end\n".len();
        if aros_common::sha256_bytes(&body.as_bytes()[..end]).to_string() != expected {
            return Err(format!("reviewed {name} target-option template changed"));
        }
    }
    Ok(())
}

/// Resolve only the reviewed literal-file guard, preserving positional scope
/// and line coordinates. Other filesystem conditions remain unresolved.
fn project_inventory_guards(
    joined: &str,
    selected: &TargetContext,
    dirs: &DirVars,
    roots: (&Path, &Path, &Path),
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(String, Vec<InventoryObservation>), String> {
    let (root, ports, relative_dir) = roots;
    let mut lines: Vec<String> = joined.lines().map(str::to_owned).collect();
    let mut observations = Vec::new();
    let mut define_depth = 0usize;
    for line_no in 0..lines.len() {
        let line = &lines[line_no];
        if line.starts_with('\t') {
            continue;
        }
        let visible = strip_make_comment(line).trim();
        if defined_macro_name(line).is_some() {
            define_depth += 1;
            continue;
        }
        if visible == "endef" && define_depth > 0 {
            define_depth -= 1;
            continue;
        }
        if define_depth > 0 {
            continue;
        }
        let (inventory_path, expect_present) = match visible {
            INVENTORY_GUARD => (INVENTORY_PATH, true),
            COMPONENT_INVENTORY_PRESENT => (COMPONENT_INVENTORY_PATH, true),
            COMPONENT_INVENTORY_ABSENT => (COMPONENT_INVENTORY_PATH, false),
            _ => continue,
        };
        let current = lines.join("\n");
        let (scope, states) = collect_vars_impl(&current, Some(selected));
        match states
            .get(line_no)
            .copied()
            .unwrap_or(ConditionalTruth::Unknown)
        {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                return Err("Mesa source inventory guard has unresolved parent scope".into())
            }
            ConditionalTruth::True => {}
        }
        let context = MakeExprContext::new(&scope, dirs, line_no, root, relative_dir)
            .with_lookup(lookup)
            .without_filesystem();
        let resolved = evaluate_make_expr(inventory_path, &context)
            .map_err(|error| format!("cannot resolve Mesa source inventory guard: {error}"))?;
        if resolved.split_whitespace().count() != 1 || resolved.contains(['*', '?', '[', ']']) {
            return Err("Mesa source inventory guard requires one literal path".into());
        }
        let observation = InventoryObservation::capture(Path::new(&resolved), ports)?;
        lines[line_no] = if observation.present() == expect_present {
            "ifneq (1,)"
        } else {
            "ifneq (,)"
        }
        .into();
        observations.push(observation);
    }
    let mut projected = lines.join("\n");
    if joined.ends_with('\n') {
        projected.push('\n');
    }
    Ok((projected, observations))
}

fn apply_configured_llvm(
    selected: &mut TargetContext,
    configuration: &ValidatedMesaConfiguration,
) -> Result<(), String> {
    let llvm = configuration.llvm_configuration();
    for (name, state) in [
        ("OPT_LLVM", &llvm.opt_llvm),
        ("TARGET_LLVM_VER", &llvm.target_llvm_ver),
    ] {
        if let Some(value) = selected.make_variables.get(name) {
            if !matches!(state, MesaMakeVersionValue::Defined(expected) if expected == value) {
                return Err(format!(
                    "selected {name} conflicts with measured Mesa LLVM configuration"
                ));
            }
        }
    }
    if let Some(requested) = selected
        .target_llvm_ver
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        if !matches!(&llvm.target_llvm_ver, MesaMakeVersionValue::Defined(value) if value == requested)
        {
            return Err(
                "selected TARGET_LLVM_VER conflicts with measured Mesa LLVM configuration".into(),
            );
        }
    }
    // Only this source-evaluation clone changes. Do not enable the frontend's
    // optional target-LLVM provider or rewrite its selection globally.
    selected.target_llvm_ver = match &llvm.target_llvm_ver {
        MesaMakeVersionValue::Undefined => None,
        MesaMakeVersionValue::Defined(value) => Some(value.clone()),
    };
    Ok(())
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{label} must be an absolute path"));
    }
    let path =
        fs::canonicalize(path).map_err(|error| format!("cannot resolve {label}: {error}"))?;
    if !path.is_dir() {
        return Err(format!("{label} is not a directory"));
    }
    Ok(path)
}

fn validate_relative_directory(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err("Mesa declaration directory must be a normalized relative path".into());
    }
    Ok(path.to_path_buf())
}

fn path_text(path: &Path, label: &str) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{label} is not UTF-8"))
}

fn merge_scalar(
    variables: &mut BTreeMap<String, String>,
    name: &str,
    value: String,
) -> Result<(), String> {
    if let Some(existing) = variables.get(name) {
        if existing != &value {
            return Err(format!(
                "configured Mesa variable {name} conflicts with the selected target context"
            ));
        }
    } else {
        variables.insert(name.to_owned(), value);
    }
    Ok(())
}

pub(super) fn read_regular_bounded(
    path: &Path,
    limit: u64,
    label: &str,
) -> Result<Vec<u8>, String> {
    aros_common::measure_regular_file_bounded(path, limit)
        .map_err(|error| format!("cannot read {label}: {error}"))?
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| format!("{label} is missing"))
}

fn extract_recursive_definitions(root: &Path) -> Result<String, String> {
    let mut contents = BTreeMap::new();
    for path in ["config/target.cfg.in", "config/make.cfg.in"] {
        let bytes = read_regular_bounded(&root.join(path), MAX_TEMPLATE_BYTES, path)?;
        contents.insert(path, aros_common::decode_source_bytes(bytes));
    }

    let mut output = String::new();
    for (path, name, expected_rhs) in RECURSIVE_DEFINITIONS {
        let text = contents
            .get(path)
            .expect("all reviewed templates were loaded");
        let joined = join_continuations(text);
        let mut found = Vec::new();
        let mut conditional_depth = 0usize;
        let mut define_depth = 0usize;
        for line in joined.lines() {
            if line.starts_with('\t') {
                continue;
            }
            let visible = strip_make_comment(line).trim();
            if define_depth > 0 {
                if defined_macro_name(line).is_some() {
                    define_depth += 1;
                } else if visible == "endef" {
                    define_depth -= 1;
                }
                continue;
            }
            if let Some(defined) = defined_macro_name(line) {
                if defined == *name {
                    return Err(format!("{path} has opaque definition for {name}"));
                }
                define_depth = 1;
                continue;
            }
            let directive = visible.split_whitespace().next().unwrap_or_default();
            match directive {
                "endef" => return Err(format!("{path} has an unmatched endef near {name}")),
                "ifeq" | "ifneq" | "ifdef" | "ifndef" => conditional_depth += 1,
                "else" if conditional_depth == 0 => {
                    return Err(format!("{path} has an unmatched else near {name}"));
                }
                "endif" if conditional_depth == 0 => {
                    return Err(format!("{path} has an unmatched endif near {name}"));
                }
                "endif" => conditional_depth -= 1,
                _ => {}
            }
            if let Some(assignment) = parse_assignment(line) {
                if assignment.name == *name {
                    if !visible.starts_with(name) {
                        return Err(format!(
                            "{path} {name} has unsupported assignment modifiers"
                        ));
                    }
                    if conditional_depth != 0 {
                        return Err(format!(
                            "{path} {name} definition is conditional and ambiguous"
                        ));
                    }
                    found.push(assignment);
                }
            }
        }
        if conditional_depth != 0 || define_depth != 0 {
            return Err(format!(
                "{path} has an unclosed conditional or define near {name}"
            ));
        }
        if found.len() != 1 {
            return Err(format!(
                "{path} must contain exactly one recursive {name} definition; found {}",
                found.len()
            ));
        }
        let assignment = &found[0];
        if assignment.operator != "=" {
            return Err(format!("{path} {name} definition is not recursive"));
        }
        if !assignment.value.is_ascii() {
            return Err(format!("{path} {name} formula is not ASCII"));
        }
        if normalize_whitespace(assignment.value) != normalize_whitespace(expected_rhs) {
            return Err(format!(
                "{path} {name} formula differs from the reviewed recursive definition"
            ));
        }
        output.push_str(name);
        output.push_str(" = ");
        output.push_str(expected_rhs);
        output.push('\n');
    }
    Ok(output)
}

fn verify_aros_configuration_wrapper(root: &Path) -> Result<(), String> {
    let path = root.join("config/aros.cfg");
    let bytes = read_regular_bounded(&path, 64 * 1024, "config/aros.cfg")?;
    let text = aros_common::decode_source_bytes(bytes);
    let active = text
        .lines()
        .map(|line| strip_make_comment(line).trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if active
        != [
            "include $(TOP)/config/make.cfg",
            "-include $(GENDIR)/config/compiler.cfg",
        ]
    {
        return Err("config/aros.cfg no longer matches the reviewed global Make wrapper".into());
    }
    Ok(())
}

fn remove_aros_wrapper_include(text: &str) -> Result<String, String> {
    let mut result = String::with_capacity(text.len());
    let mut count = 0usize;
    let mut conditional_depth = 0usize;
    let mut define_depth = 0usize;
    let mut prefix_statement = false;
    for chunk in text.split_inclusive('\n') {
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
        let visible = strip_make_comment(line).trim();
        if visible == AROS_CFG_INCLUDE {
            if line.starts_with('\t') || conditional_depth != 0 || define_depth != 0 {
                return Err("config/aros.cfg must be an unconditional top-level import".into());
            }
            if prefix_statement {
                return Err("config/aros.cfg must precede owner Make statements until positional configuration import is modeled".into());
            }
            count += 1;
            result.push_str("# Bound configured AROS make wrapper\n");
        } else {
            if !visible.is_empty() && count == 0 {
                prefix_statement = true;
            }
            result.push_str(chunk);
        }
        if line.starts_with('\t') {
            continue;
        }
        if defined_macro_name(line).is_some() {
            define_depth += 1;
        } else if visible == "endef" {
            define_depth = define_depth.checked_sub(1).ok_or("unmatched Make endef")?;
        } else if define_depth == 0 {
            match visible.split_whitespace().next().unwrap_or_default() {
                "ifeq" | "ifneq" | "ifdef" | "ifndef" => conditional_depth += 1,
                "endif" => {
                    conditional_depth = conditional_depth
                        .checked_sub(1)
                        .ok_or("unmatched Make endif")?;
                }
                _ => {}
            }
        }
    }
    if count != 1 || conditional_depth != 0 || define_depth != 0 {
        return Err(format!(
            "Mesa mmakefile must include config/aros.cfg exactly once; found {count}"
        ));
    }
    Ok(result)
}

fn reject_unmodeled_evaluation(
    text: &str,
    states: &[ConditionalTruth],
    generator_lines: &std::collections::BTreeSet<usize>,
) -> Result<(), String> {
    for (line_no, line) in text.lines().enumerate() {
        // Make expands recipes before the shell sees its '#' comments.
        let visible = if line.starts_with('\t') {
            line.trim()
        } else {
            strip_make_comment(line).trim()
        };
        let directive = visible.split_whitespace().next();
        let target_option_rule = super::configured_target_options::parse(visible).is_some()
            || generator_lines.contains(&line_no);
        let dynamic_assignment = !target_option_rule
            && visible.split_once('=').is_some_and(|(left, _)| {
                left.contains('$')
                    && !matches!(
                        directive,
                        Some("ifeq" | "ifneq" | "ifdef" | "ifndef" | "else")
                    )
            });
        let dynamic_undefine = directive == Some("undefine") && visible.contains('$');
        if !line.starts_with('\t') && (dynamic_assignment || dynamic_undefine) {
            return Err(format!(
                "configured Mesa has an unmodeled top-level Make evaluation: computed Make assignment name at line {}: {}",
                line_no + 1, visible.chars().take(180).collect::<String>()
            ));
        }
        if !line.starts_with('\t') && !target_option_rule && has_target_specific_assignment(visible)
        {
            return Err(format!(
                "unmodeled Mesa target-specific assignment at line {}",
                line_no + 1
            ));
        }
        // These roots belong to the selected execution context. The shared
        // evaluator resolves them before local scope, so a source write must
        // refuse rather than silently disappear (including an opaque define).
        let root_write = defined_macro_name(line)
            .or_else(|| parse_assignment(visible).map(|assignment| assignment.name.to_owned()))
            .or_else(|| {
                let (directive, tail) = visible.split_once(char::is_whitespace)?;
                (directive == "undefine").then(|| tail.trim().to_owned())
            });
        if !line.starts_with('\t')
            && root_write.is_some_and(|name| {
                matches!(
                    name.as_str(),
                    "TOP" | "SRCDIR" | "CURDIR" | "GENDIR" | "PORTSDIR"
                )
            })
        {
            return Err(format!(
                "Mesa source writes an immutable execution root at line {}",
                line_no + 1
            ));
        }
        // The shared collector collapses escaped dollars in simple assignments.
        // Refuse them in source data before that loses provenance: a literal
        // $${AROS_BUILD_DIR} must not become a trusted deferred engine root.
        // Recipe dollars are not compiler-option assignment data.
        if !line.starts_with('\t') && visible.contains("$$") {
            return Err(format!(
                "escaped literal dollar in Mesa source data at line {}",
                line_no + 1
            ));
        }
        let literal_error =
            [("$(error ", ')'), ("${error ", '}')]
                .into_iter()
                .any(|(prefix, close)| {
                    visible
                        .strip_prefix(prefix)
                        .and_then(|body| body.strip_suffix(close))
                        .is_some_and(|body| !body.contains(['$', '(', ')', '{', '}']))
                });
        // GNU Make does not execute assertions in a proven inactive branch.
        // Do not generalize this to opaque functions or an unknown condition.
        if !line.starts_with('\t')
            && literal_error
            && states.get(line_no) == Some(&ConditionalTruth::False)
        {
            continue;
        }
        let dynamic = ["$(", "${"].into_iter().any(|opening| {
            visible.match_indices(opening).any(|(index, _)| {
                let expression = visible[index + opening.len()..].trim_start();
                ["eval", "file", "guile", "shell", "error"]
                    .into_iter()
                    .any(|function| {
                        expression
                            .strip_prefix(function)
                            .is_some_and(|tail| tail.starts_with(char::is_whitespace))
                    })
            })
        });
        let shell_assignment =
            parse_assignment(visible).is_some_and(|assignment| assignment.operator == "!=");
        // The shared collector does not model assignment modifiers or export
        // directives. Never silently lose a source write or environment effect.
        let modified_statement = !line.starts_with('\t')
            && (visible.starts_with(".RECIPEPREFIX")
                || defined_macro_name(line).is_some_and(|name| {
                    !name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
                })
                || matches!(
                    visible.split_whitespace().next(),
                    Some("override" | "export" | "private" | "unexport")
                ));
        if dynamic
            || shell_assignment
            || modified_statement
            || (!line.starts_with('\t')
                && (visible.starts_with("$(") || visible.starts_with("${"))
                && !visible.contains(':'))
        {
            return Err(format!("configured Mesa has an unmodeled top-level Make evaluation at expanded line {}: {}", line_no + 1, visible.chars().take(180).collect::<String>()));
        }
    }
    Ok(())
}

fn has_target_specific_assignment(line: &str) -> bool {
    let mut depth = 0usize;
    let mut target_colon = false;
    let bytes = line.as_bytes();
    for (index, character) in line.char_indices() {
        match character {
            '(' | '{' => depth += 1,
            ')' | '}' => depth = depth.saturating_sub(1),
            ':' if depth == 0 && bytes.get(index + 1) != Some(&b'=') => target_colon = true,
            '=' if depth == 0 => return target_colon,
            _ => {}
        }
    }
    false
}

#[derive(Clone, Copy)]
struct Assignment<'a> {
    name: &'a str,
    operator: &'a str,
    value: &'a str,
}

fn parse_assignment(line: &str) -> Option<Assignment<'_>> {
    let line = strip_make_comment(line).trim_start();
    let mut rest = line;
    for _ in 0..3 {
        let Some((modifier, tail)) = rest.split_once(char::is_whitespace) else {
            break;
        };
        if !matches!(modifier, "override" | "export" | "private") {
            break;
        }
        rest = tail.trim_start();
    }
    let name_end =
        rest.find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))?;
    let name = &rest[..name_end];
    if name.is_empty() {
        return None;
    }
    let rest = rest[name_end..].trim_start();
    for operator in [":::=", "::=", ":=", "+=", "?=", "!=", "="] {
        if let Some(value) = rest.strip_prefix(operator) {
            return Some(Assignment {
                name,
                operator,
                value: value.trim(),
            });
        }
    }
    None
}

pub(super) fn defined_macro_name(line: &str) -> Option<String> {
    let line = strip_make_comment(line).trim_start();
    let mut rest = line;
    while let Some((modifier, tail)) = rest.split_once(char::is_whitespace) {
        if !matches!(modifier, "override" | "export" | "private") {
            break;
        }
        rest = tail.trim_start();
    }
    let (directive, tail) = rest.split_once(char::is_whitespace)?;
    if directive != "define" {
        return None;
    }
    let rest = tail.trim_start();
    let name = rest.split_whitespace().next()?;
    (!name.is_empty()).then(|| name.to_owned())
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn find_unique_gca_assignment(joined: &str) -> Result<usize, String> {
    let mut assignments = Vec::new();
    let mut exact = Vec::new();
    for (line_no, line) in joined.lines().enumerate() {
        if parse_assignment(line).is_some_and(|assignment| assignment.name == "GCA_ABI_FLAGS") {
            assignments.push(line_no);
        }
        if strip_make_comment(line).trim() == GCA_ASSIGNMENT {
            if line.starts_with('\t') {
                return Err("VC4 GCA assignment appears inside a recipe".into());
            }
            exact.push(line_no);
        }
    }
    if assignments.len() != 1 || exact.len() != 1 || assignments != exact {
        return Err(
            "VC4 source must contain exactly one reviewed immediate GCA_ABI_FLAGS assignment"
                .into(),
        );
    }
    Ok(exact[0])
}

fn verify_remaining_includes<'a>(
    joined: &str,
    states: &[ConditionalTruth],
    scope: &'a crate::make_vars::VarScope,
    dirs: &'a DirVars,
    source_and_ports: (&'a Path, &'a Path),
    relative_dir: &'a Path,
    lookup: &'a (dyn Fn(&str) -> Option<String> + 'a),
) -> Result<(), String> {
    let (root, ports) = source_and_ports;
    for (line_no, line) in joined.lines().enumerate() {
        let Some(raw_path) = include_path(line) else {
            continue;
        };
        match states
            .get(line_no)
            .copied()
            .unwrap_or(ConditionalTruth::Unknown)
        {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                return Err(format!(
                    "Make include at line {} is guarded by an unresolved conditional",
                    line_no + 1
                ));
            }
            ConditionalTruth::True => {}
        }
        if !matches!(raw_path, INVENTORY_PATH | COMPONENT_INVENTORY_PATH) {
            return Err(format!(
                "active Make include at line {} remains unbound: {raw_path}",
                line_no + 1
            ));
        }
        let context =
            MakeExprContext::new(scope, dirs, line_no, root, relative_dir).with_lookup(lookup);
        let resolved = evaluate_make_expr(raw_path, &context).map_err(|error| {
            format!("cannot resolve active Mesa source inventory include: {error}")
        })?;
        let mut paths = resolved.split_whitespace();
        let Some(path) = paths.next() else {
            return Err("active Mesa source inventory include resolved to an empty path".into());
        };
        if paths.next().is_some() {
            return Err("active Mesa source inventory include resolved to multiple paths".into());
        }
        verify_source_inventory(Path::new(path), ports)?;
    }
    Ok(())
}

fn include_path(line: &str) -> Option<&str> {
    if line.starts_with('\t') {
        return None;
    }
    let line = strip_make_comment(line).trim_start();
    for directive in ["-include", "sinclude", "-sinclude", "include"] {
        let Some(tail) = line.strip_prefix(directive) else {
            continue;
        };
        if tail.is_empty() || tail.chars().next().is_some_and(char::is_whitespace) {
            let value = tail.trim();
            return (!value.is_empty()).then_some(value);
        }
    }
    None
}

fn verify_source_inventory(path: &Path, ports: &Path) -> Result<(), String> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("active Mesa source inventory is unavailable: {error}"))?;
    if !canonical.starts_with(ports)
        || canonical.file_name().and_then(|name| name.to_str()) != Some("Makefile.sources")
    {
        return Err(
            "active Mesa source inventory leaves PORTSDIR or has an unexpected name".into(),
        );
    }
    let bytes = read_regular_bounded(&canonical, MAX_INVENTORY_BYTES, "Mesa Makefile.sources")?;
    let text = aros_common::decode_source_bytes(bytes);
    let joined = join_continuations(&text);
    for line in joined.lines() {
        let visible = strip_make_comment(line).trim();
        if visible.is_empty() {
            continue;
        }
        let Some(assignment) = parse_assignment(visible) else {
            return Err("active Mesa Makefile.sources is not a plain source inventory".into());
        };
        if assignment.operator == "!=" {
            return Err("active Mesa Makefile.sources has a shell assignment".into());
        }
        if !(assignment.name.ends_with("_SOURCES") || assignment.name.ends_with("_FILES")) {
            return Err(format!(
                "active Mesa Makefile.sources assigns non-inventory variable {}",
                assignment.name
            ));
        }
        if !assignment.value.is_ascii() || !inventory_references_are_plain(assignment.value) {
            return Err(format!(
                "active Mesa Makefile.sources has a dynamic or non-ASCII {} value",
                assignment.name
            ));
        }
    }
    Ok(())
}

fn inventory_references_are_plain(value: &str) -> bool {
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        let tail = &rest[at + 1..];
        let (close, body) = match tail.as_bytes().first() {
            Some(b'(') => (')', &tail[1..]),
            Some(b'{') => ('}', &tail[1..]),
            _ => return false,
        };
        let Some(end) = body.find(close) else {
            return false;
        };
        let name = &body[..end];
        if !(name.ends_with("_SOURCES")
            || name.ends_with("_FILES")
            || matches!(name, "GALLIUM_PATH" | "CUR_GALLIUMDIR" | "SRCDIR"))
        {
            return false;
        }
        rest = &body[end + 1..];
    }
    true
}

#[cfg(test)]
#[path = "configured_scope_tests.rs"]
mod tests;
