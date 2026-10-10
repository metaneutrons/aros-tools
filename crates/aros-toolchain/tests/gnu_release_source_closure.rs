//! Source-owned GNU release-rule probes; these do not build a compiler.

#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const ORDINARY_CROSSTOOLS_RULE: &str =
    "crosstools : crosstools-toolchain features\n\t@$(MAKE) $(MKARGS) toolchain-linklibs\n";

const GENERIC_TOOLCHAIN_ROOT: &str =
    "tools-crosstools : tools-crosstools-$(AROS_TOOLCHAIN)-$(AROS_TARGET_CPU) tools-crosstools-rust-$(TARGET_RUST)";

const GNU_RISCV_SOURCE_ROOT: &str =
    "tools-crosstools-gnu-riscv : tools-crosstools-gcc crosstools-gcc";

const GNU_GCC_PACKAGE_ROOT: &str = "crosstools-gcc : sdk-includes-$(AROS_TOOLCHAIN_RELEASE)";

const RELEASE_LINKLIBS_ROOT: &str = "toolchain-linklibs-release : linklibs-atomic toolchain-linklibs-$(AROS_TOOLCHAIN)-release-$(AROS_TARGET_CPU) toolchain-linklibs-$(AROS_TOOLCHAIN)-release";

const NORMAL_LINKLIBS_ROOT: &str = "toolchain-linklibs : linklibs-atomic toolchain-linklibs-$(AROS_TOOLCHAIN)-$(AROS_TARGET_CPU) toolchain-linklibs-$(AROS_TOOLCHAIN) toolchain-linklibs-$(AROS_TARGET_CPU)";

fn source_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os("AROS_TEST_P4_SOURCE")
            .expect("AROS_TEST_P4_SOURCE selects the actual AROS source tree"),
    )
    .canonicalize()
    .expect("AROS_TEST_P4_SOURCE exists")
}

fn read_source(root: &Path, file: &str) -> String {
    let bytes = fs::read(root.join(file)).unwrap_or_else(|error| {
        panic!(
            "cannot read source-owned {file} under {}: {error}",
            root.display()
        )
    });
    match String::from_utf8(bytes) {
        Ok(source) => source,
        // Some historical source files contain ISO-8859-1 comments. Keep
        // their byte values readable without changing the source recipes;
        // all contract markers used here are ASCII.
        Err(error) => error.into_bytes().into_iter().map(char::from).collect(),
    }
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

fn source_target_rule<'a>(source: &'a str, target: &str) -> &'a str {
    let mut offset = 0;
    let start = source
        .split_inclusive('\n')
        .find_map(|line| {
            let content = line.strip_suffix('\n').unwrap_or(line);
            let is_target = content.strip_prefix(target).is_some_and(|rest| {
                rest.starts_with(':') || rest.starts_with(' ') || rest.starts_with('\t')
            });
            let line_start = offset;
            offset += line.len();
            is_target.then_some(line_start)
        })
        .unwrap_or_else(|| panic!("missing source-owned make target: {target}"));
    let tail = &source[start..];
    let end = tail.find("\n\n").map_or(tail.len(), |at| at + 1);
    &tail[..end]
}

fn meta_rule(source: &str, target: &str) -> String {
    let lines = source.lines().collect::<Vec<_>>();
    let declarations = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let declaration = line
                .strip_prefix("#MM-")
                .or_else(|| line.strip_prefix("#MM"))?
                .trim();
            let (name, _) = declaration.split_once(':')?;
            (name.trim() == target).then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        declarations.len(),
        1,
        "expected exactly one MetaMake declaration for {target}, found {}",
        declarations.len()
    );

    let mut pieces = Vec::new();
    for line in &lines[declarations[0]..] {
        let Some(declaration) = line
            .strip_prefix("#MM-")
            .or_else(|| line.strip_prefix("#MM"))
        else {
            break;
        };
        let declaration = declaration.trim();
        let continued = declaration.ends_with('\\');
        pieces.push(
            declaration
                .trim_end_matches('\\')
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        );
        if !continued {
            return pieces.join(" ");
        }
    }
    panic!("missing complete MetaMake declaration for {target}");
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

fn fixture_makefile(variables: &str, source_rules: &str, fixture_rules: &str) -> String {
    format!(
        "{variables}\n\
         LOG := sentinels.log\n\
         .PHONY: all tools features crosstools-toolchain crosstools crosstools-release \\\n          toolchain-linklibs toolchain-linklibs-release tools-crosstools AROS.tools-crosstools \\\n          tools-crosstools-gnu-riscv tools-crosstools-gcc crosstools-gcc tools-crosstools-rust-no \\\n          linklibs-atomic linklibs-libatomic linklibs-libatomic-yes linklibs-gnu-libatomic \\\n          tools-crosstools-gcc-libatomic crosstools-gcc--fetch tools-crosstools-autolibs \\\n          gnu-libatomic-cpu-linklibs-0 gnu-libatomic-cpu-linklibs-1 \\\n          toolchain-linklibs-gnu-release-riscv toolchain-linklibs-gnu-release \\\n          toolchain-linklibs-gnu-riscv toolchain-linklibs-gnu toolchain-linklibs-riscv \\\n          sdk-includes-0 sdk-includes-1 linklibs-riscv core-linklibs \\\n          gnu-libatomic-linklibs-0 gnu-libatomic-linklibs-1\n\
         {source_rules}\n\
         {fixture_rules}\n\
         .DEFAULT:\n\
         \t@printf 'unexpected:%s\\n' '$@' >> $(LOG)\n\
         \t@exit 97\n"
    )
}

fn write_fixture(root: &Path, makefile: &str) {
    fs::write(root.join("Makefile"), makefile).expect("write isolated GNU make fixture");
    fs::write(root.join("config.status"), "").expect("write fixture config.status");
}

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

fn fixture_variables(release: u8) -> String {
    format!(
        "TOP := .\n\
         CROSSTOOLSDIR := .\n\
         CROSSTOOLS_BUILDFLAG := ./.installflag-crosstools\n\
         AROS_TOOLCHAIN_DEPS :=\n\
         CROSSTOOLS_TARGET := tools-crosstools\n\
         AROS_TOOLCHAIN_RELEASE := {release}\n\
         AROS_TOOLCHAIN := gnu\n\
         AROS_TARGET_CPU := riscv\n\
         TARGET_RUST := no\n\
         TARGET_LIBATOMIC := yes\n\
         IF := if\n\
         TEST := test\n\
         ECHO := echo\n\
         TOUCH := touch\n\
         NOP := :\n\
         CALL :=\n\
         MMAKE := $(MAKE)\n\
         MMAKE_OPTIONS := --no-print-directory -f Makefile\n\
         MKARGS := --no-print-directory -f Makefile\n"
    )
}

const fn fixture_recipes() -> &'static str {
    "tools:\n\
     \t@printf 'tools\\n' >> $(LOG)\n\
     features:\n\
     \t@printf 'features\\n' >> $(LOG)\n\
     AROS.tools-crosstools: tools-crosstools\n\
     tools-crosstools-gcc:\n\
     \t@printf 'gnu-compiler-source\\n' >> $(LOG)\n\
     crosstools-gcc:\n\
     \t@printf 'gnu-compiler-package\\n' >> $(LOG)\n\
     tools-crosstools-rust-no:\n\
     \t@printf 'rust-disabled\\n' >> $(LOG)\n\
     sdk-includes-0:\n\
     \t@printf 'sdk-includes-classic\\n' >> $(LOG)\n\
     sdk-includes-1:\n\
     \t@printf 'sdk-includes-release\\n' >> $(LOG)\n\
     crosstools-gcc--fetch:\n\
     \t@printf 'gcc-fetch\\n' >> $(LOG)\n\
     tools-crosstools-autolibs:\n\
     \t@printf 'autolibs\\n' >> $(LOG)\n\
     gnu-libatomic-cpu-linklibs-0: linklibs-riscv\n\
     \t@printf 'libatomic-cpu-classic\\n' >> $(LOG)\n\
     gnu-libatomic-cpu-linklibs-1:\n\
     \t@printf 'libatomic-cpu-release\\n' >> $(LOG)\n\
     linklibs-riscv:\n\
     \t@printf 'classic-riscv-linklibs\\n' >> $(LOG)\n\
     core-linklibs:\n\
     \t@printf 'classic-core-linklibs\\n' >> $(LOG)\n\
     tools-crosstools-gcc-libatomic:\n\
     \t@printf 'gnu-libatomic-producer\\n' >> $(LOG)\n\
     toolchain-linklibs-gnu-release-riscv:\n\
     \t@printf 'selected-cpu-release-runtime\\n' >> $(LOG)\n\
     toolchain-linklibs-gnu-release:\n\
     \t@printf 'selected-toolchain-release-runtime\\n' >> $(LOG)\n\
     toolchain-linklibs-gnu-riscv:\n\
     \t@printf 'normal-toolchain-cpu-runtime\\n' >> $(LOG)\n\
     toolchain-linklibs-gnu:\n\
     \t@printf 'normal-toolchain-runtime\\n' >> $(LOG)\n\
     toolchain-linklibs-riscv:\n\
     \t@printf 'normal-cpu-runtime\\n' >> $(LOG)\n"
}

#[test]
#[ignore = "requires AROS_TEST_P4_SOURCE; executes extracted GNU make rules, not a compiler build"]
fn actual_source_gnu_release_keeps_producer_and_selected_runtime_closures_separate() {
    let source = source_root();
    let configure_in = read_source(&source, "configure.in");
    let configure = read_source(&source, "configure");
    let makefile = read_source(&source, "Makefile.in");
    let target_cfg = read_source(&source, "config/target.cfg.in");
    let generic_mmakefile = read_source(&source, "tools/crosstools/mmakefile.src");
    let gnu_mmakefile = read_source(&source, "tools/crosstools/gnu/mmakefile.src");

    let configure_contract = section(
        &configure_in,
        "AC_MSG_CHECKING([whether to build the minimal crosstools release closure])",
        "AC_MSG_RESULT($aros_toolchain_release)",
    );
    for required in [
        "AC_ARG_ENABLE([toolchain-release],",
        "[enable_toolchain_release=no])",
        "yes) aros_toolchain_release=1 ;;",
        "no)  aros_toolchain_release=0 ;;",
        "*)   AC_MSG_ERROR([--enable-toolchain-release accepts only yes or no]) ;;",
    ] {
        assert!(
            configure_contract.contains(required),
            "configure.in release contract is missing {required:?}"
        );
    }
    assert!(
        configure_in.contains(
            "if test \"$aros_toolchain_release\" = \"1\" && test \"${crosstools}\" != \"yes\"; then"
        ),
        "configure.in must require crosstools for the release producer"
    );
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
        "if test \"${crosstools}\" = \"yes\"; then",
    );
    for required in [
        "e) enable_toolchain_release=no ;;",
        "yes) aros_toolchain_release=1 ;;",
        "no)  aros_toolchain_release=0 ;;",
        "*)   as_fn_error $? \"--enable-toolchain-release accepts only yes or no\" \"$LINENO\" 5 ;;",
    ] {
        assert!(
            generated_contract.contains(required),
            "generated configure is missing {required:?}"
        );
    }
    assert!(configure.contains(
        "if test \"$aros_toolchain_release\" = \"1\" && test \"${crosstools}\" != \"yes\"; then"
    ));
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
        target_cfg
            .lines()
            .filter(|line| {
                line.split_once(":=").is_some_and(|(name, value)| {
                    name.trim() == "AROS_TOOLCHAIN_RELEASE"
                        && value.trim() == "@aros_toolchain_release@"
                })
            })
            .count(),
        1,
        "config/target.cfg.in must substitute the configure result"
    );

    let release_rule = source_target_rule(&makefile, "crosstools-release");
    assert_eq!(
        release_rule.lines().next(),
        Some("crosstools-release : crosstools-toolchain"),
        "release must depend on the toolchain producer only"
    );
    for required in [
        "$(AROS_TOOLCHAIN_RELEASE)\" = \"1\"",
        "$(MAKE) $(MKARGS) toolchain-linklibs-release",
        "crosstools-release requires configure --enable-toolchain-release",
    ] {
        assert!(
            release_rule.contains(required),
            "release rule is missing {required:?}"
        );
    }
    assert!(
        !release_rule.contains("features"),
        "producer-only release must not depend on the broad feature target"
    );
    assert_eq!(
        source_target_rule(&makefile, "crosstools"),
        ORDINARY_CROSSTOOLS_RULE,
        "ordinary crosstools must retain features and the normal linklib closure"
    );
    let toolchain_rule = source_target_rule(&makefile, "crosstools-toolchain");
    assert_eq!(
        toolchain_rule.lines().next(),
        Some("crosstools-toolchain: tools $(CROSSTOOLS_BUILDFLAG)"),
        "the release producer must enter through the ordinary toolchain target"
    );
    let installflag_rule =
        source_target_rule(&makefile, "$(CROSSTOOLSDIR)/.installflag-crosstools");
    assert!(
        installflag_rule.contains("AROS.$(CROSSTOOLS_TARGET)"),
        "the toolchain install flag must invoke the source-selected MetaMake root"
    );

    let generic_toolchain_root = meta_rule(&generic_mmakefile, "tools-crosstools");
    let gnu_riscv_source_root = meta_rule(&gnu_mmakefile, "tools-crosstools-gnu-riscv");
    let gnu_gcc_package_root = meta_rule(&gnu_mmakefile, "crosstools-gcc");
    assert_eq!(generic_toolchain_root, GENERIC_TOOLCHAIN_ROOT);
    assert_eq!(gnu_riscv_source_root, GNU_RISCV_SOURCE_ROOT);
    assert_eq!(gnu_gcc_package_root, GNU_GCC_PACKAGE_ROOT);
    assert!(
        gnu_mmakefile.contains("all-gcc $(crosstools-gcc--make-env)"),
        "the separate GNU source producer must build the compiler source root"
    );
    assert!(
        gnu_mmakefile.contains("install-gcc $(crosstools-gcc--install_opts)"),
        "the separate GNU source producer must install the compiler source root"
    );
    assert!(
        gnu_mmakefile
            .contains("%fetch_and_build mmake=crosstools-gcc package=gcc version=$(GCC_VERSION) compiler=host"),
        "the second GNU root must build the configured GCC package and its target runtimes"
    );
    assert!(
        gnu_mmakefile.contains("hostincludes=\"sdk-includes-$(AROS_TOOLCHAIN_RELEASE)\""),
        "the GCC package must use the release-selected source/include closure"
    );

    let release_linklibs_root = meta_rule(&generic_mmakefile, "toolchain-linklibs-release");
    let normal_linklibs_root = meta_rule(&generic_mmakefile, "toolchain-linklibs");
    assert_eq!(release_linklibs_root, RELEASE_LINKLIBS_ROOT);
    assert_eq!(normal_linklibs_root, NORMAL_LINKLIBS_ROOT);
    assert!(
        !release_linklibs_root.contains("toolchain-linklibs-$(AROS_TARGET_CPU)"),
        "the producer-only release graph must not fall back to the broad CPU linklib aggregate"
    );

    let atomic_roots = [
        meta_rule(&generic_mmakefile, "linklibs-atomic"),
        meta_rule(&generic_mmakefile, "linklibs-libatomic"),
        meta_rule(&generic_mmakefile, "linklibs-libatomic-yes"),
        meta_rule(&gnu_mmakefile, "linklibs-gnu-libatomic"),
        meta_rule(&gnu_mmakefile, "tools-crosstools-gcc-libatomic"),
        meta_rule(&gnu_mmakefile, "gnu-libatomic-cpu-linklibs-0"),
    ]
    .join("\n");
    assert!(
        gnu_mmakefile.contains("gnu-libatomic-cpu-linklibs-1 :"),
        "GNU release libatomic must keep its source-owned CPU runtime selector"
    );
    assert!(
        gnu_mmakefile.contains("#MM- gnu-libatomic-linklibs-1 : tools-crosstools-autolibs"),
        "GNU release libatomic must select the declared temporary SDK linklibs"
    );
    assert!(
        gnu_mmakefile
            .contains("targetlinklibs=\"gnu-libatomic-linklibs-$(AROS_TOOLCHAIN_RELEASE)\""),
        "GNU libatomic configure must consume the selected target linklib closure"
    );
    let classic_libatomic_runtime = meta_rule(&gnu_mmakefile, "gnu-libatomic-linklibs-0");
    let release_libatomic_runtime = meta_rule(&gnu_mmakefile, "gnu-libatomic-linklibs-1");
    assert_eq!(
        classic_libatomic_runtime,
        "gnu-libatomic-linklibs-0 : core-linklibs"
    );
    assert_eq!(
        release_libatomic_runtime,
        "gnu-libatomic-linklibs-1 : tools-crosstools-autolibs"
    );

    let make = gnu_make();
    let source_rules = [
        release_rule,
        source_target_rule(&makefile, "crosstools"),
        toolchain_rule,
        installflag_rule,
        generic_toolchain_root.as_str(),
        gnu_riscv_source_root.as_str(),
        gnu_gcc_package_root.as_str(),
        release_linklibs_root.as_str(),
        normal_linklibs_root.as_str(),
        atomic_roots.as_str(),
        classic_libatomic_runtime.as_str(),
        release_libatomic_runtime.as_str(),
    ]
    .join("\n");

    let disabled = tempfile::tempdir().unwrap();
    write_fixture(
        disabled.path(),
        &fixture_makefile(&fixture_variables(0), &source_rules, fixture_recipes()),
    );
    let result = run_make(&make, disabled.path(), "crosstools-release");
    assert!(
        !result.status.success(),
        "disabled release target succeeded"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("crosstools-release requires configure --enable-toolchain-release"),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        sentinels(disabled.path()),
        [
            "tools",
            "gnu-compiler-source",
            "sdk-includes-classic",
            "gnu-compiler-package",
            "rust-disabled"
        ],
        "the producer runs before the source-owned release-off gate; no runtime root runs"
    );

    // GNU source has no separate #MM declarations for these two generic
    // release selector leaves. Their fixture recipes test dispatch and edge
    // reachability; the crosstools-gcc package is the GNU runtime build root.
    let enabled = tempfile::tempdir().unwrap();
    write_fixture(
        enabled.path(),
        &fixture_makefile(&fixture_variables(1), &source_rules, fixture_recipes()),
    );
    let result = run_make(&make, enabled.path(), "crosstools-release");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        sentinels(enabled.path()),
        [
            "tools",
            "gnu-compiler-source",
            "sdk-includes-release",
            "gnu-compiler-package",
            "rust-disabled",
            "gcc-fetch",
            "autolibs",
            "libatomic-cpu-release",
            "gnu-libatomic-producer",
            "selected-cpu-release-runtime",
            "selected-toolchain-release-runtime"
        ],
        "release enters the selected GNU source/package root, then every runtime dispatch edge"
    );

    // An unexpected concrete dependency must reach .DEFAULT and fail before
    // the release recipe can enter any selected runtime closure.
    let unexpected_dependency_rule = release_rule.replace(
        "crosstools-release : crosstools-toolchain",
        "crosstools-release : crosstools-toolchain unimplemented-gnu-runtime",
    );
    assert_ne!(unexpected_dependency_rule, release_rule);
    let unexpected_dependency = tempfile::tempdir().unwrap();
    let counterprobe_rules = source_rules.replacen(release_rule, &unexpected_dependency_rule, 1);
    write_fixture(
        unexpected_dependency.path(),
        &fixture_makefile(
            &fixture_variables(1),
            &counterprobe_rules,
            fixture_recipes(),
        ),
    );
    let result = run_make(&make, unexpected_dependency.path(), "crosstools-release");
    assert!(
        !result.status.success(),
        "unexpected dependency was accepted"
    );
    let unexpected_dependency_log = sentinels(unexpected_dependency.path());
    assert!(unexpected_dependency_log.contains(&"unexpected:unimplemented-gnu-runtime".to_owned()));
    for runtime_sentinel in [
        "gcc-fetch",
        "autolibs",
        "libatomic-cpu-release",
        "gnu-libatomic-producer",
        "selected-cpu-release-runtime",
        "selected-toolchain-release-runtime",
    ] {
        assert!(
            !unexpected_dependency_log.contains(&runtime_sentinel.to_owned()),
            "runtime sentinel {runtime_sentinel} ran despite the .DEFAULT rejection"
        );
    }

    // Adding the broad feature aggregate to the source target must be visible
    // in the fixture; the actual release target above has no such edge.
    let feature_counterprobe_rule = release_rule.replace(
        "crosstools-release : crosstools-toolchain",
        "crosstools-release : crosstools-toolchain features",
    );
    assert_ne!(feature_counterprobe_rule, release_rule);
    let feature_counterprobe = tempfile::tempdir().unwrap();
    let counterprobe_rules = source_rules.replacen(release_rule, &feature_counterprobe_rule, 1);
    write_fixture(
        feature_counterprobe.path(),
        &fixture_makefile(
            &fixture_variables(1),
            &counterprobe_rules,
            fixture_recipes(),
        ),
    );
    let result = run_make(&make, feature_counterprobe.path(), "crosstools-release");
    assert!(result.status.success(), "{result:?}");
    assert!(sentinels(feature_counterprobe.path()).contains(&"features".to_owned()));

    // Replacing the release runtime root with the ordinary root must expose
    // the extra generic CPU linklibs edge from the normal crosstools graph.
    let broad_runtime_counterprobe_rule =
        release_rule.replace("toolchain-linklibs-release", "toolchain-linklibs");
    assert_ne!(broad_runtime_counterprobe_rule, release_rule);
    let broad_runtime_counterprobe = tempfile::tempdir().unwrap();
    let counterprobe_rules =
        source_rules.replacen(release_rule, &broad_runtime_counterprobe_rule, 1);
    write_fixture(
        broad_runtime_counterprobe.path(),
        &fixture_makefile(
            &fixture_variables(1),
            &counterprobe_rules,
            fixture_recipes(),
        ),
    );
    let result = run_make(
        &make,
        broad_runtime_counterprobe.path(),
        "crosstools-release",
    );
    assert!(result.status.success(), "{result:?}");
    let broad_runtime_log = sentinels(broad_runtime_counterprobe.path());
    assert!(broad_runtime_log.contains(&"normal-cpu-runtime".to_owned()));
    assert!(!broad_runtime_log.contains(&"selected-cpu-release-runtime".to_owned()));

    // The ordinary crosstools path remains broad and keeps its feature target
    // plus the non-release CPU/toolchain linklib closure.
    let ordinary = tempfile::tempdir().unwrap();
    write_fixture(
        ordinary.path(),
        &fixture_makefile(&fixture_variables(0), &source_rules, fixture_recipes()),
    );
    let result = run_make(&make, ordinary.path(), "crosstools");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let ordinary_log = sentinels(ordinary.path());
    assert!(ordinary_log.contains(&"features".to_owned()));
    assert!(ordinary_log.contains(&"classic-riscv-linklibs".to_owned()));
    assert!(ordinary_log.contains(&"normal-toolchain-cpu-runtime".to_owned()));
    assert!(ordinary_log.contains(&"normal-toolchain-runtime".to_owned()));
    assert!(ordinary_log.contains(&"normal-cpu-runtime".to_owned()));
    assert!(!ordinary_log.contains(&"selected-cpu-release-runtime".to_owned()));

    // The configure-selected GNU libatomic target changes its own linklib
    // closure from core-linklibs to the temporary producer SDK.
    let release_atomic_sdk = tempfile::tempdir().unwrap();
    let atomic_fixture_rules = "tools-crosstools-autolibs:\n\
                                \t@printf 'autolibs\\n' >> $(LOG)\n\
                                core-linklibs:\n\
                                \t@printf 'core-linklibs\\n' >> $(LOG)\n";
    let release_atomic_rule = fixture_makefile(
        &fixture_variables(1),
        &release_libatomic_runtime,
        atomic_fixture_rules,
    );
    write_fixture(release_atomic_sdk.path(), &release_atomic_rule);
    let result = run_make(&make, release_atomic_sdk.path(), "gnu-libatomic-linklibs-1");
    assert!(result.status.success(), "{result:?}");
    assert_eq!(sentinels(release_atomic_sdk.path()), ["autolibs"]);

    let classic_atomic_sdk = tempfile::tempdir().unwrap();
    let classic_atomic_rule = fixture_makefile(
        &fixture_variables(0),
        &classic_libatomic_runtime,
        atomic_fixture_rules,
    );
    write_fixture(classic_atomic_sdk.path(), &classic_atomic_rule);
    let result = run_make(&make, classic_atomic_sdk.path(), "gnu-libatomic-linklibs-0");
    assert!(result.status.success(), "{result:?}");
    assert_eq!(sentinels(classic_atomic_sdk.path()), ["core-linklibs"]);
}
