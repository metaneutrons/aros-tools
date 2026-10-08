use super::{NativeMetaSemanticsSelection, NativeOwnerProjection};
use crate::ast::{InventoryTargetIdentity, MetaTargetRule, ModuleMacroForm, ModuleType};
use crate::genmf_projection::{expand_bytes, Limits as GenmfLimits};
use crate::metamake_owner_graph::{Limits as MetaMakeLimits, MetaMakeOwnerGraph};
use crate::{DependencyGraph, TargetContext};
use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

const CONSUMER_OWNER: &str = "fixture/consumers/mmakefile";
const CONSUMER_RECIPE: &str = "fixture/consumers/mmakefile.src";
const PROVIDER_OWNER_PREFIX: &str = "fixture/providers/";
const TEMPLATE: &str = "config/make.tmpl";
const MODULE_TEMPLATE: &str = include_str!("../tests/fixtures/empty_library_lists.tmpl");
const GEN_ARCHSPECIFICRULES_TEMPLATE: &str = r"
%define gen_archspecificrules mainmmake=/A target= subtarget=
#MM- %(mainmmake)%(target)%(subtarget) : \
#MM             %(mainmmake)-$(CPU)%(target)%(subtarget)

#MM- %(mainmmake)-$(ARCH)-$(CPU)%(target)%(subtarget) : \
#MM             %(mainmmake)-$(ARCH)-$(CPU)-$(AROS_TARGET_VARIANT)%(target)%(subtarget)

#MM- %(mainmmake)-$(ARCH)-$(AROS_TARGET_VARIANT)%(target)%(subtarget) : \
#MM             %(mainmmake)-$(ARCH)-$(CPU)%(target)%(subtarget)

#MM- %(mainmmake)-$(ARCH)%(target)%(subtarget) : \
#MM             %(mainmmake)-$(ARCH)-$(AROS_TARGET_VARIANT)%(target)%(subtarget)

#MM- %(mainmmake)-$(FAMILY)%(target)%(subtarget) : \
#MM             %(mainmmake)-$(ARCH)%(target)%(subtarget)

#MM- %(mainmmake)-$(CPU)%(target)%(subtarget) : \
#MM             %(mainmmake)-$(FAMILY)%(target)%(subtarget)
%end
";
const BUILD_LINKLIB_TEMPLATE: &str = r#"
%define build_linklib mmake=/A libname=/A files= objcfiles= cxxfiles= \
  asmfiles= objs= objdir="$(GENDIR)/$(CURDIR)" libdir="$(AROS_LIB)" \
  includedir= srcdir= \
  cppflags="$(CPPFLAGS)" cflags= dflags= cxxflags= dxxflags= \
  aflags="$(AFLAGS)" compiler=target lto="$(TARGET_LTO)" usetree=no

%gen_archspecificrules mainmmake=%(mmake)

# assign and generate the local variables used in this macro
%(mmake)_LIBNAME           := %(libname)
%(mmake)_LINKLIB           := %(libdir)/lib%(libname).a

%(mmake)_FILES             ?= %(files)
%(mmake)_ASMFILES          := %(asmfiles)
%(mmake)_OBJCFILES         := %(objcfiles)
%(mmake)_CXXFILES          := %(cxxfiles)

%(mmake)_OBJDIR            ?= %(objdir)
%(mmake)_ARCHOBJS          := $(wildcard $(%(mmake)_OBJDIR)/arch/*.o)
ifeq (%(usetree),no)
    %(mmake)_ARCHFILES     := $(basename $(notdir $(%(mmake)_ARCHOBJS)))
else
    %(mmake)_ARCHFILES     := $(basename $(patsubst $(%(mmake)_OBJDIR)/%,%,$(%(mmake)_ARCHOBJS)))
endif
%(mmake)_C_NARCHFILES      := $(filter-out $(%(mmake)_ARCHFILES),$(%(mmake)_FILES))
%(mmake)_C_FILES           ?= $(%(mmake)_C_NARCHFILES)
%(mmake)_CXX_NARCHFILES    := $(filter-out $(%(mmake)_ARCHFILES),$(%(mmake)_CXXFILES))
%(mmake)_CXX_FILES         ?= $(%(mmake)_CXX_NARCHFILES)
%(mmake)_OBJC_NARCHFILES   := $(filter-out $(%(mmake)_ARCHFILES),$(%(mmake)_OBJCFILES))
%(mmake)_OBJC_FILES        ?= $(%(mmake)_OBJC_NARCHFILES)

ifeq (%(usetree),no)
    %(mmake)_OBJ_FILES     ?= $(addprefix $(%(mmake)_OBJDIR)/,$(notdir $(%(mmake)_C_NARCHFILES:=.o) $(%(mmake)_CXX_NARCHFILES:=.o) $(%(mmake)_ASMFILES:=.o) $(%(mmake)_OBJC_NARCHFILES:=.o)))
else
    %(mmake)_OBJ_FILES     ?= $(addprefix $(%(mmake)_OBJDIR)/,$(%(mmake)_C_NARCHFILES:=.o) $(%(mmake)_CXX_NARCHFILES:=.o) $(%(mmake)_ASMFILES:=.o) $(%(mmake)_OBJC_NARCHFILES:=.o))
endif
%(mmake)_OBJS              ?= $(%(mmake)_ARCHOBJS) $(%(mmake)_OBJ_FILES) %(objs)
%(mmake)_DEPS              := $(patsubst %.o,%.d,$(%(mmake)_OBJS))

%(mmake)_CPPFLAGS          := %(cppflags)
ifneq (%(includedir),)
    %(mmake)_CPPFLAGS      += -I%(includedir)
endif

ifeq ("%(cflags)","")
ifeq (%(compiler),target)
%(mmake)_CFLAGS            := $(CFLAGS)
endif
ifeq (%(compiler),host)
%(mmake)_CFLAGS            := $(HOST_CFLAGS)
endif
ifeq (%(compiler),kernel)
%(mmake)_CFLAGS            := $(strip $(KERNEL_ISA_CFLAGS) $(KERNEL_CFLAGS))
endif
else
%(mmake)_CFLAGS            := %(cflags)
endif

ifeq ("%(cxxflags)","")
ifeq (%(compiler),target)
%(mmake)_CXXFLAGS            := $(CXXFLAGS)
endif
ifeq (%(compiler),host)
%(mmake)_CXXFLAGS            := $(HOST_CXXFLAGS)
endif
ifeq (%(compiler),kernel)
%(mmake)_CXXFLAGS            := $(strip $(KERNEL_ISA_CXXFLAGS) $(KERNEL_CXXFLAGS))
endif
else
%(mmake)_CXXFLAGS          := %(cxxflags)
endif

ifeq (%(lto),yes)
ifeq (%(compiler),target)
        %(mmake)_CFLAGS    := $(strip $(LTO_CFLAGS) $(%(mmake)_CFLAGS))
        %(mmake)_CXXFLAGS  := $(strip $(LTO_CFLAGS) $(%(mmake)_CXXFLAGS))
endif
endif
%(mmake)_AFLAGS            := %(aflags)
%(mmake)_DFLAGS            := %(dflags)
ifneq (%(dflags),)
    %(mmake)_DFLAGS        := %(dflags)
else
    %(mmake)_DFLAGS        := $(%(mmake)_CFLAGS)
endif
%(mmake)_DXXFLAGS          := %(dxxflags)
ifneq (%(dxxflags),)
    %(mmake)_DXXFLAGS      := %(dxxflags)
else
    %(mmake)_DXXFLAGS      := $(%(mmake)_CXXFLAGS)
endif

.PHONY : %(mmake) %(mmake)-clean %(mmake)-quick

#MM
%(mmake)-quick : %(mmake)

#MM %(mmake) : includes-generate-deps
%(mmake) : $(%(mmake)_LINKLIB)

ifneq ($(filter $(TARGET),%(mmake) %(mmake)-quick),)

%rule_compile_cxx_multi mmake=%(mmake) \
    basenames="$(%(mmake)_CXX_FILES)" targetdir="$(%(mmake)_OBJDIR)" \
    cppflags="$(%(mmake)_CPPFLAGS)" cxxflags="$(%(mmake)_CXXFLAGS)" dxxflags="$(%(mmake)_DXXFLAGS)" \
    compiler="%(compiler)" srcdir="%(srcdir)" usetree="%(usetree)"
%rule_compile_objc_multi mmake=%(mmake) \
    basenames="$(%(mmake)_OBJC_FILES)" targetdir="$(%(mmake)_OBJDIR)" \
    cppflags="$(%(mmake)_CPPFLAGS)" cflags="$(%(mmake)_CFLAGS)" dflags="$(%(mmake)_DFLAGS)" \
    compiler="%(compiler)" srcdir="%(srcdir)"
%rule_compile_multi mmake=%(mmake) \
    basenames="$(%(mmake)_C_FILES)" targetdir="$(%(mmake)_OBJDIR)" \
    cppflags="$(%(mmake)_CPPFLAGS)" cflags="$(%(mmake)_CFLAGS)" dflags="$(%(mmake)_DFLAGS)" \
    compiler="%(compiler)" srcdir="%(srcdir)" usetree="%(usetree)"
%rule_assemble_multi mmake=%(mmake) \
    cmd="$(%(mmake)_ASSEMBLER)" basenames="$(%(mmake)_ASMFILES)" targetdir="$(%(mmake)_OBJDIR)" \
    cppflags="$(%(mmake)_CPPFLAGS)" aflags="$(%(mmake)_AFLAGS)"

%rule_link_linklib mmake=%(mmake) libname="%(libname)" objs="$(%(mmake)_OBJS)" libdir="%(libdir)" linker="%(compiler)"
endif

%include_deps depstargets="%(mmake) %(mmake)-quick" deps="$(%(mmake)_DEPS)"


%rule_makedirs dirs="$(%(mmake)_OBJDIR) %(libdir)" setuptarget="$(%(mmake)_OBJS) $(%(mmake)_DEPS) $(%(mmake)_LINKLIB)"

%(mmake)-clean : FILES := $(%(mmake)_OBJS) $(%(mmake)_LINKLIB) $(%(mmake)_DEPS)
#MM
%(mmake)-clean ::
	$(Q)$(ECHO) "Cleaning up for metatarget %(mmake)"
	$(Q)$(RM) $(FILES)

%end
"#;

const ALIASES: [(&str, &str, &str); 3] = [
    ("consumer-stdc", "stdc.static", "linklibs-stdc-static"),
    (
        "consumer-zlib",
        "z-nogzip.static",
        "linklibs-z-nogzip-static",
    ),
    ("consumer-bz2", "bz2_nostdio", "linklibs-bz2-nostdio"),
];

struct Fixture {
    root: tempfile::TempDir,
    projection: NativeOwnerProjection,
    context: TargetContext,
}

fn make_fixture(
    handwritten_source: &str,
    source_owned_alias: Option<&str>,
    provider_libname_drift: Option<(&str, &str)>,
) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let source_root = root.path().canonicalize().unwrap();
    let template_path = source_root.join(TEMPLATE);
    fs::create_dir_all(template_path.parent().unwrap()).unwrap();
    let template =
        format!("{MODULE_TEMPLATE}\n{GEN_ARCHSPECIFICRULES_TEMPLATE}\n{BUILD_LINKLIB_TEMPLATE}\n");
    fs::write(&template_path, template.as_bytes()).unwrap();

    let mut consumer_source = String::new();
    for (consumer, library, _) in ALIASES {
        writeln!(
            consumer_source,
            "%build_module mmake={consumer} modname={consumer} modtype=library uselibs={library}"
        )
        .unwrap();
    }
    consumer_source.push_str(handwritten_source);

    let mut sources = vec![(
        CONSUMER_OWNER.to_owned(),
        CONSUMER_RECIPE.to_owned(),
        consumer_source,
    )];
    for (_, library, provider) in ALIASES {
        let recipe = format!("fixture/providers/{provider}/mmakefile.src");
        let owner = format!("{PROVIDER_OWNER_PREFIX}{provider}/mmakefile");
        let declared_library = provider_libname_drift
            .filter(|(drifted_provider, _)| *drifted_provider == provider)
            .map_or(library, |(_, drifted_name)| drifted_name);
        let supplemental_rule = if provider == "linklibs-z-nogzip-static" {
            "#MM linklibs-z-nogzip-static : zlib-fetch\n"
        } else {
            ""
        };
        let source = format!(
            "{supplemental_rule}%build_linklib mmake={provider} libname={declared_library}\n"
        );
        sources.push((owner, recipe, source));
    }
    if let Some(alias) = source_owned_alias {
        sources.push((
            "fixture/collision/mmakefile".into(),
            "fixture/collision/mmakefile.src".into(),
            format!("#MM {alias}\n"),
        ));
    }

    let globals = BTreeMap::from([
        ("ARCH".into(), "esp32p4".into()),
        ("CPU".into(), "riscv".into()),
        ("FAMILY".into(), String::new()),
        ("AROS_TARGET_VARIANT".into(), String::new()),
    ]);
    let mut expanded_files = BTreeMap::new();
    let mut expansion_origins = BTreeMap::new();
    let mut owners = BTreeMap::new();
    let mut snapshots = BTreeMap::new();
    let mut discovered_inputs = BTreeSet::new();

    for (owner, recipe, source) in &sources {
        let recipe_path = source_root.join(recipe);
        fs::create_dir_all(recipe_path.parent().unwrap()).unwrap();
        fs::write(&recipe_path, source.as_bytes()).unwrap();
        discovered_inputs.insert(recipe.clone());
        snapshots.insert(
            recipe.clone(),
            aros_common::sha256_bytes(source.as_bytes()).to_string(),
        );
        let expanded = expand_bytes(
            source.as_bytes(),
            &recipe_path,
            &template_path,
            GenmfLimits::default(),
        )
        .unwrap();
        for origin in expanded.provenance {
            let fragment = &expanded.text[origin.output_start_byte..origin.output_end_byte];
            if fragment.starts_with("#MM") {
                assert!(expansion_origins
                    .insert((owner.clone(), origin.output_line), origin)
                    .is_none());
            }
        }
        expanded_files.insert(owner.clone(), expanded.text);
        owners.insert(owner.clone(), recipe.clone());
    }

    let graph = MetaMakeOwnerGraph::parse_expanded_files(
        &expanded_files,
        &globals,
        MetaMakeLimits {
            preserve_empty_endpoints: true,
            ..MetaMakeLimits::default()
        },
    )
    .unwrap();
    snapshots.insert(
        TEMPLATE.to_owned(),
        aros_common::sha256_bytes(template.as_bytes()).to_string(),
    );
    let projection = NativeOwnerProjection {
        sealed_configuration_inputs: BTreeMap::new(),
        architecture_context: TargetContext::default(),
        metamake_globals: std::collections::BTreeMap::new(),
        graph,
        expansion_origins,
        architecture_hook_families: BTreeSet::new(),
        owners,
        discovered_inputs,
        ignored_directories: BTreeSet::new(),
        excluded_paths: BTreeSet::new(),
        snapshots,
        policy_sha256: "test-policy".into(),
        expanded_bytes: expanded_files.values().map(String::len).sum(),
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

fn identity(
    mmake: &str,
    name: &str,
    module_type: ModuleType,
    use_libs: Vec<String>,
    link_libs: Vec<String>,
) -> InventoryTargetIdentity {
    let module_macro = (module_type == ModuleType::Library).then_some(ModuleMacroForm::Full);
    InventoryTargetIdentity {
        mmake_name: mmake.into(),
        target_name: name.into(),
        module_type,
        dir_path: PathBuf::from("fixture"),
        variant_32bit: false,
        declared_mod_type: None,
        mod_suffix: None,
        linklib_output_dir: None,
        genmodule_abi: false,
        genmodule_only: false,
        module_macro,
        target_dir: None,
        linklib_name: None,
        genmodule_linklibs: None,
        config_relative_libraries: Vec::new(),
        canonical_linklib_output: false,
        canonical_linklib_eligible: false,
        empty_archive: false,
        dependencies: Vec::new(),
        link_libs,
        use_libs,
        link_options: Vec::new(),
        spec_switches: Vec::new(),
    }
}

fn native_graph(
    omit_provider: Option<&str>,
    ambiguous_library: Option<&str>,
    pre_rebound: bool,
    explicit_alias: Option<&str>,
) -> (
    DependencyGraph,
    BTreeMap<(String, String), BTreeSet<String>>,
) {
    let mut graph = DependencyGraph::default();
    let mut origins = BTreeMap::new();
    let mut root_dependencies = Vec::new();
    for (consumer, library, provider) in ALIASES {
        let alias = format!("linklibs-{library}");
        root_dependencies.push(consumer.to_owned());
        if pre_rebound && consumer == "consumer-zlib" {
            graph.add_meta_rule(MetaTargetRule {
                name: consumer.into(),
                dependencies: vec![provider.into()],
            });
        } else {
            graph.add_meta_rule(MetaTargetRule {
                name: consumer.into(),
                dependencies: vec![alias.clone()],
            });
        }
        origins.insert(
            (consumer.to_owned(), alias),
            BTreeSet::from([CONSUMER_RECIPE.to_owned()]),
        );

        let selected = if ambiguous_library == Some(library) {
            vec![provider.to_owned(), format!("{provider}-duplicate")]
        } else {
            vec![provider.to_owned()]
        };
        graph.inventory_targets.push(identity(
            consumer,
            consumer,
            ModuleType::Library,
            vec![library.to_owned()],
            selected,
        ));
        if omit_provider != Some(library) {
            graph.inventory_targets.push(identity(
                provider,
                library,
                ModuleType::LinkLib,
                Vec::new(),
                Vec::new(),
            ));
        }
        if ambiguous_library == Some(library) {
            graph.inventory_targets.push(identity(
                &format!("{provider}-duplicate"),
                library,
                ModuleType::LinkLib,
                Vec::new(),
                Vec::new(),
            ));
        }
    }
    graph.add_meta_rule(MetaTargetRule {
        name: "requested".into(),
        dependencies: std::mem::take(&mut root_dependencies),
    });
    if let Some(consumer) = explicit_alias {
        let library = ALIASES
            .iter()
            .find(|(name, _, _)| *name == consumer)
            .unwrap()
            .1;
        graph
            .explicit_meta_edges
            .insert((consumer.to_owned(), format!("linklibs-{library}")));
    }
    // The selection root is encoded in the native meta graph; all three
    // consumers are reachable from this one requested target.
    graph.meta_targets.entry("requested".into()).or_default();
    (graph, origins)
}

fn bind(
    fixture: &Fixture,
    graph: &mut DependencyGraph,
    origins: &BTreeMap<(String, String), BTreeSet<String>>,
    diagnostics: &[Diagnostic],
) -> Vec<super::VerifiedNativeLibraryAlias> {
    fixture
        .projection
        .bind_library_aliases(
            fixture.root.path(),
            graph,
            NativeMetaSemanticsSelection {
                context: &fixture.context,
                roots: &["requested".into()],
                declarations: &[],
                diagnostics,
                meta_edge_origins: origins,
            },
        )
        .unwrap()
}

#[test]
fn real_punctuation_and_underscore_aliases_keep_their_typed_producers() {
    let fixture = make_fixture("", None, None);
    let (mut graph, origins) = native_graph(None, None, false, None);
    let receipts = bind(&fixture, &mut graph, &origins, &[]);

    for (consumer, library, provider) in ALIASES {
        let dependencies = &graph.meta_targets[consumer];
        assert!(!dependencies.contains(&format!("linklibs-{library}")));
        assert!(dependencies.contains(provider));
        assert!(graph.inventory_targets.iter().any(|target| {
            target.mmake_name == provider
                && target.module_type == ModuleType::LinkLib
                && target.target_name == library
        }));
    }
    assert_eq!(receipts.len(), ALIASES.len());
    assert!(receipts.iter().all(|receipt| !receipt.already_rebound));

    // zlib's handwritten fetch prerequisite is a required same-recipe edge,
    // supplemental to (not a replacement for) the sealed build_linklib rule.
    let z_provider = "linklibs-z-nogzip-static";
    let z_declaration = fixture
        .projection
        .graph
        .declarations()
        .iter()
        .find(|declaration| {
            declaration.target == z_provider
                && declaration
                    .dependencies
                    .iter()
                    .any(|edge| edge.raw_expression == "zlib-fetch")
        })
        .expect("zlib fetch prerequisite remains in provider source graph");
    assert!(z_declaration.claims_make_owner);
    assert_eq!(z_declaration.raw_target, z_provider);
    assert_eq!(z_declaration.dependencies.len(), 1);
    assert_eq!(z_declaration.dependencies[0].concrete, "zlib-fetch");

    // build_linklib emits architecture-chain aliases through the nested
    // gen_archspecificrules macro; those virtual declarations are not owners.
    assert!(fixture
        .projection
        .graph
        .declarations()
        .iter()
        .filter(|declaration| !declaration.claims_make_owner)
        .any(|declaration| {
            fixture
                .projection
                .expansion_origins
                .get(&(declaration.file.clone(), declaration.expanded_line))
                .is_some_and(|origin| {
                    origin
                        .macro_stack
                        .iter()
                        .map(|frame| frame.name.as_str())
                        .collect::<Vec<_>>()
                        == ["build_linklib", "gen_archspecificrules"]
                })
        }));
}

#[test]
fn already_rebound_edge_is_accepted_without_rewriting_it_again() {
    let fixture = make_fixture("", None, None);
    let (mut graph, origins) = native_graph(None, None, true, None);
    let receipts = bind(&fixture, &mut graph, &origins, &[]);
    assert!(receipts.iter().any(|receipt| {
        receipt.target == "consumer-zlib"
            && receipt.provider == "linklibs-z-nogzip-static"
            && receipt.already_rebound
    }));
    assert!(graph.meta_targets["consumer-zlib"].contains("linklibs-z-nogzip-static"));
    assert!(!graph.meta_targets["consumer-zlib"].contains("linklibs-z-nogzip.static"));
}

#[test]
fn handwritten_native_claim_and_source_owned_alias_veto_rebinding() {
    let fixture = make_fixture("", None, None);
    let (mut graph, origins) = native_graph(None, None, false, Some("consumer-stdc"));
    let receipts = bind(&fixture, &mut graph, &origins, &[]);
    assert!(!receipts.iter().any(|proof| proof.target == "consumer-stdc"));
    assert!(graph.meta_targets["consumer-stdc"].contains("linklibs-stdc.static"));

    let fixture = make_fixture("", Some("linklibs-z-nogzip.static"), None);
    let (mut graph, origins) = native_graph(None, None, false, None);
    let receipts = bind(&fixture, &mut graph, &origins, &[]);
    assert!(!receipts.iter().any(|proof| proof.target == "consumer-zlib"));
    assert!(graph.meta_targets["consumer-zlib"].contains("linklibs-z-nogzip.static"));
}

#[test]
fn handwritten_source_claim_cannot_borrow_the_template_proof() {
    let fixture = make_fixture("#MM- consumer-stdc : linklibs-stdc.static\n", None, None);
    let (mut graph, origins) = native_graph(None, None, false, None);
    let receipts = bind(&fixture, &mut graph, &origins, &[]);
    assert!(!receipts.iter().any(|proof| proof.target == "consumer-stdc"));
    assert!(graph.meta_targets["consumer-stdc"].contains("linklibs-stdc.static"));
}

#[test]
fn missing_ambiguous_or_rejected_providers_remain_required() {
    for (omit, ambiguous, reject) in [
        (Some("stdc.static"), None, false),
        (None, Some("stdc.static"), false),
        (None, None, true),
    ] {
        let fixture = make_fixture("", None, None);
        let (mut graph, origins) = native_graph(omit, ambiguous, false, None);
        let diagnostics = if reject {
            vec![Diagnostic::error(
                DiagnosticCode::GraphValidation,
                DiagnosticStage::GraphValidation,
                "fixture provider rejected",
            )
            .with_context(DiagnosticContext {
                target: Some("linklibs-stdc-static".into()),
                ..DiagnosticContext::default()
            })]
        } else {
            Vec::new()
        };
        let receipts = bind(&fixture, &mut graph, &origins, &diagnostics);
        assert!(!receipts.iter().any(|proof| proof.target == "consumer-stdc"));
        assert!(graph.meta_targets["consumer-stdc"].contains("linklibs-stdc.static"));
    }
}

#[test]
fn typed_provider_name_cannot_override_a_different_source_libname() {
    let fixture = make_fixture("", None, Some(("linklibs-stdc-static", "other.name")));
    let (mut graph, origins) = native_graph(None, None, false, None);
    let receipts = bind(&fixture, &mut graph, &origins, &[]);
    assert!(!receipts.iter().any(|proof| proof.target == "consumer-stdc"));
    assert!(graph.meta_targets["consumer-stdc"].contains("linklibs-stdc.static"));
}

#[test]
fn forged_provider_origin_cannot_borrow_the_typed_provider_identity() {
    for tamper in ["source-path", "libname"] {
        let mut fixture = make_fixture("", None, None);
        let provider = "linklibs-stdc-static";
        let declaration = fixture
            .projection
            .graph
            .declarations()
            .iter()
            .find(|declaration| {
                declaration.target == provider
                    && declaration.claims_make_owner
                    && declaration.dependencies.iter().any(|edge| {
                        edge.raw_expression == "includes-generate-deps"
                            && edge.concrete == "includes-generate-deps"
                    })
            })
            .unwrap();
        let key = (declaration.file.clone(), declaration.expanded_line);
        let origin = fixture.projection.expansion_origins.get_mut(&key).unwrap();
        let [frame] = origin.macro_stack.as_mut_slice() else {
            panic!("provider producer must have one sealed macro frame");
        };
        match tamper {
            "source-path" => {
                origin.source_path = Some(
                    fixture
                        .root
                        .path()
                        .join("fixture/providers/linklibs-stdc-static/mmakefile.src"),
                );
            }
            "libname" => {
                frame
                    .resolved_arguments
                    .insert("libname".into(), "forged.static".into());
            }
            _ => unreachable!(),
        }

        let (mut graph, origins) = native_graph(None, None, false, None);
        let receipts = bind(&fixture, &mut graph, &origins, &[]);
        assert!(!receipts.iter().any(|proof| proof.target == "consumer-stdc"));
        assert!(graph.meta_targets["consumer-stdc"].contains("linklibs-stdc.static"));
    }
}

#[test]
fn changed_module_template_is_not_admitted_by_the_reviewed_hash() {
    let fixture = make_fixture("", None, None);
    let path = fixture.root.path().join(TEMPLATE);
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        original.replace("core-linklibs", "different-linklibs"),
    )
    .unwrap();
    let (mut graph, origins) = native_graph(None, None, false, None);
    let result = fixture.projection.bind_library_aliases(
        fixture.root.path(),
        &mut graph,
        NativeMetaSemanticsSelection {
            context: &fixture.context,
            roots: &["requested".into()],
            declarations: &[],
            diagnostics: &[],
            meta_edge_origins: &origins,
        },
    );
    assert!(result.is_err());
}
