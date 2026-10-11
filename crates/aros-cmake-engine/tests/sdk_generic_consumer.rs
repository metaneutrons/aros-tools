//! Closed-response probes for the generic LLVM SDK consumer boundary.
//! Most cases simulate compiler metadata after project(NONE); one focused
//! host-Clang initialization probe checks that flags reach all language commands.
//! None of these checks qualifies an SDK.
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Probe {
    cpu: &'static str,
    triple: &'static str,
    width: &'static str,
    float_abi: &'static str,
    toolchain: &'static str,
    abi_flavour: &'static str,
    check_compiler: bool,
    legacy: bool,
    repeat_apply: bool,
    compiler_cpu: &'static str,
    compiler_fpu: &'static str,
    initial_c_flags: &'static str,
    initial_c_flags_release: &'static str,
    normal_c_flags: &'static str,
    response_edit: &'static str,
    after_validation: &'static str,
    compiler_edit: &'static str,
}

impl Probe {
    const fn new(
        cpu: &'static str,
        triple: &'static str,
        width: &'static str,
        float_abi: &'static str,
    ) -> Self {
        Self {
            cpu,
            triple,
            width,
            float_abi,
            toolchain: "llvm",
            abi_flavour: "fixture-flavour",
            check_compiler: false,
            legacy: false,
            repeat_apply: false,
            compiler_cpu: "",
            compiler_fpu: "",
            initial_c_flags: "",
            initial_c_flags_release: "",
            normal_c_flags: "",
            response_edit: "",
            after_validation: "",
            compiler_edit: "",
        }
    }

    const fn compiler(mut self) -> Self {
        self.check_compiler = true;
        self
    }

    const fn legacy(mut self) -> Self {
        self.legacy = true;
        self.abi_flavour = "standalone";
        self
    }

    const fn compiler_options(mut self, cpu: &'static str, fpu: &'static str) -> Self {
        self.compiler_cpu = cpu;
        self.compiler_fpu = fpu;
        self
    }

    const fn repeat_apply(mut self) -> Self {
        self.repeat_apply = true;
        self
    }

    const fn initial_c_flags(mut self, flags: &'static str) -> Self {
        self.initial_c_flags = flags;
        self
    }

    const fn initial_c_flags_release(mut self, flags: &'static str) -> Self {
        self.initial_c_flags_release = flags;
        self
    }

    const fn normal_c_flags(mut self, flags: &'static str) -> Self {
        self.normal_c_flags = flags;
        self
    }

    const fn response(mut self, edit: &'static str) -> Self {
        self.response_edit = edit;
        self
    }

    const fn after(mut self, edit: &'static str) -> Self {
        self.after_validation = edit;
        self
    }

    const fn compiler_edit(mut self, edit: &'static str) -> Self {
        self.check_compiler = true;
        self.compiler_edit = edit;
        self
    }

    const fn toolchain(mut self, value: &'static str) -> Self {
        self.toolchain = value;
        self
    }
}

struct Harness {
    _temp: tempfile::TempDir,
    root: PathBuf,
    build: PathBuf,
    engine: PathBuf,
}

struct Outcome {
    output: Output,
    marker: bool,
    selected: Option<Vec<u8>>,
    compiler_flags: Option<Vec<u8>>,
    compile_commands: Option<Vec<u8>>,
    initialization_abi: Option<Vec<u8>>,
    configure_log: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
    binding: Option<Vec<u8>>,
}

const CMAKE_PROBE: &str = r#"
cmake_minimum_required(VERSION 3.22)
project(SdkGenericConsumerProbe NONE)
file(REMOVE "${ROOT}/passed" "${ROOT}/selected-sdk-root")
include("${ENGINE}/NativeConsumerContract.cmake")
include("${ENGINE}/tests/NativeSourceSelectionFixture.cmake")
aros_test_prepare_native_consumer_selection("${ROOT}" "${ROOT}/consumer.json")

set(AROS_TARGET_CPU "@CPU@")
set(AROS_TARGET_TRIPLE "@TRIPLE@")
set(AROS_TARGET_PLATFORM fixture)
set(AROS_TOOLCHAIN "@TOOLCHAIN@")
set(GCC_CONFIG_FLOAT_ABI "@FLOAT_ABI@")
set(AROS_ABI_FLAVOUR "@FLAVOUR@")
set(AROS_ABI_PLATFORM_SMP OFF)
set(AROS_ENABLE_MMU OFF)

file(READ "${CMAKE_CURRENT_BINARY_DIR}/native-consumer-validation-response.json" _response)
string(JSON _legacy_abi GET "${_response}" abi)
if(@LEGACY@)
    string(JSON _response SET "${_response}" schema "\"aros-native-consumer-validation-v1\"")
    string(JSON _response SET "${_response}" abi "${_legacy_abi}")
else()
    string(JSON _response REMOVE "${_response}" abi)
    string(JSON _response SET "${_response}" schema "\"aros-sdk-consumer-validation-v1\"")
    set(_target "{}")
    string(JSON _target SET "${_target}" target_triple "\"${AROS_TARGET_TRIPLE}\"")
    set(_abi "{}")
    string(JSON _abi SET "${_abi}" source_cpu "\"${AROS_TARGET_CPU}\"")
    string(JSON _abi SET "${_abi}" pointer_width @WIDTH@)
    string(JSON _abi SET "${_abi}" use_mmu false)
    string(JSON _abi SET "${_abi}" flavour "\"${AROS_ABI_FLAVOUR}\"")
    string(JSON _abi SET "${_abi}" platform_smp false)
    if(NOT GCC_CONFIG_FLOAT_ABI STREQUAL "")
        string(JSON _abi SET "${_abi}" float_abi "\"${GCC_CONFIG_FLOAT_ABI}\"")
    endif()
    if(NOT "@COMPILER_CPU@" STREQUAL "")
        string(JSON _abi SET "${_abi}" compiler_cpu "\"@COMPILER_CPU@\"")
    endif()
    if(NOT "@COMPILER_FPU@" STREQUAL "")
        string(JSON _abi SET "${_abi}" compiler_fpu "\"@COMPILER_FPU@\"")
    endif()
    string(JSON _target SET "${_target}" abi "${_abi}")
    string(JSON _response SET "${_response}" target "${_target}")
endif()
@RESPONSE_EDIT@
if(RECONFIGURE_FAIL)
    string(JSON _response SET "${_response}" qualification "\"changed-proof\"")
endif()
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/native-consumer-validation-response.json" "${_response}\n")

aros_validate_native_consumer_contract()
set(AROS_DEVELOPER_INCLUDE_DIR "${CMAKE_BINARY_DIR}/SYS/Developer/include")
aros_resolve_sdk_include_root(_selected_root)
if(NOT _selected_root STREQUAL AROS_DEVELOPER_INCLUDE_DIR)
    message(FATAL_ERROR "incorrect generic SDK root: ${_selected_root}")
endif()
@AFTER_VALIDATION@

if(@CHECK_COMPILER@)
    # Deliberately synthetic metadata: this exercises only CMake's configured-
    # compiler guard, not Clang, target code generation, or SDK qualification.
    set(CMAKE_C_COMPILER_ID Clang)
    set(CMAKE_CXX_COMPILER_ID Clang)
    set(CMAKE_ASM_COMPILER_ID Clang)
    set(CMAKE_C_COMPILER_TARGET "${AROS_TARGET_TRIPLE}")
    set(CMAKE_CXX_COMPILER_TARGET "${AROS_TARGET_TRIPLE}")
    set(CMAKE_ASM_COMPILER_TARGET "${AROS_TARGET_TRIPLE}")
    set(CMAKE_SIZEOF_VOID_P "@POINTER_BYTES@")
    foreach(_language IN ITEMS C CXX ASM)
        set(_flags_variable "CMAKE_${_language}_FLAGS")
        set(${_flags_variable} "" CACHE STRING "fixture flags" FORCE)
        unset(${_flags_variable})
        foreach(_configuration IN ITEMS DEBUG RELEASE RELWITHDEBINFO MINSIZEREL)
            set(_config_variable "CMAKE_${_language}_FLAGS_${_configuration}")
            set(${_config_variable} "" CACHE STRING "fixture flags" FORCE)
            unset(${_config_variable})
        endforeach()
    endforeach()
    if(AROS_TARGET_CPU STREQUAL "arm")
        foreach(_language IN ITEMS C CXX ASM)
            set(_flags_variable "CMAKE_${_language}_FLAGS")
            set(${_flags_variable} "-mfloat-abi=${GCC_CONFIG_FLOAT_ABI}"
                CACHE STRING "fixture flags" FORCE)
            unset(${_flags_variable})
        endforeach()
    endif()
    if(NOT "@INITIAL_C_FLAGS@" STREQUAL "")
        set(CMAKE_C_FLAGS "@INITIAL_C_FLAGS@" CACHE STRING "fixture flags" FORCE)
        unset(CMAKE_C_FLAGS)
    endif()
    if(NOT "@INITIAL_C_FLAGS_RELEASE@" STREQUAL "")
        set(CMAKE_C_FLAGS_RELEASE "@INITIAL_C_FLAGS_RELEASE@"
            CACHE STRING "fixture flags" FORCE)
        unset(CMAKE_C_FLAGS_RELEASE)
    endif()
    if(NOT "@NORMAL_C_FLAGS@" STREQUAL "")
        set(CMAKE_C_FLAGS "@NORMAL_C_FLAGS@")
    endif()
    if(AROS_TOOLCHAIN STREQUAL "gnu")
        set(AROS_GNU_TARGET_COMPILE_OPTIONS
            "-march=fixture-isa;-mabi=ilp32f;-mcmodel=medany")
    endif()
    foreach(_language IN ITEMS C CXX ASM)
        aros_apply_sdk_consumer_compiler_options_for_language(${_language})
        if(@REPEAT_APPLY@)
            aros_apply_sdk_consumer_compiler_options_for_language(${_language})
        endif()
    endforeach()
    @COMPILER_EDIT@
    aros_validate_native_consumer_compiler_target()
    foreach(_language IN ITEMS C CXX ASM)
        get_property(_${_language}_flags CACHE "CMAKE_${_language}_FLAGS" PROPERTY VALUE)
        get_property(_${_language}_type CACHE "CMAKE_${_language}_FLAGS" PROPERTY TYPE)
        if(NOT _${_language}_type STREQUAL "STRING")
            message(FATAL_ERROR "${_language} compiler flags are not a STRING cache entry")
        endif()
    endforeach()
    file(WRITE "${ROOT}/compiler-flags"
        "C=${_C_flags}\nCXX=${_CXX_flags}\nASM=${_ASM_flags}\n")
endif()
file(WRITE "${ROOT}/selected-sdk-root" "${_selected_root}")
file(WRITE "${ROOT}/passed" "selected and validated")
"#;

const CMAKE_INITIALIZATION_PROBE: &str = r##"
cmake_minimum_required(VERSION 3.22)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
set(CMAKE_C_FLAGS_INIT "@C_FLAGS_INIT@")
set(CMAKE_CXX_FLAGS_INIT "@CXX_FLAGS_INIT@")
set(CMAKE_ASM_FLAGS_INIT "@ASM_FLAGS_INIT@")

include("${ENGINE}/NativeConsumerContract.cmake")
include("${ENGINE}/tests/NativeSourceSelectionFixture.cmake")
aros_test_prepare_native_consumer_selection("${ROOT}" "${ROOT}/consumer.json")

set(AROS_TARGET_CPU arm)
set(AROS_TARGET_TRIPLE arm-unknown-aros)
set(AROS_TARGET_PLATFORM fixture)
set(AROS_TOOLCHAIN llvm)
set(GCC_CONFIG_FLOAT_ABI hard)
set(AROS_ABI_FLAVOUR fixture-flavour)
set(AROS_ABI_PLATFORM_SMP OFF)
set(AROS_ENABLE_MMU OFF)

file(READ "${CMAKE_CURRENT_BINARY_DIR}/native-consumer-validation-response.json" _response)
string(JSON _legacy_abi GET "${_response}" abi)
string(JSON _response REMOVE "${_response}" abi)
string(JSON _response SET "${_response}" schema "\"aros-sdk-consumer-validation-v1\"")
set(_target "{}")
string(JSON _target SET "${_target}" target_triple "\"${AROS_TARGET_TRIPLE}\"")
set(_abi "{}")
string(JSON _abi SET "${_abi}" source_cpu "\"${AROS_TARGET_CPU}\"")
string(JSON _abi SET "${_abi}" pointer_width 32)
string(JSON _abi SET "${_abi}" use_mmu false)
string(JSON _abi SET "${_abi}" flavour "\"${AROS_ABI_FLAVOUR}\"")
string(JSON _abi SET "${_abi}" platform_smp false)
string(JSON _abi SET "${_abi}" float_abi "\"${GCC_CONFIG_FLOAT_ABI}\"")
string(JSON _abi SET "${_abi}" compiler_cpu "\"cortex-a7\"")
string(JSON _abi SET "${_abi}" compiler_fpu "\"neon-vfpv4\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/native-consumer-validation-response.json" "${_response}\n")

aros_validate_native_consumer_contract()
set(AROS_DEVELOPER_INCLUDE_DIR "${CMAKE_BINARY_DIR}/SYS/Developer/include")
aros_resolve_sdk_include_root(_selected_root)
aros_lock_native_consumer_contract_identity()

file(WRITE "${ROOT}/global-rules-override.cmake" "# test global rules override\n")
set(CMAKE_USER_MAKE_RULES_OVERRIDE "${ROOT}/global-rules-override.cmake")
set(_global_rules_override "${CMAKE_USER_MAKE_RULES_OVERRIDE}")
if(@CONFLICT_OVERRIDE@)
    set(CMAKE_USER_MAKE_RULES_OVERRIDE_C "${ROOT}/caller-rules-override.cmake")
endif()
set(AROS_CROSS_TOOLCHAIN_ROOT "${ROOT}/mock-release-toolchain")
aros_register_sdk_consumer_compiler_option_overrides()
if(NOT CMAKE_USER_MAKE_RULES_OVERRIDE STREQUAL _global_rules_override)
    message(FATAL_ERROR "generic SDK registration replaced the global rules override")
endif()

project(SdkGenericCompilerInitialization LANGUAGES C CXX ASM)
set(_abi_diagnostic "")
foreach(_name IN ITEMS
    CMAKE_SIZEOF_VOID_P
    CMAKE_C_SIZEOF_DATA_PTR
    CMAKE_CXX_SIZEOF_DATA_PTR
    CMAKE_ASM_SIZEOF_DATA_PTR
    CMAKE_C_COMPILER_TARGET
    CMAKE_CXX_COMPILER_TARGET
    CMAKE_ASM_COMPILER_TARGET
    CMAKE_C_COMPILER_ID
    CMAKE_CXX_COMPILER_ID
    CMAKE_ASM_COMPILER_ID
    CMAKE_C_COMPILER_ABI
    CMAKE_CXX_COMPILER_ABI)
    if(DEFINED ${_name})
        string(APPEND _abi_diagnostic "${_name}=${${_name}}\n")
    else()
        string(APPEND _abi_diagnostic "${_name}=<undefined>\n")
    endif()
endforeach()
file(WRITE "${ROOT}/compiler-abi" "${_abi_diagnostic}")
message(STATUS "SDK initialization ABI diagnostics:\n${_abi_diagnostic}")
aros_validate_native_consumer_compiler_target()
add_library(sdk_compiler_options_probe STATIC
    "${ROOT}/probe.c"
    "${ROOT}/probe.cpp"
    "${ROOT}/probe.S")

foreach(_language IN ITEMS C CXX ASM)
    get_property(_${_language}_flags CACHE "CMAKE_${_language}_FLAGS" PROPERTY VALUE)
    _aros_sdk_count_compiler_options(_count _exact _split
        "${_${_language}_flags}" mcpu cortex-a7)
    if(NOT _count EQUAL 1 OR NOT _exact EQUAL 1 OR _split)
        message(FATAL_ERROR "${_language} cache flags do not contain exactly one source CPU option")
    endif()
    _aros_sdk_count_compiler_options(_count _exact _split
        "${_${_language}_flags}" mfpu neon-vfpv4)
    if(NOT _count EQUAL 1 OR NOT _exact EQUAL 1 OR _split)
        message(FATAL_ERROR "${_language} cache flags do not contain exactly one source FPU option")
    endif()
    _aros_sdk_count_compiler_options(_count _exact _split
        "${_${_language}_flags}" mfloat-abi hard)
    if(NOT _count EQUAL 1 OR NOT _exact EQUAL 1 OR _split)
        message(FATAL_ERROR "${_language} cache flags do not contain exactly one selected float ABI option")
    endif()
endforeach()
file(WRITE "${ROOT}/compiler-flags"
    "C=${_C_flags}\nCXX=${_CXX_flags}\nASM=${_ASM_flags}\n")
file(WRITE "${ROOT}/selected-sdk-root" "${_selected_root}")
file(WRITE "${ROOT}/passed" "initialized and validated")
"##;

const CMAKE_PRODUCTION_INITIALIZATION_PROBE: &str = r#"
cmake_minimum_required(VERSION 3.22)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

set(AROS_TARGET_CPU arm)
set(AROS_TARGET_PLATFORM raspi)
set(AROS_TOOLCHAIN llvm)
set(GCC_CONFIG_FLOAT_ABI hard)
set(GCC_CONFIG_FLOAT_ABI hard CACHE STRING "fixture source float ABI" FORCE)
set(AROS_ABI_FLAVOUR fixture-flavour)
set(AROS_ABI_PLATFORM_SMP OFF)
set(AROS_ENABLE_MMU OFF)

include("${ENGINE}/NativeConsumerContract.cmake")
include("${ENGINE}/tests/NativeSourceSelectionFixture.cmake")
aros_test_prepare_native_consumer_selection("${ROOT}" "${ROOT}/consumer.json")

# The fixture response represents source-owned facts. These are deliberately
# independent of the board, target platform, and toolchain selector above.
set(AROS_TARGET_PROFILE arm-raspi)
set(AROS_TARGET_TRIPLE arm-unknown-aros)
set(AROS_TARGET_FAMILY fixture-family)
set(AROS_TARGET_VARIANT fixture-variant)
set(AROS_TARGET_CPU32 fixture-cpu32)
set(AROS_MESA_VERSION fixture-mesa)
set(AROS_TOOLCHAIN llvm)
set(GCC_CONFIG_FLOAT_ABI hard)
set(AROS_ABI_FLAVOUR fixture-flavour)
set(AROS_ABI_PLATFORM_SMP OFF)
set(AROS_ENABLE_MMU OFF)

file(READ "${CMAKE_CURRENT_BINARY_DIR}/native-consumer-validation-response.json" _response)
string(JSON _response REMOVE "${_response}" abi)
string(JSON _response SET "${_response}" schema "\"aros-sdk-consumer-validation-v1\"")
string(JSON _response SET "${_response}" profile "\"${AROS_TARGET_PROFILE}\"")
set(_target "{}")
string(JSON _target SET "${_target}" target_triple "\"${AROS_TARGET_TRIPLE}\"")
set(_abi "{}")
string(JSON _abi SET "${_abi}" source_cpu "\"${AROS_TARGET_CPU}\"")
string(JSON _abi SET "${_abi}" pointer_width 32)
string(JSON _abi SET "${_abi}" use_mmu false)
string(JSON _abi SET "${_abi}" flavour "\"${AROS_ABI_FLAVOUR}\"")
string(JSON _abi SET "${_abi}" platform_smp false)
string(JSON _abi SET "${_abi}" float_abi "\"${GCC_CONFIG_FLOAT_ABI}\"")
string(JSON _abi SET "${_abi}" compiler_cpu "\"cortex-a7\"")
string(JSON _abi SET "${_abi}" compiler_fpu "\"neon-vfpv4\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/native-consumer-validation-response.json" "${_response}\n")

aros_validate_native_consumer_contract()
set(AROS_DEVELOPER_INCLUDE_DIR "${CMAKE_BINARY_DIR}/SYS/Developer/include")
aros_resolve_sdk_include_root(_selected_root)
aros_lock_native_consumer_contract_identity()
set(AROS_CROSS_TOOLCHAIN_ROOT "${ROOT}/mock-release-toolchain")
file(MAKE_DIRECTORY "${AROS_CROSS_TOOLCHAIN_ROOT}")
aros_register_sdk_consumer_compiler_option_overrides()

# Supply only the files the production prefix validator requires. Except for
# real host compiler/archive-tool symlinks, every file below is an inert stub:
# this fixture tests CMake initialization, not SDK qualification.
set(_mock_prefix "${ROOT}/mock-release-toolchain")
file(MAKE_DIRECTORY
    "${_mock_prefix}/bin"
    "${_mock_prefix}/include/c++/v1"
    "${_mock_prefix}/lib/clang/17.0.0/lib/aros"
    "${_mock_prefix}/lib")
foreach(_tool IN ITEMS clang clang++ llvm-ar llvm-ranlib)
    unset(_host_tool CACHE)
    unset(_host_tool)
    find_program(_host_tool NAMES "${_tool}" REQUIRED)
    if(NOT EXISTS "${_mock_prefix}/bin/${_tool}")
        file(CREATE_LINK "${_host_tool}" "${_mock_prefix}/bin/${_tool}" SYMBOLIC)
    endif()
endforeach()
foreach(_tool IN ITEMS ld.lld llvm-nm llvm-strip llvm-objcopy llvm-objdump)
    file(WRITE "${_mock_prefix}/bin/${_tool}" "unused test-only tool stub\n")
endforeach()
foreach(_path IN ITEMS
    include/c++/v1/algorithm
    include/c++/v1/cerrno
    include/c++/v1/cinttypes
    include/c++/v1/cstddef
    include/c++/v1/cstdint
    include/c++/v1/deque
    include/c++/v1/memory
    include/c++/v1/string
    include/c++/v1/system_error
    include/c++/v1/vector
    lib/libc++.a
    lib/libc++abi.a
    lib/libunwind.a
    lib/clang/17.0.0/lib/aros/libclang_rt.builtins-armhf.a)
    file(WRITE "${_mock_prefix}/${_path}" "unused test-only SDK fixture stub\n")
endforeach()

file(WRITE "${ROOT}/probe.c" "int sdk_c_probe(void) { return 0; }\n")
file(WRITE "${ROOT}/probe.cpp" "int sdk_cxx_probe() { return 0; }\n")
file(WRITE "${ROOT}/probe.S"
    ".text\n.globl sdk_asm_probe\nsdk_asm_probe:\n    bx lr\n")

project(SdkGenericProductionCompilerInitialization LANGUAGES C CXX ASM)
aros_validate_native_consumer_compiler_target()

set(_abi_diagnostic "")
foreach(_name IN ITEMS
    CMAKE_SIZEOF_VOID_P
    CMAKE_C_SIZEOF_DATA_PTR
    CMAKE_CXX_SIZEOF_DATA_PTR
    CMAKE_ASM_SIZEOF_DATA_PTR
    CMAKE_C_COMPILER_TARGET
    CMAKE_CXX_COMPILER_TARGET
    CMAKE_ASM_COMPILER_TARGET
    CMAKE_C_COMPILER_ID
    CMAKE_CXX_COMPILER_ID
    CMAKE_ASM_COMPILER_ID
    CMAKE_C_COMPILER_ABI
    CMAKE_CXX_COMPILER_ABI)
    if(DEFINED ${_name})
        string(APPEND _abi_diagnostic "${_name}=${${_name}}\n")
    else()
        string(APPEND _abi_diagnostic "${_name}=<undefined>\n")
    endif()
endforeach()
file(WRITE "${ROOT}/compiler-abi" "${_abi_diagnostic}")

foreach(_language IN ITEMS C CXX ASM)
    get_property(_${_language}_flags CACHE "CMAKE_${_language}_FLAGS" PROPERTY VALUE)
endforeach()
file(WRITE "${ROOT}/compiler-flags"
    "C=${_C_flags}\nCXX=${_CXX_flags}\nASM=${_ASM_flags}\n")
file(WRITE "${ROOT}/selected-sdk-root" "${_selected_root}")
file(WRITE "${ROOT}/passed" "initialized with production AROS toolchain")

add_library(sdk_production_initialization_probe STATIC
    "${ROOT}/probe.c"
    "${ROOT}/probe.cpp"
    "${ROOT}/probe.S")
"#;

fn harness(probe: &Probe) -> Harness {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let build = root.join("build");
    fs::create_dir_all(&build).unwrap();
    fs::write(root.join("one.c"), "fixture-owned source input\n").unwrap();
    let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("engine")
        .canonicalize()
        .unwrap();

    let mut script = CMAKE_PROBE.to_owned();
    for (placeholder, value) in [
        ("@CPU@", probe.cpu),
        ("@TRIPLE@", probe.triple),
        ("@WIDTH@", probe.width),
        ("@FLOAT_ABI@", probe.float_abi),
        ("@TOOLCHAIN@", probe.toolchain),
        ("@FLAVOUR@", probe.abi_flavour),
        ("@COMPILER_CPU@", probe.compiler_cpu),
        ("@COMPILER_FPU@", probe.compiler_fpu),
        ("@INITIAL_C_FLAGS@", probe.initial_c_flags),
        ("@INITIAL_C_FLAGS_RELEASE@", probe.initial_c_flags_release),
        ("@NORMAL_C_FLAGS@", probe.normal_c_flags),
        (
            "@POINTER_BYTES@",
            if probe.width == "32" { "4" } else { "8" },
        ),
        ("@RESPONSE_EDIT@", probe.response_edit),
        ("@AFTER_VALIDATION@", probe.after_validation),
        ("@COMPILER_EDIT@", probe.compiler_edit),
        (
            "@CHECK_COMPILER@",
            if probe.check_compiler { "ON" } else { "OFF" },
        ),
        ("@LEGACY@", if probe.legacy { "ON" } else { "OFF" }),
        (
            "@REPEAT_APPLY@",
            if probe.repeat_apply { "ON" } else { "OFF" },
        ),
    ] {
        script = script.replace(placeholder, value);
    }
    fs::write(root.join("CMakeLists.txt"), script).unwrap();
    // A failed configure must remove both artifacts left by an earlier run.
    fs::write(build.join("aros-native-consumer-binding.json"), b"stale\n").unwrap();
    fs::write(root.join("passed"), b"stale\n").unwrap();
    Harness {
        _temp: temp,
        root,
        build,
        engine,
    }
}

fn initialization_harness(
    c_flags_init: &str,
    cxx_flags_init: &str,
    asm_flags_init: &str,
    conflict_override: bool,
) -> Harness {
    let probe = Probe::new("arm", "arm-unknown-aros", "32", "hard")
        .compiler_options("cortex-a7", "neon-vfpv4");
    let test_harness = harness(&probe);
    let _ = fs::remove_file(test_harness.root.join("passed"));
    let _ = fs::remove_file(test_harness.root.join("selected-sdk-root"));
    let _ = fs::remove_file(test_harness.root.join("compiler-flags"));
    let _ = fs::remove_file(test_harness.root.join("compile-commands"));
    let _ = fs::remove_file(test_harness.build.join("aros-native-consumer-binding.json"));
    let mut script = CMAKE_INITIALIZATION_PROBE.to_owned();
    for (placeholder, value) in [
        ("@C_FLAGS_INIT@", c_flags_init),
        ("@CXX_FLAGS_INIT@", cxx_flags_init),
        ("@ASM_FLAGS_INIT@", asm_flags_init),
        (
            "@CONFLICT_OVERRIDE@",
            if conflict_override { "ON" } else { "OFF" },
        ),
    ] {
        script = script.replace(placeholder, value);
    }
    fs::write(test_harness.root.join("CMakeLists.txt"), script).unwrap();
    // Model the production toolchain's ABI try_compile environment. Apple's
    // ar/ranlib can exit successfully while discarding foreign ELF members;
    // an empty archive provides no pointer-width evidence to CMake.
    fs::write(
        test_harness.root.join("toolchain.cmake"),
        r"set(CMAKE_SYSTEM_NAME Generic)
set(CMAKE_SYSTEM_PROCESSOR arm)
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
foreach(_language IN ITEMS C CXX ASM)
    set(CMAKE_${_language}_COMPILER_TARGET arm-unknown-aros)
endforeach()
find_program(CMAKE_AR NAMES llvm-ar REQUIRED)
find_program(CMAKE_RANLIB NAMES llvm-ranlib REQUIRED)
",
    )
    .unwrap();
    fs::create_dir_all(test_harness.root.join("mock-release-toolchain")).unwrap();
    fs::write(
        test_harness.root.join("probe.c"),
        "int sdk_c_probe(void) { return 0; }\n",
    )
    .unwrap();
    fs::write(
        test_harness.root.join("probe.cpp"),
        "int sdk_cxx_probe() { return 0; }\n",
    )
    .unwrap();
    fs::write(
        test_harness.root.join("probe.S"),
        ".text\n.globl sdk_asm_probe\nsdk_asm_probe:\n    bx lr\n",
    )
    .unwrap();
    test_harness
}

fn production_initialization_harness() -> Harness {
    let probe = Probe::new("arm", "arm-unknown-aros", "32", "hard")
        .compiler_options("cortex-a7", "neon-vfpv4");
    let test_harness = harness(&probe);
    for path in [
        test_harness.root.join("passed"),
        test_harness.root.join("selected-sdk-root"),
        test_harness.root.join("compiler-flags"),
        test_harness.build.join("compile_commands.json"),
        test_harness.root.join("compiler-abi"),
        test_harness.build.join("aros-native-consumer-binding.json"),
    ] {
        let _ = fs::remove_file(path);
    }
    fs::write(
        test_harness.root.join("CMakeLists.txt"),
        CMAKE_PRODUCTION_INITIALIZATION_PROBE,
    )
    .unwrap();

    let toolchain = format!(
        "set(AROS_TOOLCHAIN llvm CACHE STRING \"\" FORCE)\n\
         set(AROS_TARGET_CPU arm CACHE STRING \"\" FORCE)\n\
         set(AROS_TARGET_PLATFORM raspi CACHE STRING \"\" FORCE)\n\
         set(AROS_CROSS_TOOLCHAIN_ROOT \"{}\" CACHE PATH \"\" FORCE)\n\
         include(\"{}\")\n",
        test_harness.root.join("mock-release-toolchain").display(),
        test_harness.engine.join("toolchains/AROS.cmake").display()
    );
    fs::write(
        test_harness.root.join("production-toolchain.cmake"),
        toolchain,
    )
    .unwrap();
    test_harness
}

fn configure(harness: &Harness, reconfigure_fail: bool) -> Outcome {
    let mut command = Command::new("cmake");
    command
        .arg("-S")
        .arg(&harness.root)
        .arg("-B")
        .arg(&harness.build)
        .arg(format!("-DENGINE={}", harness.engine.display()))
        .arg(format!("-DROOT={}", harness.root.display()));
    if reconfigure_fail {
        command.arg("-DRECONFIGURE_FAIL=ON");
    }
    let output = command.output().unwrap();
    Outcome {
        output,
        marker: harness.root.join("passed").exists(),
        selected: fs::read(harness.root.join("selected-sdk-root")).ok(),
        compiler_flags: fs::read(harness.root.join("compiler-flags")).ok(),
        compile_commands: fs::read(harness.root.join("compile-commands")).ok(),
        initialization_abi: None,
        configure_log: None,
        response: fs::read(
            harness
                .build
                .join("native-consumer-validation-response.json"),
        )
        .ok(),
        binding: fs::read(harness.build.join("aros-native-consumer-binding.json")).ok(),
    }
}

fn configure_initialization(
    harness: &Harness,
    c_flags: &str,
    cxx_flags: &str,
    asm_flags: &str,
) -> Outcome {
    configure_initialization_with_cache(harness, c_flags, cxx_flags, asm_flags, &[])
}

fn configure_initialization_with_cache(
    harness: &Harness,
    c_flags: &str,
    cxx_flags: &str,
    asm_flags: &str,
    cache_variables: &[(&str, &str)],
) -> Outcome {
    let mut command = Command::new("cmake");
    command
        .arg("-S")
        .arg(&harness.root)
        .arg("-B")
        .arg(&harness.build)
        .arg(format!("-DENGINE={}", harness.engine.display()))
        .arg(format!("-DROOT={}", harness.root.display()))
        .arg(format!(
            "-DCMAKE_TOOLCHAIN_FILE={}",
            harness.root.join("toolchain.cmake").display()
        ))
        .arg("-DCMAKE_C_COMPILER=clang")
        .arg("-DCMAKE_CXX_COMPILER=clang++")
        .arg("-DCMAKE_ASM_COMPILER=clang");
    for (name, value) in cache_variables {
        command.arg(format!("-D{name}={value}"));
    }
    let output = command
        .env("CFLAGS", c_flags)
        .env("CXXFLAGS", cxx_flags)
        .env("ASMFLAGS", asm_flags)
        .env_remove("AROS_CROSS_TOOLCHAIN_ROOT")
        .output()
        .unwrap();
    Outcome {
        output,
        marker: harness.root.join("passed").exists(),
        selected: fs::read(harness.root.join("selected-sdk-root")).ok(),
        compiler_flags: fs::read(harness.root.join("compiler-flags")).ok(),
        compile_commands: fs::read(harness.build.join("compile_commands.json")).ok(),
        initialization_abi: fs::read(harness.root.join("compiler-abi")).ok(),
        configure_log: fs::read(harness.build.join("CMakeFiles/CMakeConfigureLog.yaml")).ok(),
        response: fs::read(
            harness
                .build
                .join("native-consumer-validation-response.json"),
        )
        .ok(),
        binding: fs::read(harness.build.join("aros-native-consumer-binding.json")).ok(),
    }
}

fn configure_production_initialization(
    harness: &Harness,
    c_flags: &str,
    cxx_flags: &str,
    asm_flags: &str,
) -> Outcome {
    let output = Command::new("cmake")
        .arg("-S")
        .arg(&harness.root)
        .arg("-B")
        .arg(&harness.build)
        .arg(format!("-DENGINE={}", harness.engine.display()))
        .arg(format!("-DROOT={}", harness.root.display()))
        .arg(format!(
            "-DCMAKE_TOOLCHAIN_FILE={}",
            harness.root.join("production-toolchain.cmake").display()
        ))
        .env("CFLAGS", c_flags)
        .env("CXXFLAGS", cxx_flags)
        .env("ASMFLAGS", asm_flags)
        .env_remove("AROS_CROSS_TOOLCHAIN_ROOT")
        .output()
        .unwrap();
    Outcome {
        output,
        marker: harness.root.join("passed").exists(),
        selected: fs::read(harness.root.join("selected-sdk-root")).ok(),
        compiler_flags: fs::read(harness.root.join("compiler-flags")).ok(),
        compile_commands: fs::read(harness.build.join("compile_commands.json")).ok(),
        initialization_abi: fs::read(harness.root.join("compiler-abi")).ok(),
        configure_log: fs::read(harness.build.join("CMakeFiles/CMakeConfigureLog.yaml")).ok(),
        response: fs::read(
            harness
                .build
                .join("native-consumer-validation-response.json"),
        )
        .ok(),
        binding: fs::read(harness.build.join("aros-native-consumer-binding.json")).ok(),
    }
}

fn diagnostic(outcome: &Outcome) -> String {
    let mut diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&outcome.output.stdout),
        String::from_utf8_lossy(&outcome.output.stderr)
    );
    if !outcome.output.status.success() {
        if let Some(abi) = &outcome.initialization_abi {
            diagnostic.push_str("\nInitialization ABI values:\n");
            diagnostic.push_str(&String::from_utf8_lossy(abi));
        }
        if let Some(configure_log) = &outcome.configure_log {
            let log = String::from_utf8_lossy(configure_log);
            let tail = log
                .char_indices()
                .rev()
                .nth(12_000)
                .map_or(0, |(index, _)| index);
            diagnostic.push_str("\nCMakeConfigureLog.yaml tail:\n");
            diagnostic.push_str(&log[tail..]);
        }
    }
    diagnostic
}

fn assert_failure(outcome: &Outcome, expected: &str) {
    let error = diagnostic(outcome);
    let normalized_error = error.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized_expected = expected.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        !outcome.output.status.success(),
        "unexpected configure success; expected rejection: {expected}\n{error}"
    );
    assert!(!outcome.marker, "failure left a pass marker\n{error}");
    assert!(
        outcome.binding.is_none(),
        "failure left a consumer binding\n{error}"
    );
    assert!(
        normalized_error.contains(&normalized_expected),
        "missing {expected:?}\n{error}"
    );
}

fn assert_success(harness: &Harness, outcome: &Outcome) {
    let error = diagnostic(outcome);
    assert!(
        outcome.output.status.success() && outcome.marker,
        "generic SDK configure failed\n{error}"
    );
    let response = outcome.response.as_ref().expect("mock validation response");
    assert_eq!(
        outcome.binding.as_ref(),
        Some(response),
        "deferred binding must preserve exact response bytes"
    );
    let expected_root = format!("{}/SYS/Developer/include", harness.build.display());
    assert_eq!(
        outcome.selected.as_deref(),
        Some(expected_root.as_bytes()),
        "generic SDK consumer must select the source Developer include root"
    );
}

#[test]
fn generic_sdk_targets_bind_and_check_simulated_clang_metadata() {
    for probe in [
        Probe::new("x86_64", "x86_64-unknown-aros", "64", "").compiler(),
        Probe::new("aarch64", "aarch64-unknown-aros", "64", "").compiler(),
        Probe::new("arm", "arm-unknown-aros", "32", "hard").compiler(),
    ] {
        let test_harness = harness(&probe);
        let outcome = configure(&test_harness, false);
        assert_success(&test_harness, &outcome);
        let expected_flags = if probe.cpu == "arm" {
            "C=-mfloat-abi=hard\nCXX=-mfloat-abi=hard\nASM=-mfloat-abi=hard\n"
        } else {
            "C=\nCXX=\nASM=\n"
        };
        assert_eq!(
            String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap()),
            expected_flags,
            "generic SDK without compiler option facts must preserve existing flags"
        );
    }
}

#[test]
fn source_compiler_options_reach_each_cache_flag_once_and_legacy_is_unchanged() {
    let probe = Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
        .compiler_options("future-cpu+rev2", "future-fpu.v4")
        .initial_c_flags("-O2 -mcpu=future-cpu+rev2 -mfpu=future-fpu.v4 -fno-common")
        .compiler()
        .repeat_apply();
    let both_harness = harness(&probe);
    let outcome = configure(&both_harness, false);
    assert_success(&both_harness, &outcome);
    let flags = String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap());
    assert_eq!(
        flags,
        "C=-O2 -mcpu=future-cpu+rev2 -mfpu=future-fpu.v4 -fno-common\n\
         CXX=-mcpu=future-cpu+rev2 -mfpu=future-fpu.v4\n\
         ASM=-mcpu=future-cpu+rev2 -mfpu=future-fpu.v4\n"
    );

    for (cpu, fpu, expected) in [
        ("future-cpu", "", "-mcpu=future-cpu"),
        ("", "future-fpu", "-mfpu=future-fpu"),
    ] {
        let probe = Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
            .compiler_options(cpu, fpu)
            .compiler();
        let test_harness = harness(&probe);
        let outcome = configure(&test_harness, false);
        assert_success(&test_harness, &outcome);
        let flags = String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap());
        for language in ["C", "CXX", "ASM"] {
            let line = flags
                .lines()
                .find(|line| line.starts_with(&format!("{language}=")))
                .unwrap();
            assert_eq!(line.matches(expected).count(), 1, "{language}: {line}");
            let unexpected = if cpu.is_empty() { "-mcpu=" } else { "-mfpu=" };
            assert!(!line.contains(unexpected), "{language}: {line}");
        }
    }

    let shadowed_cache = Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
        .compiler_options("future-cpu", "")
        .initial_c_flags("-cached-base")
        .normal_c_flags("-O1 -fno-common")
        .compiler()
        .repeat_apply();
    let shadowed_harness = harness(&shadowed_cache);
    let outcome = configure(&shadowed_harness, false);
    assert_success(&shadowed_harness, &outcome);
    let flags = String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap());
    assert!(
        flags.starts_with("C=-O1 -fno-common -mcpu=future-cpu\n"),
        "{flags}"
    );

    let float_abi_shadow = Probe::new("arm", "arm-unknown-aros", "32", "hard")
        .normal_c_flags("-O1")
        .compiler();
    let float_abi_harness = harness(&float_abi_shadow);
    let outcome = configure(&float_abi_harness, false);
    assert_success(&float_abi_harness, &outcome);
    let flags = String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap());
    assert!(
        flags.starts_with("C=-O1 -mfloat-abi=hard\n"),
        "explicit normal shadow dropped source float ABI: {flags}"
    );

    let legacy = Probe::new("riscv32", "riscv32-aros", "32", "ilp32f")
        .toolchain("gnu")
        .legacy()
        .compiler();
    let test_harness = harness(&legacy);
    let outcome = configure(&test_harness, false);
    assert_success(&test_harness, &outcome);
    assert_eq!(
        String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap()),
        "C=\nCXX=\nASM=\n"
    );
}

#[test]
fn source_compiler_arch_is_exclusive_and_reaches_each_driver_flag() {
    let select_arch = r#"string(JSON _abi SET "${_abi}" compiler_arch "\"armv7-a+fp\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#;
    let arch_probe = Probe::new("arm", "arm-unknown-aros", "32", "hard")
        .response(select_arch)
        .compiler();
    let arch_harness = harness(&arch_probe);
    let outcome = configure(&arch_harness, false);
    assert_success(&arch_harness, &outcome);
    let flags = String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap());
    assert_eq!(
        flags,
        "C=-mfloat-abi=hard -march=armv7-a+fp\n\
         CXX=-mfloat-abi=hard -march=armv7-a+fp\n\
         ASM=-mfloat-abi=hard -march=armv7-a+fp\n"
    );
    for language in ["C", "CXX", "ASM"] {
        let line = flags
            .lines()
            .find(|line| line.starts_with(&format!("{language}=")))
            .unwrap();
        assert_eq!(line.matches("-march=armv7-a+fp").count(), 1, "{line}");
        assert!(!line.contains("-mcpu="), "{line}");
    }

    let ambiguous_arch_and_cpu = Probe::new("arm", "arm-unknown-aros", "32", "hard").response(
        r#"string(JSON _abi SET "${_abi}" compiler_arch "\"armv7-a+fp\"")
string(JSON _abi SET "${_abi}" compiler_cpu "\"cortex-a7\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
    );
    assert_failure(
        &configure(&harness(&ambiguous_arch_and_cpu), false),
        "SDK target compiler_arch and compiler_cpu are mutually exclusive",
    );

    let invalid_arch = Probe::new("arm", "arm-unknown-aros", "32", "hard").response(
        r#"string(JSON _abi SET "${_abi}" compiler_arch "\"armv7/a\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
    );
    assert_failure(
        &configure(&harness(&invalid_arch), false),
        "SDK target compiler_arch: must be 1..=64 ASCII characters",
    );

    let configuration_override = Probe::new("arm", "arm-unknown-aros", "32", "hard")
        .response(select_arch)
        .after(
            "set(CMAKE_CONFIGURATION_TYPES Coverage)\nset(CMAKE_C_FLAGS_COVERAGE_INIT \"-march=other-arch\")",
        )
        .compiler();
    assert_failure(
        &configure(&harness(&configuration_override), false),
        "C per-configuration flags cannot override source-selected -march",
    );
}

#[test]
fn source_code_model_reaches_each_driver_once_without_architecture_inference() {
    let select_model = r#"string(JSON _abi SET "${_abi}" compiler_code_model "\"large\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#;
    for (cpu, triple) in [
        ("x86_64", "x86_64-unknown-aros"),
        ("aarch64", "aarch64-unknown-aros"),
    ] {
        let selected = Probe::new(cpu, triple, "64", "")
            .response(select_model)
            .initial_c_flags("-O2 -mcmodel=large -fno-common")
            .repeat_apply()
            .compiler();
        let test_harness = harness(&selected);
        let outcome = configure(&test_harness, false);
        assert_success(&test_harness, &outcome);
        assert_eq!(
            String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap()),
            "C=-O2 -mcmodel=large -fno-common\nCXX=-mcmodel=large\nASM=-mcmodel=large\n"
        );
        let second = configure(&test_harness, false);
        assert_success(&test_harness, &second);
        assert_eq!(second.compiler_flags, outcome.compiler_flags);

        let absent = Probe::new(cpu, triple, "64", "")
            .initial_c_flags("-O2 -fno-common")
            .compiler();
        let absent_harness = harness(&absent);
        let outcome = configure(&absent_harness, false);
        assert_success(&absent_harness, &outcome);
        assert_eq!(
            String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap()),
            "C=-O2 -fno-common\nCXX=\nASM=\n"
        );
    }

    let generic = Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
        .response(
            r#"string(JSON _abi SET "${_abi}" compiler_code_model "\"future-model.v2\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
        )
        .compiler();
    let test_harness = harness(&generic);
    let outcome = configure(&test_harness, false);
    assert_success(&test_harness, &outcome);
    assert_eq!(
        String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap()),
        "C=-mcmodel=future-model.v2\nCXX=-mcmodel=future-model.v2\nASM=-mcmodel=future-model.v2\n"
    );
}

#[test]
fn source_code_model_rejects_untyped_undeclared_conflicting_and_configuration_flags() {
    let select_model = r#"string(JSON _abi SET "${_abi}" compiler_code_model "\"large\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#;
    for (probe, expected) in [
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .response(
                    r#"string(JSON _abi SET "${_abi}" compiler_code_model true)
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
                ),
            "SDK target.compiler_code_model: must be STRING",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .response(
                    r#"string(JSON _abi SET "${_abi}" compiler_code_model "\"large;-O0\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
                ),
            "SDK target compiler_code_model: must be 1..=64 ASCII characters",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .initial_c_flags("-mcmodel=large")
                .compiler(),
            "C flags contain an undeclared -mcmodel option",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .response(select_model)
                .initial_c_flags("-mcmodel=small")
                .compiler(),
            "C flags must contain exactly one selected -mcmodel=large option",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .response(select_model)
                .initial_c_flags("-mcmodel=large -mcmodel=large")
                .compiler(),
            "C flags must contain exactly one selected -mcmodel=large option",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .response(select_model)
                .initial_c_flags("-mcmodel large")
                .compiler(),
            "C flags use split -mcmodel syntax",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .response(select_model)
                .after("set(CMAKE_CONFIGURATION_TYPES Coverage)\nset(CMAKE_C_FLAGS_COVERAGE_INIT \"-mcmodel=small\")")
                .compiler(),
            "C per-configuration flags cannot override source-selected -mcmodel",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .response(select_model)
                .after("set(CMAKE_TRY_COMPILE_CONFIGURATION Coverage)\nset(CMAKE_C_FLAGS_COVERAGE \"-mcmodel=small\")")
                .compiler(),
            "C per-configuration flags cannot override source-selected -mcmodel",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .response(select_model)
                .after("set(CMAKE_TRY_COMPILE_CONFIGURATION Coverage)\nset(CMAKE_CXX_FLAGS_COVERAGE_INIT \"-mcmodel=small\")")
                .compiler(),
            "CXX per-configuration flags cannot override source-selected -mcmodel",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .response(select_model)
                .after("set(CMAKE_TRY_COMPILE_CONFIGURATION Coverage)\nset(CMAKE_ASM_FLAGS_COVERAGE \"-mcmodel=small\")")
                .compiler(),
            "ASM per-configuration flags cannot override source-selected -mcmodel",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .response(select_model)
                .compiler_edit("set(CMAKE_CXX_FLAGS_DEBUG \"-mcmodel=large\")"),
            "CXX per-configuration flags cannot override source-selected -mcmodel",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .response(select_model)
                .after("set(CMAKE_ASM_FLAGS_RELEASE_INIT \"-mcmodel=large\")")
                .compiler(),
            "ASM per-configuration flags cannot override source-selected -mcmodel",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .response(select_model)
                .compiler()
                .compiler_edit("set(CMAKE_ASM_FLAGS \"-mcmodel=small\")"),
            "ASM flags must contain exactly one selected -mcmodel=large option",
        ),
    ] {
        let outcome = configure(&harness(&probe), false);
        assert_failure(&outcome, expected);
    }
}

#[test]
fn source_compiler_option_facts_are_typed_and_portable() {
    for (edit, expected) in [
        (
            r#"string(JSON _abi SET "${_abi}" compiler_cpu true)
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            "SDK target.compiler_cpu: must be STRING",
        ),
        (
            r#"string(JSON _abi SET "${_abi}" compiler_fpu "\"cpu name\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            "SDK target compiler_fpu: must be 1..=64 ASCII characters",
        ),
        (
            r#"string(JSON _abi SET "${_abi}" compiler_cpu "\"_cpu\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            "SDK target compiler_cpu: must be 1..=64 ASCII characters",
        ),
    ] {
        let probe = Probe::new("x86_64", "x86_64-unknown-aros", "64", "").response(edit);
        let outcome = configure(&harness(&probe), false);
        assert_failure(&outcome, expected);
    }
}

#[test]
fn source_target_fields_are_closed_typed_and_match_the_requested_selection() {
    let probes = [
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "").response(
                r#"string(JSON _target SET "${_target}" target_triple "\"other-unknown-aros\"")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "source SDK target differs from the requested configuration",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "").response(
                r#"string(JSON _abi SET "${_abi}" pointer_width 16)
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "SDK pointer width must be explicit 32 or 64",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "").response(
                r#"string(JSON _abi SET "${_abi}" pointer_width "\"64\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "executable SDK ABI.pointer_width: must be NUMBER",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "").response(
                r#"string(JSON _abi SET "${_abi}" use_mmu "\"false\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "executable SDK ABI.use_mmu: must be BOOLEAN",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "").response(
                r#"string(JSON _abi SET "${_abi}" flavour "\"other-flavour\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "source SDK target differs from the requested configuration",
        ),
        (
            Probe::new("arm", "arm-unknown-aros", "32", "hard").response(
                r#"string(JSON _abi SET "${_abi}" float_abi "\"soft\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "source SDK target differs from the requested configuration",
        ),
        (
            Probe::new("arm", "arm-unknown-aros", "32", "hard").response(
                r#"string(JSON _abi REMOVE "${_abi}" float_abi)
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "ARM SDK target requires an explicit source float_abi",
        ),
        (
            Probe::new("arm", "arm-unknown-aros", "32", "fast"),
            "ARM SDK target requires an explicit source float_abi",
        ),
    ];
    for (probe, expected) in probes {
        let outcome = configure(&harness(&probe), false);
        assert_failure(&outcome, expected);
    }
}

#[test]
fn legacy_and_generic_response_shapes_cannot_be_mixed() {
    let generic_with_legacy_abi = Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
        .response(r#"string(JSON _response SET "${_response}" abi "${_legacy_abi}")"#);
    let outcome = configure(&harness(&generic_with_legacy_abi), false);
    assert_failure(&outcome, "consumer response: has unknown field 'abi'");

    let legacy_with_generic_target = Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
        .toolchain("gnu")
        .response(r#"string(JSON _response SET "${_response}" schema "\"aros-native-consumer-validation-v1\"")"#);
    let outcome = configure(&harness(&legacy_with_generic_target), false);
    assert_failure(&outcome, "consumer response: has unknown field 'target'");

    let generic_on_gnu = Probe::new("x86_64", "x86_64-unknown-aros", "64", "").toolchain("gnu");
    let outcome = configure(&harness(&generic_on_gnu), false);
    assert_failure(
        &outcome,
        "generic SDK compiler adapter requires the explicit LLVM toolchain",
    );
}

#[test]
fn configured_compiler_guards_reject_mismatches_and_overrides() {
    let cases = [
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_edit("set(CMAKE_CXX_COMPILER_ID GNU)"),
            "CXX driver must be Clang with the exact SDK target triple",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_edit("set(CMAKE_ASM_COMPILER_TARGET other-unknown-aros)"),
            "ASM driver must be Clang with the exact SDK target triple",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_edit("set(CMAKE_ASM_COMPILER_ID GNU)"),
            "ASM driver must be Clang with the exact SDK target triple",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .compiler_edit("set(CMAKE_SIZEOF_VOID_P 4)"),
            "detected compiler pointer width differs from the SDK target",
        ),
        (
            Probe::new("arm", "arm-unknown-aros", "32", "hard")
                .compiler_edit("set(CMAKE_C_FLAGS \"-mfloat-abi=soft\")"),
            "C float ABI flags differ from the selected SDK target",
        ),
        (
            Probe::new("arm", "arm-unknown-aros", "32", "hard")
                .compiler_edit("set(CMAKE_CXX_FLAGS \"-mfloat-abi=hard -mfloat-abi=hard\")"),
            "ARM SDK requires exactly one selected float ABI flag per driver",
        ),
        (
            Probe::new("arm", "arm-unknown-aros", "32", "hard")
                .compiler_edit("set(CMAKE_C_FLAGS_RELEASE \"-mfloat-abi=hard\")"),
            "C float ABI flags differ from the selected SDK target",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .compiler_edit("set(CMAKE_C_FLAGS \"--target=other-unknown-aros\")"),
            "C flags cannot override the selected target or sysroot",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_options("future-cpu", "future-fpu")
                .compiler_edit("set(CMAKE_C_FLAGS \"-mcpu=future-cpu -mcpu=future-cpu\")"),
            "C flags must contain exactly one selected -mcpu=future-cpu option",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_options("future-cpu", "")
                .initial_c_flags("-mcpu=other-cpu")
                .compiler(),
            "C flags must contain exactly one selected -mcpu=future-cpu option",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_options("future-cpu", "future-fpu")
                .compiler_edit("set(CMAKE_C_FLAGS \"-mcpu=future-cpu -mfpu=other-fpu\")"),
            "C flags must contain exactly one selected -mfpu=future-fpu option",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_options("future-cpu", "future-fpu")
                .compiler_edit("set(CMAKE_C_FLAGS \"-mcpu future-cpu\")"),
            "C flags use split -mcpu syntax",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_options("future-cpu", "future-fpu")
                .compiler_edit("set(CMAKE_C_FLAGS_RELEASE \"-mfpu=future-fpu\")"),
            "C per-configuration flags cannot override source-selected -mfpu",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_options("future-cpu", "")
                .initial_c_flags_release("-mcpu=future-cpu")
                .compiler(),
            "C per-configuration flags cannot override source-selected -mcpu",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_options("future-cpu", "")
                .after("set(CMAKE_CONFIGURATION_TYPES Coverage)\nset(CMAKE_C_FLAGS_COVERAGE_INIT \"-mcpu=future-cpu\")")
                .compiler(),
            "C per-configuration flags cannot override source-selected -mcpu",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_options("future-cpu", "")
                .after("set(CMAKE_BUILD_TYPE Coverage)\nset(CMAKE_C_FLAGS_COVERAGE \"-mcpu=future-cpu\")")
                .compiler(),
            "C per-configuration flags cannot override source-selected -mcpu",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .after("set(CMAKE_CONFIGURATION_TYPES Coverage)\nset(CMAKE_C_FLAGS_COVERAGE_INIT \"--target=other-unknown-aros\")")
                .compiler(),
            "C flags cannot override the selected target or sysroot",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .after("set(CMAKE_BUILD_TYPE Coverage)\nset(CMAKE_CXX_FLAGS_COVERAGE \"-isysroot /other/sysroot\")")
                .compiler(),
            "CXX flags cannot override the selected target or sysroot",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .after("set(CMAKE_CONFIGURATION_TYPES Coverage)\nset(CMAKE_ASM_FLAGS_COVERAGE \"--sysroot=/other/sysroot\")")
                .compiler(),
            "ASM flags cannot override the selected target or sysroot",
        ),
        (
            Probe::new("arm", "arm-unknown-aros", "32", "hard")
                .after("set(CMAKE_CONFIGURATION_TYPES Coverage)\nset(CMAKE_ASM_FLAGS_COVERAGE \"-mfloat-abi=soft\")")
                .compiler(),
            "ASM per-configuration flags cannot override source-selected float ABI",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_options("future-cpu", "")
                .compiler_edit("set(CMAKE_C_FLAGS \"\")"),
            "C flags require exactly one selected -mcpu=future-cpu option",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_edit("set(CMAKE_C_FLAGS \"-mcpu=unselected-cpu\")"),
            "C flags contain an undeclared -mcpu option",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .initial_c_flags("-mcpu=unselected-cpu")
                .compiler(),
            "C flags contain an undeclared -mcpu option",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_options("future-cpu", "")
                .compiler_edit("set(CMAKE_C_FLAGS \"-mcpu=future-cpu -march=generic\")"),
            "C flags cannot override source-selected compiler_cpu with -march=generic",
        ),
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_options("future-cpu", "")
                .compiler_edit("set(CMAKE_C_FLAGS \"-mcpu=future-cpu -march generic\")"),
            "C flags cannot override source-selected compiler_cpu with -march",
        ),
    ];
    for (probe, expected) in cases {
        let outcome = configure(&harness(&probe), false);
        assert_failure(&outcome, expected);
    }
}

#[test]
fn changed_selection_or_contract_after_validation_invalidates_admission() {
    for (probe, expected) in [
        (Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
            .after("set(AROS_TARGET_CPU aarch64)\naros_resolve_sdk_include_root(_late_root)"),
            "AROS_TARGET_CPU changed after validation"),
        (Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
            .after("file(APPEND \"${AROS_NATIVE_CONSUMER_CONTRACT}\" \"changed\")\naros_resolve_sdk_include_root(_late_root)"),
            "contract changed after validation"),
    ] {
        let outcome = configure(&harness(&probe), false);
        assert_failure(&outcome, expected);
    }
}

#[test]
fn failed_reconfigure_removes_the_previous_binding_and_marker() {
    let probe = Probe::new("x86_64", "x86_64-unknown-aros", "64", "");
    let test_harness = harness(&probe);
    let first = configure(&test_harness, false);
    assert_success(&test_harness, &first);
    let second = configure(&test_harness, true);
    assert_failure(
        &second,
        "validation response differs from the requested binding",
    );
}

#[test]
fn language_rule_overrides_preserve_init_and_environment_flags_in_real_commands() {
    let test_harness = initialization_harness(
        "-mfloat-abi=hard -g",
        "-mfloat-abi=hard -g1",
        "-mfloat-abi=hard -g",
        false,
    );
    let outcome = configure_initialization(&test_harness, "-O0", "-O1", "-O2");
    assert_success(&test_harness, &outcome);
    let flags = String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap());
    for (language, environment, initial) in [
        ("C", "-O0", "-g"),
        ("CXX", "-O1", "-g1"),
        ("ASM", "-O2", "-g"),
    ] {
        let line = flags
            .lines()
            .find(|line| line.starts_with(&format!("{language}=")))
            .unwrap();
        assert!(line.contains(environment), "{language}: {line}");
        assert!(line.contains(initial), "{language}: {line}");
        assert_eq!(
            line.matches("-mcpu=cortex-a7").count(),
            1,
            "{language}: {line}"
        );
        assert_eq!(
            line.matches("-mfpu=neon-vfpv4").count(),
            1,
            "{language}: {line}"
        );
        assert_eq!(
            line.matches("-mfloat-abi=hard").count(),
            1,
            "{language}: {line}"
        );
    }
    let commands = String::from_utf8_lossy(outcome.compile_commands.as_ref().unwrap());
    assert_eq!(commands.matches("-mcpu=cortex-a7").count(), 3);
    assert_eq!(commands.matches("-mfpu=neon-vfpv4").count(), 3);
    assert_eq!(commands.matches("-mfloat-abi=hard").count(), 3);
    for source in ["probe.c", "probe.cpp", "probe.S"] {
        assert!(
            commands.contains(source),
            "missing {source} compile command"
        );
    }
    let build = Command::new("cmake")
        .arg("--build")
        .arg(&test_harness.build)
        .arg("--target")
        .arg("sdk_compiler_options_probe")
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "real ARM C/CXX/ASM archive build failed:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let archive = test_harness.build.join("libsdk_compiler_options_probe.a");
    let archive_bytes = fs::read(archive).unwrap();
    for symbol in ["sdk_c_probe", "sdk_cxx_probe", "sdk_asm_probe"] {
        assert!(
            archive_bytes
                .windows(symbol.len())
                .any(|window| window == symbol.as_bytes()),
            "real archive lost the {symbol} object"
        );
    }

    // Reconfiguration must reuse CMake's initialized cache values rather
    // than adding the selected option again.
    let reconfigured = configure_initialization(&test_harness, "-O0", "-O1", "-O2");
    assert_success(&test_harness, &reconfigured);
    let flags = String::from_utf8_lossy(reconfigured.compiler_flags.as_ref().unwrap());
    for language in ["C", "CXX", "ASM"] {
        let line = flags
            .lines()
            .find(|line| line.starts_with(&format!("{language}=")))
            .unwrap();
        assert_eq!(
            line.matches("-mcpu=cortex-a7").count(),
            1,
            "{language}: {line}"
        );
    }
}

#[test]
fn try_compile_configuration_guards_preserve_unrelated_custom_flags() {
    // Coverage is selected only for compiler ABI try_compile calls. Neither
    // the project build type nor the generator's configuration list selects it.
    let test_harness = initialization_harness(
        "-mfloat-abi=hard -g",
        "-mfloat-abi=hard -g1",
        "-mfloat-abi=hard -g",
        false,
    );
    let cache_variables = [
        ("CMAKE_TRY_COMPILE_CONFIGURATION", "Coverage"),
        ("CMAKE_BUILD_TYPE", ""),
        ("CMAKE_C_FLAGS_COVERAGE", "-O3 -fno-common"),
        ("CMAKE_CXX_FLAGS_COVERAGE", "-O2 -g1"),
        ("CMAKE_ASM_FLAGS_COVERAGE", "-g"),
    ];
    let outcome =
        configure_initialization_with_cache(&test_harness, "-O0", "-O1", "-O2", &cache_variables);
    assert_success(&test_harness, &outcome);

    let abi = String::from_utf8_lossy(outcome.initialization_abi.as_ref().unwrap());
    assert!(abi.contains("CMAKE_SIZEOF_VOID_P=4\n"), "{abi}");
    let commands = String::from_utf8_lossy(outcome.compile_commands.as_ref().unwrap());
    for (source, flag) in [("probe.c", "-O0"), ("probe.cpp", "-O1"), ("probe.S", "-O2")] {
        assert!(
            commands.contains(source),
            "missing {source} compile command"
        );
        assert!(
            commands.contains(flag),
            "missing unrelated {flag} for {source}"
        );
    }

    let cache = fs::read_to_string(test_harness.build.join("CMakeCache.txt")).unwrap();
    let cache_value = |name: &str| {
        cache
            .lines()
            .find(|line| line.starts_with(&format!("{name}:")))
            .and_then(|line| line.split_once('=').map(|(_, value)| value))
    };
    assert_eq!(
        cache_value("CMAKE_TRY_COMPILE_CONFIGURATION"),
        Some("Coverage")
    );
    assert_eq!(cache_value("CMAKE_BUILD_TYPE"), Some(""));
    if let Some(configurations) = cache_value("CMAKE_CONFIGURATION_TYPES") {
        assert!(
            !configurations
                .split(';')
                .any(|configuration| configuration.eq_ignore_ascii_case("Coverage")),
            "CMAKE_CONFIGURATION_TYPES unexpectedly selects Coverage: {configurations}"
        );
    }
    for (name, value) in [
        ("CMAKE_C_FLAGS_COVERAGE", "-O3 -fno-common"),
        ("CMAKE_CXX_FLAGS_COVERAGE", "-O2 -g1"),
        ("CMAKE_ASM_FLAGS_COVERAGE", "-g"),
    ] {
        assert_eq!(cache_value(name), Some(value), "{name} was not preserved");
    }
}

#[test]
fn try_compile_only_configuration_rejects_conflicts_for_each_language() {
    for (language, variable, conflict, expected) in [
        (
            "C",
            "CMAKE_C_FLAGS_COVERAGE",
            "-mcpu=cortex-a9",
            "C per-configuration flags cannot override source-selected -mcpu",
        ),
        (
            "CXX",
            "CMAKE_CXX_FLAGS_COVERAGE",
            "-mfpu=vfpv3",
            "CXX per-configuration flags cannot override source-selected -mfpu",
        ),
        (
            "ASM",
            "CMAKE_ASM_FLAGS_COVERAGE",
            "-mfloat-abi=soft",
            "ASM per-configuration flags cannot override source-selected float ABI",
        ),
    ] {
        let test_harness = initialization_harness("", "", "", false);
        let mut cache_variables = vec![
            ("CMAKE_TRY_COMPILE_CONFIGURATION", "Coverage"),
            ("CMAKE_BUILD_TYPE", ""),
            ("CMAKE_C_FLAGS_COVERAGE", "-O3"),
            ("CMAKE_CXX_FLAGS_COVERAGE", "-O2"),
            ("CMAKE_ASM_FLAGS_COVERAGE", "-g"),
        ];
        let selected_index = cache_variables
            .iter_mut()
            .position(|(name, _)| *name == variable)
            .expect("the selected language has a custom configuration variable");
        cache_variables[selected_index].1 = conflict;

        let outcome = configure_initialization_with_cache(
            &test_harness,
            "-O0",
            "-O1",
            "-O2",
            &cache_variables,
        );
        assert_failure(&outcome, expected);
        assert!(
            !outcome.marker,
            "{language} custom configuration conflict left a success marker"
        );
    }
}

#[test]
fn language_rule_overrides_reject_conflicting_environment_and_init_options() {
    for (language, c, cxx, asm) in [
        ("C", "-O0 -mcpu=cortex-a9", "-O1", "-O2"),
        ("CXX", "-O0", "-O1 -mcpu=cortex-a9", "-O2"),
        ("ASM", "-O0", "-O1", "-O2 -mcpu=cortex-a9"),
    ] {
        let test_harness = initialization_harness("", "", "", false);
        let outcome = configure_initialization(&test_harness, c, cxx, asm);
        assert_failure(
            &outcome,
            &format!("{language} flags must contain exactly one selected -mcpu=cortex-a7 option"),
        );
    }

    for (language, c_init, cxx_init, asm_init) in [
        ("C", "-mcpu=cortex-a9", "", ""),
        ("CXX", "", "-mcpu=cortex-a9", ""),
        ("ASM", "", "", "-mcpu=cortex-a9"),
    ] {
        let test_harness = initialization_harness(c_init, cxx_init, asm_init, false);
        let outcome = configure_initialization(&test_harness, "-O0", "-O1", "-O2");
        assert_failure(
            &outcome,
            &format!("{language} flags must contain exactly one selected -mcpu=cortex-a7 option"),
        );
    }

    for (language, c, cxx, asm) in [
        ("C", "-O0 -mfloat-abi=soft", "-O1", "-O2"),
        ("CXX", "-O0", "-O1 -mfloat-abi=soft", "-O2"),
        ("ASM", "-O0", "-O1", "-O2 -mfloat-abi=soft"),
    ] {
        let test_harness = initialization_harness(
            "-mfloat-abi=hard",
            "-mfloat-abi=hard",
            "-mfloat-abi=hard",
            false,
        );
        let outcome = configure_initialization(&test_harness, c, cxx, asm);
        assert_failure(
            &outcome,
            &format!("{language} flags must contain exactly one selected -mfloat-abi=hard option"),
        );
    }

    for (language, c_init, cxx_init, asm_init) in [
        ("C", "-mfloat-abi=soft", "", ""),
        ("CXX", "", "-mfloat-abi=soft", ""),
        ("ASM", "", "", "-mfloat-abi=soft"),
    ] {
        let test_harness = initialization_harness(c_init, cxx_init, asm_init, false);
        let outcome = configure_initialization(&test_harness, "-O0", "-O1", "-O2");
        assert_failure(
            &outcome,
            &format!("{language} flags must contain exactly one selected -mfloat-abi=hard option"),
        );
    }
}

#[test]
fn unsealed_response_files_are_rejected_before_compiler_detection() {
    use std::os::unix::fs::PermissionsExt;

    // Measure the actual process boundary, not a marker written after project().
    // A valid configuration must call every wrapper; rejected caller inputs
    // must call none, even for compiler-ID discovery.
    for opaque in [false, true] {
        let test_harness = initialization_harness("", "", "", false);
        let invocations = test_harness.root.join("compiler-invocations");
        let mut wrappers = Vec::new();
        for (language, compiler) in [("C", "clang"), ("CXX", "clang++"), ("ASM", "clang")] {
            let wrapper = test_harness
                .root
                .join(format!("record-{language}-{compiler}"));
            let quoted_marker = format!(
                "'{}'",
                invocations.display().to_string().replace('\'', "'\\''")
            );
            fs::write(
                &wrapper,
                format!("#!/bin/sh\nprintf '%s\\n' '{language}' >> {quoted_marker}\nexec {compiler} \"$@\"\n"),
            )
            .unwrap();
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
            wrappers.push((format!("CMAKE_{language}_COMPILER"), wrapper));
        }
        let wrapper_values: Vec<_> = wrappers
            .iter()
            .map(|(name, path)| (name.clone(), path.display().to_string()))
            .collect();
        let cache_variables: Vec<_> = wrapper_values
            .iter()
            .map(|(name, path)| (name.as_str(), path.as_str()))
            .collect();
        let response = test_harness.root.join("unsealed-flags.rsp");
        fs::write(&response, "-mcpu=cortex-a9\n").unwrap();
        let selected = if opaque {
            format!("-O0 -mcpu=cortex-a7 @{}", response.display())
        } else {
            "-O0 -mcpu=cortex-a7".to_owned()
        };
        let outcome = configure_initialization_with_cache(
            &test_harness,
            &selected,
            "-O1",
            "-O2",
            &cache_variables,
        );
        if opaque {
            assert_failure(
                &outcome,
                "SDK compiler flags contain an opaque compiler argument",
            );
            assert!(
                !invocations.exists(),
                "opaque flags reached a compiler process"
            );
        } else {
            assert_success(&test_harness, &outcome);
            let actual = fs::read_to_string(&invocations).unwrap();
            for language in ["C", "CXX", "ASM"] {
                assert!(
                    actual.lines().any(|line| line == language),
                    "missing {language} compiler invocation"
                );
            }
        }
    }
}

#[test]
fn opaque_compiler_arguments_cannot_override_sealed_source_options() {
    for language in ["C", "CXX", "ASM"] {
        for escape in [
            "-Xclang -target-cpu -Xclang cortex-a9",
            "-mllvm -enable-no-infs-fp-math",
            "--config=unsealed.cfg",
        ] {
            let test_harness = initialization_harness("", "", "", false);
            let selected = format!("-O0 -mcpu=cortex-a7 {escape}");
            let (c, cxx, asm) = match language {
                "C" => (selected.as_str(), "-O1", "-O2"),
                "CXX" => ("-O0", selected.as_str(), "-O2"),
                "ASM" => ("-O0", "-O1", selected.as_str()),
                _ => unreachable!(),
            };
            let outcome = configure_initialization(&test_harness, c, cxx, asm);
            assert_failure(
                &outcome,
                "SDK compiler flags contain an opaque compiler argument",
            );
        }

        let test_harness = initialization_harness("", "", "", false);
        let response = test_harness.root.join("unsealed-flags.rsp");
        fs::write(&response, "-mcpu=cortex-a9\n").unwrap();
        let selected = format!("-O0 -mcpu=cortex-a7 @{}", response.display());
        let (c, cxx, asm) = match language {
            "C" => (selected.as_str(), "-O1", "-O2"),
            "CXX" => ("-O0", selected.as_str(), "-O2"),
            "ASM" => ("-O0", "-O1", selected.as_str()),
            _ => unreachable!(),
        };
        let outcome = configure_initialization(&test_harness, c, cxx, asm);
        assert_failure(
            &outcome,
            "SDK compiler flags contain an opaque compiler argument",
        );

        let (c_init, cxx_init, asm_init) = match language {
            "C" => ("-Xclang -target-cpu -Xclang cortex-a9", "", ""),
            "CXX" => ("", "-Xclang -target-cpu -Xclang cortex-a9", ""),
            "ASM" => ("", "", "-Xclang -target-cpu -Xclang cortex-a9"),
            _ => unreachable!(),
        };
        let test_harness = initialization_harness(c_init, cxx_init, asm_init, false);
        let outcome = configure_initialization(&test_harness, "-O0", "-O1", "-O2");
        assert_failure(
            &outcome,
            "SDK compiler flags contain an opaque compiler argument",
        );

        let test_harness = initialization_harness("", "", "", false);
        let variable = format!("CMAKE_{language}_FLAGS_COVERAGE");
        let outcome = configure_initialization_with_cache(
            &test_harness,
            "-O0",
            "-O1",
            "-O2",
            &[
                ("CMAKE_TRY_COMPILE_CONFIGURATION", "Coverage"),
                (variable.as_str(), "-Xclang -target-cpu -Xclang cortex-a9"),
            ],
        );
        assert_failure(
            &outcome,
            "SDK compiler flags contain an opaque compiler argument",
        );
    }
}

#[test]
fn language_rule_overrides_reuse_matching_environment_options_and_reject_replacement() {
    let test_harness = initialization_harness(
        "-mfloat-abi=hard -g",
        "-mfloat-abi=hard -g1",
        "-mfloat-abi=hard -g",
        false,
    );
    let outcome = configure_initialization(
        &test_harness,
        "-O0 -mcpu=cortex-a7",
        "-O1 -mcpu=cortex-a7",
        "-O2 -mcpu=cortex-a7",
    );
    assert_success(&test_harness, &outcome);
    let flags = String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap());
    for language in ["C", "CXX", "ASM"] {
        let line = flags
            .lines()
            .find(|line| line.starts_with(&format!("{language}=")))
            .unwrap();
        assert_eq!(
            line.matches("-mcpu=cortex-a7").count(),
            1,
            "{language}: {line}"
        );
    }

    let conflicting = initialization_harness("", "", "", true);
    let outcome = configure_initialization(&conflicting, "-O0", "-O1", "-O2");
    assert_failure(&outcome, "CMAKE_USER_MAKE_RULES_OVERRIDE_C is already set");
}

#[test]
fn production_arm_toolchain_reuses_matching_environment_float_abi_once() {
    let test_harness = production_initialization_harness();
    let outcome = configure_production_initialization(
        &test_harness,
        "-O0 -mfloat-abi=hard -g",
        "-O1 -mfloat-abi=hard -g",
        "-O2 -mfloat-abi=hard -g",
    );
    assert_success(&test_harness, &outcome);
    let abi = String::from_utf8_lossy(outcome.initialization_abi.as_ref().unwrap());
    for detected in [
        "CMAKE_SIZEOF_VOID_P=4",
        "CMAKE_C_COMPILER_ID=Clang",
        "CMAKE_CXX_COMPILER_ID=Clang",
        "CMAKE_ASM_COMPILER_ID=Clang",
        "CMAKE_C_COMPILER_TARGET=arm-unknown-aros",
        "CMAKE_CXX_COMPILER_TARGET=arm-unknown-aros",
        "CMAKE_ASM_COMPILER_TARGET=arm-unknown-aros",
    ] {
        assert!(
            abi.contains(detected),
            "missing real compiler detection {detected}:\n{abi}"
        );
    }
    let flags = String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap());
    for (language, unrelated) in [("C", "-O0"), ("CXX", "-O1"), ("ASM", "-O2")] {
        let line = flags
            .lines()
            .find(|line| line.starts_with(&format!("{language}=")))
            .unwrap();
        assert_eq!(line.matches("-mfloat-abi=hard").count(), 1, "{line}");
        assert_eq!(line.matches("-mcpu=cortex-a7").count(), 1, "{line}");
        assert_eq!(line.matches("-mfpu=neon-vfpv4").count(), 1, "{line}");
        assert!(
            line.contains(unrelated),
            "{language} lost ENV option: {line}"
        );
        assert_eq!(
            line.matches(unrelated).count(),
            1,
            "{language} duplicated ENV option: {line}"
        );
        assert!(
            line.contains("-g"),
            "{language} lost ENV debug option: {line}"
        );
    }
    let commands = String::from_utf8_lossy(outcome.compile_commands.as_ref().unwrap());
    for source in ["probe.c", "probe.cpp"] {
        assert!(
            commands.contains(source),
            "missing production compile command for {source}"
        );
    }
    let build = Command::new("cmake")
        .arg("--build")
        .arg(&test_harness.build)
        .arg("--target")
        .arg("sdk_production_initialization_probe")
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "production ARM compiler probe build failed:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    let reconfigured = configure_production_initialization(
        &test_harness,
        "-O0 -mfloat-abi=hard -g",
        "-O1 -mfloat-abi=hard -g",
        "-O2 -mfloat-abi=hard -g",
    );
    assert_success(&test_harness, &reconfigured);
    let flags = String::from_utf8_lossy(reconfigured.compiler_flags.as_ref().unwrap());
    for language in ["C", "CXX", "ASM"] {
        let line = flags
            .lines()
            .find(|line| line.starts_with(&format!("{language}=")))
            .unwrap();
        assert_eq!(line.matches("-mfloat-abi=hard").count(), 1, "{line}");
        assert_eq!(line.matches("-mcpu=cortex-a7").count(), 1, "{line}");
        assert_eq!(line.matches("-mfpu=neon-vfpv4").count(), 1, "{line}");
    }

    let default_environment = production_initialization_harness();
    let outcome = configure_production_initialization(&default_environment, "", "", "");
    assert_success(&default_environment, &outcome);
    let flags = String::from_utf8_lossy(outcome.compiler_flags.as_ref().unwrap());
    for language in ["C", "CXX", "ASM"] {
        let line = flags
            .lines()
            .find(|line| line.starts_with(&format!("{language}=")))
            .unwrap();
        assert_eq!(line.matches("-mfloat-abi=hard").count(), 1, "{line}");
    }

    let conflict = production_initialization_harness();
    let outcome = configure_production_initialization(&conflict, "-mfloat-abi=soft", "", "");
    assert_failure(
        &outcome,
        "C flags must contain exactly one selected -mfloat-abi=hard option",
    );
}

#[test]
fn generic_sdk_registration_fails_before_project_without_a_release_root() {
    let test_harness = initialization_harness("", "", "", false);
    let cmake_lists = test_harness.root.join("CMakeLists.txt");
    let script = fs::read_to_string(&cmake_lists).unwrap();
    let without_root = script.replace(
        r#"set(AROS_CROSS_TOOLCHAIN_ROOT "${ROOT}/mock-release-toolchain")"#,
        r#"set(AROS_CROSS_TOOLCHAIN_ROOT "")"#,
    );
    fs::write(cmake_lists, without_root).unwrap();
    let outcome = configure_initialization(&test_harness, "-O0", "-O1", "-O2");
    assert_failure(
        &outcome,
        "generic SDK consumer requires AROS_CROSS_TOOLCHAIN_ROOT before compiler detection",
    );
}
