use super::*;
use crate::fetch::collect_fetches_with_scope;
use crate::make_vars::collect_vars_impl;
use crate::parser::{join_continuations, macro_invocations};
use std::fs;
use tempfile::tempdir;

const CONFIG: &str = "AROSDIR := $(TARGETDIR)/SYS\nAROS_DIR_DEVELOPER := Developer\nAROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\nAROS_DIR_SDK := SDK\nAROS_SDK := $(AROS_DEVELOPER)/$(AROS_DIR_SDK)\nAROS_DIR_FD := fd\nAROS_SDK_FD := $(AROS_SDK)/$(AROS_DIR_FD)\nAROS_DIR_LIB := lib\nAROS_LIB := $(AROS_DEVELOPER)/$(AROS_DIR_LIB)\nPORTSDIR := $(TARGETDIR)/Ports\n";

fn fixture(
    source: &str,
    add_local_input: bool,
) -> (
    tempfile::TempDir,
    Vec<Invocation>,
    VarScope,
    DirVars,
    Vec<FetchDecl>,
    Vec<ConditionalTruth>,
) {
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("config/make.cfg.in"), CONFIG).unwrap();
    fs::create_dir_all(root.join("module")).unwrap();
    fs::write(root.join("module/mmakefile.src"), source).unwrap();
    if add_local_input {
        fs::create_dir_all(root.join("module/local")).unwrap();
        fs::write(root.join("module/local/local.fd"), b"local").unwrap();
    }
    let joined = join_continuations(source);
    let (scope, states) = collect_vars_impl(&joined, None);
    let dirs = DirVars::load(root);
    let invocations = macro_invocations(&joined);
    let (fetches, _) = collect_fetches_with_scope(source, Path::new("module"), &scope);
    (temp, invocations, scope, dirs, fetches, states)
}

fn fetched_fixture(copy_line: &str) -> String {
    format!(
        "ARCHIVE_DIR := $(PORTSDIR)/fixture/source\n%fetch mmake=fixture-fetch archive=fixture destination=$(PORTSDIR)/fixture\n#MM fixture-fd-copy : \\\n#MM     fixture-fetch\n{copy_line}\n"
    )
}

fn developer_bin_man_source(copies: &str) -> String {
    format!(
        "ARCHBASE := bzip2-1.0.8\nARCHSRCDIR := $(PORTSDIR)/bzip2/$(ARCHBASE)\nSH_FILES := bzdiff bzgrep bzmore\nMAN_FILES := bzdiff.1 bzgrep.1 bzip2.1 bzmore.1\nBIN_DIR := $(AROS_DEVELOPER)/bin\nMAN_DIR := $(AROS_DEVELOPER)/man/man1\n%fetch mmake=external-bzip2-fetch archive=$(ARCHBASE) destination=$(PORTSDIR)/bzip2\n#MM- external-bz2-bzip2 : external-bz2-bzip2-install-sh external-bz2-bzip2-install-man-cpy\n#MM external-bz2-bzip2-install-man-cpy : external-bz2-bzip2-install-man\n{copies}\n"
    )
}

#[test]
fn explicit_fetched_file_copy_has_one_local_fetch_owner() {
    let source = fetched_fixture(
        "%copy_files_q mmake=fixture-fd-copy files=api_lib.fd src=$(ARCHIVE_DIR)/developer/fd dst=$(AROS_SDK_FD)",
    );
    let (temp, invocations, scope, dirs, fetches, states) = fixture(&source, false);
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].owner, "fixture-fd-copy");
    assert_eq!(declarations[0].files, ["api_lib.fd"]);
    assert_eq!(
        declarations[0].source_dir,
        "${AROS_PORTS_DIR}/fixture/source/developer/fd"
    );
    assert_eq!(declarations[0].destination, FD_DIRECTORY_ALIAS);
    assert_eq!(
        declarations[0].fetch_owner.as_deref(),
        Some("fixture-fetch")
    );
}

#[test]
fn in_tree_regular_file_is_copied_without_a_fetch_edge() {
    let source = "#MM fixture-local-copy :\n%copy_files_q mmake=fixture-local-copy files=local.fd src=local dst=$(AROS_SDK_FD)\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(source, true);
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(
        declarations[0].source_dir,
        "${AROS_SOURCE_DIR}/module/local"
    );
    assert_eq!(declarations[0].fetch_owner, None);
}

#[test]
fn local_macro_proves_its_own_fd_producer_without_a_handwritten_edge() {
    let source = "#MM- public-headers : fixture-local-copy\n%copy_files_q mmake=fixture-local-copy files=local.fd src=local dst=$(AROS_SDK_FD)\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(source, true);
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].fetch_owner, None);
    for suffix in [
        "fixture-local-copy:\n\t@echo unmodelled\n",
        "%copy_files_q mmake=fixture-local-copy files=local.fd src=local dst=$(AROS_SDK_FD)\n",
    ] {
        let changed = format!("{source}{suffix}");
        let (temp, invocations, scope, dirs, fetches, states) = fixture(&changed, true);
        let (declarations, rejected) = collect(
            &invocations,
            &scope,
            &dirs,
            temp.path(),
            Path::new("module"),
            Some(&states),
            &fetches,
        );
        assert!(declarations.is_empty());
        assert!(!rejected.is_empty());
    }
}

#[test]
fn developer_bin_and_man_copies_bind_the_local_fetch_and_keep_meta_prerequisites() {
    let source = developer_bin_man_source(
        "%copy_files_q mmake=external-bz2-bzip2-install-sh src=$(ARCHSRCDIR)/. files=$(SH_FILES) dst=$(BIN_DIR)\n%copy_files_q mmake=external-bz2-bzip2-install-man-cpy src=$(ARCHSRCDIR)/. files=$(MAN_FILES) dst=$(MAN_DIR)",
    );
    let (temp, invocations, scope, dirs, fetches, states) = fixture(&source, false);
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(declarations.len(), 2);
    assert_eq!(declarations[0].owner, "external-bz2-bzip2-install-sh");
    assert_eq!(declarations[0].files, ["bzdiff", "bzgrep", "bzmore"]);
    assert_eq!(declarations[0].destination, DEVELOPER_BIN_DIRECTORY_ALIAS);
    assert_eq!(
        declarations[0].source_dir,
        "${AROS_PORTS_DIR}/bzip2/bzip2-1.0.8"
    );
    assert_eq!(
        declarations[0].fetch_owner.as_deref(),
        Some("external-bzip2-fetch")
    );
    assert_eq!(declarations[1].owner, "external-bz2-bzip2-install-man-cpy");
    assert_eq!(
        declarations[1].files,
        ["bzdiff.1", "bzgrep.1", "bzip2.1", "bzmore.1"]
    );
    assert_eq!(declarations[1].destination, DEVELOPER_MAN1_DIRECTORY_ALIAS);
    assert_eq!(
        declarations[1].fetch_owner.as_deref(),
        Some("external-bzip2-fetch")
    );

    let parsed_edges = parse_meta_edges_from_snapshot(
        temp.path(),
        Path::new("module"),
        Some(&states),
        &join_continuations(&source),
        false,
    )
    .unwrap();
    assert_eq!(
        parsed_edges
            .iter()
            .find(|edge| edge.owner == "external-bz2-bzip2-install-man-cpy")
            .unwrap()
            .prerequisites,
        ["external-bz2-bzip2-install-man"]
    );
}

#[test]
fn developer_file_copies_reject_ambiguous_or_foreign_fetch_owners() {
    let source = developer_bin_man_source(
        "%copy_files_q mmake=external-bz2-bzip2-install-sh src=$(ARCHSRCDIR)/. files=$(SH_FILES) dst=$(BIN_DIR)",
    );
    let ambiguous = source.replace(
        "%fetch mmake=external-bzip2-fetch archive=$(ARCHBASE) destination=$(PORTSDIR)/bzip2",
        "%fetch mmake=external-bzip2-fetch archive=$(ARCHBASE) destination=$(PORTSDIR)/bzip2\n%fetch mmake=other-bzip2-fetch archive=other destination=$(PORTSDIR)/bzip2",
    );
    let (temp, invocations, scope, dirs, fetches, states) = fixture(&ambiguous, false);
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(declarations.is_empty());
    assert!(rejected.iter().any(|rejection| rejection
        .reason
        .contains("ambiguous matching local `%fetch`")));

    let (_, _, _, _, mut foreign_fetches, _) = fixture(&source, false);
    foreign_fetches[0].dir = "another/recipe".into();
    let without_local_fetch = source.replace(
        "%fetch mmake=external-bzip2-fetch archive=$(ARCHBASE) destination=$(PORTSDIR)/bzip2\n",
        "",
    );
    let (temp, invocations, scope, dirs, _, states) = fixture(&without_local_fetch, false);
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &foreign_fetches,
    );
    assert!(declarations.is_empty());
    assert!(rejected.iter().any(|rejection| rejection
        .reason
        .contains("no uniquely matching local `%fetch`")));
}

#[test]
fn developer_file_copies_reject_open_lists_conditions_overrides_and_owner_collisions() {
    let cases = [
        (
            "%copy_files_q mmake=external-bz2-bzip2-install-sh src=$(ARCHSRCDIR)/. files=$(UNKNOWN_FILES) dst=$(BIN_DIR)",
            "unresolved or not source-owned",
        ),
        (
            "SH_FILES := $(shell echo bzdiff)\n%copy_files_q mmake=external-bz2-bzip2-install-sh src=$(ARCHSRCDIR)/. files=$(SH_FILES) dst=$(BIN_DIR)",
            "unsupported Make reference",
        ),
        (
            "%copy_files_q mmake=external-bz2-bzip2-install-sh src=$(ARCHSRCDIR)/. files=$(SH_FILES) dst=$(BIN_DIR)/nested",
            "destination is not the configured Developer bin path",
        ),
        (
            "external-bz2-bzip2-install-sh:\n\t@echo handwritten\n%copy_files_q mmake=external-bz2-bzip2-install-sh src=$(ARCHSRCDIR)/. files=$(SH_FILES) dst=$(BIN_DIR)",
            "handwritten Make rule also defines Developer bin copy owner",
        ),
        (
            "%copy_files_q mmake=external-bz2-bzip2-install-sh src=$(ARCHSRCDIR)/. files=$(SH_FILES) dst=$(BIN_DIR)\n%copy_files_q mmake=external-bz2-bzip2-install-sh src=$(ARCHSRCDIR)/. files=$(SH_FILES) dst=$(BIN_DIR)",
            "duplicate producer declarations",
        ),
        (
            "%copy_files_q mmake=external-bz2-bzip2-install-sh mmake=external-bz2-bzip2-install-sh src=$(ARCHSRCDIR)/. files=$(SH_FILES) dst=$(BIN_DIR)",
            "duplicate or unsupported arguments",
        ),
        (
            "ifeq ($(UNKNOWN_CONDITION), enabled)\n%copy_files_q mmake=external-bz2-bzip2-install-sh src=$(ARCHSRCDIR)/. files=$(SH_FILES) dst=$(BIN_DIR)\nendif",
            "unresolved Make conditional",
        ),
        (
            "%copy_files_q mmake=external-bz2-bzip2-install-sh src=$(ARCHSRCDIR)/../../escape files=$(SH_FILES) dst=$(BIN_DIR)",
            "not a safe CMake-rooted path",
        ),
    ];
    for (copies, expected) in cases {
        let source = developer_bin_man_source(copies);
        let (temp, invocations, scope, dirs, fetches, states) = fixture(&source, false);
        let (declarations, rejected) = collect(
            &invocations,
            &scope,
            &dirs,
            temp.path(),
            Path::new("module"),
            Some(&states),
            &fetches,
        );
        assert!(declarations.is_empty(), "{copies}: {declarations:?}");
        assert!(
            rejected
                .iter()
                .any(|rejection| rejection.reason.contains(expected)),
            "expected {expected:?}, got {rejected:?} for {copies}"
        );
    }
}

#[cfg(unix)]
#[test]
fn developer_bin_copy_rejects_symlinked_local_inputs_and_opaque_controls() {
    let source = "#MM- consumer : bin-copy\n%copy_files_q mmake=bin-copy files=local.fd src=local dst=$(AROS_DEVELOPER)/bin\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(source, true);
    fs::remove_file(temp.path().join("module/local/local.fd")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", temp.path().join("module/local/local.fd")).unwrap();
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(declarations.is_empty());
    assert!(rejected
        .iter()
        .any(|rejection| rejection.reason.contains("symlink")));

    let defined_only_consumer = source
        .replace("#MM- consumer : bin-copy", "#MM- consumer : unrelated")
        + "define TEMPLATE\n#MM- fake-consumer : bin-copy\nendef\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(&defined_only_consumer, true);
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(declarations.is_empty());
    assert!(rejected.iter().any(|rejection| rejection
        .reason
        .contains("no active local `#MM` consumer dependency")));

    let malformed = format!("{source}endif\n");
    let (temp, invocations, scope, dirs, fetches, states) = fixture(&malformed, true);
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(declarations.is_empty());
    assert!(rejected
        .iter()
        .any(|rejection| rejection.reason.contains("unmatched or malformed `endif`")));
}

#[test]
fn leaves_other_copy_files_destinations_to_their_existing_capabilities() {
    let source = "%copy_files_q mmake=unrelated-copy files=api.h src=local dst=$(AROS_INCLUDES)\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(source, false);
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(declarations.is_empty());
    assert!(rejected.is_empty());
}

#[test]
fn refuses_nonliteral_files_unsafe_names_unknown_conditions_and_wrong_edges() {
    let cases = [
        ("files=$(FILES)", "developer/fd", "explicit literals"),
        ("files=../escape.fd", "developer/fd", "safe basename"),
        ("files=-option.fd", "developer/fd", "safe basename"),
        ("files=\"api.fd API.fd\"", "developer/fd", "safe basename"),
        ("files=api.fd stray.fd", "developer/fd", "unnamed"),
        ("files=*.fd", "developer/fd", "without expansion or globs"),
        ("files=\"\"", "developer/fd", "file list is empty"),
        ("files=api_lib.fd", "../../escape", "SDK fd copy source"),
    ];
    for (file_list, source_tail, expected) in cases {
        let copy = format!(
            "%copy_files_q mmake=fixture-fd-copy {file_list} src=$(ARCHIVE_DIR)/{source_tail} dst=$(AROS_SDK_FD)"
        );
        let source = fetched_fixture(&copy);
        let (temp, invocations, scope, dirs, fetches, states) = fixture(&source, false);
        let (declarations, rejected) = collect(
            &invocations,
            &scope,
            &dirs,
            temp.path(),
            Path::new("module"),
            Some(&states),
            &fetches,
        );
        assert!(declarations.is_empty());
        assert!(
            rejected.iter().any(|item| item.reason.contains(expected)),
            "{rejected:?}"
        );
    }

    let source = fetched_fixture(
        "%copy_files_q mmake=fixture-fd-copy mmake=fixture-fd-copy files=api_lib.fd src=$(ARCHIVE_DIR)/developer/fd dst=$(AROS_SDK_FD)",
    );
    let (temp, invocations, scope, dirs, fetches, states) = fixture(&source, false);
    let mut unknown = states;
    let copy_line = invocations
        .iter()
        .find(|item| item.name == "copy_files_q")
        .unwrap()
        .line;
    unknown[copy_line] = ConditionalTruth::Unknown;
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&unknown),
        &fetches,
    );
    assert!(declarations.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("unresolved Make conditional")));

    let source = fetched_fixture(
        "%copy_files_q mmake=fixture-fd-copy files=api_lib.fd src=$(ARCHIVE_DIR)/developer/fd dst=$(AROS_SDK_FD)",
    );
    let (temp, invocations, scope, dirs, fetches, states) = fixture(&source, false);
    let mut unknown_edge = states;
    let joined = join_continuations(&source);
    let edge_line = joined
        .lines()
        .position(|line| line.starts_with("#MM fixture-fd-copy"))
        .unwrap();
    unknown_edge[edge_line] = ConditionalTruth::Unknown;
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&unknown_edge),
        &fetches,
    );
    assert!(declarations.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("`#MM` edge is guarded")));

    let source = fetched_fixture(
        "%copy_files_q mmake=fixture-fd-copy files=api_lib.fd src=$(ARCHIVE_DIR)/developer/fd dst=$(AROS_SDK_FD)",
    )
    .replace("#MM     fixture-fetch", "#MM     other-fetch");
    let (temp, invocations, scope, dirs, fetches, states) = fixture(&source, false);
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(declarations.is_empty());
    assert!(rejected
        .iter()
        .any(|item| item.reason.contains("no matching local invocation")));
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_in_tree_source_file() {
    let source = "#MM fixture-local-copy :\n%copy_files_q mmake=fixture-local-copy files=local.fd src=local dst=$(AROS_SDK_FD)\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(source, true);
    fs::remove_file(temp.path().join("module/local/local.fd")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", temp.path().join("module/local/local.fd")).unwrap();
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(declarations.is_empty());
    assert!(rejected.iter().any(|item| item.reason.contains("symlink")));
}

#[test]
fn developer_lib_copy_expands_autofile_and_macro_defaults_in_order() {
    let source = "AROS_DIR_LIB := lib\nAUTOFILE := \\\n auto\n#MM linklibs-autoinit : includes linklibs-autoinit-autofile\n%copy_files_q mmake=linklibs-autoinit-autofile files=$(AUTOFILE) dst=$(AROS_LIB)\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(source, false);
    fs::write(temp.path().join("module/auto"), b"auto").unwrap();
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].owner, "linklibs-autoinit-autofile");
    assert_eq!(declarations[0].files, ["auto"]);
    assert_eq!(declarations[0].source_dir, "${AROS_SOURCE_DIR}/module");
    assert_eq!(declarations[0].destination, DEVELOPER_LIB_DIRECTORY_ALIAS);
    assert_eq!(declarations[0].fetch_owner, None);

    let source = "AROS_DIR_LIB := lib\nFILES := second first\n#MM default-consumer : default-copy\n%copy_files_q mmake=default-copy dst=$(AROS_LIB)\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(source, false);
    fs::write(temp.path().join("mmakefile.src"), source).unwrap();
    fs::write(temp.path().join("second"), b"second").unwrap();
    fs::write(temp.path().join("first"), b"first").unwrap();
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new(""),
        Some(&states),
        &fetches,
    );
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].source_dir, "${AROS_SOURCE_DIR}");
    assert_eq!(declarations[0].files, ["second", "first"]);
}

#[test]
fn uninstantiated_make_define_cannot_supply_a_live_developer_lib_file_list() {
    let source = "AROS_DIR_LIB := lib\ndefine TEMPLATE\nFILES := auto\nendef\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(source, false);
    fs::write(temp.path().join("module/auto"), b"auto").unwrap();
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(
        declarations.is_empty(),
        "uninstantiated definition fabricated a copy"
    );
    assert!(!rejected.is_empty(), "missing live FILES must be diagnosed");
}

#[test]
fn developer_lib_accepts_multiple_distinct_active_consumers() {
    let source = "AROS_DIR_LIB := lib\nFILES := auto\n#MM first-consumer : lib-copy\n#MM second-consumer : lib-copy\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(source, false);
    fs::write(temp.path().join("module/auto"), b"auto").unwrap();
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(declarations.len(), 1);
}

#[test]
fn developer_lib_requires_unambiguous_active_source_local_consumers() {
    let cases = [
        (
            "AROS_DIR_LIB := lib\nFILES := auto\n#MM lib-copy :\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "also has a local `#MM` producer edge",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\n#MM lib-copy : unrelated-prerequisite\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "also has a local `#MM` producer edge",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\n#MM consumer : lib-copy\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "duplicate local `#MM` consumer",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\nifeq ($(AROS_TARGET_CPU), unknown)\n#MM conditional-consumer : lib-copy\nendif\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "conditional or unknown",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\ndefine GENERATED\n#MM fake-consumer : lib-copy\nendef\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "no active local `#MM` consumer",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\nendif\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "unmatched or malformed `endif`",
        ),
    ];
    for (source, expected) in cases {
        let (temp, invocations, scope, dirs, fetches, states) = fixture(source, false);
        fs::write(temp.path().join("module/auto"), b"auto").unwrap();
        let (declarations, rejected) = collect(
            &invocations,
            &scope,
            &dirs,
            temp.path(),
            Path::new("module"),
            Some(&states),
            &fetches,
        );
        assert!(declarations.is_empty(), "{source}");
        assert!(
            rejected.iter().any(|item| item.reason.contains(expected)),
            "expected {expected:?}, got {rejected:?} for {source}"
        );
    }
}

#[test]
fn developer_lib_rejects_unresolved_unsafe_conditional_and_fetched_inputs() {
    let cases = [
        (
            "AROS_DIR_LIB := lib\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy files=$(UNKNOWN_FILES) dst=$(AROS_LIB)\n",
            "unresolved or not source-owned",
        ),
        (
            "AROS_DIR_LIB := lib\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy files=*.a dst=$(AROS_LIB)\n",
            "unsupported syntax",
        ),
        (
            "AROS_DIR_LIB := lib\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy files=$(shell echo auto) dst=$(AROS_LIB)\n",
            "unnamed or malformed argument",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := ../escape\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy files=$(FILES) dst=$(AROS_LIB)\n",
            "unsupported syntax",
        ),
        (
            "AROS_DIR_LIB := lib\nifdef UNKNOWN\nFILES := auto\nendif\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy files=$(FILES) dst=$(AROS_LIB)\n",
            "conditional assignment",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy files=$(FILES) src=$(PORTSDIR)/fixture dst=$(AROS_LIB)\n",
            "must be local to the selected source tree",
        ),
    ];
    for (source, expected) in cases {
        let (temp, invocations, scope, dirs, fetches, states) = fixture(source, false);
        fs::write(temp.path().join("module/auto"), b"auto").unwrap();
        let (declarations, rejected) = collect(
            &invocations,
            &scope,
            &dirs,
            temp.path(),
            Path::new("module"),
            Some(&states),
            &fetches,
        );
        assert!(declarations.is_empty(), "{source}");
        assert!(
            rejected.iter().any(|item| item.reason.contains(expected)),
            "expected {expected:?}, got {rejected:?} for {source}"
        );
    }
}

#[test]
fn developer_lib_rejects_redirected_or_nested_destination() {
    let cases = [
        (
            "AROS_DIR_LIB := lib\nAROS_LIB := /foreign/lib\nFILES := auto\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "not the configured Developer lib path",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)/nested\n",
            "not the configured Developer lib path",
        ),
    ];
    for (source, expected) in cases {
        let (temp, invocations, scope, dirs, fetches, states) = fixture(source, false);
        fs::write(temp.path().join("module/auto"), b"auto").unwrap();
        let (declarations, rejected) = collect(
            &invocations,
            &scope,
            &dirs,
            temp.path(),
            Path::new("module"),
            Some(&states),
            &fetches,
        );
        assert!(declarations.is_empty(), "{source}");
        assert!(
            rejected.iter().any(|item| item.reason.contains(expected)),
            "expected {expected:?}, got {rejected:?} for {source}"
        );
    }
}

#[test]
fn developer_lib_rejects_other_owner_operations() {
    let cases = [
        (
            "AROS_DIR_LIB := lib\nFILES := auto\nlib-copy: dependency\n\t@touch lib-copy\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "handwritten Make rule also defines",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\n$(DYNAMIC_OWNER): dependency\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "handwritten Make rule target `$(DYNAMIC_OWNER)` is unresolved",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\n#MM consumer : lib-copy\n%build_linklib mmake=lib-copy libname=extra files=extra.a\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "also has a `%build_linklib mmake=` provider",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\n#MM consumer : lib-copy\n%build_linklib mmake=unrelated mainmmake=lib-copy libname=extra files=extra.a\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "also has a `%build_linklib mainmmake=` provider",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\n#MM consumer : lib-copy\n%build_linklib mmake=unrelated parentmmake=lib-copy libname=extra files=extra.a\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "also has a `%build_linklib parentmmake=` provider",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\n#MM consumer : lib-copy\n%build_linklib mmake=$(DYNAMIC_OWNER) libname=extra files=extra.a\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "is unresolved, so ownership",
        ),
        (
            "AROS_DIR_LIB := lib\nFILES := auto\n#MM consumer : lib-copy\n%build_linklib mmake=unrelated parentmmake=$(DYNAMIC_OWNER) libname=extra files=extra.a\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n",
            "parentmmake is unresolved",
        ),
    ];
    for (source, expected) in cases {
        let (temp, invocations, scope, dirs, fetches, states) = fixture(source, false);
        fs::write(temp.path().join("module/auto"), b"auto").unwrap();
        let (declarations, rejected) = collect(
            &invocations,
            &scope,
            &dirs,
            temp.path(),
            Path::new("module"),
            Some(&states),
            &fetches,
        );
        assert!(declarations.is_empty(), "{source}");
        assert!(
            rejected.iter().any(|item| item.reason.contains(expected)),
            "expected {expected:?}, got {rejected:?} for {source}"
        );
    }
}

#[test]
fn developer_lib_consumer_proof_uses_the_caller_source_snapshot() {
    let snapshot =
        "AROS_DIR_LIB := lib\nFILES := auto\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n";
    let disk_source = "AROS_DIR_LIB := lib\nFILES := auto\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(snapshot, false);
    fs::write(temp.path().join("module/mmakefile.src"), disk_source).unwrap();
    fs::write(temp.path().join("module/auto"), b"auto").unwrap();
    let joined_snapshot = join_continuations(snapshot);
    let (declarations, rejected) = collect_from_snapshot(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
        &joined_snapshot,
    );
    assert!(declarations.is_empty());
    assert!(rejected.iter().any(|item| {
        item.reason
            .contains("no active local `#MM` consumer dependency")
    }));
}

#[test]
fn developer_lib_does_not_count_tabbed_else_or_recipe_meta_as_consumers() {
    let source = "AROS_DIR_LIB := lib\nFILES := auto\nifeq ($(AROS_TARGET_CPU), active)\nconditional-target:\n\telse\n#MM inactive-consumer : lib-copy\nendif\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n";
    let (temp, _, _, dirs, _, _) = fixture(source, false);
    fs::write(temp.path().join("module/auto"), b"auto").unwrap();
    let joined = join_continuations(source);
    let target = crate::parser::TargetContext {
        cpu: Some("inactive".into()),
        ..crate::parser::TargetContext::default()
    };
    let (scope, states) = collect_vars_impl(&joined, Some(&target));
    let invocations = macro_invocations(&joined);
    let (fetches, _) = collect_fetches_with_scope(source, Path::new("module"), &scope);
    let (declarations, rejected) = collect_from_snapshot(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
        &joined,
    );
    assert!(declarations.is_empty());
    assert!(rejected.iter().any(|item| {
        item.reason
            .contains("no active local `#MM` consumer dependency")
    }));

    let source = "AROS_DIR_LIB := lib\nFILES := auto\nrecipe-target:\n\t#MM fake-consumer : lib-copy\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(source, false);
    fs::write(temp.path().join("module/auto"), b"auto").unwrap();
    let joined = join_continuations(source);
    let (declarations, rejected) = collect_from_snapshot(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
        &joined,
    );
    assert!(declarations.is_empty());
    assert!(rejected.iter().any(|item| {
        item.reason
            .contains("no active local `#MM` consumer dependency")
    }));
}

#[cfg(unix)]
#[test]
fn developer_lib_refuses_symlinked_local_inputs() {
    let source = "AROS_DIR_LIB := lib\nFILES := auto\n#MM consumer : lib-copy\n%copy_files_q mmake=lib-copy dst=$(AROS_LIB)\n";
    let (temp, invocations, scope, dirs, fetches, states) = fixture(source, false);
    std::os::unix::fs::symlink("/etc/passwd", temp.path().join("module/auto")).unwrap();
    let (declarations, rejected) = collect(
        &invocations,
        &scope,
        &dirs,
        temp.path(),
        Path::new("module"),
        Some(&states),
        &fetches,
    );
    assert!(declarations.is_empty());
    assert!(rejected.iter().any(|item| item.reason.contains("symlink")));
}
