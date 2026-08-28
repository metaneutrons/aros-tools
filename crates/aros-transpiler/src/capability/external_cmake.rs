//! The one `%build_with_cmake` declaration: CUnit.
//!
//! An upstream CMake project, fetched and built with its own build system. The
//! downstream helper drives that build; what is decided here is that the
//! declaration is the audited one, with the audited options, and that its fetch
//! edge and product closure are exact. Everything else stays out of the
//! executable graph.

use super::normalized_make_capability_block;
use crate::ast::ExternalCMakeDecl;
use crate::fetch::FetchDecl;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::parser::{evaluate_name, macro_arg, macro_argument_names, Invocation, TargetContext};
use std::path::Path;

pub(crate) const AOM_DECLARED_CAPABILITY: &str = "\
LIBAOM_CMAKEOPTIONS := -DBUILD_SHARED_LIBS=OFF -DENABLE_NASM=ON -DENABLE_EXAMPLES=OFF -DENABLE_TESTS=OFF -DENABLE_TOOLS=OFF -DCONFIG_AV1_ENCODER=0 -DCONFIG_AV1_DECODER=1 -DCONFIG_MULTITHREAD=0\n\
ifneq (,$(findstring x86_64,$(AROS_TARGET_CPU)))\n\
ifeq ($(NASM),)\n\
LIBAOM_TARGET_CPU=generic\n\
endif\n\
else\n\
ifneq (,$(findstring i386,$(AROS_TARGET_CPU)))\n\
ifeq ($(NASM),)\n\
LIBAOM_TARGET_CPU=generic\n\
endif\n\
endif\n\
endif\n\
ifeq ($(LIBAOM_TARGET_CPU),)\n\
LIBAOM_CMAKEOPTIONS += -DAOM_TARGET_CPU=$(AROS_TARGET_CPU)\n\
else\n\
LIBAOM_CMAKEOPTIONS += -DAOM_TARGET_CPU=$(LIBAOM_TARGET_CPU)\n\
endif\n\
ifneq (,$(findstring arm,$(AROS_TARGET_CPU)))\n\
AOM_NOCPUDETECT=yes\n\
LIBAOM_CMAKEOPTIONS += -DENABLE_NEON=0\n\
endif\n\
ifneq (,$(findstring riscv64,$(AROS_TARGET_CPU)))\n\
AOM_NOCPUDETECT=yes\n\
else\n\
ifneq (,$(findstring riscv,$(AROS_TARGET_CPU)))\n\
LIBAOM_CMAKEOPTIONS += -DENABLE_RVV=0\n\
AOM_NOCPUDETECT=yes\n\
endif\n\
endif\n\
ifneq (,$(findstring ppc,$(AROS_TARGET_CPU)))\n\
AOM_NOCPUDETECT=yes\n\
endif\n\
ifeq ($(AOM_NOCPUDETECT),yes)\n\
LIBAOM_CMAKEOPTIONS += -DCONFIG_RUNTIME_CPU_DETECT=0\n\
endif\n\
LIBAOM_LDFLAGS+=$(TARGET_CXX_LDFLAGS)\n\
ifneq ($(TARGET_CXX_LIBS),)\n\
LIBAOM_LDFLAGS+=-Wl,--start-group $(TARGET_CXX_LIBS) -Wl,--end-group\n\
endif";

pub(crate) const AOM_COMMON_OPTIONS: &[&str] = &[
    "-DBUILD_SHARED_LIBS=OFF",
    "-DENABLE_NASM=ON",
    "-DENABLE_EXAMPLES=OFF",
    "-DENABLE_TESTS=OFF",
    "-DENABLE_TOOLS=OFF",
    "-DCONFIG_AV1_ENCODER=0",
    "-DCONFIG_AV1_DECODER=1",
    "-DCONFIG_MULTITHREAD=0",
    // config/make-cmake.tmpl supplies this legacy default outside
    // LIBAOM_CMAKEOPTIONS. Make it explicit in the standalone capability.
    "-DCMAKE_BUILD_TYPE=Release",
];

pub(crate) fn aom_profile_options(
    target: Option<&TargetContext>,
) -> std::result::Result<Vec<String>, String> {
    let Some(target) = target else {
        return Err("AOM capability requires a concrete target profile".to_owned());
    };
    let profile = (
        target.cpu.as_deref(),
        target.platform.as_deref(),
        target.toolchain.as_deref(),
        target.cpu32.as_deref(),
        target.use_mmu.as_deref(),
        target.float_abi.as_deref(),
    );
    let specific: &[&str] = match profile {
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard")) => &[
            "-DAOM_TARGET_CPU=arm",
            "-DENABLE_NEON=0",
            "-DCONFIG_RUNTIME_CPU_DETECT=0",
        ],
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some(""))
        | (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("")) => {
            // The legacy expression expands to `aarch64`, but the audited
            // migration contract deliberately retains the probe-proven scalar
            // configuration shared with the reproducible x86_64 profile.
            &["-DAOM_TARGET_CPU=generic"]
        }
        (Some("riscv64"), Some("opensbi"), Some("llvm"), Some(""), Some("1"), Some("")) => {
            &["-DAOM_TARGET_CPU=riscv64", "-DCONFIG_RUNTIME_CPU_DETECT=0"]
        }
        _ => {
            return Err(format!(
                "AOM capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
                target.cpu.as_deref().unwrap_or("<unset>"),
                target.platform.as_deref().unwrap_or("<unset>"),
                target.toolchain.as_deref().unwrap_or("<unset>"),
                target.cpu32.as_deref().unwrap_or("<unset>"),
                target.use_mmu.as_deref().unwrap_or("<unset>"),
                target.float_abi.as_deref().unwrap_or("<unset>")
            ));
        }
    };
    Ok(AOM_COMMON_OPTIONS
        .iter()
        .chain(specific)
        .map(|option| (*option).to_owned())
        .collect())
}

/// Parses one deliberately narrow `%build_with_cmake` capability.
///
/// Generic external-project passthrough would let a newly added host compiler,
/// source tree or install prefix silently execute in target builds. Each
/// admitted declaration must match its complete audited arguments, owning
/// fetch and target-profile contract. Everything else remains an explicit
/// skip with a precise diagnostic.
pub(crate) fn parse(
    invocation: &Invocation,
    expression_context: &MakeExprContext<'_>,
    relative_dir: &Path,
    fetches: &[FetchDecl],
    target: Option<&TargetContext>,
    make_source: &str,
) -> std::result::Result<ExternalCMakeDecl, String> {
    const CUNIT_MMAKE: &str = "linklibs-yes-cunit";
    const CUNIT_SOURCE: &str = "${AROS_PORTS_DIR}/cunit/cunit-3.5.5";
    const CUNIT_PREFIX: &str = "${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras";
    const CUNIT_FETCH: &str = "cunit-fetch";
    const DECLARED_OPTIONS: &[&str] = &[
        "-DCUNIT_DISABLE_EXAMPLES=yes",
        "-DCUNIT_DISABLE_TESTS=yes",
        "-DCMAKE_BUILD_TYPE=DEBUG",
        "-Wno-error=dev",
    ];

    let mmake_raw = macro_arg(&invocation.args, "mmake")
        .ok_or_else(|| "missing required mmake= argument".to_owned())?;
    let mmake = evaluate_name(&mmake_raw, expression_context)
        .map_err(|reason| format!("mmake={mmake_raw} is unresolved: {reason}"))?;
    if relative_dir == Path::new("workbench/classes/datatypes/heic")
        && mmake == "datatypes-heic-linklibs-aom"
    {
        return parse_aom(
            invocation,
            expression_context,
            target,
            make_source,
            fetches,
            relative_dir,
            mmake,
        );
    }
    if relative_dir != Path::new("compiler/cunit") || mmake != CUNIT_MMAKE {
        return Err(format!(
            "unsupported external-CMake capability (modelled: compiler/cunit mmake={CUNIT_MMAKE}; workbench/classes/datatypes/heic mmake=datatypes-heic-linklibs-aom)"
        ));
    }

    let argument_names = macro_argument_names(&invocation.args);
    let mut unique_names = argument_names.clone();
    unique_names.sort();
    unique_names.dedup();
    if unique_names.len() != argument_names.len() {
        return Err("duplicate macro argument".to_owned());
    }
    let mut expected_names = vec!["extraoptions", "mmake", "prefix", "srcdir"];
    expected_names.sort_unstable();
    if unique_names != expected_names {
        return Err(format!(
            "argument set [{}] does not match audited CUnit capability [{}]",
            unique_names.join(", "),
            expected_names.join(", ")
        ));
    }

    let evaluate_path = |key: &str| -> std::result::Result<String, String> {
        let raw = macro_arg(&invocation.args, key)
            .ok_or_else(|| format!("missing required {key}= argument"))?;
        let value = evaluate_make_expr(&raw, expression_context)
            .map_err(|reason| format!("{key}={raw} cannot be evaluated: {reason}"))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("{key}={raw} expanded to an empty value"));
        }
        Ok(value.to_owned())
    };
    let source_dir = evaluate_path("srcdir")?;
    if source_dir != CUNIT_SOURCE {
        return Err(format!(
            "srcdir resolves to {source_dir}, expected {CUNIT_SOURCE}"
        ));
    }
    let install_prefix = evaluate_path("prefix")?;
    if install_prefix != CUNIT_PREFIX {
        return Err(format!(
            "prefix resolves to {install_prefix}, expected {CUNIT_PREFIX}"
        ));
    }

    let options_raw = macro_arg(&invocation.args, "extraoptions")
        .ok_or_else(|| "missing required extraoptions= argument".to_owned())?;
    let options = evaluate_make_list(&options_raw, expression_context)
        .map_err(|reason| format!("extraoptions={options_raw} cannot be evaluated: {reason}"))?;
    if options != DECLARED_OPTIONS {
        return Err(format!(
            "extraoptions resolve to [{}], expected [{}]",
            options.join(" "),
            DECLARED_OPTIONS.join(" ")
        ));
    }

    let matching_fetches: Vec<_> = fetches
        .iter()
        .filter(|fetch| fetch.name == CUNIT_FETCH)
        .collect();
    let [fetch] = matching_fetches.as_slice() else {
        return Err(format!(
            "requires exactly one %fetch mmake={CUNIT_FETCH} declaration, found {}",
            matching_fetches.len()
        ));
    };
    for (field, actual, expected) in [
        ("archive", fetch.archive.as_str(), "cunit-3.5.5"),
        ("suffixes", fetch.suffixes.as_str(), "tar.bz2"),
        (
            "location",
            fetch.location.as_str(),
            "${AROS_PORTS_SOURCE_DIR}",
        ),
        (
            "destination",
            fetch.destination.as_str(),
            "${AROS_PORTS_DIR}/cunit",
        ),
        (
            "patches_specs",
            fetch.patches.as_str(),
            "cunit-3.5.5-aros.diff:cunit-3.5.5:-f,-p1",
        ),
        (
            "patches_origins",
            fetch.patch_origins.as_str(),
            "${CMAKE_SOURCE_DIR}/compiler/cunit",
        ),
        ("base", fetch.base.as_str(), ""),
        ("declaring directory", fetch.dir.as_str(), "compiler/cunit"),
    ] {
        if actual != expected {
            return Err(format!(
                "%{CUNIT_FETCH} {field} is {actual}, expected {expected}"
            ));
        }
    }

    Ok(ExternalCMakeDecl {
        mmake_name: mmake,
        source_dir,
        binary_dir: "${AROS_BUILD_DIR}/gen/external-cmake/compiler/cunit".to_owned(),
        install_prefix: install_prefix.clone(),
        fetch_target: CUNIT_FETCH.to_owned(),
        local_patch_files: vec![
            "${CMAKE_SOURCE_DIR}/compiler/cunit/cunit-3.5.5-aros.diff".to_owned()
        ],
        provided_library: "cunit".to_owned(),
        provider_target: "linklibs-yes-cunit-external-cunit".to_owned(),
        library_products: vec![format!("{install_prefix}/lib/libcunit.a")],
        header_products: [
            "Automated.h",
            "AutomatedJUnitXml.h",
            "Basic.h",
            "CUAssert.h",
            "CUCurses.h",
            "CUError.h",
            "CUnit.h",
            "CUnitCI.h",
            "CUnitCITypes.h",
            "CUnit_intl.h",
            "Console.h",
            "MessageHandlers.h",
            "MyMem.h",
            "Simple.h",
            "TestDB.h",
            "TestFixture.h",
            "TestRun.h",
            "Util.h",
            "wxWidget.h",
        ]
        .into_iter()
        .map(|header| format!("{install_prefix}/include/CUnit/{header}"))
        .collect(),
        // CUnit also installs build-system source files and CMake package
        // metadata, but no AROS target consumes them. Only public, repaired
        // capability products belong in this contract.
        auxiliary_products: Vec::new(),
        public_include_dirs: vec![format!("{install_prefix}/include")],
        options: vec![
            "-DCUNIT_DISABLE_EXAMPLES=yes".to_owned(),
            "-DCUNIT_DISABLE_TESTS=yes".to_owned(),
            "-DCMAKE_BUILD_TYPE=DEBUG".to_owned(),
            "-Wno-error=dev".to_owned(),
        ],
        dir_path: relative_dir.to_path_buf(),
    })
}

pub(crate) fn parse_aom(
    invocation: &Invocation,
    expression_context: &MakeExprContext<'_>,
    target: Option<&TargetContext>,
    make_source: &str,
    fetches: &[FetchDecl],
    relative_dir: &Path,
    mmake: String,
) -> std::result::Result<ExternalCMakeDecl, String> {
    const AOM_FETCH: &str = "linklibs-aom-fetch";
    const AOM_SOURCE: &str = "${AROS_PORTS_DIR}/libaom/libaom-3.12.1";
    const AOM_PREFIX: &str = "${AROS_BUILD_DIR}/SYS/Developer";

    let argument_names = macro_argument_names(&invocation.args);
    let mut unique_names = argument_names.clone();
    unique_names.sort();
    unique_names.dedup();
    if unique_names.len() != argument_names.len() {
        return Err("duplicate macro argument".to_owned());
    }
    let mut expected_names = vec![
        "extraoptions",
        "extraldflags",
        "mmake",
        "package",
        "prefix",
        "srcdir",
    ];
    expected_names.sort_unstable();
    if unique_names != expected_names {
        return Err(format!(
            "argument set [{}] does not match audited AOM capability [{}]",
            unique_names.join(", "),
            expected_names.join(", ")
        ));
    }

    for (key, expected) in [
        ("package", "aom"),
        ("srcdir", "$(AOMARCHSRCDIR)"),
        ("prefix", "$(AROS_DEVELOPER)"),
        ("extraoptions", "$(LIBAOM_CMAKEOPTIONS)"),
        ("extraldflags", "$(LIBAOM_LDFLAGS)"),
    ] {
        let actual = macro_arg(&invocation.args, key)
            .ok_or_else(|| format!("missing required {key}= argument"))?;
        if actual != expected {
            return Err(format!(
                "{key} uses `{actual}`, expected audited form `{expected}`"
            ));
        }
    }

    let declared_block = normalized_make_capability_block(
        make_source,
        "LIBAOM_CMAKEOPTIONS :=",
        "%build_with_cmake",
    )
    .ok_or_else(|| "AOM option/extraldflags capability block is missing".to_owned())?;
    if declared_block != AOM_DECLARED_CAPABILITY {
        return Err(
            "AOM option/extraldflags declaration block differs from audited capability".to_owned(),
        );
    }

    let evaluate_path = |key: &str| -> std::result::Result<String, String> {
        let raw = macro_arg(&invocation.args, key)
            .ok_or_else(|| format!("missing required {key}= argument"))?;
        let value = evaluate_make_expr(&raw, expression_context)
            .map_err(|reason| format!("{key}={raw} cannot be evaluated: {reason}"))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("{key}={raw} expanded to an empty value"));
        }
        Ok(value.to_owned())
    };
    let source_dir = evaluate_path("srcdir")?;
    if source_dir != AOM_SOURCE {
        return Err(format!(
            "srcdir resolves to {source_dir}, expected {AOM_SOURCE}"
        ));
    }
    let install_prefix = evaluate_path("prefix")?;
    if install_prefix != AOM_PREFIX {
        return Err(format!(
            "prefix resolves to {install_prefix}, expected {AOM_PREFIX}"
        ));
    }
    let options = aom_profile_options(target)?;

    let matching_fetches: Vec<_> = fetches
        .iter()
        .filter(|fetch| fetch.name == AOM_FETCH)
        .collect();
    let [fetch] = matching_fetches.as_slice() else {
        return Err(format!(
            "requires exactly one %fetch mmake={AOM_FETCH} declaration, found {}",
            matching_fetches.len()
        ));
    };
    for (field, actual, expected) in [
        ("archive", fetch.archive.as_str(), "libaom-3.12.1"),
        ("suffixes", fetch.suffixes.as_str(), "tar.gz"),
        (
            "archive_origins",
            fetch.origins.as_str(),
            "https://storage.googleapis.com/aom-releases",
        ),
        (
            "location",
            fetch.location.as_str(),
            "${AROS_PORTS_SOURCE_DIR}",
        ),
        (
            "destination",
            fetch.destination.as_str(),
            "${AROS_PORTS_DIR}/libaom",
        ),
        (
            "patches_specs",
            fetch.patches.as_str(),
            "libaom-3.12.1-aros.diff:libaom-3.12.1:-f,-p1",
        ),
        (
            "patches_origins",
            fetch.patch_origins.as_str(),
            "${CMAKE_SOURCE_DIR}/workbench/classes/datatypes/heic",
        ),
        ("base", fetch.base.as_str(), ""),
        (
            "declaring directory",
            fetch.dir.as_str(),
            "workbench/classes/datatypes/heic",
        ),
    ] {
        if actual != expected {
            return Err(format!(
                "%fetch mmake={AOM_FETCH} {field} is {actual}, expected {expected}"
            ));
        }
    }

    Ok(ExternalCMakeDecl {
        mmake_name: mmake,
        source_dir,
        binary_dir: "${AROS_BUILD_DIR}/gen/external-cmake/workbench/classes/datatypes/heic/aom"
            .to_owned(),
        install_prefix: install_prefix.clone(),
        fetch_target: AOM_FETCH.to_owned(),
        local_patch_files: vec![
            "${CMAKE_SOURCE_DIR}/workbench/classes/datatypes/heic/libaom-3.12.1-aros.diff"
                .to_owned(),
        ],
        provided_library: "aom".to_owned(),
        provider_target: "datatypes-heic-linklibs-aom-external-aom".to_owned(),
        library_products: vec![format!("{install_prefix}/lib/libaom.a")],
        header_products: [
            "aom.h",
            "aom_codec.h",
            "aom_decoder.h",
            "aom_frame_buffer.h",
            "aom_image.h",
            "aom_integer.h",
            "aomdx.h",
        ]
        .into_iter()
        .map(|header| format!("{install_prefix}/include/aom/{header}"))
        .collect(),
        auxiliary_products: vec![format!("{install_prefix}/lib/pkgconfig/aom.pc")],
        public_include_dirs: vec![format!("{install_prefix}/include")],
        options,
        dir_path: relative_dir.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::{parse, Invocation};
    use crate::make_expr::MakeExprContext;
    use crate::make_vars::collect_vars_impl;
    use crate::make_vars::VarScope;
    use crate::parser::{join_continuations, select_target_invocations};
    use crate::testing::{dirs, root, target_context};
    use aros_common::read_source;
    use std::path::Path;

    fn parsed_cunit_capability() -> (Invocation, VarScope, Vec<crate::fetch::FetchDecl>) {
        let root = root();
        let relative_dir = Path::new("compiler/cunit");
        let content = read_source(&root.join(relative_dir).join("mmakefile.src")).unwrap();
        let joined = join_continuations(&content);
        let profile = target_context("x86_64", "pc", "");
        let (scope, states) = collect_vars_impl(&joined, Some(&profile));
        let mut skipped = Vec::new();
        let invocation =
            select_target_invocations(&joined, Some(&states), relative_dir, &mut skipped)
                .into_iter()
                .find(|invocation| invocation.name == "build_with_cmake")
                .unwrap();
        assert!(skipped.is_empty(), "{skipped:#?}");
        let (fetches, skipped_fetches) =
            crate::fetch::collect_fetches_with_scope(&content, relative_dir, &scope);
        assert!(skipped_fetches.is_empty(), "{skipped_fetches:#?}");
        (invocation, scope, fetches)
    }

    #[test]
    fn cunit_external_cmake_capability_is_complete_and_exact() {
        let root = root();
        let relative_dir = Path::new("compiler/cunit");
        let (invocation, scope, fetches) = parsed_cunit_capability();
        let directory_vars = dirs();
        let expression_context = MakeExprContext::new(
            &scope,
            &directory_vars,
            invocation.line,
            &root,
            relative_dir,
        );
        let declaration = parse(
            &invocation,
            &expression_context,
            relative_dir,
            &fetches,
            None,
            "",
        )
        .unwrap();

        assert_eq!(declaration.mmake_name, "linklibs-yes-cunit");
        assert_eq!(
            declaration.source_dir,
            "${AROS_PORTS_DIR}/cunit/cunit-3.5.5"
        );
        assert_eq!(
            declaration.install_prefix,
            "${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras"
        );
        assert_eq!(declaration.fetch_target, "cunit-fetch");
        assert_eq!(declaration.provided_library, "cunit");
        assert_eq!(
            declaration.provider_target,
            "linklibs-yes-cunit-external-cunit"
        );
        assert_eq!(
            declaration.library_products,
            ["${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras/lib/libcunit.a"]
        );
        assert_eq!(
            declaration.public_include_dirs,
            ["${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras/include"]
        );
        assert_eq!(declaration.header_products.len(), 19);
        assert!(declaration.auxiliary_products.is_empty());
        assert_eq!(
            declaration.header_products.first().map(String::as_str),
            Some("${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras/include/CUnit/Automated.h")
        );
        assert_eq!(
            declaration.header_products.last().map(String::as_str),
            Some("${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras/include/CUnit/wxWidget.h")
        );
        assert_eq!(
            declaration.options,
            [
                "-DCUNIT_DISABLE_EXAMPLES=yes",
                "-DCUNIT_DISABLE_TESTS=yes",
                "-DCMAKE_BUILD_TYPE=DEBUG",
                "-Wno-error=dev",
            ]
        );
    }

    #[test]
    fn cunit_external_cmake_capability_rejects_any_contract_drift() {
        let root = root();
        let relative_dir = Path::new("compiler/cunit");
        let (invocation, scope, fetches) = parsed_cunit_capability();
        let directory_vars = dirs();
        let expression_context = MakeExprContext::new(
            &scope,
            &directory_vars,
            invocation.line,
            &root,
            relative_dir,
        );
        let parse = |invocation: &Invocation, fetches: &[crate::fetch::FetchDecl]| {
            parse(
                invocation,
                &expression_context,
                relative_dir,
                fetches,
                None,
                "",
            )
            .unwrap_err()
        };

        let mut changed = invocation.clone();
        changed.args = changed.args.replace(
            "srcdir=$(PORTSDIR)/cunit/$(ARCHBASE)",
            "srcdir=$(AROS_DEVELOPER)",
        );
        assert!(parse(&changed, &fetches).contains("srcdir resolves to"));

        let mut changed = invocation.clone();
        changed.args = changed
            .args
            .replace("prefix=$(AROS_CONTRIB_SDK)", "prefix=$(AROS_DEVELOPER)");
        assert!(parse(&changed, &fetches).contains("prefix resolves to"));

        let mut changed = invocation.clone();
        changed.args = changed.args.replace(
            "extraoptions=$(CUNIT_CMAKE_FLAGS)",
            "extraoptions=-DUNAUDITED=yes",
        );
        assert!(parse(&changed, &fetches).contains("extraoptions resolve to"));

        let mut changed = invocation.clone();
        changed.args.push_str(" compiler=host");
        assert!(parse(&changed, &fetches).contains("argument set"));

        let mut changed_fetches = fetches;
        changed_fetches[0].archive = "cunit-unreviewed".to_owned();
        assert!(parse(&invocation, &changed_fetches).contains("archive is"));

        assert!(parse(&invocation, &[]).contains("exactly one"));
    }
}
