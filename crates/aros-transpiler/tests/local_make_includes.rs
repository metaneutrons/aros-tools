#[path = "../src/local_make_includes.rs"]
mod local_make_includes;

use local_make_includes::{
    inline_local_make_includes, LocalMakeFragmentPolicy, LocalMakeIncludeIssueKind,
    LocalMakeIncludeLimits,
};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn scan(root: &Path, mmake: &str, content: &str) -> local_make_includes::LocalMakeIncludeScan {
    inline_local_make_includes(
        content,
        root,
        Path::new(mmake),
        LocalMakeIncludeLimits::default(),
        LocalMakeFragmentPolicy::PlainSourceLists,
    )
}

fn scan_scopes(
    root: &Path,
    mmake: &str,
    content: &str,
) -> local_make_includes::LocalMakeIncludeScan {
    inline_local_make_includes(
        content,
        root,
        Path::new(mmake),
        LocalMakeIncludeLimits::default(),
        LocalMakeFragmentPolicy::SafeVariableScopes,
    )
}

#[test]
fn assignment_fragment_is_inserted_at_the_include_site() {
    let tree = TempDir::new().unwrap();
    write(
        &tree.path().join("module/files.list"),
        "FILES += from_fragment\n",
    );
    let source = "FILES := before\n\
include $(SRCDIR)/$(CURDIR)/files.list\n\
%build_linklib files=$(FILES)\n\
FILES := after\n";

    let result = scan(tree.path(), "module/mmakefile.src", source);

    assert!(result.issues.is_empty(), "{:?}", result.issues);
    let before = result.expanded.find("FILES := before").unwrap();
    let included = result.expanded.find("FILES += from_fragment").unwrap();
    let declaration = result.expanded.find("%build_linklib").unwrap();
    let after = result.expanded.find("FILES := after").unwrap();
    assert!(before < included && included < declaration && declaration < after);
    assert_eq!(result.fragments.len(), 1);
    assert_eq!(result.fragments[0].path, Path::new("module/files.list"));
    assert_eq!(
        result.fragments[0].included_from,
        Path::new("module/mmakefile.src")
    );
    assert_eq!(result.fragments[0].include_line, 2);
    assert_eq!(result.fragments[0].assigned_variables, ["FILES"]);
    assert!(!result.fragments[0].has_conditionals);
    assert!(result.fragments[0].plain_source_list);
}

#[test]
fn include_stays_inside_the_declaring_target_conditional() {
    let tree = TempDir::new().unwrap();
    write(&tree.path().join("module/files.list"), "FILES := arm\n");
    let source = "ifeq ($(AROS_TARGET_CPU),arm)\n\
include $(SRCDIR)/$(CURDIR)/files.list\n\
%build_linklib files=$(FILES)\n\
endif\n";

    let result = scan(tree.path(), "module/mmakefile.src", source);

    assert!(result.issues.is_empty(), "{:?}", result.issues);
    let condition = result.expanded.find("ifeq").unwrap();
    let assignment = result.expanded.find("FILES := arm").unwrap();
    let declaration = result.expanded.find("%build_linklib").unwrap();
    let end = result.expanded.rfind("endif").unwrap();
    assert!(condition < assignment && assignment < declaration && declaration < end);
}

#[test]
fn balanced_fragment_conditionals_are_preserved_and_identified() {
    let tree = TempDir::new().unwrap();
    write(
        &tree.path().join("module/files.list"),
        "FILES := common\nifeq ($(AROS_TARGET_CPU),arm)\nFILES += arm\nelse\nFILES += other\nendif\n",
    );
    let source = "include $(SRCDIR)/$(CURDIR)/files.list\n%build_linklib files=$(FILES)\n";

    let result = scan_scopes(tree.path(), "module/mmakefile.src", source);

    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert!(result.expanded.contains("FILES += arm"));
    assert!(result.expanded.contains("FILES += other"));
    assert!(result.fragments[0].has_conditionals);
    assert!(!result.fragments[0].plain_source_list);
    assert_eq!(result.fragments[0].assigned_variables, ["FILES"]);
}

#[test]
fn nested_local_fragments_are_atomic_and_keep_provenance() {
    let tree = TempDir::new().unwrap();
    write(
        &tree.path().join("module/outer.list"),
        "OUTER := one\ninclude $(SRCDIR)/$(CURDIR)/inner.list\nOUTER += three\n",
    );
    write(&tree.path().join("module/inner.list"), "INNER := two\n");
    let source = "include $(SRCDIR)/$(CURDIR)/outer.list\n%build_linklib files=$(OUTER)\n";

    let result = scan_scopes(tree.path(), "module/mmakefile.src", source);

    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert!(result.expanded.contains("INNER := two"));
    assert_eq!(result.fragments.len(), 2);
    assert_eq!(result.fragments[0].path, Path::new("module/outer.list"));
    assert_eq!(result.fragments[1].path, Path::new("module/inner.list"));
    assert_eq!(
        result.fragments[1].included_from,
        Path::new("module/outer.list")
    );

    write(
        &tree.path().join("module/inner.list"),
        "generated.h: input.idl\n\ttool input.idl > generated.h\n",
    );
    let rejected = scan_scopes(tree.path(), "module/mmakefile.src", source);
    assert!(!rejected.expanded.contains("OUTER := one"));
    assert!(rejected.fragments.is_empty());
    assert!(rejected
        .issues
        .iter()
        .any(|item| item.kind == LocalMakeIncludeIssueKind::UnsafeSyntax));
}

#[test]
fn rules_recipes_metamake_and_side_effects_are_rejected_and_reported() {
    let cases = [
        ("output: input\n", "output: input"),
        ("\ttool input > output\n", "tool input > output"),
        ("#MM- root : dependency\n", "#MM- root : dependency"),
        ("%build_prog mmake=hidden files=main\n", "%build_prog"),
        ("FILES := $(shell find . -name '*.c')\n", "shell"),
        ("FILES := $(shell)\n", "shell"),
        ("FILES := $(eval MORE := hidden)\n", "eval"),
    ];

    for (fragment, expected) in cases {
        let tree = TempDir::new().unwrap();
        write(&tree.path().join("module/unsafe.list"), fragment);
        let source = "include $(SRCDIR)/$(CURDIR)/unsafe.list\n%build_linklib files=$(FILES)\n";
        let result = scan(tree.path(), "module/mmakefile.src", source);
        assert!(!result.expanded.contains(fragment.trim()), "{fragment}");
        assert!(result.fragments.is_empty());
        assert!(
            result.issues.iter().any(|item| {
                item.kind == LocalMakeIncludeIssueKind::UnsafeSyntax
                    && (item.subject.contains(expected) || item.detail.contains(expected))
            }),
            "{fragment}: {:?}",
            result.issues
        );
    }
}

#[test]
fn unresolved_missing_and_out_of_tree_paths_are_not_silenced() {
    let tree = TempDir::new().unwrap();
    write(&tree.path().join("outside.list"), "FILES := escaped\n");
    write(&tree.path().join("source/module/placeholder"), "x\n");
    let root = tree.path().join("source");
    let source = "include $(SRCDIR)/$(CURDIR)/$(LIST)\n\
include $(SRCDIR)/$(CURDIR)/missing.list\n\
include $(SRCDIR)/$(CURDIR)/../../outside.list\n";

    let result = scan(&root, "module/mmakefile.src", source);

    assert!(result.fragments.is_empty());
    assert_eq!(result.expanded, source);
    assert!(result
        .issues
        .iter()
        .any(|item| item.kind == LocalMakeIncludeIssueKind::UnresolvedPath));
    assert!(result
        .issues
        .iter()
        .any(|item| item.kind == LocalMakeIncludeIssueKind::Missing));
    assert!(result
        .issues
        .iter()
        .any(|item| item.kind == LocalMakeIncludeIssueKind::OutsideSourceTree));
}

#[test]
fn a_symlink_escape_is_rejected_after_canonicalization() {
    let tree = TempDir::new().unwrap();
    let root = tree.path().join("source");
    write(&root.join("module/placeholder"), "x\n");
    write(&tree.path().join("outside.list"), "FILES := escaped\n");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        tree.path().join("outside.list"),
        root.join("module/link.list"),
    )
    .unwrap();

    let source = "include $(SRCDIR)/$(CURDIR)/link.list\n";
    let result = scan(&root, "module/mmakefile.src", source);

    assert!(result.fragments.is_empty());
    assert!(result
        .issues
        .iter()
        .any(|item| item.kind == LocalMakeIncludeIssueKind::OutsideSourceTree));
}

#[test]
fn nested_non_local_scope_and_cycles_reject_the_parent_atomically() {
    let tree = TempDir::new().unwrap();
    write(
        &tree.path().join("module/outer.list"),
        "OUTER := one\ninclude $(TOP)/generated.cfg\n",
    );
    let source = "include $(SRCDIR)/$(CURDIR)/outer.list\n";
    let non_local = scan_scopes(tree.path(), "module/mmakefile.src", source);
    assert!(!non_local.expanded.contains("OUTER := one"));
    assert!(non_local.fragments.is_empty());
    assert!(non_local
        .issues
        .iter()
        .any(|item| item.kind == LocalMakeIncludeIssueKind::NestedNonLocalInclude));

    write(
        &tree.path().join("module/outer.list"),
        "OUTER := one\ninclude $(SRCDIR)/$(CURDIR)/inner.list\n",
    );
    write(
        &tree.path().join("module/inner.list"),
        "INNER := two\ninclude $(SRCDIR)/$(CURDIR)/outer.list\n",
    );
    let cyclic = scan_scopes(tree.path(), "module/mmakefile.src", source);
    assert!(!cyclic.expanded.contains("OUTER := one"));
    assert!(cyclic.fragments.is_empty());
    assert!(cyclic
        .issues
        .iter()
        .any(|item| item.kind == LocalMakeIncludeIssueKind::Cycle));
}

#[test]
fn depth_file_and_byte_limits_are_enforced() {
    let tree = TempDir::new().unwrap();
    write(
        &tree.path().join("module/one.list"),
        "ONE := 1\ninclude $(SRCDIR)/$(CURDIR)/two.list\n",
    );
    write(&tree.path().join("module/two.list"), "TWO := 2\n");
    let source = "include $(SRCDIR)/$(CURDIR)/one.list\n";

    let depth = inline_local_make_includes(
        source,
        tree.path(),
        Path::new("module/mmakefile.src"),
        LocalMakeIncludeLimits {
            depth: 1,
            ..LocalMakeIncludeLimits::default()
        },
        LocalMakeFragmentPolicy::SafeVariableScopes,
    );
    assert!(depth
        .issues
        .iter()
        .any(|item| item.kind == LocalMakeIncludeIssueKind::DepthLimit));
    assert!(depth.fragments.is_empty());

    let files = inline_local_make_includes(
        source,
        tree.path(),
        Path::new("module/mmakefile.src"),
        LocalMakeIncludeLimits {
            files: 1,
            ..LocalMakeIncludeLimits::default()
        },
        LocalMakeFragmentPolicy::SafeVariableScopes,
    );
    assert!(files
        .issues
        .iter()
        .any(|item| item.kind == LocalMakeIncludeIssueKind::FileLimit));

    let bytes = inline_local_make_includes(
        source,
        tree.path(),
        Path::new("module/mmakefile.src"),
        LocalMakeIncludeLimits {
            bytes: 2,
            ..LocalMakeIncludeLimits::default()
        },
        LocalMakeFragmentPolicy::SafeVariableScopes,
    );
    assert!(bytes
        .issues
        .iter()
        .any(|item| item.kind == LocalMakeIncludeIssueKind::ByteLimit));
}

#[test]
fn non_local_include_families_are_left_for_their_existing_collectors() {
    let tree = TempDir::new().unwrap();
    let source = "include $(SRCDIR)/config/aros.cfg\n\
-include $(GENDIR)/module/generated.cfg\n\
-include $(SRCDIR)/arch/$(CPU)-$(ARCH)/timer/make.opts\n\
-include $(SRCDIR)/$(CURDIR)/make.opts\n";

    let result = scan(tree.path(), "module/mmakefile.src", source);

    assert_eq!(result.expanded, source);
    assert!(result.fragments.is_empty());
    assert!(result.issues.is_empty());
}

#[test]
fn real_btcore_fragment_is_a_generic_28_source_assignment() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("../../../..").canonicalize().unwrap();
    let mmake = Path::new("rom/bluetooth/stack/mmakefile.src");
    let content = fs::read_to_string(root.join(mmake)).unwrap();

    let result = scan(&root, mmake.to_str().unwrap(), &content);

    assert!(result.issues.is_empty(), "{:?}", result.issues);
    assert_eq!(result.fragments.len(), 1);
    let fragment = &result.fragments[0];
    assert_eq!(fragment.path, Path::new("rom/bluetooth/stack/core.files"));
    assert_eq!(fragment.assigned_variables, ["BTCORE_FILES"]);
    assert!(!fragment.has_conditionals);
    assert!(fragment.plain_source_list);

    let assignment = result.expanded.find("BTCORE_FILES :=").unwrap();
    let declaration = result
        .expanded
        .find("%build_linklib mmake=linklibs-btcore")
        .unwrap();
    assert!(assignment < declaration);
    let body = fs::read_to_string(root.join(&fragment.path)).unwrap();
    let source_stems = body
        .lines()
        .skip(1)
        .map(str::trim)
        .map(|line| line.trim_end_matches('\\').trim())
        .filter(|line| !line.is_empty() && !line.contains(":="))
        .collect::<Vec<_>>();
    assert_eq!(source_stems.len(), 28);
    assert!(source_stems.contains(&"core/buffer/endian"));
    assert!(source_stems.contains(&"aros/input_bridge"));
}

#[test]
fn broad_or_recipe_bearing_real_fragments_remain_reported_and_unexpanded() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("../../../..").canonicalize().unwrap();
    let cases = [
        (
            "workbench/libs/z/mmakefile.src",
            "ARCHSRCDIR :=",
            "workbench/libs/z/make.opt",
            LocalMakeIncludeIssueKind::DeferredScope,
        ),
        (
            "workbench/devs/networks/atheros5000/hal/mmakefile.src",
            "HAL_OBJS=",
            "workbench/devs/networks/atheros5000/hal/Makefile.inc",
            LocalMakeIncludeIssueKind::UnsafeSyntax,
        ),
    ];

    for (mmake, fragment_marker, fragment_path, expected_kind) in cases {
        let content = fs::read_to_string(root.join(mmake)).unwrap();
        let result = scan(&root, mmake, &content);
        assert!(!result.expanded.contains(fragment_marker), "{mmake}");
        assert!(result.fragments.is_empty(), "{mmake}");
        assert!(
            result.issues.iter().any(|item| {
                item.kind == expected_kind
                    && (item.source == Path::new(fragment_path)
                        || item.subject.contains(
                            Path::new(fragment_path)
                                .file_name()
                                .unwrap()
                                .to_str()
                                .unwrap(),
                        ))
            }),
            "{mmake}: {:?}",
            result.issues
        );
    }
}

#[test]
fn strict_tree_inventory_enables_only_audited_plain_source_fragments() {
    fn visit(dir: &Path, files: &mut Vec<PathBuf>) {
        let mut entries = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                if !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "build" | "target")
                ) {
                    visit(&path, files);
                }
            } else if matches!(
                entry.file_name().to_str(),
                Some("mmakefile.src" | "mmakefile")
            ) {
                files.push(path);
            }
        }
    }

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("../../../..").canonicalize().unwrap();
    let mut files = Vec::new();
    visit(&root, &mut files);
    let mut accepted = Vec::new();
    for path in files {
        let relative = path.strip_prefix(&root).unwrap();
        let bytes = fs::read(&path).unwrap();
        let content = String::from_utf8_lossy(&bytes);
        let result = scan(&root, relative.to_str().unwrap(), &content);
        accepted.extend(
            result
                .fragments
                .into_iter()
                .map(|fragment| format!("{} -> {}", relative.display(), fragment.path.display())),
        );
    }
    accepted.sort();

    assert_eq!(
        accepted,
        [
            "rom/bluetooth/stack/mmakefile.src -> rom/bluetooth/stack/core.files",
            "workbench/libs/zstd/mmakefile.src -> workbench/libs/zstd/zstd-1.5.7.files",
        ]
    );
}
