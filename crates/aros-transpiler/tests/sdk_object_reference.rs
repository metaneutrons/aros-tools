//! Independent GNU Make reference for SDK implicit lane selection.

use aros_transpiler::{dirs::DirVars, parse_mmakefile_with_dirs_and_context, TargetContext};
use std::{fs, process::Command};

#[test]
fn sdk_output_lane_honors_unrequested_mixed_prerequisite() {
    let root = tempfile::tempdir().expect("private SDK object fixture");
    let root = root.path();
    let local = root.join("local");
    fs::create_dir_all(root.join("config")).expect("create fixture config directory");
    fs::create_dir_all(&local).expect("create fixture source directory");
    fs::write(
        root.join("config/make.cfg.in"),
        "AROS_LIB := ${AROS_BUILD_DIR}/SYS/Developer/lib\n",
    )
    .expect("write directory configuration");
    fs::write(local.join("foo.c"), "int sdk_lane_fixture;\n").expect("write the local source");

    let source_makefile = concat!(
        "$(AROS_LIB)/%.o : $(GENDIR)/$(CURDIR)/%.o\n",
        "\t@$(CP) $< $@\n",
        "%rule_compile basename=% targetdir=$(GENDIR)/$(CURDIR)\n",
        "$(AROS_LIB)/%.o : $(GENDIR)/$(CURDIR)/alt/%.o\n",
        "\t@$(CP) $< $@\n",
        "%rule_compile basename=% targetdir=$(GENDIR)/$(CURDIR)/alt\n",
        "requested: $(AROS_LIB)/foo.o\n",
        "unrequested: $(GENDIR)/$(CURDIR)/alt/foo.o unrelated.h\n",
    );
    let source_path = local.join("mmakefile.src");
    fs::write(&source_path, source_makefile).expect("write source-derived Make fixture");

    let dirs = DirVars::load(root);
    let parsed =
        parse_mmakefile_with_dirs_and_context(&source_path, root, &dirs, &TargetContext::default())
            .expect("parse source-derived SDK object rules");
    let requested = parsed
        .sdk_object_groups
        .iter()
        .find(|group| group.owner == "requested")
        .expect("retain the finite requested group");
    assert_eq!(requested.objects.len(), 1);
    assert!(
        requested.objects[0]
            .intermediate
            .ends_with("/gen/local/alt/foo.o"),
        "collector selected the wrong implicit lane: {:#?}",
        requested.objects[0]
    );
    assert!(parsed.native_graph_errors.iter().any(|diagnostic| {
        diagnostic
            .context
            .as_ref()
            .and_then(|context| context.target.as_deref())
            == Some("unrequested")
            && diagnostic.message.contains("unsupported prerequisite")
    }));

    let make = match Command::new("make").arg("--version").output() {
        Ok(version)
            if version.status.success()
                && String::from_utf8_lossy(&version.stdout).contains("GNU Make") =>
        {
            "make"
        }
        _ => {
            eprintln!("skipping GNU Make reference: GNU `make` is unavailable");
            return;
        }
    };
    let lib = root.join("build/SYS/Developer/lib");
    let plain_gen = root.join("build/gen/local");
    fs::create_dir_all(&lib).expect("create fixture Developer library directory");
    fs::create_dir_all(plain_gen.join("alt")).expect("create fixture generated lane directories");

    // These are the two pattern rule pairs after expanding the source's
    // $(AROS_LIB), $(GENDIR), and $(CURDIR) references for this fixture.
    let reference_makefile = format!(
        "AROS_LIB := {}\nLOCAL_GEN := {}\n\
         $(AROS_LIB)/%.o : $(LOCAL_GEN)/%.o\n\
         \t@printf plain-stage > $@\n\
         $(AROS_LIB)/%.o : $(LOCAL_GEN)/alt/%.o\n\
         \t@printf alt-stage > $@\n\
         $(LOCAL_GEN)/%.o : %.c\n\
         \t@printf plain-compile > $@\n\
         $(LOCAL_GEN)/alt/%.o : %.c\n\
         \t@printf alt-compile > $@\n\
         requested: $(AROS_LIB)/foo.o\n\
         unrequested: $(LOCAL_GEN)/alt/foo.o unrelated.h\n",
        lib.display(),
        plain_gen.display()
    );
    let makefile = root.join("GNUmakefile");
    fs::write(&makefile, reference_makefile).expect("write hand-expanded GNU Make reference");
    let output = Command::new(make)
        .args(["-rR", "-f"])
        .arg(&makefile)
        .arg("requested")
        .current_dir(&local)
        .output()
        .expect("run clean GNU Make reference");
    assert!(
        output.status.success(),
        "GNU Make failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(lib.join("foo.o")).expect("read generated lane marker"),
        "alt-stage",
        "GNU Make must select the lane whose intermediate is an explicit prerequisite elsewhere"
    );
}
