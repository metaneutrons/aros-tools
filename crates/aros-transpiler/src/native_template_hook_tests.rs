use super::{NativeMetaSemanticsSelection, NativeOwnerProjection};
use crate::ast::MetaTargetRule;
use crate::genmf_projection::{expand_bytes, Limits as GenmfLimits};
use crate::metamake_owner_graph::{Limits as MetaMakeLimits, MetaMakeOwnerGraph};
use crate::{DependencyGraph, TargetContext};
use aros_common::native_build_contract::{NativeMetaAbsence, NativeOptionalMetaDependency};
use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

const OWNER: &str = "fixture/mmakefile";
const RECIPE: &str = "fixture/mmakefile.src";
const TEMPLATE: &str = "config/make.tmpl";

const ARCH_FLAG_TEMPLATE: &str = concat!(
    "%define set_archincludes mainmmake=/A maindir=/A modname=/A pri=/A arch=/A includes= genincdir=yes\n",
    "ifeq (\"%(genincdir)\",\"yes\")\n",
    "$(GENDIR)/%(maindir)/%(modname)/include:\n\t%mkdir_q dir=\"$@\"\nendif\n\n",
    "#MM\n%(mainmmake)-%(arch)-set-archincludes: | $(GENDIR)/%(maindir)/%(modname)/include\n",
    "\t$(Q)$(ECHO)  \"%(includes) \" > $(GENDIR)/%(maindir)/%(modname)/include/.%(modname).includeflag.%(pri).%(arch)\n\n%end\n",
    "%define get_archincludes includeflag=USER_INCLUDES maindir=/A modname=/A\n",
    "%(includeflag)_INCFFILES:=$(call WILDCARD, $(GENDIR)/%(maindir)/%(modname)/include/.%(modname).includeflag.*)\n",
    "ifneq ($(%(includeflag)_INCFFILES),)\n",
    "%(includeflag)+=$(shell cat $(%(includeflag)_INCFFILES))\nendif\n%end\n",
    "%define mkdir_q dir=/A\n\t@mkdir -p %(dir)\n%end\n",
);

#[test]
fn native_header_flag_catalog_seals_source_and_both_template_roles() {
    let source = concat!(
        "%set_archincludes mainmmake=demo maindir=rom/demo modname=demo pri=10 arch=esp32p4-riscv includes=\"-DFIRST=1 -I$(SRCDIR)/$(CURDIR)\"\n",
        "%get_archincludes maindir=rom/demo modname=demo includeflag=FLAGS\n",
    );
    let mut fixture = fixture_from_text(&[], source, ARCH_FLAG_TEMPLATE);
    fixture.projection.architecture_context = fixture.context.clone();
    let derived = fixture
        .projection
        .native_header_context(fixture.root.path())
        .unwrap();
    assert!(
        derived.native_arch_include_errors.is_empty(),
        "{:?}",
        derived.native_arch_include_errors
    );
    assert_eq!(derived.native_arch_include_effects.len(), 1);
    let crate::arch_endpoint_effects::ArchEndpointEffectData::SetArchIncludes { arguments, .. } =
        &derived.native_arch_include_effects[0].data
    else {
        panic!("flag producer")
    };
    assert_eq!(arguments, &["-DFIRST=1", "-I${AROS_SOURCE_DIR}/fixture"]);

    fs::write(
        fixture.root.path().join(RECIPE),
        source.replace("FIRST=1", "FIRST=2"),
    )
    .unwrap();
    assert!(fixture
        .projection
        .native_header_context(fixture.root.path())
        .is_err());
    fs::write(fixture.root.path().join(RECIPE), source).unwrap();
    let altered = ARCH_FLAG_TEMPLATE.replace("%(includeflag)+=", "%(includeflag):=");
    fs::write(fixture.root.path().join(TEMPLATE), &altered).unwrap();
    fixture.projection.snapshots.insert(
        TEMPLATE.into(),
        aros_common::sha256_bytes(altered.as_bytes()).to_string(),
    );
    let refused = fixture
        .projection
        .native_header_context(fixture.root.path())
        .unwrap();
    assert!(!refused.native_arch_include_errors.is_empty());
}

// Exact `gen_archspecificrules` declaration from config/make.tmpl. Its source
// definition fingerprint is checked by the production hook verifier.
const GEN_ARCHSPECIFICRULES: &str = concat!(
    "%define gen_archspecificrules mainmmake=/A target= subtarget=\n",
    "#MM- %(mainmmake)%(target)%(subtarget) : \\\n",
    "#MM             %(mainmmake)-$(CPU)%(target)%(subtarget)\n",
    "\n",
    "#MM- %(mainmmake)-$(ARCH)-$(CPU)%(target)%(subtarget) : \\\n",
    "#MM             %(mainmmake)-$(ARCH)-$(CPU)-$(AROS_TARGET_VARIANT)%(target)%(subtarget)\n",
    "\n",
    "#MM- %(mainmmake)-$(ARCH)-$(AROS_TARGET_VARIANT)%(target)%(subtarget) : \\\n",
    "#MM             %(mainmmake)-$(ARCH)-$(CPU)%(target)%(subtarget)\n",
    "\n",
    "#MM- %(mainmmake)-$(ARCH)%(target)%(subtarget) : \\\n",
    "#MM             %(mainmmake)-$(ARCH)-$(AROS_TARGET_VARIANT)%(target)%(subtarget)\n",
    "\n",
    "#MM- %(mainmmake)-$(FAMILY)%(target)%(subtarget) : \\\n",
    "#MM             %(mainmmake)-$(ARCH)%(target)%(subtarget)\n",
    "\n",
    "#MM- %(mainmmake)-$(CPU)%(target)%(subtarget) : \\\n",
    "#MM             %(mainmmake)-$(FAMILY)%(target)%(subtarget)\n",
    "%end\n",
);

const BUILD_MODULE: &str = concat!(
    "%define build_module mainmmake=/A\n",
    "%gen_archspecificrules mainmmake=%(mainmmake)\n",
    "%gen_archspecificrules mainmmake=%(mainmmake) target=-set-archincludes\n",
    "%gen_archspecificrules mainmmake=%(mainmmake) target=-linklib\n",
    "%end\n",
);

const FAMILIES: [(&str, &str); 3] = [
    ("module", ""),
    ("set-archincludes", "-set-archincludes"),
    ("linklib", "-linklib"),
];

struct Fixture {
    root: tempfile::TempDir,
    projection: NativeOwnerProjection,
    context: TargetContext,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HookEdge {
    family: &'static str,
    raw_target: String,
    raw_dependency: String,
    canonical_dependency: String,
    target: String,
    dependency: String,
}

fn hook_edge(mainmmake: &str, family: &'static str, suffix: &str) -> HookEdge {
    HookEdge {
        family,
        raw_target: format!("{mainmmake}-$(ARCH)-$(CPU){suffix}"),
        raw_dependency: format!("{mainmmake}-$(ARCH)-$(CPU)-$(AROS_TARGET_VARIANT){suffix}"),
        canonical_dependency: format!(
            "{mainmmake}-${{AROS_TARGET_PLATFORM}}-${{AROS_TARGET_CPU}}-${{AROS_TARGET_VARIANT}}{suffix}"
        ),
        target: format!("{mainmmake}-esp32p4-riscv{suffix}"),
        dependency: format!("{mainmmake}-esp32p4-riscv-{suffix}"),
    }
}

fn all_hook_edges(mainmmake: &str) -> Vec<HookEdge> {
    FAMILIES
        .into_iter()
        .map(|(family, suffix)| hook_edge(mainmmake, family, suffix))
        .collect()
}

fn standard_template() -> String {
    format!("{GEN_ARCHSPECIFICRULES}{BUILD_MODULE}")
}

// Exact reviewed module definitions, copied from the source template. Keeping
// full blocks (rather than test-only fingerprints) exercises production proof.
const EMPTY_LIBRARY_TEMPLATE: &str = include_str!("../tests/fixtures/empty_library_lists.tmpl");

fn empty_library_fixture(extra: &str, args: &str) -> Fixture {
    let source = format!(
        "%build_module mmake=kernel-example modname=example modtype=library {args}\n{extra}"
    );
    fixture_from_text(&[], &source, EMPTY_LIBRARY_TEMPLATE)
}

type LibraryEdgeOrigins = BTreeMap<(String, String), BTreeSet<String>>;

fn empty_library_graph() -> (DependencyGraph, Vec<String>, LibraryEdgeOrigins) {
    let (mut graph, _) = request_graph(vec!["kernel-example".into()]);
    graph.add_meta_rule(MetaTargetRule {
        name: "kernel-example".into(),
        dependencies: vec!["linklibs-".into()],
    });
    (
        graph,
        vec!["requested".into()],
        BTreeMap::from([(
            ("kernel-example".into(), "linklibs-".into()),
            BTreeSet::from([RECIPE.into()]),
        )]),
    )
}

#[test]
fn verified_empty_library_list_removes_only_the_generated_edge() {
    for args in ["", "uselibs=", "uselibs=\"\"", "uselibs=\"   \""] {
        let fixture = empty_library_fixture("", args);
        let (mut graph, roots, origins) = empty_library_graph();
        let evidence = bind(&fixture, &mut graph, &roots, &[], &origins).unwrap();
        assert_eq!(evidence.empty_library_list_omissions.len(), 1, "{args}");
        assert!(!graph.meta_targets["kernel-example"].contains("linklibs-"));
        assert!(graph.meta_targets["kernel-example"].contains("core-linklibs"));
        assert!(!graph
            .native_metadata_endpoint_names(&fixture.context)
            .contains("linklibs-"));
    }
}

#[test]
fn nonempty_or_unresolved_library_lists_are_never_normalized() {
    for args in ["uselibs=missing", "uselibs=\"a b\""] {
        let fixture = empty_library_fixture("", args);
        let (mut graph, roots, origins) = empty_library_graph();
        let result = bind(&fixture, &mut graph, &roots, &[], &origins);
        if let Ok(evidence) = result {
            assert!(evidence.empty_library_list_omissions.is_empty());
            assert!(!graph
                .audit_native_dependency_graph(&roots, &fixture.context, &[])
                .missing_endpoints
                .is_empty());
        }
    }
}

#[test]
fn unresolved_library_list_is_rejected_before_native_normalization() {
    let root = tempfile::tempdir().unwrap();
    let source_root = root.path().canonicalize().unwrap();
    let template = source_root.join("make.tmpl");
    fs::write(&template, EMPTY_LIBRARY_TEMPLATE).unwrap();
    let source =
        "%build_module mmake=kernel-example modname=example modtype=library uselibs=$(UNKNOWN)\n";
    let expanded = expand_bytes(
        source.as_bytes(),
        &source_root.join("mmakefile.src"),
        &template,
        GenmfLimits::default(),
    )
    .unwrap();
    let files = BTreeMap::from([(OWNER.into(), expanded.text)]);
    let error = MetaMakeOwnerGraph::parse_expanded_files(
        &files,
        &BTreeMap::new(),
        MetaMakeLimits::default(),
    )
    .unwrap_err();
    assert!(error.contains("unbound project-global variable UNKNOWN"));
}

#[test]
fn handwritten_colliding_empty_library_claim_cannot_borrow_template_proof() {
    for extra in [
        "#MM- kernel-example : linklibs-\n",
        "#MM kernel-example : linklibs-\n",
    ] {
        let fixture = empty_library_fixture(extra, "");
        let (mut graph, roots, origins) = empty_library_graph();
        let evidence = bind(&fixture, &mut graph, &roots, &[], &origins).unwrap();
        assert!(evidence.empty_library_list_omissions.is_empty());
        assert!(graph.meta_targets["kernel-example"].contains("linklibs-"));
    }
}

#[test]
fn independent_empty_library_ingress_remains_required() {
    let fixture = empty_library_fixture("#MM- other : linklibs-\n", "");
    let (mut graph, roots, origins) = empty_library_graph();
    graph.add_meta_rule(MetaTargetRule {
        name: "requested".into(),
        dependencies: vec!["other".into()],
    });
    let evidence = bind(&fixture, &mut graph, &roots, &[], &origins).unwrap();
    assert_eq!(evidence.empty_library_list_omissions.len(), 1);
    assert!(graph.meta_targets["other"].contains("linklibs-"));
    assert!(missing(&graph, &fixture, &roots, &[]).contains("linklibs-"));
}

#[test]
fn declared_empty_library_endpoint_is_preserved() {
    let fixture = empty_library_fixture("#MM- linklibs-\n", "");
    let (mut graph, roots, origins) = empty_library_graph();
    let evidence = bind(&fixture, &mut graph, &roots, &[], &origins).unwrap();
    assert!(evidence.empty_library_list_omissions.is_empty());
    assert!(graph.meta_targets["kernel-example"].contains("linklibs-"));
}

#[test]
fn source_imported_empty_library_edge_retains_its_exact_origin() {
    let fixture = empty_library_fixture("", "");
    let (mut graph, roots) = request_graph(vec!["kernel-example".into()]);
    // The known real module has no raw-parser metadata dependency: that edge
    // is imported independently from the exact source owner projection.
    graph.add_meta_rule(MetaTargetRule {
        name: "kernel-example".into(),
        dependencies: vec![],
    });
    let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap();
    assert_eq!(evidence.empty_library_list_omissions.len(), 1);
    assert!(!graph.meta_targets["kernel-example"].contains("linklibs-"));
}

#[test]
fn equivalent_lexical_source_root_keeps_empty_library_provenance() {
    let mut fixture = empty_library_fixture("", "");
    let lexical_root = fixture.root.path().join(".");
    for origin in fixture.projection.expansion_origins.values_mut() {
        if let Some(invocation) = &mut origin.top_level_source_invocation {
            invocation.path = lexical_root.join(RECIPE);
        }
    }
    let (mut graph, roots, origins) = empty_library_graph();
    let evidence = fixture
        .projection
        .bind_native_meta_semantics(
            &lexical_root,
            &mut graph,
            NativeMetaSemanticsSelection {
                context: &fixture.context,
                roots: &roots,
                declarations: &[],
                diagnostics: &[],
                meta_edge_origins: &origins,
            },
            &mut BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(evidence.empty_library_list_omissions.len(), 1);
}

#[test]
fn changed_template_cannot_authorize_empty_library_normalization() {
    let source = "%build_module mmake=kernel-example modname=example modtype=library\n";
    let changed = EMPTY_LIBRARY_TEMPLATE.replace(
        "#MM %(mmake)-clean",
        "#MM %(mmake)-clean\n#MM- unexpected : missing",
    );
    let fixture = fixture_from_text(&[], source, &changed);
    let (mut graph, roots, origins) = empty_library_graph();
    assert!(bind(&fixture, &mut graph, &roots, &[], &origins)
        .unwrap_err()
        .contains("supported semantics"));
    assert!(graph.meta_targets["kernel-example"].contains("linklibs-"));
}

#[test]
fn extra_native_origin_vetoes_empty_library_normalization() {
    let fixture = empty_library_fixture("", "");
    let (mut graph, roots, mut origins) = empty_library_graph();
    origins
        .values_mut()
        .next()
        .unwrap()
        .insert("unrelated/mmakefile.src".into());
    let evidence = bind(&fixture, &mut graph, &roots, &[], &origins).unwrap();
    assert!(evidence.empty_library_list_omissions.is_empty());
    assert!(graph.meta_targets["kernel-example"].contains("linklibs-"));
}

#[test]
fn empty_library_list_on_a_real_kobj_owner_does_not_replace_its_producer() {
    let fixture = empty_library_fixture("", "");
    assert!(fixture
        .projection
        .graph
        .owner_files("kernel-example-kobj")
        .is_some());
    let (mut graph, roots) = request_graph(vec!["kernel-example-kobj".into()]);
    graph.add_meta_rule(MetaTargetRule {
        name: "kernel-example-kobj".into(),
        dependencies: vec!["linklibs-".into()],
    });
    let origins = BTreeMap::from([(
        ("kernel-example-kobj".into(), "linklibs-".into()),
        BTreeSet::from([RECIPE.into()]),
    )]);
    let evidence = bind(&fixture, &mut graph, &roots, &[], &origins).unwrap();
    assert_eq!(evidence.empty_library_list_omissions.len(), 2);
    assert!(!graph.meta_targets["kernel-example-kobj"].contains("linklibs-"));
    assert!(fixture
        .projection
        .graph
        .owner_files("kernel-example-kobj")
        .is_some());
}

#[test]
fn rejected_empty_library_endpoint_is_not_erased() {
    let fixture = empty_library_fixture("", "");
    let (mut graph, roots, origins) = empty_library_graph();
    let evidence = bind(
        &fixture,
        &mut graph,
        &roots,
        &[selector_diagnostic("linklibs-")],
        &origins,
    )
    .unwrap();
    assert!(evidence.empty_library_list_omissions.is_empty());
    assert!(graph.meta_targets["kernel-example"].contains("linklibs-"));
}

fn standard_fixture(families: &[&str], mainmmake: &str, extra_source: &str) -> Fixture {
    let source = format!("%build_module mainmmake={mainmmake}\n{extra_source}");
    fixture_from_text(families, &source, &standard_template())
}

fn direct_caller_fixture(
    families: &[&str],
    caller: &str,
    mainmmake: &str,
    suffix: &str,
) -> Fixture {
    let call = if suffix.is_empty() {
        "%gen_archspecificrules mainmmake=%(mainmmake)\n".to_owned()
    } else {
        format!("%gen_archspecificrules mainmmake=%(mainmmake) target={suffix}\n")
    };
    let template = format!("{GEN_ARCHSPECIFICRULES}%define {caller} mainmmake=/A\n{call}%end\n");
    let source = format!("%{caller} mainmmake={mainmmake}\n");
    fixture_from_text(families, &source, &template)
}

fn fixture_from_text(families: &[&str], source: &str, template: &str) -> Fixture {
    let temp_root = std::env::temp_dir().canonicalize().unwrap();
    let root = tempfile::tempdir_in(temp_root).unwrap();
    fs::create_dir_all(root.path().join("fixture")).unwrap();
    fs::create_dir_all(root.path().join("config")).unwrap();
    let recipe_path = root.path().join(RECIPE);
    let template_path = root.path().join(TEMPLATE);
    fs::write(&recipe_path, source.as_bytes()).unwrap();
    fs::write(&template_path, template.as_bytes()).unwrap();

    let expanded = expand_bytes(
        source.as_bytes(),
        &recipe_path,
        &template_path,
        GenmfLimits::default(),
    )
    .unwrap();
    let globals = BTreeMap::from([
        ("ARCH".into(), "esp32p4".into()),
        ("CPU".into(), "riscv".into()),
        ("FAMILY".into(), String::new()),
        ("AROS_TARGET_VARIANT".into(), String::new()),
    ]);
    let expanded_files = BTreeMap::from([(OWNER.to_owned(), expanded.text.clone())]);
    let graph = MetaMakeOwnerGraph::parse_expanded_files(
        &expanded_files,
        &globals,
        MetaMakeLimits {
            preserve_empty_endpoints: true,
            ..MetaMakeLimits::default()
        },
    )
    .unwrap();
    let mut expansion_origins = BTreeMap::new();
    for origin in expanded.provenance {
        let fragment = &expanded.text[origin.output_start_byte..origin.output_end_byte];
        if fragment.starts_with("#MM") {
            assert!(expansion_origins
                .insert((OWNER.to_owned(), origin.output_line), origin)
                .is_none());
        }
    }
    let source_digest = aros_common::sha256_bytes(source.as_bytes()).to_string();
    let template_digest = aros_common::sha256_bytes(template.as_bytes()).to_string();
    let projection = NativeOwnerProjection {
        sealed_configuration_inputs: BTreeMap::new(),
        architecture_context: TargetContext::default(),
        metamake_globals: std::collections::BTreeMap::new(),
        graph,
        expansion_origins,
        architecture_hook_families: families.iter().map(|family| (*family).to_owned()).collect(),
        owners: BTreeMap::from([(OWNER.to_owned(), RECIPE.to_owned())]),
        discovered_inputs: BTreeSet::from([RECIPE.to_owned()]),
        ignored_directories: BTreeSet::new(),
        excluded_paths: BTreeSet::new(),
        snapshots: BTreeMap::from([
            (RECIPE.to_owned(), source_digest),
            (TEMPLATE.to_owned(), template_digest),
        ]),
        policy_sha256: "test-policy".into(),
        expanded_bytes: expanded.text.len(),
    };
    Fixture {
        root,
        projection,
        context: TargetContext {
            cpu: Some("riscv".into()),
            platform: Some("esp32p4".into()),
            family: Some(String::new()),
            variant: Some(String::new()),
            toolchain: Some("gnu".into()),
            ..TargetContext::default()
        },
    }
}

fn bind(
    fixture: &Fixture,
    graph: &mut DependencyGraph,
    roots: &[String],
    diagnostics: &[Diagnostic],
    meta_edge_origins: &BTreeMap<(String, String), BTreeSet<String>>,
) -> Result<super::NativeMetaSemanticsEvidence, String> {
    bind_with_declarations(fixture, graph, roots, diagnostics, meta_edge_origins, &[])
}

fn bind_with_declarations(
    fixture: &Fixture,
    graph: &mut DependencyGraph,
    roots: &[String],
    diagnostics: &[Diagnostic],
    meta_edge_origins: &BTreeMap<(String, String), BTreeSet<String>>,
    declarations: &[NativeOptionalMetaDependency],
) -> Result<super::NativeMetaSemanticsEvidence, String> {
    fixture.projection.bind_native_meta_semantics(
        fixture.root.path(),
        graph,
        NativeMetaSemanticsSelection {
            context: &fixture.context,
            roots,
            declarations,
            diagnostics,
            meta_edge_origins,
        },
        &mut BTreeMap::new(),
    )
}

#[test]
fn selector_contract_only_omits_source_declared_virtual_dependencies() {
    for (source, expected_optional) in [
        ("#MM- required : absent-$(ARCH)\n", true),
        ("#MM required : absent-$(ARCH)\n", false),
        (
            "#MM- required : absent-$(ARCH)\n#MM required : absent-$(ARCH)\n",
            false,
        ),
    ] {
        let fixture = fixture_from_text(&[], source, "");
        let (mut graph, roots) = request_graph(vec!["required".into()]);
        // Physical #MM targets are supplied by the native parser, not imported
        // as virtual aliases. Model that production path and its exact origin.
        graph.add_meta_rule(MetaTargetRule {
            name: "required".into(),
            dependencies: vec!["absent-${AROS_TARGET_PLATFORM}".into()],
        });
        let origins = BTreeMap::from([(
            ("required".into(), "absent-esp32p4".into()),
            BTreeSet::from([RECIPE.into()]),
        )]);
        let declarations = [NativeOptionalMetaDependency {
            recipe: RECIPE.into(),
            target: "required".into(),
            dependency: "absent-${AROS_TARGET_PLATFORM}".into(),
            absence: NativeMetaAbsence::Selector,
        }];
        let result =
            bind_with_declarations(&fixture, &mut graph, &roots, &[], &origins, &declarations);
        if expected_optional {
            let evidence = result.expect("source-declared virtual selector is valid");
            assert_eq!(evidence.architecture_hook_omissions.len(), 1);
            assert!(!missing(&graph, &fixture, &roots, &[]).contains("absent-esp32p4"));
        } else {
            let error = result.expect_err("contract must not reclassify a mandatory #MM edge");
            assert!(error.contains("mandatory source dependency"), "{error}");
            assert!(missing(&graph, &fixture, &roots, &[]).contains("absent-esp32p4"));
        }
    }
}

#[test]
fn literal_mandatory_collision_cannot_cut_a_virtual_selectors_missing_child() {
    let fixture = fixture_from_text(
        &[],
        concat!(
            "#MM- required : branch-$(ARCH)\n",
            "#MM required : branch-esp32p4\n",
            "#MM- branch-$(ARCH) : absent-$(CPU)\n",
        ),
        "",
    );
    let (mut graph, roots) = request_graph(vec!["required".into()]);
    graph.add_meta_rule(MetaTargetRule {
        name: "required".into(),
        dependencies: vec![
            "branch-${AROS_TARGET_PLATFORM}".into(),
            "branch-esp32p4".into(),
        ],
    });
    let origins = BTreeMap::from([(
        ("required".into(), "branch-esp32p4".into()),
        BTreeSet::from([RECIPE.into()]),
    )]);
    let declarations = [NativeOptionalMetaDependency {
        recipe: RECIPE.into(),
        target: "required".into(),
        dependency: "branch-${AROS_TARGET_PLATFORM}".into(),
        absence: NativeMetaAbsence::Selector,
    }];
    let evidence =
        bind_with_declarations(&fixture, &mut graph, &roots, &[], &origins, &declarations)
            .expect("the virtual declaration is exact; a literal collision vetoes omissions");
    assert!(evidence.architecture_hook_omissions.is_empty());
    assert!(graph.meta_targets["branch-esp32p4"].contains("absent-riscv"));
    assert!(missing(&graph, &fixture, &roots, &[]).contains("absent-riscv"));
}

fn request_graph(dependencies: Vec<String>) -> (DependencyGraph, Vec<String>) {
    let mut graph = DependencyGraph::default();
    graph.add_meta_rule(MetaTargetRule {
        name: "requested".into(),
        dependencies,
    });
    (graph, vec!["requested".into()])
}

fn missing(
    graph: &DependencyGraph,
    fixture: &Fixture,
    roots: &[String],
    diagnostics: &[Diagnostic],
) -> BTreeSet<String> {
    graph
        .audit_native_dependency_graph(roots, &fixture.context, diagnostics)
        .missing_endpoints
        .into_iter()
        .map(|endpoint| endpoint.name)
        .collect()
}

fn selector_diagnostic(target: &str) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::CapabilityDrift,
        DiagnosticStage::CapabilityValidation,
        "fixture rejected producer",
    )
    .with_context(DiagnosticContext {
        target: Some(target.to_owned()),
        ..DiagnosticContext::default()
    })
}

fn native_hook_origins(edge: &HookEdge) -> BTreeMap<(String, String), BTreeSet<String>> {
    BTreeMap::from([(
        (edge.target.clone(), edge.canonical_dependency.clone()),
        BTreeSet::from([RECIPE.to_owned()]),
    )])
}

#[test]
fn fixture_uses_the_exact_supported_architecture_macro_body() {
    let digest = aros_common::sha256_bytes(
        GEN_ARCHSPECIFICRULES
            .strip_suffix('\n')
            .expect("macro body ends with one newline")
            .as_bytes(),
    );
    assert_eq!(
        digest.as_str(),
        "fc50253aa57b06aac5fdd739e2e871391d4e9a78d84989f83e52af71b162a062"
    );
}

#[test]
fn selected_policy_omits_only_the_three_generated_variant_slots() {
    let fixture = standard_fixture(&["module", "set-archincludes", "linklib"], "demo", "");
    let edges = all_hook_edges("demo");
    let (mut graph, roots) = request_graph(edges.iter().map(|edge| edge.target.clone()).collect());
    let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap();

    let actual: BTreeSet<_> = evidence
        .architecture_hook_omissions
        .iter()
        .map(|omission| (omission.target.clone(), omission.dependency.clone()))
        .collect();
    let expected: BTreeSet<_> = edges
        .iter()
        .map(|edge| (edge.target.clone(), edge.dependency.clone()))
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(evidence.verified_template_hooks.len(), 3);
    assert_eq!(
        evidence
            .verified_template_hooks
            .iter()
            .map(|proof| proof.family.as_str())
            .collect::<BTreeSet<_>>(),
        edges.iter().map(|edge| edge.family).collect()
    );
    for edge in &edges {
        assert!(!graph
            .meta_targets
            .get(&edge.target)
            .is_some_and(|dependencies| dependencies.contains(&edge.dependency)));
    }
    assert!(missing(&graph, &fixture, &roots, &[]).is_empty());
}

#[test]
fn unselected_family_keeps_its_exact_missing_variant_edges() {
    let fixture = standard_fixture(&["module"], "demo", "");
    let edges = all_hook_edges("demo");
    let (mut graph, roots) = request_graph(edges.iter().map(|edge| edge.target.clone()).collect());
    let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap();
    let actual: BTreeSet<_> = evidence
        .architecture_hook_omissions
        .iter()
        .map(|omission| (omission.target.clone(), omission.dependency.clone()))
        .collect();
    assert_eq!(
        actual,
        BTreeSet::from([(edges[0].target.clone(), edges[0].dependency.clone())])
    );
    assert!(graph.meta_targets[&edges[1].target].contains(&edges[1].dependency));
    assert!(graph.meta_targets[&edges[2].target].contains(&edges[2].dependency));
    assert_eq!(
        missing(&graph, &fixture, &roots, &[]),
        BTreeSet::from([edges[1].dependency.clone(), edges[2].dependency.clone()])
    );
}

#[test]
fn handwritten_selector_and_literal_collisions_do_not_borrow_template_proof() {
    let edge = hook_edge("demo", "module", "");
    let handwritten = [
        format!("#MM- {} : {}\n", edge.raw_target, edge.raw_dependency),
        format!("#MM- {} : {}\n", edge.raw_target, edge.dependency),
    ];
    for declaration in handwritten {
        let fixture = standard_fixture(&["module"], "demo", &declaration);
        let (mut graph, roots) = request_graph(vec![edge.target.clone()]);
        let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap();
        assert!(evidence.architecture_hook_omissions.is_empty());
        assert!(graph.meta_targets[&edge.target].contains(&edge.dependency));
        assert!(missing(&graph, &fixture, &roots, &[]).contains(&edge.dependency));

        let handwritten_declaration = fixture
            .projection
            .graph
            .declarations()
            .iter()
            .find(|candidate| {
                candidate.raw_target == edge.raw_target
                    && candidate.expanded_line > 20
                    && candidate
                        .dependencies
                        .iter()
                        .any(|dependency| dependency.concrete == edge.dependency)
            })
            .expect("handwritten declaration is in the source graph");
        let origin = fixture
            .projection
            .expansion_origin(handwritten_declaration)
            .unwrap();
        assert!(origin.template_path.is_none());
        assert!(origin.macro_stack.is_empty());
    }
}

#[test]
fn physical_and_bare_source_owners_of_a_child_keep_the_edge_required() {
    let edge = hook_edge("demo", "module", "");
    for declaration in [
        format!("#MM {}\n", edge.dependency),
        format!("#MM-\n{} :\n", edge.dependency),
    ] {
        let fixture = standard_fixture(&["module"], "demo", &declaration);
        assert!(fixture
            .projection
            .graph
            .owner_files(&edge.dependency)
            .is_some());
        let owner_declaration = fixture
            .projection
            .graph
            .declarations()
            .iter()
            .find(|candidate| candidate.target == edge.dependency)
            .unwrap();
        assert!(owner_declaration.claims_make_owner);

        let (mut graph, roots) = request_graph(vec![edge.target.clone()]);
        let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap();
        assert!(evidence.architecture_hook_omissions.is_empty());
        assert!(graph.meta_targets[&edge.target].contains(&edge.dependency));
        assert!(missing(&graph, &fixture, &roots, &[]).contains(&edge.dependency));
    }
}

#[test]
fn nonvirtual_selector_claim_on_the_parent_vetoes_the_generated_claim() {
    let edge = hook_edge("demo", "module", "");
    let handwritten_owner = format!("#MM {} : {}\n", edge.raw_target, edge.raw_dependency);
    let fixture = standard_fixture(&["module"], "demo", &handwritten_owner);
    assert!(fixture.projection.graph.owner_files(&edge.target).is_some());

    // Model the independently parsed native spelling for this same edge. Its
    // origin remains bound to the source recipe, but the handwritten physical
    // declaration still vetoes the template-only omission.
    let (mut graph, roots) = request_graph(vec![edge.target.clone()]);
    graph.add_meta_rule(MetaTargetRule {
        name: edge.target.clone(),
        dependencies: vec![edge.canonical_dependency.clone()],
    });
    let origins = native_hook_origins(&edge);
    let evidence = bind(&fixture, &mut graph, &roots, &[], &origins).unwrap();
    assert!(evidence.architecture_hook_omissions.is_empty());
    assert!(missing(&graph, &fixture, &roots, &[]).contains(&edge.dependency));
}

#[test]
fn bare_owner_of_a_parent_cannot_be_hidden_by_a_virtual_template_edge() {
    let edge = hook_edge("demo", "module", "");
    let bare_owner = format!("#MM-\n{} :\n", edge.target);
    let fixture = standard_fixture(&["module"], "demo", &bare_owner);
    assert!(fixture.projection.graph.owner_files(&edge.target).is_some());

    let (mut graph, roots) = request_graph(vec![edge.target.clone()]);
    graph.add_meta_rule(MetaTargetRule {
        name: edge.target.clone(),
        dependencies: vec![edge.canonical_dependency.clone()],
    });
    let origins = native_hook_origins(&edge);
    let evidence = bind(&fixture, &mut graph, &roots, &[], &origins).unwrap();
    assert!(evidence.architecture_hook_omissions.is_empty());
    assert!(graph.meta_targets[&edge.target].contains(&edge.canonical_dependency));
    assert!(missing(&graph, &fixture, &roots, &[]).contains(&edge.dependency));
}

fn physical_linklib_fixture(extra: &str) -> (Fixture, DependencyGraph, Vec<String>, HookEdge) {
    physical_architecture_fixture("linklib", "-linklib", extra)
}

fn physical_architecture_fixture(
    family: &'static str,
    suffix: &str,
    extra: &str,
) -> (Fixture, DependencyGraph, Vec<String>, HookEdge) {
    let template = format!(
        "{}{}",
        standard_template(),
        include_str!("../tests/fixtures/architecture_objects.tmpl")
    );
    let source = format!(
        "%build_module mainmmake=demo\n\
         %build_archspecific mainmmake=demo maindir=rom/demo modname=demo arch=esp32p4-riscv files=base\n{extra}"
    );
    let mut fixture = fixture_from_text(&[family], &source, &template);
    fixture.projection.architecture_context = fixture.context.clone();
    let (scope, states) = crate::arch_endpoint_effects::collect_arch_effect_scope_with_context(
        &source,
        Some(&fixture.context),
    );
    let scan = crate::arch_endpoint_effects::collect_arch_endpoint_effects(
        &source,
        std::path::Path::new(RECIPE),
        &scope,
        Some(&states),
    )
    .unwrap();
    assert!(scan.rejected.is_empty());
    let edge = hook_edge("demo", family, suffix);
    let (mut graph, roots) = request_graph(vec![edge.target.clone()]);
    graph.arch_endpoint_effects = scan
        .effects
        .into_iter()
        .filter(|effect| family == "module" || effect.endpoint == edge.target)
        .collect();
    assert_eq!(
        graph.arch_endpoint_effects.len(),
        if family == "module" { 2 } else { 1 }
    );
    (fixture, graph, roots, edge)
}

#[test]
fn physical_object_parent_omits_only_its_generated_variant_slot() {
    let (fixture, mut graph, roots, edge) = physical_architecture_fixture("module", "", "");
    let original_effects = graph.arch_endpoint_effects.clone();
    let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap();
    assert!(evidence.architecture_hook_omissions.iter().any(|omission| {
        omission.target == edge.target && omission.dependency == edge.dependency
    }));
    assert_eq!(graph.arch_endpoint_effects, original_effects);
    assert!(!missing(&graph, &fixture, &roots, &[]).contains(&edge.dependency));
}

#[test]
fn physical_object_parent_retains_continued_source_prerequisites() {
    let (fixture, mut graph, roots, edge) = physical_architecture_fixture(
        "module",
        "",
        concat!(
            "#MM- demo-esp32p4-riscv : \\\n",
            "#MM headers-one \\\n",
            "#MM headers-two\n",
        ),
    );
    let original_effects = graph.arch_endpoint_effects.clone();
    let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap();
    assert!(evidence.architecture_hook_omissions.iter().any(|omission| {
        omission.target == edge.target && omission.dependency == edge.dependency
    }));
    assert_eq!(graph.arch_endpoint_effects, original_effects);
    let missing = missing(&graph, &fixture, &roots, &[]);
    assert!(!missing.contains(&edge.dependency));
    assert!(missing.contains("headers-one"));
    assert!(missing.contains("headers-two"));
}

#[test]
fn independently_verified_physical_parent_retains_its_effect_and_omits_only_variant() {
    let (fixture, mut graph, roots, edge) = physical_linklib_fixture("");
    let original_effect = graph.arch_endpoint_effects[0].clone();
    let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap();
    assert!(evidence.architecture_hook_omissions.iter().any(|omission| {
        omission.target == edge.target && omission.dependency == edge.dependency
    }));
    assert_eq!(graph.arch_endpoint_effects, vec![original_effect]);
    assert!(!missing(&graph, &fixture, &roots, &[]).contains(&edge.dependency));
}

#[test]
fn forged_or_colliding_physical_parent_never_authorizes_a_variant_omission() {
    let (fixture, mut graph, roots, _) = physical_linklib_fixture("");
    graph.arch_endpoint_effects[0].line += 1;
    assert!(bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).is_err());

    let (fixture, mut graph, roots, _) = physical_linklib_fixture("");
    if let crate::arch_endpoint_effects::ArchEndpointEffectData::EmptyLinklibAggregate {
        module_sources,
        ..
    } = &mut graph.arch_endpoint_effects[0].data
    {
        module_sources.push("unselected-feature-source".into());
    }
    assert!(bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).is_err());

    let (fixture, mut graph, roots, _) =
        physical_linklib_fixture("#MM demo-esp32p4-riscv-linklib\n");
    assert!(bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).is_err());
}

#[test]
fn handwritten_variant_collision_on_a_verified_parent_remains_required() {
    let edge = hook_edge("demo", "linklib", "-linklib");
    let (fixture, mut graph, roots, _) =
        physical_linklib_fixture(&format!("#MM- {} : {}\n", edge.raw_target, edge.dependency));
    let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap();
    assert!(evidence.architecture_hook_omissions.is_empty());
    assert!(missing(&graph, &fixture, &roots, &[]).contains(&edge.dependency));
}

#[test]
fn rejected_child_and_independent_ingress_remain_required() {
    let edge = hook_edge("demo", "module", "");
    let fixture = standard_fixture(&["module"], "demo", "");
    let rejected = selector_diagnostic(&edge.dependency);
    let (mut rejected_graph, roots) = request_graph(vec![edge.target.clone()]);
    let evidence = bind(
        &fixture,
        &mut rejected_graph,
        &roots,
        std::slice::from_ref(&rejected),
        &BTreeMap::new(),
    )
    .unwrap();
    assert!(evidence.architecture_hook_omissions.is_empty());
    assert!(rejected_graph.meta_targets[&edge.target].contains(&edge.dependency));

    let mut graph = DependencyGraph::default();
    graph.add_meta_rule(MetaTargetRule {
        name: "requested".into(),
        dependencies: vec![
            edge.target.clone(),
            edge.dependency.clone(),
            "other-parent".into(),
        ],
    });
    graph.add_meta_rule(MetaTargetRule {
        name: "other-parent".into(),
        dependencies: vec![edge.dependency.clone()],
    });
    let roots = vec!["requested".into()];
    let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap();
    assert!(evidence.architecture_hook_omissions.iter().any(|omission| {
        omission.target == edge.target && omission.dependency == edge.dependency
    }));
    assert!(!graph.meta_targets[&edge.target].contains(&edge.dependency));
    assert!(graph.meta_targets["requested"].contains(&edge.dependency));
    assert!(graph.meta_targets["other-parent"].contains(&edge.dependency));
    assert!(missing(&graph, &fixture, &roots, &[]).contains(&edge.dependency));
}

#[test]
fn changed_or_duplicate_architecture_macro_is_rejected() {
    let edge = hook_edge("demo", "module", "");
    let changed = GEN_ARCHSPECIFICRULES.replacen(
        "%(mainmmake)-$(CPU)%(target)%(subtarget)",
        "%(mainmmake)-$(CPU)-changed%(target)%(subtarget)",
        1,
    );
    assert_ne!(changed, GEN_ARCHSPECIFICRULES);
    let changed_template = format!("{changed}{BUILD_MODULE}");
    let fixture = fixture_from_text(
        &["module"],
        "%build_module mainmmake=demo\n",
        &changed_template,
    );
    let (mut graph, roots) = request_graph(vec![edge.target.clone()]);
    let error = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap_err();
    assert!(error.contains("supported semantics"), "{error}");

    let duplicate_template =
        format!("{GEN_ARCHSPECIFICRULES}{GEN_ARCHSPECIFICRULES}{BUILD_MODULE}");
    let fixture = fixture_from_text(
        &["module"],
        "%build_module mainmmake=demo\n",
        &duplicate_template,
    );
    let (mut graph, roots) = request_graph(vec![edge.target]);
    let error = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap_err();
    assert!(error.contains("not unique/exact"), "{error}");
}

#[test]
fn hook_proof_requires_the_immediate_build_module_caller() {
    let edge = hook_edge("demo", "module", "");
    let fixture = fixture_from_text(
        &["module"],
        "%gen_archspecificrules mainmmake=demo\n",
        &standard_template(),
    );
    let (mut graph, roots) = request_graph(vec![edge.target.clone()]);
    let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap();
    assert!(evidence.architecture_hook_omissions.is_empty());
    assert!(missing(&graph, &fixture, &roots, &[]).contains(&edge.dependency));
}

#[test]
fn duplicate_architecture_suffix_is_literal_and_not_normalized() {
    let mainmmake = "demo-esp32p4-riscv";
    let edge = hook_edge(mainmmake, "module", "");
    let normalized_child = "demo-esp32p4-riscv-".to_owned();
    let fixture = standard_fixture(&["module"], mainmmake, "");
    let (mut graph, roots) = request_graph(vec![edge.target.clone(), normalized_child.clone()]);
    let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new()).unwrap();
    assert!(evidence.architecture_hook_omissions.iter().any(|omission| {
        omission.target == edge.target && omission.dependency == edge.dependency
    }));
    assert_ne!(normalized_child, edge.dependency);
    assert!(!evidence
        .architecture_hook_omissions
        .iter()
        .any(|omission| omission.dependency == normalized_child));
    assert!(!graph.meta_targets[&edge.target].contains(&edge.dependency));
    assert!(graph.meta_targets["requested"].contains(&normalized_child));
    let missing = missing(&graph, &fixture, &roots, &[]);
    assert!(!missing.contains(&edge.dependency));

    let raw = fixture
        .projection
        .graph
        .declarations()
        .iter()
        .find(|declaration| declaration.target == edge.target)
        .unwrap();
    assert_eq!(raw.raw_target, edge.raw_target);
}

#[test]
fn production_caller_names_do_not_authorize_renamed_fixture_macros() {
    for caller in ["build_linklib", "build_prog", "build_progs"] {
        let fixture = direct_caller_fixture(&["module"], caller, "demo", "");
        let edge = hook_edge("demo", "module", "");
        let (mut graph, roots) = request_graph(vec![edge.target]);
        let error = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new())
            .expect_err("renaming the small fixture macro must not borrow production proof");
        assert!(
            error.contains(&format!("template macro {caller}"))
                && error.contains("differs from its supported semantics"),
            "{caller}: {error}"
        );
    }
}

#[test]
fn production_caller_names_do_not_admit_suffix_specific_hooks() {
    for caller in ["build_linklib", "build_prog", "build_progs"] {
        for (family, suffix) in [
            ("linklib", "-linklib"),
            ("set-archincludes", "-set-archincludes"),
        ] {
            let fixture = direct_caller_fixture(&[family], caller, "demo", suffix);
            let edge = hook_edge("demo", family, suffix);
            let (mut graph, roots) = request_graph(vec![edge.target.clone()]);
            let evidence = bind(&fixture, &mut graph, &roots, &[], &BTreeMap::new())
                .unwrap_or_else(|error| panic!("{caller} {suffix}: {error}"));

            assert!(evidence.architecture_hook_omissions.is_empty());
            assert!(evidence.verified_template_hooks.is_empty());
            assert!(graph.meta_targets[&edge.target].contains(&edge.dependency));
            assert!(missing(&graph, &fixture, &roots, &[]).contains(&edge.dependency));
        }
    }
}
