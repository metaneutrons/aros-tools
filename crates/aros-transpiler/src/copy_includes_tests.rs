use super::*;
use std::path::PathBuf;

#[test]
fn wildcard_with_dir_flattens_to_basenames() {
    // rom/hidds/kbd/mmakefile.src
    let src =
        "INCLUDE_FILES := $(call WILDCARD, include/*.h)\n%copy_includes path=hidd dir=include\n";
    let CopyIncludesScan { decls, skipped, .. } =
        collect_copy_includes(src, &PathBuf::from("rom/hidds/kbd"));
    assert!(skipped.is_empty());
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].dest, "hidd");
    assert_eq!(decls[0].source_dir, "rom/hidds/kbd/include");
    assert_eq!(decls[0].patterns, vec!["*.h"]);
    assert!(decls[0].flatten);
}

#[test]
fn explicit_name_without_dir_keeps_relative_path() {
    // workbench/classes/zune/iconimage/mmakefile.src
    let src = "INCLUDE_FILES := iconimage.h\n%copy_includes path=zune\n";
    let CopyIncludesScan { decls, .. } =
        collect_copy_includes(src, &PathBuf::from("workbench/classes/zune/iconimage"));
    assert_eq!(decls[0].dest, "zune");
    assert_eq!(decls[0].source_dir, "workbench/classes/zune/iconimage");
    assert_eq!(decls[0].patterns, vec!["iconimage.h"]);
    assert!(!decls[0].flatten);
}

#[test]
fn explicit_dot_destination_reaches_cmake_as_the_include_root() {
    let src = "INCLUDE_FILES := zlib.h\n%copy_includes mmake=z-includes path=. dir=include\n";
    let CopyIncludesScan { decls, skipped, .. } =
        collect_copy_includes(src, &PathBuf::from("workbench/libs/z"));
    assert!(skipped.is_empty());
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].name, "z-includes");
    assert_eq!(decls[0].dest, ".");
    assert_eq!(decls[0].patterns, ["zlib.h"]);
}

#[test]
fn oop_case_maps_to_sdk_prefix() {
    // rom/oop/mmakefile.src, the header that blocked 49 compilations.
    let src =
        "INCLUDE_FILES := $(call WILDCARD, include/*.h)\n%copy_includes path=oop dir=include\n";
    let CopyIncludesScan { decls, .. } = collect_copy_includes(src, &PathBuf::from("rom/oop"));
    assert_eq!(decls[0].dest, "oop");
    assert_eq!(decls[0].source_dir, "rom/oop/include");
}

#[test]
fn expands_character_classes_for_cmake_glob() {
    let src = "INCLUDE_FILES := $(call WILDCARD, *.[hi] aros/*.[hi])\n%copy_includes path=x\n";
    let CopyIncludesScan { decls, .. } = collect_copy_includes(src, &PathBuf::from("d"));
    let p = &decls[0].patterns;
    assert!(p.contains(&"*.h".to_owned()));
    assert!(p.contains(&"*.i".to_owned()));
    assert!(p.contains(&"aros/*.h".to_owned()));
    assert!(p.contains(&"aros/*.i".to_owned()));
}

#[test]
fn inline_include_list_is_honoured() {
    let src = "%copy_includes path=libraries includes=\"speechcore.h aros_resource.h\"\n";
    let CopyIncludesScan { decls, .. } = collect_copy_includes(src, &PathBuf::from("d"));
    assert_eq!(decls[0].patterns, vec!["speechcore.h", "aros_resource.h"]);
}

#[test]
fn unresolvable_third_party_dir_is_skipped_not_guessed() {
    let src = "INCLUDE_FILES := $(call WILDCARD, *.h)\n%copy_includes path=freetype dir=$(FT2SRCDIR)/include\n";
    let CopyIncludesScan { decls, skipped, .. } = collect_copy_includes(src, &PathBuf::from("d"));
    assert!(decls.is_empty());
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].contains("FT2SRCDIR"));
}

#[test]
fn unresolvable_include_list_is_skipped() {
    let src = "%copy_includes path=x dir=include\n";
    let CopyIncludesScan { decls, skipped, .. } = collect_copy_includes(src, &PathBuf::from("d"));
    assert!(
        decls.is_empty(),
        "no INCLUDE_FILES defined, nothing to copy"
    );
    assert_eq!(skipped.len(), 1);
}

#[test]
fn parent_relative_dir_resolves() {
    let src = "INCLUDE_FILES := ../include/mui/BetterString_mcc.h\n%copy_includes path=mui dir=../include/mui\n";
    let CopyIncludesScan { decls, .. } =
        collect_copy_includes(src, &PathBuf::from("workbench/classes/zune/betterstring"));
    assert_eq!(decls[0].source_dir, "workbench/classes/zune/include/mui");
    assert_eq!(decls[0].patterns, vec!["BetterString_mcc.h"]);
}

#[test]
fn reassigned_include_files_uses_value_in_force_at_the_directive() {
    // arch/all-native/acpica/mmakefile.src reassigns INCLUDE_FILES between
    // the two directives; Make resolves each against the current value.
    let src = "\
INCLUDE_FILES := $(call WILDCARD, include/*.h)
%copy_includes path=libraries dir=include
INCLUDE_FILES = $(call WILDCARD, $(ACPICA_INCLUDES)/*.h)
%copy_includes mmake=acpica-includes-copy path=acpica dir=$(ACPICA_INCLUDES)
";
    let CopyIncludesScan { decls, skipped, .. } =
        collect_copy_includes(src, &PathBuf::from("arch/all-native/acpica"));
    // The first directive must resolve; the second is out-of-tree.
    assert_eq!(decls.len(), 1, "decls: {decls:?}");
    assert_eq!(decls[0].dest, "libraries");
    assert_eq!(decls[0].source_dir, "arch/all-native/acpica/include");
    assert_eq!(decls[0].patterns, vec!["*.h"]);
    assert_eq!(skipped.len(), 1);
}

#[test]
fn nested_parens_in_wildcard_do_not_truncate_the_glob() {
    // `$(call WILDCARD, $(X)/*.h)` must not be cut at the inner ")".
    let src = "INCLUDE_FILES := $(call WILDCARD, $(SOMEDIR)/*.h)\n%copy_includes path=x\n";
    let CopyIncludesScan { decls, skipped, .. } = collect_copy_includes(src, &PathBuf::from("d"));
    assert!(
        decls.is_empty(),
        "unresolved variable must not yield a pattern"
    );
    assert_eq!(skipped.len(), 1);
    assert!(
        !skipped[0].contains("*.h)"),
        "glob was truncated: {}",
        skipped[0]
    );
}

#[test]
fn detects_hand_written_staging_rules() {
    // rom/hidds/pci/mmakefile.src stages a renamed header with a plain rule.
    let src = "\
$(AROS_INCLUDES)/hidd/pci.h: include/pci_hidd.h
\t$(CP) $< $(AROS_INCLUDES)/hidd/pci.h

$(GENINCDIR)/hidd/pci.h: include/pci_hidd.h
\t$(CP) $< $(GENINCDIR)/hidd/pci.h
";
    let CopyIncludesScan { adhoc, .. } =
        collect_copy_includes(src, &PathBuf::from("rom/hidds/pci"));
    assert_eq!(adhoc.len(), 2, "adhoc: {adhoc:?}");
    assert_eq!(adhoc[0].file, "rom/hidds/pci/mmakefile.src");
    assert_eq!(adhoc[0].line, 1);
    assert_eq!(adhoc[0].dest, "hidd/pci.h");
    assert_eq!(adhoc[0].prereqs, "include/pci_hidd.h");
    assert_eq!(adhoc[1].dest, "hidd/pci.h");
}

#[test]
fn promotes_a_literal_anchored_sed_header_to_a_real_transform() {
    let src = r"
ARCHSRCDIR := $(PORTSDIR)/zlib/zlib
z-geninc : $(AROS_INCLUDES)/zconf.h

$(AROS_INCLUDES)/zconf.h : $(ARCHSRCDIR)/zconf.h.chr
	@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi
	@$(SED) -e 's/^#if !defined(CHROMIUM_ZLIB_NO_CHROMECONF)/#if defined(ZLIB_USE_CHROMECONF)/' \
	    $< > $@
";
    let CopyIncludesScan {
        transforms, adhoc, ..
    } = collect_copy_includes(src, &PathBuf::from("workbench/libs/z"));
    assert!(adhoc.is_empty(), "promoted rule stayed adhoc: {adhoc:?}");
    assert_eq!(transforms.len(), 1, "transforms: {transforms:?}");
    let transform = &transforms[0];
    assert_eq!(transform.name, "z-geninc");
    assert_eq!(transform.input, "${AROS_PORTS_DIR}/zlib/zlib/zconf.h.chr");
    assert_eq!(transform.output, "${AROS_SDK_INCLUDE_DIR}/zconf.h");
    assert_eq!(
        transform.match_text,
        "#if !defined(CHROMIUM_ZLIB_NO_CHROMECONF)"
    );
    assert_eq!(transform.replacement, "#if defined(ZLIB_USE_CHROMECONF)");
    assert!(!transform.copy_only);
}

#[test]
fn promotes_an_exact_header_copy_with_safe_prelude() {
    let src = r#"
webp-generated : $(GENDIR)/$(CURDIR)/src/webp/config.h

$(GENDIR)/$(CURDIR)/src/webp/config.h : $(SRCDIR)/$(CURDIR)/config.h $(SRCDIR)/$(CURDIR)/mmakefile.src
	@$(ECHO) "Generating src/webp/config.h ..."
	%mkdir_q dir=$(GENDIR)/$(CURDIR)/src/webp
	@$(CP) $< $@
"#;
    let CopyIncludesScan {
        transforms, adhoc, ..
    } = collect_copy_includes(src, &PathBuf::from("workbench/classes/datatypes/webp"));
    assert!(adhoc.is_empty(), "promoted copy stayed adhoc: {adhoc:?}");
    assert_eq!(transforms.len(), 1, "transforms: {transforms:?}");
    let transform = &transforms[0];
    assert!(transform.copy_only);
    assert_eq!(transform.name, "webp-generated");
    assert_eq!(
        transform.input,
        "${CMAKE_SOURCE_DIR}/workbench/classes/datatypes/webp/config.h"
    );
    assert_eq!(
        transform.output,
        "${CMAKE_BINARY_DIR}/gen/workbench/classes/datatypes/webp/src/webp/config.h"
    );
}

#[test]
fn promotes_the_real_tiff_sdk_header_copies_without_recipe_echo_suppression() {
    let src = include_str!("../../../../../workbench/libs/tiff/mmakefile.src");
    let CopyIncludesScan {
        transforms, adhoc, ..
    } = collect_copy_includes(src, &PathBuf::from("workbench/libs/tiff"));

    for header in ["tiffconf.h", "tifftypes.h", "tiffinline.h"] {
        let transform = transforms
            .iter()
            .find(|transform| transform.output.ends_with(header))
            .unwrap_or_else(|| panic!("missing transform for {header}: {transforms:?}"));
        assert_eq!(transform.name, "workbench-libs-tiff-generated");
        assert!(transform.copy_only);
        assert!(transform
            .input
            .ends_with(&format!("/workbench/libs/tiff/{header}")));
    }
    assert!(
        adhoc.iter().all(
            |rule| !["tiffconf.h", "tifftypes.h", "tiffinline.h"].contains(&rule.dest.as_str())
        ),
        "TIFF copies stayed in the residual audit: {adhoc:?}"
    );
}

#[test]
fn promotes_a_copy_through_a_module_private_include_variable() {
    let src = r#"
JXLGENINCDIR := $(GENDIR)/$(CURDIR)/include
$(JXLGENINCDIR)/jxl/jxl_export.h : $(SRCDIR)/$(CURDIR)/jxl_export.h
	@$(ECHO) "Generating jxl/jxl_export.h ..."
	%mkdir_q dir=$(JXLGENINCDIR)/jxl
	@$(CP) $< $@

jxl-genfiles : $(JXLGENINCDIR)/jxl/jxl_export.h
"#;
    let CopyIncludesScan {
        transforms, adhoc, ..
    } = collect_copy_includes(src, &PathBuf::from("workbench/classes/datatypes/jpegxl"));
    assert!(adhoc.is_empty(), "promoted copy stayed adhoc: {adhoc:?}");
    assert_eq!(transforms.len(), 1, "transforms: {transforms:?}");
    assert!(transforms[0].copy_only);
    assert_eq!(transforms[0].name, "jxl-genfiles");
    assert_eq!(
        transforms[0].output,
        "${CMAKE_BINARY_DIR}/gen/workbench/classes/datatypes/jpegxl/include/jxl/jxl_export.h"
    );
}

#[test]
fn real_jpegxl_export_copy_is_promoted() {
    let src = include_str!("../../../../../workbench/classes/datatypes/jpegxl/mmakefile.src");
    let CopyIncludesScan {
        transforms, adhoc, ..
    } = collect_copy_includes(src, &PathBuf::from("workbench/classes/datatypes/jpegxl"));
    assert!(
        transforms.iter().any(|transform| {
            transform.copy_only && transform.output.ends_with("/jxl/jxl_export.h")
        }),
        "transforms: {transforms:?}; adhoc: {adhoc:?}"
    );
    let version = transforms
        .iter()
        .find(|transform| transform.output.ends_with("/jxl/version.h"))
        .expect("version template was not promoted");
    assert_eq!(
        version.substitutions,
        [
            "@JPEGXL_MAJOR_VERSION@",
            "0",
            "@JPEGXL_MINOR_VERSION@",
            "12",
            "@JPEGXL_PATCH_VERSION@",
            "0",
        ]
    );
}

#[test]
fn promotes_exact_bison_output_and_binds_its_dependency_owner() {
    let src = r"
$(OBJDIR)/evalParser.tab.c : evalParser.y
	@$(ECHO) Generating $(notdir $@) from $<...
	@$(BISON) -o $@ $<

$(workbench-c-eval_DEPS) : $(OBJDIR)/evalParser.tab.c
";
    let scan = collect_copy_includes(src, &PathBuf::from("workbench/c"));
    assert!(
        scan.generated_files.is_empty(),
        "{:?}",
        scan.generated_files
    );
    assert_eq!(scan.bison_outputs.len(), 1, "{:?}", scan.bison_outputs);
    let output = &scan.bison_outputs[0];
    assert_eq!(output.owner, "workbench-c-eval");
    assert_eq!(output.input, "${CMAKE_SOURCE_DIR}/workbench/c/evalParser.y");
    assert_eq!(
        output.output,
        "${CMAKE_BINARY_DIR}/gen/workbench/c/evalParser.tab.c"
    );
}

#[test]
fn arbitrary_sed_regex_remains_reported_as_adhoc() {
    let src = r"
x-geninc : $(AROS_INCLUDES)/x.h
$(AROS_INCLUDES)/x.h : input.h
	@$(SED) -e 's/^version=.*/version=1/' $< > $@
";
    let CopyIncludesScan {
        transforms, adhoc, ..
    } = collect_copy_includes(src, &PathBuf::from("workbench/libs/x"));
    assert!(transforms.is_empty());
    assert_eq!(adhoc.len(), 1);
    assert_eq!(adhoc[0].dest, "x.h");
}

#[test]
fn sed_transform_rejects_extra_commands_suffixes_and_special_replacements() {
    let recipes = [
        "\t@$(NOP)\n\t@$(SED) -e 's/^literal/changed/' $< > $@",
        "\t@$(SED) -e 's/^literal/changed/' $< > $@ ; $(TOUCH) marker",
        "\t@$(SED) -e 's/^literal/prefix-&/' $< > $@",
        "\t@$(SED) -e 's/^literal/changed/' $< > $@\n\t@$(NOP)",
    ];
    for recipe in recipes {
        let src = format!(
            "x-geninc : $(AROS_INCLUDES)/x.h\n\
                 $(AROS_INCLUDES)/x.h : input.h\n{recipe}\n"
        );
        let CopyIncludesScan {
            transforms, adhoc, ..
        } = collect_copy_includes(&src, &PathBuf::from("workbench/libs/x"));
        assert!(transforms.is_empty(), "unsafe recipe promoted: {recipe}");
        assert_eq!(adhoc.len(), 1, "unsafe recipe disappeared: {recipe}");
    }
}

#[test]
fn variable_assignment_is_not_mistaken_for_a_rule() {
    let src = "INCLUDE_FILES := a.h\n%copy_includes path=x\n";
    let CopyIncludesScan { decls, adhoc, .. } = collect_copy_includes(src, &PathBuf::from("d"));
    assert!(adhoc.is_empty());
    assert_eq!(decls.len(), 1);
}

#[test]
fn stages_headers_from_a_fetched_port() {
    // The real arch/all-native/acpica declaration: the source directory is
    // rooted in $(PORTSDIR), which %fetch unpacks into.
    let src = "\
ACPICAPACKAGE      := acpica
ACPICAVERSION      := 20260408
ACPICAARCHBASE     := $(ACPICAPACKAGE)-unix-$(ACPICAVERSION)
ACPICASRCDIR       := $(PORTSDIR)/acpica/$(ACPICAARCHBASE)
ACPICA_INCLUDES    := $(ACPICASRCDIR)/source/include
INCLUDE_FILES = $(call WILDCARD, $(ACPICA_INCLUDES)/*.h)
%copy_includes mmake=acpica-includes-copy path=acpica dir=$(ACPICA_INCLUDES)
";
    let CopyIncludesScan { decls, skipped, .. } =
        collect_copy_includes(src, &PathBuf::from("arch/all-native/acpica"));
    assert!(skipped.is_empty(), "skipped: {skipped:?}");
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].dest, "acpica");
    assert_eq!(
        decls[0].source_dir,
        "${AROS_PORTS_DIR}/acpica/acpica-unix-20260408/source/include"
    );
    assert_eq!(decls[0].patterns, vec!["*.h"]);
    assert!(decls[0].flatten);
}

#[test]
fn preserves_nested_notdir_wildcards_and_literal_filter_out_for_freetype() {
    // workbench/libs/freetype2/mmakefile.src publishes a fetched header
    // tree, but deliberately generates ftoption.h instead of copying its
    // upstream default.  The source directory does not exist at configure
    // time, so the resulting glob and exclusion must remain declarative.
    let src = "\
FT2NAME := freetype
FT2VERS := 2.14.3
ARCHBASE := $(FT2NAME)-$(FT2VERS)
FT2SRCDIR := $(PORTSDIR)/$(FT2NAME)2/$(ARCHBASE)
FT2_INCLUDE_FILES := $(notdir $(call WILDCARD, $(FT2SRCDIR)/include/*.h))
%copy_includes mmake=workbench-libs-freetype-includes-copy dir=$(FT2SRCDIR)/include includes=$(FT2_INCLUDE_FILES)
FT2I_INCLUDE_FILES := $(notdir $(call WILDCARD, $(FT2SRCDIR)/include/freetype/*.h))
%copy_includes mmake=workbench-libs-freetype-includes-copy path=freetype dir=$(FT2SRCDIR)/include/freetype includes=$(FT2I_INCLUDE_FILES)
FT2OPTIONFILE := ftoption.h
FT2CONFIG_INCLUDE_FILES := $(filter-out $(FT2OPTIONFILE),$(notdir $(call WILDCARD, $(FT2SRCDIR)/include/freetype/config/*.h)))
%copy_includes mmake=workbench-libs-freetype-includes-copy path=freetype/config dir=$(FT2SRCDIR)/include/freetype/config includes=$(FT2CONFIG_INCLUDE_FILES)
FT2INT_INCLUDE_FILES := $(notdir $(call WILDCARD, $(FT2SRCDIR)/include/freetype/internal/*.h))
%copy_includes mmake=workbench-libs-freetype-includes-copy path=freetype/internal dir=$(FT2SRCDIR)/include/freetype/internal includes=$(FT2INT_INCLUDE_FILES)
FT2SVC_INCLUDE_FILES := $(notdir $(call WILDCARD, $(FT2SRCDIR)/include/freetype/internal/services/*.h))
%copy_includes mmake=workbench-libs-freetype-includes-copy path=freetype/internal/services dir=$(FT2SRCDIR)/include/freetype/internal/services includes=$(FT2SVC_INCLUDE_FILES)
";
    let CopyIncludesScan { decls, skipped, .. } =
        collect_copy_includes(src, &PathBuf::from("workbench/libs/freetype2"));

    assert!(skipped.is_empty(), "skipped: {skipped:?}");
    assert_eq!(decls.len(), 5, "decls: {decls:?}");
    assert!(decls.iter().all(|decl| {
        decl.name == "workbench-libs-freetype-includes-copy"
            && decl
                .source_dir
                .starts_with("${AROS_PORTS_DIR}/freetype2/freetype-2.14.3/include")
            && decl.patterns == ["*.h"]
            && decl.flatten
    }));
    let config = decls
        .iter()
        .find(|decl| decl.dest == "freetype/config")
        .expect("freetype config header group");
    assert_eq!(config.excludes, ["ftoption.h"]);
    assert!(decls
        .iter()
        .filter(|decl| decl.dest != "freetype/config")
        .all(|decl| decl.excludes.is_empty()));
}

#[test]
fn resolves_gnu_wildcard_sort_and_patsubst_header_lists() {
    let src = "\
ARCHSRCDIR := $(PORTSDIR)/xz/xz-5.8.3
API_HEADERS := $(sort $(wildcard $(ARCHSRCDIR)/src/liblzma/api/lzma/*.h))
APIINCLUDE_FILES := $(patsubst $(ARCHSRCDIR)/src/liblzma/api/lzma/%,%,$(API_HEADERS))
%copy_includes mmake=lzma-includes dir=$(ARCHSRCDIR)/src/liblzma/api/lzma includes=$(APIINCLUDE_FILES) path=lzma
INCLUDE_FILES := $(wildcard include/*.h)
%copy_includes mmake=dbus-includes dir=include path=dbus
";
    let CopyIncludesScan { decls, skipped, .. } =
        collect_copy_includes(src, &PathBuf::from("workbench/libs/example"));

    assert!(skipped.is_empty(), "skipped: {skipped:?}");
    assert_eq!(decls.len(), 2);
    assert_eq!(
        decls[0].source_dir,
        "${AROS_PORTS_DIR}/xz/xz-5.8.3/src/liblzma/api/lzma"
    );
    assert_eq!(decls[0].patterns, ["*.h"]);
    assert_eq!(decls[0].dest, "lzma");
    assert_eq!(decls[1].source_dir, "workbench/libs/example/include");
    assert_eq!(decls[1].patterns, ["*.h"]);
    assert_eq!(decls[1].dest, "dbus");
}

#[test]
fn module_private_generated_header_is_recorded() {
    // rom/dos/mmakefile.src:90. Before $(GENDIR) was a recognised root this
    // rule went unreported, and the missing errorlist.h surfaced as an
    // undeclared MSG_STRING_* in displayerror.c instead.
    let src = "$(GENDIR)/$(CURDIR)/dos/errorlist.h : $(SRCDIR)/$(CURDIR)/catalogs/dos.cd\n";
    let CopyIncludesScan { adhoc, .. } = collect_copy_includes(src, &PathBuf::from("rom/dos"));
    assert_eq!(adhoc.len(), 1);
    assert_eq!(adhoc[0].root, "$(GENDIR)/");
    assert_eq!(adhoc[0].dest, "$(CURDIR)/dos/errorlist.h");
}

#[test]
fn non_header_under_gendir_is_reported_separately() {
    let src = "$(GENDIR)/boot/aros-amiga-m68k.elf : $(KOBJS_rom)\n";
    let CopyIncludesScan {
        adhoc,
        generated_files,
        ..
    } = collect_copy_includes(src, &PathBuf::from("arch/m68k-amiga/boot"));
    assert!(adhoc.is_empty());
    assert_eq!(generated_files.len(), 1);
    assert!(generated_files[0].contains("aros-amiga-m68k.elf"));
}

#[test]
fn directory_and_dependency_rules_are_not_reported() {
    // CMake makes output directories itself and tracks header dependencies
    // through the compiler, so neither can go missing.
    let src = "\
$(GENDIR)/$(CURDIR) :
$(GENDIR)/$(CURDIR)/errorlist.d : $(GENDIR)/$(CURDIR)/errorlist.h
$(GENDIR)/$(CURDIR)/.includes-generated : $(GENMODULE)
";
    let CopyIncludesScan {
        adhoc,
        generated_files,
        ..
    } = collect_copy_includes(src, &PathBuf::from("rom/dos"));
    assert!(adhoc.is_empty(), "adhoc: {adhoc:?}");
    assert!(generated_files.is_empty(), "reported: {generated_files:?}");
}

#[test]
fn target_in_an_include_root_counts_as_a_header_whatever_its_name() {
    // $(AROS_INCLUDES) is an include root, so a variable-named or pattern
    // target there is still a header. Only $(GENDIR) mixes file kinds.
    let src = "\
$(AROS_INCLUDES)/freetype/config/$(FT2OPTIONFILE) : $(FT2SRCDIR)/x
$(AROS_INCLUDES)/% : %
";
    let CopyIncludesScan {
        adhoc,
        generated_files,
        ..
    } = collect_copy_includes(src, &PathBuf::from("workbench/libs/freetype2"));
    assert_eq!(adhoc.len(), 2);
    assert!(generated_files.is_empty());
}

#[test]
fn a_static_pattern_rule_into_the_sdk_is_recorded() {
    // workbench/network/common/include stages 81 headers this way, the
    // whole BSD socket interface. The include root sits in the middle
    // field, not in the target, so matching only the target missed it and
    // netdb.h, netinet/in.h and proto/socket.h went absent unreported.
    let src = "\
INCLUDES      := $(call WILDCARD, *.h arpa/*.h)
DEST_INCLUDES := $(foreach f,$(INCLUDES),$(AROS_INCLUDES)/$(f))

$(DEST_INCLUDES) : $(AROS_INCLUDES)/% : $(SRCDIR)/$(CURDIR)/%
";
    let CopyIncludesScan { adhoc, .. } =
        collect_copy_includes(src, &PathBuf::from("workbench/network/common/include"));
    assert_eq!(adhoc.len(), 1, "adhoc: {adhoc:?}");
    assert_eq!(adhoc[0].root, "$(AROS_INCLUDES)/");
    assert_eq!(adhoc[0].dest, "%");
    assert!(adhoc[0].prereqs.contains("DEST_INCLUDES"));
}

#[test]
fn the_udis86_itab_rule_is_modelled_not_reported() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let rel = "arch/all-pc/udis86";
    let content = aros_common::read_source(&root.join(rel).join("mmakefile.src")).unwrap();
    let scan = collect_copy_includes(&content, std::path::Path::new(rel));

    assert_eq!(
        scan.skipped_script_outputs.len(),
        0,
        "{:?}",
        scan.skipped_script_outputs
    );
    assert_eq!(scan.script_outputs.len(), 1, "{:?}", scan.script_outputs);
    let decl = &scan.script_outputs[0];
    assert_eq!(
        decl.output,
        "${CMAKE_BINARY_DIR}/gen/arch/all-pc/udis86/libudis86/itab.c"
    );
    assert!(
        decl.script.ends_with("scripts/ud_itab.py"),
        "{}",
        decl.script
    );
    assert_eq!(decl.arguments.len(), 2, "{:?}", decl.arguments);
    assert!(decl.arguments[0].ends_with("docs/x86/optable.xml"));
    assert!(decl.arguments[1].ends_with("gen/arch/all-pc/udis86/libudis86"));
    // The rule stays out of the unmodelled report now that it is modelled.
    assert!(
        !scan
            .generated_files
            .iter()
            .any(|note| note.contains("itab.c")),
        "{:?}",
        scan.generated_files
    );
}

#[test]
fn vc4_cle_stdout_generators_are_modelled_exactly() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let rel = "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium";
    let content = aros_common::read_source(&root.join(rel).join("mmakefile.src")).unwrap();
    let external = |name: &str| match name {
        "top_builddir" => Some("$(GENDIR)/workbench/libs/mesa/20.0.8".to_owned()),
        "top_srcdir" => Some("$(PORTSDIR)/mesa/mesa-20.0.8".to_owned()),
        _ => None,
    };
    let scan =
        collect_copy_includes_with_lookup(&content, std::path::Path::new(rel), Some(&external));

    assert!(
        !scan
            .skipped_script_outputs
            .iter()
            .any(|reason| reason.contains("v3d_packet")),
        "{:?}",
        scan.skipped_script_outputs
    );
    assert_eq!(scan.script_outputs.len(), 4, "{:?}", scan.script_outputs);
    let v33 = scan
        .script_outputs
        .iter()
        .find(|declaration| declaration.output.ends_with("v3d_packet_v33_pack.h"))
        .expect("v33 generator");
    assert!(v33.stdout);
    assert_eq!(v33.arguments.last().map(String::as_str), Some("33"));
    assert!(v33.script.ends_with("/src/broadcom/cle/gen_pack_header.py"));
    assert!(v33
        .depends
        .iter()
        .any(|path| path.ends_with("v3d_packet_v33.xml")));
    assert!(v33.working_directory.is_none());
    assert!(
        !scan
            .generated_files
            .iter()
            .any(|note| note.contains("v3d_packet")),
        "{:?}",
        scan.generated_files
    );
}

#[test]
fn generated_owner_resolves_addprefix_and_filter() {
    let src = r"
GENERATED := one.c two.h
ROOT := $(GENDIR)/module
owner-generated : $(addprefix $(ROOT)/,$(filter %.h,$(GENERATED)))
$(ROOT)/two.h: $(SRCDIR)/generator.py
	$(PYTHON) $< > $@
";
    let scan = collect_copy_includes(src, Path::new("module"));
    assert_eq!(scan.script_outputs.len(), 1, "{:?}", scan.script_outputs);
    assert_eq!(scan.script_outputs[0].consumer_targets, ["owner-generated"]);
}

#[test]
fn aboutaros_private_python_headers_are_modelled_with_their_consumer() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let rel = "workbench/system/AboutAROS";
    let content = aros_common::read_source(&root.join(rel).join("mmakefile.src")).unwrap();
    let scan = collect_copy_includes(&content, std::path::Path::new(rel));

    assert_eq!(scan.script_outputs.len(), 3, "{:?}", scan.script_outputs);
    assert!(scan.skipped_script_outputs.is_empty());
    for declaration in &scan.script_outputs {
        assert!(
            declaration
                .output
                .starts_with("${AROS_BUILD_DIR}/workbench/system/AboutAROS/"),
            "{}",
            declaration.output
        );
        assert_eq!(declaration.consumer_source_stems, ["aboutaros"]);
        assert_eq!(declaration.arguments.last(), Some(&declaration.output));
    }
}
