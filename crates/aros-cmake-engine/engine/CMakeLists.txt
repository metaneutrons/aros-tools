cmake_minimum_required(VERSION 3.22)

# The supported top-level targets are bare-metal.  If a caller does not
# provide a toolchain/system explicitly, select CMake's generic platform
# before compiler detection so an Apple-Silicon host cannot inject its native
# `-arch arm64` flag into the target compiler invocation.  Keep an explicit
# caller choice intact for future hosted targets.
if(NOT DEFINED CMAKE_SYSTEM_NAME)
    set(CMAKE_SYSTEM_NAME Generic)
endif()

project(AROS-NX
    VERSION 0.1.0
    LANGUAGES C CXX ASM
)

# libc++ deliberately ships C-compatible wrapper headers such as math.h.
# They must precede the target SDK's C headers for C++ compilations so the
# wrapper can add the standard overloads and then reach AROS via include_next.
# C compilations continue to see the SDK directly.
if(AROS_CROSS_TOOLCHAIN_ROOT)
    # CMake otherwise suppresses this explicit directory because Clang also
    # reports it as implicit. The implicit search class comes after every -I
    # SDK directory, which is precisely the ordering we must override.
    list(REMOVE_ITEM CMAKE_CXX_IMPLICIT_INCLUDE_DIRECTORIES
        "${AROS_CROSS_TOOLCHAIN_ROOT}/include/c++/v1")
    include_directories(BEFORE
        "$<$<COMPILE_LANGUAGE:CXX>:${AROS_CROSS_TOOLCHAIN_ROOT}/include/c++/v1>")
    add_compile_options("$<$<COMPILE_LANGUAGE:CXX>:-nostdinc++>")

    # Target builders legitimately prepend their own SDK namespaces after the
    # directory default above. Finalise every compiling target only after the
    # complete generated graph has been declared, otherwise those later
    # BEFORE additions can place AROS' C math.h ahead of libc++'s wrapper.
    function(_aros_finalize_cxx_wrapper_order)
        get_property(_targets DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)
        set(_wrapper
            "$<$<COMPILE_LANGUAGE:CXX>:${AROS_CROSS_TOOLCHAIN_ROOT}/include/c++/v1>")
        foreach(_target IN LISTS _targets)
            get_target_property(_type "${_target}" TYPE)
            if(NOT _type MATCHES
               "^(EXECUTABLE|STATIC_LIBRARY|SHARED_LIBRARY|MODULE_LIBRARY|OBJECT_LIBRARY)$")
                continue()
            endif()
            get_target_property(_includes "${_target}" INCLUDE_DIRECTORIES)
            if(NOT _includes OR _includes STREQUAL "_includes-NOTFOUND")
                set(_includes "")
            endif()
            list(REMOVE_ITEM _includes "${_wrapper}")
            set_property(TARGET "${_target}" PROPERTY INCLUDE_DIRECTORIES
                "${_wrapper}" ${_includes})
        endforeach()
    endfunction()
    cmake_language(DEFER CALL _aros_finalize_cxx_wrapper_order)
endif()

include("${CMAKE_SOURCE_DIR}/cmake/ToolchainIdentity.cmake")
include("${CMAKE_SOURCE_DIR}/cmake/Executable.cmake")
include("${CMAKE_SOURCE_DIR}/cmake/ArosTools.cmake")
aros_configure_rust_tools()
aros_lock_build_tree_toolchain()

include("${CMAKE_SOURCE_DIR}/cmake/EnableLLVMObjectiveC.cmake")

message(STATUS "🚀 Initializing AROS-NX (v0.1.0) Build Engine...")
message(STATUS "Target: ${AROS_TARGET_CPU}-${AROS_TARGET_PLATFORM}")

# A target is described by two variables. AROS_TARGET_CPU is the instruction
# set (x86_64, aarch64, arm, m68k), AROS_TARGET_PLATFORM the machine (pc,
# raspi, amiga, darwin). Together they name the arch/ directories that
# AROS_ARCH_SOURCE_DIRS accepts.
#
# The historic build calls the second one AROS_TARGET_ARCH, which reads like a
# CPU name and is not one, so the transpiler maps an mmakefile's
# $(AROS_TARGET_ARCH) onto AROS_TARGET_PLATFORM (packages.rs:98). A cache entry
# of that name is therefore ambiguous, and nothing here reads it. It used to be
# in the presets holding a CPU value, where CMake reported it as unused and
# nobody noticed.
if(DEFINED AROS_TARGET_ARCH)
    message(WARNING
        "AROS_TARGET_ARCH is not read by this build and has been ignored. "
        "The historic build uses that name for what is AROS_TARGET_PLATFORM "
        "here; pass AROS_TARGET_CPU and AROS_TARGET_PLATFORM instead.")
endif()

# The reference configure pairs every x86_64 target with i386 for the few
# bootstrap rules that explicitly build a 32-bit companion (for example the
# GRUB EFI helper). Other target CPUs have no companion value.
if(AROS_TARGET_CPU STREQUAL "x86_64")
    set(AROS_TARGET_CPU32 "i386")
else()
    set(AROS_TARGET_CPU32 "")
endif()

# Values used by target-dependent MetaMake conditionals. Keep them as named
# cache settings so the transpiler receives configuration, never Rust-side
# architecture guesses. Bare-metal pc and raspi use an empty MetaMake FAMILY;
# hosted targets may later set it to unix. The four current presets pin both
# Clang and the LLVM MetaMake lane explicitly, so neither the compiler commands
# nor the generated graph depend on the host's default compiler. A release
# toolchain file overrides those names with its verified prefix-owned tools.
# MMU follows the reference default.
set(AROS_TARGET_VARIANT "" CACHE STRING "MetaMake target variant")
set(AROS_TARGET_FAMILY "" CACHE STRING
    "Historic MetaMake target family (empty for bare-metal targets)")
if(CMAKE_C_COMPILER_ID MATCHES "Clang")
    set(_aros_default_toolchain "llvm")
elseif(CMAKE_C_COMPILER_ID STREQUAL "GNU")
    set(_aros_default_toolchain "gnu")
else()
    set(_aros_default_toolchain "")
endif()
set(AROS_TOOLCHAIN "${_aros_default_toolchain}" CACHE STRING
    "MetaMake target toolchain family")
set_property(CACHE AROS_TOOLCHAIN PROPERTY STRINGS llvm gnu)
if(AROS_TARGET_PLATFORM STREQUAL "pc")
    set(_aros_default_bootloader "grub2gfx")
else()
    set(_aros_default_bootloader "")
endif()
set(AROS_TARGET_BOOTLOADER "${_aros_default_bootloader}" CACHE STRING
    "MetaMake target bootloader lane")
set_property(CACHE AROS_TARGET_BOOTLOADER PROPERTY STRINGS "" grub grub2 grub2gfx)
option(AROS_ENABLE_MMU "Include the MetaMake MMU kernel sources" ON)
set(GCC_CONFIG_FLOAT_ABI "" CACHE STRING
    "MetaMake ARM floating-point ABI (for example hard or soft)")

# MetaMake calls the compound machine/CPU selector AROS_TARGET_PLATFORM. Keep
# that legacy spelling distinct from this build's machine-only platform value
# so generated meta-target names expand unambiguously.
set(AROS_TARGET_LEGACY_PLATFORM
    "${AROS_TARGET_PLATFORM}-${AROS_TARGET_CPU}")

# 1. Compiler Cache Integration (sccache / ccache)
include("${CMAKE_SOURCE_DIR}/cmake/CompilerCache.cmake")

# 2. Target Platform Setup & Core Macros
include("${CMAKE_SOURCE_DIR}/cmake/AROS.cmake")

# 3. Tools that run on the build machine, and the header rules using them.
include("${CMAKE_SOURCE_DIR}/cmake/HostTools.cmake")
include("${CMAKE_SOURCE_DIR}/cmake/FlexCatSources.cmake")
include("${CMAKE_SOURCE_DIR}/cmake/IlbmSources.cmake")

# AHI is admitted only for the three audited LLVM target profiles.  Its
# configure-style build needs the historical Perl SFD compiler, which is
# materialised as a private host product rather than discovered through PATH.
# Keep the interpreter identity explicit so generated targets cannot silently
# select a different host implementation.
set(_aros_ahi_supported_profile FALSE)
if(AROS_ENABLE_MMU AND AROS_TOOLCHAIN STREQUAL "llvm")
    if(AROS_TARGET_CPU STREQUAL "x86_64" AND
       AROS_TARGET_PLATFORM STREQUAL "pc")
        set(_aros_ahi_supported_profile TRUE)
    elseif(AROS_TARGET_CPU STREQUAL "arm" AND
           AROS_TARGET_PLATFORM STREQUAL "raspi" AND
           GCC_CONFIG_FLOAT_ABI STREQUAL "hard")
        set(_aros_ahi_supported_profile TRUE)
    elseif(AROS_TARGET_CPU STREQUAL "aarch64" AND
           AROS_TARGET_PLATFORM STREQUAL "raspi")
        set(_aros_ahi_supported_profile TRUE)
    endif()
endif()
if(_aros_ahi_supported_profile)
    set(AROS_HOST_PERL "/usr/bin/perl")
    find_program(AROS_AHI_MAKE NAMES gmake make)
    set(AROS_AHI_MAKE "${AROS_AHI_MAKE}" CACHE FILEPATH
        "GNU make executable for the closed AHI build")
    aros_path_is_executable("${AROS_HOST_PERL}" _aros_host_perl_executable)
    if(NOT _aros_host_perl_executable)
        message(FATAL_ERROR
            "AHI requires the explicit executable Perl interpreter ${AROS_HOST_PERL}")
    endif()
    include("${CMAKE_SOURCE_DIR}/cmake/SfdcHostTool.cmake")
    include("${CMAKE_SOURCE_DIR}/cmake/AhiBuild.cmake")
    aros_build_host_sfdc(PERL "${AROS_HOST_PERL}")
endif()

include("${CMAKE_SOURCE_DIR}/cmake/Icons.cmake")
include("${CMAKE_SOURCE_DIR}/cmake/GeneratedHeaders.cmake")

# 4. Dynamic Target Transpilation
set(TRANSPILER_BIN "${AROS_TRANSPILER_BIN}")
set(GENERATED_TARGETS_CMAKE "${CMAKE_BINARY_DIR}/generated_targets.cmake")

aros_path_is_executable("${TRANSPILER_BIN}" _aros_transpiler_executable)
if(NOT _aros_transpiler_executable)
    message(FATAL_ERROR
        "AROS-NX requires the executable aros-transpiler at ${TRANSPILER_BIN}. "
        "Install a complete aros-tools release, or set AROS_RUST_TOOLS_DIR / "
        "AROS_TRANSPILER_BIN explicitly.")
endif()

message(STATUS "⚡ AROS-NX: Running Rust aros-transpiler on mmakefile tree...")
if(AROS_ENABLE_MMU)
    set(_aros_transpiler_use_mmu "1")
else()
    set(_aros_transpiler_use_mmu "0")
endif()
execute_process(
    COMMAND "${TRANSPILER_BIN}"
            "--source-dir" "${CMAKE_SOURCE_DIR}"
            "--output" "${GENERATED_TARGETS_CMAKE}"
            "--ports-dir" "${AROS_PORTS_DIR}"
            "--cpu" "${AROS_TARGET_CPU}"
            "--platform" "${AROS_TARGET_PLATFORM}"
            "--family" "${AROS_TARGET_FAMILY}"
            "--variant" "${AROS_TARGET_VARIANT}"
            "--toolchain" "${AROS_TOOLCHAIN}"
            "--cpu32" "${AROS_TARGET_CPU32}"
            "--use-mmu" "${_aros_transpiler_use_mmu}"
            "--float-abi" "${GCC_CONFIG_FLOAT_ABI}"
    RESULT_VARIABLE TRANSPILER_RES
    ERROR_VARIABLE _aros_transpiler_error
)
if(NOT TRANSPILER_RES EQUAL 0 OR NOT EXISTS "${GENERATED_TARGETS_CMAKE}")
    message(FATAL_ERROR
        "AROS-NX transpiler failed (${TRANSPILER_RES}) and did not produce "
        "${GENERATED_TARGETS_CMAKE}.\n${_aros_transpiler_error}")
endif()

# A Make wildcard below PORTSDIR cannot be turned into compilation units until
# its archive has been unpacked. The same applies to exact transitive header
# ownership when the public and private headers both live in fetched trees. The
# first cold transpiler pass writes only the relevant owning fetch declarations
# to a sidecar. Materialise those inventories, then replace the partial graph
# with a second pass before it is ever included.
set(_aros_source_inventory_manifest
    "${CMAKE_BINARY_DIR}/generated_targets.source-inventory.cmake")
include("${_aros_source_inventory_manifest}")
if(AROS_SOURCE_INVENTORY_FETCH_COUNT GREATER 0)
    math(EXPR _aros_source_inventory_last
        "${AROS_SOURCE_INVENTORY_FETCH_COUNT} - 1")
    foreach(_index RANGE 0 ${_aros_source_inventory_last})
        aros_fetch_source_inventory(
            NAME "${AROS_SOURCE_INVENTORY_FETCH_${_index}_NAME}"
            ARCHIVE "${AROS_SOURCE_INVENTORY_FETCH_${_index}_ARCHIVE}"
            SUFFIXES "${AROS_SOURCE_INVENTORY_FETCH_${_index}_SUFFIXES}"
            ORIGINS "${AROS_SOURCE_INVENTORY_FETCH_${_index}_ORIGINS}"
            CHECKSUMS "${AROS_SOURCE_INVENTORY_FETCH_${_index}_CHECKSUMS}"
            LOCATION "${AROS_SOURCE_INVENTORY_FETCH_${_index}_LOCATION}"
            DESTINATION "${AROS_SOURCE_INVENTORY_FETCH_${_index}_DESTINATION}"
            BASE "${AROS_SOURCE_INVENTORY_FETCH_${_index}_BASE}"
            PATCH_ORIGINS "${AROS_SOURCE_INVENTORY_FETCH_${_index}_PATCH_ORIGINS}"
            PATCHES "${AROS_SOURCE_INVENTORY_FETCH_${_index}_PATCHES}")
    endforeach()
    execute_process(
        COMMAND "${TRANSPILER_BIN}"
                "--source-dir" "${CMAKE_SOURCE_DIR}"
                "--output" "${GENERATED_TARGETS_CMAKE}"
                "--ports-dir" "${AROS_PORTS_DIR}"
                "--cpu" "${AROS_TARGET_CPU}"
                "--platform" "${AROS_TARGET_PLATFORM}"
                "--family" "${AROS_TARGET_FAMILY}"
                "--variant" "${AROS_TARGET_VARIANT}"
                "--toolchain" "${AROS_TOOLCHAIN}"
                "--cpu32" "${AROS_TARGET_CPU32}"
                "--use-mmu" "${_aros_transpiler_use_mmu}"
                "--float-abi" "${GCC_CONFIG_FLOAT_ABI}"
        RESULT_VARIABLE TRANSPILER_RES
        ERROR_VARIABLE _aros_transpiler_error)
    if(NOT TRANSPILER_RES EQUAL 0)
        message(FATAL_ERROR
            "AROS-NX transpiler failed after fetching source inventories "
            "(${TRANSPILER_RES}).\n${_aros_transpiler_error}")
    endif()
    include("${_aros_source_inventory_manifest}")
    if(AROS_SOURCE_INVENTORY_FETCH_COUNT GREATER 0)
        message(FATAL_ERROR
            "Fetched source inventories remain unresolved after the second transpiler pass")
    endif()
endif()

# What produced the file above, recorded so the run can be repeated outside
# CMake: `aros golden` replays this argv with a different --output to compare a
# changed transpiler against a captured baseline. The scoped arguments are
# derived during configuration, and they change the output, so a golden run
# that re-derived them would not be comparing the same run. OPEN-POINTS 13.
#
# The argv is written out twice rather than expanded from one list, because an
# unquoted CMake list discards its empty elements when it expands into a
# command, and three of these values are legitimately empty: `--variant ""` is
# how the transpiler is told "the ordinary variant". `aros golden capture`
# checks the record end to end by comparing its own first run against the file
# this run just wrote, so a record that drifts from the call is reported rather
# than trusted.
set(_aros_transpiler_record
    "--source-dir" "${CMAKE_SOURCE_DIR}"
    "--output" "${GENERATED_TARGETS_CMAKE}"
    "--ports-dir" "${AROS_PORTS_DIR}"
    "--cpu" "${AROS_TARGET_CPU}"
    "--platform" "${AROS_TARGET_PLATFORM}"
    "--family" "${AROS_TARGET_FAMILY}"
    "--variant" "${AROS_TARGET_VARIANT}"
    "--toolchain" "${AROS_TOOLCHAIN}"
    "--cpu32" "${AROS_TARGET_CPU32}"
    "--use-mmu" "${_aros_transpiler_use_mmu}"
    "--float-abi" "${GCC_CONFIG_FLOAT_ABI}")
string(REPLACE ";" "\n" _aros_transpiler_invocation "${_aros_transpiler_record}")
file(WRITE "${GENERATED_TARGETS_CMAKE}.invocation"
    "${_aros_transpiler_invocation}\n")
include("${CMAKE_SOURCE_DIR}/cmake/DefaultLinkSet.cmake")
include("${CMAKE_SOURCE_DIR}/cmake/BinaryObjects.cmake")
include("${CMAKE_SOURCE_DIR}/cmake/KickstartObjects.cmake")
include("${CMAKE_SOURCE_DIR}/cmake/StandaloneLink.cmake")
include("${CMAKE_SOURCE_DIR}/cmake/HostGeneratedHeaders.cmake")
include("${GENERATED_TARGETS_CMAKE}")

# Compiler depfiles cannot express the producer of a header which was absent
# when compilation started. Once the complete generated graph exists, attach
# ordinary source consumers to the %copy_includes owners they reference.
_aros_attach_source_header_dependencies()

# The target compiler's spec appends a default set of archives to every link,
# and our rule invokes ld.lld directly, so nothing else would. This runs here
# because generated_targets.cmake creates both the archives and the artefacts
# that consume them.
aros_apply_default_link_set()

# Architecture overrides that resolved to no file at all.
aros_report_arch_override_gaps()

# %rule_link_binary declarations this configuration could not build.
aros_report_binary_object_gaps()

# Kickstart members that could not get their own object.
aros_report_kickstart_gaps()

# Programs that are standalone executables rather than AROS modules.
aros_finalize_standalone_links()

# Companion-CPU archives can include headers written by host tools. Their
# declarations are discovered in either order, so bind them after the complete
# generated graph has been included.
aros_attach_host_header_dependencies()

# Host-tool headers that could not be generated.
aros_report_host_header_gaps()

# compiler/startup's raw object rules are outside the generated declarations.
# Locked C++ partial links explicitly consume cxx-startup.o, so make the
# historic aggregate publish that concrete sysroot object as well.
if(AROS_CROSS_TOOLCHAIN_ROOT)
    aros_bind_cxx_startup_target("linklibs-startup")
endif()

# The universal SDK deliberately exposes hostlib.resource's public API even
# when the selected runtime is not a hosted one. `rom/mmakefile.src` puts this
# exact includes target in `includes-generate`; without it, WinDebug and other
# source-tree consumers encounter a missing proto/hostlib.h long after the
# relevant generated-header rule was discarded by architecture filtering.
if(TARGET "includes-generate")
    if(NOT TARGET "kernel-hostlib-includes")
        add_custom_target("kernel-hostlib-includes")
    endif()
    aros_materialize_genmodule_headers(
        NAME kernel-hostlib-includes-generated
        OWNER kernel-hostlib-includes
        CONFIG "arch/all-hosted/hostlib/hostlib.conf"
        MODULE hostlib
        MODTYPE resource
        INCLUDE_NAME hostlib)
    aros_add_target_dependency("includes-generate" "kernel-hostlib-includes")
    _aros_register_genmodule_public_includes("kernel-hostlib-includes")
endif()

# icon.library and Wanderer's private Icon MUI class share a .conf stem.
# BootstrapSDK intentionally withholds ambiguous shared headers; the legacy
# `includes` closure selects icon.library for linklibs-amiga. Materialise that
# exact declaration as the real producer rather than letting configure-time
# traversal order decide the SDK API.
# Since every modtype=library module builds its client archive, icon.library's
# own ABI support already materialises this exact declaration. Keep the
# special case only for the configurations where it does not.
if(TARGET "workbench-libs-icon-includes" AND
   NOT TARGET "workbench-libs-icon-includes-generated")
    aros_materialize_genmodule_headers(
        NAME workbench-libs-icon-includes-generated
        OWNER workbench-libs-icon-includes
        CONFIG "${CMAKE_SOURCE_DIR}/workbench/libs/icon/icon.conf"
        MODULE icon
        MODTYPE library
        INCLUDE_NAME icon)
endif()

# libpng's historical build rewrites a fetched prebuilt header before
# linklibs-png-nostdio compiles. It is a real SDK-only output rather than a
# configure-time placeholder, so retain its fetch ordering.
if(TARGET "libpng-fetch" OR TARGET "workbench-libs-png-generated")
    if(NOT TARGET "libpng-fetch" OR
       NOT TARGET "workbench-libs-png-generated")
        message(FATAL_ERROR
            "libpng pnglibconf staging requires both generated targets")
    endif()
    include("${CMAKE_SOURCE_DIR}/cmake/PngLibconf.cmake")
    aros_stage_pnglibconf(
        NAME workbench-libs-png-generated-pnglibconf
        FETCH_TARGET libpng-fetch
        OWNER workbench-libs-png-generated
        PORTS_DIR "${AROS_PORTS_DIR}"
        SDK_INCLUDE_DIR "${AROS_SDK_INCLUDE_DIR}")
endif()

# FreeType publishes its ordinary SDK headers via %copy_includes, but replaces
# ftoption.h with the target's configuration before the aggregate `includes`
# target can satisfy any FreeType consumer.  Keep the historic generated
# header as a fetch-dependent output; using the port source as an include path
# would accidentally expose FreeType's unconfigured defaults.
if(TARGET "freetype2-fetch" OR TARGET "workbench-libs-freetype-genincludes")
    if(NOT TARGET "freetype2-fetch" OR
       NOT TARGET "workbench-libs-freetype-genincludes")
        message(FATAL_ERROR
            "FreeType option staging requires both generated targets")
    endif()
    include("${CMAKE_SOURCE_DIR}/cmake/FreetypeOptions.cmake")
    # The ordinary and module FreeType link libraries compile the configured
    # FreeType sources directly.  The demos' ftcommon library independently
    # includes freetype/freetype.h before either final demo program exists.
    # Bind every direct compiler consumer, rather than relying on the
    # aggregate `includes`/`linklibs` siblings to happen to finish first.
    aros_stage_freetype_options(
        NAME workbench-libs-freetype-genincludes-ftoption
        FETCH_TARGET freetype2-fetch
        OWNER workbench-libs-freetype-genincludes
        CONSUMERS
            workbench-libs-freetype-linklib
            workbench-libs-freetype-module-linklib
            test-freetype-lib-ftcommon
        INPUT "${AROS_PORTS_DIR}/freetype2/freetype-2.14.3/include/freetype/config/ftoption.h"
        OUTPUT "${AROS_SDK_INCLUDE_DIR}/freetype/config/ftoption.h")
endif()

# libde265 and libheif both configure private version headers after fetching
# their immutable ports.  The generated `*-genfiles` meta targets are only
# aggregate identities; make their real headers explicit so direct CMake/Ninja
# consumers cannot race the port fetch or compile against an absent template
# output.
if(TARGET "linklibs-de265-fetch" OR
   TARGET "datatypes-heic-linklibs-de265-genfiles")
    if(NOT TARGET "linklibs-de265-fetch" OR
       NOT TARGET "datatypes-heic-linklibs-de265-genfiles")
        message(FATAL_ERROR
            "libde265 version-header staging requires its fetch and genfiles targets")
    endif()
    include("${CMAKE_SOURCE_DIR}/cmake/HeicVersionHeaders.cmake")
    aros_stage_heic_version_header(
        NAME datatypes-heic-linklibs-de265-version-header
        FETCH_TARGET linklibs-de265-fetch
        OWNER datatypes-heic-linklibs-de265-genfiles
        KIND DE265
        INPUT "${AROS_PORTS_DIR}/libde265/libde265-1.1.1/libde265/de265-version.h.in"
        OUTPUT "${AROS_GEN_DIR}/workbench/classes/datatypes/heic/libde265/de265-version.h")
endif()

if(TARGET "linklibs-heif-fetch" OR
   TARGET "datatypes-heic-linklibs-heif-genfiles")
    if(NOT TARGET "linklibs-heif-fetch" OR
       NOT TARGET "datatypes-heic-linklibs-heif-genfiles")
        message(FATAL_ERROR
            "libheif version-header staging requires its fetch and genfiles targets")
    endif()
    include("${CMAKE_SOURCE_DIR}/cmake/HeicVersionHeaders.cmake")
    aros_stage_heic_version_header(
        NAME datatypes-heic-linklibs-heif-version-header
        FETCH_TARGET linklibs-heif-fetch
        OWNER datatypes-heic-linklibs-heif-genfiles
        KIND HEIF
        INPUT "${AROS_PORTS_DIR}/libheif/libheif-1.23.1/libheif/api/libheif/heif_version.h.in"
        OUTPUT "${AROS_GEN_DIR}/workbench/classes/datatypes/heic/libheif/heif_version.h")
endif()

# The GRUB2 host-tool lanes above produce the real BIOS and EFI generators,
# but the historic ISO recipe's asset staging is a separate, output-producing
# step. Materialise it only for its one audited x86_64-pc LLVM profile, and
# only on the host capability that actually implements the closed GRUB
# contract, after the generated declarations have created all three concrete
# host-lane targets.
if(AROS_TOOLCHAIN STREQUAL "llvm" AND
   AROS_TARGET_CPU STREQUAL "x86_64" AND
   AROS_TARGET_PLATFORM STREQUAL "pc" AND
   AROS_GRUB2_HOST_LANES_AVAILABLE)
    foreach(_aros_grub_host_lane IN ITEMS
            grub2-host grub2-efi-host grub2-efi32-host)
        if(NOT TARGET "${_aros_grub_host_lane}")
            message(FATAL_ERROR
                "GRUB2 ISO asset staging requires ${_aros_grub_host_lane}")
        endif()
    endforeach()
    include("${CMAKE_SOURCE_DIR}/cmake/GrubIsoAssets.cmake")
    aros_stage_grub2_iso_assets(
        MODE x86_64
        BINARY_DIR "${CMAKE_BINARY_DIR}/gen/grub2-iso-assets/x86_64"
        SYS_DIR "${CMAKE_BINARY_DIR}/SYS"
        HOST_PC "${CMAKE_BINARY_DIR}/hosttools/grub2/pc"
        HOST_EFI64 "${CMAKE_BINARY_DIR}/hosttools/grub2/efi-x86_64"
        HOST_EFI32 "${CMAKE_BINARY_DIR}/hosttools/grub2/efi-i386")
endif()

# 4a. Configure-time diagnostics accumulated by the CMake target helpers. The
#     report writer sorts and deduplicates stable machine-readable entries and
#     removes an old report when its cause has gone away.
function(_aros_write_property_report property_name report_name description)
    get_property(_entries GLOBAL PROPERTY "${property_name}")
    set(_report "${CMAKE_BINARY_DIR}/${report_name}")
    if(_entries)
        list(SORT _entries)
        list(REMOVE_DUPLICATES _entries)
        list(LENGTH _entries _count)
        string(REPLACE ";" "\n" _body "${_entries}")
        file(WRITE "${_report}" "${_body}\n")
        message(WARNING
            "${_count} ${description} -> ${_report}")
    else()
        file(REMOVE "${_report}")
    endif()
endfunction()

_aros_write_property_report(
    AROS_ICON_OUTPUT_CONFLICTS
    "generated_targets.icon-output-conflicts.txt"
    "icon output conflict(s); the first declaration is used")
_aros_write_property_report(
    AROS_ICON_MISSING_INPUTS
    "generated_targets.missing-icon-inputs.txt"
    "missing icon input(s)")
_aros_write_property_report(
    AROS_ICON_MISSING_HOST_TOOL
    "generated_targets.missing-icon-host-tool.txt"
    "icon declaration(s) without a usable host ilbmtoicon")
_aros_write_property_report(
    AROS_SKIPPED_SOURCE_LANGUAGE_LANES
    "generated_targets.skipped-source-languages.txt"
    "source-language lane(s) skipped")
_aros_write_property_report(
    AROS_EMPTY_CONCRETE_TARGETS
    "generated_targets.empty-concrete-targets.txt"
    "concrete declaration(s) resolved to zero sources")
_aros_write_property_report(
    AROS_MISSING_SOURCES
    "generated_targets.missing-sources.txt"
    "declared source(s) that resolved to no file")

get_property(_icon_targets GLOBAL PROPERTY AROS_ICON_TARGETS)
get_property(_icon_outputs GLOBAL PROPERTY AROS_ICON_OUTPUTS)
if(_icon_targets)
    list(REMOVE_DUPLICATES _icon_targets)
    list(LENGTH _icon_targets _n_icon_targets)
else()
    set(_n_icon_targets 0)
endif()
if(_icon_outputs)
    list(REMOVE_DUPLICATES _icon_outputs)
    list(LENGTH _icon_outputs _n_icon_outputs)
else()
    set(_n_icon_outputs 0)
endif()
if(_n_icon_targets OR _n_icon_outputs)
    message(STATUS
        "🖼️  AROS-NX: ${_n_icon_targets} icon target(s), "
        "${_n_icon_outputs} unique .info output rule(s)")
endif()

# 4b. Attach the generated headers recorded in GeneratedHeaders.cmake to the
#     targets that include them. Done here because the targets only exist once
#     generated_targets.cmake has run.
get_property(_gen_header_deps GLOBAL PROPERTY AROS_GENERATED_HEADER_DEPS)
set(_gen_header_missing "")
foreach(_pair IN LISTS _gen_header_deps)
    string(FIND "${_pair}" "|" _sep)
    if(_sep LESS 0)
        continue()
    endif()
    string(SUBSTRING "${_pair}" 0 ${_sep} _tgt)
    math(EXPR _rest "${_sep} + 1")
    string(SUBSTRING "${_pair}" ${_rest} -1 _hdr)

    if(NOT TARGET ${_tgt})
        # The module is not part of this target's build; nothing to attach to.
        continue()
    endif()

    get_filename_component(_hdr_name "${_hdr}" NAME)
    string(MAKE_C_IDENTIFIER "${_tgt}-${_hdr_name}" _stamp)
    if(NOT TARGET gen-${_stamp})
        add_custom_target(gen-${_stamp} DEPENDS "${_hdr}")
    endif()
    add_dependencies(${_tgt} gen-${_stamp})
    # The module's own sources are compiled into several targets (module, link
    # library, kernel object); each carries the same dependency.
    foreach(_suffix linklib kobj)
        if(TARGET ${_tgt}-${_suffix})
            add_dependencies(${_tgt}-${_suffix} gen-${_stamp})
        endif()
    endforeach()
endforeach()

# 4c. Targets excluded from `all` because their sources belong to another
#     architecture. Reported, not silent: the count is how many declarations
#     this configuration deliberately does not build.
get_property(_foreign GLOBAL PROPERTY AROS_FOREIGN_ARCH_TARGETS)
if(_foreign)
    list(LENGTH _foreign _n_foreign)
    set(_foreign_report "${CMAKE_BINARY_DIR}/generated_targets.foreign-arch-targets.txt")
    string(REPLACE ";" "\n" _foreign_body "${_foreign}")
    file(WRITE "${_foreign_report}" "${_foreign_body}\n")
    message(STATUS
        "🚫 AROS-NX: ${_n_foreign} target(s) kept out of `all` (other architecture) -> "
        "${_foreign_report}")
else()
    # Removed when empty. A report left behind outlives its cause and the next
    # reader takes it for the current state.
    file(REMOVE "${CMAKE_BINARY_DIR}/generated_targets.foreign-arch-targets.txt")
endif()

# 5. Report what SDK header staging produced (populated while including
#    generated_targets.cmake above).
get_property(_staged GLOBAL PROPERTY AROS_STAGED_HEADERS)
get_property(_empty GLOBAL PROPERTY AROS_STAGED_RULES_EMPTY)
message(STATUS "📥 AROS-NX: staged ${_staged} SDK header(s); ${_empty} staging rule(s) matched nothing")

# Hand-written header staging rules nobody has accounted for. Treated as a
# warning rather than a note: an unhandled rule means a header silently never
# reaches the SDK, which surfaces far away from its cause.
get_property(_adhoc_unknown GLOBAL PROPERTY AROS_ADHOC_HEADERS_UNKNOWN)
if(_adhoc_unknown)
    list(LENGTH _adhoc_unknown _n_adhoc)
    string(REPLACE ";" "\n    " _adhoc_pretty "${_adhoc_unknown}")
    message(WARNING
        "${_n_adhoc} hand-written header staging rule(s) have no static CMake counterpart.\n"
        "Add an aros_stage_header() call and list the target in "
        "AROS_ADHOC_HEADERS_HANDLED, or record it in "
        "AROS_ADHOC_HEADERS_OUT_OF_SCOPE (both in cmake/AROS.cmake):\n"
        "    ${_adhoc_pretty}")
endif()

# 5b. Uninitialised git submodules. Several build inputs live in submodules,
#     notably the translation catalogues under rom/*/catalogs: rom/dos's
#     dos.cd is the source both errorlist.h and strings.h are generated from,
#     so without it dos.library does not compile. That failure shows up as an
#     undeclared MSG_STRING_* identifier, which says nothing about its cause,
#     hence the check here.
find_package(Git QUIET)
if(GIT_FOUND AND EXISTS "${CMAKE_SOURCE_DIR}/.gitmodules")
    execute_process(
        COMMAND "${GIT_EXECUTABLE}" submodule status
        WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}"
        OUTPUT_VARIABLE _submodule_status
        ERROR_QUIET
        OUTPUT_STRIP_TRAILING_WHITESPACE)
    string(REPLACE "\n" ";" _submodule_lines "${_submodule_status}")
    set(_submodules_missing "")
    foreach(_line IN LISTS _submodule_lines)
        # A leading "-" marks a submodule that is not checked out.
        if(_line MATCHES "^-[0-9a-f]+ +(.+)$")
            list(APPEND _submodules_missing "${CMAKE_MATCH_1}")
        endif()
    endforeach()
    if(_submodules_missing)
        list(LENGTH _submodules_missing _n_submodules)
        set(_submodule_report
            "${CMAKE_BINARY_DIR}/generated_targets.uninitialised-submodules.txt")
        string(REPLACE ";" "\n" _submodule_body "${_submodules_missing}")
        file(WRITE "${_submodule_report}" "${_submodule_body}\n")
        message(WARNING
            "${_n_submodules} git submodule(s) are not checked out, so their "
            "files are absent from the source tree. Anything generated from "
            "them fails to build, with an error that points at the consumer "
            "rather than the missing input. Run\n"
            "    git submodule update --init --recursive\n"
            "or initialise individual paths. Full list:\n"
            "    ${_submodule_report}")
    else()
        file(REMOVE
            "${CMAKE_BINARY_DIR}/generated_targets.uninitialised-submodules.txt")
    endif()
endif()

# 5d. Header staging skipped because it belongs to another architecture.
get_property(_foreign_staging GLOBAL PROPERTY AROS_STAGING_FOREIGN_ARCH)
if(_foreign_staging)
    list(LENGTH _foreign_staging _n_fs)
    message(STATUS
        "🚫 AROS-NX: ${_n_fs} header staging rule(s) skipped (other architecture)")
endif()

# 5c. `ninja verify` compares what we emitted against the historic build's own
#     macro expansion. Not part of `all`: it shells out to genmf for every
#     mmakefile, which takes about half a minute on a cold cache.
find_program(AROS_PYTHON3 NAMES python3)
if(AROS_PYTHON3 AND EXISTS "${AROS_VERIFY_BIN}")
    add_custom_target(verify
        COMMAND "${AROS_VERIFY_BIN}"
                --source "${CMAKE_SOURCE_DIR}"
                --generated "${GENERATED_TARGETS_CMAKE}"
                --work "${CMAKE_BINARY_DIR}/verify"
                --build-dir "${CMAKE_BINARY_DIR}"
                --cpu "${AROS_TARGET_CPU}"
                --platform "${AROS_TARGET_PLATFORM}"
                --toolchain "${AROS_TOOLCHAIN}"
                --bootloader "${AROS_TARGET_BOOTLOADER}"
        COMMENT "Comparing transpiled targets against the genmf expansion"
        USES_TERMINAL
        VERBATIM)
    add_custom_target(verify-report
        COMMAND "${AROS_VERIFY_BIN}"
                --source "${CMAKE_SOURCE_DIR}"
                --generated "${GENERATED_TARGETS_CMAKE}"
                --work "${CMAKE_BINARY_DIR}/verify"
                --build-dir "${CMAKE_BINARY_DIR}"
                --cpu "${AROS_TARGET_CPU}"
                --platform "${AROS_TARGET_PLATFORM}"
                --toolchain "${AROS_TOOLCHAIN}"
                --bootloader "${AROS_TARGET_BOOTLOADER}"
                --no-gate
        COMMENT "Same comparison, reporting only"
        USES_TERMINAL
        VERBATIM)
endif()

# 6. Aggregate target for the third-party fetches declared via %fetch. Not part
#    of `all`: fetching reaches out to the network and stays explicit.
get_property(_fetch_targets GLOBAL PROPERTY AROS_FETCH_TARGETS)
if(_fetch_targets)
    add_custom_target(fetch-ports DEPENDS ${_fetch_targets})
    list(LENGTH _fetch_targets _n_fetch)
    message(STATUS "🌐 AROS-NX: ${_n_fetch} third-party fetch target(s); run 'ninja fetch-ports' to download")
endif()

# 7. Kickstart packages. Must come after generated_targets.cmake, because the
#    package definitions reference the transpiled module targets.
include("${CMAKE_SOURCE_DIR}/cmake/Kickstart.cmake")

# 8. Symbol audit. Answers the one question a green build cannot: whether a
#    module has any undefined symbols left, and so any chance of loading.
include("${CMAKE_SOURCE_DIR}/cmake/SymbolAudit.cmake")

# 8. Explicit, opt-in roots for the boot core and distribution closure. These
#    are deliberately separate from `all`: broadening the default build would
#    hide rather than expose the remaining direct-recipe/output gaps.
include("${CMAKE_SOURCE_DIR}/cmake/RootTargets.cmake")

# 9. Raspberry Pi boot payload. This is deliberately after Kickstart: the
#    Pi image embeds the core ELF and the Pi BSP package generated from the
#    transpiled target graph.
include("${CMAKE_SOURCE_DIR}/cmake/RaspberryPi.cmake")

# 10. OpenSBI/UEFI removable-media payload for reviewed RISC-V boards.
include("${CMAKE_SOURCE_DIR}/cmake/OpenSbiUefi.cmake")

# Compare every concrete declaration against a freshly generated upstream
# libdefs header. This is a build target rather than a configure-time glob: in
# an incremental tree the old reference files may exist but be stale.
aros_add_libdefs_audit_target()

# Match MetaMake's configured default invocation. Every translated declaration
# remains nameable, while the implicit CMake `all` target contains only the
# exact dependency closure rooted at AROS.
aros_limit_all_to_metamake_root(AROS)

get_property(_outside_default_root GLOBAL PROPERTY
    AROS_OUTSIDE_DEFAULT_ROOT_TARGETS)
if(_outside_default_root)
    list(LENGTH _outside_default_root _outside_default_root_count)
    set(_outside_default_root_report
        "${CMAKE_BINARY_DIR}/generated_targets.outside-default-root.txt")
    string(REPLACE ";" "\n" _outside_default_root_body
        "${_outside_default_root}")
    file(WRITE "${_outside_default_root_report}"
        "${_outside_default_root_body}\n")
    message(STATUS
        "🚧 AROS-NX: ${_outside_default_root_count} explicit/manual target(s) "
        "kept out of MetaMake's default AROS closure -> "
        "${_outside_default_root_report}")
else()
    file(REMOVE
        "${CMAKE_BINARY_DIR}/generated_targets.outside-default-root.txt")
endif()

message(STATUS "✅ AROS-NX (v0.1.0) Configuration Complete.")
