use super::collect_from_snapshot;
use crate::dirs::DirVars;
use crate::make_vars::collect_vars_impl;
use crate::parser::{join_continuations, macro_invocations};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
    relative: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("aros-sdk-objects-{}-{id}", std::process::id()));
        let relative = PathBuf::from("compiler/startup");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join(&relative)).unwrap();
        fs::write(
            root.join("config/make.cfg.in"),
            "AROS_LIB := ${AROS_BUILD_DIR}/SYS/Developer/lib\n",
        )
        .unwrap();
        Self { root, relative }
    }

    fn source(&self, basename: &str, extension: &str) {
        fs::write(
            self.root
                .join(&self.relative)
                .join(format!("{basename}.{extension}")),
            "int sdk_object_fixture;\n",
        )
        .unwrap();
    }

    fn collect(
        &self,
        source: &str,
    ) -> (
        Vec<super::SdkObjectGroupDecl>,
        Vec<super::SdkObjectRejection>,
    ) {
        let joined = join_continuations(source);
        let (scope, states) = collect_vars_impl(&joined, None);
        let invocations = macro_invocations(&joined);
        let dirs = DirVars::load(&self.root);
        collect_from_snapshot(
            &invocations,
            &scope,
            &dirs,
            &self.root,
            &self.relative,
            Some(&states),
            &joined,
        )
    }

    fn stage(lane: &str, recipe: &str) -> String {
        let lane = if lane.is_empty() {
            String::new()
        } else {
            format!("/{lane}")
        };
        format!("$(AROS_LIB)/%.o : $(GENDIR)/$(CURDIR){lane}/%.o\n\t{recipe}\n")
    }

    fn compile(lane: &str, language: &str) -> String {
        let lane = if lane.is_empty() {
            String::new()
        } else {
            format!("/{lane}")
        };
        format!("%rule_compile{language} basename=% targetdir=$(GENDIR)/$(CURDIR){lane}\n")
    }

    fn aggregate(owner: &str, objects: &[String]) -> String {
        format!("{owner}: {}\n", objects.join(" "))
    }

    fn intermediates(lane: &str, names: &[&str]) -> Vec<String> {
        names
            .iter()
            .map(|name| {
                let lane = if lane.is_empty() {
                    String::new()
                } else {
                    format!("/{lane}")
                };
                format!("$(GENDIR)/$(CURDIR){lane}/{name}.o")
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn outputs(names: &[&str]) -> Vec<String> {
    names
        .iter()
        .map(|name| format!("$(AROS_LIB)/{name}.o"))
        .collect()
}

#[test]
fn sdk_objects_collect_plain_nix_and_cxx_lanes_with_positional_flags() {
    let fixture = Fixture::new();
    for name in ["plain1", "plain2", "plain3", "plain4", "nixmain"] {
        fixture.source(name, "c");
    }
    for name in ["cxx1", "cxx2", "cxx3"] {
        fixture.source(name, "cpp");
    }
    let mut source = String::new();
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str("USER_CPPFLAGS += -D_XOPEN_SOURCE=700\n");
    source.push_str(&Fixture::stage("nix", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("nix", ""));
    source.push_str(&Fixture::stage("cxx", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("cxx", "_cxx"));
    let mut prerequisites = outputs(&["plain1", "plain2", "plain3", "plain4"]);
    prerequisites.extend(Fixture::intermediates("nix", &["nixmain"]));
    prerequisites.extend(outputs(&["nixmain"]));
    prerequisites.extend(Fixture::intermediates("cxx", &["cxx1", "cxx2", "cxx3"]));
    prerequisites.extend(outputs(&["cxx1", "cxx2", "cxx3"]));
    source.push_str(&Fixture::aggregate("startup-aggregate", &prerequisites));
    let (groups, rejected) = fixture.collect(&source);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(groups.len(), 1);
    let objects = &groups[0].objects;
    assert_eq!(objects.len(), 8);
    assert_eq!(
        objects
            .iter()
            .filter(|object| object.language == "C")
            .count(),
        5
    );
    assert_eq!(
        objects
            .iter()
            .filter(|object| object.language == "CXX")
            .count(),
        3
    );
    assert!(objects[..4].iter().all(|object| object.defines.is_empty()));
    assert_eq!(objects[4].defines, vec!["_XOPEN_SOURCE=700"]);
    assert!(objects[5..]
        .iter()
        .all(|object| object.defines == ["_XOPEN_SOURCE=700"]));
    assert!(objects.iter().all(|object| object
        .output
        .starts_with("${AROS_BUILD_DIR}/SYS/Developer/lib/")));
}

#[test]
fn sdk_objects_implicit_lane_uses_prerequisites_from_other_aggregates() {
    let fixture = Fixture::new();
    fixture.source("same", "c");
    let mut source = String::new();
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::stage("alternate", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("alternate", ""));
    source.push_str(&Fixture::aggregate("requested", &outputs(&["same"])));
    source.push_str("unrequested-root: $(GENDIR)/$(CURDIR)/alternate/same.o\n");
    let (groups, rejected) = fixture.collect(&source);
    assert!(rejected.is_empty(), "{rejected:#?}");
    let requested = groups
        .iter()
        .find(|group| group.owner == "requested")
        .unwrap();
    assert!(requested.objects[0]
        .intermediate
        .ends_with("/alternate/same.o"));
}

#[test]
fn sdk_objects_implicit_lane_uses_known_objects_from_rejected_aggregates() {
    let fixture = Fixture::new();
    fixture.source("same", "c");
    let mut source = String::new();
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::stage("alternate", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("alternate", ""));
    source.push_str(&Fixture::aggregate("requested", &outputs(&["same"])));
    source.push_str("rejected-root: $(GENDIR)/$(CURDIR)/alternate/same.o unrelated.h\n");
    let (groups, rejected) = fixture.collect(&source);
    assert!(rejected.iter().any(|item| item.owner == "rejected-root"));
    let requested = groups
        .iter()
        .find(|group| group.owner == "requested")
        .unwrap();
    assert!(requested.objects[0]
        .intermediate
        .ends_with("/alternate/same.o"));
}

#[test]
fn sdk_objects_refuses_implicit_lane_when_sibling_prerequisites_are_unresolved() {
    let fixture = Fixture::new();
    fixture.source("same", "c");
    let mut source = String::new();
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::stage("alternate", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("alternate", ""));
    source.push_str(&Fixture::aggregate("requested", &outputs(&["same"])));
    source.push_str(
        "unresolved-root: $(GENDIR)/$(CURDIR)/alternate/same.o $(UNKNOWN_PREREQUISITES)\n",
    );
    let (groups, rejected) = fixture.collect(&source);
    assert!(groups.is_empty());
    assert!(rejected.iter().any(|item| {
        item.owner == "requested" && item.reason.contains("cannot prove implicit lane")
    }));
}

#[test]
fn sdk_objects_refuses_lane_that_depends_on_an_unresolved_conditional() {
    let fixture = Fixture::new();
    fixture.source("same", "c");
    let mut source = String::new();
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::stage("alternate", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("alternate", ""));
    source.push_str("ifeq ($(UNKNOWN_LANE),1)\n");
    source.push_str("conditional-root: $(GENDIR)/$(CURDIR)/alternate/same.o\n");
    source.push_str("endif\n");
    source.push_str(&Fixture::aggregate("requested", &outputs(&["same"])));
    let (groups, rejected) = fixture.collect(&source);
    assert!(groups.is_empty());
    assert!(rejected
        .iter()
        .any(|item| { item.owner == "requested" && item.reason.contains("unresolved aggregate") }));
}

#[test]
fn sdk_objects_reject_unknown_debug_and_frozen_flag_values() {
    let fixture = Fixture::new();
    fixture.source("debug", "c");
    let mut source = String::new();
    source.push_str("ifeq ($(STARTUP_DEBUG),1)\nUSER_CPPFLAGS += -DDEBUG=1\nendif\n");
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::aggregate("debug-owner", &outputs(&["debug"])));
    let (groups, rejected) = fixture.collect(&source);
    assert!(groups.is_empty());
    assert!(rejected.iter().any(|item| item.owner == "debug-owner"));

    let frozen = Fixture::new();
    frozen.source("frozen", "c");
    let mut source = String::from("USER_CPPFLAGS := $(UNKNOWN_FLAGS)\n");
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::aggregate("frozen-owner", &outputs(&["frozen"])));
    let (groups, rejected) = frozen.collect(&source);
    assert!(groups.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.owner == "frozen-owner" && item.reason.contains("compile flags")));
}

#[test]
fn sdk_objects_reject_conflicting_macro_actions_and_default_flag_replacements() {
    for (flags, expected_reason) in [
        ("-DNAME=1 -DNAME=2", "conflicting preprocessor actions"),
        ("-DNAME=1 -UNAME", "conflicting preprocessor actions"),
        ("-UNAME -DNAME=1", "conflicting preprocessor actions"),
        ("-D=NAME", "unsafe preprocessor definition"),
    ] {
        let fixture = Fixture::new();
        fixture.source("conflict", "c");
        let mut source = format!("USER_CPPFLAGS += {flags}\n");
        source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
        source.push_str(&Fixture::compile("", ""));
        source.push_str(&Fixture::aggregate(
            "conflict-owner",
            &outputs(&["conflict"]),
        ));
        let (groups, rejected) = fixture.collect(&source);
        assert!(groups.is_empty());
        assert!(rejected
            .iter()
            .any(|item| { item.reason.contains(expected_reason) }));
    }

    let fixture = Fixture::new();
    fixture.source("replaced", "c");
    let mut source = String::from("CFLAGS := -O2\n");
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::aggregate(
        "replacement-owner",
        &outputs(&["replaced"]),
    ));
    let (groups, rejected) = fixture.collect(&source);
    assert!(groups.is_empty());
    assert!(rejected.iter().any(|item| {
        item.owner == "replacement-owner"
            && item
                .reason
                .contains("assignment replaces the compile macro default")
    }));
}

#[test]
fn sdk_objects_reject_altered_copy_unknown_prerequisite_and_duplicate_stage() {
    let altered = Fixture::new();
    altered.source("copy", "c");
    let mut source = Fixture::stage("", "@cp $< $@");
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::aggregate("copy-owner", &outputs(&["copy"])));
    let (groups, rejected) = altered.collect(&source);
    assert!(groups.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("sole exact")));

    let extra = Fixture::new();
    extra.source("extra", "c");
    let mut source = Fixture::stage("", "@$(CP) $< $@");
    source.push_str(&Fixture::compile("", ""));
    source.push_str("extra-owner: $(AROS_LIB)/extra.o unrelated.h\n");
    let (groups, rejected) = extra.collect(&source);
    assert!(groups.is_empty());
    assert!(rejected.iter().any(
        |item| item.owner == "extra-owner" && item.reason.contains("unsupported prerequisite")
    ));

    let duplicate = Fixture::new();
    duplicate.source("duplicate", "c");
    let mut source = Fixture::stage("", "@$(CP) $< $@");
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::aggregate(
        "duplicate-owner",
        &outputs(&["duplicate"]),
    ));
    let (groups, rejected) = duplicate.collect(&source);
    assert!(groups.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("duplicate")));
}

#[cfg(unix)]
#[test]
fn sdk_objects_reject_unsafe_source_and_ignore_define_bodies() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    fixture.source("real", "c");
    symlink(
        fixture.root.join(&fixture.relative).join("real.c"),
        fixture.root.join(&fixture.relative).join("unsafe.c"),
    )
    .unwrap();
    let mut source = Fixture::stage("", "@$(CP) $< $@");
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::aggregate("unsafe-owner", &outputs(&["unsafe"])));
    let (groups, rejected) = fixture.collect(&source);
    assert!(groups.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("unsafe SDK object source")));

    let defined = Fixture::new();
    defined.source("body", "c");
    let mut source = String::from("define dormant\n");
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str("define override export nested\n");
    source.push_str("export override define deeper\n");
    source.push_str("endef\n");
    source.push_str("endef\n");
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::aggregate(
        "nested-body-owner",
        &outputs(&["body"]),
    ));
    source.push_str("endef\n");
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::aggregate("active-owner", &outputs(&["body"])));
    let (groups, rejected) = defined.collect(&source);
    assert!(rejected.is_empty(), "{rejected:#?}");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].owner, "active-owner");

    let malformed = Fixture::new();
    malformed.source("body", "c");
    let mut source = String::from("define override\nendef\n");
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::aggregate("malformed-owner", &outputs(&["body"])));
    let (groups, rejected) = malformed.collect(&source);
    assert!(groups.is_empty());
    assert!(rejected.iter().any(|item| item.owner == "malformed-owner"));
}

#[test]
fn sdk_objects_reject_unsafe_codegen_flag_macros() {
    let fixture = Fixture::new();
    fixture.source("plugin", "c");
    let mut source = String::from("USER_CFLAGS += -fplugin=payload.so\n");
    source.push_str(&Fixture::stage("", "@$(CP) $< $@"));
    source.push_str(&Fixture::compile("", ""));
    source.push_str(&Fixture::aggregate("plugin-owner", &outputs(&["plugin"])));
    let (groups, rejected) = fixture.collect(&source);
    assert!(groups.is_empty());
    assert!(rejected.iter().any(|item| item
        .reason
        .contains("unsupported or unsafe user compile flag")));
}
