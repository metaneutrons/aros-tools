use super::*;
use crate::make_vars::collect_vars;
use crate::testing::TempTree;
use std::path::{Path, PathBuf};

const CONFIG: &str = "AROS_DIR_DEVELOPER := Developer\n\
AROS_DIR_INCLUDE := include\n\
AROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\n\
AROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\n\
AROS_INCLUDES := $(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)\n\
GENINCDIR := $(GENDIR)/include\n";

struct Fixture {
    tree: TempTree,
    relative_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tree = TempTree::new();
        let relative_dir = PathBuf::from("components/sample/include");
        fs::create_dir_all(tree.0.join("config")).unwrap();
        fs::write(tree.0.join("config/make.cfg.in"), CONFIG).unwrap();
        fs::create_dir_all(tree.0.join(&relative_dir)).unwrap();
        Self { tree, relative_dir }
    }

    fn file(&self, name: &str, bytes: &[u8]) {
        let path = self.tree.0.join(&self.relative_dir).join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn scan(
        &self,
        content: &str,
        states: Option<&[ConditionalTruth]>,
    ) -> (Vec<HeaderTransformDecl>, Vec<Rejection>) {
        let scope = collect_vars(content);
        let dirs = DirVars::load(&self.tree.0);
        collect(
            content,
            &scope,
            &dirs,
            &self.tree.0,
            &self.relative_dir,
            states,
        )
    }
}

fn source_rule(root: &str, source: &str) -> String {
    format!(
        "DEST_INCLUDES := $(foreach f,$(INCLUDES),{root}/$(f))\n\
         sample-includes-copy : $(DEST_INCLUDES)\n\
         $(DEST_INCLUDES) : {root}/% : {source}\n\
         \t@$(CP) $< $@\n"
    )
}

fn sdk_rule(files: &str) -> String {
    format!(
        "INCLUDES := {files}\n{}",
        source_rule("$(AROS_INCLUDES)", "$(SRCDIR)/$(CURDIR)/%")
    )
}

fn assert_rejected(fixture: &Fixture, content: &str, reason: &str) {
    let (declarations, rejections) = fixture.scan(content, None);
    assert!(
        declarations.is_empty(),
        "unexpected declarations: {declarations:#?}"
    );
    assert!(
        rejections
            .iter()
            .any(|rejection| rejection.reason.contains(reason)),
        "expected rejection containing {reason:?}; got {rejections:#?}"
    );
}

fn assert_ignored(fixture: &Fixture, content: &str) {
    let (declarations, rejections) = fixture.scan(content, None);
    assert!(
        declarations.is_empty() && rejections.is_empty(),
        "noncandidate unexpectedly entered the local-header capability: declarations={declarations:#?}, rejections={rejections:#?}"
    );
}

#[test]
fn wildcard_static_pattern_copies_nested_binary_headers_to_one_sdk_root() {
    let fixture = Fixture::new();
    fixture.file("socket.h", b"SOCKET\r\n");
    fixture.file("arpa/inet.h", b"inet\0\xff\n");
    fixture.file("netinet/in.h", b"nested source header\n");
    let content = format!(
        "INCLUDES := $(call WILDCARD, *.h arpa/*.h netinet/*.h)\n{}",
        source_rule("$(AROS_INCLUDES)", "$(SRCDIR)/$(CURDIR)/%")
    );

    let (declarations, rejections) = fixture.scan(&content, None);
    assert!(rejections.is_empty(), "{rejections:#?}");
    assert_eq!(declarations.len(), 3, "{declarations:#?}");
    let nested = declarations
        .iter()
        .find(|declaration| declaration.output.ends_with("/arpa/inet.h"))
        .expect("nested declaration");
    assert_eq!(nested.name, "sample-includes-copy");
    assert_eq!(nested.file, "components/sample/include/mmakefile.src");
    assert!(nested.line > 0);
    assert_eq!(
        nested.input,
        "${AROS_SOURCE_DIR}/components/sample/include/arpa/inet.h"
    );
    assert_eq!(nested.output, "${AROS_SDK_INCLUDE_DIR}/arpa/inet.h");
    assert!(nested.copy_only);
    assert!(!nested.replace_whole_line_containing);
    assert!(nested.match_text.is_empty() && nested.replacement.is_empty());
    assert!(nested.substitutions.is_empty());
    assert!(nested.dependencies.is_empty() && nested.consumers.is_empty());
}

#[test]
fn generated_include_root_is_normalized_without_a_second_mirror() {
    let fixture = Fixture::new();
    fixture.file("sys/types.h", b"types\n");
    let content = format!(
        "INCLUDES := sys/types.h\n{}",
        source_rule("$(GENINCDIR)", "$(SRCDIR)/$(CURDIR)/%")
    );
    let (declarations, rejections) = fixture.scan(&content, None);
    assert!(rejections.is_empty(), "{rejections:#?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(
        declarations[0].input,
        "${AROS_SOURCE_DIR}/components/sample/include/sys/types.h"
    );
    assert_eq!(declarations[0].output, "${AROS_GENINC_DIR}/sys/types.h");
}

fn thunderbolt_rules() -> &'static str {
    "includes-copy : $(AROS_INCLUDES)/hidd/thunderbolt.h $(GENINCDIR)/hidd/thunderbolt.h\n\
     $(AROS_INCLUDES)/hidd/thunderbolt.h: include/thunderbolt_hidd.h\n\
     \t$(CP) $< $(AROS_INCLUDES)/hidd/thunderbolt.h\n\
     $(GENINCDIR)/hidd/thunderbolt.h: include/thunderbolt_hidd.h\n\
     \t$(CP) $< $(GENINCDIR)/hidd/thunderbolt.h\n"
}

#[test]
fn explicit_literal_rules_copy_one_source_to_sdk_and_generated_roots() {
    let fixture = Fixture::new();
    fixture.file("include/thunderbolt_hidd.h", b"thunderbolt\0header\n");
    let (declarations, rejections) = fixture.scan(thunderbolt_rules(), None);
    assert!(rejections.is_empty(), "{rejections:#?}");
    assert_eq!(declarations.len(), 2, "{declarations:#?}");
    let sdk = declarations
        .iter()
        .find(|declaration| declaration.output == "${AROS_SDK_INCLUDE_DIR}/hidd/thunderbolt.h")
        .expect("SDK header declaration");
    let generated = declarations
        .iter()
        .find(|declaration| declaration.output == "${AROS_GENINC_DIR}/hidd/thunderbolt.h")
        .expect("generated header declaration");
    assert_eq!(sdk.name, "includes-copy");
    assert_eq!(generated.name, "includes-copy");
    assert_eq!(sdk.file, "components/sample/include/mmakefile.src");
    assert_eq!(
        sdk.input,
        "${AROS_SOURCE_DIR}/components/sample/include/include/thunderbolt_hidd.h"
    );
    assert_eq!(generated.input, sdk.input);
    assert!(sdk.copy_only && generated.copy_only);
}

#[test]
fn implicit_literal_pattern_is_instantiated_only_by_its_finite_owner() {
    let fixture = Fixture::new();
    fixture.file("c_iff.h", b"IFF header\n");
    let content = "includes-copy : $(AROS_INCLUDES)/c_iff.h\n\
                   $(AROS_INCLUDES)/%.h : %.h\n\
                   \t$(CP) $< $(AROS_INCLUDES)\n";
    let (declarations, rejections) = fixture.scan(content, None);
    assert!(rejections.is_empty(), "{rejections:#?}");
    assert_eq!(declarations.len(), 1, "{declarations:#?}");
    assert_eq!(declarations[0].name, "includes-copy");
    assert_eq!(
        declarations[0].input,
        "${AROS_SOURCE_DIR}/components/sample/include/c_iff.h"
    );
    assert_eq!(declarations[0].output, "${AROS_SDK_INCLUDE_DIR}/c_iff.h");
    assert!(declarations[0].copy_only);
}

#[test]
fn ordinary_header_rules_reject_order_only_extra_commands_and_owner_ambiguity() {
    let fixture = Fixture::new();
    fixture.file("include/thunderbolt_hidd.h", b"header\n");
    let order_only = thunderbolt_rules().replace(
        "include/thunderbolt_hidd.h\n",
        "include/thunderbolt_hidd.h | setup\n",
    );
    assert_rejected(&fixture, &order_only, "order-only prerequisites");

    let extra_command = thunderbolt_rules().replace(
        "$(CP) $< $(AROS_INCLUDES)/hidd/thunderbolt.h\n",
        "$(CP) $< $(AROS_INCLUDES)/hidd/thunderbolt.h\n\t@echo unsafe\n",
    );
    assert_rejected(&fixture, &extra_command, "exactly one copy recipe command");

    let ambiguous_owner = format!(
        "{}other-includes-copy : $(AROS_INCLUDES)/hidd/thunderbolt.h\n",
        thunderbolt_rules()
    );
    assert_rejected(&fixture, &ambiguous_owner, "multiple named owners");

    let duplicate_target = format!(
        "{}$(AROS_INCLUDES)/hidd/thunderbolt.h: include/thunderbolt_hidd.h\n\
         \t$(CP) $< $(AROS_INCLUDES)/hidd/thunderbolt.h\n",
        thunderbolt_rules()
    );
    assert_rejected(&fixture, &duplicate_target, "has 2 active target rules");

    let target_specific = format!(
        "{}$(AROS_INCLUDES)/hidd/thunderbolt.h: CP := unsafe\n",
        thunderbolt_rules()
    );
    assert_rejected(
        &fixture,
        &target_specific,
        "source reassigns or target-specifies `CP`",
    );

    let conditional = format!("ifeq ($(UNKNOWN),1)\n{}endif\n", thunderbolt_rules());
    assert_rejected(&fixture, &conditional, "unconditional");

    let redirected = format!("AROS_INCLUDES := elsewhere\n{}", thunderbolt_rules());
    assert_rejected(&fixture, &redirected, "differs from its configured mapping");
}

#[cfg(unix)]
#[test]
fn explicit_literal_copy_rejects_symlinked_input() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    fixture.file("include/real_header.h", b"real\n");
    symlink(
        fixture
            .tree
            .0
            .join(&fixture.relative_dir)
            .join("include/real_header.h"),
        fixture
            .tree
            .0
            .join(&fixture.relative_dir)
            .join("include/thunderbolt_hidd.h"),
    )
    .unwrap();
    assert_rejected(&fixture, thunderbolt_rules(), "crosses a symlink");
}

#[test]
fn implicit_header_rule_rejects_order_only_controls_and_nonroot_copy() {
    let fixture = Fixture::new();
    fixture.file("c_iff.h", b"IFF header\n");
    let order_only = "includes-copy : $(AROS_INCLUDES)/c_iff.h\n\
                      $(AROS_INCLUDES)/%.h : %.h | setup\n\
                      \t$(CP) $< $(AROS_INCLUDES)\n";
    assert_rejected(
        &fixture,
        order_only,
        "must be exactly `$(AROS_INCLUDES)/%.h : %.h`",
    );
    let wrong_destination = "includes-copy : $(AROS_INCLUDES)/c_iff.h\n\
                             $(AROS_INCLUDES)/%.h : %.h\n\
                             \t$(CP) $< $(GENDIR)/include\n";
    assert_rejected(&fixture, wrong_destination, "recipe must be exactly");
    let extra_owner_prerequisite = "includes-copy : $(AROS_INCLUDES)/c_iff.h setup\n\
                                   $(AROS_INCLUDES)/%.h : %.h\n\
                                   \t$(CP) $< $(AROS_INCLUDES)\n";
    assert_rejected(
        &fixture,
        extra_owner_prerequisite,
        "not a literal include path",
    );
}

#[test]
fn generators_and_fetched_config_headers_are_not_local_copy_candidates() {
    let fixture = Fixture::new();
    let cases = [
        "includes-copy : $(AROS_INCLUDES)/clib/cia_protos.h\n\
         $(AROS_INCLUDES)/clib/cia_protos.h: cia_lib.sfd\n\
         \t$(SFDC) --mode=clib --target=x-aros --output=$@ $<\n",
        "includes-copy : $(AROS_INCLUDES)/pnglibconf.h\n\
         $(AROS_INCLUDES)/pnglibconf.h: $(ARCHSRCDIR)/scripts/pnglibconf.h.prebuilt\n\
         \t$(SED) -e 's/old/new/' $< > $@\n",
        "includes-copy : $(AROS_INCLUDES)/zconf.h\n\
         $(AROS_INCLUDES)/zconf.h: $(ARCHSRCDIR)/zconf.h.chr\n\
         \t$(SED) -e 's/old/new/' $< > $@\n",
        "includes-copy : $(AROS_INCLUDES)/aros/i386/libcall.h $(GENINCDIR)/aros/i386/libcall.h\n\
         $(AROS_INCLUDES)/aros/i386/libcall.h: $(HOSTGENDIR)/tools/gencall_i386 | $(AROS_INCLUDES)/aros/i386\n\
         \t$(HOSTGENDIR)/tools/gencall_i386 > $@\n\
         $(GENINCDIR)/aros/i386/libcall.h: $(AROS_INCLUDES)/aros/i386/libcall.h | $(GENINCDIR)/aros/i386\n\
         \t$(CP) $< $@\n",
    ];
    for content in cases {
        assert_ignored(&fixture, content);
    }
}

#[test]
fn define_bodies_do_not_create_header_copy_endpoints_even_with_long_modifiers() {
    let fixture = Fixture::new();
    let content = "define override export private STORED_RULES\n\
                   $(AROS_INCLUDES)/fake.h: fake.h\n\
                   \t$(CP) $< $(AROS_INCLUDES)/fake.h\n\
                   DEST_INCLUDES := $(AROS_INCLUDES)/fake-static.h\n\
                   fake-static-copy : $(DEST_INCLUDES)\n\
                   $(DEST_INCLUDES) : $(AROS_INCLUDES)/% : $(SRCDIR)/$(CURDIR)/%\n\
                   \t@$(CP) $< $@\n\
                   ifeq ($(UNKNOWN),1)\n\
                   endef\n\
                   override export private override define MALFORMED\n\
                   $(AROS_INCLUDES)/also-fake.h: also-fake.h\n\
                   \t$(CP) $< $(AROS_INCLUDES)/also-fake.h\n\
                   endef\n";
    assert_ignored(&fixture, content);
}

#[test]
fn source_local_copy_assignments_cannot_shadow_the_host_copy_command() {
    let fixture = Fixture::new();
    fixture.file("api.h", b"api\n");
    let local_override = format!(
        "CP := true\nINCLUDES := api.h\n{}",
        source_rule("$(AROS_INCLUDES)", "$(SRCDIR)/$(CURDIR)/%")
    );
    assert_rejected(
        &fixture,
        &local_override,
        "source reassigns or target-specifies `CP`",
    );

    let static_override = format!("CP := true\n{}", sdk_rule("api.h"));
    assert_rejected(
        &fixture,
        &static_override,
        "source reassigns or target-specifies `CP`",
    );

    let target_specific = format!(
        "{}$(AROS_INCLUDES)/hidd/thunderbolt.h: CP := true\n",
        thunderbolt_rules()
    );
    fixture.file("include/thunderbolt_hidd.h", b"header\n");
    assert_rejected(
        &fixture,
        &target_specific,
        "source reassigns or target-specifies `CP`",
    );
}

#[test]
fn ordinary_local_header_traversal_remains_a_fatal_candidate() {
    let fixture = Fixture::new();
    let content = "includes-copy : $(AROS_INCLUDES)/bad.h\n\
                   $(AROS_INCLUDES)/bad.h: ../escape.h\n\
                   \t$(CP) $< $(AROS_INCLUDES)/bad.h\n";
    assert_rejected(&fixture, content, "not one safe source-relative header");
}

#[test]
fn actual_thunderbolt_and_c_iff_rules_are_admitted_when_source_root_is_supplied() {
    let Ok(source_root) = std::env::var("AROS_TEST_SOURCE_ROOT") else {
        return;
    };
    let source_root = PathBuf::from(source_root);
    let dirs = DirVars::load(&source_root);
    for (relative, expected) in [
        (
            Path::new("rom/hidds/thunderbolt"),
            vec![
                "${AROS_SDK_INCLUDE_DIR}/hidd/thunderbolt.h",
                "${AROS_GENINC_DIR}/hidd/thunderbolt.h",
            ],
        ),
        (
            Path::new("tools/dtdesc/c_iff"),
            vec!["${AROS_SDK_INCLUDE_DIR}/c_iff.h"],
        ),
    ] {
        let makefile = source_root.join(relative).join("mmakefile.src");
        let content = fs::read_to_string(&makefile).unwrap();
        let scope = collect_vars(&content);
        let (declarations, rejections) =
            collect(&content, &scope, &dirs, &source_root, relative, None);
        assert!(
            rejections.is_empty(),
            "{}: {rejections:#?}",
            makefile.display()
        );
        let mut actual = declarations
            .iter()
            .map(|declaration| declaration.output.as_str())
            .collect::<Vec<_>>();
        actual.sort_unstable();
        let mut expected = expected;
        expected.sort_unstable();
        assert_eq!(actual, expected, "{}", makefile.display());
    }
}

#[test]
fn source_root_declaring_directory_normalizes_only_its_trailing_separator() {
    let fixture = Fixture::new();
    fs::write(fixture.tree.0.join("root.h"), b"root header\n").unwrap();
    let content = sdk_rule("root.h");
    let scope = collect_vars(&content);
    let dirs = DirVars::load(&fixture.tree.0);
    let (declarations, rejections) = collect(
        &content,
        &scope,
        &dirs,
        &fixture.tree.0,
        Path::new(""),
        None,
    );
    assert!(rejections.is_empty(), "{rejections:#?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].file, "mmakefile.src");
    assert_eq!(declarations[0].input, "${AROS_SOURCE_DIR}/root.h");
    assert_eq!(declarations[0].output, "${AROS_SDK_INCLUDE_DIR}/root.h");
}

#[test]
fn rejects_parent_traversal_and_casefold_duplicate_outputs() {
    let fixture = Fixture::new();
    fixture.file("escape.h", b"not actually selected\n");
    assert_rejected(
        &fixture,
        &sdk_rule("../escape.h"),
        "not a safe relative .h path",
    );

    fixture.file("Public.h", b"upper\n");
    fixture.file("public.h", b"lower\n");
    assert_rejected(&fixture, &sdk_rule("Public.h public.h"), "duplicated");
}

#[test]
fn rejects_local_include_root_overrides_even_when_both_expressions_agree() {
    let fixture = Fixture::new();
    fixture.file("api.h", b"api\n");
    for variable in ["AROS_DEVELOPER", "AROS_DIR_INCLUDE"] {
        let content = format!("{variable} := redirected\n{}", sdk_rule("api.h"));
        assert_rejected(&fixture, &content, "differs from its configured mapping");
    }
    let content = format!(
        "GENDIR := redirected\nINCLUDES := api.h\n{}",
        source_rule("$(GENINCDIR)", "$(SRCDIR)/$(CURDIR)/%")
    );
    assert_rejected(&fixture, &content, "differs from its configured mapping");
}

#[test]
fn rejects_noncanonical_source_mapping_and_owner_prerequisites() {
    let fixture = Fixture::new();
    fixture.file("api.h", b"api\n");
    let wrong_source = format!(
        "INCLUDES := api.h\n{}",
        source_rule("$(AROS_INCLUDES)", "$(PORTSDIR)/foreign/%")
    );
    assert_rejected(&fixture, &wrong_source, "not `$(SRCDIR)/$(CURDIR)/%`");

    let extra_prerequisite = "INCLUDES := api.h\n\
         DEST_INCLUDES := $(foreach f,$(INCLUDES),$(AROS_INCLUDES)/$(f))\n\
         sample-includes-copy : $(DEST_INCLUDES) setup\n\
         $(DEST_INCLUDES) : $(AROS_INCLUDES)/% : $(SRCDIR)/$(CURDIR)/%\n\
         \t@$(CP) $< $@\n";
    assert_rejected(&fixture, extra_prerequisite, "extra prerequisites");
}

#[test]
fn rejects_orphan_ambiguous_and_recipe_bearing_owners() {
    let fixture = Fixture::new();
    fixture.file("api.h", b"api\n");
    let orphan = "INCLUDES := api.h\nDEST_INCLUDES := $(foreach f,$(INCLUDES),$(AROS_INCLUDES)/$(f))\n$(DEST_INCLUDES) : $(AROS_INCLUDES)/% : $(SRCDIR)/$(CURDIR)/%\n\t@$(CP) $< $@\n";
    assert_rejected(&fixture, orphan, "orphan");

    let ambiguous = format!(
        "INCLUDES := api.h\n{}\nsample-includes-copy-alt : $(DEST_INCLUDES)\n",
        source_rule("$(AROS_INCLUDES)", "$(SRCDIR)/$(CURDIR)/%")
    );
    assert_rejected(&fixture, &ambiguous, "multiple named owners");

    let owner_recipe = "INCLUDES := api.h\nDEST_INCLUDES := $(foreach f,$(INCLUDES),$(AROS_INCLUDES)/$(f))\nsample-includes-copy : $(DEST_INCLUDES)\n\t@echo unsafe\n$(DEST_INCLUDES) : $(AROS_INCLUDES)/% : $(SRCDIR)/$(CURDIR)/%\n\t@$(CP) $< $@\n";
    assert_rejected(&fixture, owner_recipe, "must not have recipe commands");

    let extra_copy_recipe = "INCLUDES := api.h\nDEST_INCLUDES := $(foreach f,$(INCLUDES),$(AROS_INCLUDES)/$(f))\nsample-includes-copy : $(DEST_INCLUDES)\n$(DEST_INCLUDES) : $(AROS_INCLUDES)/% : $(SRCDIR)/$(CURDIR)/%\n\t@$(CP) $< $@\n\t@echo unsafe\n";
    assert_rejected(
        &fixture,
        extra_copy_recipe,
        "exactly one copy recipe command",
    );
}

#[test]
fn rejects_unknown_guarded_candidate_and_unsafe_pattern_mapping() {
    let fixture = Fixture::new();
    fixture.file("api.h", b"api\n");
    let guarded = "INCLUDES := api.h\nDEST_INCLUDES := $(foreach f,$(INCLUDES),$(AROS_INCLUDES)/$(f))\nifeq ($(UNKNOWN),1)\nsample-includes-copy : $(DEST_INCLUDES)\n$(DEST_INCLUDES) : $(AROS_INCLUDES)/% : $(SRCDIR)/$(CURDIR)/%\n\t@$(CP) $< $@\nendif\n";
    let mut states = vec![ConditionalTruth::True; guarded.lines().count()];
    for (index, line) in guarded.lines().enumerate() {
        if line.starts_with("sample-includes-copy")
            || line.starts_with("$(DEST_INCLUDES) :")
            || line.starts_with('\t')
        {
            states[index] = ConditionalTruth::Unknown;
        }
    }
    assert_rejected(
        &fixture,
        guarded,
        "guarded by an unresolved Make conditional",
    );
    let scope = collect_vars(guarded);
    let dirs = DirVars::load(&fixture.tree.0);
    let (declarations, rejections) = collect(
        guarded,
        &scope,
        &dirs,
        &fixture.tree.0,
        &fixture.relative_dir,
        Some(&states),
    );
    assert!(declarations.is_empty());
    assert!(rejections.iter().any(|rejection| {
        rejection
            .reason
            .contains("guarded by an unresolved Make conditional")
    }));

    let wrong_include = "INCLUDES := api.h\nDEST_INCLUDES := $(foreach f,$(INCLUDES),$(AROS_INCLUDES)/$(f))\nsample-includes-copy : $(DEST_INCLUDES)\n$(DEST_INCLUDES) : $(AROS_INCLUDES)/subdir/% : $(SRCDIR)/$(CURDIR)/%\n\t@$(CP) $< $@\n";
    assert_rejected(
        &fixture,
        wrong_include,
        "must be exactly one known include root",
    );

    let unknown_include = "INCLUDES := api.h\nDEST_INCLUDES := $(foreach f,$(INCLUDES),$(GENDIR)/foreign/$(f))\nsample-includes-copy : $(DEST_INCLUDES)\n$(DEST_INCLUDES) : $(GENDIR)/foreign/% : $(SRCDIR)/$(CURDIR)/%\n\t@$(CP) $< $@\n";
    assert_rejected(
        &fixture,
        unknown_include,
        "must be exactly one known include root",
    );
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_header_source() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    fixture.file("real.h", b"real\n");
    symlink(
        fixture.tree.0.join(&fixture.relative_dir).join("real.h"),
        fixture.tree.0.join(&fixture.relative_dir).join("linked.h"),
    )
    .unwrap();
    assert_rejected(&fixture, &sdk_rule("linked.h"), "crosses a symlink");
}

#[test]
fn rejects_output_lists_over_the_fixed_capacity() {
    let fixture = Fixture::new();
    let repeated = std::iter::repeat_n("same.h", MAX_OUTPUTS + 1)
        .collect::<Vec<_>>()
        .join(" ");
    assert_rejected(
        &fixture,
        &sdk_rule(&repeated),
        "target list exceeds 4096 files",
    );
}
