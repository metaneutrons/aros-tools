mod common;

use aros_common::read_source;

fn assignment(source: &str, variable: &str) -> String {
    let joined = source.replace("\\\n", " ");
    let values: Vec<_> = joined
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(":=")?;
            (name.trim() == variable).then(|| value.trim().to_owned())
        })
        .collect();
    assert_eq!(values.len(), 1, "expected one {variable} assignment");
    values.into_iter().next().unwrap()
}

#[test]
fn llvmpipe_uses_real_target_llvm_rtti_without_null_typeinfo_stubs() {
    let root = common::source_root();
    let hidd = root.join("workbench/hidds/llvmpipe");
    assert!(!hidd.join("llvmpipe_llvm_rtti.c").exists());
    for file in ["mmakefile.src", "llvmpipe_init.c", "llvmpipe_intern.h"] {
        let source = read_source(&hidd.join(file)).expect("llvmpipe HIDD source");
        for obsolete in [
            "Llvmpipe_ForceLLVMPipeRTTI",
            "llvmpipe_llvm_rtti",
            "_ZTIN4llvm11ObjectCacheE",
            "_ZTIN4llvm19RTDyldMemoryManagerE",
        ] {
            assert!(!source.contains(obsolete), "obsolete RTTI stub in {file}");
        }
    }
    let recipe = read_source(&hidd.join("mmakefile.src")).unwrap();
    let link_flags = assignment(&recipe, "USER_LDFLAGS");
    assert!(link_flags.contains("-Wl,--start-group"));
    assert!(link_flags.contains("-lgalliumvm"));
    assert!(link_flags.contains("$(LLVM_LIBS)"));
    assert!(recipe.contains("USER_LDFLAGS += -Wl,--end-group"));

    let llvm = read_source(&root.join("workbench/libs/llvm/mmakefile.src")).unwrap();
    let target = assignment(&llvm, "LLVM_TARGET_CMAKEOPTIONS");
    assert!(target
        .split_whitespace()
        .any(|flag| flag == "-DLLVM_ENABLE_RTTI=ON"));
    assert!(!target.contains("-DLLVM_ENABLE_RTTI=OFF"));
    for other in [
        "LLVM_COMMON_CMAKEOPTIONS",
        "LLVM_TBLGEN_CMAKEOPTIONS",
        "LLVM_TOOLS_CMAKEOPTIONS",
    ] {
        assert!(
            !assignment(&llvm, other).contains("LLVM_ENABLE_RTTI"),
            "Target LLVM RTTI policy must not leak into {other}"
        );
    }
}
