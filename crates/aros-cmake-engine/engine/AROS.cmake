# AROS-NX Core CMake Module (v0.1.0)
# Modern Multi-Platform Build System for AROS

include(CMakeParseArguments)
include("${CMAKE_CURRENT_LIST_DIR}/Executable.cmake")
include("${CMAKE_CURRENT_LIST_DIR}/GenmoduleManifest.cmake")
include("${CMAKE_CURRENT_LIST_DIR}/GenmoduleTargets.cmake")
include("${CMAKE_CURRENT_LIST_DIR}/GenmoduleHeaders.cmake")
include("${CMAKE_CURRENT_LIST_DIR}/PythonGenerators.cmake")
include("${CMAKE_CURRENT_LIST_DIR}/TransitiveHeaderBindings.cmake")
include("${CMAKE_CURRENT_LIST_DIR}/SourceInventory.cmake")
include("${CMAKE_CURRENT_LIST_DIR}/LibdefsAudit.cmake")
include("${CMAKE_CURRENT_LIST_DIR}/DefaultBuildClosure.cmake")
# aros_add_program calls aros_standalone_link_wanted, so the module that
# defines it belongs here rather than only in the top-level CMakeLists: a
# fixture that includes AROS.cmake on its own got "Unknown CMake command"
# (OPEN-POINTS 45). include_guard makes the top-level include a no-op.
include("${CMAKE_CURRENT_LIST_DIR}/StandaloneLink.cmake")
# The arch/ subdirectories this configuration may build from. Directory names
# there are <cpu>-<platform> with "all" as a wildcard, plus "native" as a
# pseudo-platform shared by every non-hosted target.
#
# A CPU also admits the ones it is backward compatible with, because AROS
# relies on that: compiler/include/asm/io.h forwards to <asm/i386/io.h> for
# __x86_64__ as well, and arch/x86_64-pc/boot lists kernel-pc-i386-serial and
# kernel-pc-i386-parallel among its BSP modules. Ordered least to most
# specific, since header staging copies in this order and the last write wins.
#
# The same set is used for header staging, the per-target gate, package
# declarations and genmodule's --arch-dirs, so they cannot drift apart.
set(AROS_ARCH_COMPATIBLE_CPUS "${AROS_TARGET_CPU}")
if(AROS_TARGET_CPU STREQUAL "x86_64")
    list(PREPEND AROS_ARCH_COMPATIBLE_CPUS "i386")
elseif(AROS_TARGET_CPU STREQUAL "aarch64")
    list(PREPEND AROS_ARCH_COMPATIBLE_CPUS "arm")
elseif(AROS_TARGET_CPU STREQUAL "riscv64")
    list(PREPEND AROS_ARCH_COMPATIBLE_CPUS "riscv")
endif()

set(AROS_ARCH_SOURCE_DIRS "all-native")
foreach(_cpu IN LISTS AROS_ARCH_COMPATIBLE_CPUS)
    list(APPEND AROS_ARCH_SOURCE_DIRS
        "${_cpu}-all"
        "${_cpu}-native"
        "all-${AROS_TARGET_PLATFORM}"
        "${_cpu}-${AROS_TARGET_PLATFORM}")
endforeach()
list(REMOVE_DUPLICATES AROS_ARCH_SOURCE_DIRS)

# Packages need the narrower set. Sources and headers may come from a
# compatible CPU, but a package belongs to exactly one architecture:
# arch/i386-pc and arch/x86_64-pc both declare $(AROSARCHDIR)/aros-bsp.pkg,
# and only one of them may write it.
set(AROS_ARCH_PACKAGE_DIRS
    "all-native"
    "${AROS_TARGET_CPU}-all"
    "${AROS_TARGET_CPU}-native"
    "all-${AROS_TARGET_PLATFORM}"
    "${AROS_TARGET_CPU}-${AROS_TARGET_PLATFORM}")
list(REMOVE_DUPLICATES AROS_ARCH_PACKAGE_DIRS)

include("${CMAKE_SOURCE_DIR}/cmake/BootstrapSDK.cmake")

# Target output directories.  These mirror config/make.cfg.in:97-124 and the
# effective defaults written by genmodule; keeping the complete module layout
# here prevents individual builders from inventing their own approximation of
# the system tree.  genmodule deliberately spells DataTypes with a capital T
# (tools/genmodule/config.c:294-297), despite make.cfg.in's Datatypes spelling.
set(AROS_BUILD_DIR "${CMAKE_BINARY_DIR}")
set(AROS_SYS_DIR "${AROS_BUILD_DIR}/SYS")
set(AROS_BOOT_DIR "${AROS_SYS_DIR}/boot")
set(AROS_BOOT_ARCH_DIR "${AROS_BOOT_DIR}/${AROS_TARGET_PLATFORM}")
set(AROS_C_DIR "${AROS_SYS_DIR}/C")
set(AROS_CLASSES_DIR "${AROS_SYS_DIR}/Classes")
set(AROS_DATATYPES_DIR "${AROS_CLASSES_DIR}/DataTypes")
set(AROS_GADGETS_DIR "${AROS_CLASSES_DIR}/Gadgets")
set(AROS_CLASSIMAGES_DIR "${AROS_CLASSES_DIR}/Images")
set(AROS_ZUNE_CLASSES_DIR "${AROS_CLASSES_DIR}/Zune")
set(AROS_USB_CLASSES_DIR "${AROS_CLASSES_DIR}/USB")
set(AROS_BLUETOOTH_CLASSES_DIR "${AROS_CLASSES_DIR}/Bluetooth")
set(AROS_DEVS_DIR "${AROS_SYS_DIR}/Devs")
set(AROS_RESOURCES_DIR "${AROS_DEVS_DIR}")
set(AROS_DRIVERS_DIR "${AROS_DEVS_DIR}/Drivers")
set(AROS_PRINTERS_DIR "${AROS_DEVS_DIR}/Printers")
set(AROS_FS_DIR "${AROS_SYS_DIR}/L")
set(AROS_LIBS_DIR "${AROS_SYS_DIR}/Libs")
set(AROS_DEVELOPER_DIR "${AROS_SYS_DIR}/Developer")
set(AROS_DEVELOPER_INCLUDE_DIR "${AROS_DEVELOPER_DIR}/include")
set(AROS_DEVELOPER_LIB_DIR "${AROS_DEVELOPER_DIR}/lib")
set(AROS_DEVELOPER_SDK_DIR "${AROS_DEVELOPER_DIR}/SDK")
set(AROS_DEVELOPER_FD_DIR "${AROS_DEVELOPER_SDK_DIR}/fd")

# genmodule keeps module-private headers outside the shared SDK tree.  This is
# an output-root contract for later generated-file rules as well, so establish
# it in directory scope rather than only inside the bootstrap helper.
set(AROS_GEN_DIR "${CMAKE_BINARY_DIR}/gen")

# Release compilers intentionally have no producer build directory embedded as
# DEFAULT_SYSROOT.  Match config/features.in's external-toolchain contract:
# every consumer supplies its own Developer tree explicitly.  This path also
# provides cxx-startup.o to the locked direct-ld.lld C++ partial link.
set(AROS_TARGET_SYSROOT "${AROS_DEVELOPER_DIR}")

file(MAKE_DIRECTORY
    "${AROS_BOOT_ARCH_DIR}"
    "${AROS_C_DIR}"
    "${AROS_DATATYPES_DIR}"
    "${AROS_GADGETS_DIR}"
    "${AROS_CLASSIMAGES_DIR}"
    "${AROS_ZUNE_CLASSES_DIR}"
    "${AROS_USB_CLASSES_DIR}"
    "${AROS_BLUETOOTH_CLASSES_DIR}"
    "${AROS_RESOURCES_DIR}"
    "${AROS_DRIVERS_DIR}"
    "${AROS_PRINTERS_DIR}"
    "${AROS_FS_DIR}"
    "${AROS_LIBS_DIR}"
    "${AROS_DEVELOPER_INCLUDE_DIR}"
    "${AROS_DEVELOPER_LIB_DIR}"
    "${AROS_DEVELOPER_FD_DIR}")

# Bootstrap SDK Includes
aros_bootstrap_sdk_includes()

# AROS' normal clang++ driver adds this object to C++ links.  Locked CMake
# consumers deliberately use the prefix-owned ld.lld directly, so the
# transpiled graph must publish and name that equivalent input explicitly.
# The historic make graph produces it under AROS_LIB, but the CMake graph has
# no concrete owner for compiler/startup's raw object rules.  Its source is
# deliberately located relative to this module so the focused CMake fixture
# exercises the real source rather than maintaining a copy.
if(AROS_CROSS_TOOLCHAIN_ROOT)
    set(_aros_cxx_startup_source
        "${CMAKE_CURRENT_LIST_DIR}/../compiler/startup/cxx-startup.c")
    if(NOT EXISTS "${_aros_cxx_startup_source}")
        message(FATAL_ERROR
            "Locked AROS C++ consumer cannot find cxx-startup source: "
            "${_aros_cxx_startup_source}")
    endif()
    set(_aros_cxx_startup_output "${AROS_DEVELOPER_LIB_DIR}/cxx-startup.o")
    add_library(aros-cxx-startup-objects OBJECT EXCLUDE_FROM_ALL
        "${_aros_cxx_startup_source}")
    set_target_properties(aros-cxx-startup-objects PROPERTIES
        POSITION_INDEPENDENT_CODE OFF)
    add_custom_command(
        OUTPUT "${_aros_cxx_startup_output}"
        COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "$<TARGET_OBJECTS:aros-cxx-startup-objects>"
            "${_aros_cxx_startup_output}"
        DEPENDS "$<TARGET_OBJECTS:aros-cxx-startup-objects>"
        COMMENT "Publishing cxx-startup.o for locked AROS C++ partial links"
        COMMAND_EXPAND_LISTS
        VERBATIM)
    add_custom_target(aros-cxx-startup DEPENDS "${_aros_cxx_startup_output}")
    add_dependencies(aros-cxx-startup aros-cxx-startup-objects)
    set(AROS_CXX_STARTUP_TARGET "aros-cxx-startup")
endif()

# AppleClang does not ship the ELF utilities needed by target archives, AHI
# and GRUB. Direct preset builds may discover a complete LLVM installation;
# locked builds never enter this path because every tool was fixed by the
# release toolchain before project(). Prefer PATH everywhere, with Homebrew's
# queryable LLVM prefix as the macOS fallback (no host-specific absolute path).
if(NOT AROS_CROSS_TOOLCHAIN_ROOT AND
   CMAKE_C_COMPILER_ID MATCHES "Clang")
    find_program(_aros_development_objcopy NAMES llvm-objcopy NO_CACHE)
    if(_aros_development_objcopy)
        cmake_path(GET _aros_development_objcopy PARENT_PATH
            _aros_development_llvm_bin)
    elseif(CMAKE_HOST_SYSTEM_NAME STREQUAL "Darwin")
        find_program(_aros_homebrew NAMES brew NO_CACHE)
        if(_aros_homebrew)
            execute_process(
                COMMAND "${_aros_homebrew}" --prefix llvm
                RESULT_VARIABLE _aros_brew_llvm_result
                OUTPUT_VARIABLE _aros_brew_llvm_prefix
                ERROR_QUIET
                OUTPUT_STRIP_TRAILING_WHITESPACE)
            if(_aros_brew_llvm_result EQUAL 0 AND
               IS_DIRECTORY "${_aros_brew_llvm_prefix}/bin" AND
               EXISTS "${_aros_brew_llvm_prefix}/bin/llvm-objcopy")
                set(_aros_development_llvm_bin
                    "${_aros_brew_llvm_prefix}/bin")
            endif()
        endif()
    endif()
    if(_aros_development_llvm_bin)
        foreach(_aros_tool_pair IN ITEMS
                "CMAKE_AR|llvm-ar"
                "CMAKE_RANLIB|llvm-ranlib"
                "CMAKE_NM|llvm-nm"
                "CMAKE_STRIP|llvm-strip"
                "CMAKE_OBJCOPY|llvm-objcopy"
                "CMAKE_OBJDUMP|llvm-objdump")
            string(REPLACE "|" ";" _aros_tool_fields "${_aros_tool_pair}")
            list(GET _aros_tool_fields 0 _aros_cmake_tool)
            list(GET _aros_tool_fields 1 _aros_llvm_tool)
            set(_aros_tool_path
                "${_aros_development_llvm_bin}/${_aros_llvm_tool}")
            if(EXISTS "${_aros_tool_path}" AND
               NOT IS_DIRECTORY "${_aros_tool_path}")
                set(${_aros_cmake_tool} "${_aros_tool_path}"
                    CACHE FILEPATH "LLVM target utility" FORCE)
                # The cache set alone was not enough. CMake's compiler
                # detection leaves CMAKE_AR and friends as ordinary variables in
                # this scope, and an ordinary variable shadows the cache entry
                # of the same name. So the cache read llvm-ar while every
                # generated rule kept using /usr/bin/ar, and on an ELF object
                # the macOS ar answers
                #
                #     ranlib: warning: archive member '...obj' not a mach-o file
                #
                # and writes a 96-byte archive with no members. All 119 static
                # libraries in this build were empty, which is why strlen,
                # memset and the rest of the C runtime read as undefined across
                # hundreds of modules while being defined in the tree.
                set(${_aros_cmake_tool} "${_aros_tool_path}")
            endif()
        endforeach()
        message(STATUS
            "AROS-NX direct build uses LLVM target utilities from "
            "${_aros_development_llvm_bin}")
    endif()
endif()

# Restated because CMake assembles CMAKE_<LANG>_ARCHIVE_* while configuring the
# language, which happens in project() before this file runs. Without this the
# rules keep whichever ar was current then, and the override above never
# reaches them.
foreach(_aros_archive_lang C CXX ASM)
    set(CMAKE_${_aros_archive_lang}_ARCHIVE_CREATE
        "<CMAKE_AR> qc <TARGET> <LINK_FLAGS> <OBJECTS>")
    set(CMAKE_${_aros_archive_lang}_ARCHIVE_APPEND
        "<CMAKE_AR> q <TARGET> <OBJECTS>")
    set(CMAKE_${_aros_archive_lang}_ARCHIVE_FINISH
        "<CMAKE_RANLIB> <TARGET>")
endforeach()


# Canonical AROS ELF linker rules. A locked release toolchain defines the
# linker before project() and must stay entirely inside its immutable prefix.
# The direct-CMake development path may discover an ld.lld from PATH.
if(AROS_CROSS_TOOLCHAIN_ROOT)
    if(NOT AROS_LLD_BIN STREQUAL
            "${AROS_CROSS_TOOLCHAIN_ROOT}/bin/ld.lld")
        message(FATAL_ERROR
            "Locked AROS build must use its prefix-owned ld.lld")
    endif()
else()
    find_program(AROS_LLD_BIN NAMES ld.lld)
endif()
if(AROS_LLD_BIN)
    # CMake 3.27+ may ask a detected LLD to write link dependencies by adding
    # the compiler-driver spelling `-Xlinker --dependency-file=...`. The rules
    # below deliberately invoke aros-collect and ld.lld directly, so forwarding
    # a driver-only option makes a Linux cold build fail at its first link.
    # Object and archive dependencies are already explicit in the generated
    # Ninja graph; disable only CMake's linker-assisted supplement.
    set(CMAKE_LINK_DEPENDS_USE_LINKER FALSE)

    # Every link below runs through aros-collect rather than calling ld.lld
    # directly, and that is not a wrapper for convenience.
    #
    # For an AROS target the linker the compiler spec names is not ld but
    # collect-aros (config/elf-specs.in `*linker:` -> scripts/aros-ld.in:5), and
    # TARGET_LD is the same wrapper (configure:18209; use_ld_wrapper is
    # unconditionally yes). It links twice: `ld -r` over the inputs, then
    # `ld -r -T <generated script>` over that result, and the script is what
    # lays each .aros.set.* section out as the array the code reads
    # (tools/collect-aros/gensets.c:69). A plain `-r`, which is what this rule
    # used to be, is exactly the mode collect-aros stops early in
    # (collect-aros.c:184), so every symbol set in this build was the empty weak
    # `{0, 0}` of DEFINESET and no INITLIB, OPENLIB, LIBS or CTORS function had
    # ever run. OPEN-POINTS point 32 has the measurements.
    # Direct module consumers may provide an exact binary. Otherwise use the
    # installed suite, independent of the source checkout's location.
    if(NOT AROS_COLLECT_BIN)
        find_program(AROS_COLLECT_BIN NAMES aros-collect)
    endif()
    aros_path_is_executable("${AROS_COLLECT_BIN}" _aros_collect_executable)
    if(NOT _aros_collect_executable)
        message(FATAL_ERROR
            "AROS-NX requires executable aros-collect at ${AROS_COLLECT_BIN}. "
            "Install aros-tools, or set AROS_COLLECT_BIN explicitly. Without "
            "it every symbol set links empty.")
    endif()
    set(_aros_link "\"${AROS_COLLECT_BIN}\" --ld \"${AROS_LLD_BIN}\" --")

    set(CMAKE_C_LINK_EXECUTABLE
        "${_aros_link} -r --sysroot=\"${AROS_TARGET_SYSROOT}\" <LINK_FLAGS> <OBJECTS> -o <TARGET> <LINK_LIBRARIES>")
    set(CMAKE_C_CREATE_SHARED_MODULE
        "${_aros_link} -r --sysroot=\"${AROS_TARGET_SYSROOT}\" <LINK_FLAGS> <OBJECTS> -o <TARGET> <LINK_LIBRARIES>")
    if(AROS_CROSS_TOOLCHAIN_ROOT)
        if(NOT AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES)
            message(FATAL_ERROR
                "Locked AROS C++ links require the validated prefix runtime list from "
                "cmake/toolchains/AROS.cmake")
        endif()
        list(LENGTH AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES
            _aros_cxx_runtime_library_count)
        if(NOT _aros_cxx_runtime_library_count EQUAL 4)
            message(FATAL_ERROR
                "Locked AROS C++ links require exactly libc++, libc++abi, libunwind "
                "and compiler-rt from the release prefix")
        endif()
        set(_aros_cxx_runtime_link_args "")
        foreach(_aros_cxx_runtime_library
                IN LISTS AROS_CROSS_TOOLCHAIN_CXX_RUNTIME_LIBRARIES)
            if(NOT IS_ABSOLUTE "${_aros_cxx_runtime_library}")
                message(FATAL_ERROR
                    "Locked AROS C++ runtime archive is not an absolute prefix path: "
                    "${_aros_cxx_runtime_library}")
            endif()
            string(APPEND _aros_cxx_runtime_link_args
                " \"${_aros_cxx_runtime_library}\"")
        endforeach()

        # `alwayscxxlink=yes` is a real upstream ABI contract.  Link locked
        # C++ modules through the prefix-owned lld directly instead of the
        # AROS clang++ driver: that driver delegates to collect-aros, whose
        # build-local configuration is intentionally absent from relocatable
        # release prefixes.  Name cxx-startup.o and the four validated runtime
        # archives explicitly, so this partial link has neither driver defaults
        # nor host-PATH resolution.
        set(_aros_cxx_partial_link
            "${_aros_link} -r --sysroot=\"${AROS_TARGET_SYSROOT}\" <LINK_FLAGS> \"${_aros_cxx_startup_output}\" <OBJECTS> -o <TARGET> <LINK_LIBRARIES> --start-group${_aros_cxx_runtime_link_args} --end-group")
        set(CMAKE_CXX_LINK_EXECUTABLE "${_aros_cxx_partial_link}")
        set(CMAKE_CXX_CREATE_SHARED_MODULE "${_aros_cxx_partial_link}")
    else()
        set(CMAKE_CXX_LINK_EXECUTABLE
            "${_aros_link} -r <LINK_FLAGS> <OBJECTS> -o <TARGET> <LINK_LIBRARIES>")
        set(CMAKE_CXX_CREATE_SHARED_MODULE
            "${_aros_link} -r <LINK_FLAGS> <OBJECTS> -o <TARGET> <LINK_LIBRARIES>")
    endif()
endif()

# Target architecture compilation options
if(AROS_CROSS_TOOLCHAIN_ROOT)
    if(NOT AROS_TARGET_TRIPLE)
        message(FATAL_ERROR "Locked AROS build lacks AROS_TARGET_TRIPLE")
    endif()
    add_compile_options(
        "--target=${AROS_TARGET_TRIPLE}"
        "--sysroot=${AROS_TARGET_SYSROOT}")
elseif(AROS_TARGET_CPU STREQUAL "x86_64")
    add_compile_options(-target x86_64-unknown-elf)
elseif(AROS_TARGET_CPU STREQUAL "aarch64")
    add_compile_options(-target aarch64-unknown-elf)
elseif(AROS_TARGET_CPU STREQUAL "arm")
    # The legacy build takes the ISA flags from Autoconf (ISA_ARM_FLAGS), which
    # has no counterpart here, so clang fell back to its arm-none-eabi default.
    # That default predates ARMv7 and rejects the data barriers and other
    # instructions arch/arm-native uses.
    #
    # Fixed at ARMv7 (Pi 2 and later). The tree's raspi BSP is bcm2835, i.e.
    # Pi 1, whose ARMv6 core cannot run this code; covering it would need
    # per-BSP flags and three source sites reworked. Anything from Pi 3 on runs
    # the rpi-aarch64 target instead, so no supported board is lost.
    add_compile_options(-target arm-none-eabi -mcpu=cortex-a7 -mfpu=neon-vfpv4
        -mfloat-abi=hard)
endif()

add_compile_definitions(
    __AROS__=1
    __AROS_VERSION__=1
    # config/specs.in injects these into every target-compiler invocation.
    # The bare LLVM driver does not read the installed AROS specs, so retain
    # the historic Amiga-compatibility branch selection explicitly. Network
    # sources such as lineread.c use this to choose bsdsocket's recv() over
    # an undeclared hosted POSIX read().
    AMIGA=1
    _AMIGA=1
)

# Build-date stamps. 52 mmakefiles put the current date into a define via
# $(shell date '+<fmt>'); the transpiler maps those two formats onto these
# variables. Evaluated once per configure rather than per compile, so a build
# is at least consistent within itself.
string(TIMESTAMP AROS_BUILD_DATE_DMY "%d.%m.%Y")
string(TIMESTAMP AROS_BUILD_DATE_ISO "%Y-%m-%d")

add_compile_options(
    -ffreestanding
    -fno-builtin
    # Some Linux distributions configure Clang with stack-protector-strong as
    # a driver default.  A freestanding target has no host runtime providing
    # __stack_chk_fail, and inheriting the host default also makes identical
    # presets produce different objects.  State the bare-metal contract.
    -fno-stack-protector
    -fno-strict-aliasing
    -fno-common
    -Wall
    -Wextra
    -Wno-unused-parameter
)

# The generated trees come first. The target compiler's legacy specs search
# the POSIX and standard-C namespaces before the shared SDK root. LLVM is a
# bare driver here and has no installed AROS specs, so repeat that order for
# every target rather than only the handful of genmodule callers which used to
# carry it locally. Otherwise <errno.h> and <stdlib.h> resolve to the smaller
# C99 namespace and POSIX declarations such as ESRCH, EMFILE and random() are
# silently lost. Keep these as ordinary includes: a later -isystem path would
# still lose to the shared SDK's -I path in the compiler's search order.
#
# The historic build has no -I into the source tree at all: compiler/include
# is staged into the SDK by %copy_includes, and genmodule then writes over what
# it supersedes, so the generated header is the one that gets found. Keeping
# compiler/include ahead of the SDK inverted that, and the hand-written
# clib/input_protos.h -- which predates genmodule and still declares
# PeekQualifier through AROS_LP0 -- shadowed the generated one.
include_directories(
    "${CMAKE_BINARY_DIR}/GENINCDIR"
    "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc"
    "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc"
    "${CMAKE_BINARY_DIR}/SDK/include"
    "${CMAKE_SOURCE_DIR}/compiler/include"
    "${CMAKE_SOURCE_DIR}/arch/all-native/include"
)

# Closed local-source runners for the audited `%build_with_configure`
# capabilities.  Kept in a separate module because the implementation also
# serves focused repair/no-op regression fixtures.
include("${CMAKE_CURRENT_LIST_DIR}/ConfigureBuild.cmake")

# Closed GRUB 2.12 host-tool lanes selected by the transpiler.  Like the
# configure helper, its implementation carries the complete audited runner
# contract and is also exercised by focused repair/no-op fixtures.
include("${CMAKE_CURRENT_LIST_DIR}/GrubBuild.cmake")

# =============================================================================
# SDK header staging
# =============================================================================
#
# Modules publish public headers via %copy_includes. The historic macro copies
# them into two include roots: the target SDK ($(AROS_INCLUDES)) and the host
# tool tree ($(GENINCDIR)). Headers referenced with a category prefix such as
# <oop/oop.h> or <hidd/hidd.h> can only be found this way, because the prefix
# is part of the #include and no search path supplies it.
#
# The transpiler turns each declaration into an aros_copy_includes() call; the
# glob is resolved here rather than in the transpiler, so adding a header needs
# no regeneration by hand.

set(AROS_SDK_INCLUDE_DIR "${CMAKE_BINARY_DIR}/SDK/include")
set(AROS_GENINC_DIR "${CMAKE_BINARY_DIR}/GENINCDIR")

# Counters so the configure output states what was staged.
set_property(GLOBAL PROPERTY AROS_STAGED_HEADERS 0)
set_property(GLOBAL PROPERTY AROS_STAGED_RULES_EMPTY 0)
set_property(GLOBAL PROPERTY AROS_STAGED_HEADER_BINDINGS "")
set(_AROS_DEFERRED_HEADER_REPORT
    "${CMAKE_BINARY_DIR}/generated_targets.deferred-header-staging.txt")
file(REMOVE "${_AROS_DEFERRED_HEADER_REPORT}")

# aros_copy_includes([NAME <mmake>] DEST <subdir> SOURCE <src-relative dir>
#                    PATTERNS <globs...> [EXCLUDES <literal names...>]
#                    [FLATTEN] [ALLOW_FOREIGN_ARCH])
#
# FLATTEN mirrors the macro's $(notdir ...) behaviour, which applies when the
# declaration passes dir=. Without it the listed relative path is preserved.

# Joins a %copy_includes destination and file name into the canonical spelling
# used by output paths and by genmodule's literal-include bindings.  In
# particular, MetaMake's conventional `path=.` must bind `<zlib.h>`, not the
# textually different `<./zlib.h>`.
function(_aros_staged_header_path out_var dest name)
    set(_path "${dest}/${name}")
    cmake_path(NORMAL_PATH _path)
    set(${out_var} "${_path}" PARENT_SCOPE)
endfunction()

# aros_record_load_set(NAME <set> MEMBERS <targets...>)
#
# Records that these targets are loaded together, so `ninja symbol-audit` can
# resolve their undefined symbols against each other instead of demanding that
# every module be self-contained.
#
# This is what the audit was missing. A kickstart member may legitimately call
# into another member: the members are relocated as one unit, so the reference
# to it is satisfied then. Checking each artefact alone reported those as gaps,
# which is why the remaining count was an upper bound rather than a defect
# count. rom/intuition declares no uselibs and the reference links no C library
# into it either, so its calls into the rest of the kickstart are correct.
#
# KIND separates the two loaders, and they differ in exactly one place.
# bootstrap/elfloader.c:157 resolves an undefined `SysBase` in a kickstart
# member to the default base and fails on anything else;
# rom/dos/internalloadseg_elf.c:509 fails on any undefined symbol at all, with
# no exception. So a kickstart member may leave SysBase open and a separately
# loaded module may not.
#
# Only targets that produce a file are recorded; a transpiler meta-target has
# no TARGET_FILE.
function(aros_record_load_set)
    set(oneValueArgs NAME KIND)
    set(multiValueArgs MEMBERS)
    cmake_parse_arguments(LS "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})
    if(NOT LS_NAME OR NOT LS_MEMBERS)
        return()
    endif()
    set(_paths "")
    foreach(_member IN LISTS LS_MEMBERS)
        if(TARGET "${_member}")
            get_target_property(_type "${_member}" TYPE)
            if(_type STREQUAL "EXECUTABLE" OR _type STREQUAL "STATIC_LIBRARY"
               OR _type STREQUAL "SHARED_LIBRARY"
               OR _type STREQUAL "MODULE_LIBRARY")
                list(APPEND _paths "$<TARGET_FILE:${_member}>")
            endif()
        endif()
    endforeach()
    if(NOT _paths)
        return()
    endif()
    get_property(_sets GLOBAL PROPERTY AROS_LOAD_SETS)
    string(REPLACE ";" "\t" _joined "${_paths}")
    list(APPEND _sets "${LS_KIND}\t${LS_NAME}\t${_joined}")
    set_property(GLOBAL PROPERTY AROS_LOAD_SETS "${_sets}")
endfunction()

# aros_arch_path_matches(<out-var> <path>)
#
# Whether a path under arch/ belongs to this configuration. A path outside
# arch/ is architecture-neutral and always matches.
#
# Used by everything that consumes a source location: header staging, the
# per-target gate and the package declarations. Without it the SDK ends up
# holding another architecture's headers, and which one wins depends on parse
# order: arch/m68k-amiga/include/asm/cpu.h replaced the x86_64 one, so every
# kernel source using rdmsri, wrcr or struct PML4E stopped compiling.
function(aros_arch_path_matches out_var path)
    # Works on the raw string: staging sources arrive relative to the source
    # tree, target directories absolute, and file(RELATIVE_PATH) rejects a
    # relative input.
    string(REPLACE "${CMAKE_SOURCE_DIR}/" "" _rel "${path}")
    if(NOT _rel MATCHES "^arch/([^/]+)")
        set(${out_var} TRUE PARENT_SCOPE)
        return()
    endif()
    if(CMAKE_MATCH_1 IN_LIST AROS_ARCH_SOURCE_DIRS)
        set(${out_var} TRUE PARENT_SCOPE)
    else()
        set(${out_var} FALSE PARENT_SCOPE)
    endif()
endfunction()

function(aros_copy_includes)
    set(options FLATTEN ALLOW_FOREIGN_ARCH)
    set(oneValueArgs NAME DEST SOURCE)
    set(multiValueArgs PATTERNS EXCLUDES)
    cmake_parse_arguments(CI "${options}" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(NOT CI_DEST OR NOT CI_SOURCE OR NOT CI_PATTERNS)
        return()
    endif()

    # Headers from another architecture must not reach the SDK: they land under
    # the same name as the right ones and whichever is copied last wins.
    aros_arch_path_matches(_arch_ok "${CI_SOURCE}")
    if(NOT _arch_ok AND NOT CI_ALLOW_FOREIGN_ARCH)
        get_property(_skipped GLOBAL PROPERTY AROS_STAGING_FOREIGN_ARCH)
        list(APPEND _skipped "${CI_SOURCE} -> ${CI_DEST}")
        set_property(GLOBAL PROPERTY AROS_STAGING_FOREIGN_ARCH "${_skipped}")
        return()
    endif()

    # Give every named declaration its real MetaMake identity before the graph
    # phase.  Several declarations may contribute to one target (Mesa stages
    # GL, KHR, EGL and Vulkan through mesa3d-includes-copy).
    if(CI_NAME AND NOT TARGET "${CI_NAME}")
        add_custom_target("${CI_NAME}")
    endif()

    # A source directory is normally relative to the source tree, but a module
    # may also stage headers out of a fetched port, which lives under the build
    # tree. Those arrive already absolute.
    if(IS_ABSOLUTE "${CI_SOURCE}")
        set(SRC_ABS "${CI_SOURCE}")
    else()
        set(SRC_ABS "${CMAKE_SOURCE_DIR}/${CI_SOURCE}")
    endif()
    if(NOT IS_DIRECTORY "${SRC_ABS}")
        # A cache-empty fetched port cannot be globbed at configure time.  An
        # explicit file list has known output names even though its port has
        # not been fetched yet.  Bind those outputs to their MetaMake owner
        # below, ordered after the fetch which owns the source path.
        set(_fetch_owner "")
        set(_fetch_owner_len -1)
        get_property(_fetch_targets GLOBAL PROPERTY AROS_FETCH_TARGETS)
        foreach(_fetch IN LISTS _fetch_targets)
            if(NOT TARGET "${_fetch}")
                continue()
            endif()
            get_property(_fetch_dest TARGET "${_fetch}" PROPERTY AROS_FETCH_DESTINATION)
            if(NOT _fetch_dest)
                continue()
            endif()
            string(LENGTH "${_fetch_dest}" _fetch_len)
            string(FIND "${SRC_ABS}" "${_fetch_dest}/" _fetch_prefix)
            if(("${SRC_ABS}" STREQUAL "${_fetch_dest}" OR _fetch_prefix EQUAL 0)
               AND _fetch_len GREATER _fetch_owner_len)
                set(_fetch_owner "${_fetch}")
                set(_fetch_owner_len "${_fetch_len}")
            endif()
        endforeach()

        set(_unsupported "")
        if(NOT CI_NAME)
            set(_unsupported "has no mmake owner")
        elseif(NOT _fetch_owner)
            set(_unsupported "has no matching %fetch destination owner")
        endif()
        if(_unsupported)
            set(_note "${CI_NAME}|${SRC_ABS}|${_unsupported}")
            set_property(GLOBAL APPEND PROPERTY
                AROS_DEFERRED_HEADER_UNSUPPORTED "${_note}")
            return()
        endif()

        foreach(_pattern IN LISTS CI_PATTERNS)
            if(_pattern MATCHES "[*?\\[]")
                set(_glob_args
                    NAME "${CI_NAME}"
                    SOURCE "${SRC_ABS}"
                    DEST "${CI_DEST}"
                    PATTERN "${_pattern}"
                    FETCH "${_fetch_owner}")
                if(CI_EXCLUDES)
                    list(APPEND _glob_args EXCLUDES ${CI_EXCLUDES})
                endif()
                if(CI_FLATTEN)
                    list(APPEND _glob_args FLATTEN)
                endif()
                _aros_materialize_deferred_header_glob(${_glob_args})
                continue()
            endif()
            if(CI_FLATTEN)
                get_filename_component(_name "${_pattern}" NAME)
            else()
                set(_name "${_pattern}")
            endif()
            # Keep explicit fetched files consistent with the existing-source
            # path below. A bounded `filter-out` may contain literal entries
            # alongside a wildcard; those literals must not be materialised
            # merely because the port source was absent at configure time.
            if(_name IN_LIST CI_EXCLUDES)
                continue()
            endif()
            _aros_staged_header_path(
                _header_path "${CI_DEST}" "${_name}")
            set(_source "${SRC_ABS}/${_pattern}")
            set(_sdk_output "${AROS_SDK_INCLUDE_DIR}/${_header_path}")
            set(_gen_output "${AROS_GENINC_DIR}/${_header_path}")
            string(SHA256 _copy_hash
                "${CI_NAME}|${_header_path}|${_source}")
            string(SUBSTRING "${_copy_hash}" 0 16 _copy_hash)
            set_property(GLOBAL APPEND PROPERTY
                AROS_DEFERRED_HEADER_HASHES "${_copy_hash}")
            set_property(GLOBAL PROPERTY
                "AROS_DEFERRED_HEADER_${_copy_hash}_SOURCE" "${_source}")
            set_property(GLOBAL PROPERTY
                "AROS_DEFERRED_HEADER_${_copy_hash}_SDK" "${_sdk_output}")
            set_property(GLOBAL PROPERTY
                "AROS_DEFERRED_HEADER_${_copy_hash}_GEN" "${_gen_output}")
            set_property(GLOBAL PROPERTY
                "AROS_DEFERRED_HEADER_${_copy_hash}_FETCH" "${_fetch_owner}")
            set_property(GLOBAL PROPERTY
                "AROS_DEFERRED_HEADER_${_copy_hash}_TARGET" "${CI_NAME}")
            set_property(GLOBAL PROPERTY
                "AROS_DEFERRED_HEADER_${_copy_hash}_LABEL"
                "${CI_NAME}|${_header_path}|${_source}")
            set_property(GLOBAL APPEND PROPERTY AROS_STAGED_HEADER_BINDINGS
                "${_header_path}|${CI_NAME}|${_copy_hash}|${_source}")
            # Unlike a glob, an explicit port header has a stable name at
            # configure time.  Materialising it now preserves the historic
            # owner edge (for example ports-includes -> bzlib.h) while the
            # custom command remains lazy until that owner is requested.
            _aros_materialize_deferred_header("${_copy_hash}")
        endforeach()
        return()
    endif()

    set(FOUND "")
    foreach(pat IN LISTS CI_PATTERNS)
        file(GLOB matches RELATIVE "${SRC_ABS}" "${SRC_ABS}/${pat}")
        list(APPEND FOUND ${matches})
    endforeach()
    if(NOT FOUND)
        get_property(n GLOBAL PROPERTY AROS_STAGED_RULES_EMPTY)
        math(EXPR n "${n} + 1")
        set_property(GLOBAL PROPERTY AROS_STAGED_RULES_EMPTY ${n})
        return()
    endif()
    list(REMOVE_DUPLICATES FOUND)

    if(CI_EXCLUDES)
        set(_filtered_found "")
        foreach(_found IN LISTS FOUND)
            if(CI_FLATTEN)
                get_filename_component(_published_name "${_found}" NAME)
            else()
                set(_published_name "${_found}")
            endif()
            if(NOT _published_name IN_LIST CI_EXCLUDES)
                list(APPEND _filtered_found "${_found}")
            endif()
        endforeach()
        set(FOUND "${_filtered_found}")
    endif()

    get_property(count GLOBAL PROPERTY AROS_STAGED_HEADERS)
    foreach(rel IN LISTS FOUND)
        if(CI_FLATTEN)
            get_filename_component(name "${rel}" NAME)
        else()
            set(name "${rel}")
        endif()
        _aros_staged_header_path(
            _header_path "${CI_DEST}" "${name}")

        # configure_file(COPYONLY) copies only on change and registers the
        # source as a configure dependency, so editing a public header
        # re-stages it on the next build.
        foreach(root "${AROS_SDK_INCLUDE_DIR}" "${AROS_GENINC_DIR}")
            configure_file("${SRC_ABS}/${rel}" "${root}/${_header_path}" COPYONLY)
        endforeach()
        if(CI_NAME)
            set_property(GLOBAL APPEND PROPERTY AROS_STAGED_HEADER_BINDINGS
                "${_header_path}|${CI_NAME}||${SRC_ABS}/${rel}")
        endif()
        math(EXPR count "${count} + 1")
    endforeach()
    set_property(GLOBAL PROPERTY AROS_STAGED_HEADERS ${count})
endfunction()

# aros_copy_dir_recursive(NAME <mmake-id> SOURCE <absolute-or-source-relative>
#                         DESTINATION <build-tree-directory>
#                         [DEPENDS <target>...])
#
# Materialises the safe subset of MetaMake's %copy_dir_recursive.  A fetched
# source tree does not exist while CMake configures, so the copy has one real
# stamp output and waits for the owning fetch completion stamp.  In-tree
# directories are tracked through their concrete files instead.  Copies retain
# MetaMake's overlay semantics, so sibling declarations may safely stage into
# nested destination directories.  The destination is deliberately confined to
# the build tree: this helper models staged build products, not arbitrary host
# filesystem writes.
function(aros_copy_dir_recursive)
    set(oneValueArgs NAME SOURCE DESTINATION)
    set(multiValueArgs DEPENDS)
    cmake_parse_arguments(CDR "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(CDR_UNPARSED_ARGUMENTS OR CDR_KEYWORDS_MISSING_VALUES OR
       NOT CDR_NAME OR NOT CDR_SOURCE OR NOT CDR_DESTINATION)
        message(FATAL_ERROR
            "aros_copy_dir_recursive: NAME, SOURCE and DESTINATION are required")
    endif()

    if(NOT TARGET "${CDR_NAME}")
        add_custom_target("${CDR_NAME}")
    endif()

    if(IS_ABSOLUTE "${CDR_SOURCE}")
        set(_source "${CDR_SOURCE}")
    else()
        set(_source "${CMAKE_SOURCE_DIR}/${CDR_SOURCE}")
    endif()
    cmake_path(NORMAL_PATH _source)

    if(NOT IS_ABSOLUTE "${CDR_DESTINATION}")
        message(FATAL_ERROR
            "${CDR_NAME}: %copy_dir_recursive destination must be absolute: "
            "${CDR_DESTINATION}")
    endif()
    set(_destination "${CDR_DESTINATION}")
    cmake_path(NORMAL_PATH _destination)
    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR NORMALIZE
        OUTPUT_VARIABLE _binary_root)
    cmake_path(IS_PREFIX _binary_root "${_destination}" NORMALIZE
        _destination_owned)
    if(NOT _destination_owned OR _destination STREQUAL _binary_root)
        message(FATAL_ERROR
            "${CDR_NAME}: %copy_dir_recursive destination is outside the build tree: "
            "${_destination}")
    endif()

    # Pick the deepest fetched-port owner, matching the deferred literal
    # %copy_includes logic above.  A port may unpack several nested trees.
    set(_fetch_owner "")
    set(_fetch_owner_len -1)
    get_property(_fetch_targets GLOBAL PROPERTY AROS_FETCH_TARGETS)
    foreach(_fetch IN LISTS _fetch_targets)
        if(NOT TARGET "${_fetch}")
            continue()
        endif()
        get_property(_fetch_dest TARGET "${_fetch}" PROPERTY
            AROS_FETCH_DESTINATION)
        if(NOT _fetch_dest)
            continue()
        endif()
        cmake_path(NORMAL_PATH _fetch_dest)
        string(LENGTH "${_fetch_dest}" _fetch_len)
        string(FIND "${_source}" "${_fetch_dest}/" _fetch_prefix)
        if(("${_source}" STREQUAL "${_fetch_dest}" OR _fetch_prefix EQUAL 0)
           AND _fetch_len GREATER _fetch_owner_len)
            set(_fetch_owner "${_fetch}")
            set(_fetch_owner_len "${_fetch_len}")
        endif()
    endforeach()

    set(_dependencies "")
    foreach(_dependency_target IN LISTS CDR_DEPENDS)
        if(NOT TARGET "${_dependency_target}")
            message(FATAL_ERROR
                "${CDR_NAME}: %copy_dir_recursive dependency target is missing: "
                "${_dependency_target}")
        endif()
        get_property(_dependency_stamp TARGET "${_dependency_target}" PROPERTY
            AROS_FETCH_COMPLETION_STAMP)
        if(_dependency_stamp)
            list(APPEND _dependencies "${_dependency_stamp}")
        else()
            list(APPEND _dependencies "${_dependency_target}")
        endif()
    endforeach()
    if(_fetch_owner)
        get_property(_fetch_stamp TARGET "${_fetch_owner}" PROPERTY
            AROS_FETCH_COMPLETION_STAMP)
        if(NOT _fetch_stamp)
            message(FATAL_ERROR
                "${CDR_NAME}: fetch owner ${_fetch_owner} has no completion stamp")
        endif()
        list(APPEND _dependencies "${_fetch_stamp}")
    endif()
    if(IS_DIRECTORY "${_source}")
        # Concrete in-tree assets should retrigger the staging rule when any
        # payload changes.  Fetched ports are immutable after their completion
        # stamp, so their stamp is the correct dependency instead.
        file(GLOB_RECURSE _source_inputs CONFIGURE_DEPENDS
            LIST_DIRECTORIES FALSE "${_source}/*")
        list(APPEND _dependencies ${_source_inputs})
    elseif(NOT _dependencies)
        message(FATAL_ERROR
            "${CDR_NAME}: %copy_dir_recursive source is absent and has no "
            "declared or matching dependency owner: ${_source}")
    endif()
    list(REMOVE_DUPLICATES _dependencies)

    string(SHA256 _copy_hash
        "${CDR_NAME}|${_source}|${_destination}")
    string(SUBSTRING "${_copy_hash}" 0 16 _copy_hash)
    set(_stamp
        "${CMAKE_BINARY_DIR}/CMakeFiles/aros-copy-dir-${_copy_hash}.stamp")
    set(_copy_target "aros-copy-dir-${_copy_hash}")
    if(NOT TARGET "${_copy_target}")
        add_custom_command(
            OUTPUT "${_stamp}"
            COMMAND "${CMAKE_COMMAND}"
                "-DAROS_COPY_DIR_SOURCE=${_source}"
                "-DAROS_COPY_DIR_DESTINATION=${_destination}"
                "-DAROS_COPY_DIR_STAMP=${_stamp}"
                -P "${CMAKE_SOURCE_DIR}/cmake/CopyDirRecursive.cmake"
            DEPENDS ${_dependencies}
            COMMENT "Staging recursive directory ${CDR_NAME}"
            VERBATIM)
        add_custom_target("${_copy_target}" DEPENDS "${_stamp}")
    endif()
    add_dependencies("${CDR_NAME}" "${_copy_target}")
endfunction()

# A wildcard header list from a fetched port cannot name its outputs while
# CMake configures: the port's source directory materialises only after its
# fetch completion stamp.  Keep the list as one real build-time staging rule
# instead of letting cache state decide which headers enter the graph.
function(_aros_materialize_deferred_header_glob)
    set(options FLATTEN)
    set(oneValueArgs NAME SOURCE DEST PATTERN FETCH)
    set(multiValueArgs EXCLUDES)
    cmake_parse_arguments(DHG "${options}" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(DHG_UNPARSED_ARGUMENTS OR DHG_KEYWORDS_MISSING_VALUES OR
       NOT DHG_NAME OR NOT DHG_SOURCE OR NOT DHG_DEST OR NOT DHG_PATTERN OR
       NOT DHG_FETCH)
        message(FATAL_ERROR
            "_aros_materialize_deferred_header_glob requires NAME, SOURCE, DEST, "
            "PATTERN and FETCH")
    endif()
    if(NOT TARGET "${DHG_NAME}" OR NOT TARGET "${DHG_FETCH}")
        message(FATAL_ERROR
            "${DHG_NAME}: deferred header glob has no declared owner or fetch target")
    endif()
    if(NOT DHG_PATTERN MATCHES "[*?\\[]")
        message(FATAL_ERROR
            "${DHG_NAME}: deferred header staging requires a glob pattern")
    endif()
    foreach(_exclude IN LISTS DHG_EXCLUDES)
        # Do not encode these as one character-class regex: CMake's regex
        # escaping treats `\\n`/`\\r` as ordinary letters, which would reject
        # a legitimate name such as ftoption.h in a cold fetched-port build.
        string(FIND "${_exclude}" ";" _exclude_semicolon)
        string(FIND "${_exclude}" "|" _exclude_pipe)
        string(FIND "${_exclude}" "\n" _exclude_newline)
        string(FIND "${_exclude}" "\r" _exclude_carriage_return)
        if(NOT _exclude_semicolon EQUAL -1 OR
           NOT _exclude_pipe EQUAL -1 OR
           NOT _exclude_newline EQUAL -1 OR
           NOT _exclude_carriage_return EQUAL -1 OR
           _exclude MATCHES "(^|/)\\.\\.(/|$)" OR
           IS_ABSOLUTE "${_exclude}")
            message(FATAL_ERROR
                "${DHG_NAME}: invalid literal header exclusion '${_exclude}'")
        endif()
    endforeach()

    get_property(_fetch_stamp TARGET "${DHG_FETCH}" PROPERTY
        AROS_FETCH_COMPLETION_STAMP)
    if(NOT _fetch_stamp)
        message(FATAL_ERROR
            "${DHG_NAME}: ${DHG_FETCH} has no fetch completion stamp")
    endif()

    if(DHG_FLATTEN)
        set(_flatten 1)
    else()
        set(_flatten 0)
    endif()
    string(JOIN "|" _excludes ${DHG_EXCLUDES})
    string(SHA256 _copy_hash
        "${DHG_NAME}|${DHG_SOURCE}|${DHG_DEST}|${DHG_PATTERN}|${_excludes}|${_flatten}")
    string(SUBSTRING "${_copy_hash}" 0 16 _copy_hash)
    set(_stamp
        "${CMAKE_BINARY_DIR}/CMakeFiles/aros-copy-includes-glob-${_copy_hash}.stamp")
    set(_copy_target "aros-copy-includes-glob-${_copy_hash}")
    set(_stage_script "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/StageHeaderGlob.cmake")
    if(NOT EXISTS "${_stage_script}")
        message(FATAL_ERROR "${DHG_NAME}: missing header-glob staging script")
    endif()
    if(NOT TARGET "${_copy_target}")
        add_custom_command(
            OUTPUT "${_stamp}"
            COMMAND "${CMAKE_COMMAND}"
                "-DAROS_STAGE_HEADERS_SOURCE=${DHG_SOURCE}"
                "-DAROS_STAGE_HEADERS_DEST=${DHG_DEST}"
                "-DAROS_STAGE_HEADERS_PATTERN=${DHG_PATTERN}"
                "-DAROS_STAGE_HEADERS_EXCLUDES=${_excludes}"
                "-DAROS_STAGE_HEADERS_FLATTEN=${_flatten}"
                "-DAROS_STAGE_HEADERS_SDK_ROOT=${AROS_SDK_INCLUDE_DIR}"
                "-DAROS_STAGE_HEADERS_GEN_ROOT=${AROS_GENINC_DIR}"
                "-DAROS_STAGE_HEADERS_STAMP=${_stamp}"
                -P "${_stage_script}"
            DEPENDS "${_fetch_stamp}" "${_stage_script}"
            COMMENT "Staging fetched header group ${DHG_NAME}"
            VERBATIM)
        add_custom_target("${_copy_target}" DEPENDS "${_stamp}")
    endif()
    add_dependencies("${DHG_NAME}" "${_copy_target}")
endfunction()

function(_aros_materialize_deferred_header hash)
    get_property(_done GLOBAL PROPERTY
        "AROS_DEFERRED_HEADER_${hash}_MATERIALIZED")
    if(_done)
        return()
    endif()
    foreach(_field IN ITEMS SOURCE SDK GEN FETCH TARGET LABEL)
        get_property(_${_field} GLOBAL PROPERTY
            "AROS_DEFERRED_HEADER_${hash}_${_field}")
    endforeach()
    if(NOT _SOURCE OR NOT _SDK OR NOT _GEN OR NOT _FETCH OR NOT _TARGET)
        return()
    endif()
    get_filename_component(_sdk_dir "${_SDK}" DIRECTORY)
    get_filename_component(_gen_dir "${_GEN}" DIRECTORY)
    add_custom_command(
        OUTPUT "${_SDK}" "${_GEN}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory
            "${_sdk_dir}" "${_gen_dir}"
        COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${_SOURCE}" "${_SDK}"
        COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${_SOURCE}" "${_GEN}"
        DEPENDS "${_FETCH}"
        COMMENT "Staging fetched header ${_LABEL}"
        VERBATIM)
    set(_copy_target "aros-copy-includes-${hash}")
    add_custom_target("${_copy_target}" DEPENDS "${_SDK}" "${_GEN}")
    add_dependencies("${_TARGET}" "${_copy_target}")
    set_property(GLOBAL PROPERTY
        "AROS_DEFERRED_HEADER_${hash}_MATERIALIZED" TRUE)
endfunction()

# _aros_add_genmodule_quote_dirs(<target> <dir>...)
#
# Puts genmodule's own output directories on the quoted-include search path,
# ahead of everything aros_apply_include_dirs() contributes.
#
# The module's libdefs is reached as `#include LC_LIBDEFS_FILE`, which expands to
# a quoted name, and a quoted include searches every -iquote path before any -I.
# The genmodule directories were only ever added with target_include_directories,
# i.e. as -I, while `${CMAKE_BINARY_DIR}/gen/<module>` goes on as -iquote. So any
# same-named header sitting in the gen/ tree wins over the file the build
# actually generates, no matter that the -I order says otherwise.
#
# That is not a theoretical ordering nicety. 338 of the 340 modules whose libdefs
# existed in both trees were compiling against a stale gen/ copy, 307 of them
# with a different FUNCTIONS_COUNT -- which sizes a library base's jump table, so
# MakeFunctions wrote past the allocation. For kernel.resource that landed on the
# ROM MemHeader in SysBase->MemList. OPEN-POINTS 27g and 50.
#
# Only the genmodule directories are added here. The SDK's posixc and stdc paths
# accompany them in the -I lists but are deliberately left off: promoting those
# to -iquote would move them ahead of the architecture directories, and
# aros_apply_include_dirs() records what that breaks.
function(_aros_add_genmodule_quote_dirs target)
    if(NOT TARGET "${target}")
        return()
    endif()
    set(_quotes "")
    foreach(_dir IN LISTS ARGN)
        if(_dir)
            list(APPEND _quotes "-iquote${_dir}")
        endif()
    endforeach()
    if(_quotes)
        target_compile_options("${target}" BEFORE PRIVATE ${_quotes})
    endif()
endfunction()

# Adds dependencies for literal headers included by a genmodule config and by
# their already-staged in-tree headers.  The config text is copied into
# generated prototypes and link stubs, so CMake's compiler dependency scanner
# cannot discover a missing fetched header until after compilation has already
# started.  Binding the transitive include chain to its `%copy_includes` owners
# closes that cache-empty ordering gap generically.
function(_aros_attach_bound_header_dependencies target owners deferred_hashes)
    if(NOT TARGET "${target}")
        return()
    endif()

    # One public header may include siblings from another declaration sharing
    # the same mmake target (GL/gl.h pulls GL/glext.h and KHR/khrplatform.h).
    # Once any deferred binding reaches an owner, materialise its complete
    # declared header set rather than parsing files which do not exist yet.
    get_property(_all_deferred GLOBAL PROPERTY AROS_DEFERRED_HEADER_HASHES)
    foreach(_deferred_hash IN LISTS deferred_hashes)
        get_property(_deferred_owner GLOBAL PROPERTY
            "AROS_DEFERRED_HEADER_${_deferred_hash}_TARGET")
        foreach(_candidate IN LISTS _all_deferred)
            get_property(_candidate_owner GLOBAL PROPERTY
                "AROS_DEFERRED_HEADER_${_candidate}_TARGET")
            if("${_candidate_owner}" STREQUAL "${_deferred_owner}")
                _aros_materialize_deferred_header("${_candidate}")
            endif()
        endforeach()
    endforeach()
    foreach(_owner IN LISTS owners)
        if(TARGET "${_owner}" AND NOT "${_owner}" STREQUAL "${target}")
            add_dependencies("${target}" "${_owner}")
        endif()
    endforeach()
endfunction()

function(_aros_add_genmodule_config_header_dependencies target config)
    if(NOT TARGET "${target}" OR NOT EXISTS "${config}")
        return()
    endif()
    # The transitive header-owner edges are derived from the declaration text
    # during configuration, so they must be recalculated after it changes.
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
        "${config}")
    _aros_collect_transitive_header_bindings(
        _owners _deferred_hashes "${config}")
    _aros_attach_bound_header_dependencies(
        "${target}" "${_owners}" "${_deferred_hashes}")
endfunction()

# Bind ordinary compiled sources to the %copy_includes declarations whose
# public headers they consume. Compiler depfiles cannot order a missing header:
# they are written only after compilation succeeds. This configure-time pass
# closes that cold-build gap for every compiled target, including fetched Port
# sources reached through aros_resolve_sources() wrappers.
function(_aros_attach_source_header_dependencies)
    _aros_prepare_staged_header_binding_index()
    get_property(_targets DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)
    foreach(_target IN LISTS _targets)
        get_target_property(_type "${_target}" TYPE)
        if(NOT _type MATCHES
           "^(EXECUTABLE|STATIC_LIBRARY|SHARED_LIBRARY|MODULE_LIBRARY|OBJECT_LIBRARY)$")
            continue()
        endif()
        get_target_property(_sources "${_target}" SOURCES)
        get_target_property(_source_dir "${_target}" SOURCE_DIR)
        set(_scan_sources "")
        foreach(_source IN LISTS _sources)
            if(_source MATCHES "^\\$<")
                continue()
            endif()
            if(IS_ABSOLUTE "${_source}")
                set(_source_path "${_source}")
            else()
                set(_source_path "${_source_dir}/${_source}")
            endif()
            cmake_path(NORMAL_PATH _source_path)
            if(EXISTS "${_source_path}" AND NOT IS_DIRECTORY "${_source_path}")
                list(APPEND _scan_sources "${_source_path}")
                set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
                    "${_source_path}")
            endif()
        endforeach()
        if(NOT _scan_sources)
            continue()
        endif()
        _aros_collect_transitive_header_bindings(
            _owners _deferred_hashes ${_scan_sources})
        _aros_attach_bound_header_dependencies(
            "${_target}" "${_owners}" "${_deferred_hashes}")
    endforeach()
endfunction()

function(_aros_write_deferred_header_report)
    get_property(_entries GLOBAL PROPERTY AROS_DEFERRED_HEADER_UNSUPPORTED)
    get_property(_hashes GLOBAL PROPERTY AROS_DEFERRED_HEADER_HASHES)
    list(REMOVE_DUPLICATES _hashes)
    foreach(_hash IN LISTS _hashes)
        get_property(_done GLOBAL PROPERTY
            "AROS_DEFERRED_HEADER_${_hash}_MATERIALIZED")
        if(NOT _done)
            get_property(_label GLOBAL PROPERTY
                "AROS_DEFERRED_HEADER_${_hash}_LABEL")
            list(APPEND _entries "${_label}|no declared genmodule consumer")
        endif()
    endforeach()
    if(_entries)
        list(SORT _entries)
        list(REMOVE_DUPLICATES _entries)
        string(REPLACE ";" "\n" _body "${_entries}")
        file(WRITE "${_AROS_DEFERRED_HEADER_REPORT}" "${_body}\n")
        list(LENGTH _entries _count)
        message(WARNING
            "${_count} deferred header staging rule(s) remain unsupported or "
            "unbound -> ${_AROS_DEFERRED_HEADER_REPORT}")
    else()
        file(REMOVE "${_AROS_DEFERRED_HEADER_REPORT}")
    endif()
endfunction()
cmake_language(DEFER CALL _aros_write_deferred_header_report)

# aros_stage_header(SOURCE <src-relative file> DEST <root-relative file>)
#
# Stages a single header, optionally under a different name. A handful of
# modules publish headers through a hand-written Make rule rather than
# %copy_includes; those rules are arbitrary Make and cannot be transpiled
# generically, so the ones that matter are listed explicitly below.
function(aros_stage_header)
    set(oneValueArgs SOURCE DEST)
    cmake_parse_arguments(SH "" "${oneValueArgs}" "" ${ARGN})
    if(NOT SH_SOURCE OR NOT SH_DEST)
        return()
    endif()
    set(src "${CMAKE_SOURCE_DIR}/${SH_SOURCE}")
    if(NOT EXISTS "${src}")
        return()
    endif()
    foreach(root "${AROS_SDK_INCLUDE_DIR}" "${AROS_GENINC_DIR}")
        configure_file("${src}" "${root}/${SH_DEST}" COPYONLY)
    endforeach()
    get_property(count GLOBAL PROPERTY AROS_STAGED_HEADERS)
    math(EXPR count "${count} + 1")
    set_property(GLOBAL PROPERTY AROS_STAGED_HEADERS ${count})
endfunction()

# --- Hand-written staging rules ---------------------------------------------
#
# Some mmakefiles stage headers with a plain Make rule targeting
# `$(AROS_INCLUDES)/...` instead of %copy_includes. Those recipes are arbitrary
# Make (renames, generator tools, pattern rules), so they get a static
# counterpart here. The transpiler detects every such rule in the tree and
# declares it via aros_adhoc_header_rule(); anything not listed as handled or
# deliberately out of scope below is reported at configure time, so a rule
# added upstream is noticed here and not as a missing header much later.

# Rules with a counterpart in this file. Keys are the rule's target path
# relative to the include root.
set(AROS_ADHOC_HEADERS_HANDLED
    "hidd/pci.h"
    "hidd/thunderbolt.h"
    # Counterpart in cmake/PngLibconf.cmake, installed after libpng fetch.
    "pnglibconf.h"
    # Counterparts in cmake/GeneratedHeaders.cmake.
    "$(CURDIR)/dosboot/nomedia_image.h"
    "$(CURDIR)/bootpic_image.h"
    # libraries/mui.h is generated there by buildincludes; these are the three
    # rules that build it, copy it into the SDK and create its directory.
    "libraries/mui.h"
    "libraries/mui.h $(AROS_INCLUDES)/libraries"
    "libraries $(AROS_INCLUDES)/libraries"
    # The BSD socket header tree, staged there as well.
    "%"
)

# Rules deliberately out of scope, with the reason. Same key format; a trailing
# "*" matches a prefix.
set(AROS_ADHOC_HEADERS_OUT_OF_SCOPE
    # Bootstrapped directly by BootstrapSDK.cmake from compiler/include.
    "%.h" "%.hpp" "exec/execbase.h"
    # Host-tool generated, and only for architectures we do not build.
    "aros/m68k/*" "aros/i386/*" "aros/%.h" "asm/%.h" "exec/%.h" "aros/$(CPU)/asm.h"
    # Directory-creation rule, not a header (compiler/include/mmakefile.src:177).
    "aros/$(CPU)"
    "clib/cia_protos.h" "defines/cia.h" "proto/cia.h"
    # Hosted ports only.
    "sigcore.h"
    # Third-party libraries, not part of a bootable kickstart.
    "tiffconf.h" "tifftypes.h" "tiffinline.h"
    "libraries"
    # A pattern rule whose destination is a directory category, not a file.
    "hidd/%.h"
    # isapnp is x86 legacy and in no package.
    "$(CURDIR)/version.h"
    # libtiff's config header, substituted from a template in the port.
    "$(CURDIR)/tif_config.h"
)

# Files whose rules are ignored wholesale, because the whole subtree is out of
# scope for a bootable target.
#
# workbench/libs/ used to be listed here and should not have been: it hid the
# rules for libraries/mui.h, which 215 compile failures depended on, and for
# reqtools.h with another 62. Ignoring a subtree wholesale silences exactly the
# reports this mechanism exists for.
set(AROS_ADHOC_HEADER_FILES_IGNORED
    "arch/.unmaintained/"
    "tools/"
)

set_property(GLOBAL PROPERTY AROS_ADHOC_HEADERS_UNKNOWN "")

# aros_adhoc_header_rule(FILE <mmakefile> LINE <n> ROOT <root> DEST <path>
#                        PREREQS <text>)
#
# Called from the generated target file for every hand-written header rule the
# transpiler found. Records the ones nobody has accounted for.
#
# ROOT is the generated root the target sits in. $(AROS_INCLUDES) and
# $(GENINCDIR) mean the header is staged into the SDK; $(GENDIR) means it is
# private to its own module. The allowlists below key on DEST alone, which
# stays unambiguous because a $(GENDIR) destination is always prefixed with
# $(CURDIR).
function(aros_adhoc_header_rule)
    set(oneValueArgs FILE LINE ROOT DEST PREREQS)
    cmake_parse_arguments(AR "" "${oneValueArgs}" "" ${ARGN})
    if(NOT AR_DEST)
        return()
    endif()

    # Exact Python/Bison capabilities are declared before the residual header
    # audit. If one already owns this build-tree output, the dependency-only
    # or target-specific-variable line is not an unhandled staging rule.
    separate_arguments(_adhoc_destinations UNIX_COMMAND "${AR_DEST}")
    set(_adhoc_all_owned TRUE)
    foreach(_adhoc_destination IN LISTS _adhoc_destinations)
        if(IS_ABSOLUTE "${_adhoc_destination}")
            set(_adhoc_output "${_adhoc_destination}")
        else()
            set(_adhoc_output "${AR_ROOT}${_adhoc_destination}")
        endif()
        if(NOT _adhoc_output MATCHES "^${CMAKE_BINARY_DIR}/")
            set(_adhoc_all_owned FALSE)
            break()
        endif()
        string(SHA256 _adhoc_output_key "${_adhoc_output}")
        get_property(_adhoc_python_owner GLOBAL PROPERTY
            "AROS_PYTHON_OUTPUT_OWNER_${_adhoc_output_key}")
        if(NOT _adhoc_python_owner)
            set(_adhoc_all_owned FALSE)
            break()
        endif()
    endforeach()
    if(_adhoc_destinations AND _adhoc_all_owned)
        return()
    endif()

    foreach(prefix IN LISTS AROS_ADHOC_HEADER_FILES_IGNORED)
        if(AR_FILE MATCHES "^${prefix}")
            return()
        endif()
    endforeach()

    # compiler/softfloat/mmakefile.src:291 generates its private platform.h
    # through cmake/GeneratedHeaders.cmake.  Keep this ownership file-specific:
    # a broad $(CURDIR)/platform.h allowlist would hide an unrelated future
    # hand-written header rule with the same legacy spelling.
    if(AR_FILE STREQUAL "compiler/softfloat/mmakefile.src" AND
       AR_DEST STREQUAL "$(CURDIR)/platform.h")
        return()
    endif()

    # workbench/libs/freetype2/mmakefile.src:165 generates ftoption.h from
    # the fetched FreeType source with four target-specific substitutions.
    # Its exact CMake counterpart is installed after the generated graph in
    # FreetypeOptions.cmake.  Keep this file-specific so a newly added
    # FreeType header rule is still reported rather than hidden by a prefix.
    if(AR_FILE STREQUAL "workbench/libs/freetype2/mmakefile.src" AND
       AR_DEST STREQUAL "freetype/config/$(FT2OPTIONFILE)")
        return()
    endif()

    # The two HEIC port headers are concrete, fetch-dependent outputs of
    # HeicVersionHeaders.cmake. Keep this acknowledgement file-specific so a
    # future datatype rule still reaches the unknown-rule audit.
    if(AR_FILE STREQUAL "workbench/classes/datatypes/heic/mmakefile.src" AND
       (AR_DEST STREQUAL "$(CURDIR)/libde265/de265-version.h" OR
        AR_DEST STREQUAL "$(CURDIR)/libheif/heif_version.h"))
        return()
    endif()

    if(AR_DEST IN_LIST AROS_ADHOC_HEADERS_HANDLED)
        return()
    endif()

    foreach(pat IN LISTS AROS_ADHOC_HEADERS_OUT_OF_SCOPE)
        if(AR_DEST STREQUAL pat)
            return()
        endif()
        if(pat MATCHES "\\*$")
            string(REGEX REPLACE "\\*$" "" stem "${pat}")
            if(AR_DEST MATCHES "^${stem}")
                return()
            endif()
        endif()
    endforeach()

    get_property(unknown GLOBAL PROPERTY AROS_ADHOC_HEADERS_UNKNOWN)
    list(APPEND unknown
         "${AR_ROOT}${AR_DEST} <- ${AR_PREREQS}  (${AR_FILE}:${AR_LINE})")
    set_property(GLOBAL PROPERTY AROS_ADHOC_HEADERS_UNKNOWN "${unknown}")
endfunction()

# Counterparts for the handled rules above. Both rename on the way in.
#   rom/hidds/pci/mmakefile.src:17
aros_stage_header(SOURCE "rom/hidds/pci/include/pci_hidd.h" DEST "hidd/pci.h")
#   rom/hidds/thunderbolt/mmakefile.src:8
aros_stage_header(SOURCE "rom/hidds/thunderbolt/include/thunderbolt_hidd.h"
                  DEST "hidd/thunderbolt.h")

# =============================================================================
# Include path propagation
# =============================================================================
#
# The historic build feeds each module's USER_INCLUDES into its CFLAGS. The
# transpiler resolves those into an INCLUDES list, plus an ARCH_INCLUDES list of
# "<tag>|<path>" pairs coming from %set_archincludes declarations in the arch/
# tree. The tags that apply to this configuration are computed once here.
#
# MetaMake FAMILY is empty for the bare-metal pc/raspi targets; hosted ports
# may set it to "unix". Keep the standalone include usable without the
# top-level cache declaration, but do not infer FAMILY from the platform.
if(NOT DEFINED AROS_TARGET_FAMILY)
    set(AROS_TARGET_FAMILY "")
endif()

# Tag forms used by %set_archincludes across the tree, most specific first:
# "<platform>-<cpu>", "<platform>", "<cpu>", then the bare-metal group "native".
set(AROS_ARCH_INCLUDE_TAGS
    "${AROS_TARGET_PLATFORM}-${AROS_TARGET_CPU}"
    "${AROS_TARGET_PLATFORM}"
    "${AROS_TARGET_CPU}"
    "native"
)


# aros_gate_arch(<target> <directory>)
#
# Keeps a target out of `all` when its sources live under an arch/ directory
# belonging to a different architecture. The transpiler is target-agnostic and
# emits every declaration it finds, so without this `ninja all` tries to build
# the PowerPC and Windows-hosted kernels against an x86 SDK. 45 objects failed
# that way, with errors that look like missing headers.
#
# Excluded rather than dropped: the target stays in the build graph and can be
# named explicitly, which is what makes it possible to check whether it would
# build at all.
function(aros_gate_arch target directory)
    if(NOT directory OR NOT TARGET ${target})
        return()
    endif()
    file(RELATIVE_PATH _rel "${CMAKE_SOURCE_DIR}" "${directory}")
    if(NOT _rel MATCHES "^arch/([^/]+)")
        return()
    endif()
    set(_arch_dir "${CMAKE_MATCH_1}")
    if(_arch_dir IN_LIST AROS_ARCH_SOURCE_DIRS)
        return()
    endif()
    set_target_properties(${target} PROPERTIES EXCLUDE_FROM_ALL TRUE)
    set_property(TARGET ${target} PROPERTY AROS_FOREIGN_ARCH TRUE)
    get_property(_n GLOBAL PROPERTY AROS_FOREIGN_ARCH_TARGETS)
    list(APPEND _n "${target} (arch/${_arch_dir})")
    set_property(GLOBAL PROPERTY AROS_FOREIGN_ARCH_TARGETS "${_n}")
endfunction()

# aros_gate_platform(TARGET <target> PLATFORMS <platform>... [REASON <text>])
#
# Keeps an otherwise source-tree-neutral target out of `all` when it is tied
# to a particular hosted platform. Unlike a target below arch/, there is no
# directory name from which aros_gate_arch() can infer that restriction. The
# target remains addressable for its supported platform and is reported at
# configure time instead of failing later on an unavailable hosted API.
# aros_apply_includes(<target> [MODULE_DIR <dir>] INCLUDES <dirs...>
#                     ARCH_INCLUDES <tag|dir...>)
#
# Adds the include directories the transpiler resolved. Non-existent
# directories are dropped: an unmapped Make variable or a stale path then shows
# up as a missing header rather than as a confusing CMake include entry.
#
# MODULE_DIR is the module's own source directory. config/make.tmpl:31 passes it
# to every compile, and modules rely on it: a `##begin cdefprivate` block
# typically does `#include "<mod>_intern.h"`, and that block ends up in
# <mod>_libdefs.h under the SDK, from where the private header is only reachable
# through the search path.
function(aros_apply_includes target_name)
    set(oneValueArgs MODULE_DIR)
    set(multiValueArgs INCLUDES ARCH_INCLUDES)
    cmake_parse_arguments(INC "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    # A header name can exist at several levels, and the first match wins, so
    # order matters:
    #
    #   1. the module's generated tree (its private <mod>_libdefs.h)
    #   2. ARCH_INCLUDES from %set_archincludes
    #   3. INCLUDES, i.e. the mmakefile's USER_INCLUDES
    #   4. MODULE_DIR last
    #
    # Step 2 precedes step 3 because that is where the reference puts it:
    # rom/exec writes
    #   PRIV_EXEC_INCLUDES = $(TARGET_EXEC_INCLUDES) -I$(SRCDIR)/rom/exec ...
    # with the architecture variable first, and the transpiler emits that
    # variable separately as ARCH_INCLUDES, losing its position in the list.
    # With rom/exec first, rom/exec/exec_platform.h would shadow
    # arch/all-pc/exec/exec_platform.h and the kernel headers it pulls in would
    # never be reached.
    #
    # MODULE_DIR is only the implicit fallback that lets `#include "x_intern.h"`
    # resolve; the reference build passes it as an -iquote path, which does not
    # take part in `<...>` lookup at all. Putting it first would let
    # rom/timer/timer_platform.h shadow arch/all-pc/timer/timer_platform.h,
    # and the platform struct would come out missing its fields.
    set(GEN_DIRS "")
    set(ARCH_DIRS "")
    set(NAMESPACE_DIRS "")
    set(GENERIC_DIRS "")
    set(FALLBACK_DIRS "")

    if(INC_MODULE_DIR AND IS_DIRECTORY "${INC_MODULE_DIR}")
        file(RELATIVE_PATH _rel "${CMAKE_SOURCE_DIR}" "${INC_MODULE_DIR}")
        if(_rel AND NOT _rel MATCHES "^\\.\\.")
            set(_gen "${CMAKE_BINARY_DIR}/gen/${_rel}")
            if(IS_DIRECTORY "${_gen}")
                list(APPEND GEN_DIRS "${_gen}")
            endif()
        endif()
        list(APPEND FALLBACK_DIRS "${INC_MODULE_DIR}")
    endif()

    # The target compiler's specs search these two libc namespaces before the
    # common SDK include root. Bare-metal Clang has no installed AROS specs, so
    # an exact declaration-local request recreates that lane explicitly. The
    # fixed order is semantic; do not inherit a reversed order from an
    # assignment assembled through several Make variables.
    foreach(_namespace IN ITEMS
            "${AROS_SDK_INCLUDE_DIR}/aros/posixc"
            "${AROS_SDK_INCLUDE_DIR}/aros/stdc")
        if(_namespace IN_LIST INC_INCLUDES)
            file(MAKE_DIRECTORY "${_namespace}")
            list(APPEND NAMESPACE_DIRS "${_namespace}")
        endif()
    endforeach()

    set(_binary_root "${CMAKE_BINARY_DIR}")
    set(_ports_root "${AROS_PORTS_DIR}")
    cmake_path(NORMAL_PATH _binary_root)
    if(_ports_root)
        cmake_path(NORMAL_PATH _ports_root)
    endif()

    foreach(d IN LISTS INC_INCLUDES)
        if(d IN_LIST NAMESPACE_DIRS)
            continue()
        endif()
        # A path inside the build tree normally holds generated files, so it
        # may not exist yet at configure time: rom/dos asks for
        # -I$(GENDIR)/$(CURDIR)/dos, which only appears once its catalog
        # headers are built. Fetch-owned paths below Ports are the exception:
        # keep the include entry, but never pre-create an archive's extraction
        # root. aros-fetch is the sole owner of those trees and deliberately
        # rejects an existing destination without a completion stamp.
        cmake_path(IS_PREFIX _binary_root "${d}" NORMALIZE _in_build)
        set(_in_ports FALSE)
        if(_ports_root)
            cmake_path(IS_PREFIX _ports_root "${d}" NORMALIZE _in_ports)
        endif()
        if(_in_build)
            if(NOT _in_ports)
                file(MAKE_DIRECTORY "${d}")
            endif()
            list(APPEND GENERIC_DIRS "${d}")
        elseif(IS_DIRECTORY "${d}")
            list(APPEND GENERIC_DIRS "${d}")
        endif()
    endforeach()

    if(NAMESPACE_DIRS)
        list(REMOVE_DUPLICATES NAMESPACE_DIRS)
        target_include_directories(${target_name} BEFORE PRIVATE
            ${NAMESPACE_DIRS})
    endif()

    foreach(pair IN LISTS INC_ARCH_INCLUDES)
        # Split "<tag>|<path>"; a path may not contain "|".
        string(FIND "${pair}" "|" sep)
        if(sep LESS 0)
            continue()
        endif()
        string(SUBSTRING "${pair}" 0 ${sep} tag)
        math(EXPR rest "${sep} + 1")
        string(SUBSTRING "${pair}" ${rest} -1 path)

        if(NOT tag IN_LIST AROS_ARCH_INCLUDE_TAGS)
            continue()
        endif()
        if(IS_DIRECTORY "${path}")
            list(APPEND ARCH_DIRS "${path}")
        endif()
    endforeach()

    set(DIRS ${GEN_DIRS} ${ARCH_DIRS} ${GENERIC_DIRS} ${FALLBACK_DIRS})

    if(DIRS)
        list(REMOVE_DUPLICATES DIRS)
        target_include_directories(${target_name} PRIVATE ${DIRS})
    endif()

    # A generated header can share its name with a system header: rom/dos
    # generates strings.h, and the SDK also stages the POSIX strings.h. The
    # historic build has no conflict, because %build_catalogs writes the
    # generated one next to the sources, where `#include "strings.h"` finds it
    # before any -I path.
    #
    # -iquote reproduces that without writing into the source tree: it applies
    # only to the quoted form and is searched ahead of every -I, so `<strings.h>`
    # still reaches the POSIX header.
    #
    # The order has to be the same as for -I, ARCH_DIRS included. Leaving them
    # out put every -iquote path ahead of every architecture path, so
    # `#include "kernel_debug.h"` in arch/x86_64-pc/kernel resolved to
    # rom/kernel's header instead of arch/all-pc's, and __cli went missing.
    set(QUOTE_DIRS
        ${GEN_DIRS} ${ARCH_DIRS} ${NAMESPACE_DIRS}
        ${GENERIC_DIRS} ${FALLBACK_DIRS})
    if(QUOTE_DIRS)
        list(REMOVE_DUPLICATES QUOTE_DIRS)
        foreach(d IN LISTS QUOTE_DIRS)
            target_compile_options(${target_name} PRIVATE "-iquote${d}")
        endforeach()
    endif()
endfunction()

# aros_apply_flags(<target> DEFINES <d...> UNDEFINES <u...> COMPILE_OPTIONS <o...>)
#
# Applies the preprocessor state the transpiler resolved from USER_CPPFLAGS and
# USER_CFLAGS. Modules depend on these for semantics: rom/devs/ahci declares the
# method-base fields of its library base only under
# `#if defined(__OOP_NOMETHODBASES__)`, so the define decides whether the module
# compiles at all.
#
# Only simple defines and an allowlisted set of codegen options arrive here;
# warning bundles and shell-built defines are reported by the transpiler and
# deliberately not passed on.
function(aros_apply_flags target_name)
    set(multiValueArgs DEFINES UNDEFINES COMPILE_OPTIONS
        ARCH_DEFINES ARCH_COMPILE_OPTIONS)
    cmake_parse_arguments(FL "" "" "${multiValueArgs}" ${ARGN})

    # Architecture-conditional flags come from an `arch/.../make.opts` pulled in
    # with `-include`. arch/all-pc/timer/make.opts sets -DUSE_VBLANK_EMU, which
    # is what makes rom/timer/timer_intern.h declare tb_vblank_timerequest.
    set(_arch_defines "")
    foreach(pair IN LISTS FL_ARCH_DEFINES)
        string(FIND "${pair}" "|" sep)
        if(sep LESS 0)
            continue()
        endif()
        string(SUBSTRING "${pair}" 0 ${sep} tag)
        math(EXPR rest "${sep} + 1")
        string(SUBSTRING "${pair}" ${rest} -1 value)
        if(tag IN_LIST AROS_ARCH_INCLUDE_TAGS)
            list(APPEND _arch_defines "${value}")
        endif()
    endforeach()
    if(_arch_defines)
        list(REMOVE_DUPLICATES _arch_defines)
        target_compile_definitions(${target_name} PRIVATE ${_arch_defines})
    endif()

    set(_arch_opts "")
    foreach(pair IN LISTS FL_ARCH_COMPILE_OPTIONS)
        string(FIND "${pair}" "|" sep)
        if(sep LESS 0)
            continue()
        endif()
        string(SUBSTRING "${pair}" 0 ${sep} tag)
        math(EXPR rest "${sep} + 1")
        string(SUBSTRING "${pair}" ${rest} -1 value)
        if(tag IN_LIST AROS_ARCH_INCLUDE_TAGS)
            list(APPEND _arch_opts "${value}")
        endif()
    endforeach()
    if(_arch_opts)
        list(REMOVE_DUPLICATES _arch_opts)
        target_compile_options(${target_name} PRIVATE ${_arch_opts})
    endif()

    if(FL_DEFINES)
        target_compile_definitions(${target_name} PRIVATE ${FL_DEFINES})
    endif()
    foreach(u IN LISTS FL_UNDEFINES)
        target_compile_options(${target_name} PRIVATE "-U${u}")
    endforeach()
    if(FL_COMPILE_OPTIONS)
        target_compile_options(${target_name} PRIVATE ${FL_COMPILE_OPTIONS})
    endif()
endfunction()

# Bind one complete MetaMake library list after all currently available target
# names are known.  Keeping this separate from the public helper lets a
# forward reference retain its original group and item order instead of
# splitting the link line into "already declared" and "declared later"
# fragments.
function(_aros_bind_link_libraries target_name)
    if(NOT TARGET "${target_name}")
        return()
    endif()

    set(CLEAN_LIBS "")
    set(_client_namespace_includes "")
    foreach(lib ${ARGN})
        if(lib STREQUAL "debug" OR lib STREQUAL "optimized" OR lib STREQUAL "general")
            # Avoid CMake build-type keywords
        elseif(TARGET ${lib})
            list(APPEND CLEAN_LIBS "${lib}")
            get_target_property(_namespace_includes "${lib}"
                AROS_CLIENT_NAMESPACE_INCLUDES)
            if(_namespace_includes AND
               NOT _namespace_includes STREQUAL
                   "_namespace_includes-NOTFOUND")
                list(APPEND _client_namespace_includes
                    ${_namespace_includes})
            endif()
        endif()
    endforeach()
    if(_client_namespace_includes)
        list(REMOVE_DUPLICATES _client_namespace_includes)
        # A client archive whose config declares relative C-runtime libraries
        # exposes prototypes using those namespaces.  Propagate only those
        # proven include roots to its consumers.
        #
        # Not BEFORE. This call runs after the target's own prepends, so BEFORE
        # put the propagated namespace ahead of everything, including the
        # target's own. compiler-posixc links stdc_rel, so it came out with
        # aros/stdc first and aros/posixc second, and a bare <limits.h> then
        # resolved to the C99 variant instead of the POSIX superset that
        # includes it. 179 build steps failed on PASS_MAX, PATH_MAX, off_t,
        # EBADF and EISDIR while every header was staged correctly.
        #
        # Appending is sufficient for the stated purpose: the directory-level
        # include_directories() above already lists aros/posixc and aros/stdc,
        # in that order, ahead of the common SDK root, so a propagated
        # namespace needs no help to be found before it.
        target_include_directories(${target_name} PRIVATE
            ${_client_namespace_includes})
    endif()
    if(CLEAN_LIBS)
        # MetaMake's link_module_q rescans the explicit uselibs/rellibs as a
        # group.  Keep the marker tokens in the link-item lane: the canonical
        # AROS rule invokes ld.lld directly, so compiler-driver LINKER:
        # translation would be incorrect here.
        target_link_libraries(${target_name} PRIVATE
            --start-group ${CLEAN_LIBS} --end-group)
    endif()
endfunction()

# Helper to filter CMake keyword collisions in link libraries.  Generated
# declarations are sorted for reproducibility, not dependency order, so a
# consumer may legitimately precede its provider.  CMake's TARGET predicate is
# false for that forward reference at the point the consumer macro runs.  Save
# the complete invocation and bind it once every concrete declaration exists;
# truly unknown names are still discarded by _aros_bind_link_libraries, as
# they were before.
function(aros_link_libraries target_name)
    if(NOT TARGET "${target_name}")
        return()
    endif()

    set(_has_forward_reference FALSE)
    foreach(lib ${ARGN})
        if(lib STREQUAL "debug" OR lib STREQUAL "optimized" OR
           lib STREQUAL "general")
            continue()
        endif()
        if(NOT TARGET "${lib}")
            set(_has_forward_reference TRUE)
            break()
        endif()
    endforeach()

    if(NOT _has_forward_reference)
        _aros_bind_link_libraries("${target_name}" ${ARGN})
        return()
    endif()

    get_property(_serial GLOBAL PROPERTY AROS_DEFERRED_LINK_SERIAL)
    if(NOT _serial)
        set(_serial 0)
    endif()
    math(EXPR _serial "${_serial} + 1")
    set_property(GLOBAL PROPERTY AROS_DEFERRED_LINK_SERIAL "${_serial}")
    set_property(GLOBAL APPEND PROPERTY AROS_DEFERRED_LINK_IDS "${_serial}")
    set_property(GLOBAL PROPERTY
        "AROS_DEFERRED_LINK_${_serial}_TARGET" "${target_name}")
    set_property(GLOBAL PROPERTY
        "AROS_DEFERRED_LINK_${_serial}_LIBRARIES" "${ARGN}")

    # The generated file finalises immediately after its concrete target
    # section.  This directory-end fallback also covers hand-written callers
    # which introduce another forward reference later in configuration.
    cmake_language(DEFER CALL aros_finalize_link_libraries)
endfunction()

function(aros_finalize_link_libraries)
    get_property(_ids GLOBAL PROPERTY AROS_DEFERRED_LINK_IDS)
    if(NOT _ids)
        return()
    endif()

    # Clear the queue before binding so this function is idempotent and a
    # directory-end deferred call cannot replay an already applied link list.
    set_property(GLOBAL PROPERTY AROS_DEFERRED_LINK_IDS "")
    foreach(_id IN LISTS _ids)
        get_property(_target GLOBAL PROPERTY
            "AROS_DEFERRED_LINK_${_id}_TARGET")
        get_property(_libraries GLOBAL PROPERTY
            "AROS_DEFERRED_LINK_${_id}_LIBRARIES")
        if(TARGET "${_target}")
            _aros_bind_link_libraries("${_target}" ${_libraries})
        endif()
        set_property(GLOBAL PROPERTY
            "AROS_DEFERRED_LINK_${_id}_TARGET" "")
        set_property(GLOBAL PROPERTY
            "AROS_DEFERRED_LINK_${_id}_LIBRARIES" "")
    endforeach()
endfunction()

# Apply the graph-validated declaration-local USER_LDFLAGS snapshot. MetaMake
# places this lane after the objects. Only `-l<name>` items with a proven public
# archive producer or an exact matching private `-L` directory reach this
# point. CMake may therefore emit them in <LINK_LIBRARIES> even though the
# canonical rule invokes ld.lld rather than a compiler driver.
function(aros_apply_link_options target_name)
    if(TARGET "${target_name}" AND ARGN)
        target_link_directories("${target_name}" PRIVATE
            "${AROS_DEVELOPER_LIB_DIR}")
        target_link_libraries("${target_name}" PRIVATE ${ARGN})
    endif()
endfunction()

# Attach a generated MetaMake edge and mirror it to %build_progs members.
# CMake dependencies point from an aggregate to its prerequisites, so a fetch
# attached only to the aggregate does not constrain a direct Ninja build of an
# individual program. The group records its realised member targets below.
function(aros_add_target_dependency target_name dependency)
    if(NOT TARGET "${target_name}" OR NOT TARGET "${dependency}")
        return()
    endif()
    get_target_property(_target_is_foreign "${target_name}" AROS_FOREIGN_ARCH)
    get_target_property(_dependency_is_foreign "${dependency}" AROS_FOREIGN_ARCH)
    if(_dependency_is_foreign AND NOT _target_is_foreign)
        # EXCLUDE_FROM_ALL alone is not transitive: a common meta target can
        # otherwise pull an ARM-only producer back into an x86 build. Make's
        # architecture-selected directory graph has no such edge.
        return()
    endif()
    add_dependencies("${target_name}" "${dependency}")

    # Output-producing #MM prerequisites may publish a private generated
    # include directory. Apply it only to compilable consumers, preserving
    # quoted-include semantics and avoiding directory-wide global includes.
    get_property(_generated_include TARGET "${dependency}"
        PROPERTY AROS_GENERATED_INCLUDE_DIRECTORY)
    set(_include_consumers "${target_name}")

    get_target_property(_members "${target_name}" AROS_PROGRAM_GROUP_MEMBERS)
    if(_members AND NOT _members STREQUAL "_members-NOTFOUND")
        list(APPEND _include_consumers ${_members})
    endif()
    list(REMOVE_DUPLICATES _include_consumers)
    foreach(_consumer IN LISTS _include_consumers)
        if(NOT TARGET "${_consumer}")
            continue()
        endif()
        if(NOT "${_consumer}" STREQUAL "${target_name}")
            add_dependencies("${_consumer}" "${dependency}")
        endif()
        if(_generated_include)
            get_target_property(_consumer_type "${_consumer}" TYPE)
            if(_consumer_type MATCHES
                    "^(EXECUTABLE|STATIC_LIBRARY|SHARED_LIBRARY|MODULE_LIBRARY|OBJECT_LIBRARY)$")
                get_target_property(_attached "${_consumer}"
                    AROS_GENERATED_DEPENDENCY_INCLUDE_DIRS)
                if(NOT "${_generated_include}" IN_LIST _attached)
                    target_compile_options("${_consumer}" BEFORE PRIVATE
                        "-iquote${_generated_include}")
                    set_property(TARGET "${_consumer}" APPEND PROPERTY
                        AROS_GENERATED_DEPENDENCY_INCLUDE_DIRS
                        "${_generated_include}")
                endif()
            endif()
        endif()
    endforeach()
endfunction()

# The generated full-tree graph creates linklibs-startup only after this
# module is loaded. Bind its aggregate edge at that point, while preserving a
# fail-closed error if a locked consumer ever loses the linker-visible object.
function(aros_bind_cxx_startup_target target_name)
    if(NOT AROS_CROSS_TOOLCHAIN_ROOT)
        return()
    endif()
    if(NOT TARGET "${target_name}")
        message(FATAL_ERROR
            "Locked AROS C++ consumer requires aggregate target ${target_name}")
    endif()
    if(NOT AROS_CXX_STARTUP_TARGET OR
       NOT TARGET "${AROS_CXX_STARTUP_TARGET}")
        message(FATAL_ERROR
            "Locked AROS C++ consumer has no cxx-startup producer target")
    endif()
    aros_add_target_dependency("${target_name}" "${AROS_CXX_STARTUP_TARGET}")
endfunction()

# A fetched source named without its suffix cannot be a Ninja source node: the
# archive is deliberately absent at configure time. Marking that path GENERATED
# looks tempting, but CMake then gives it target-order dependencies. If two
# dependent targets compile the same port source, those synthetic dependencies
# form a cycle through the shared file.
#
# Use an ordinary, configure-time proxy instead. The owning target's fetch
# dependency orders compilation after unpacking, while the preprocessor records
# the real source in its depfile. Including rather than copying also preserves
# quoted-include lookup relative to the upstream source. C++ needs several
# candidates because imported projects use .cpp, .cxx, .cc, .c++ and .C.
function(_aros_port_source_proxy out_var source_stem language explicit_suffix)
    string(SHA256 _source_hash
        "${language}|${explicit_suffix}|${source_stem}")
    set(_proxy_dir "${CMAKE_BINARY_DIR}/CMakeFiles/aros-port-sources")
    if(language STREQUAL "CXX")
        set(_proxy_suffix ".cpp")
        set(_candidate_suffixes ".cpp" ".cxx" ".cc" ".c++" ".C")
    elseif(language STREQUAL "OBJC")
        set(_proxy_suffix ".m")
        set(_candidate_suffixes ".m")
    elseif(language STREQUAL "ASM")
        if(source_stem MATCHES "\\.s$")
            set(_proxy_suffix ".s")
        else()
            set(_proxy_suffix ".S")
        endif()
        set(_candidate_suffixes "")
    else()
        set(_proxy_suffix ".c")
        set(_candidate_suffixes ".c")
    endif()
    set(_proxy "${_proxy_dir}/${_source_hash}${_proxy_suffix}")

    get_property(_proxy_created GLOBAL PROPERTY
        "AROS_PORT_SOURCE_PROXY_${_source_hash}")
    if(NOT _proxy_created)
        file(MAKE_DIRECTORY "${_proxy_dir}")
        string(CONCAT _content
            "/* Generated by aros_resolve_sources; do not edit. */\n"
            "#if !defined(__has_include)\n"
            "# error The compiler cannot select an unfetched AROS port source\n"
            "#endif\n")
        set(_branch "if")
        set(_candidate_comments "")
        set(_candidates "${source_stem}")
        if(NOT explicit_suffix)
            foreach(_suffix IN LISTS _candidate_suffixes)
                list(APPEND _candidates "${source_stem}${_suffix}")
            endforeach()
        endif()
        foreach(_candidate IN LISTS _candidates)
            string(REPLACE "\\" "\\\\" _include_source "${_candidate}")
            string(REPLACE "\"" "\\\"" _include_source "${_include_source}")
            string(APPEND _content
                "#${_branch} __has_include(\"${_include_source}\")\n"
                "# include \"${_include_source}\"\n")
            string(APPEND _candidate_comments " *   ${_candidate}\n")
            set(_branch "elif")
        endforeach()
        string(APPEND _content
            "#else\n"
            "/* Expected one of:\n${_candidate_comments} */\n"
            "# error Fetched AROS port source was not found; check the fetch dependency\n"
            "#endif\n")
        set(_write_proxy TRUE)
        if(EXISTS "${_proxy}" AND NOT IS_DIRECTORY "${_proxy}")
            file(READ "${_proxy}" _existing_content)
            if(_existing_content STREQUAL _content)
                set(_write_proxy FALSE)
            endif()
        endif()
        if(_write_proxy)
            file(WRITE "${_proxy}" "${_content}")
        endif()
        set_property(GLOBAL PROPERTY
            "AROS_PORT_SOURCE_PROXY_${_source_hash}" TRUE)
    endif()

    if(NOT language STREQUAL "C")
        set_source_files_properties("${_proxy}" PROPERTIES
            LANGUAGE "${language}")
    endif()
    set(${out_var} "${_proxy}" PARENT_SCOPE)
endfunction()

# Resolve source files with or without extensions.  Legacy positional calls
# retain the old .c/.cpp/.S/.s search.  New transpiler output supplies a
# LANGUAGE lane so an extensionless source in an unfetched port can be kept in
# the graph without guessing C versus C++.
#
#   aros_resolve_sources(<out> <module-dir> <legacy-sources...>)
#   aros_resolve_sources(<out> <module-dir>
#       LANGUAGE <C|CXX|OBJC|ASM> SOURCES <sources...>)
function(aros_resolve_sources out_var dir)
    set(oneValueArgs LANGUAGE MMAKE_ID)
    set(multiValueArgs SOURCES)
    cmake_parse_arguments(RS "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(RS_SOURCES)
        set(_sources ${RS_SOURCES})
    else()
        set(_sources ${RS_UNPARSED_ARGUMENTS})
    endif()

    if(RS_LANGUAGE STREQUAL "OBJC")
        get_property(_enabled_languages GLOBAL PROPERTY ENABLED_LANGUAGES)
        if(NOT "OBJC" IN_LIST _enabled_languages)
            if(_sources)
                string(REPLACE ";" ", " _source_list "${_sources}")
                message(WARNING
                    "Objective-C source lane in '${dir}' is skipped because "
                    "the OBJC language is not enabled: ${_source_list}")
                get_property(_skipped GLOBAL PROPERTY
                    AROS_SKIPPED_SOURCE_LANGUAGE_LANES)
                list(APPEND _skipped
                    "${RS_MMAKE_ID}|OBJC|${dir}|${_source_list}|language-not-enabled")
                set_property(GLOBAL PROPERTY
                    AROS_SKIPPED_SOURCE_LANGUAGE_LANES "${_skipped}")
            endif()
            set(${out_var} "" PARENT_SCOPE)
            return()
        endif()
    endif()

    if(RS_LANGUAGE STREQUAL "C")
        set(_suffixes ".c")
    elseif(RS_LANGUAGE STREQUAL "CXX")
        set(_suffixes ".cpp" ".cxx" ".cc" ".c++" ".C")
    elseif(RS_LANGUAGE STREQUAL "OBJC")
        set(_suffixes ".m")
    elseif(RS_LANGUAGE STREQUAL "ASM")
        set(_suffixes ".S" ".s")
    else()
        set(_suffixes ".c" ".cpp" ".S" ".s")
    endif()

    set(_ports_root "${AROS_PORTS_DIR}")
    if(_ports_root)
        cmake_path(ABSOLUTE_PATH _ports_root
            BASE_DIRECTORY "${CMAKE_BINARY_DIR}" NORMALIZE)
    endif()

    set(RESOLVED "")
    foreach(src IN LISTS _sources)
        if(IS_ABSOLUTE "${src}")
            set(_source_stem "${src}")
        else()
            set(_source_stem "${dir}/${src}")
        endif()
        cmake_path(ABSOLUTE_PATH _source_stem
            BASE_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}" NORMALIZE)

        set(_candidates "${_source_stem}")
        foreach(_suffix IN LISTS _suffixes)
            list(APPEND _candidates "${_source_stem}${_suffix}")
        endforeach()

        set(_resolved "")
        # Exact registered generator products take precedence over filesystem
        # probes. On a case-insensitive host a generated lowercase `.s` also
        # satisfies EXISTS for the earlier `.S` candidate, but those spellings
        # have different preprocessing semantics and CMake object identities.
        foreach(_candidate IN LISTS _candidates)
            string(SHA256 _generated_output_key "${_candidate}")
            # Paired hand-written FlexCat rules generate a C translation unit
            # next to a module in MetaMake. CMake rehomes it under gen/ to
            # keep the source tree clean; consult that exact nominal-path map
            # before touching the filesystem so a stale source-tree locale.c
            # can never win over the declared output.
            get_property(_flexcat_generated_source GLOBAL PROPERTY
                "AROS_FLEXCAT_GENERATED_SOURCE_${_generated_output_key}")
            if(_flexcat_generated_source)
                set(_resolved "${_flexcat_generated_source}")
                set_source_files_properties("${_resolved}" PROPERTIES GENERATED TRUE)
                if(RS_LANGUAGE AND NOT RS_LANGUAGE STREQUAL "C")
                    set_source_files_properties("${_resolved}" PROPERTIES
                        LANGUAGE "${RS_LANGUAGE}")
                endif()
                list(APPEND RESOLVED "${_resolved}")
                break()
            endif()
            get_property(_generated_output_owner GLOBAL PROPERTY
                "AROS_PYTHON_OUTPUT_OWNER_${_generated_output_key}")
            if(NOT _generated_output_owner)
                continue()
            endif()
            set(_resolved "${_candidate}")
            set_source_files_properties("${_resolved}" PROPERTIES GENERATED TRUE)
            if(RS_LANGUAGE AND NOT RS_LANGUAGE STREQUAL "C")
                set_source_files_properties("${_resolved}" PROPERTIES
                    LANGUAGE "${RS_LANGUAGE}")
            endif()
            list(APPEND RESOLVED "${_resolved}")
            break()
        endforeach()
        if(_resolved)
            continue()
        endif()

        # Sources below an audited generator's fetched source root stay behind
        # their cold-configure proxies after fetch as well. This prevents a
        # normal reconfigure from replacing every object node with a differently
        # named direct source path.
        set(_stable_port_source FALSE)
        get_property(_stable_source_roots GLOBAL PROPERTY
            AROS_STABLE_PORT_SOURCE_ROOTS)
        foreach(_stable_source_root IN LISTS _stable_source_roots)
            cmake_path(IS_PREFIX _stable_source_root "${_source_stem}"
                NORMALIZE _inside_stable_source_root)
            if(_inside_stable_source_root)
                set(_stable_port_source TRUE)
                break()
            endif()
        endforeach()

        if(NOT _stable_port_source)
            foreach(_candidate IN LISTS _candidates)
                if(EXISTS "${_candidate}" AND NOT IS_DIRECTORY "${_candidate}")
                    set(_resolved "${_candidate}")
                    break()
                endif()
            endforeach()
        endif()
        if(_resolved)
            # Explicit non-C lanes are authoritative even when an upstream
            # project gives the file an unusual or extensionless name.
            if(RS_LANGUAGE AND NOT RS_LANGUAGE STREQUAL "C")
                set_source_files_properties("${_resolved}" PROPERTIES
                    LANGUAGE "${RS_LANGUAGE}")
            endif()
            list(APPEND RESOLVED "${_resolved}")
            continue()
        endif()

        # Missing in-tree sources remain rejected.  Only the fetched-port root
        # is allowed to contribute sources that do not exist at configure time;
        # NORMALIZE prevents a Ports/../ path from escaping that root.
        set(_is_port_source FALSE)
        if(_ports_root)
            cmake_path(IS_PREFIX _ports_root "${_source_stem}"
                NORMALIZE _is_port_source)
        endif()
        if(NOT _is_port_source)
            # A declared in-tree source that resolves to nothing used to be
            # dropped here in silence, so a target quietly built with fewer
            # objects than its declaration names. That is how
            # `linklibs-udis86` came to be missing its generated itab.c with
            # nothing to say so: the compile then failed on `itab.h` instead,
            # one step removed from the cause.
            get_property(_missing GLOBAL PROPERTY AROS_MISSING_SOURCES)
            list(APPEND _missing "${RS_MMAKE_ID}|${dir}|${src}")
            set_property(GLOBAL PROPERTY AROS_MISSING_SOURCES "${_missing}")
            continue()
        endif()

        # Route every absent port source through an existing proxy. Even an
        # explicit path must not be a shared GENERATED Ninja node (see the
        # cycle explanation on _aros_port_source_proxy).
        set(_explicit_source_suffix FALSE)
        if(_source_stem MATCHES
           "\\.(c|C|cc|cp|cpp|CPP|cxx|m|mm|M|s|S|asm)$" OR
           _source_stem MATCHES "\\.c\\+\\+$")
            set(_explicit_source_suffix TRUE)
        endif()

        set(_proxy_language "${RS_LANGUAGE}")
        if(NOT _proxy_language AND _explicit_source_suffix)
            if(_source_stem MATCHES "\\.(C|cc|cp|cpp|CPP|cxx)$" OR
               _source_stem MATCHES "\\.c\\+\\+$")
                set(_proxy_language CXX)
            elseif(_source_stem MATCHES "\\.(m|mm|M)$")
                set(_proxy_language OBJC)
            elseif(_source_stem MATCHES "\\.(s|S|asm)$")
                set(_proxy_language ASM)
            else()
                set(_proxy_language C)
            endif()
        endif()

        if(_proxy_language STREQUAL "ASM" AND
           NOT _explicit_source_suffix)
            # There is no safe extensionless fallback for assembly: .S and .s
            # differ in preprocessing semantics. Require an explicit suffix.
            message(WARNING
                "Cannot infer .S versus .s for fetched assembly source '${src}'; "
                "the declaration is not emitted")
        elseif(_proxy_language)
            if(_proxy_language STREQUAL "OBJC")
                get_property(_enabled_languages GLOBAL PROPERTY ENABLED_LANGUAGES)
                if(NOT "OBJC" IN_LIST _enabled_languages)
                    message(WARNING
                        "Fetched Objective-C source '${src}' is skipped because "
                        "the OBJC language is not enabled")
                    continue()
                endif()
            endif()
            _aros_port_source_proxy(_resolved "${_source_stem}"
                "${_proxy_language}" "${_explicit_source_suffix}")
            list(APPEND RESOLVED "${_resolved}")
        endif()
    endforeach()
    set(${out_var} "${RESOLVED}" PARENT_SCOPE)
endfunction()

function(aros_resolve_source_lanes out_var dir)
    set(oneValueArgs MMAKE_ID)
    set(multiValueArgs SOURCES CXX_SOURCES OBJC_SOURCES ASM_SOURCES)
    cmake_parse_arguments(SL "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    aros_resolve_sources(_c_sources "${dir}"
        LANGUAGE C MMAKE_ID "${SL_MMAKE_ID}" SOURCES ${SL_SOURCES})
    aros_resolve_sources(_cxx_sources "${dir}"
        LANGUAGE CXX MMAKE_ID "${SL_MMAKE_ID}" SOURCES ${SL_CXX_SOURCES})
    aros_resolve_sources(_objc_sources "${dir}"
        LANGUAGE OBJC MMAKE_ID "${SL_MMAKE_ID}" SOURCES ${SL_OBJC_SOURCES})
    aros_resolve_sources(_asm_sources "${dir}"
        LANGUAGE ASM MMAKE_ID "${SL_MMAKE_ID}" SOURCES ${SL_ASM_SOURCES})

    set(_resolved
        ${_c_sources}
        ${_cxx_sources}
        ${_objc_sources}
        ${_asm_sources})
    list(REMOVE_ITEM _resolved "")
    set(${out_var} "${_resolved}" PARENT_SCOPE)
endfunction()

# Record a concrete declaration only after generic and architecture sources
# have both been resolved. Reporting in aros_resolve_source_lanes is too early:
# a valid architecture-only target starts with an empty generic lane.
function(aros_report_empty_concrete_target)
    set(oneValueArgs MMAKE_ID DIRECTORY)
    set(multiValueArgs RESOLVED_SOURCES SOURCES CXX_SOURCES OBJC_SOURCES
        ASM_SOURCES ARCH_SOURCES)
    cmake_parse_arguments(EC "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(EC_RESOLVED_SOURCES OR NOT EC_MMAKE_ID)
        return()
    endif()

    set(_declared_lanes "")
    set(_empty_lane_reasons "")
    get_property(_enabled_languages GLOBAL PROPERTY ENABLED_LANGUAGES)
    foreach(_lane IN ITEMS C CXX OBJC ASM ARCH)
        if(_lane STREQUAL "C")
            set(_declared ${EC_SOURCES})
        elseif(_lane STREQUAL "CXX")
            set(_declared ${EC_CXX_SOURCES})
        elseif(_lane STREQUAL "OBJC")
            set(_declared ${EC_OBJC_SOURCES})
        elseif(_lane STREQUAL "ASM")
            set(_declared ${EC_ASM_SOURCES})
        else()
            set(_declared ${EC_ARCH_SOURCES})
        endif()
        if(NOT _declared)
            continue()
        endif()

        string(REPLACE ";" "," _declared_csv "${_declared}")
        string(REPLACE "|" "%7C" _declared_csv "${_declared_csv}")
        list(APPEND _declared_lanes "${_lane}=[${_declared_csv}]")
        if(_lane STREQUAL "OBJC" AND
           NOT "OBJC" IN_LIST _enabled_languages)
            list(APPEND _empty_lane_reasons
                "${_lane}=language-not-enabled")
        else()
            list(APPEND _empty_lane_reasons
                "${_lane}=source-paths-not-resolved")
        endif()
    endforeach()

    if(NOT _declared_lanes)
        return()
    endif()
    string(JOIN "," _declared_summary ${_declared_lanes})
    string(JOIN "," _reason_summary ${_empty_lane_reasons})
    get_property(_empty_targets GLOBAL PROPERTY AROS_EMPTY_CONCRETE_TARGETS)
    list(APPEND _empty_targets
        "${EC_MMAKE_ID}|${EC_DIRECTORY}|${_declared_summary}|${_reason_summary}")
    set_property(GLOBAL PROPERTY AROS_EMPTY_CONCRETE_TARGETS
        "${_empty_targets}")
endfunction()

# =============================================================================
# Third-party source fetching
# =============================================================================
#
# Some modules build against sources that are not in the tree; ACPICA is the one
# that matters here, since libraries/acpica.h pulls headers out of the unpacked
# archive. The transpiler turns each %fetch declaration into an
# aros_fetch_archive() call.
#
# Downloading is delegated to the validated Rust aros-fetch tool. The upstream
# scripts/fetch.sh remains available to the unmodified GNU Make build, but the
# AROS-NX CMake path never silently falls back to the shell implementation.
# These targets are deliberately NOT part of `all`: fetching
# reaches out to the network, so it stays an explicit step.
#
#   ninja fetch-ports          # everything
#   ninja acpica-fetch         # one archive

set(AROS_PORTS_DIR "${CMAKE_BINARY_DIR}/Ports"
    CACHE PATH "Where fetched third-party sources are unpacked")
set(AROS_PORTS_SOURCE_DIR "${CMAKE_BINARY_DIR}/portssources"
    CACHE PATH "Where downloaded third-party archives are kept")
set(AROS_FETCH_OFFLINE OFF CACHE BOOL
    "Prohibit network access while fetching third-party sources")
set(AROS_FETCH_REQUIRE_CHECKSUMS OFF CACHE BOOL
    "Require every third-party archive to declare an explicit SHA-256")

if(NOT AROS_FETCH_BIN)
    find_program(AROS_FETCH_BIN NAMES aros-fetch)
endif()

set_property(GLOBAL PROPERTY AROS_FETCH_TARGETS "")

# aros_fetch_archive(NAME <t> ARCHIVE <a> SUFFIXES <s> ORIGINS <o>
#                    [CHECKSUMS <filename=sha256:digest...>]
#                    LOCATION <l> DESTINATION <d> [BASE <b>]
#                    PATCH_ORIGINS <po> PATCHES <p>
#                    [SOURCE_DIR <audited-source>
#                     LOCAL_PATCH_FILES <files...>])
#
# Declares a fetch target. The recipe mirrors the %fetch macro's invocation of
# aros-fetch. The completion stamp lives in the concrete unpack
# destination rather than the optionally shared archive cache: an archive may
# be shared by several profiles, but each profile still has to unpack and patch
# its own Ports tree.
function(aros_fetch_archive)
    set(oneValueArgs NAME ARCHIVE SUFFIXES ORIGINS CHECKSUMS LOCATION DESTINATION BASE
        PATCH_ORIGINS PATCHES SOURCE_DIR)
    set(multiValueArgs LOCAL_PATCH_FILES)
    cmake_parse_arguments(FA "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    # Empty LOCATION/BASE/SUFFIXES values are part of the legacy %fetch
    # contract, and cmake_parse_arguments reports them as missing values.
    # Unknown positional tokens are still always malformed; the audited
    # SOURCE_DIR/PATCH trio is validated independently below.
    if(FA_UNPARSED_ARGUMENTS)
        message(FATAL_ERROR "aros_fetch_archive received malformed arguments")
    endif()

    if(NOT FA_NAME OR NOT FA_ARCHIVE OR NOT FA_DESTINATION)
        return()
    endif()
    aros_path_is_executable("${AROS_FETCH_BIN}" _aros_fetch_executable)
    if(NOT _aros_fetch_executable)
        message(FATAL_ERROR
            "${FA_NAME}: required aros-fetch executable is unavailable at ${AROS_FETCH_BIN}. "
            "Run `aros build-tools build` or set AROS_FETCH_BIN explicitly.")
    endif()
    if(TARGET ${FA_NAME})
        return()
    endif()

    # The macro falls back to the unpack directory when no location is given.
    set(_loc "${FA_LOCATION}")
    if(NOT _loc)
        set(_loc "${FA_DESTINATION}")
    endif()
    set(_base "${FA_BASE}")
    if(NOT _base)
        set(_base "${FA_DESTINATION}")
    endif()
    set(_stamp "${FA_DESTINATION}/.${FA_ARCHIVE}-fetched")

    # Strict external-CMake profiles track their in-tree patches directly.
    # aros-fetch deliberately caches both the copied patch and an `.applied` marker, so a
    # plain file dependency would rerun the recipe but still use the old
    # patch.  When one of these audited inputs changes, discard only the
    # declared archive source directory and its own cache markers before
    # letting aros-fetch unpack and patch it again.
    set(_patch_refresh_commands "")
    set(_patch_dependency_args "")
    if(FA_SOURCE_DIR OR FA_LOCAL_PATCH_FILES)
        if(NOT FA_SOURCE_DIR OR NOT FA_LOCAL_PATCH_FILES)
            message(FATAL_ERROR
                "${FA_NAME}: SOURCE_DIR and LOCAL_PATCH_FILES must be declared together")
        endif()
        list(LENGTH FA_LOCAL_PATCH_FILES _local_patch_count)

        set(_destination "${FA_DESTINATION}")
        set(_source "${FA_SOURCE_DIR}")
        set(_patch_base "${_base}")
        foreach(_path_var IN ITEMS _destination _source _patch_base)
            if("${${_path_var}}" MATCHES "[;\"$\\\r\n]")
                message(FATAL_ERROR
                    "${FA_NAME}: unsafe audited fetch path '${${_path_var}}'")
            endif()
            cmake_path(ABSOLUTE_PATH ${_path_var}
                BASE_DIRECTORY "${CMAKE_BINARY_DIR}" NORMALIZE
                OUTPUT_VARIABLE ${_path_var})
        endforeach()
        cmake_path(IS_PREFIX _destination "${_source}" NORMALIZE
            _source_owned)
        if(NOT _source_owned OR _source STREQUAL _destination)
            message(FATAL_ERROR
                "${FA_NAME}: audited source must be a private child of the fetch destination: ${_source}")
        endif()
        cmake_path(RELATIVE_PATH _source BASE_DIRECTORY "${_destination}"
            OUTPUT_VARIABLE _source_relative)

        separate_arguments(_patch_specs UNIX_COMMAND "${FA_PATCHES}")
        set(_local_patch_inputs "")
        set(_cached_patch_paths "")
        set(_applied_patch_markers "")
        math(EXPR _last_local_patch "${_local_patch_count} - 1")
        foreach(_index RANGE 0 ${_last_local_patch})
            list(GET FA_LOCAL_PATCH_FILES ${_index} _raw_patch)

            if("${_raw_patch}" MATCHES "[;\"$\\\r\n]")
                message(FATAL_ERROR
                    "${FA_NAME}: unsafe local patch path '${_raw_patch}'")
            endif()
            set(_patch "${_raw_patch}")
            cmake_path(ABSOLUTE_PATH _patch
                BASE_DIRECTORY "${CMAKE_SOURCE_DIR}" NORMALIZE
                OUTPUT_VARIABLE _patch)
            cmake_path(ABSOLUTE_PATH CMAKE_SOURCE_DIR NORMALIZE
                OUTPUT_VARIABLE _source_root)
            cmake_path(IS_PREFIX _source_root "${_patch}" NORMALIZE
                _patch_owned)
            cmake_path(IS_PREFIX _destination "${_patch}" NORMALIZE
                _patch_in_destination)
            cmake_path(IS_PREFIX _patch_base "${_patch}" NORMALIZE
                _patch_in_base)
            if(NOT _patch_owned OR _patch STREQUAL _source_root OR
               _patch_in_destination OR _patch_in_base OR
               NOT EXISTS "${_patch}" OR IS_DIRECTORY "${_patch}")
                message(FATAL_ERROR
                    "${FA_NAME}: local patch is missing or outside the source tree: ${_patch}")
            endif()

            cmake_path(GET _patch FILENAME _patch_name)
            set(_matching_spec_count 0)
            foreach(_patch_spec IN LISTS _patch_specs)
                string(REPLACE ":" ";" _patch_fields "${_patch_spec}")
                list(GET _patch_fields 0 _spec_name)
                list(LENGTH _patch_fields _patch_field_count)
                if(_patch_field_count GREATER 1)
                    list(GET _patch_fields 1 _spec_subdir)
                else()
                    set(_spec_subdir "")
                endif()
                if(_spec_name STREQUAL _patch_name AND
                   _spec_subdir STREQUAL _source_relative)
                    math(EXPR _matching_spec_count "${_matching_spec_count} + 1")
                endif()
            endforeach()
            if(NOT _matching_spec_count EQUAL 1)
                message(FATAL_ERROR
                    "${FA_NAME}: local patch ${_patch_name} must have exactly one spec rooted at ${_source_relative}")
            endif()

            list(APPEND _local_patch_inputs "${_patch}")
            list(APPEND _cached_patch_paths "${_patch_base}/${_patch_name}")
            list(APPEND _applied_patch_markers
                "${_patch_base}/.${_patch_name}.applied")
        endforeach()

        separate_arguments(_archive_suffixes UNIX_COMMAND "${FA_SUFFIXES}")
        set(_archive_unpack_markers "")
        foreach(_suffix IN LISTS _archive_suffixes)
            list(APPEND _archive_unpack_markers
                "${_patch_base}/.${FA_ARCHIVE}.${_suffix}.unpacked")
        endforeach()
        list(APPEND _patch_refresh_commands
            COMMAND "${CMAKE_COMMAND}" -E rm -rf "${_source}"
            COMMAND "${CMAKE_COMMAND}" -E rm -f
                ${_archive_unpack_markers}
                ${_cached_patch_paths}
                ${_applied_patch_markers})
        foreach(_index RANGE 0 ${_last_local_patch})
            list(GET _local_patch_inputs ${_index} _patch)
            cmake_path(GET _patch FILENAME _patch_name)
            list(APPEND _patch_refresh_commands
                COMMAND "${CMAKE_COMMAND}" -E copy_if_different
                    "${_patch}" "${_patch_base}/${_patch_name}")
        endforeach()
        set(_patch_dependency_args DEPENDS
            ${_local_patch_inputs})
    endif()

    set(_fetch_policy_args "")
    if(AROS_FETCH_OFFLINE)
        list(APPEND _fetch_policy_args --offline)
    endif()
    if(AROS_FETCH_REQUIRE_CHECKSUMS)
        list(APPEND _fetch_policy_args --require-checksums)
    endif()

    add_custom_command(
        OUTPUT "${_stamp}"
        COMMAND ${CMAKE_COMMAND} -E make_directory "${_loc}"
        COMMAND ${CMAKE_COMMAND} -E make_directory "${_base}"
        COMMAND ${CMAKE_COMMAND} -E make_directory "${FA_DESTINATION}"
        ${_patch_refresh_commands}
        COMMAND "${AROS_FETCH_BIN}"
                --archive-origins "${FA_ORIGINS}"
                --archive "${FA_ARCHIVE}"
                --suffixes "${FA_SUFFIXES}"
                --checksums "${FA_CHECKSUMS}"
                --location "${_loc}"
                --destination "${FA_DESTINATION}"
                --base "${_base}"
                --patch-origins "${FA_PATCH_ORIGINS}"
                --patches "${FA_PATCHES}"
                ${_fetch_policy_args}
        COMMAND ${CMAKE_COMMAND} -E touch "${_stamp}"
        ${_patch_dependency_args}
        COMMENT "🌐 Fetching ${FA_ARCHIVE}"
        VERBATIM
    )
    add_custom_target(${FA_NAME} DEPENDS "${_stamp}")
    set_property(TARGET ${FA_NAME} PROPERTY
        AROS_FETCH_DESTINATION "${FA_DESTINATION}")
    set_property(TARGET ${FA_NAME} PROPERTY
        AROS_FETCH_COMPLETION_STAMP "${_stamp}")

    get_property(_all GLOBAL PROPERTY AROS_FETCH_TARGETS)
    list(APPEND _all "${FA_NAME}")
    set_property(GLOBAL PROPERTY AROS_FETCH_TARGETS "${_all}")
endfunction()

# aros_build_external_cmake(
#     MMAKE_ID <target>
#     SOURCE_DIR <fetched-source>
#     BINARY_DIR <private-build-dir>
#     INSTALL_PREFIX <build-tree-prefix>
#     FETCH_TARGET <fetch-owner>
#     PROVIDED_LIBRARY <uselibs-name>
#     OPTIONS <cmake-options...>
#     LIBRARY_PRODUCTS <installed-archives...>
#     [HEADER_PRODUCTS <installed-headers...>]
#     [AUXILIARY_PRODUCTS <installed-metadata...>]
#     PUBLIC_INCLUDE_DIRS <installed-include-dirs...>)
#
# Materialises the deliberately small, audited subset of
# %build_with_cmake. The transpiler proves which declaration owns the source,
# options and installed products; this helper independently validates every
# path and produces one output-tracked configure/build/install rule. It is not
# an escape hatch for arbitrary configure commands.
function(aros_build_external_cmake)
    set(oneValueArgs MMAKE_ID SOURCE_DIR BINARY_DIR INSTALL_PREFIX
        FETCH_TARGET PROVIDED_LIBRARY)
    set(multiValueArgs OPTIONS LIBRARY_PRODUCTS HEADER_PRODUCTS
        AUXILIARY_PRODUCTS PUBLIC_INCLUDE_DIRS)
    cmake_parse_arguments(PARSE_ARGV 0 EC "" "${oneValueArgs}" "${multiValueArgs}")

    if(EC_UNPARSED_ARGUMENTS OR EC_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_build_external_cmake received malformed arguments")
    endif()
    foreach(_required IN ITEMS MMAKE_ID SOURCE_DIR BINARY_DIR INSTALL_PREFIX
        FETCH_TARGET PROVIDED_LIBRARY)
        if(NOT EC_${_required})
            message(FATAL_ERROR
                "aros_build_external_cmake requires ${_required}")
        endif()
    endforeach()
    if(NOT EC_LIBRARY_PRODUCTS OR NOT EC_PUBLIC_INCLUDE_DIRS)
        message(FATAL_ERROR
            "${EC_MMAKE_ID}: external CMake library requires products and public includes")
    endif()
    foreach(_name IN ITEMS EC_MMAKE_ID EC_FETCH_TARGET EC_PROVIDED_LIBRARY)
        if(NOT "${${_name}}" MATCHES "^[A-Za-z0-9_.+-]+$")
            message(FATAL_ERROR
                "${EC_MMAKE_ID}: invalid external CMake target/library name '${${_name}}'")
        endif()
    endforeach()
    if(TARGET "${EC_MMAKE_ID}")
        message(FATAL_ERROR
            "${EC_MMAKE_ID}: external CMake target was already declared")
    endif()
    set(_interface_target
        "${EC_MMAKE_ID}-external-${EC_PROVIDED_LIBRARY}")
    if(TARGET "${_interface_target}")
        message(FATAL_ERROR
            "${EC_MMAKE_ID}: external CMake interface target already exists")
    endif()
    if(NOT TARGET "${EC_FETCH_TARGET}")
        message(FATAL_ERROR
            "${EC_MMAKE_ID}: missing fetch target ${EC_FETCH_TARGET}")
    endif()
    get_property(_fetch_destination TARGET "${EC_FETCH_TARGET}"
        PROPERTY AROS_FETCH_DESTINATION)
    get_property(_fetch_stamp TARGET "${EC_FETCH_TARGET}"
        PROPERTY AROS_FETCH_COMPLETION_STAMP)
    if(NOT _fetch_destination OR NOT _fetch_stamp)
        message(FATAL_ERROR
            "${EC_MMAKE_ID}: ${EC_FETCH_TARGET} is not a complete fetch owner")
    endif()

    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR
        NORMALIZE OUTPUT_VARIABLE _build_root)
    set(_raw_source "${EC_SOURCE_DIR}")
    set(_raw_binary "${EC_BINARY_DIR}")
    set(_raw_prefix "${EC_INSTALL_PREFIX}")
    set(_raw_fetch "${_fetch_destination}")
    foreach(_kind IN ITEMS source binary prefix fetch)
        set(_raw_path "${_raw_${_kind}}")
        string(FIND "${_raw_path}" ";" _semicolon)
        string(FIND "${_raw_path}" "$" _dollar)
        string(FIND "${_raw_path}" "\\" _backslash)
        string(FIND "${_raw_path}" "\n" _newline)
        if(NOT _semicolon EQUAL -1 OR NOT _dollar EQUAL -1 OR
           NOT _backslash EQUAL -1 OR NOT _newline EQUAL -1)
            message(FATAL_ERROR
                "${EC_MMAKE_ID}: unsafe external CMake ${_kind} path '${_raw_path}'")
        endif()
        set(_normal_path "${_raw_path}")
        cmake_path(ABSOLUTE_PATH _normal_path
            BASE_DIRECTORY "${_build_root}" NORMALIZE
            OUTPUT_VARIABLE _${_kind})
    endforeach()

    cmake_path(IS_PREFIX _fetch "${_source}" NORMALIZE _source_owned)
    if(NOT _source_owned)
        message(FATAL_ERROR
            "${EC_MMAKE_ID}: source is outside fetch destination: ${_source}")
    endif()
    cmake_path(IS_PREFIX _build_root "${_binary}" NORMALIZE _binary_owned)
    set(_external_binary_root "${_build_root}/gen/external-cmake")
    cmake_path(NORMAL_PATH _external_binary_root)
    cmake_path(IS_PREFIX _external_binary_root "${_binary}"
        NORMALIZE _binary_helper_owned)
    if(NOT _binary_owned OR NOT _binary_helper_owned OR
       _binary STREQUAL _external_binary_root)
        message(FATAL_ERROR
            "${EC_MMAKE_ID}: external binary directory must be a private child of ${_external_binary_root}: ${_binary}")
    endif()
    string(SHA256 _binary_key "${_binary}")
    get_property(_binary_owner GLOBAL PROPERTY
        "AROS_EXTERNAL_BINARY_OWNER_${_binary_key}")
    if(_binary_owner AND NOT _binary_owner STREQUAL EC_MMAKE_ID)
        message(FATAL_ERROR
            "${EC_MMAKE_ID}: external binary directory is already owned by ${_binary_owner}: ${_binary}")
    endif()
    set_property(GLOBAL PROPERTY
        "AROS_EXTERNAL_BINARY_OWNER_${_binary_key}" "${EC_MMAKE_ID}")
    cmake_path(IS_PREFIX _build_root "${_prefix}" NORMALIZE _prefix_owned)
    if(NOT _prefix_owned OR _prefix STREQUAL _build_root)
        message(FATAL_ERROR
            "${EC_MMAKE_ID}: external install prefix escapes the build tree: ${_prefix}")
    endif()
    foreach(_kind IN ITEMS source fetch prefix)
        cmake_path(IS_PREFIX _binary "${_${_kind}}"
            NORMALIZE _binary_contains_owned_path)
        cmake_path(IS_PREFIX _${_kind} "${_binary}"
            NORMALIZE _owned_path_contains_binary)
        if(_binary_contains_owned_path OR _owned_path_contains_binary)
            message(FATAL_ERROR
                "${EC_MMAKE_ID}: external binary directory overlaps ${_kind}: ${_${_kind}}")
        endif()
    endforeach()
    set(_products "")
    set(_library_products "")
    set(_header_products "")
    foreach(_kind IN ITEMS LIBRARY_PRODUCTS HEADER_PRODUCTS AUXILIARY_PRODUCTS)
        foreach(_raw_product IN LISTS EC_${_kind})
            if(_raw_product MATCHES "[;\"$\\\r\n]")
                message(FATAL_ERROR
                    "${EC_MMAKE_ID}: unsafe external product path '${_raw_product}'")
            endif()
            set(_product "${_raw_product}")
            cmake_path(ABSOLUTE_PATH _product
                BASE_DIRECTORY "${_prefix}" NORMALIZE
                OUTPUT_VARIABLE _product)
            cmake_path(IS_PREFIX _prefix "${_product}" NORMALIZE _inside_prefix)
            if(NOT _inside_prefix OR _product STREQUAL _prefix)
                message(FATAL_ERROR
                    "${EC_MMAKE_ID}: external product escapes install prefix: ${_product}")
            endif()
            list(APPEND _products "${_product}")
            if(_kind STREQUAL "LIBRARY_PRODUCTS")
                list(APPEND _library_products "${_product}")
            else()
                list(APPEND _header_products "${_product}")
            endif()
        endforeach()
    endforeach()
    list(LENGTH EC_LIBRARY_PRODUCTS _declared_library_count)
    list(LENGTH EC_HEADER_PRODUCTS _declared_header_count)
    list(LENGTH EC_AUXILIARY_PRODUCTS _declared_auxiliary_count)
    math(EXPR _declared_product_count
        "${_declared_library_count} + ${_declared_header_count} + ${_declared_auxiliary_count}")
    list(REMOVE_DUPLICATES _products)
    list(LENGTH _products _product_count)
    if(NOT _declared_product_count EQUAL _product_count)
        message(FATAL_ERROR
            "${EC_MMAKE_ID}: duplicate external product")
    endif()
    list(LENGTH _library_products _library_count)
    set(_expected_archive
        "${_prefix}/lib/${CMAKE_STATIC_LIBRARY_PREFIX}${EC_PROVIDED_LIBRARY}${CMAKE_STATIC_LIBRARY_SUFFIX}")
    cmake_path(NORMAL_PATH _expected_archive)
    if(NOT _library_count EQUAL 1 OR
       NOT _expected_archive IN_LIST _library_products)
        message(FATAL_ERROR
            "${EC_MMAKE_ID}: provided library must install exactly ${_expected_archive}")
    endif()
    foreach(_product IN LISTS _products)
        string(SHA256 _product_key "${_product}")
        get_property(_previous_owner GLOBAL PROPERTY
            "AROS_EXTERNAL_PRODUCT_OWNER_${_product_key}")
        if(_previous_owner AND NOT _previous_owner STREQUAL EC_MMAKE_ID)
            message(FATAL_ERROR
                "${EC_MMAKE_ID}: ${_product} is already owned by ${_previous_owner}")
        endif()
        set_property(GLOBAL PROPERTY
            "AROS_EXTERNAL_PRODUCT_OWNER_${_product_key}" "${EC_MMAKE_ID}")
    endforeach()
    _aros_claim_linklib_archive(
        "${EC_MMAKE_ID}" "${_prefix}/lib" "${EC_PROVIDED_LIBRARY}")

    set(_public_include_dirs "")
    foreach(_raw_include IN LISTS EC_PUBLIC_INCLUDE_DIRS)
        if(_raw_include MATCHES "[;\"$\\\r\n]")
            message(FATAL_ERROR
                "${EC_MMAKE_ID}: unsafe public include path '${_raw_include}'")
        endif()
        set(_include "${_raw_include}")
        cmake_path(ABSOLUTE_PATH _include
            BASE_DIRECTORY "${_prefix}" NORMALIZE OUTPUT_VARIABLE _include)
        cmake_path(IS_PREFIX _prefix "${_include}" NORMALIZE _inside_prefix)
        if(NOT _inside_prefix OR _include STREQUAL _prefix)
            message(FATAL_ERROR
                "${EC_MMAKE_ID}: public include directory escapes install prefix: ${_include}")
        endif()
        list(APPEND _public_include_dirs "${_include}")
    endforeach()
    list(REMOVE_DUPLICATES _public_include_dirs)

    foreach(_option IN LISTS EC_OPTIONS)
        string(FIND "${_option}" ";" _semicolon)
        string(FIND "${_option}" "\n" _newline)
        if(NOT _semicolon EQUAL -1 OR NOT _newline EQUAL -1 OR
           NOT _option MATCHES "^(-D[A-Za-z_][A-Za-z0-9_]*(:[A-Za-z]+)?=[A-Za-z0-9_.+:/=-]+|-Wno-error=dev)$")
            message(FATAL_ERROR
                "${EC_MMAKE_ID}: unsafe external CMake option '${_option}'")
        endif()
        if(_option MATCHES
           "^-DCMAKE_(INSTALL_PREFIX|TOOLCHAIN_FILE|SYSTEM_NAME|SYSTEM_PROCESSOR|C_COMPILER|CXX_COMPILER|ASM_COMPILER|AR|RANLIB|C_FLAGS|CXX_FLAGS|ASM_FLAGS|EXE_LINKER_FLAGS|TRY_COMPILE_TARGET_TYPE)(:|=)")
            message(FATAL_ERROR
                "${EC_MMAKE_ID}: external option overrides a forced toolchain setting: ${_option}")
        endif()
    endforeach()

    # Reuse the parent directory's target options/definitions so a nested
    # build selects the exact same ISA and AROS ABI. Bare-metal Clang has no
    # installed AROS specs, hence the explicit POSIX/stdc namespace order.
    get_directory_property(_parent_options COMPILE_OPTIONS)
    get_directory_property(_parent_definitions COMPILE_DEFINITIONS)
    get_directory_property(_parent_includes INCLUDE_DIRECTORIES)
    set(_target_flags "")
    foreach(_option IN LISTS _parent_options)
        if(_option STREQUAL "$<$<COMPILE_LANGUAGE:CXX>:-nostdinc++>")
            # Reapplied to the nested C++ flags below together with the
            # explicit libc++ include root.
            continue()
        endif()
        if(_option MATCHES "[;\r\n]" OR _option MATCHES "^\\$<")
            message(FATAL_ERROR
                "${EC_MMAKE_ID}: unsupported parent compile option '${_option}'")
        endif()
        list(APPEND _target_flags "${_option}")
    endforeach()
    foreach(_definition IN LISTS _parent_definitions)
        if(_definition MATCHES "[;\r\n]" OR _definition MATCHES "^\\$<")
            message(FATAL_ERROR
                "${EC_MMAKE_ID}: unsupported parent compile definition '${_definition}'")
        endif()
        list(APPEND _target_flags "-D${_definition}")
    endforeach()
    set(_external_includes
        "${AROS_SDK_INCLUDE_DIR}/aros/posixc"
        "${AROS_SDK_INCLUDE_DIR}/aros/stdc")
    list(APPEND _external_includes ${_parent_includes})
    list(REMOVE_DUPLICATES _external_includes)
    foreach(_include IN LISTS _external_includes)
        if(_include STREQUAL
           "$<$<COMPILE_LANGUAGE:CXX>:${AROS_CROSS_TOOLCHAIN_ROOT}/include/c++/v1>")
            # The nested build receives this language-specific root below.
            # It cannot be copied into the common C/CXX/ASM flag list.
            continue()
        endif()
        if(_include MATCHES "[;\r\n]" OR _include MATCHES "^\\$<")
            message(FATAL_ERROR
                "${EC_MMAKE_ID}: unsupported parent include '${_include}'")
        endif()
        # CMAKE_<LANG>_FLAGS is a command-line string rather than a CMake
        # argument list. Preserve an include root containing whitespace when
        # the nested generator parses that string into compiler arguments.
        list(APPEND _target_flags "-I\"${_include}\"")
    endforeach()
    string(JOIN " " _target_flags_string ${_target_flags})
    foreach(_language IN ITEMS C CXX ASM)
        set(_parent_language_flags "${CMAKE_${_language}_FLAGS}")
        if(_parent_language_flags MATCHES "[;\r\n]")
            message(FATAL_ERROR
                "${EC_MMAKE_ID}: unsafe parent ${_language} flags")
        endif()
        string(STRIP
            "${_parent_language_flags} ${_target_flags_string}"
            _${_language}_flags)
    endforeach()
    if(AROS_CROSS_TOOLCHAIN_ROOT)
        # libc++'s C-compatible wrappers must be searched before the AROS SDK
        # in nested C++ builds for the same reason as in the parent graph.
        string(PREPEND _CXX_flags
            "-nostdinc++ -I\"${AROS_CROSS_TOOLCHAIN_ROOT}/include/c++/v1\" ")
    endif()

    cmake_path(GET CMAKE_CURRENT_FUNCTION_LIST_DIR PARENT_PATH
        _aros_source_root)
    set(_output_verifier
        "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/VerifyOutputs.cmake")

    # Ninja and Make both trust a successful custom command even if it failed
    # to create one of its declared outputs. Verify the complete install
    # contract before writing the success stamp. file(GENERATE) preserves the
    # manifest timestamp across no-op reconfigures.
    set(_products_manifest
        "${CMAKE_CURRENT_BINARY_DIR}/.aros-${EC_MMAKE_ID}-products.cmake")
    set(_products_manifest_content "set(EXPECTED_OUTPUTS\n")
    foreach(_product IN LISTS _products)
        string(APPEND _products_manifest_content "    \"${_product}\"\n")
    endforeach()
    string(APPEND _products_manifest_content ")\n")
    file(GENERATE OUTPUT "${_products_manifest}"
        CONTENT "${_products_manifest_content}")

    set(_forced_options
        "-DCMAKE_SYSTEM_NAME=AROS"
        "-DCMAKE_SYSTEM_VERSION=1"
        "-DCMAKE_SYSTEM_PROCESSOR=${AROS_TARGET_CPU}"
        "-DCMAKE_MODULE_PATH=${_aros_source_root}/config/cmake"
        "-DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY"
        "-DBUILD_SHARED_LIBS=OFF"
        "-DCMAKE_INSTALL_PREFIX=${_prefix}"
        "-DCMAKE_C_COMPILER=${CMAKE_C_COMPILER}"
        "-DCMAKE_CXX_COMPILER=${CMAKE_CXX_COMPILER}"
        "-DCMAKE_ASM_COMPILER=${CMAKE_ASM_COMPILER}"
        "-DCMAKE_AR=${CMAKE_AR}"
        "-DCMAKE_RANLIB=${CMAKE_RANLIB}"
        "-DCMAKE_C_FLAGS=${_C_flags}"
        "-DCMAKE_CXX_FLAGS=${_CXX_flags}"
        "-DCMAKE_ASM_FLAGS=${_ASM_flags}")
    if(CMAKE_C_COMPILER_LAUNCHER)
        list(APPEND _forced_options
            "-DCMAKE_C_COMPILER_LAUNCHER=${CMAKE_C_COMPILER_LAUNCHER}")
    endif()
    if(CMAKE_CXX_COMPILER_LAUNCHER)
        list(APPEND _forced_options
            "-DCMAKE_CXX_COMPILER_LAUNCHER=${CMAKE_CXX_COMPILER_LAUNCHER}")
    endif()

    set(_stamp "${_binary}/.aros-${EC_MMAKE_ID}-installed")
    add_custom_command(
        OUTPUT "${_stamp}" ${_products}
        # The rule runs only when an input/contract changed or a product is
        # missing. A clean private cache prevents a source-version change or
        # removed option from surviving in the nested CMakeCache.txt.
        COMMAND "${CMAKE_COMMAND}" -E rm -rf "${_binary}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_binary}" "${_prefix}"
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_binary}"
            -G "${CMAKE_GENERATOR}"
            ${EC_OPTIONS}
            ${_forced_options}
        COMMAND "${CMAKE_COMMAND}" --build "${_binary}"
        COMMAND "${CMAKE_COMMAND}" --install "${_binary}"
        COMMAND "${CMAKE_COMMAND}"
            "-DMANIFEST=${_products_manifest}"
            -P "${_output_verifier}"
        COMMAND "${CMAKE_COMMAND}" -E touch "${_stamp}"
        DEPENDS "${_fetch_stamp}" "${_output_verifier}"
            "${_products_manifest}"
        COMMENT "Building external CMake target ${EC_MMAKE_ID}"
        VERBATIM
        COMMAND_EXPAND_LISTS)
    add_custom_target("${EC_MMAKE_ID}"
        DEPENDS "${_stamp}" ${_products})

    add_library("${_interface_target}" INTERFACE)
    add_dependencies("${_interface_target}" "${EC_MMAKE_ID}")
    target_link_libraries("${_interface_target}" INTERFACE "${_expected_archive}")
    target_include_directories("${_interface_target}" INTERFACE
        ${_public_include_dirs})
    set_property(TARGET "${EC_MMAKE_ID}" PROPERTY
        AROS_EXTERNAL_INTERFACE_TARGET "${_interface_target}")
endfunction()

# aros_generate_defines_header(OWNER <mmake> OUTPUT <file>
#                              DEFINES "<identifier> <literal>"...
#                              [DEPENDS <make-source-files...>]
#                              [CONSUMERS <compile-targets...>])
#
# Materialises the deliberately narrow literal `echo "#define ..."` recipe
# shape accepted by the transpiler. The output is always a build product below
# CMAKE_BINARY_DIR; no configure-time placeholder is created.
function(aros_generate_defines_header)
    set(oneValueArgs OWNER OUTPUT)
    set(multiValueArgs DEFINES DEPENDS CONSUMERS)
    cmake_parse_arguments(DH "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(DH_UNPARSED_ARGUMENTS OR DH_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_generate_defines_header received malformed arguments")
    endif()
    if(NOT DH_OWNER OR NOT DH_OUTPUT OR NOT DH_DEFINES)
        message(FATAL_ERROR
            "aros_generate_defines_header requires OWNER, OUTPUT and DEFINES")
    endif()
    if(TARGET "${DH_OWNER}")
        message(FATAL_ERROR
            "aros_generate_defines_header owner '${DH_OWNER}' was already declared")
    endif()

    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR
        NORMALIZE OUTPUT_VARIABLE _binary_root)
    cmake_path(ABSOLUTE_PATH DH_OUTPUT
        BASE_DIRECTORY "${_binary_root}" NORMALIZE OUTPUT_VARIABLE _output)
    cmake_path(IS_PREFIX _binary_root "${_output}" NORMALIZE _inside_build)
    if(NOT _inside_build OR _output STREQUAL _binary_root)
        message(FATAL_ERROR
            "${DH_OWNER}: defines-header output escapes the build tree: ${_output}")
    endif()

    set(_define_names "")
    foreach(_definition IN LISTS DH_DEFINES)
        if(NOT _definition MATCHES
           "^([A-Za-z_][A-Za-z0-9_]*) ([A-Za-z0-9_+.,:/<>=!&|%*~?@#^()-]+)$")
            message(FATAL_ERROR
                "${DH_OWNER}: invalid literal define payload: '${_definition}'")
        endif()
        set(_define_name "${CMAKE_MATCH_1}")
        if(_define_name IN_LIST _define_names)
            message(FATAL_ERROR
                "${DH_OWNER}: duplicate literal define: ${_define_name}")
        endif()
        list(APPEND _define_names "${_define_name}")
    endforeach()

    string(SHA256 _output_key "${_output}")
    get_property(_previous_owner GLOBAL PROPERTY
        "AROS_DEFINE_HEADER_OWNER_${_output_key}")
    if(_previous_owner AND NOT _previous_owner STREQUAL DH_OWNER)
        message(FATAL_ERROR
            "${DH_OWNER}: ${_output} is already owned by ${_previous_owner}")
    endif()
    set_property(GLOBAL PROPERTY
        "AROS_DEFINE_HEADER_OWNER_${_output_key}" "${DH_OWNER}")

    set(_dependencies "")
    foreach(_dependency IN LISTS DH_DEPENDS)
        cmake_path(ABSOLUTE_PATH _dependency
            BASE_DIRECTORY "${CMAKE_SOURCE_DIR}"
            NORMALIZE OUTPUT_VARIABLE _dependency_abs)
        if(NOT EXISTS "${_dependency_abs}" OR IS_DIRECTORY "${_dependency_abs}")
            message(FATAL_ERROR
                "${DH_OWNER}: missing defines-header dependency ${_dependency_abs}")
        endif()
        list(APPEND _dependencies "${_dependency_abs}")
    endforeach()
    list(REMOVE_DUPLICATES _dependencies)
    if(_dependencies)
        set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
            ${_dependencies})
    endif()

    set(_writer "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/WriteDefinesHeader.cmake")
    get_filename_component(_output_dir "${_output}" DIRECTORY)
    add_custom_command(
        OUTPUT "${_output}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
        COMMAND "${CMAKE_COMMAND}"
            "-DBINARY_ROOT=${_binary_root}"
            "-DOUTPUT=${_output}"
            "-DDEFINES=${DH_DEFINES}"
            -P "${_writer}"
        # The standalone writer preserves mtime when contents are identical.
        # A build rule must nevertheless make its declared OUTPUT newer than
        # a changed dependency, otherwise Makefile generators rerun forever.
        COMMAND "${CMAKE_COMMAND}" -E touch "${_output}"
        DEPENDS "${_writer}" ${_dependencies}
        COMMENT "Generating literal defines header ${_output}"
        VERBATIM)
    add_custom_target("${DH_OWNER}" DEPENDS "${_output}")

    list(REMOVE_DUPLICATES DH_CONSUMERS)
    foreach(_consumer IN LISTS DH_CONSUMERS)
        if(NOT TARGET "${_consumer}")
            message(FATAL_ERROR
                "${DH_OWNER}: missing defines-header consumer ${_consumer}")
        endif()
        get_target_property(_consumer_type "${_consumer}" TYPE)
        if(_consumer_type STREQUAL "UTILITY")
            message(FATAL_ERROR
                "${DH_OWNER}: defines-header consumer ${_consumer} does not compile")
        endif()
        if(NOT _consumer STREQUAL DH_OWNER)
            add_dependencies("${_consumer}" "${DH_OWNER}")
        endif()
        target_include_directories("${_consumer}" BEFORE PRIVATE "${_output_dir}")
    endforeach()
endfunction()

function(aros_generate_bison_output)
    set(oneValueArgs OWNER INPUT OUTPUT)
    cmake_parse_arguments(BO "" "${oneValueArgs}" "" ${ARGN})
    if(NOT BO_OWNER OR NOT BO_INPUT OR NOT BO_OUTPUT)
        message(FATAL_ERROR
            "aros_generate_bison_output requires OWNER, INPUT and OUTPUT")
    endif()
    if(NOT TARGET "${BO_OWNER}")
        message(FATAL_ERROR
            "${BO_OWNER}: Bison output consumer target does not exist")
    endif()
    cmake_path(ABSOLUTE_PATH BO_INPUT NORMALIZE OUTPUT_VARIABLE _input)
    cmake_path(ABSOLUTE_PATH BO_OUTPUT NORMALIZE OUTPUT_VARIABLE _output)
    cmake_path(ABSOLUTE_PATH CMAKE_SOURCE_DIR NORMALIZE OUTPUT_VARIABLE _source_root)
    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR NORMALIZE OUTPUT_VARIABLE _build_root)
    cmake_path(IS_PREFIX _source_root "${_input}" NORMALIZE _input_allowed)
    cmake_path(IS_PREFIX _build_root "${_output}" NORMALIZE _output_allowed)
    if(NOT _input_allowed OR NOT _output_allowed OR
       NOT _input MATCHES "\\.y$" OR NOT _output MATCHES "\\.c$")
        message(FATAL_ERROR
            "${BO_OWNER}: unsafe Bison input/output contract: ${_input} -> ${_output}")
    endif()

    find_program(_aros_bison NAMES bison
        PATHS /opt/homebrew/opt/bison/bin /usr/local/opt/bison/bin
        NO_DEFAULT_PATH)
    if(NOT _aros_bison)
        find_program(_aros_bison NAMES bison)
    endif()
    if(NOT _aros_bison)
        message(FATAL_ERROR
            "${BO_OWNER}: the exact MetaMake recipe requires a host Bison executable")
    endif()
    execute_process(
        COMMAND "${_aros_bison}" --version
        RESULT_VARIABLE _bison_result
        OUTPUT_VARIABLE _bison_version
        ERROR_VARIABLE _bison_error
        OUTPUT_STRIP_TRAILING_WHITESPACE
        TIMEOUT 10)
    if(NOT _bison_result EQUAL 0)
        message(FATAL_ERROR
            "${BO_OWNER}: host Bison is not executable: ${_bison_error}")
    endif()

    get_filename_component(_output_dir "${_output}" DIRECTORY)
    string(SHA256 _output_key "${_output}")
    string(SUBSTRING "${_output_key}" 0 16 _output_suffix)
    set(_product_owner "${BO_OWNER}--bison-${_output_suffix}")
    add_custom_command(
        OUTPUT "${_output}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
        COMMAND "${_aros_bison}" -o "${_output}" "${_input}"
        DEPENDS "${_input}"
        COMMENT "Generating ${_output} with ${_bison_version}"
        VERBATIM)
    add_custom_target("${_product_owner}" DEPENDS "${_output}")
    add_dependencies("${BO_OWNER}" "${_product_owner}")
    target_include_directories("${BO_OWNER}" BEFORE PRIVATE "${_output_dir}")
endfunction()

# aros_transform_header(NAME <mmake> INPUT <file> OUTPUT <file>
#                       [COPY_ONLY | MATCH <literal> REPLACEMENT <literal> |
#                        SUBSTITUTIONS <token replacement...>]
#                       [DEPENDS <fetch-targets...>]
#                       [CONSUMERS <compile-targets...>])
#
# Materialises the deliberately narrow hand-written Make-recipe subset the
# transpiler can prove safe: a line-anchored literal sed substitution.  The
# output is a normal Ninja product, never a configure-time placeholder.
function(aros_transform_header)
    set(oneValueArgs NAME INPUT OUTPUT MATCH REPLACEMENT)
    set(multiValueArgs DEPENDS CONSUMERS SUBSTITUTIONS)
    cmake_parse_arguments(TH "COPY_ONLY" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(NOT TH_NAME OR NOT TH_INPUT OR NOT TH_OUTPUT OR
       (NOT TH_COPY_ONLY AND NOT TH_MATCH AND NOT TH_SUBSTITUTIONS))
        message(FATAL_ERROR
            "aros_transform_header requires NAME, INPUT, OUTPUT and a safe operation")
    endif()
    string(SHA256 _owner_key "${TH_NAME}")
    get_property(_aggregate_owner GLOBAL PROPERTY
        "AROS_TRANSFORM_HEADER_AGGREGATE_${_owner_key}")
    if(TARGET "${TH_NAME}" AND
       NOT "${_aggregate_owner}" STREQUAL "${TH_NAME}")
        message(FATAL_ERROR
            "aros_transform_header owner '${TH_NAME}' was already declared")
    elseif(NOT TARGET "${TH_NAME}")
        add_custom_target("${TH_NAME}")
        set_property(GLOBAL PROPERTY
            "AROS_TRANSFORM_HEADER_AGGREGATE_${_owner_key}" "${TH_NAME}")
    endif()

    cmake_path(ABSOLUTE_PATH TH_INPUT NORMALIZE OUTPUT_VARIABLE _input)
    cmake_path(ABSOLUTE_PATH TH_OUTPUT NORMALIZE OUTPUT_VARIABLE _output)
    set(_output_allowed FALSE)
    foreach(_root IN ITEMS
            "${AROS_SDK_INCLUDE_DIR}"
            "${AROS_GENINC_DIR}"
            "${CMAKE_BINARY_DIR}/gen")
        cmake_path(ABSOLUTE_PATH _root NORMALIZE OUTPUT_VARIABLE _allowed_root)
        cmake_path(IS_PREFIX _allowed_root "${_output}" NORMALIZE _inside)
        if(_inside)
            set(_output_allowed TRUE)
        endif()
    endforeach()
    if(NOT _output_allowed)
        message(FATAL_ERROR
            "${TH_NAME}: transformed header output escapes generated roots: ${_output}")
    endif()

    set(_input_allowed FALSE)
    foreach(_root IN ITEMS "${CMAKE_SOURCE_DIR}" "${CMAKE_BINARY_DIR}")
        cmake_path(ABSOLUTE_PATH _root NORMALIZE OUTPUT_VARIABLE _allowed_root)
        cmake_path(IS_PREFIX _allowed_root "${_input}" NORMALIZE _inside)
        if(_inside)
            set(_input_allowed TRUE)
        endif()
    endforeach()
    if(NOT _input_allowed OR _input STREQUAL _output)
        message(FATAL_ERROR
            "${TH_NAME}: invalid transformed header input: ${_input}")
    endif()

    string(SHA256 _output_key "${_output}")
    get_property(_previous_owner GLOBAL PROPERTY
        "AROS_TRANSFORM_HEADER_OWNER_${_output_key}")
    if(_previous_owner AND NOT _previous_owner STREQUAL TH_NAME)
        message(FATAL_ERROR
            "${TH_NAME}: ${_output} is already owned by ${_previous_owner}")
    endif()
    set_property(GLOBAL PROPERTY
        "AROS_TRANSFORM_HEADER_OWNER_${_output_key}" "${TH_NAME}")

    set(_dep_files "")
    if(TH_SUBSTITUTIONS)
        list(APPEND _dep_files "${CMAKE_SOURCE_DIR}/cmake/SubstituteHeader.cmake")
    elseif(NOT TH_COPY_ONLY)
        list(APPEND _dep_files "${CMAKE_SOURCE_DIR}/cmake/TransformHeader.cmake")
    endif()
    set(_input_fetch_owner "")
    foreach(_dependency IN LISTS TH_DEPENDS)
        if(NOT TARGET "${_dependency}")
            message(FATAL_ERROR
                "${TH_NAME}: missing transform dependency ${_dependency}")
        endif()
        get_property(_fetch_destination TARGET "${_dependency}"
            PROPERTY AROS_FETCH_DESTINATION)
        get_property(_fetch_stamp TARGET "${_dependency}"
            PROPERTY AROS_FETCH_COMPLETION_STAMP)
        if(_fetch_stamp)
            list(APPEND _dep_files "${_fetch_stamp}")
            if(_fetch_destination)
                cmake_path(ABSOLUTE_PATH _fetch_destination NORMALIZE
                    OUTPUT_VARIABLE _fetch_destination)
                cmake_path(IS_PREFIX _fetch_destination "${_input}" NORMALIZE
                    _input_below_fetch)
                if(_input_below_fetch)
                    set(_input_fetch_owner "${_dependency}")
                endif()
            endif()
        else()
            list(APPEND _dep_files "${_dependency}")
        endif()
    endforeach()
    if(_input_fetch_owner)
        # A source below a fetch destination does not exist in a clean build
        # tree. Naming it as a Ninja file prerequisite would fail graph
        # validation before the fetch stamp gets a chance to materialise it.
        # The content-locked completion stamp is the dependency contract.
    elseif(EXISTS "${_input}")
        list(APPEND _dep_files "${_input}")
    else()
        message(FATAL_ERROR
            "${TH_NAME}: transform input ${_input} does not exist and is not "
            "owned by a declared fetch dependency")
    endif()

    get_filename_component(_output_dir "${_output}" DIRECTORY)
    if(TH_COPY_ONLY)
        add_custom_command(
            OUTPUT "${_output}"
            COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
            COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${_input}" "${_output}"
            DEPENDS ${_dep_files}
            COMMENT "Copying generated header ${_output}"
            VERBATIM)
    elseif(TH_SUBSTITUTIONS)
        list(LENGTH TH_SUBSTITUTIONS _substitution_count)
        math(EXPR _substitution_remainder "${_substitution_count} % 2")
        if(_substitution_remainder OR _substitution_count LESS 2)
            message(FATAL_ERROR
                "${TH_NAME}: template substitutions must be token/replacement pairs")
        endif()
        add_custom_command(
            OUTPUT "${_output}"
            COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
            COMMAND "${CMAKE_COMMAND}"
                "-DINPUT=${_input}"
                "-DOUTPUT=${_output}"
                "-DSUBSTITUTIONS=${TH_SUBSTITUTIONS}"
                -P "${CMAKE_SOURCE_DIR}/cmake/SubstituteHeader.cmake"
            DEPENDS ${_dep_files}
            COMMENT "Substituting generated header ${_output}"
            VERBATIM)
    else()
        add_custom_command(
            OUTPUT "${_output}"
            COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
            COMMAND "${CMAKE_COMMAND}"
                "-DINPUT=${_input}"
                "-DOUTPUT=${_output}"
                "-DMATCH_TEXT=${TH_MATCH}"
                "-DREPLACEMENT=${TH_REPLACEMENT}"
                -P "${CMAKE_SOURCE_DIR}/cmake/TransformHeader.cmake"
            DEPENDS ${_dep_files}
            COMMENT "Generating transformed header ${_output}"
            VERBATIM)
    endif()
    string(SUBSTRING "${_output_key}" 0 16 _output_suffix)
    set(_output_owner "${TH_NAME}--header-${_output_suffix}")
    if(TARGET "${_output_owner}")
        message(FATAL_ERROR
            "${TH_NAME}: duplicate transformed-header product target ${_output_owner}")
    endif()
    add_custom_target("${_output_owner}" DEPENDS "${_output}")
    add_dependencies("${TH_NAME}" "${_output_owner}")

    foreach(_consumer IN LISTS TH_CONSUMERS)
        if(NOT TARGET "${_consumer}")
            message(FATAL_ERROR
                "${TH_NAME}: missing transformed-header consumer ${_consumer}")
        endif()
        if(NOT _consumer STREQUAL TH_NAME)
            add_dependencies("${_consumer}" "${TH_NAME}")
        endif()
    endforeach()
endfunction()

# aros_mark_preprocessed_asm(<sources...>)
#
# AROS assembly sources use preprocessor directives regardless of case, e.g.
# arch/x86_64-all/exec/execstubs.s opens with `#define PUSH ...`. CMake hands
# `.s` to the assembler without running the preprocessor, so those files must
# be compiled as `assembler-with-cpp` explicitly. `.S` already implies it.
function(aros_mark_preprocessed_asm)
    foreach(src ${ARGN})
        if(src MATCHES "\\.[sS]$")
            set_source_files_properties("${src}" PROPERTIES
                LANGUAGE ASM
                COMPILE_OPTIONS "-x;assembler-with-cpp"
            )
        endif()
    endforeach()
endfunction()

# aros_resolve_arch_sources(<out_sources> <out_dropped> <module_dir>
#                           SOURCES <names...> ARCH_SOURCES <tag|dir|files...>)
#
# Applies architecture-specific source overrides. For every declaration whose
# tag applies to this target, the named files are taken from the architecture
# directory and the same-named generic sources are dropped.
#
# This mirrors config/make.tmpl:1661, where the generic list is filtered against
# the architecture object names. Overriding by base name is what lets
# arch/x86_64-all/exec/stackswap.S replace rom/exec/stackswap.c, whose generic
# body is only an `#error`.
# aros_set_arch_source_options(<tag>|<dir>|<file>|<option> ...)
#
# Codegen options that belong to one architecture lane's own sources.
#
# Per file, not per target. `arch/i386-all/hidd/gfx` compiles rgbconv_sse.c with
# -msse2 and rgbconv_avx.c with -mavx2, and the baseline dispatcher beside them
# must stay baseline ISA -- its own comment says so, because it is the code that
# always runs and decides at runtime which implementation to install. Applying
# either flag to the whole target would defeat that. And once a lane is attached
# to another lane the tag no longer separates them, so the file has to.
function(aros_set_arch_source_options)
    set_property(GLOBAL PROPERTY AROS_ARCH_SOURCE_OPTIONS "${ARGN}")
endfunction()

# _aros_apply_arch_source_options(<tag> <dir> <name> <resolved-path>)
function(_aros_apply_arch_source_options tag dir name path)
    get_property(_entries GLOBAL PROPERTY AROS_ARCH_SOURCE_OPTIONS)
    set(_options "")
    foreach(_entry IN LISTS _entries)
        string(REPLACE "|" ";" _parts "${_entry}")
        list(LENGTH _parts _n)
        if(NOT _n EQUAL 4)
            continue()
        endif()
        list(GET _parts 0 _tag)
        list(GET _parts 1 _dir)
        list(GET _parts 2 _file)
        list(GET _parts 3 _option)
        if(_tag STREQUAL tag AND _dir STREQUAL dir AND _file STREQUAL name)
            list(APPEND _options "${_option}")
        endif()
    endforeach()
    if(NOT _options)
        return()
    endif()
    list(REMOVE_DUPLICATES _options)
    get_source_file_property(_existing "${path}" COMPILE_OPTIONS)
    if(_existing AND NOT _existing STREQUAL "NOTFOUND")
        list(APPEND _options ${_existing})
        list(REMOVE_DUPLICATES _options)
    endif()
    set_source_files_properties("${path}" PROPERTIES
        COMPILE_OPTIONS "${_options}")
endfunction()

function(aros_resolve_arch_sources out_sources out_dropped module_dir)
    set(multiValueArgs SOURCES ARCH_SOURCES)
    cmake_parse_arguments(AS "" "" "${multiValueArgs}" ${ARGN})

    set(OVERRIDE_NAMES "")
    set(ARCH_FILES "")
    set(CLAIMED_NAMES "")
    set(DECLARED_NAMES "")

    # Two architecture directories can declare the same file, and both tags can
    # apply: for raspi-aarch64, arch/aarch64-all/exec (arch=aarch64) and
    # arch/aarch64-native/exec (arch=raspi-aarch64) both provide cachecleare and
    # preparecontext. Taking both yields duplicate symbols at link time.
    #
    # Walk the tags most specific first -- that is the order
    # AROS_ARCH_INCLUDE_TAGS is built in -- and let the first declaration to
    # claim a base name keep it.
    foreach(want_tag IN LISTS AROS_ARCH_INCLUDE_TAGS)
        foreach(entry IN LISTS AS_ARCH_SOURCES)
            # "<tag>|<dir>|<f1>,<f2>,..."
            string(REPLACE "|" ";" parts "${entry}")
            list(LENGTH parts n)
            if(NOT n EQUAL 3)
                continue()
            endif()
            list(GET parts 0 tag)
            list(GET parts 1 dir)
            list(GET parts 2 names)

            if(NOT tag STREQUAL want_tag)
                continue()
            endif()

            set(abs_dir "${CMAKE_SOURCE_DIR}/${dir}")
            if(NOT IS_DIRECTORY "${abs_dir}")
                continue()
            endif()

            string(REPLACE "," ";" name_list "${names}")
            foreach(nm IN LISTS name_list)
                list(APPEND DECLARED_NAMES "${nm}")
                if(nm IN_LIST CLAIMED_NAMES)
                    continue()
                endif()
                aros_resolve_sources(RESOLVED "${abs_dir}" "${nm}")
                # Claim, and override the generic source, only when this
                # directory really provides the file.
                #
                # config/make.tmpl:2919 filters the generic list against
                # $(basename $(notdir $(wildcard $(OBJDIR)/arch/*.o))), so a
                # name that produced no arch object overrides nothing. Claiming
                # on declaration instead dropped the file entirely whenever a
                # more specific declaration named it without holding it, and
                # two arch declarations of one target share that object
                # directory precisely so they can do that:
                # arch/x86_64-pc/kernel names kernel_bootmem, the file lives in
                # arch/all-native/kernel, and rom/kernel lost krnAllocBootMem.
                if(NOT RESOLVED)
                    continue()
                endif()
                foreach(_resolved_path IN LISTS RESOLVED)
                    _aros_apply_arch_source_options(
                        "${tag}" "${dir}" "${nm}" "${_resolved_path}")
                endforeach()
                list(APPEND CLAIMED_NAMES "${nm}")
                list(APPEND OVERRIDE_NAMES "${nm}")
                foreach(f IN LISTS RESOLVED)
                    list(APPEND ARCH_FILES "${f}")
                endforeach()
            endforeach()
        endforeach()
    endforeach()

    # A name every applicable declaration asked for and none could provide is
    # a real gap: neither an arch file nor, if the generic list has one, a
    # reason to keep believing the generic one is unwanted.
    if(DECLARED_NAMES)
        list(REMOVE_DUPLICATES DECLARED_NAMES)
        foreach(nm IN LISTS DECLARED_NAMES)
            if(NOT nm IN_LIST CLAIMED_NAMES)
                set_property(GLOBAL APPEND PROPERTY AROS_ARCH_OVERRIDE_GAPS
                    "${module_dir}: ${nm}")
            endif()
        endforeach()
    endif()

    if(NOT OVERRIDE_NAMES)
        set(${out_sources} "" PARENT_SCOPE)
        set(${out_dropped} "" PARENT_SCOPE)
        return()
    endif()
    list(REMOVE_DUPLICATES OVERRIDE_NAMES)

    set(KEPT "")
    set(DROPPED "")
    foreach(src IN LISTS AS_SOURCES)
        get_filename_component(base "${src}" NAME_WE)
        if(base IN_LIST OVERRIDE_NAMES)
            list(APPEND DROPPED "${base}")
        else()
            list(APPEND KEPT "${src}")
        endif()
    endforeach()

    # Architecture objects come first in the link, as in the reference build.
    set(${out_sources} "${ARCH_FILES};${KEPT}" PARENT_SCOPE)
    set(${out_dropped} "${DROPPED}" PARENT_SCOPE)
endfunction()

# aros_report_arch_override_gaps()
#
# Names an architecture declaration asked for that no applicable architecture
# directory holds. Each one is a source the reference build compiles and we do
# not, so it is written out rather than left implicit.
function(aros_report_arch_override_gaps)
    get_property(_gaps GLOBAL PROPERTY AROS_ARCH_OVERRIDE_GAPS)
    set(_report "${CMAKE_BINARY_DIR}/generated_targets.arch-override-gaps.txt")
    if(NOT _gaps)
        file(REMOVE "${_report}")
        return()
    endif()
    list(REMOVE_DUPLICATES _gaps)
    list(SORT _gaps)
    string(REPLACE ";" "\n" _body "${_gaps}")
    file(WRITE "${_report}" "${_body}\n")
    list(LENGTH _gaps _count)
    message(STATUS
        "⚠️  ${_count} architecture source override(s) name a file no applicable "
        "arch directory holds -> ${_report}")
endfunction()

# _aros_module_install_dir(<out-var> <default-dir> <requested-dir>)
#
# Resolves an optional moduledir= override.  build_module_core prefixes a
# relative moduledir with $(AROSDIR) (make.tmpl:2661), while values derived from
# variables such as $(AROS_DEVS) are already absolute.  Generated CMake uses the
# same contract through the public INSTALL_DIR argument on each module builder.
function(_aros_module_install_dir out_var default_dir requested_dir)
    if(requested_dir)
        if(IS_ABSOLUTE "${requested_dir}")
            set(_result "${requested_dir}")
        else()
            set(_result "${AROS_SYS_DIR}/${requested_dir}")
        endif()
    else()
        set(_result "${default_dir}")
    endif()
    cmake_path(NORMAL_PATH _result)
    set(${out_var} "${_result}" PARENT_SCOPE)
endfunction()

# _aros_module_output_name(<out-var> <base-name> <default-suffix>
#                          <requested-suffix>)
#
# MODSUFFIX in %build_module replaces the module type as the runtime suffix.
# A handler is the sole dash-separated form. An empty effective suffix is used
# by %build_module_simple's printer type, whose runtime file has no extension.
function(_aros_module_output_name out_var base_name default_suffix requested_suffix)
    if(requested_suffix)
        set(_suffix "${requested_suffix}")
    else()
        set(_suffix "${default_suffix}")
    endif()

    if(_suffix STREQUAL "handler")
        set(_result "${base_name}-handler")
    elseif(_suffix)
        set(_result "${base_name}.${_suffix}")
    else()
        set(_result "${base_name}")
    endif()
    set(${out_var} "${_result}" PARENT_SCOPE)
endfunction()

# _aros_attach_genmodule_public_includes()
#
# Public genmodule headers live in a shared namespace.  A direct Ninja build of
# a consumer must not race the declaration which owns that namespace, notably
# for arosx: the ABI-only library config and the usbclass implementation config
# describe different APIs under the same name.  Once all generated targets are
# known, attach the authoritative header barrier to every compiled target.
function(_aros_attach_genmodule_public_includes)
    if(NOT TARGET aros-genmodule-public-includes)
        return()
    endif()

    get_property(_targets DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)
    foreach(_target IN LISTS _targets)
        if(_target STREQUAL "aros-genmodule-public-includes")
            continue()
        endif()
        get_target_property(_type "${_target}" TYPE)
        if(_type MATCHES "^(EXECUTABLE|STATIC_LIBRARY|SHARED_LIBRARY|MODULE_LIBRARY|OBJECT_LIBRARY)$")
            add_dependencies("${_target}" aros-genmodule-public-includes)
        endif()
    endforeach()
endfunction()

function(_aros_register_genmodule_public_includes include_target)
    if(NOT TARGET aros-genmodule-public-includes)
        add_custom_target(aros-genmodule-public-includes)
    endif()
    add_dependencies(aros-genmodule-public-includes "${include_target}")

    get_property(_scheduled GLOBAL PROPERTY AROS_GENMODULE_INCLUDE_BARRIER_SCHEDULED)
    if(NOT _scheduled)
        set_property(GLOBAL PROPERTY AROS_GENMODULE_INCLUDE_BARRIER_SCHEDULED TRUE)
        cmake_language(DEFER CALL _aros_attach_genmodule_public_includes)
    endif()
endfunction()

# _aros_generate_module_support(<prefix>
#     TARGET <module-name> MMAKE_ID <target-id> DIRECTORY <source-dir>
#     MODTYPE <type> [MODSUFFIX <suffix>] [ABI])
#
# Runs the reference tools/genmodule against one exact .conf.  All generation is
# declaration-private first.  Public headers are then copied into the three
# include roots the CMake build and the system image expose.  Keeping the
# private root is essential for duplicate config stems: the global Rust scan is
# intentionally broad and currently lets rom/usb/classes/arosx/arosx.conf race
# rom/usb/classes/arosx/include/arosx.conf for the same SDK files.
# Every modtype tools/genmodule/config.c:249 accepts. Kept as a list so a
# misspelling still fails here rather than inside the generator.
set(AROS_GENMODULE_MODTYPES
    library class mcc mui mcp device resource gadget image datatype
    usbclass btclass hidd handler hook)

# aros_set_module_config(<mmake-id> <config-path>)
#
# The `.conf` a declaration names with `conffile=`. 81 of the 83 declarations
# that state one give a file whose stem is not modname -- con_handler.conf for
# modname=con, gauge.conf for modname=Gauge, VMM_Handler.conf for modname=VMM --
# and both lookups below used to derive `<modname>.conf` and find nothing. For
# con-handler that meant no genmodule scaffolding at all, so con_handler.c's
# `extern const char GM_UNIQUENAME(LibName)[]` stayed undefined and the ELF
# loader refused the whole boot over it.
#
# Kept in a global keyed by the declaration id rather than threaded through
# every module builder's argument list: the two places that need it both have
# the id, and eight builder signatures do not have to learn a field they only
# forward.
function(aros_set_module_config mmake config)
    string(MAKE_C_IDENTIFIER "${mmake}" _key)
    set_property(GLOBAL PROPERTY "AROS_MODULE_CONFIG_${_key}" "${config}")
endfunction()

# _aros_module_config(<out-var> <mmake-id> <directory> <target>)
#
# The declaration's own config when it named one, and `<target>.conf` in the
# module directory otherwise, which is what a declaration without `conffile=`
# means.
function(_aros_module_config out_var mmake directory target)
    string(MAKE_C_IDENTIFIER "${mmake}" _key)
    get_property(_explicit GLOBAL PROPERTY "AROS_MODULE_CONFIG_${_key}")
    if(_explicit)
        set(${out_var} "${_explicit}" PARENT_SCOPE)
    else()
        set(${out_var} "${directory}/${target}.conf" PARENT_SCOPE)
    endif()
endfunction()

function(_aros_generate_module_support out_prefix)
    set(options ABI SOURCES_ONLY)
    set(oneValueArgs TARGET MMAKE_ID DIRECTORY MODTYPE MODSUFFIX)
    cmake_parse_arguments(GM "${options}" "${oneValueArgs}" "" ${ARGN})

    if(NOT GM_TARGET OR NOT GM_MMAKE_ID OR NOT GM_DIRECTORY OR NOT GM_MODTYPE)
        message(FATAL_ERROR
            "_aros_generate_module_support: TARGET, MMAKE_ID, DIRECTORY and MODTYPE are required")
    endif()
    if(NOT GM_MODTYPE IN_LIST AROS_GENMODULE_MODTYPES)
        message(FATAL_ERROR
            "${GM_MMAKE_ID}: '${GM_MODTYPE}' is not a genmodule module type")
    endif()
    if(GM_ABI AND GM_SOURCES_ONLY)
        message(FATAL_ERROR
            "${GM_MMAKE_ID}: ABI and SOURCES_ONLY are mutually exclusive")
    endif()
    if(NOT AROS_HOST_GENMODULE)
        message(FATAL_ERROR
            "${GM_MMAKE_ID}: legacy genmodule host tool was not registered")
    endif()

    cmake_path(ABSOLUTE_PATH GM_DIRECTORY NORMALIZE OUTPUT_VARIABLE _module_dir)
    _aros_module_config(_conf "${GM_MMAKE_ID}" "${_module_dir}" "${GM_TARGET}")
    if(NOT EXISTS "${_conf}")
        message(FATAL_ERROR "${GM_MMAKE_ID}: missing genmodule config ${_conf}")
    endif()

    file(RELATIVE_PATH _module_rel "${CMAKE_SOURCE_DIR}" "${_module_dir}")
    if(_module_rel MATCHES "^\\.\\." OR IS_ABSOLUTE "${_module_rel}")
        string(SHA256 _module_rel "${_module_dir}")
    endif()
    string(MAKE_C_IDENTIFIER "${GM_MMAKE_ID}" _safe_id)
    set(_root "${CMAKE_BINARY_DIR}/genmodule/${_module_rel}/${_safe_id}")
    set(_gen_dir "${_root}/gen")
    set(_include_dir "${_root}/include")
    set(_stub_dir "${_root}/linklib")
    set(_fd_dir "${_root}/fd")

    set(_opts -c "${_conf}")
    if(GM_MODSUFFIX)
        list(APPEND _opts -s "${GM_MODSUFFIX}")
    endif()

    set(_include_rel
        "clib/${GM_TARGET}_protos.h"
        "inline/${GM_TARGET}.h"
        "defines/${GM_TARGET}.h"
        "defines/${GM_TARGET}_LVO.h"
        "proto/${GM_TARGET}.h")
    set(_private_headers "")
    set(_published_headers "")
    set(_publish_commands "")
    set(_private_include_dirs
        "${_include_dir}/clib" "${_include_dir}/inline"
        "${_include_dir}/defines" "${_include_dir}/proto"
        "${_include_dir}/interface")
    set(_publish_dirs
        "${AROS_SDK_INCLUDE_DIR}/clib" "${AROS_SDK_INCLUDE_DIR}/inline"
        "${AROS_SDK_INCLUDE_DIR}/defines" "${AROS_SDK_INCLUDE_DIR}/proto"
        "${AROS_GENINC_DIR}/clib" "${AROS_GENINC_DIR}/inline"
        "${AROS_GENINC_DIR}/defines" "${AROS_GENINC_DIR}/proto"
        "${AROS_DEVELOPER_INCLUDE_DIR}/clib" "${AROS_DEVELOPER_INCLUDE_DIR}/inline"
        "${AROS_DEVELOPER_INCLUDE_DIR}/defines" "${AROS_DEVELOPER_INCLUDE_DIR}/proto")
    foreach(_rel IN LISTS _include_rel)
        set(_private "${_include_dir}/${_rel}")
        list(APPEND _private_headers "${_private}")
        foreach(_public_root
                "${AROS_SDK_INCLUDE_DIR}"
                "${AROS_GENINC_DIR}"
                "${AROS_DEVELOPER_INCLUDE_DIR}")
            set(_public "${_public_root}/${_rel}")
            list(APPEND _published_headers "${_public}")
            list(APPEND _publish_commands
                COMMAND "${CMAKE_COMMAND}" -E copy_if_different
                    "${_private}" "${_public}")
        endforeach()
    endforeach()

    # SOURCES_ONLY skips the public headers for two reasons. genmodule emits
    # them only under `options includes` (tools/genmodule/genmodule.c:39), so
    # declaring the five outputs for a module that states `noincludes` would
    # make Ninja fail on a rule that legitimately wrote nothing. And the SDK
    # header story is already owned by the Rust aros-genmodule's configure-time
    # scan; publishing 265 more modules into the same three roots would put
    # same-named headers into a race whose winner is parse order.
    set(_includes_target "")
    if(NOT GM_SOURCES_ONLY)
        # BootstrapSDK's broad configure-time scan may just have written one of
        # these paths from a same-named, non-ABI config.  Remove only the outputs
        # this exact declaration owns; Ninja will now require the rule below and
        # cannot consume the transient wrong arosx headers.
        file(REMOVE ${_published_headers})
        add_custom_command(
            OUTPUT ${_private_headers} ${_published_headers}
            COMMAND "${CMAKE_COMMAND}" -E make_directory
                "${_include_dir}" ${_private_include_dirs} ${_publish_dirs}
            COMMAND "${AROS_HOST_GENMODULE}" ${_opts} -d "${_include_dir}"
                writeincludes "${GM_TARGET}" "${GM_MODTYPE}"
            ${_publish_commands}
            DEPENDS "${AROS_HOST_GENMODULE}" "${_conf}"
            COMMENT "Generating exact ${GM_TARGET}.${GM_MODTYPE} ABI headers"
            VERBATIM)
        set(_includes_target "${GM_MMAKE_ID}-includes-generated")
        add_custom_target("${_includes_target}" DEPENDS ${_published_headers})
    else()
        set(_published_headers "")
    endif()

    set(_libdefs "${_gen_dir}/${GM_TARGET}_libdefs.h")
    add_custom_command(
        OUTPUT "${_libdefs}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_gen_dir}"
        COMMAND "${AROS_HOST_GENMODULE}" ${_opts} -d "${_gen_dir}"
            writelibdefs "${GM_TARGET}" "${GM_MODTYPE}"
        DEPENDS "${AROS_HOST_GENMODULE}" "${_conf}"
        COMMENT "Generating exact ${GM_TARGET}.${GM_MODTYPE} libdefs"
        VERBATIM)
    # Only an existing Rust header with the same module name can shadow this
    # declaration's exact reference header. Most explicit conffile users have
    # no such broad output and therefore resolve directly to the reference.
    # Check both the declaration directory and the config directory: MetaMake
    # permits those to differ, and the broad scanner owns the latter.
    aros_arch_path_matches(_audit_arch_ok "${_module_dir}")
    if(_audit_arch_ok)
        get_filename_component(_conf_dir "${_conf}" DIRECTORY)
        file(RELATIVE_PATH _conf_rel "${CMAKE_SOURCE_DIR}" "${_conf_dir}")
        set(_rust_candidates
            "${AROS_GEN_DIR}/${_module_rel}/${GM_TARGET}_libdefs.h"
            "${AROS_GEN_DIR}/${_conf_rel}/${GM_TARGET}_libdefs.h")
        list(REMOVE_DUPLICATES _rust_candidates)
        set(_candidate_index 0)
        foreach(_rust_libdefs IN LISTS _rust_candidates)
            if(EXISTS "${_rust_libdefs}" AND NOT IS_DIRECTORY "${_rust_libdefs}")
                math(EXPR _candidate_index "${_candidate_index} + 1")
                aros_register_libdefs_audit(
                    "${GM_MMAKE_ID}#${_candidate_index}"
                    "${_rust_libdefs}" "${_libdefs}")
            endif()
        endforeach()
    endif()

    set(_start "${_gen_dir}/${GM_TARGET}_start.c")
    set(_end "${_gen_dir}/${GM_TARGET}_end.c")
    set(_entrypoints "${_gen_dir}/${GM_TARGET}${GM_MODTYPE}.entrypoints")
    aros_genmodule_writefiles_manifest(_manifest
        CONFIG "${_conf}"
        MODULE "${GM_TARGET}"
        MODTYPE "${GM_MODTYPE}"
        GEN_DIR "${_gen_dir}"
        STUB_DIR "${_stub_dir}")
    set(_normal_linklib_sources
        ${_manifest_NORMAL_STUBS}
        ${_manifest_NORMAL_AUTOINIT}
        ${_manifest_NORMAL_GETLIBBASE})
    set(_rel_linklib_sources
        ${_manifest_REL_STUBS}
        ${_manifest_REL_AUTOINIT}
        ${_manifest_REL_GETLIBBASE})
    set(_stub_sources ${_normal_linklib_sources})
    set_source_files_properties(${_manifest_ALL_OUTPUTS} PROPERTIES GENERATED TRUE)

    add_custom_command(
        OUTPUT ${_manifest_ALL_OUTPUTS}
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_gen_dir}" "${_stub_dir}"
        COMMAND "${AROS_HOST_GENMODULE}" ${_opts}
            -d "${_gen_dir}" -l "${_stub_dir}"
            writefiles "${GM_TARGET}" "${GM_MODTYPE}"
        DEPENDS "${AROS_HOST_GENMODULE}" "${_conf}" "${_libdefs}"
        COMMENT "Generating ${GM_TARGET}.${GM_MODTYPE} module support sources"
        VERBATIM)

    # The MetaMake graph names <mmake>-genmodfiles directly.  Bind that public
    # identity to the real writefiles outputs here; the transpiler's later
    # guarded meta-target declaration then reuses it and may add genmakefile
    # ordering without turning the generation step back into an empty phony.
    set(_genmodfiles_target "${GM_MMAKE_ID}-genmodfiles-generated")
    add_custom_target("${_genmodfiles_target}"
        DEPENDS ${_manifest_ALL_OUTPUTS})
    _aros_genmodule_alias("${GM_MMAKE_ID}-genmodfiles"
        "${_genmodfiles_target}")

    set(_fd "")
    set(_fd_target "")
    set(_has_exported_functions FALSE)
    if(GM_ABI)
        file(STRINGS "${_conf}" _conf_lines)
        set(_in_function_list FALSE)
        foreach(_conf_line IN LISTS _conf_lines)
            string(STRIP "${_conf_line}" _conf_line)
            if(_conf_line MATCHES "^##[ \\t]*begin[ \\t]+(c)?functionlist$")
                set(_in_function_list TRUE)
            elseif(_conf_line MATCHES "^##[ \\t]*end[ \\t]+(c)?functionlist$")
                set(_in_function_list FALSE)
            elseif(_in_function_list AND
                   NOT _conf_line STREQUAL "" AND
                   NOT _conf_line MATCHES "^#")
                set(_has_exported_functions TRUE)
                break()
            endif()
        endforeach()
    endif()
    if(GM_ABI AND _has_exported_functions)
        set(_private_fd "${_fd_dir}/${GM_TARGET}_lib.fd")
        set(_fd "${AROS_DEVELOPER_FD_DIR}/${GM_TARGET}_lib.fd")
        file(REMOVE "${_fd}")
        add_custom_command(
            OUTPUT "${_private_fd}" "${_fd}"
            COMMAND "${CMAKE_COMMAND}" -E make_directory
                "${_fd_dir}" "${AROS_DEVELOPER_FD_DIR}"
            COMMAND "${AROS_HOST_GENMODULE}" ${_opts} -d "${_fd_dir}"
                writefd "${GM_TARGET}" "${GM_MODTYPE}"
            COMMAND "${CMAKE_COMMAND}" -E copy_if_different
                "${_private_fd}" "${_fd}"
            DEPENDS "${AROS_HOST_GENMODULE}" "${_conf}"
            COMMENT "Generating ${GM_TARGET}.${GM_MODTYPE} FD"
            VERBATIM)
        set(_fd_target "${GM_MMAKE_ID}-fd-generated")
        add_custom_target("${_fd_target}" DEPENDS "${_fd}")
    endif()

    set(${out_prefix}_ROOT "${_root}" PARENT_SCOPE)
    set(${out_prefix}_GEN_DIR "${_gen_dir}" PARENT_SCOPE)
    set(${out_prefix}_INCLUDE_DIR "${_include_dir}" PARENT_SCOPE)
    set(${out_prefix}_HEADERS "${_published_headers}" PARENT_SCOPE)
    set(${out_prefix}_INCLUDES_TARGET "${_includes_target}" PARENT_SCOPE)
    set(${out_prefix}_LIBDEFS "${_libdefs}" PARENT_SCOPE)
    set(${out_prefix}_START "${_start}" PARENT_SCOPE)
    set(${out_prefix}_END "${_end}" PARENT_SCOPE)
    set(${out_prefix}_ENTRYPOINTS "${_entrypoints}" PARENT_SCOPE)
    set(${out_prefix}_STUB_SOURCES "${_stub_sources}" PARENT_SCOPE)
    set(${out_prefix}_NORMAL_LINKLIB_SOURCES
        "${_normal_linklib_sources}" PARENT_SCOPE)
    set(${out_prefix}_REL_LINKLIB_SOURCES
        "${_rel_linklib_sources}" PARENT_SCOPE)
    set(${out_prefix}_HAS_REL_LINKLIB
        "${_manifest_HAS_REL_LINKLIB}" PARENT_SCOPE)
    set(${out_prefix}_RELLIBS "${_manifest_RELLIBS}" PARENT_SCOPE)
    set(${out_prefix}_RUNTIME_DEFINES
        "${_manifest_RUNTIME_DEFINES}" PARENT_SCOPE)
    set(${out_prefix}_LINKLIB_DEFINES
        "${_manifest_LINKLIB_DEFINES}" PARENT_SCOPE)
    set(${out_prefix}_GENMODFILES_TARGET "${_genmodfiles_target}" PARENT_SCOPE)
    set(${out_prefix}_FD "${_fd}" PARENT_SCOPE)
    set(${out_prefix}_FD_TARGET "${_fd_target}" PARENT_SCOPE)
endfunction()

# aros_mirror_module_objects(<module> <sources>)
#
# A second compilation of the module's own sources, as an object library, so a
# kickstart member can be linked twice: once as the loadable module and once as
# the kickstart object, which uses a different library set entirely
# (cmake/KickstartObjects.cmake).
#
# The compile state is copied from the finished module target rather than
# reconstructed, so the two compilations cannot drift. It does mean the member's
# sources are compiled twice; three modules are members of the pc-x86_64
# kickstart, and the alternative is restructuring every module builder around an
# object library.
function(aros_mirror_module_objects module)
    set(_objects "${module}-objs")
    if(TARGET "${_objects}" OR NOT TARGET "${module}")
        return()
    endif()
    get_target_property(_sources "${module}" SOURCES)
    if(NOT _sources)
        return()
    endif()
    # $<TARGET_OBJECTS:> yields only what the library compiles, so a source
    # that is already an object -- the wrapped SMP trampoline is one -- has to
    # be carried separately or it silently leaves the kickstart object without
    # _binary_smpbootstrap_start.
    set(_compiled "")
    set(_external "")
    foreach(_source IN LISTS _sources)
        get_source_file_property(_is_object "${_source}" EXTERNAL_OBJECT)
        if(_is_object)
            list(APPEND _external "${_source}")
        else()
            list(APPEND _compiled "${_source}")
        endif()
    endforeach()
    if(NOT _compiled)
        return()
    endif()
    set_property(GLOBAL PROPERTY
        "AROS_KICKSTART_EXTOBJS_${module}" "${_external}")
    add_library("${_objects}" OBJECT ${_compiled})
    set_target_properties("${_objects}" PROPERTIES LINKER_LANGUAGE C)
    foreach(_property INCLUDE_DIRECTORIES COMPILE_DEFINITIONS COMPILE_OPTIONS)
        get_target_property(_value "${module}" ${_property})
        if(_value AND NOT _value STREQUAL "_value-NOTFOUND")
            set_property(TARGET "${_objects}" PROPERTY ${_property} "${_value}")
        endif()
    endforeach()
    # The generated sources have to exist before this compiles too.
    get_target_property(_deps "${module}" MANUALLY_ADDED_DEPENDENCIES)
    if(_deps AND NOT _deps STREQUAL "_deps-NOTFOUND")
        add_dependencies("${_objects}" ${_deps})
    endif()
    set_property(GLOBAL PROPERTY "AROS_KICKSTART_OBJECTS_${module}" "${_objects}")
endfunction()

# aros_apply_32bit_isa(<target>)
#
# Compiles a declaration for the 32-bit companion CPU.
#
# The reference states this as `ISA_FLAGS := $(ISA_32_FLAGS)` and gets the value
# from Autoconf (configure.in:1463, `--target=i386-unknown-aros` for llvm).
# There is no Autoconf here, so this substitutes the 32-bit form of the triple
# this build already chooses per CPU at cmake/AROS.cmake:301 -- the same
# substitution, one CPU down. Without it gen/lib32 holds 64-bit objects and the
# 32-bit PC bootstrap cannot link against them.
function(aros_apply_32bit_isa target)
    if(NOT TARGET "${target}" OR NOT AROS_TARGET_CPU32)
        return()
    endif()
    if(AROS_TARGET_CPU STREQUAL "x86_64" AND
       AROS_TARGET_CPU32 STREQUAL "i386")
        # This is the exact LLVM value configure.in installs in
        # ISA_32_FLAGS. The locked x86_64 release contract validates that the
        # same prefix also contains libclang_rt.builtins-i386.a, so a release
        # toolchain is explicitly dual-target here rather than single-triple.
        set(_companion_triple "i386-unknown-aros")
    else()
        message(FATAL_ERROR
            "${target}: unsupported 32-bit companion ${AROS_TARGET_CPU32} "
            "for ${AROS_TARGET_CPU}")
    endif()
    if(AROS_CROSS_TOOLCHAIN_ROOT AND
       NOT AROS_CROSS_TOOLCHAIN_COMPANION_TRIPLE STREQUAL _companion_triple)
        message(FATAL_ERROR
            "${target}: locked toolchain does not declare the required "
            "companion triple ${_companion_triple}")
    endif()
    # The single-argument form: `-target <triple>` as two list items is split
    # by CMake and clang then reads the triple as a file name.
    target_compile_options("${target}" PRIVATE
        "--target=${_companion_triple}")
    set_property(TARGET "${target}" PROPERTY AROS_VARIANT_32BIT TRUE)
    set_property(GLOBAL APPEND PROPERTY AROS_32BIT_TARGETS "${target}")
endfunction()

# aros_module_is_kickstart_member(<out-var> <arch-list>)
#
# Whether the module belongs to a kickstart of the configured architecture. The
# transpiler marks the member with every kickstart architecture that claims it,
# because a module can be in another architecture's kickstart and must not grow
# a second artefact here for that one.
function(aros_module_is_kickstart_member out_var)
    set(${out_var} FALSE PARENT_SCOPE)
    foreach(_arch IN LISTS ARGN)
        aros_package_arch_matches(_matches "${_arch}")
        if(_matches)
            set(${out_var} TRUE PARENT_SCOPE)
            return()
        endif()
    endforeach()
endfunction()

# aros_module_scaffolding(<out-sources> <prefix> MODTYPE <t> TARGET <name>
#                         MMAKE_ID <id> DIRECTORY <dir> [MODSUFFIX <s>])
#
# The generated `<mod>_start.c` and `<mod>_end.c` a module needs to be a module
# at all: the start file carries the romtag and the init entry
# (tools/genmodule/writestart.c) and the end file the marker the romtag scanner
# leaps to (tools/genmodule/writeend.c:44). Only aros_add_library asked for
# them, so 265 declarations with a genmodule config -- every resource, device,
# hidd, gadget, datatype, mcc and custom module target -- were built without an
# entry point. rom/kernel is the visible case: kernel_init.c references its own
# kernel_End and kernel_FuncTable, and nothing generated them.
#
# Returns an empty source list when the declaration has no config -- the one it
# names with `conffile=`, or `<name>.conf` when it names none. That is not a
# defect; a hand-written module has nothing for genmodule to read. The caller
# does not have to check.
#
# Sets <prefix>_GEN_DIR, <prefix>_INCLUDE_DIR, <prefix>_GENMODFILES_TARGET and
# <prefix>_RUNTIME_DEFINES for aros_attach_module_scaffolding.
function(aros_module_scaffolding out_sources out_prefix)
    set(oneValueArgs MODTYPE TARGET MMAKE_ID DIRECTORY MODSUFFIX)
    cmake_parse_arguments(MS "" "${oneValueArgs}" "" ${ARGN})

    set(${out_sources} "" PARENT_SCOPE)
    set(${out_prefix}_GEN_DIR "" PARENT_SCOPE)
    set(${out_prefix}_INCLUDE_DIR "" PARENT_SCOPE)
    set(${out_prefix}_GENMODFILES_TARGET "" PARENT_SCOPE)
    set(${out_prefix}_RUNTIME_DEFINES "" PARENT_SCOPE)
    if(NOT MS_TARGET OR NOT MS_MMAKE_ID OR NOT MS_DIRECTORY OR NOT MS_MODTYPE)
        return()
    endif()
    _aros_module_config(_ms_conf "${MS_MMAKE_ID}" "${MS_DIRECTORY}" "${MS_TARGET}")
    if(NOT EXISTS "${_ms_conf}")
        return()
    endif()
    # A declaration that already owns the full generation must not get a second
    # one under the same private root.
    if(TARGET "${MS_MMAKE_ID}-genmodfiles-generated")
        return()
    endif()

    _aros_generate_module_support(_ms SOURCES_ONLY
        TARGET "${MS_TARGET}"
        MMAKE_ID "${MS_MMAKE_ID}"
        DIRECTORY "${MS_DIRECTORY}"
        MODTYPE "${MS_MODTYPE}"
        MODSUFFIX "${MS_MODSUFFIX}")

    set(${out_sources} "${_ms_START}" "${_ms_END}" PARENT_SCOPE)
    set(${out_prefix}_GEN_DIR "${_ms_GEN_DIR}" PARENT_SCOPE)
    set(${out_prefix}_INCLUDE_DIR "${_ms_INCLUDE_DIR}" PARENT_SCOPE)
    set(${out_prefix}_GENMODFILES_TARGET
        "${_ms_GENMODFILES_TARGET}" PARENT_SCOPE)
    set(${out_prefix}_RUNTIME_DEFINES
        "${_ms_RUNTIME_DEFINES}" PARENT_SCOPE)
endfunction()

# aros_place_module_scaffolding(<sources-var> <scaffold-list>)
#
# Order matters, and only in one direction: the generated start file has to come
# first and the end file last.
#
# config/make.tmpl:2681 puts `$(addsuffix .o,$(_STARTFILES))` at the head of a
# module's object list, and :2712 passes the end object separately so the link
# rule can put it at the tail. Both are load-bearing:
#
#   bootstrap/elfloader.c:676  takes the first executable section of the
#                              loaded image as the entry point, so the start
#                              file's code must be the first code there
#   tools/genmodule/writeend.c:44
#                              the End marker must be the last thing in the
#                              module, because the romtag's rt_EndSkip points
#                              at it
#
# Appending both put kernel_init's text first in the kickstart, so the bootstrap
# jumped into the middle of it. The trace ran off into zeroed memory and stopped
# on a stray 0x2f:
#
#   v=06 IP=0008:000000000031052f
function(aros_place_module_scaffolding sources_var scaffold)
    list(LENGTH scaffold _count)
    if(_count EQUAL 0)
        return()
    endif()
    if(NOT _count EQUAL 2)
        message(FATAL_ERROR
            "aros_place_module_scaffolding: expected a start and an end file, got: ${scaffold}")
    endif()
    list(GET scaffold 0 _start)
    list(GET scaffold 1 _end)
    set(_sources "${_start}" ${${sources_var}} "${_end}")
    set(${sources_var} "${_sources}" PARENT_SCOPE)
endfunction()

# aros_attach_module_scaffolding(<target> <prefix> <directory> <module>)
#
# Puts the private generated tree ahead of every other include path, so the
# generated start file finds its own `<mod>_libdefs.h`, and orders the target
# after the generation step.
function(aros_attach_module_scaffolding target prefix directory module)
    if(NOT TARGET "${target}" OR NOT ${prefix}_GEN_DIR)
        return()
    endif()
    target_include_directories("${target}" BEFORE PRIVATE
        "${${prefix}_INCLUDE_DIR}" "${${prefix}_GEN_DIR}")
    _aros_add_genmodule_quote_dirs("${target}"
        "${${prefix}_INCLUDE_DIR}" "${${prefix}_GEN_DIR}")
    if(${prefix}_RUNTIME_DEFINES)
        target_compile_definitions("${target}" PRIVATE
            ${${prefix}_RUNTIME_DEFINES})
    endif()
    _aros_add_genmodule_config_header_dependencies(
        "${target}" "${directory}/${module}.conf")
    if(${prefix}_GENMODFILES_TARGET AND TARGET "${${prefix}_GENMODFILES_TARGET}")
        add_dependencies("${target}" "${${prefix}_GENMODFILES_TARGET}")
    endif()
endfunction()

# Select the linker language for a runtime module or program.  Upstream's
# C++ compile rule claims the linker whenever `cxxfiles=` is non-empty;
# `alwayscxxlink=yes` is the additional force for C/ASM-only wrappers such as
# Mesa's HIDDs.  The release C++ partial-link rule supplies libc++, libc++abi
# and libunwind, so forcing a C++ source through the naked C linker loses part
# of the target ABI contract.
function(_aros_set_module_linker_language target always_cxx_link)
    set(multiValueArgs CXX_SOURCES)
    cmake_parse_arguments(ML "" "" "${multiValueArgs}" ${ARGN})
    if(always_cxx_link OR ML_CXX_SOURCES)
        set_target_properties("${target}" PROPERTIES LINKER_LANGUAGE CXX)
        if(AROS_CROSS_TOOLCHAIN_ROOT)
            if(NOT AROS_CXX_STARTUP_TARGET OR
               NOT TARGET "${AROS_CXX_STARTUP_TARGET}")
                message(FATAL_ERROR
                    "${target}: locked C++ link has no cxx-startup producer target")
            endif()
            add_dependencies("${target}" "${AROS_CXX_STARTUP_TARGET}")
        endif()
    else()
        set_target_properties("${target}" PROPERTIES LINKER_LANGUAGE C)
    endif()
endfunction()

# aros_add_module_abi(TARGET <name> MMAKE_ID <id> DIRECTORY <dir>
#                     MODTYPE <type> [MODSUFFIX <suffix>] ...)
#
# %build_module_abi has no runtime module.  Its concrete product is the static
# client link library, accompanied by public headers and an FD.  Keep the main
# mmake identity as its aggregate: graph-wide dependencies such as
# core-linklibs belong there, while linklibs-<name> must reach the archive
# without inheriting that main-target closure.
function(aros_add_module_abi)
    set(oneValueArgs TARGET MMAKE_ID DIRECTORY MODTYPE MODSUFFIX)
    set(multiValueArgs LIBS USELIBS INCLUDES ARCH_INCLUDES
        DEFINES UNDEFINES COMPILE_OPTIONS
        ARCH_DEFINES ARCH_COMPILE_OPTIONS)
    cmake_parse_arguments(ARG "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(NOT ARG_TARGET OR NOT ARG_MMAKE_ID OR NOT ARG_DIRECTORY OR NOT ARG_MODTYPE)
        message(FATAL_ERROR
            "aros_add_module_abi: TARGET, MMAKE_ID, DIRECTORY and MODTYPE are required")
    endif()
    if(TARGET "${ARG_MMAKE_ID}")
        message(FATAL_ERROR "aros_add_module_abi: duplicate target ${ARG_MMAKE_ID}")
    endif()

    _aros_generate_module_support(_gm ABI
        TARGET "${ARG_TARGET}"
        MMAKE_ID "${ARG_MMAKE_ID}"
        DIRECTORY "${ARG_DIRECTORY}"
        MODTYPE "${ARG_MODTYPE}"
        MODSUFFIX "${ARG_MODSUFFIX}")

    _aros_bind_genmodule_abi_targets("${ARG_MMAKE_ID}"
        "${_gm_INCLUDES_TARGET}" "${_gm_FD_TARGET}")

    add_library("${ARG_MMAKE_ID}-linklib" STATIC EXCLUDE_FROM_ALL
        ${_gm_STUB_SOURCES})
    set_target_properties("${ARG_MMAKE_ID}-linklib" PROPERTIES
        OUTPUT_NAME "${ARG_TARGET}"
        ARCHIVE_OUTPUT_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}"
        LINKER_LANGUAGE C)
    target_include_directories("${ARG_MMAKE_ID}-linklib" BEFORE PRIVATE
        "${_gm_INCLUDE_DIR}" "${_gm_GEN_DIR}"
        "${AROS_SDK_INCLUDE_DIR}/aros/posixc"
        "${AROS_SDK_INCLUDE_DIR}/aros/stdc")
    _aros_add_genmodule_quote_dirs("${ARG_MMAKE_ID}-linklib"
        "${_gm_INCLUDE_DIR}" "${_gm_GEN_DIR}")
    add_dependencies("${ARG_MMAKE_ID}-linklib"
        "${_gm_INCLUDES_TARGET}" "${_gm_FD_TARGET}")
    _aros_add_genmodule_config_header_dependencies(
        "${ARG_MMAKE_ID}-linklib"
        "${ARG_DIRECTORY}/${ARG_TARGET}.conf")
    aros_gate_arch("${ARG_MMAKE_ID}-linklib" "${ARG_DIRECTORY}")
    aros_apply_includes("${ARG_MMAKE_ID}-linklib"
        MODULE_DIR "${ARG_DIRECTORY}"
        INCLUDES ${ARG_INCLUDES}
        ARCH_INCLUDES ${ARG_ARCH_INCLUDES})
    aros_apply_flags("${ARG_MMAKE_ID}-linklib"
        DEFINES ${ARG_DEFINES}
        UNDEFINES ${ARG_UNDEFINES}
        COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
        ARCH_DEFINES ${ARG_ARCH_DEFINES}
        ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS})

    add_custom_target("${ARG_MMAKE_ID}")
    add_dependencies("${ARG_MMAKE_ID}"
        "${ARG_MMAKE_ID}-includes"
        "${ARG_MMAKE_ID}-fd"
        "${ARG_MMAKE_ID}-linklib")
    _aros_genmodule_alias("includes-${ARG_TARGET}" "${ARG_MMAKE_ID}-includes")
    _aros_genmodule_alias("includes-${ARG_TARGET}_rel" "${ARG_MMAKE_ID}-includes")
    _aros_genmodule_alias("linklibs-${ARG_TARGET}" "${ARG_MMAKE_ID}-linklib")
    _aros_genmodule_alias("linklibs-${ARG_TARGET}_rel" "${ARG_MMAKE_ID}-linklib")
    _aros_genmodule_alias(includes-all "${ARG_MMAKE_ID}-includes")
    _aros_register_genmodule_public_includes("${_gm_INCLUDES_TARGET}")
endfunction()

# Macro: aros_add_library
function(aros_add_library)
    set(options ALWAYS_CXX_LINK GENMODULE_ONLY GENMODULE_LINKLIBS)
    # DEFAULT_MODTYPE belongs here, and its absence was not harmless. It is
    # read at :4067, so it looked declared; it was not, so `DEFAULT_MODTYPE mcc`
    # from aros_add_mcc extended whatever multi-value argument came before it in
    # the emitted call -- SOURCES. The 48 gadget, mcc and datatype declarations
    # therefore each gained two sources named `DEFAULT_MODTYPE` and their
    # modtype, and lost the modtype itself, so their genmodule scaffolding was
    # generated as `library`: `error superclass specified when not a BOOPSI
    # class`. The generated-source report added beside this is what showed the
    # two bogus sources.
    set(oneValueArgs TARGET MMAKE_ID DIRECTORY INSTALL_DIR
        MODSUFFIX DEFAULT_MODTYPE DEFAULT_INSTALL_DIR DEFAULT_MODSUFFIX
        LINKLIB_NAME)
    set(multiValueArgs SOURCES CXX_SOURCES OBJC_SOURCES ASM_SOURCES
        LIBS USELIBS INCLUDES ARCH_INCLUDES
        DEFINES UNDEFINES COMPILE_OPTIONS ARCH_SOURCES
        ARCH_DEFINES ARCH_COMPILE_OPTIONS LINK_OPTIONS
        LINKLIB_SOURCES LINKLIB_OBJECT_SOURCES KICKSTART_MEMBER)
    cmake_parse_arguments(ARG "${options}" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(ARG_GENMODULE_ONLY)
        if(NOT ARG_TARGET OR NOT ARG_MMAKE_ID OR NOT ARG_DIRECTORY)
            message(FATAL_ERROR
                "aros_add_library(GENMODULE_ONLY): TARGET, MMAKE_ID and DIRECTORY are required")
        endif()
        if(ARG_SOURCES OR ARG_CXX_SOURCES OR ARG_OBJC_SOURCES OR
           ARG_ASM_SOURCES OR ARG_ARCH_SOURCES)
            message(FATAL_ERROR
                "${ARG_MMAKE_ID}: GENMODULE_ONLY is only valid for an explicitly source-free module")
        endif()
        if(TARGET "${ARG_MMAKE_ID}")
            message(FATAL_ERROR "aros_add_library: duplicate target ${ARG_MMAKE_ID}")
        endif()

        # A source-free full module may also have no exported functions (the
        # version.library skeleton is the only current case).  genmodule then
        # deliberately emits no FD file, so keep this path on its historical
        # headers/linklib contract instead of declaring a nonexistent output.
        _aros_generate_module_support(_gm
            TARGET "${ARG_TARGET}"
            MMAKE_ID "${ARG_MMAKE_ID}"
            DIRECTORY "${ARG_DIRECTORY}"
            MODTYPE library
            MODSUFFIX "${ARG_MODSUFFIX}")

        _aros_genmodule_alias("${ARG_MMAKE_ID}-includes"
            "${_gm_INCLUDES_TARGET}")
        add_library("${ARG_MMAKE_ID}-linklib" STATIC ${_gm_STUB_SOURCES})
        set_target_properties("${ARG_MMAKE_ID}-linklib" PROPERTIES
            OUTPUT_NAME "${ARG_TARGET}"
            ARCHIVE_OUTPUT_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}"
            LINKER_LANGUAGE C)
        target_include_directories("${ARG_MMAKE_ID}-linklib" BEFORE PRIVATE
            "${_gm_INCLUDE_DIR}" "${_gm_GEN_DIR}"
            "${AROS_SDK_INCLUDE_DIR}/aros/posixc"
            "${AROS_SDK_INCLUDE_DIR}/aros/stdc")
        _aros_add_genmodule_quote_dirs("${ARG_MMAKE_ID}-linklib"
            "${_gm_INCLUDE_DIR}" "${_gm_GEN_DIR}")
        _aros_add_genmodule_config_header_dependencies(
            "${ARG_MMAKE_ID}-linklib"
            "${ARG_DIRECTORY}/${ARG_TARGET}.conf")
        aros_gate_arch("${ARG_MMAKE_ID}-linklib" "${ARG_DIRECTORY}")
        aros_apply_includes("${ARG_MMAKE_ID}-linklib"
            MODULE_DIR "${ARG_DIRECTORY}"
            INCLUDES ${ARG_INCLUDES}
            ARCH_INCLUDES ${ARG_ARCH_INCLUDES})
        aros_apply_flags("${ARG_MMAKE_ID}-linklib"
            DEFINES ${ARG_DEFINES}
            UNDEFINES ${ARG_UNDEFINES}
            COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
            ARCH_DEFINES ${ARG_ARCH_DEFINES}
            ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS})
        add_dependencies("${ARG_MMAKE_ID}-linklib" "${ARG_MMAKE_ID}-includes")

        if(ARG_DEFAULT_INSTALL_DIR)
            set(_default_install_dir "${ARG_DEFAULT_INSTALL_DIR}")
        else()
            set(_default_install_dir "${AROS_LIBS_DIR}")
        endif()
        if(ARG_DEFAULT_MODSUFFIX)
            set(_default_modsuffix "${ARG_DEFAULT_MODSUFFIX}")
        else()
            set(_default_modsuffix "library")
        endif()
        _aros_module_install_dir(_install_dir
            "${_default_install_dir}" "${ARG_INSTALL_DIR}")
        _aros_module_output_name(_output_name "${ARG_TARGET}"
            "${_default_modsuffix}" "${ARG_MODSUFFIX}")

        # nostartup=yes adds compiler/libinit/libentry.o in make.tmpl:2684-2688;
        # compile its source into this otherwise generator-only module.
        add_executable("${ARG_MMAKE_ID}"
            "${CMAKE_SOURCE_DIR}/compiler/libinit/libentry.c"
            "${_gm_START}"
            "${_gm_END}")
        target_compile_definitions("${ARG_MMAKE_ID}" PRIVATE
            LC_LIBDEFS_FILE="${ARG_TARGET}_libdefs.h"
            __AROS_LIBNAME__=${ARG_TARGET}
            __AROS_MODNAME__=${ARG_TARGET})
        target_include_directories("${ARG_MMAKE_ID}" BEFORE PRIVATE
            "${_gm_INCLUDE_DIR}" "${_gm_GEN_DIR}"
            "${AROS_SDK_INCLUDE_DIR}/aros/posixc"
            "${AROS_SDK_INCLUDE_DIR}/aros/stdc")
        _aros_add_genmodule_quote_dirs("${ARG_MMAKE_ID}"
            "${_gm_INCLUDE_DIR}" "${_gm_GEN_DIR}")
        set_target_properties("${ARG_MMAKE_ID}" PROPERTIES
            OUTPUT_NAME "${_output_name}"
            RUNTIME_OUTPUT_DIRECTORY "${_install_dir}")
        _aros_set_module_linker_language("${ARG_MMAKE_ID}"
            "${ARG_ALWAYS_CXX_LINK}"
            CXX_SOURCES ${ARG_CXX_SOURCES})
        add_dependencies("${ARG_MMAKE_ID}"
            "${ARG_MMAKE_ID}-includes"
            "${ARG_MMAKE_ID}-linklib")
        aros_gate_arch("${ARG_MMAKE_ID}" "${ARG_DIRECTORY}")
        aros_apply_includes("${ARG_MMAKE_ID}"
            MODULE_DIR "${ARG_DIRECTORY}"
            INCLUDES ${ARG_INCLUDES}
            ARCH_INCLUDES ${ARG_ARCH_INCLUDES})
        aros_apply_flags("${ARG_MMAKE_ID}"
            DEFINES ${ARG_DEFINES}
            UNDEFINES ${ARG_UNDEFINES}
            COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
            ARCH_DEFINES ${ARG_ARCH_DEFINES}
            ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS})
        aros_apply_link_options("${ARG_MMAKE_ID}" ${ARG_LINK_OPTIONS})
        if(ARG_LIBS)
            aros_link_libraries("${ARG_MMAKE_ID}" ${ARG_LIBS})
        endif()

        _aros_genmodule_alias("includes-${ARG_TARGET}" "${ARG_MMAKE_ID}-includes")
        _aros_genmodule_alias("includes-${ARG_TARGET}_rel" "${ARG_MMAKE_ID}-includes")
        _aros_genmodule_alias("linklibs-${ARG_TARGET}" "${ARG_MMAKE_ID}-linklib")
        _aros_genmodule_alias("linklibs-${ARG_TARGET}_rel" "${ARG_MMAKE_ID}-linklib")
        _aros_genmodule_alias(includes-all "${ARG_MMAKE_ID}-includes")
        _aros_register_genmodule_public_includes("${_gm_INCLUDES_TARGET}")
        return()
    endif()

    if((NOT ARG_SOURCES AND NOT ARG_CXX_SOURCES AND
        NOT ARG_OBJC_SOURCES AND NOT ARG_ASM_SOURCES AND
        NOT ARG_ARCH_SOURCES) OR
       NOT ARG_MMAKE_ID)
        return()
    endif()

    # Sourceful full modules opt in only when their declaration publishes a
    # client archive or the dependency graph proves that another enabled
    # module needs their relative provider.  Both cases carry an exact parsed
    # linklibfiles/linklibobjs manifest; unrelated legacy libraries stay on the
    # ordinary runtime-only path.
    set(_has_genmodule FALSE)
    if(ARG_GENMODULE_LINKLIBS OR ARG_LINKLIB_NAME)
        _aros_generate_module_support(_gm ABI
            TARGET "${ARG_TARGET}"
            MMAKE_ID "${ARG_MMAKE_ID}"
            DIRECTORY "${ARG_DIRECTORY}"
            MODTYPE library
            MODSUFFIX "${ARG_MODSUFFIX}")
        set(_has_genmodule TRUE)

        if(_gm_FD_TARGET)
            _aros_bind_genmodule_abi_targets("${ARG_MMAKE_ID}"
                "${_gm_INCLUDES_TARGET}" "${_gm_FD_TARGET}")
        else()
            # genmodule deliberately emits no FD for an empty function list,
            # even when a sourceful module still publishes a client archive.
            _aros_genmodule_alias("${ARG_MMAKE_ID}-includes"
                "${_gm_INCLUDES_TARGET}")
        endif()

        aros_resolve_source_lanes(_linklib_sources "${ARG_DIRECTORY}"
            MMAKE_ID "${ARG_MMAKE_ID}-linklib-inputs"
            SOURCES ${ARG_LINKLIB_SOURCES})
        aros_resolve_source_lanes(_linklib_object_sources "${ARG_DIRECTORY}"
            MMAKE_ID "${ARG_MMAKE_ID}-linklib-object-inputs"
            SOURCES ${ARG_LINKLIB_OBJECT_SOURCES})

        set(_linklib_object_target "")
        set(_linklib_object_stamp "")
        set(_linklib_object_stamp_target "")
        if(_linklib_object_sources)
            add_library("${ARG_MMAKE_ID}-linklib-objects" OBJECT
                ${_linklib_object_sources})
            set(_linklib_object_target
                "${ARG_MMAKE_ID}-linklib-objects")
            target_include_directories(
                "${ARG_MMAKE_ID}-linklib-objects" BEFORE PRIVATE
                "${_gm_INCLUDE_DIR}" "${_gm_GEN_DIR}"
                "${AROS_SDK_INCLUDE_DIR}/aros/posixc"
                "${AROS_SDK_INCLUDE_DIR}/aros/stdc")
            _aros_add_genmodule_quote_dirs(
                "${ARG_MMAKE_ID}-linklib-objects"
                "${_gm_INCLUDE_DIR}" "${_gm_GEN_DIR}")
            target_compile_definitions(
                "${ARG_MMAKE_ID}-linklib-objects" PRIVATE
                LC_LIBDEFS_FILE="${ARG_TARGET}_libdefs.h"
                ${_gm_RUNTIME_DEFINES})
            add_dependencies("${ARG_MMAKE_ID}-linklib-objects"
                "${ARG_MMAKE_ID}-includes" "${_gm_GENMODFILES_TARGET}")
            _aros_add_genmodule_config_header_dependencies(
                "${ARG_MMAKE_ID}-linklib-objects"
                "${ARG_DIRECTORY}/${ARG_TARGET}.conf")
            aros_gate_arch(
                "${ARG_MMAKE_ID}-linklib-objects" "${ARG_DIRECTORY}")
            aros_apply_includes("${ARG_MMAKE_ID}-linklib-objects"
                MODULE_DIR "${ARG_DIRECTORY}"
                INCLUDES ${ARG_INCLUDES}
                ARCH_INCLUDES ${ARG_ARCH_INCLUDES})
            aros_apply_flags("${ARG_MMAKE_ID}-linklib-objects"
                DEFINES ${ARG_DEFINES}
                UNDEFINES ${ARG_UNDEFINES}
                COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
                ARCH_DEFINES ${ARG_ARCH_DEFINES}
                ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS})

            # CMake always places TARGET_OBJECTS before ordinary archive
            # sources, whereas MetaMake appends linklibobjs last.  A separate
            # stamp gives the client archives a real, content-sensitive link
            # dependency while their POST_BUILD step below preserves the
            # legacy member order.
            set(_linklib_object_stamp
                "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/${ARG_MMAKE_ID}-linklib-objects.ready")
            set(_linklib_object_stamp_target
                "${ARG_MMAKE_ID}-linklib-objects-ready")
            add_custom_command(
                OUTPUT "${_linklib_object_stamp}"
                COMMAND "${CMAKE_COMMAND}" -E touch
                    "${_linklib_object_stamp}"
                DEPENDS "$<TARGET_OBJECTS:${_linklib_object_target}>"
                COMMENT "Tracking exact linklibobjs for ${ARG_MMAKE_ID}"
                COMMAND_EXPAND_LISTS
                VERBATIM)
            add_custom_target("${_linklib_object_stamp_target}"
                DEPENDS "${_linklib_object_stamp}")
            add_dependencies("${_linklib_object_stamp_target}"
                "${_linklib_object_target}")
        endif()

        # make.tmpl archives declaration linklibfiles first, generated client
        # stubs second, and precompiled linklibobjs last. CMake places
        # $<TARGET_OBJECTS:...> before ordinary sources regardless of its
        # textual position, so linklibobjs are appended explicitly below.
        set(_normal_client_sources
            ${_linklib_sources}
            ${_gm_NORMAL_LINKLIB_SOURCES})
        add_library("${ARG_MMAKE_ID}-linklib" STATIC
            ${_normal_client_sources})
        set_target_properties("${ARG_MMAKE_ID}-linklib" PROPERTIES
            OUTPUT_NAME "${ARG_TARGET}"
            ARCHIVE_OUTPUT_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}"
            LINKER_LANGUAGE C)
        set(_client_link_targets "${ARG_MMAKE_ID}-linklib")

        set(_client_namespace_includes "")
        foreach(_rellib IN LISTS _gm_RELLIBS)
            if(_rellib STREQUAL "posixc" OR _rellib STREQUAL "stdc")
                list(APPEND _client_namespace_includes
                    "${AROS_SDK_INCLUDE_DIR}/aros/${_rellib}")
            endif()
        endforeach()

        if(_gm_HAS_REL_LINKLIB)
            set(_rel_client_sources
                ${_linklib_sources}
                ${_gm_REL_LINKLIB_SOURCES})
            add_library("${ARG_MMAKE_ID}-linklib-rel" STATIC
                ${_rel_client_sources})
            set_target_properties("${ARG_MMAKE_ID}-linklib-rel" PROPERTIES
                OUTPUT_NAME "${ARG_TARGET}_rel"
                ARCHIVE_OUTPUT_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}"
                LINKER_LANGUAGE C)
            list(APPEND _client_link_targets
                "${ARG_MMAKE_ID}-linklib-rel")
        endif()

        foreach(_client_target IN LISTS _client_link_targets)
            if(_client_namespace_includes)
                set_property(TARGET "${_client_target}" PROPERTY
                    AROS_CLIENT_NAMESPACE_INCLUDES
                    "${_client_namespace_includes}")
            endif()
            if(_linklib_object_target)
                add_dependencies("${_client_target}"
                    "${_linklib_object_stamp_target}")
                if(_client_target STREQUAL "${ARG_MMAKE_ID}-linklib-rel")
                    set(_client_sources "${_rel_client_sources}")
                    set(_client_anchor_sources
                        "${_gm_REL_LINKLIB_SOURCES}")
                else()
                    set(_client_sources "${_normal_client_sources}")
                    set(_client_anchor_sources
                        "${_gm_NORMAL_LINKLIB_SOURCES}")
                endif()
                if(NOT _client_anchor_sources)
                    set(_client_anchor_sources "${_client_sources}")
                endif()
                list(GET _client_anchor_sources 0
                    _linklib_dependency_anchor)
                set_property(SOURCE "${_linklib_dependency_anchor}" APPEND
                    PROPERTY OBJECT_DEPENDS "${_linklib_object_stamp}")
                add_custom_command(TARGET "${_client_target}" POST_BUILD
                    COMMAND "${CMAKE_AR}" q
                        "$<TARGET_FILE:${_client_target}>"
                        "$<TARGET_OBJECTS:${_linklib_object_target}>"
                    COMMAND "${CMAKE_RANLIB}"
                        "$<TARGET_FILE:${_client_target}>"
                    COMMENT "Appending exact linklibobjs to ${_client_target}"
                    COMMAND_EXPAND_LISTS
                    VERBATIM)
            endif()
            target_include_directories("${_client_target}" BEFORE PRIVATE
                "${_gm_INCLUDE_DIR}" "${_gm_GEN_DIR}"
                "${AROS_SDK_INCLUDE_DIR}/aros/posixc"
                "${AROS_SDK_INCLUDE_DIR}/aros/stdc")
            _aros_add_genmodule_quote_dirs("${_client_target}"
                "${_gm_INCLUDE_DIR}" "${_gm_GEN_DIR}")
            target_compile_definitions("${_client_target}" PRIVATE
                LC_LIBDEFS_FILE="${ARG_TARGET}_libdefs.h"
                ${_gm_LINKLIB_DEFINES})
            _aros_add_genmodule_config_header_dependencies(
                "${_client_target}"
                "${ARG_DIRECTORY}/${ARG_TARGET}.conf")
            aros_gate_arch("${_client_target}" "${ARG_DIRECTORY}")
            aros_apply_includes("${_client_target}"
                MODULE_DIR "${ARG_DIRECTORY}"
                INCLUDES ${ARG_INCLUDES}
                ARCH_INCLUDES ${ARG_ARCH_INCLUDES})
            aros_apply_flags("${_client_target}"
                DEFINES ${ARG_DEFINES}
                UNDEFINES ${ARG_UNDEFINES}
                COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
                ARCH_DEFINES ${ARG_ARCH_DEFINES}
                ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS})
            add_dependencies("${_client_target}"
                "${ARG_MMAKE_ID}-includes" "${_gm_GENMODFILES_TARGET}")
        endforeach()

        # linklibname= is a second public archive spelling for the generated
        # client interface. Keep one compilation owner per variant and publish
        # both aliases as tracked byproducts.
        if(ARG_LINKLIB_NAME AND NOT ARG_LINKLIB_NAME STREQUAL ARG_TARGET)
            set(_linklib_alias
                "${AROS_DEVELOPER_LIB_DIR}/lib${ARG_LINKLIB_NAME}.a")
            add_custom_command(TARGET "${ARG_MMAKE_ID}-linklib" POST_BUILD
                BYPRODUCTS "${_linklib_alias}"
                COMMAND "${CMAKE_COMMAND}" -E copy_if_different
                    "$<TARGET_FILE:${ARG_MMAKE_ID}-linklib>"
                    "${_linklib_alias}"
                COMMENT "Publishing ${ARG_LINKLIB_NAME} client link library"
                VERBATIM)
            if(_gm_HAS_REL_LINKLIB)
                set(_rel_linklib_alias
                    "${AROS_DEVELOPER_LIB_DIR}/lib${ARG_LINKLIB_NAME}_rel.a")
                add_custom_command(
                    TARGET "${ARG_MMAKE_ID}-linklib-rel" POST_BUILD
                    BYPRODUCTS "${_rel_linklib_alias}"
                    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
                        "$<TARGET_FILE:${ARG_MMAKE_ID}-linklib-rel>"
                        "${_rel_linklib_alias}"
                    COMMENT "Publishing ${ARG_LINKLIB_NAME}_rel client link library"
                    VERBATIM)
            endif()
        endif()
    endif()

    aros_resolve_source_lanes(RESOLVED_SOURCES "${ARG_DIRECTORY}"
        MMAKE_ID "${ARG_MMAKE_ID}"
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES})
    if(ARG_ARCH_SOURCES)
        aros_resolve_arch_sources(_ARCH_RESOLVED _ARCH_DROPPED "${ARG_DIRECTORY}"
            SOURCES ${RESOLVED_SOURCES}
            ARCH_SOURCES ${ARG_ARCH_SOURCES}
        )
        if(_ARCH_RESOLVED)
            set(RESOLVED_SOURCES "${_ARCH_RESOLVED}")
            list(REMOVE_ITEM RESOLVED_SOURCES "")
        endif()
    endif()
    aros_report_empty_concrete_target(
        MMAKE_ID "${ARG_MMAKE_ID}"
        DIRECTORY "${ARG_DIRECTORY}"
        RESOLVED_SOURCES ${RESOLVED_SOURCES}
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES}
        ARCH_SOURCES ${ARG_ARCH_SOURCES})
    aros_mark_preprocessed_asm(${RESOLVED_SOURCES})

    if(RESOLVED_SOURCES)
        if(_has_genmodule)
            list(APPEND RESOLVED_SOURCES "${_gm_START}" "${_gm_END}")
        endif()
        # aros_add_gadget, aros_add_mcc and aros_add_datatype come through here,
        # and their module type is not `library`. The full generation above runs
        # only for a declaration with a client archive, so without this the 48
        # of them get no start or end file either.
        if(ARG_DEFAULT_MODTYPE)
            set(_library_modtype "${ARG_DEFAULT_MODTYPE}")
        else()
            set(_library_modtype "library")
        endif()
        aros_module_scaffolding(_scaffold_sources _scaffold
            MODTYPE "${_library_modtype}"
            TARGET "${ARG_TARGET}"
            MMAKE_ID "${ARG_MMAKE_ID}"
            DIRECTORY "${ARG_DIRECTORY}"
            MODSUFFIX "${ARG_MODSUFFIX}")
        aros_place_module_scaffolding(RESOLVED_SOURCES "${_scaffold_sources}")
        if(ARG_DEFAULT_INSTALL_DIR)
            set(_default_install_dir "${ARG_DEFAULT_INSTALL_DIR}")
        else()
            set(_default_install_dir "${AROS_LIBS_DIR}")
        endif()
        if(ARG_DEFAULT_MODSUFFIX)
            set(_default_modsuffix "${ARG_DEFAULT_MODSUFFIX}")
        else()
            set(_default_modsuffix "library")
        endif()
        _aros_module_install_dir(_install_dir
            "${_default_install_dir}" "${ARG_INSTALL_DIR}")
        if(_has_genmodule)
            set(_module_base_name "${ARG_TARGET}")
        else()
            set(_module_base_name "${ARG_MMAKE_ID}")
        endif()
        _aros_module_output_name(_output_name "${_module_base_name}"
            "${_default_modsuffix}" "${ARG_MODSUFFIX}")

        add_executable(${ARG_MMAKE_ID} ${RESOLVED_SOURCES})
        aros_attach_module_scaffolding("${ARG_MMAKE_ID}" _scaffold
            "${ARG_DIRECTORY}" "${ARG_TARGET}")
        target_compile_definitions(${ARG_MMAKE_ID} PRIVATE
            LC_LIBDEFS_FILE="${ARG_TARGET}_libdefs.h"
            __AROS_LIBNAME__=${ARG_TARGET}
            __AROS_MODNAME__=${ARG_TARGET}
            ${_gm_RUNTIME_DEFINES}
        )
        set_target_properties(${ARG_MMAKE_ID} PROPERTIES
            OUTPUT_NAME "${_output_name}"
            RUNTIME_OUTPUT_DIRECTORY "${_install_dir}")
        _aros_set_module_linker_language("${ARG_MMAKE_ID}"
            "${ARG_ALWAYS_CXX_LINK}"
            CXX_SOURCES ${ARG_CXX_SOURCES})
        if(_has_genmodule)
            target_include_directories(${ARG_MMAKE_ID} BEFORE PRIVATE
                "${_gm_INCLUDE_DIR}" "${_gm_GEN_DIR}"
                "${AROS_SDK_INCLUDE_DIR}/aros/posixc"
                "${AROS_SDK_INCLUDE_DIR}/aros/stdc")
            _aros_add_genmodule_quote_dirs(${ARG_MMAKE_ID}
                "${_gm_INCLUDE_DIR}" "${_gm_GEN_DIR}")
            _aros_add_genmodule_config_header_dependencies(
                "${ARG_MMAKE_ID}"
                "${ARG_DIRECTORY}/${ARG_TARGET}.conf")
            add_dependencies(${ARG_MMAKE_ID}
                "${ARG_MMAKE_ID}-includes"
                "${ARG_MMAKE_ID}-linklib")
            if(_gm_FD_TARGET)
                add_dependencies(${ARG_MMAKE_ID}
                    "${ARG_MMAKE_ID}-fd")
            endif()
            if(_gm_HAS_REL_LINKLIB)
                add_dependencies(${ARG_MMAKE_ID}
                    "${ARG_MMAKE_ID}-linklib-rel")
            endif()
        endif()
        aros_gate_arch(${ARG_MMAKE_ID} "${ARG_DIRECTORY}")
        aros_apply_includes(${ARG_MMAKE_ID}
            MODULE_DIR "${ARG_DIRECTORY}"
            INCLUDES ${ARG_INCLUDES}
            ARCH_INCLUDES ${ARG_ARCH_INCLUDES}
        )
        aros_apply_flags(${ARG_MMAKE_ID}
            DEFINES ${ARG_DEFINES}
            UNDEFINES ${ARG_UNDEFINES}
            COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
            ARCH_DEFINES ${ARG_ARCH_DEFINES}
            ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS}
        )
        aros_apply_link_options(${ARG_MMAKE_ID} ${ARG_LINK_OPTIONS})
        if(ARG_LIBS)
            aros_link_libraries(${ARG_MMAKE_ID} ${ARG_LIBS})
        endif()
        # A kickstart member needs a second link with a different library set,
        # so make this compilation reusable. cmake/KickstartObjects.cmake.
        aros_module_is_kickstart_member(_is_kickstart ${ARG_KICKSTART_MEMBER})
        if(_is_kickstart)
            aros_mirror_module_objects("${ARG_MMAKE_ID}")
            set_property(GLOBAL PROPERTY
                "AROS_KICKSTART_USELIBS_${ARG_MMAKE_ID}" "${ARG_USELIBS}")
            # config/make.tmpl:2758 passes $(USER_LDFLAGS) to the kobj link as
            # well, which is how arch/all-pc/kernel/make.opts' -lbootconsole
            # and -lacpica reach a kickstart member.
            set_property(GLOBAL PROPERTY
                "AROS_KICKSTART_LDOPTS_${ARG_MMAKE_ID}" "${ARG_LINK_OPTIONS}")
        endif()

        if(_has_genmodule)
            _aros_genmodule_alias("includes-${ARG_TARGET}"
                "${ARG_MMAKE_ID}-includes")
            _aros_genmodule_alias("includes-${ARG_TARGET}_rel"
                "${ARG_MMAKE_ID}-includes")
            _aros_genmodule_alias("linklibs-${ARG_TARGET}"
                "${ARG_MMAKE_ID}-linklib")
            if(_gm_HAS_REL_LINKLIB)
                _aros_genmodule_alias("linklibs-${ARG_TARGET}_rel"
                    "${ARG_MMAKE_ID}-linklib-rel")
            endif()
            if(ARG_LINKLIB_NAME)
                _aros_genmodule_alias("linklibs-${ARG_LINKLIB_NAME}"
                    "${ARG_MMAKE_ID}-linklib")
                if(_gm_HAS_REL_LINKLIB)
                    _aros_genmodule_alias("linklibs-${ARG_LINKLIB_NAME}_rel"
                        "${ARG_MMAKE_ID}-linklib-rel")
                endif()
            endif()
            _aros_genmodule_alias(includes-all "${ARG_MMAKE_ID}-includes")
            _aros_register_genmodule_public_includes("${_gm_INCLUDES_TARGET}")
        endif()
    endif()
endfunction()

# Macro: aros_add_device
function(aros_add_device)
    set(options ALWAYS_CXX_LINK)
    set(oneValueArgs TARGET MMAKE_ID DIRECTORY INSTALL_DIR MODSUFFIX)
    set(multiValueArgs SOURCES CXX_SOURCES OBJC_SOURCES ASM_SOURCES
        LIBS USELIBS INCLUDES ARCH_INCLUDES
        DEFINES UNDEFINES COMPILE_OPTIONS ARCH_SOURCES
        ARCH_DEFINES ARCH_COMPILE_OPTIONS LINK_OPTIONS)
    cmake_parse_arguments(ARG "${options}" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if((NOT ARG_SOURCES AND NOT ARG_CXX_SOURCES AND
        NOT ARG_OBJC_SOURCES AND NOT ARG_ASM_SOURCES AND
        NOT ARG_ARCH_SOURCES) OR
       NOT ARG_MMAKE_ID)
        return()
    endif()

    aros_resolve_source_lanes(RESOLVED_SOURCES "${ARG_DIRECTORY}"
        MMAKE_ID "${ARG_MMAKE_ID}"
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES})
    if(ARG_ARCH_SOURCES)
        aros_resolve_arch_sources(_ARCH_RESOLVED _ARCH_DROPPED "${ARG_DIRECTORY}"
            SOURCES ${RESOLVED_SOURCES}
            ARCH_SOURCES ${ARG_ARCH_SOURCES}
        )
        if(_ARCH_RESOLVED)
            set(RESOLVED_SOURCES "${_ARCH_RESOLVED}")
            list(REMOVE_ITEM RESOLVED_SOURCES "")
        endif()
    endif()
    aros_report_empty_concrete_target(
        MMAKE_ID "${ARG_MMAKE_ID}"
        DIRECTORY "${ARG_DIRECTORY}"
        RESOLVED_SOURCES ${RESOLVED_SOURCES}
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES}
        ARCH_SOURCES ${ARG_ARCH_SOURCES})
    aros_mark_preprocessed_asm(${RESOLVED_SOURCES})

    if(RESOLVED_SOURCES)
        aros_module_scaffolding(_scaffold_sources _scaffold
            MODTYPE device
            TARGET "${ARG_TARGET}"
            MMAKE_ID "${ARG_MMAKE_ID}"
            DIRECTORY "${ARG_DIRECTORY}"
            MODSUFFIX "${ARG_MODSUFFIX}")
        aros_place_module_scaffolding(RESOLVED_SOURCES "${_scaffold_sources}")
        _aros_module_install_dir(_install_dir
            "${AROS_DEVS_DIR}" "${ARG_INSTALL_DIR}")
        _aros_module_output_name(_output_name "${ARG_MMAKE_ID}"
            "device" "${ARG_MODSUFFIX}")
        add_executable(${ARG_MMAKE_ID} ${RESOLVED_SOURCES})
        aros_attach_module_scaffolding("${ARG_MMAKE_ID}" _scaffold
            "${ARG_DIRECTORY}" "${ARG_TARGET}")
        target_compile_definitions(${ARG_MMAKE_ID} PRIVATE
            LC_LIBDEFS_FILE="${ARG_TARGET}_libdefs.h"
            __AROS_DEVNAME__=${ARG_TARGET}
            __AROS_MODNAME__=${ARG_TARGET}
        )
        set_target_properties(${ARG_MMAKE_ID} PROPERTIES
            OUTPUT_NAME "${_output_name}"
            RUNTIME_OUTPUT_DIRECTORY "${_install_dir}")
        _aros_set_module_linker_language("${ARG_MMAKE_ID}"
            "${ARG_ALWAYS_CXX_LINK}"
            CXX_SOURCES ${ARG_CXX_SOURCES})
        aros_gate_arch(${ARG_MMAKE_ID} "${ARG_DIRECTORY}")
        aros_apply_includes(${ARG_MMAKE_ID}
            MODULE_DIR "${ARG_DIRECTORY}"
            INCLUDES ${ARG_INCLUDES}
            ARCH_INCLUDES ${ARG_ARCH_INCLUDES}
        )
        aros_apply_flags(${ARG_MMAKE_ID}
            DEFINES ${ARG_DEFINES}
            UNDEFINES ${ARG_UNDEFINES}
            COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
            ARCH_DEFINES ${ARG_ARCH_DEFINES}
            ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS}
        )
        aros_apply_link_options(${ARG_MMAKE_ID} ${ARG_LINK_OPTIONS})
        if(ARG_LIBS)
            aros_link_libraries(${ARG_MMAKE_ID} ${ARG_LIBS})
        endif()
    endif()
endfunction()

# Macro: aros_add_resource
function(aros_add_resource)
    set(options ALWAYS_CXX_LINK)
    set(oneValueArgs TARGET MMAKE_ID DIRECTORY INSTALL_DIR MODSUFFIX)
    set(multiValueArgs SOURCES CXX_SOURCES OBJC_SOURCES ASM_SOURCES
        LIBS USELIBS INCLUDES ARCH_INCLUDES
        DEFINES UNDEFINES COMPILE_OPTIONS ARCH_SOURCES
        ARCH_DEFINES ARCH_COMPILE_OPTIONS LINK_OPTIONS KICKSTART_MEMBER)
    cmake_parse_arguments(ARG "${options}" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if((NOT ARG_SOURCES AND NOT ARG_CXX_SOURCES AND
        NOT ARG_OBJC_SOURCES AND NOT ARG_ASM_SOURCES AND
        NOT ARG_ARCH_SOURCES) OR
       NOT ARG_MMAKE_ID)
        return()
    endif()

    aros_resolve_source_lanes(RESOLVED_SOURCES "${ARG_DIRECTORY}"
        MMAKE_ID "${ARG_MMAKE_ID}"
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES})
    if(ARG_ARCH_SOURCES)
        aros_resolve_arch_sources(_ARCH_RESOLVED _ARCH_DROPPED "${ARG_DIRECTORY}"
            SOURCES ${RESOLVED_SOURCES}
            ARCH_SOURCES ${ARG_ARCH_SOURCES}
        )
        if(_ARCH_RESOLVED)
            set(RESOLVED_SOURCES "${_ARCH_RESOLVED}")
            list(REMOVE_ITEM RESOLVED_SOURCES "")
        endif()
    endif()
    aros_report_empty_concrete_target(
        MMAKE_ID "${ARG_MMAKE_ID}"
        DIRECTORY "${ARG_DIRECTORY}"
        RESOLVED_SOURCES ${RESOLVED_SOURCES}
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES}
        ARCH_SOURCES ${ARG_ARCH_SOURCES})
    aros_mark_preprocessed_asm(${RESOLVED_SOURCES})

    if(RESOLVED_SOURCES)
        aros_module_scaffolding(_scaffold_sources _scaffold
            MODTYPE resource
            TARGET "${ARG_TARGET}"
            MMAKE_ID "${ARG_MMAKE_ID}"
            DIRECTORY "${ARG_DIRECTORY}"
            MODSUFFIX "${ARG_MODSUFFIX}")
        aros_place_module_scaffolding(RESOLVED_SOURCES "${_scaffold_sources}")
        _aros_module_install_dir(_install_dir
            "${AROS_RESOURCES_DIR}" "${ARG_INSTALL_DIR}")
        _aros_module_output_name(_output_name "${ARG_MMAKE_ID}"
            "resource" "${ARG_MODSUFFIX}")
        add_executable(${ARG_MMAKE_ID} ${RESOLVED_SOURCES})
        aros_attach_module_scaffolding("${ARG_MMAKE_ID}" _scaffold
            "${ARG_DIRECTORY}" "${ARG_TARGET}")
        target_compile_definitions(${ARG_MMAKE_ID} PRIVATE
            LC_LIBDEFS_FILE="${ARG_TARGET}_libdefs.h"
            __AROS_RESNAME__=${ARG_TARGET}
            __AROS_MODNAME__=${ARG_TARGET}
        )
        set_target_properties(${ARG_MMAKE_ID} PROPERTIES
            OUTPUT_NAME "${_output_name}"
            RUNTIME_OUTPUT_DIRECTORY "${_install_dir}")
        _aros_set_module_linker_language("${ARG_MMAKE_ID}"
            "${ARG_ALWAYS_CXX_LINK}"
            CXX_SOURCES ${ARG_CXX_SOURCES})
        aros_gate_arch(${ARG_MMAKE_ID} "${ARG_DIRECTORY}")
        aros_apply_includes(${ARG_MMAKE_ID}
            MODULE_DIR "${ARG_DIRECTORY}"
            INCLUDES ${ARG_INCLUDES}
            ARCH_INCLUDES ${ARG_ARCH_INCLUDES}
        )
        aros_apply_flags(${ARG_MMAKE_ID}
            DEFINES ${ARG_DEFINES}
            UNDEFINES ${ARG_UNDEFINES}
            COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
            ARCH_DEFINES ${ARG_ARCH_DEFINES}
            ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS}
        )
        aros_apply_link_options(${ARG_MMAKE_ID} ${ARG_LINK_OPTIONS})
        if(ARG_LIBS)
            aros_link_libraries(${ARG_MMAKE_ID} ${ARG_LIBS})
        endif()
        # A kickstart member needs a second link with a different library set,
        # so make this compilation reusable. cmake/KickstartObjects.cmake.
        aros_module_is_kickstart_member(_is_kickstart ${ARG_KICKSTART_MEMBER})
        if(_is_kickstart)
            aros_mirror_module_objects("${ARG_MMAKE_ID}")
            set_property(GLOBAL PROPERTY
                "AROS_KICKSTART_USELIBS_${ARG_MMAKE_ID}" "${ARG_USELIBS}")
            # config/make.tmpl:2758 passes $(USER_LDFLAGS) to the kobj link as
            # well, which is how arch/all-pc/kernel/make.opts' -lbootconsole
            # and -lacpica reach a kickstart member.
            set_property(GLOBAL PROPERTY
                "AROS_KICKSTART_LDOPTS_${ARG_MMAKE_ID}" "${ARG_LINK_OPTIONS}")
        endif()
    endif()
endfunction()

# Macro: aros_add_hidd
function(aros_add_hidd)
    set(options ALWAYS_CXX_LINK)
    set(oneValueArgs TARGET MMAKE_ID DIRECTORY INSTALL_DIR MODSUFFIX)
    set(multiValueArgs SOURCES CXX_SOURCES OBJC_SOURCES ASM_SOURCES
        LIBS USELIBS INCLUDES ARCH_INCLUDES
        DEFINES UNDEFINES COMPILE_OPTIONS ARCH_SOURCES
        ARCH_DEFINES ARCH_COMPILE_OPTIONS LINK_OPTIONS)
    cmake_parse_arguments(ARG "${options}" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if((NOT ARG_SOURCES AND NOT ARG_CXX_SOURCES AND
        NOT ARG_OBJC_SOURCES AND NOT ARG_ASM_SOURCES AND
        NOT ARG_ARCH_SOURCES) OR
       NOT ARG_MMAKE_ID)
        return()
    endif()

    aros_resolve_source_lanes(RESOLVED_SOURCES "${ARG_DIRECTORY}"
        MMAKE_ID "${ARG_MMAKE_ID}"
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES})
    if(ARG_ARCH_SOURCES)
        aros_resolve_arch_sources(_ARCH_RESOLVED _ARCH_DROPPED "${ARG_DIRECTORY}"
            SOURCES ${RESOLVED_SOURCES}
            ARCH_SOURCES ${ARG_ARCH_SOURCES}
        )
        if(_ARCH_RESOLVED)
            set(RESOLVED_SOURCES "${_ARCH_RESOLVED}")
            list(REMOVE_ITEM RESOLVED_SOURCES "")
        endif()
    endif()
    aros_report_empty_concrete_target(
        MMAKE_ID "${ARG_MMAKE_ID}"
        DIRECTORY "${ARG_DIRECTORY}"
        RESOLVED_SOURCES ${RESOLVED_SOURCES}
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES}
        ARCH_SOURCES ${ARG_ARCH_SOURCES})
    aros_mark_preprocessed_asm(${RESOLVED_SOURCES})

    if(RESOLVED_SOURCES)
        aros_module_scaffolding(_scaffold_sources _scaffold
            MODTYPE hidd
            TARGET "${ARG_TARGET}"
            MMAKE_ID "${ARG_MMAKE_ID}"
            DIRECTORY "${ARG_DIRECTORY}"
            MODSUFFIX "${ARG_MODSUFFIX}")
        aros_place_module_scaffolding(RESOLVED_SOURCES "${_scaffold_sources}")
        _aros_module_install_dir(_install_dir
            "${AROS_DRIVERS_DIR}" "${ARG_INSTALL_DIR}")
        _aros_module_output_name(_output_name "${ARG_MMAKE_ID}"
            "hidd" "${ARG_MODSUFFIX}")
        add_executable(${ARG_MMAKE_ID} ${RESOLVED_SOURCES})
        aros_attach_module_scaffolding("${ARG_MMAKE_ID}" _scaffold
            "${ARG_DIRECTORY}" "${ARG_TARGET}")
        target_compile_definitions(${ARG_MMAKE_ID} PRIVATE
            LC_LIBDEFS_FILE="${ARG_TARGET}_libdefs.h"
            __AROS_HIDDNAME__=${ARG_TARGET}
            __AROS_MODNAME__=${ARG_TARGET}
        )
        set_target_properties(${ARG_MMAKE_ID} PROPERTIES
            OUTPUT_NAME "${_output_name}"
            RUNTIME_OUTPUT_DIRECTORY "${_install_dir}")
        _aros_set_module_linker_language("${ARG_MMAKE_ID}"
            "${ARG_ALWAYS_CXX_LINK}"
            CXX_SOURCES ${ARG_CXX_SOURCES})
        aros_gate_arch(${ARG_MMAKE_ID} "${ARG_DIRECTORY}")
        aros_apply_includes(${ARG_MMAKE_ID}
            MODULE_DIR "${ARG_DIRECTORY}"
            INCLUDES ${ARG_INCLUDES}
            ARCH_INCLUDES ${ARG_ARCH_INCLUDES}
        )
        aros_apply_flags(${ARG_MMAKE_ID}
            DEFINES ${ARG_DEFINES}
            UNDEFINES ${ARG_UNDEFINES}
            COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
            ARCH_DEFINES ${ARG_ARCH_DEFINES}
            ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS}
        )
        aros_apply_link_options(${ARG_MMAKE_ID} ${ARG_LINK_OPTIONS})
        if(ARG_LIBS)
            aros_link_libraries(${ARG_MMAKE_ID} ${ARG_LIBS})
        endif()
    endif()
endfunction()

# Macro: aros_add_datatype
function(aros_add_datatype)
    aros_add_library(${ARGN}
        DEFAULT_MODTYPE datatype
        DEFAULT_INSTALL_DIR "${AROS_DATATYPES_DIR}"
        DEFAULT_MODSUFFIX "datatype")
endfunction()

# Macro: aros_add_gadget
function(aros_add_gadget)
    aros_add_library(${ARGN}
        DEFAULT_MODTYPE gadget
        DEFAULT_INSTALL_DIR "${AROS_GADGETS_DIR}"
        DEFAULT_MODSUFFIX "gadget")
endfunction()

# Macro: aros_add_mcc
function(aros_add_mcc)
    aros_add_library(${ARGN}
        DEFAULT_MODTYPE mcc
        DEFAULT_INSTALL_DIR "${AROS_ZUNE_CLASSES_DIR}"
        DEFAULT_MODSUFFIX "mcc")
endfunction()

# A generated-source marker emitted by the transpiler has this form:
#
#   @AROS_GENMODULE|<normal|rel>|<components>|<module>|<modtype>|<config>
#
# Components is a comma-separated subset of
# stackstubs,regcallstubs,autoinit,getlibbase.  The marker stays in
# TargetDefinition's ordinary C source lane, avoiding a target-name allowlist
# or a second, parallel target model.  It is replaced here with the explicit
# output manifest before add_library() sees it.
function(_aros_genmodule_linklib_sources
        out_sources out_target out_include_dir out_config directory marker)
    string(REPLACE "|" ";" _fields "${marker}")
    list(LENGTH _fields _field_count)
    if(NOT _field_count EQUAL 6)
        message(FATAL_ERROR
            "invalid genmodule linklib source marker '${marker}'")
    endif()
    list(GET _fields 0 _tag)
    list(GET _fields 1 _variant)
    list(GET _fields 2 _component_string)
    list(GET _fields 3 _module)
    list(GET _fields 4 _modtype)
    list(GET _fields 5 _config_arg)
    if(NOT "${_tag}" STREQUAL "@AROS_GENMODULE")
        message(FATAL_ERROR "invalid genmodule linklib tag '${_tag}'")
    endif()
    if(NOT "${_variant}" STREQUAL "normal" AND
       NOT "${_variant}" STREQUAL "rel")
        message(FATAL_ERROR
            "invalid genmodule linklib variant '${_variant}' in '${marker}'")
    endif()
    if(IS_ABSOLUTE "${_config_arg}")
        set(_config "${_config_arg}")
    else()
        set(_config "${directory}/${_config_arg}")
    endif()
    cmake_path(NORMAL_PATH _config)

    string(SHA256 _signature "${_config}|${_module}|${_modtype}")
    string(SUBSTRING "${_signature}" 0 16 _short_hash)
    set(_root "${CMAKE_BINARY_DIR}/genmodule/linklibs/${_short_hash}")
    set(_gen_dir "${_root}/gen")
    set(_stub_dir "${_root}/stubs")
    set(_include_dir "${_root}/include")
    set(_write_target "aros-genmodule-linklib-${_short_hash}")

    set(_include_rel
        "clib/${_module}_protos.h"
        "inline/${_module}.h"
        "defines/${_module}.h"
        "defines/${_module}_LVO.h"
        "proto/${_module}.h")
    set(_private_headers "")
    foreach(_rel IN LISTS _include_rel)
        list(APPEND _private_headers "${_include_dir}/${_rel}")
    endforeach()
    set(_private_include_dirs
        "${_include_dir}/clib" "${_include_dir}/inline"
        "${_include_dir}/defines" "${_include_dir}/proto"
        "${_include_dir}/interface")

    aros_genmodule_writefiles_manifest(_gm_linklib
        CONFIG "${_config}"
        MODULE "${_module}"
        MODTYPE "${_modtype}"
        GEN_DIR "${_gen_dir}"
        STUB_DIR "${_stub_dir}")

    if(NOT TARGET "${_write_target}")
        if(NOT AROS_HOST_GENMODULE)
            message(FATAL_ERROR
                "${marker}: legacy genmodule host tool was not registered")
        endif()
        add_custom_command(
            OUTPUT ${_private_headers} ${_gm_linklib_ALL_OUTPUTS}
            COMMAND "${CMAKE_COMMAND}" -E make_directory
                "${_gen_dir}" "${_stub_dir}" "${_include_dir}"
                ${_private_include_dirs}
            # A same-named public header can describe a different declaration
            # (posixc_lfa.conf versus posixc.conf is the concrete case).  Keep
            # the exact declaration private, just as module ABI targets do.
            COMMAND "${AROS_HOST_GENMODULE}" -c "${_config}"
                -d "${_include_dir}"
                writeincludes "${_module}" "${_modtype}"
            COMMAND "${AROS_HOST_GENMODULE}" -c "${_config}"
                -d "${_gen_dir}" -l "${_stub_dir}"
                writefiles "${_module}" "${_modtype}"
            DEPENDS "${AROS_HOST_GENMODULE}" "${_config}"
            COMMENT "Generating ${_module}.${_modtype} client-link sources"
            VERBATIM)
        add_custom_target("${_write_target}"
            DEPENDS ${_private_headers} ${_gm_linklib_ALL_OUTPUTS})
    endif()

    if("${_variant}" STREQUAL "normal")
        set(_prefix _gm_linklib_NORMAL)
    else()
        set(_prefix _gm_linklib_REL)
    endif()
    string(REPLACE "," ";" _components "${_component_string}")
    set(_selected "")
    foreach(_component IN LISTS _components)
        if("${_component}" STREQUAL "stackstubs")
            list(APPEND _selected ${${_prefix}_STACK_STUBS})
        elseif("${_component}" STREQUAL "regcallstubs")
            list(APPEND _selected ${${_prefix}_REGCALL_STUBS})
        elseif("${_component}" STREQUAL "autoinit")
            list(APPEND _selected ${${_prefix}_AUTOINIT})
        elseif("${_component}" STREQUAL "getlibbase")
            list(APPEND _selected ${${_prefix}_GETLIBBASE})
        else()
            message(FATAL_ERROR
                "unknown genmodule linklib component '${_component}' in '${marker}'")
        endif()
    endforeach()
    list(REMOVE_DUPLICATES _selected)
    if(NOT _selected)
        message(FATAL_ERROR
            "genmodule linklib marker '${marker}' selected no generated sources")
    endif()

    set_source_files_properties(${_selected} PROPERTIES GENERATED TRUE)
    set(${out_sources} "${_selected}" PARENT_SCOPE)
    set(${out_target} "${_write_target}" PARENT_SCOPE)
    set(${out_include_dir} "${_include_dir}" PARENT_SCOPE)
    set(${out_config} "${_config}" PARENT_SCOPE)
endfunction()

# Resolve one explicit `%build_linklib libdir=` output. Private archives must
# stay under this configuration's build tree; an absolute host path or parent
# traversal is never a valid generated-build destination.
function(_aros_validate_linklib_output_directory out_var owner requested)
    string(FIND "${requested}" ";" _has_semicolon)
    string(FIND "${requested}" "$" _has_dollar)
    string(FIND "${requested}" "\\" _has_backslash)
    if(NOT _has_semicolon EQUAL -1 OR
       NOT _has_dollar EQUAL -1 OR
       NOT _has_backslash EQUAL -1)
        message(FATAL_ERROR
            "${owner}: private linklib output contains unsafe syntax: ${requested}")
    endif()
    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR
        NORMALIZE OUTPUT_VARIABLE _binary_root)
    set(_requested "${requested}")
    cmake_path(ABSOLUTE_PATH _requested
        BASE_DIRECTORY "${_binary_root}" NORMALIZE OUTPUT_VARIABLE _output)
    cmake_path(IS_PREFIX _binary_root "${_output}" NORMALIZE _inside_build)
    if(NOT _inside_build OR _output STREQUAL _binary_root)
        message(FATAL_ERROR
            "${owner}: private linklib output escapes the build tree: ${_output}")
    endif()
    set(${out_var} "${_output}" PARENT_SCOPE)
endfunction()

# Reserve the complete archive path before creating its target. CMake normally
# notices duplicate outputs only when the backend is generated, and the error
# can depend on declaration order. A stable ownership check reports both
# MetaMake declarations at configure time instead.
function(_aros_claim_linklib_archive owner output_dir output_name)
    set(_archive
        "${output_dir}/${CMAKE_STATIC_LIBRARY_PREFIX}${output_name}${CMAKE_STATIC_LIBRARY_SUFFIX}")
    cmake_path(NORMAL_PATH _archive)
    string(SHA256 _archive_key "${_archive}")
    get_property(_previous_owner GLOBAL PROPERTY
        "AROS_LINKLIB_ARCHIVE_OWNER_${_archive_key}")
    if(_previous_owner AND NOT _previous_owner STREQUAL owner)
        message(FATAL_ERROR
            "${owner}: ${_archive} is already owned by ${_previous_owner}")
    endif()
    set_property(GLOBAL PROPERTY
        "AROS_LINKLIB_ARCHIVE_OWNER_${_archive_key}" "${owner}")
endfunction()

# Macro: aros_add_linklib
function(aros_add_linklib)
    set(options CANONICAL_OUTPUT EMPTY_ARCHIVE VARIANT_32BIT)
    set(oneValueArgs TARGET MMAKE_ID DIRECTORY OUTPUT_DIR)
    set(multiValueArgs SOURCES CXX_SOURCES OBJC_SOURCES ASM_SOURCES
        LIBS USELIBS INCLUDES ARCH_INCLUDES
        DEFINES UNDEFINES COMPILE_OPTIONS ARCH_SOURCES
        ARCH_DEFINES ARCH_COMPILE_OPTIONS LINK_OPTIONS)
    # PARSE_ARGV preserves semicolons inside quoted values so the private
    # output validator can reject them instead of silently seeing only the
    # first list element.
    cmake_parse_arguments(PARSE_ARGV 0 ARG
        "${options}" "${oneValueArgs}" "${multiValueArgs}")

    if(ARG_UNPARSED_ARGUMENTS)
        message(FATAL_ERROR
            "${ARG_MMAKE_ID}: aros_add_linklib contains unsafe syntax or unknown arguments: ${ARG_UNPARSED_ARGUMENTS}")
    endif()
    if(ARG_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "${ARG_MMAKE_ID}: aros_add_linklib has missing values: ${ARG_KEYWORDS_MISSING_VALUES}")
    endif()

    if(ARG_CANONICAL_OUTPUT AND ARG_OUTPUT_DIR)
        message(FATAL_ERROR
            "${ARG_MMAKE_ID}: CANONICAL_OUTPUT and OUTPUT_DIR are mutually exclusive")
    endif()
    if(ARG_EMPTY_ARCHIVE AND
       (ARG_SOURCES OR ARG_CXX_SOURCES OR ARG_OBJC_SOURCES OR
        ARG_ASM_SOURCES OR ARG_ARCH_SOURCES))
        message(FATAL_ERROR
            "${ARG_MMAKE_ID}: EMPTY_ARCHIVE cannot carry source inputs")
    endif()
    if(ARG_EMPTY_ARCHIVE AND NOT ARG_OUTPUT_DIR)
        message(FATAL_ERROR
            "${ARG_MMAKE_ID}: EMPTY_ARCHIVE requires an explicit private OUTPUT_DIR")
    endif()
    set(_private_output_dir "")
    if(ARG_OUTPUT_DIR)
        if(NOT ARG_TARGET OR NOT ARG_TARGET MATCHES "^[A-Za-z0-9_.+-]+$")
            message(FATAL_ERROR
                "${ARG_MMAKE_ID}: private linklib output requires a literal archive name")
        endif()
        _aros_validate_linklib_output_directory(
            _private_output_dir "${ARG_MMAKE_ID}" "${ARG_OUTPUT_DIR}")
    endif()

    set(_ordinary_c_sources "")
    set(_genmodule_sources "")
    set(_genmodule_targets "")
    set(_genmodule_include_dirs "")
    set(_genmodule_configs "")
    foreach(_source IN LISTS ARG_SOURCES)
        if(_source MATCHES "^@AROS_GENMODULE\\|")
            _aros_genmodule_linklib_sources(
                _marker_sources _marker_target _marker_include_dir _marker_config
                "${ARG_DIRECTORY}" "${_source}")
            list(APPEND _genmodule_sources ${_marker_sources})
            list(APPEND _genmodule_targets "${_marker_target}")
            list(APPEND _genmodule_include_dirs "${_marker_include_dir}")
            list(APPEND _genmodule_configs "${_marker_config}")
        else()
            list(APPEND _ordinary_c_sources "${_source}")
        endif()
    endforeach()
    list(REMOVE_DUPLICATES _genmodule_sources)
    list(REMOVE_DUPLICATES _genmodule_targets)
    list(REMOVE_DUPLICATES _genmodule_include_dirs)
    list(REMOVE_DUPLICATES _genmodule_configs)

    if((NOT ARG_EMPTY_ARCHIVE AND
        NOT _ordinary_c_sources AND NOT _genmodule_sources AND
        NOT ARG_CXX_SOURCES AND
        NOT ARG_OBJC_SOURCES AND NOT ARG_ASM_SOURCES AND
        NOT ARG_ARCH_SOURCES) OR
       NOT ARG_MMAKE_ID)
        return()
    endif()

    if(ARG_EMPTY_ARCHIVE)
        # A HEADER_FILE_ONLY source makes this a normal, linkable CMake static
        # library while contributing no object to its archiver invocation.
        # file(GENERATE) keeps the anchor stable across reconfiguration and the
        # hashed name cannot collide with another MetaMake owner.
        string(SHA256 _empty_anchor_key "${ARG_MMAKE_ID}")
        set(_empty_anchor_dir
            "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/aros-empty-linklibs")
        file(MAKE_DIRECTORY "${_empty_anchor_dir}")
        set(_empty_anchor "${_empty_anchor_dir}/${_empty_anchor_key}.h")
        file(GENERATE OUTPUT "${_empty_anchor}"
            CONTENT "/* Intentionally empty archive: ${ARG_MMAKE_ID}. */\n")
        set_source_files_properties("${_empty_anchor}" PROPERTIES
            GENERATED TRUE
            HEADER_FILE_ONLY TRUE)
        set(RESOLVED_SOURCES "${_empty_anchor}")
    else()
        aros_resolve_source_lanes(RESOLVED_SOURCES "${ARG_DIRECTORY}"
            MMAKE_ID "${ARG_MMAKE_ID}"
            SOURCES ${_ordinary_c_sources}
            CXX_SOURCES ${ARG_CXX_SOURCES}
            OBJC_SOURCES ${ARG_OBJC_SOURCES}
            ASM_SOURCES ${ARG_ASM_SOURCES})
        list(APPEND RESOLVED_SOURCES ${_genmodule_sources})
        if(ARG_ARCH_SOURCES)
            aros_resolve_arch_sources(_ARCH_RESOLVED _ARCH_DROPPED "${ARG_DIRECTORY}"
                SOURCES ${RESOLVED_SOURCES}
                ARCH_SOURCES ${ARG_ARCH_SOURCES}
            )
            if(_ARCH_RESOLVED)
                set(RESOLVED_SOURCES "${_ARCH_RESOLVED}")
                list(REMOVE_ITEM RESOLVED_SOURCES "")
            endif()
        endif()
    endif()
    aros_report_empty_concrete_target(
        MMAKE_ID "${ARG_MMAKE_ID}"
        DIRECTORY "${ARG_DIRECTORY}"
        RESOLVED_SOURCES ${RESOLVED_SOURCES}
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES}
        ARCH_SOURCES ${ARG_ARCH_SOURCES})
    aros_mark_preprocessed_asm(${RESOLVED_SOURCES})

    if(RESOLVED_SOURCES)
        if(_private_output_dir)
            _aros_claim_linklib_archive(
                "${ARG_MMAKE_ID}" "${_private_output_dir}" "${ARG_TARGET}")
        endif()
        add_library(${ARG_MMAKE_ID} STATIC ${RESOLVED_SOURCES})
        set_target_properties(${ARG_MMAKE_ID} PROPERTIES LINKER_LANGUAGE C)
        if(ARG_VARIANT_32BIT)
            aros_apply_32bit_isa("${ARG_MMAKE_ID}")
        endif()

        # Existing linklibs may target a host cross-tools directory, a private
        # bootstrap directory or lib32, and several deliberately share one
        # libname.  Move an archive to the public target SDK only when the
        # transpiler proved this declaration uses the default target compiler,
        # default libdir and uniquely fetch-owned port sources.
        if(ARG_CANONICAL_OUTPUT)
            set_target_properties(${ARG_MMAKE_ID} PROPERTIES
                OUTPUT_NAME "${ARG_TARGET}"
                ARCHIVE_OUTPUT_DIRECTORY "${AROS_DEVELOPER_LIB_DIR}")
        elseif(_private_output_dir)
            set_target_properties(${ARG_MMAKE_ID} PROPERTIES
                OUTPUT_NAME "${ARG_TARGET}"
                ARCHIVE_OUTPUT_DIRECTORY "${_private_output_dir}")
        endif()
        if(_genmodule_sources)
            # The target compiler's specs search the POSIX and standard-C
            # namespaces in this order before the common SDK root.  Clang's
            # bare-metal driver has no installed AROS specs, so reproduce that
            # ordering for these generated client sources explicitly.
            target_include_directories(${ARG_MMAKE_ID} BEFORE PRIVATE
                ${_genmodule_include_dirs}
                "${AROS_SDK_INCLUDE_DIR}/aros/posixc"
                "${AROS_SDK_INCLUDE_DIR}/aros/stdc")
            _aros_add_genmodule_quote_dirs(${ARG_MMAKE_ID}
                ${_genmodule_include_dirs})
            add_dependencies(${ARG_MMAKE_ID} ${_genmodule_targets})
            foreach(_config IN LISTS _genmodule_configs)
                _aros_add_genmodule_config_header_dependencies(
                    "${ARG_MMAKE_ID}" "${_config}")
            endforeach()
        endif()
        aros_gate_arch(${ARG_MMAKE_ID} "${ARG_DIRECTORY}")
        aros_apply_includes(${ARG_MMAKE_ID}
            MODULE_DIR "${ARG_DIRECTORY}"
            INCLUDES ${ARG_INCLUDES}
            ARCH_INCLUDES ${ARG_ARCH_INCLUDES}
        )
        aros_apply_flags(${ARG_MMAKE_ID}
            DEFINES ${ARG_DEFINES}
            UNDEFINES ${ARG_UNDEFINES}
            COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
            ARCH_DEFINES ${ARG_ARCH_DEFINES}
            ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS}
        )
    endif()
endfunction()

# Macro: aros_add_program
function(aros_add_program)
    set(options ALWAYS_CXX_LINK)
    set(oneValueArgs TARGET MMAKE_ID DIRECTORY INSTALL_DIR)
    set(multiValueArgs SOURCES CXX_SOURCES OBJC_SOURCES ASM_SOURCES
        LIBS USELIBS INCLUDES ARCH_INCLUDES
        DEFINES UNDEFINES COMPILE_OPTIONS ARCH_SOURCES
        ARCH_DEFINES ARCH_COMPILE_OPTIONS LINK_OPTIONS
        DRIVER_LINK_OPTIONS ISA_LINK_OPTIONS)
    cmake_parse_arguments(ARG "${options}" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if((NOT ARG_SOURCES AND NOT ARG_CXX_SOURCES AND
        NOT ARG_OBJC_SOURCES AND NOT ARG_ASM_SOURCES AND
        NOT ARG_ARCH_SOURCES) OR
       NOT ARG_MMAKE_ID)
        return()
    endif()

    aros_resolve_source_lanes(RESOLVED_SOURCES "${ARG_DIRECTORY}"
        MMAKE_ID "${ARG_MMAKE_ID}"
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES})
    if(ARG_ARCH_SOURCES)
        aros_resolve_arch_sources(_ARCH_RESOLVED _ARCH_DROPPED "${ARG_DIRECTORY}"
            SOURCES ${RESOLVED_SOURCES}
            ARCH_SOURCES ${ARG_ARCH_SOURCES}
        )
        if(_ARCH_RESOLVED)
            set(RESOLVED_SOURCES "${_ARCH_RESOLVED}")
            list(REMOVE_ITEM RESOLVED_SOURCES "")
        endif()
    endif()
    aros_report_empty_concrete_target(
        MMAKE_ID "${ARG_MMAKE_ID}"
        DIRECTORY "${ARG_DIRECTORY}"
        RESOLVED_SOURCES ${RESOLVED_SOURCES}
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES}
        ARCH_SOURCES ${ARG_ARCH_SOURCES})
    aros_mark_preprocessed_asm(${RESOLVED_SOURCES})

    # A declaration carrying a linker script is not an AROS module but a
    # standalone executable, and cannot go through the ordinary ld.lld -r rule.
    # cmake/StandaloneLink.cmake.
    aros_standalone_link_wanted(_standalone ${ARG_DRIVER_LINK_OPTIONS})
    if(RESOLVED_SOURCES AND _standalone)
        aros_program_output_dir(_prog_outdir "${ARG_DIRECTORY}"
            "${ARG_INSTALL_DIR}")
        set(_objects "${ARG_MMAKE_ID}-objs")
        add_library("${_objects}" OBJECT ${RESOLVED_SOURCES})
        set_target_properties("${_objects}" PROPERTIES LINKER_LANGUAGE C)
        target_compile_definitions("${_objects}" PRIVATE
            LC_LIBDEFS_FILE="${ARG_TARGET}_libdefs.h"
            __AROS_PROGNAME__=${ARG_TARGET}
            __AROS_MODNAME__=${ARG_TARGET})
        # The ISA options belong to the compilation as well: this declaration
        # builds for a different architecture than the rest of the tree.
        if(ARG_ISA_LINK_OPTIONS)
            target_compile_options("${_objects}" PRIVATE
                ${ARG_ISA_LINK_OPTIONS})
            # A %rule_link_binary attached to this program has to compile for
            # the same architecture; its own declaration does not say which.
            set_property(GLOBAL PROPERTY
                "AROS_ISA_OPTIONS_${ARG_MMAKE_ID}" "${ARG_ISA_LINK_OPTIONS}")
        endif()
        aros_gate_arch("${_objects}" "${ARG_DIRECTORY}")
        aros_apply_includes("${_objects}"
            MODULE_DIR "${ARG_DIRECTORY}"
            INCLUDES ${ARG_INCLUDES}
            ARCH_INCLUDES ${ARG_ARCH_INCLUDES})
        aros_apply_flags("${_objects}"
            DEFINES ${ARG_DEFINES}
            UNDEFINES ${ARG_UNDEFINES}
            COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
            ARCH_DEFINES ${ARG_ARCH_DEFINES}
            ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS})
        aros_declare_standalone_link(
            NAME "${ARG_MMAKE_ID}"
            OBJECTS "${_objects}"
            OUTPUT "${_prog_outdir}/${ARG_TARGET}"
            USELIBS ${ARG_USELIBS}
            LINK_OPTIONS ${ARG_LINK_OPTIONS}
            DRIVER_LINK_OPTIONS ${ARG_DRIVER_LINK_OPTIONS}
            ISA_LINK_OPTIONS ${ARG_ISA_LINK_OPTIONS})
        return()
    endif()

    if(RESOLVED_SOURCES)
        add_executable(${ARG_MMAKE_ID} ${RESOLVED_SOURCES})
        target_compile_definitions(${ARG_MMAKE_ID} PRIVATE
            LC_LIBDEFS_FILE="${ARG_TARGET}_libdefs.h"
            __AROS_PROGNAME__=${ARG_TARGET}
            __AROS_MODNAME__=${ARG_TARGET}
        )
        aros_program_output_dir(_prog_outdir "${ARG_DIRECTORY}"
            "${ARG_INSTALL_DIR}")
        set_target_properties(${ARG_MMAKE_ID} PROPERTIES
            # progname, not the mmake id: the reference installs
            # aros-tcpip-apps-syslog as SysLog. The per-directory output
            # location mirrors targetdir="$(AROSDIR)/$(CURDIR)"; a flat one
            # collides, since two mmakefiles both build `testboot`.
            OUTPUT_NAME "${ARG_TARGET}"
            RUNTIME_OUTPUT_DIRECTORY "${_prog_outdir}")
        _aros_set_module_linker_language("${ARG_MMAKE_ID}"
            "${ARG_ALWAYS_CXX_LINK}"
            CXX_SOURCES ${ARG_CXX_SOURCES})
        aros_gate_arch(${ARG_MMAKE_ID} "${ARG_DIRECTORY}")
        aros_apply_includes(${ARG_MMAKE_ID}
            MODULE_DIR "${ARG_DIRECTORY}"
            INCLUDES ${ARG_INCLUDES}
            ARCH_INCLUDES ${ARG_ARCH_INCLUDES}
        )
        aros_apply_flags(${ARG_MMAKE_ID}
            DEFINES ${ARG_DEFINES}
            UNDEFINES ${ARG_UNDEFINES}
            COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
            ARCH_DEFINES ${ARG_ARCH_DEFINES}
            ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS}
        )
        aros_apply_link_options(${ARG_MMAKE_ID} ${ARG_LINK_OPTIONS})
        if(ARG_LIBS)
            aros_link_libraries(${ARG_MMAKE_ID} ${ARG_LIBS})
        endif()
    endif()
endfunction()

# aros_add_custom_target(TARGET <name> MMAKE_ID <id> MODTYPE <type> ...)
#
# A module whose modtype the transpiler has no dedicated variant for. This was
# an empty stub, and 97 declarations with 313 source files routed into it:
# every filesystem handler, 30 USB classes, 40 Zune/MUI classes, 9 Reaction
# classes. No output and no report, which is why kernel-package-base could not
# find kernel-fs-con or kernel-fs-ram and kernel-package-fs was missing four of
# its five members.
#
# In the reference, modtype supplies the default file suffix and install
# directory (config/make.tmpl:2048-2095), while modsuffix can replace that
# suffix; the compilation is identical to modtype=library. So this builds like
# aros_add_library and differs only in its runtime output properties.
function(aros_add_custom_target)
    set(options ALWAYS_CXX_LINK)
    set(oneValueArgs TARGET MMAKE_ID DIRECTORY MODTYPE INSTALL_DIR MODSUFFIX)
    set(multiValueArgs SOURCES CXX_SOURCES OBJC_SOURCES ASM_SOURCES
        LIBS USELIBS INCLUDES ARCH_INCLUDES
        DEFINES UNDEFINES COMPILE_OPTIONS ARCH_SOURCES
        ARCH_DEFINES ARCH_COMPILE_OPTIONS LINK_OPTIONS)
    cmake_parse_arguments(ARG "${options}" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if((NOT ARG_SOURCES AND NOT ARG_CXX_SOURCES AND
        NOT ARG_OBJC_SOURCES AND NOT ARG_ASM_SOURCES AND
        NOT ARG_ARCH_SOURCES) OR
       NOT ARG_MMAKE_ID)
        return()
    endif()

    # Install location per modtype, from make.tmpl:2048-2095.  `class` is the
    # Reaction class spelling supplied by genmodule and shares AROS_CLASSES.
    set(_moddir "${AROS_LIBS_DIR}")
    if(ARG_MODTYPE STREQUAL "handler")
        set(_moddir "${AROS_FS_DIR}")
    elseif(ARG_MODTYPE STREQUAL "device")
        set(_moddir "${AROS_DEVS_DIR}")
    elseif(ARG_MODTYPE STREQUAL "resource" OR ARG_MODTYPE STREQUAL "hook")
        set(_moddir "${AROS_RESOURCES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "hidd")
        set(_moddir "${AROS_DRIVERS_DIR}")
    elseif(ARG_MODTYPE STREQUAL "datatype")
        set(_moddir "${AROS_DATATYPES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "gadget")
        set(_moddir "${AROS_GADGETS_DIR}")
    elseif(ARG_MODTYPE STREQUAL "image")
        set(_moddir "${AROS_CLASSIMAGES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "mui" OR ARG_MODTYPE STREQUAL "mcc"
           OR ARG_MODTYPE STREQUAL "mcp")
        set(_moddir "${AROS_ZUNE_CLASSES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "usbclass")
        set(_moddir "${AROS_USB_CLASSES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "btclass")
        set(_moddir "${AROS_BLUETOOTH_CLASSES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "class")
        set(_moddir "${AROS_CLASSES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "printer")
        set(_moddir "${AROS_PRINTERS_DIR}")
    endif()
    _aros_module_install_dir(_moddir "${_moddir}" "${ARG_INSTALL_DIR}")

    # genmodule maps usbclass and btclass to the runtime suffix `.class` even
    # when no explicit modsuffix was supplied. Other full modules default to
    # their modtype. The mmake id remains the basename until the known duplicate
    # modname outputs can be represented without generating duplicate rules.
    if(ARG_MODTYPE STREQUAL "usbclass" OR ARG_MODTYPE STREQUAL "btclass")
        set(_default_modsuffix "class")
    elseif(ARG_MODTYPE STREQUAL "printer")
        set(_default_modsuffix "")
    else()
        set(_default_modsuffix "${ARG_MODTYPE}")
    endif()
    _aros_module_output_name(_outname "${ARG_MMAKE_ID}"
        "${_default_modsuffix}" "${ARG_MODSUFFIX}")

    aros_resolve_source_lanes(RESOLVED_SOURCES "${ARG_DIRECTORY}"
        MMAKE_ID "${ARG_MMAKE_ID}"
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES})
    if(ARG_ARCH_SOURCES)
        aros_resolve_arch_sources(_ARCH_RESOLVED _ARCH_DROPPED "${ARG_DIRECTORY}"
            SOURCES ${RESOLVED_SOURCES}
            ARCH_SOURCES ${ARG_ARCH_SOURCES}
        )
        if(_ARCH_RESOLVED)
            set(RESOLVED_SOURCES "${_ARCH_RESOLVED}")
            list(REMOVE_ITEM RESOLVED_SOURCES "")
        endif()
    endif()
    aros_report_empty_concrete_target(
        MMAKE_ID "${ARG_MMAKE_ID}"
        DIRECTORY "${ARG_DIRECTORY}"
        RESOLVED_SOURCES ${RESOLVED_SOURCES}
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES}
        ARCH_SOURCES ${ARG_ARCH_SOURCES})
    if(NOT RESOLVED_SOURCES)
        return()
    endif()
    aros_module_scaffolding(_scaffold_sources _scaffold
        MODTYPE "${ARG_MODTYPE}"
        TARGET "${ARG_TARGET}"
        MMAKE_ID "${ARG_MMAKE_ID}"
        DIRECTORY "${ARG_DIRECTORY}"
        MODSUFFIX "${ARG_MODSUFFIX}")
    aros_place_module_scaffolding(RESOLVED_SOURCES "${_scaffold_sources}")
    aros_mark_preprocessed_asm(${RESOLVED_SOURCES})

    add_executable(${ARG_MMAKE_ID} ${RESOLVED_SOURCES})
    target_compile_definitions(${ARG_MMAKE_ID} PRIVATE
        LC_LIBDEFS_FILE="${ARG_TARGET}_libdefs.h"
        __AROS_MODNAME__=${ARG_TARGET}
    )
    aros_attach_module_scaffolding("${ARG_MMAKE_ID}" _scaffold
        "${ARG_DIRECTORY}" "${ARG_TARGET}")
    set_target_properties(${ARG_MMAKE_ID} PROPERTIES
        OUTPUT_NAME "${_outname}"
        RUNTIME_OUTPUT_DIRECTORY "${_moddir}")
    _aros_set_module_linker_language("${ARG_MMAKE_ID}"
        "${ARG_ALWAYS_CXX_LINK}"
        CXX_SOURCES ${ARG_CXX_SOURCES})
    aros_gate_arch(${ARG_MMAKE_ID} "${ARG_DIRECTORY}")
    aros_apply_includes(${ARG_MMAKE_ID}
        MODULE_DIR "${ARG_DIRECTORY}"
        INCLUDES ${ARG_INCLUDES}
        ARCH_INCLUDES ${ARG_ARCH_INCLUDES}
    )
    aros_apply_flags(${ARG_MMAKE_ID}
        DEFINES ${ARG_DEFINES}
        UNDEFINES ${ARG_UNDEFINES}
        COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
        ARCH_DEFINES ${ARG_ARCH_DEFINES}
        ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS}
    )
    aros_apply_link_options(${ARG_MMAKE_ID} ${ARG_LINK_OPTIONS})
    if(ARG_LIBS)
        aros_link_libraries(${ARG_MMAKE_ID} ${ARG_LIBS})
    endif()
endfunction()

# =============================================================================
# Bootable Image & Distribution Targets
# =============================================================================
find_program(MKISOFS_BIN mkisofs HINTS "/opt/homebrew/bin" "/usr/bin" "/usr/local/bin")
find_program(HDIUTIL_BIN hdiutil HINTS "/usr/bin")

set(AROS_BOOT_ISO "${CMAKE_BINARY_DIR}/aros-${AROS_TARGET_CPU}-${AROS_TARGET_PLATFORM}.iso")

if(MKISOFS_BIN)
    add_custom_target(boot-iso
        COMMAND ${CMAKE_COMMAND} -E make_directory "${CMAKE_BINARY_DIR}/SYS/S"
        COMMAND ${CMAKE_COMMAND} -E copy_if_different "${CMAKE_SOURCE_DIR}/workbench/s/Startup-Sequence" "${CMAKE_BINARY_DIR}/SYS/S/Startup-Sequence"
        COMMAND ${MKISOFS_BIN} -o "${AROS_BOOT_ISO}"
                -V "AROS Live CD"
                -p "The AROS Dev Team"
                -iso-level 4 -l -J -r
                "${CMAKE_BINARY_DIR}/SYS"
        DEPENDS workbench-c
        COMMENT "💿 Packaging AROS-NX Bootable ISO Disk Image -> ${AROS_BOOT_ISO}"
    )
elseif(HDIUTIL_BIN)
    add_custom_target(boot-iso
        COMMAND ${CMAKE_COMMAND} -E make_directory "${CMAKE_BINARY_DIR}/SYS/S"
        COMMAND ${CMAKE_COMMAND} -E copy_if_different "${CMAKE_SOURCE_DIR}/workbench/s/Startup-Sequence" "${CMAKE_BINARY_DIR}/SYS/S/Startup-Sequence"
        COMMAND ${HDIUTIL_BIN} makehybrid -iso -joliet -o "${AROS_BOOT_ISO}" "${CMAKE_BINARY_DIR}/SYS"
        DEPENDS workbench-c
        COMMENT "💿 Packaging AROS-NX Bootable ISO Disk Image via hdiutil -> ${AROS_BOOT_ISO}"
    )
endif()

# =============================================================================
# Kickstart Packages (PKG containers)
# =============================================================================
#
# The 32-bit bootstrap (arch/all-pc/bootstrap/bootstrap.c, AddModule) accepts a
# Multiboot module in one of three shapes: a bare relocatable ELF, an ar(1)
# archive, or a PKG container. PKG is what the boot configuration in
# arch/x86_64-pc/boot/modules.default lists, and aros-romtool builds it.
#
# Note on naming: upstream's /boot/pc/kernel is NOT a PKG. config/make.tmpl
# builds it with %link_kickstart, which links kernel_resource.o + exec + task
# into a single relocatable ELF. That first module supplies the kickstart entry
# point, because elfloader.c takes the first executable section of the first
# module it sees (see `need_entry` in bootstrap/elfloader.c). Only the driver
# and library collections (aros-bsp, aros-acpi, aros-base, aros-fs, poseidon)
# are PKG containers. Load order therefore matters: the kernel ELF must come
# first, packages after it.

if(NOT AROS_ROMTOOL_BIN)
    find_program(AROS_ROMTOOL_BIN NAMES aros-romtool)
endif()

# aros_make_package(NAME <target> OUTPUT <file>
#                   MODULES <targets...> MEMBER_NAMES <runtime names...>)
#
# Packs the build products of the given CMake targets into a PKG container, in
# the order given. MEMBER_NAMES is positionally aligned with MODULES and gives
# each member's canonical runtime basename; target output names are deliberately
# not used because CMake target ids still disambiguate some duplicate modules.
# Targets that do not exist in this configuration are skipped together with
# their aligned name and reported, so a partial module tree still produces a
# usable package rather than a configure-time error.

# aros_package_arch_matches(<arch-dir>)
#
# Whether a package declared under arch/<arch-dir> belongs to this
# configuration. Same rule as AROS_ARCH_SOURCE_DIRS: three architectures
# declare $(AROSARCHDIR)/aros-bsp.pkg, and all three render to the same path,
# so only one of them may build it.
function(aros_package_arch_matches out_var arch_dir)
    if(NOT arch_dir)
        set(${out_var} TRUE PARENT_SCOPE)
        return()
    endif()
    if(arch_dir IN_LIST AROS_ARCH_PACKAGE_DIRS)
        set(${out_var} TRUE PARENT_SCOPE)
    else()
        set(${out_var} FALSE PARENT_SCOPE)
    endif()
endfunction()

function(aros_make_package)
    set(oneValueArgs NAME OUTPUT ARCH)
    set(multiValueArgs MODULES MEMBER_NAMES)
    cmake_parse_arguments(ARG "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    aros_package_arch_matches(_arch_ok "${ARG_ARCH}")
    if(NOT _arch_ok)
        return()
    endif()

    if(NOT ARG_NAME OR NOT ARG_OUTPUT)
        message(FATAL_ERROR "aros_make_package: NAME and OUTPUT are required")
    endif()

    list(LENGTH ARG_MODULES _module_count)
    list(LENGTH ARG_MEMBER_NAMES _member_name_count)
    if(NOT _module_count EQUAL _member_name_count)
        message(FATAL_ERROR
            "aros_make_package(${ARG_NAME}): MODULES has ${_module_count} item(s), "
            "but MEMBER_NAMES has ${_member_name_count}; the lists must be positionally aligned")
    endif()

    if(NOT AROS_ROMTOOL_BIN)
        message(STATUS "📦 ${ARG_NAME}: skipped, aros-romtool not built yet")
        return()
    endif()

    # Only targets that actually produce a file can be packaged. Meta-targets
    # created by the transpiler for #MM rules are UTILITY targets and have no
    # TARGET_FILE, so they are reported as not configured rather than breaking
    # the generate step.
    set(PRESENT "")
    set(PRESENT_NAMES "")
    set(PRESENT_INDICES "")
    set(MISSING "")
    if(_module_count GREATER 0)
        math(EXPR _module_last "${_module_count} - 1")
        foreach(_index RANGE 0 ${_module_last})
            list(GET ARG_MODULES ${_index} mod)
            list(GET ARG_MEMBER_NAMES ${_index} member_name)

            # A member name becomes a path below the private staging root. It
            # must remain one basename; per-index subdirectories allow repeated
            # canonical names without two producers writing the same file.
            if(NOT member_name MATCHES "^[-A-Za-z0-9_.+]+$"
               OR member_name STREQUAL "." OR member_name STREQUAL "..")
                message(FATAL_ERROR
                    "aros_make_package(${ARG_NAME}): unsafe MEMBER_NAMES item "
                    "at index ${_index}: '${member_name}'")
            endif()

            set(has_file FALSE)
            if(TARGET "${mod}")
                get_target_property(mod_type "${mod}" TYPE)
                if(mod_type STREQUAL "EXECUTABLE"
                   OR mod_type STREQUAL "STATIC_LIBRARY"
                   OR mod_type STREQUAL "SHARED_LIBRARY"
                   OR mod_type STREQUAL "MODULE_LIBRARY")
                    set(has_file TRUE)
                endif()
            endif()
            if(has_file)
                list(APPEND PRESENT "${mod}")
                list(APPEND PRESENT_NAMES "${member_name}")
                list(APPEND PRESENT_INDICES "${_index}")
            else()
                list(APPEND MISSING "${mod}")
            endif()
        endforeach()
    endif()
    aros_record_load_set(NAME "${ARG_NAME}" KIND package MEMBERS ${PRESENT})

    if(NOT PRESENT)
        message(STATUS "📦 ${ARG_NAME}: skipped, none of its modules are configured")
        return()
    endif()

    # Report what is not in the package, so an incomplete kickstart is visible
    # at configure time instead of failing mysteriously at boot.
    if(MISSING)
        list(LENGTH MISSING n_missing)
        list(LENGTH PRESENT n_present)
        message(STATUS
            "📦 ${ARG_NAME}: ${n_present} module(s) packaged, ${n_missing} not configured: ${MISSING}")
    endif()

    get_filename_component(OUT_DIR "${ARG_OUTPUT}" DIRECTORY)

    # Stage each target under the runtime name the package declaration uses.
    # The hashed, package-private root cannot be redirected by NAME or OUTPUT;
    # an index directory keeps duplicate canonical names distinct while
    # --basename records only MEMBER_NAMES in the container.
    string(SHA256 _stage_key "${ARG_NAME}|${ARG_OUTPUT}")
    set(_stage_root
        "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/aros-package-stage/${_stage_key}")
    set(STAGED_FILES "")
    set(STAGE_COMMANDS "")
    list(LENGTH PRESENT _present_count)
    math(EXPR _present_last "${_present_count} - 1")
    foreach(_present_index RANGE 0 ${_present_last})
        list(GET PRESENT ${_present_index} mod)
        list(GET PRESENT_NAMES ${_present_index} member_name)
        list(GET PRESENT_INDICES ${_present_index} original_index)
        set(_member_stage_dir "${_stage_root}/${original_index}")
        set(_staged_file "${_member_stage_dir}/${member_name}")
        list(APPEND STAGED_FILES "${_staged_file}")
        list(APPEND STAGE_COMMANDS
            COMMAND ${CMAKE_COMMAND} -E make_directory "${_member_stage_dir}"
            COMMAND ${CMAKE_COMMAND} -E copy_if_different
                    "$<TARGET_FILE:${mod}>" "${_staged_file}")
    endforeach()

    add_custom_command(
        OUTPUT "${ARG_OUTPUT}"
        BYPRODUCTS ${STAGED_FILES}
        COMMAND ${CMAKE_COMMAND} -E make_directory "${OUT_DIR}"
        ${STAGE_COMMANDS}
        COMMAND "${AROS_ROMTOOL_BIN}" pkg create --basename
                -o "${ARG_OUTPUT}" ${STAGED_FILES}
        DEPENDS ${PRESENT}
        COMMENT "📦 Packing kickstart package ${ARG_NAME}"
        VERBATIM
        COMMAND_EXPAND_LISTS
    )

    # The mmake name is usually also a metatarget from the #MM rules, so a
    # target under that name may already exist. Attach to it rather than
    # declaring a second one.
    if(TARGET ${ARG_NAME})
        add_custom_target(${ARG_NAME}-file DEPENDS "${ARG_OUTPUT}")
        add_dependencies(${ARG_NAME} ${ARG_NAME}-file)
    else()
        add_custom_target(${ARG_NAME} DEPENDS "${ARG_OUTPUT}")
    endif()

    get_property(_pkgs GLOBAL PROPERTY AROS_PACKAGE_TARGETS)
    list(APPEND _pkgs ${ARG_NAME})
    set_property(GLOBAL PROPERTY AROS_PACKAGE_TARGETS "${_pkgs}")
endfunction()

# The kickstart aggregate lives in cmake/Kickstart.cmake, which is
# included from CMakeLists.txt AFTER generated_targets.cmake, because
# aros_make_package() needs the module targets to already exist.

# aros_program_output_dir(<out-var> <source-directory> [requested-directory])
#
# Programs go into a directory mirroring their source location, which is what
# targetdir="$(AROSDIR)/$(CURDIR)" does in the reference. A single flat
# directory does not work: two %build_progs groups both build a program called
# `version`, and ninja refuses two rules writing the same output.
function(aros_program_output_dir out_var directory requested_directory)
    if(requested_directory)
        if(IS_ABSOLUTE "${requested_directory}")
            # DirVars already rendered Make's absolute output locations to the
            # corresponding CMake build paths. Preserve those verbatim; in
            # particular, host tools live outside the target system tree.
            set(${out_var} "${requested_directory}" PARENT_SCOPE)
        else()
            # A literal relative targetdir is relative to AROSDIR in the
            # program macros, whose counterpart in this build is SYS.
            set(_requested "${AROS_SYS_DIR}/${requested_directory}")
            cmake_path(NORMAL_PATH _requested)
            set(${out_var} "${_requested}" PARENT_SCOPE)
        endif()
        return()
    endif()

    file(RELATIVE_PATH _rel "${CMAKE_SOURCE_DIR}" "${directory}")
    if(NOT _rel OR _rel MATCHES "^\\.\\.")
        set(${out_var} "${AROS_C_DIR}" PARENT_SCOPE)
    else()
        set(${out_var} "${AROS_C_DIR}/${_rel}" PARENT_SCOPE)
    endif()
endfunction()

# =============================================================================
# The remaining link kinds
# =============================================================================

# aros_add_programs(MMAKE_ID <id> DIRECTORY <dir> SOURCES <file>... ...)
#
# %build_progs: one executable per source file, all under a single mmake name
# (make.tmpl:1850). %build_prog, by contrast, links one executable from all its
# sources. Both were previously treated as the second case, which produced one
# binary where the tree wants several.
#
# Each file gets its own CMake target, named "<mmake-id>-<stem>" so the ids stay
# unique, with the plain stem as the output name. A phony target under the mmake
# id ties them together, which is what the historic build's metatarget does.
function(aros_add_programs)
    set(oneValueArgs TARGET MMAKE_ID DIRECTORY INSTALL_DIR)
    set(multiValueArgs SOURCES CXX_SOURCES OBJC_SOURCES ASM_SOURCES
        LIBS USELIBS INCLUDES ARCH_INCLUDES
        DEFINES UNDEFINES COMPILE_OPTIONS ARCH_SOURCES
        ARCH_DEFINES ARCH_COMPILE_OPTIONS LINK_OPTIONS)
    cmake_parse_arguments(ARG "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if((NOT ARG_SOURCES AND NOT ARG_CXX_SOURCES AND
        NOT ARG_OBJC_SOURCES AND NOT ARG_ASM_SOURCES AND
        NOT ARG_ARCH_SOURCES) OR
       NOT ARG_MMAKE_ID)
        return()
    endif()

    set(_source_entries "")
    foreach(src IN LISTS ARG_SOURCES)
        list(APPEND _source_entries "C|${src}")
    endforeach()
    foreach(src IN LISTS ARG_CXX_SOURCES)
        list(APPEND _source_entries "CXX|${src}")
    endforeach()
    foreach(src IN LISTS ARG_OBJC_SOURCES)
        list(APPEND _source_entries "OBJC|${src}")
    endforeach()
    foreach(src IN LISTS ARG_ASM_SOURCES)
        list(APPEND _source_entries "ASM|${src}")
    endforeach()

    set(_members "")
    foreach(_entry IN LISTS _source_entries)
        string(FIND "${_entry}" "|" _separator)
        string(SUBSTRING "${_entry}" 0 ${_separator} _language)
        math(EXPR _source_start "${_separator} + 1")
        string(SUBSTRING "${_entry}" ${_source_start} -1 src)
        aros_resolve_sources(_resolved "${ARG_DIRECTORY}"
            LANGUAGE "${_language}" MMAKE_ID "${ARG_MMAKE_ID}"
            SOURCES "${src}")
        if(NOT _resolved)
            continue()
        endif()
        get_filename_component(_stem "${src}" NAME_WE)
        set(_tgt "${ARG_MMAKE_ID}-${_stem}")
        if(TARGET ${_tgt})
            continue()
        endif()

        aros_mark_preprocessed_asm(${_resolved})
        add_executable(${_tgt} ${_resolved})
        aros_program_output_dir(_outdir "${ARG_DIRECTORY}"
            "${ARG_INSTALL_DIR}")
        set_target_properties(${_tgt} PROPERTIES
            OUTPUT_NAME "${_stem}"
            RUNTIME_OUTPUT_DIRECTORY "${_outdir}")
        if(_language STREQUAL "CXX")
            set(_member_cxx_sources "${src}")
        else()
            set(_member_cxx_sources "")
        endif()
        _aros_set_module_linker_language("${_tgt}" ""
            CXX_SOURCES ${_member_cxx_sources})
        aros_gate_arch(${_tgt} "${ARG_DIRECTORY}")
        aros_apply_includes(${_tgt}
            MODULE_DIR "${ARG_DIRECTORY}"
            INCLUDES ${ARG_INCLUDES}
            ARCH_INCLUDES ${ARG_ARCH_INCLUDES}
        )
        aros_apply_flags(${_tgt}
            DEFINES ${ARG_DEFINES}
            UNDEFINES ${ARG_UNDEFINES}
            COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
            ARCH_DEFINES ${ARG_ARCH_DEFINES}
            ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS}
        )
        aros_apply_link_options(${_tgt} ${ARG_LINK_OPTIONS})
        if(ARG_LIBS)
            aros_link_libraries(${_tgt} ${ARG_LIBS})
        endif()
        list(APPEND _members ${_tgt})
    endforeach()

    # %build_progs bypasses the combined lane resolver because every source is
    # a separate executable. The aggregate declaration still needs the same
    # visibility guarantee when every member source is discarded.
    aros_report_empty_concrete_target(
        MMAKE_ID "${ARG_MMAKE_ID}"
        DIRECTORY "${ARG_DIRECTORY}"
        RESOLVED_SOURCES ${_members}
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES}
        ARCH_SOURCES ${ARG_ARCH_SOURCES})
    if(_members)
        if(NOT TARGET ${ARG_MMAKE_ID})
            add_custom_target(${ARG_MMAKE_ID} DEPENDS ${_members})
        endif()
        set_property(TARGET ${ARG_MMAKE_ID} PROPERTY
            AROS_PROGRAM_GROUP_MEMBERS "${_members}")
    endif()
endfunction()

# aros_add_module_simple(TARGET <name> MODTYPE <type> ...)
#
# %build_module_simple links a module without the genmodule chain: no .conf, so
# no generated libdefs header and no LC_LIBDEFS_FILE. Defining it anyway would
# point the module at a header that is never generated.
#
# The extension follows modtype, which is a required argument there, except
# that printer modules are suffixless. The 28 declarations in the tree use
# mcc, resource, library, mcp, hook and printer.
function(aros_add_module_simple)
    set(options ALWAYS_CXX_LINK)
    set(oneValueArgs TARGET MMAKE_ID DIRECTORY MODTYPE INSTALL_DIR MODSUFFIX)
    set(multiValueArgs SOURCES CXX_SOURCES OBJC_SOURCES ASM_SOURCES
        LIBS USELIBS INCLUDES ARCH_INCLUDES
        DEFINES UNDEFINES COMPILE_OPTIONS ARCH_SOURCES
        ARCH_DEFINES ARCH_COMPILE_OPTIONS LINK_OPTIONS)
    cmake_parse_arguments(ARG "${options}" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if((NOT ARG_SOURCES AND NOT ARG_CXX_SOURCES AND
        NOT ARG_OBJC_SOURCES AND NOT ARG_ASM_SOURCES AND
        NOT ARG_ARCH_SOURCES) OR
       NOT ARG_MMAKE_ID)
        return()
    endif()

    aros_resolve_source_lanes(RESOLVED_SOURCES "${ARG_DIRECTORY}"
        MMAKE_ID "${ARG_MMAKE_ID}"
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES})
    if(ARG_ARCH_SOURCES)
        aros_resolve_arch_sources(_ARCH_RESOLVED _ARCH_DROPPED "${ARG_DIRECTORY}"
            SOURCES ${RESOLVED_SOURCES}
            ARCH_SOURCES ${ARG_ARCH_SOURCES}
        )
        if(_ARCH_RESOLVED)
            set(RESOLVED_SOURCES "${_ARCH_RESOLVED}")
            list(REMOVE_ITEM RESOLVED_SOURCES "")
        endif()
    endif()
    aros_report_empty_concrete_target(
        MMAKE_ID "${ARG_MMAKE_ID}"
        DIRECTORY "${ARG_DIRECTORY}"
        RESOLVED_SOURCES ${RESOLVED_SOURCES}
        SOURCES ${ARG_SOURCES}
        CXX_SOURCES ${ARG_CXX_SOURCES}
        OBJC_SOURCES ${ARG_OBJC_SOURCES}
        ASM_SOURCES ${ARG_ASM_SOURCES}
        ARCH_SOURCES ${ARG_ARCH_SOURCES})
    if(NOT RESOLVED_SOURCES)
        return()
    endif()
    aros_mark_preprocessed_asm(${RESOLVED_SOURCES})

    set(_default_modsuffix "${ARG_MODTYPE}")
    if(NOT _default_modsuffix)
        set(_default_modsuffix "library")
    elseif(_default_modsuffix STREQUAL "printer")
        # Unlike full genmodule modules, simple printer modules have no suffix.
        set(_default_modsuffix "")
    endif()

    # build_module_simple uses the same default moduledir table as the full
    # module builder (make.tmpl:2048-2092).
    set(_default_install_dir "${AROS_LIBS_DIR}")
    if(ARG_MODTYPE STREQUAL "handler")
        set(_default_install_dir "${AROS_FS_DIR}")
    elseif(ARG_MODTYPE STREQUAL "device")
        set(_default_install_dir "${AROS_DEVS_DIR}")
    elseif(ARG_MODTYPE STREQUAL "resource" OR ARG_MODTYPE STREQUAL "hook")
        set(_default_install_dir "${AROS_RESOURCES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "hidd")
        set(_default_install_dir "${AROS_DRIVERS_DIR}")
    elseif(ARG_MODTYPE STREQUAL "datatype")
        set(_default_install_dir "${AROS_DATATYPES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "gadget")
        set(_default_install_dir "${AROS_GADGETS_DIR}")
    elseif(ARG_MODTYPE STREQUAL "image")
        set(_default_install_dir "${AROS_CLASSIMAGES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "mui" OR ARG_MODTYPE STREQUAL "mcc"
           OR ARG_MODTYPE STREQUAL "mcp")
        set(_default_install_dir "${AROS_ZUNE_CLASSES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "usbclass")
        set(_default_install_dir "${AROS_USB_CLASSES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "btclass")
        set(_default_install_dir "${AROS_BLUETOOTH_CLASSES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "class")
        set(_default_install_dir "${AROS_CLASSES_DIR}")
    elseif(ARG_MODTYPE STREQUAL "printer")
        set(_default_install_dir "${AROS_PRINTERS_DIR}")
    endif()
    # No scaffolding here. config/make.tmpl:1974 defines %build_module_simple
    # with no genmodule step at all: it links its own objects and nothing else.
    # Generating a start file for workbench/libs/gl added its whole 463-entry
    # function table as undefined references, because this declaration is an ABI
    # shell whose implementation lives in the Mesa port.
    _aros_module_install_dir(_install_dir
        "${_default_install_dir}" "${ARG_INSTALL_DIR}")
    _aros_module_output_name(_output_name "${ARG_MMAKE_ID}"
        "${_default_modsuffix}" "${ARG_MODSUFFIX}")

    add_executable(${ARG_MMAKE_ID} ${RESOLVED_SOURCES})
    target_compile_definitions(${ARG_MMAKE_ID} PRIVATE
        __AROS_MODNAME__=${ARG_TARGET}
    )
    set_target_properties(${ARG_MMAKE_ID} PROPERTIES
        # Named after the mmake id, as every other module builder here does;
        # known duplicate modnames would otherwise produce duplicate rules.
        OUTPUT_NAME "${_output_name}"
        RUNTIME_OUTPUT_DIRECTORY "${_install_dir}")
    _aros_set_module_linker_language("${ARG_MMAKE_ID}"
        "${ARG_ALWAYS_CXX_LINK}"
        CXX_SOURCES ${ARG_CXX_SOURCES})
    aros_gate_arch(${ARG_MMAKE_ID} "${ARG_DIRECTORY}")
    aros_apply_includes(${ARG_MMAKE_ID}
        MODULE_DIR "${ARG_DIRECTORY}"
        INCLUDES ${ARG_INCLUDES}
        ARCH_INCLUDES ${ARG_ARCH_INCLUDES}
    )
    aros_apply_flags(${ARG_MMAKE_ID}
        DEFINES ${ARG_DEFINES}
        UNDEFINES ${ARG_UNDEFINES}
        COMPILE_OPTIONS ${ARG_COMPILE_OPTIONS}
        ARCH_DEFINES ${ARG_ARCH_DEFINES}
        ARCH_COMPILE_OPTIONS ${ARG_ARCH_COMPILE_OPTIONS}
    )
    aros_apply_link_options(${ARG_MMAKE_ID} ${ARG_LINK_OPTIONS})
    if(ARG_LIBS)
        aros_link_libraries(${ARG_MMAKE_ID} ${ARG_LIBS})
    endif()
endfunction()

# aros_link_kickstart(NAME <id> OUTPUT <file> MODULES <target>... [USELIBS <l>...])
#
# %link_kickstart, from config/make.tmpl:3850. A few modules cannot be loaded
# from a package: the bootstrap takes its entry point from the first executable
# section of the first module it sees (arch/all-pc/bootstrap/elfloader.c:662),
# so kernel, exec and task are linked into one relocatable ELF that the
# bootstrap loads directly.
#
# MODULES arrives in declaration order with the startup module first, which is
# the order the reference links them in and the reason the entry point lands
# where the bootstrap expects it.
function(aros_link_kickstart)
    set(oneValueArgs NAME OUTPUT ARCH)
    set(multiValueArgs MODULES USELIBS)
    cmake_parse_arguments(ARG "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    aros_package_arch_matches(_arch_ok "${ARG_ARCH}")
    if(NOT _arch_ok)
        return()
    endif()

    if(NOT ARG_NAME OR NOT ARG_OUTPUT OR NOT ARG_MODULES)
        return()
    endif()

    set(PRESENT "")
    set(MISSING "")
    foreach(mod IN LISTS ARG_MODULES)
        set(has_file FALSE)
        if(TARGET ${mod})
            get_target_property(mod_type ${mod} TYPE)
            if(mod_type STREQUAL "EXECUTABLE" OR mod_type STREQUAL "STATIC_LIBRARY")
                set(has_file TRUE)
            endif()
        endif()
        if(has_file)
            list(APPEND PRESENT ${mod})
        else()
            list(APPEND MISSING ${mod})
        endif()
    endforeach()

    if(MISSING)
        # A kickstart missing a module links but does not boot, so say so now.
        message(WARNING
            "🧩 ${ARG_NAME}: cannot link, module(s) not configured: ${MISSING}")
        return()
    endif()

    aros_record_load_set(NAME "${ARG_NAME}" KIND kickstart MEMBERS ${PRESENT})

    # Each member contributes its kickstart object, not its loadable module:
    # the module carries the compiler spec's default link set and keeps its
    # library bases global, so linking three of those together fails on
    # duplicate LibNextTagItem and set_call_libfuncs. See
    # cmake/KickstartObjects.cmake.
    set(_objs "")
    set(_member_deps "")
    foreach(mod IN LISTS PRESENT)
        aros_kickstart_member_object(_member_object "${mod}")
        list(APPEND _objs "${_member_object}")
        # The object path, not the custom target: a DEPENDS on a generated file
        # is a real input edge, where a target is only ordering and left the
        # files absent at link time.
        if(TARGET "${mod}-kickstart-object")
            list(APPEND _member_deps "${_member_object}")
        else()
            list(APPEND _member_deps "${mod}")
        endif()
    endforeach()

    set(_libs "")
    foreach(l IN LISTS ARG_USELIBS)
        list(APPEND _libs "-l${l}")
    endforeach()

    # The reference links a kickstart with $(TARGET_CC) (config/make.tmpl:3904),
    # so the compiler spec's default set applies here, and its LDFLAGS carry
    # -nosysbase. That is where a member gets libamiga, libautoinit, liblibinit
    # and the C runtime, all of which are deliberately excluded from the
    # member's own object (config/make.tmpl:2752).
    # nostdc as well as nosysbase, which is a reading of the reference rather
    # than a transcription of it: config/make.tmpl:3899 passes only
    # `-static -nosysbase`.
    #
    # The reasoning is the same one that justifies -nosysbase. A kickstart runs
    # before any library exists, so it cannot call one. Without nostdc the spec
    # contributes -lstdc, whose strstr is a stub that loads StdCBase and jumps
    # through its LVO table (compiler/crt/stdc's generated
    # __strstr_StdCBase_wrapper). StdCBase is necessarily NULL in
    # kernel_cstart, which is the first code to run, so
    # arch/x86_64-pc/kernel/kernel_startup.c:358's strstr(cmdline, "vesahack")
    # faulted:
    #
    #   movabsq $0x138efa0, %r11 ; movq (%r11), %r11 ; jmpq *-0xb70(%r11)
    #   v=0e CR2=fffffffffffff490
    #
    # With nostdc the spec gives -lstdc.static instead, whose strstr is the
    # real implementation. That rom/exec states `uselibs="stdc.static"` for its
    # own object is the same choice, made one level down.
    aros_default_link_set_files(_default_files _default_deps nosysbase nostdc)
    list(APPEND _member_deps ${_default_deps})

    get_filename_component(_dir "${ARG_OUTPUT}" DIRECTORY)

    # Linked through aros-collect rather than the compiler driver, as every
    # module target here is: config/make.tmpl:3899 passes -Wl,-Ur, and driving
    # clang would hand the job to the host linker, which rejects that option.
    #
    # The two halves of -Ur are supplied separately. `-r` keeps the output
    # relocatable so the bootstrap can load it, which is all lld needs to be
    # told; the other half of what -Ur means to collect-aros is the second pass
    # that builds the symbol sets (collect-aros.c:188), and that is what
    # aros-collect does here.
    add_custom_command(
        OUTPUT "${ARG_OUTPUT}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_dir}"
        COMMAND "${AROS_COLLECT_BIN}" --ld "${AROS_LLD_BIN}"
                --report "${CMAKE_BINARY_DIR}/generated_targets.kickstart-sets.txt"
                -- -r
                -o "${ARG_OUTPUT}" ${_objs} ${_libs} ${_default_files}
        DEPENDS ${_member_deps}
        COMMENT "Kickstart ${ARG_NAME} -> ${ARG_OUTPUT}"
        VERBATIM
        COMMAND_EXPAND_LISTS)

    # The mmake name is usually also a metatarget from the #MM rules, so a
    # target under that name may already exist. Attach to it rather than
    # declaring a second one.
    if(TARGET ${ARG_NAME})
        add_custom_target(${ARG_NAME}-file DEPENDS "${ARG_OUTPUT}")
        add_dependencies(${ARG_NAME} ${ARG_NAME}-file)
    else()
        add_custom_target(${ARG_NAME} DEPENDS "${ARG_OUTPUT}")
    endif()

    get_property(_ks GLOBAL PROPERTY AROS_KICKSTART_TARGETS)
    list(APPEND _ks ${ARG_NAME})
    set_property(GLOBAL PROPERTY AROS_KICKSTART_TARGETS "${_ks}")
endfunction()
