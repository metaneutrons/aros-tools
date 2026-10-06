//! Source-owned GNU release-rule probes; these do not build a compiler.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const RELEASE_RULE: &str = concat!(
    "ifeq ($(AROS_TOOLCHAIN_RELEASE),1)\n",
    "crosstools-release: tools\n",
    "\t@+$(CALL) $(MMAKE) $(MMAKE_OPTIONS) AROS.tools-crosstools-gnu-release\n",
    "else\n",
    "crosstools-release:\n",
    "\t@$(ECHO) 'crosstools-release requires --enable-toolchain-release' >&2\n",
    "\t@exit 1\n",
    "endif\n",
    ".PHONY: crosstools-release\n",
);

const CROSSTOOLS_RULE: &str =
    "crosstools : crosstools-toolchain features\n\t@$(MAKE) $(MKARGS) toolchain-linklibs\n";

const META_GRAPH_SOURCE: &str = "#MM- tools-crosstools-gnu-release : tools-crosstools-gcc-libgcc\n\
                                  #MM tools-crosstools-gcc-libgcc : tools-crosstools-gcc";

const META_GRAPH_RULES: &str = "tools-crosstools-gnu-release : tools-crosstools-gcc-libgcc\n\
                                tools-crosstools-gcc-libgcc : tools-crosstools-gcc\n";

const LIBGCC_RULE: &str = concat!(
    "tools-crosstools-gcc-libgcc :\n",
    "\t$(MAKE) -C $(HOSTGENDIR)/$(CURDIR)/gcc all-target-libgcc $(crosstools-gcc-make-env)\n",
    "\t$(MAKE) -j1 -C $(HOSTGENDIR)/$(CURDIR)/gcc install-target-libgcc $(crosstools-gcc-install-env)",
);

const RELEASE_MMAKE_ROOT: &str = "AROS.tools-crosstools-gnu-release";
const GENERIC_MMAKE_ROOT: &str = "AROS.tools-crosstools-gnu-riscv";

fn source_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("AROS_TEST_P4_SOURCE")
            .expect("AROS_TEST_P4_SOURCE selects the actual AROS source tree"),
    )
    .canonicalize()
    .expect("AROS_TEST_P4_SOURCE exists")
}

fn read_source(root: &Path, file: &str) -> String {
    fs::read_to_string(root.join(file)).unwrap_or_else(|error| {
        panic!(
            "cannot read source-owned {file} under {}: {error}",
            root.display()
        )
    })
}

fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_at = source
        .find(start)
        .unwrap_or_else(|| panic!("missing source contract start marker: {start}"));
    let tail = &source[start_at..];
    let end_at = tail
        .find(end)
        .unwrap_or_else(|| panic!("missing source contract end marker: {end}"));
    &tail[..end_at]
}

fn shell_list<'a>(source: &'a str, name: &str) -> &'a str {
    let marker = format!("{name}='");
    let start = source
        .find(&marker)
        .unwrap_or_else(|| panic!("generated configure omits {name}"))
        + marker.len();
    let tail = &source[start..];
    let end = tail
        .find("'\n")
        .unwrap_or_else(|| panic!("generated configure has unterminated {name}"));
    &tail[..end]
}

fn source_owned_release_rule(makefile: &str) -> &str {
    let start_marker = "ifeq ($(AROS_TOOLCHAIN_RELEASE),1)\n";
    let end_marker = ".PHONY: crosstools-release\n";
    let start = makefile
        .find(start_marker)
        .expect("source-owned opt-in rule starts with its release selector");
    let end = start
        + makefile[start..]
            .find(end_marker)
            .expect("source-owned release rule declares its target phony")
        + end_marker.len();
    &makefile[start..end]
}

fn release_meta_source(mmakefile: &str) -> &str {
    section(mmakefile, "#MM- tools-crosstools-gnu-release", "\n\n")
}

fn release_meta_rules(mmakefile: &str) -> String {
    let mut rules = String::new();
    for line in release_meta_source(mmakefile).lines() {
        let declaration = line
            .strip_prefix("#MM-")
            .or_else(|| line.strip_prefix("#MM"))
            .expect("release graph contains MetaMake declarations")
            .trim();
        rules.push_str(declaration);
        rules.push('\n');
    }
    rules
}

fn source_libgcc_rule(mmakefile: &str) -> &str {
    section(mmakefile, "tools-crosstools-gcc-libgcc :\n", "\n\n")
}

fn gnu_make() -> PathBuf {
    for candidate in ["gmake", "make"] {
        let Ok(path) = which::which(candidate) else {
            continue;
        };
        let Ok(version) = Command::new(&path).arg("--version").output() else {
            continue;
        };
        if String::from_utf8_lossy(&version.stdout)
            .lines()
            .next()
            .is_some_and(|line| line.contains("GNU Make"))
        {
            return path;
        }
    }
    panic!("this ignored source probe requires GNU make (gmake or make)");
}

fn fixture_makefile(release_rule: &str, mode: u8) -> String {
    format!(
        "AROS_TOOLCHAIN_RELEASE := {mode}\n\
         ECHO := echo\n\
         CALL :=\n\
         MMAKE := ./mmake-sentinel\n\
         MMAKE_OPTIONS :=\n\
         MKARGS := --no-print-directory -f Makefile\n\
         \n\
         .PHONY: all sdk default tools crosstools-toolchain features broad-sdk toolchain-linklibs\n\
         all:\n\
         \t@printf 'broad-sdk\\n' >> sentinels.log\n\
         sdk:\n\
         \t@printf 'broad-sdk\\n' >> sentinels.log\n\
         default:\n\
         \t@printf 'default\\n' >> sentinels.log\n\
         tools:\n\
         \t@printf 'tools\\n' >> sentinels.log\n\
         crosstools-toolchain:\n\
         \t@printf 'broad-toolchain\\n' >> sentinels.log\n\
         features:\n\
         \t@printf 'features\\n' >> sentinels.log\n\
         broad-sdk:\n\
         \t@printf 'broad-sdk\\n' >> sentinels.log\n\
         toolchain-linklibs:\n\
         \t@printf 'toolchain-linklibs\\n' >> sentinels.log\n\
         .DEFAULT:\n\
         \t@printf 'default:%s\\n' '$@' >> sentinels.log\n\
         \t@exit 97\n\
         \n\
         {release_rule}\n\
         {CROSSTOOLS_RULE}",
    )
}

const MMAKE_SENTINEL: &str = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> mmake-roots.log
case "$*" in
  *AROS.tools-crosstools-gnu-release*)
    printf '%s\n' release-root >> sentinels.log ;;
  *AROS.tools-crosstools-gnu-riscv*)
    printf '%s\n' generic-root broad-includes all-target-runtimes >> sentinels.log ;;
  *)
    printf '%s\n' default-mmake-root >> sentinels.log
    exit 97 ;;
esac
"#;

fn run_make(make: &Path, root: &Path, target: &str) -> Output {
    Command::new(make)
        .current_dir(root)
        .args(["--no-print-directory", "-f", "Makefile", target])
        .output()
        .expect("GNU make runs the isolated source-rule fixture")
}

fn sentinels(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("sentinels.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn write_fixture(root: &Path, release_rule: &str, mode: u8) {
    fs::write(root.join("Makefile"), fixture_makefile(release_rule, mode))
        .expect("write isolated GNU make fixture");
    let sentinel = root.join("mmake-sentinel");
    fs::write(&sentinel, MMAKE_SENTINEL).expect("write MMAKE root sentinel");
    let mut permissions = fs::metadata(&sentinel).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(sentinel, permissions).unwrap();
}

fn meta_fixture_makefile(meta_rules: &str, libgcc_rule: &str) -> String {
    let fixture_rule = libgcc_rule.replace("$(HOSTGENDIR)/$(CURDIR)/gcc", "$(GCC_FIXTURE)");
    assert_eq!(
        fixture_rule.matches("$(GCC_FIXTURE)").count(),
        2,
        "only the source path is rebound into the fake GCC tree"
    );
    format!(
        "GCC_FIXTURE := $(CURDIR)/gcc\n\
         GRAPH_LOG := graph.log\n\
         .PHONY: tools-crosstools-gnu-release tools-crosstools-gcc-libgcc tools-crosstools-gcc\n\
         {meta_rules}\n\
         tools-crosstools-gcc:\n\
         \t@printf 'compiler\\n' >> $(GRAPH_LOG)\n\
         {fixture_rule}\n\
         .DEFAULT:\n\
         \t@printf 'default:%s\\n' '$@' >> $(GRAPH_LOG)\n\
         \t@exit 97\n"
    )
}

fn write_meta_fixture(root: &Path, meta_rules: &str, libgcc_rule: &str) -> PathBuf {
    fs::write(
        root.join("Makefile"),
        meta_fixture_makefile(meta_rules, libgcc_rule),
    )
    .expect("write isolated MetaMake-graph fixture");
    let gcc = root.join("gcc");
    fs::create_dir(&gcc).unwrap();
    fs::write(
        gcc.join("Makefile"),
        "GOALS_LOG := child-goals.log\n\
         .PHONY: all-target-libgcc install-target-libgcc\n\
         all-target-libgcc:\n\
         \t@printf 'all-target-libgcc\\n' >> $(GOALS_LOG)\n\
         install-target-libgcc:\n\
         \t@printf 'install-target-libgcc\\n' >> $(GOALS_LOG)\n\
         .DEFAULT:\n\
         \t@printf 'unexpected:%s\\n' '$@' >> $(GOALS_LOG)\n\
         \t@exit 97\n",
    )
    .expect("write fake child GCC Makefile");
    gcc
}

#[test]
#[ignore = "requires AROS_TEST_P4_SOURCE; executes only extracted GNU make rules, not a compiler build"]
fn actual_source_gnu_release_is_opt_in_and_keeps_the_legacy_crosstools_closure() {
    let source = source_root();
    let configure_in = read_source(&source, "configure.in");
    let configure = read_source(&source, "configure");
    let makefile = read_source(&source, "Makefile.in");
    let gnu_mmakefile = read_source(&source, "tools/crosstools/gnu/mmakefile.src");

    let configure_contract = section(
        &configure_in,
        "AC_MSG_CHECKING([whether to build the GNU compiler-only release closure])",
        "\nif test \"${crosstools}\" = \"yes\"; then",
    );
    for required in [
        "AC_ARG_ENABLE([toolchain-release],",
        "[enable_toolchain_release=no])",
        "yes) aros_toolchain_release=1 ;;",
        "no)  aros_toolchain_release=0 ;;",
        "*)   AC_MSG_ERROR([--enable-toolchain-release accepts only yes or no]) ;;",
        "if test \"$crosstools\" != \"yes\"; then",
        "if test \"$aros_toolchain\" != \"gnu\"; then",
    ] {
        assert!(
            configure_contract.contains(required),
            "configure.in release contract is missing {required:?}"
        );
    }
    assert_eq!(
        configure_in
            .matches("AC_SUBST(aros_toolchain_release)")
            .count(),
        1,
        "configure.in must export the validated switch once"
    );

    let generated_contract = section(
        &configure,
        "# Check whether --enable-toolchain-release was given.",
        "\nif test \"${crosstools}\" = \"yes\"; then",
    );
    for required in [
        "e) enable_toolchain_release=no ;;",
        "yes) aros_toolchain_release=1 ;;",
        "no)  aros_toolchain_release=0 ;;",
        "*)   as_fn_error $? \"--enable-toolchain-release accepts only yes or no\" \"$LINENO\" 5 ;;",
        "if test \"$crosstools\" != \"yes\"; then",
        "if test \"$aros_toolchain\" != \"gnu\"; then",
    ] {
        assert!(
            generated_contract.contains(required),
            "generated configure is missing {required:?}"
        );
    }
    assert!(
        shell_list(&configure, "ac_user_opts")
            .lines()
            .any(|option| option == "enable_toolchain_release"),
        "generated configure must register the option"
    );
    assert!(
        shell_list(&configure, "ac_subst_vars")
            .lines()
            .any(|variable| variable == "aros_toolchain_release"),
        "generated configure must export the substitution"
    );
    assert_eq!(
        makefile
            .lines()
            .filter(|line| *line == "AROS_TOOLCHAIN_RELEASE := @aros_toolchain_release@")
            .count(),
        1,
        "Makefile.in must substitute the configure result directly"
    );

    let release_rule = source_owned_release_rule(&makefile);
    assert_eq!(
        release_rule, RELEASE_RULE,
        "release closure must be disabled by default and call only the source-owned GNU release root"
    );
    assert_eq!(
        makefile.matches(CROSSTOOLS_RULE).count(),
        1,
        "ordinary crosstools must retain its feature and toolchain-linklibs path"
    );
    assert_eq!(
        release_meta_source(&gnu_mmakefile),
        META_GRAPH_SOURCE,
        "the release MetaMake root has a closed compiler/libgcc dependency chain"
    );
    let meta_rules = release_meta_rules(&gnu_mmakefile);
    assert_eq!(meta_rules, META_GRAPH_RULES);
    let libgcc_rule = source_libgcc_rule(&gnu_mmakefile);
    assert_eq!(
        libgcc_rule, LIBGCC_RULE,
        "libgcc must use the two explicit target goals with serial installation"
    );
    assert!(
        gnu_mmakefile.contains("#MM- tools-crosstools-gnu-riscv"),
        "negative control expects the generic GNU architecture root"
    );
    assert!(
        gnu_mmakefile.contains("#MM crosstools-gcc : includes-copy"),
        "negative control expects the generic runtime root's copied include edge"
    );

    let make = gnu_make();
    let disabled = tempfile::tempdir().unwrap();
    write_fixture(disabled.path(), release_rule, 0);
    let result = run_make(&make, disabled.path(), "crosstools-release");
    assert!(
        !result.status.success(),
        "disabled release target succeeded"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("crosstools-release requires --enable-toolchain-release"),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        sentinels(disabled.path()).is_empty(),
        "disabled target reached a prerequisite, broad SDK, or default recipe"
    );

    let enabled = tempfile::tempdir().unwrap();
    write_fixture(enabled.path(), release_rule, 1);
    let result = run_make(&make, enabled.path(), "crosstools-release");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(sentinels(enabled.path()), ["tools", "release-root"]);
    assert_eq!(
        fs::read_to_string(enabled.path().join("mmake-roots.log"))
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [RELEASE_MMAKE_ROOT]
    );

    // Counterprobe the allowlist if features are attached to the release
    // target; the prerequisite sentinel must make that broadened closure clear.
    let features_rule = release_rule.replace(
        "crosstools-release: tools\n",
        "crosstools-release: tools features\n",
    );
    assert_ne!(features_rule, release_rule);
    let features_counterprobe = tempfile::tempdir().unwrap();
    write_fixture(features_counterprobe.path(), &features_rule, 1);
    let result = run_make(&make, features_counterprobe.path(), "crosstools-release");
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        sentinels(features_counterprobe.path()),
        ["tools", "features", "release-root"]
    );

    let mmake_graph = tempfile::tempdir().unwrap();
    let child_gcc = write_meta_fixture(mmake_graph.path(), &meta_rules, libgcc_rule);
    let result = run_make(&make, mmake_graph.path(), "tools-crosstools-gnu-release");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read_to_string(mmake_graph.path().join("graph.log"))
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["compiler"]
    );
    assert_eq!(
        fs::read_to_string(child_gcc.join("child-goals.log"))
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["all-target-libgcc", "install-target-libgcc"]
    );

    // Counterprobe the top-level rule with the broad architecture root. Its
    // include and runtime sentinels prove that an old root would be detected.
    let broad_root_rule = release_rule.replace(RELEASE_MMAKE_ROOT, GENERIC_MMAKE_ROOT);
    assert_ne!(
        broad_root_rule, release_rule,
        "counterprobe must alter the root"
    );
    let broad_root = tempfile::tempdir().unwrap();
    write_fixture(broad_root.path(), &broad_root_rule, 1);
    let result = run_make(&make, broad_root.path(), "crosstools-release");
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        sentinels(broad_root.path()),
        [
            "tools",
            "generic-root",
            "broad-includes",
            "all-target-runtimes"
        ]
    );

    // Removing the explicit target exposes the top-level .DEFAULT fallback.
    let default_fallback = tempfile::tempdir().unwrap();
    write_fixture(default_fallback.path(), "", 0);
    let result = run_make(&make, default_fallback.path(), "crosstools-release");
    assert!(!result.status.success());
    assert_eq!(
        sentinels(default_fallback.path()),
        ["default:crosstools-release"]
    );

    let ordinary = tempfile::tempdir().unwrap();
    write_fixture(ordinary.path(), release_rule, 0);
    let result = run_make(&make, ordinary.path(), "crosstools");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        sentinels(ordinary.path()),
        ["broad-toolchain", "features", "toolchain-linklibs"]
    );
}
