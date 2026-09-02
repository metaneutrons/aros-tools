# =============================================================================
# Hand-written generator rules from the mmakefiles
# =============================================================================
#
# The transpiler reports every Make rule that produces a header but cannot be
# expressed generically, because its recipe is arbitrary Make (see
# AROS_ADHOC_HEADERS_UNKNOWN). Those rules need a counterpart here. Each one
# below names the mmakefile line it stands for.
#
# Destinations follow the module's own USER_INCLUDES, so a source file's
# #include "..." resolves the way it does in the historic build.

set(_gen "${CMAKE_BINARY_DIR}/gen")
set_property(GLOBAL PROPERTY AROS_GENERATED_HEADER_DEPS "")

# _aros_needs_header(<target> <header>)
#
# Records that <target> cannot compile before <header> exists. Resolved after
# generated_targets.cmake has created the targets.
function(_aros_needs_header target header)
    get_property(_deps GLOBAL PROPERTY AROS_GENERATED_HEADER_DEPS)
    list(APPEND _deps "${target}|${header}")
    set_property(GLOBAL PROPERTY AROS_GENERATED_HEADER_DEPS "${_deps}")
endfunction()

# rom/dos/errorlist.h is no longer duplicated here: the transpiler recognises
# its exact `$(PYTHON) ... > $@` rule, owns the output, and binds kernel-dos.
# The catalog machinery independently owns strings.h from the same dos.cd.

# -----------------------------------------------------------------------------
# Boot images
# -----------------------------------------------------------------------------
#
#   rom/dosboot/mmakefile.src:39    -> nomedia_image.h
#   rom/cgxbootpic/mmakefile.src:19 -> bootpic_image.h
#
# ilbmtoc emits chunky pixels by default and planar bitplanes with -p. The
# mmakefile picks planar for m68k only, since native Amiga hardware has a
# planar display and chunky-to-planar conversion on a 7 MHz 68000 is too slow.
if(AROS_TARGET_CPU STREQUAL "m68k")
    set(_ilbm_flags -p)
else()
    set(_ilbm_flags "")
endif()

aros_ilbm_header(
    ILBM "${CMAKE_SOURCE_DIR}/rom/dosboot/nomedia.ilbm"
    OUTPUT "${_gen}/rom/dosboot/dosboot/nomedia_image.h"
    FLAGS ${_ilbm_flags})
_aros_needs_header(kernel-dosboot "${_gen}/rom/dosboot/dosboot/nomedia_image.h")

# cgxbootpic asks for -I$(GENDIR)/$(CURDIR), without the extra segment.
aros_ilbm_header(
    ILBM "${CMAKE_SOURCE_DIR}/rom/cgxbootpic/bootpic.ilbm"
    OUTPUT "${_gen}/rom/cgxbootpic/bootpic_image.h")
_aros_needs_header(kernel-cgxbootpic "${_gen}/rom/cgxbootpic/bootpic_image.h")

# -----------------------------------------------------------------------------
# SoftFloat platform configuration
# -----------------------------------------------------------------------------
#
# compiler/softfloat/mmakefile.src:278-290 generates a deliberately tiny
# private platform.h before its fetched sources are compiled.  Do not replace
# it with SoftFloat's build/*/platform.h: those upstream variants select host
# compiler-specific helpers that the historic AROS rule intentionally leaves
# disabled.  The legacy ARM-family branch changes only its explicit big-endian
# variant; every other current target is little-endian.
set(_softfloat_littleendian 1)
set(_softfloat_arm_family arm armeb aarch64)
if(AROS_TARGET_CPU IN_LIST _softfloat_arm_family AND
   AROS_TARGET_VARIANT STREQUAL "be")
    set(_softfloat_littleendian 0)
endif()
set(_softfloat_platform_h "${_gen}/compiler/softfloat/platform.h")
aros_generate_defines_header(
    OWNER linklibs-softfloat-genfiles
    OUTPUT "${_softfloat_platform_h}"
    DEFINES "LITTLEENDIAN ${_softfloat_littleendian}"
    DEPENDS "${CMAKE_SOURCE_DIR}/compiler/softfloat/mmakefile.src")
_aros_needs_header(linklibs-softfloat "${_softfloat_platform_h}")

# -----------------------------------------------------------------------------
# libraries/mui.h
# -----------------------------------------------------------------------------
#
# workbench/libs/muimaster/mmakefile.src:459-465. muimaster does not ship this
# header; it generates one from mui.h, macros.h and every class header. Missing,
# it was the largest single gap in the build: 215 compile failures plus the
# undeclared identifiers that follow.
#
# Generated at configure time rather than as a build step, the same way the SDK
# headers are bootstrapped. Every consumer includes it as <libraries/mui.h>, so
# it has to exist before the first compile; making several hundred targets
# depend on one custom command would express that far more expensively. The
# trade-off is the same one BootstrapSDK.cmake makes: editing a muimaster class
# header needs a re-configure.
set(_mui_header "${CMAKE_BINARY_DIR}/GENINCDIR/libraries/mui.h")
set(_mui_dir "${CMAKE_SOURCE_DIR}/workbench/libs/muimaster")
if(EXISTS "${_mui_dir}/buildincludes.c" AND NOT EXISTS "${_mui_header}")
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/GENINCDIR/libraries")
    set(_mui_tool "${AROS_HOST_TOOL_DIR}/buildincludes")
    execute_process(
        COMMAND "${AROS_HOST_CC}" -O2 -w "${_mui_dir}/buildincludes.c" -o "${_mui_tool}"
        RESULT_VARIABLE _mui_cc_res
        ERROR_VARIABLE _mui_cc_err)
    if(_mui_cc_res EQUAL 0)
        execute_process(
            COMMAND "${_mui_tool}"
            WORKING_DIRECTORY "${_mui_dir}"
            OUTPUT_FILE "${_mui_header}"
            RESULT_VARIABLE _mui_res
            ERROR_VARIABLE _mui_err)
        if(NOT _mui_res EQUAL 0)
            message(WARNING "buildincludes failed, libraries/mui.h not generated: ${_mui_err}")
        endif()
    else()
        message(WARNING "cannot build buildincludes: ${_mui_cc_err}")
    endif()
endif()
if(EXISTS "${_mui_header}")
    file(STRINGS "${_mui_header}" _mui_lines)
    list(LENGTH _mui_lines _n_mui)
    message(STATUS "🧵 AROS-NX: generated libraries/mui.h (${_n_mui} lines)")
endif()

# -----------------------------------------------------------------------------
# The BSD socket interface
# -----------------------------------------------------------------------------
#
# workbench/network/common/include/mmakefile.src:27 stages its whole header
# tree into the SDK with a static pattern rule:
#
#     $(DEST_INCLUDES) : $(AROS_INCLUDES)/% : $(SRCDIR)/$(CURDIR)/%
#
# 81 headers, and the directory structure has to survive, since consumers write
# <netinet/in.h> and <proto/socket.h>. The pattern list is the one the
# mmakefile's WILDCARD call names; no FLATTEN, so the subdirectories are kept.
aros_copy_includes(
    DEST "."
    SOURCE "workbench/network/common/include"
    PATTERNS "*.h" "arpa/*.h" "bsdsocket/*.h" "clib/*.h" "defines/*.h"
             "libraries/*.h" "net/*.h" "netinet/*.h" "proto/*.h" "sys/*.h")

# -----------------------------------------------------------------------------
# The vendored Boost subset
# -----------------------------------------------------------------------------
#
# compiler/include/aros/preprocessor/ includes Boost unconditionally, and
# inline/posixc.h reaches it, so every C source that touches proto/posixc.h
# needs Boost headers in the SDK. Without them the build stops at
#
#     SDK/include/aros/preprocessor/variadic/size.hpp:4:11:
#         fatal error: 'boost/preprocessor/cat.hpp' file not found
#
# compiler/boost/include carries boost/preprocessor and boost/config for
# exactly this; see the README there for provenance and why they are in the
# tree rather than fetched.
#
# Copied at configure time rather than as a build step, for the reason
# libraries/mui.h above is: several hundred compiles need these headers to
# exist before the first one runs, and expressing that as a dependency on a
# custom command would cost far more than one copy. MetaMake stages the same
# files through compiler-boost-subset-includes-copy, which is reachable from
# sdk-includes-1 and therefore covers the producer's route.
set(_boost_subset "${CMAKE_SOURCE_DIR}/compiler/boost/include/boost")
if(IS_DIRECTORY "${_boost_subset}")
    foreach(_root "${AROS_SDK_INCLUDE_DIR}" "${AROS_GENINC_DIR}")
        # Unconditional: file(COPY) is copy-if-different, and guarding on the
        # directory existing left it half filled where another rule had already
        # staged part of it. SDK/include/acpica had the Port's 40 top-level
        # headers and no platform/, which fails just as hard as having none.
        file(COPY "${_boost_subset}" DESTINATION "${_root}")
    endforeach()
    file(GLOB_RECURSE _boost_staged "${AROS_SDK_INCLUDE_DIR}/boost/*")
    list(LENGTH _boost_staged _n_boost)
    message(STATUS "🧵 AROS-NX: staged ${_n_boost} vendored Boost header(s)")
else()
    message(WARNING
        "compiler/boost/include/boost is missing; every source reaching "
        "proto/posixc.h will fail on boost/preprocessor")
endif()

# -----------------------------------------------------------------------------
# The vendored ACPICA headers
# -----------------------------------------------------------------------------
#
# Four arch/all-pc/kernel sources include libraries/acpica.h, which needs eight
# headers from ACPICA's source/include. They normally arrive with the fetched
# Port; a plain configure does not fetch Ports, so the build stopped at
#
#     GENINCDIR/libraries/acpica.h:47:10:
#         fatal error: 'acpica/actypes.h' file not found
#
# and with it kernel-kernel and everything downstream of the kernel.
#
# arch/all-native/acpica/include/acpica carries them; see the README there.
# Copied at configure time for the same reason the Boost subset above is.
set(_acpica_subset "${CMAKE_SOURCE_DIR}/arch/all-native/acpica/include/acpica")
if(IS_DIRECTORY "${_acpica_subset}")
    foreach(_root "${AROS_SDK_INCLUDE_DIR}" "${AROS_GENINC_DIR}")
        # Unconditional: file(COPY) is copy-if-different, and guarding on the
        # directory existing left it half filled where another rule had already
        # staged part of it. SDK/include/acpica had the Port's 40 top-level
        # headers and no platform/, which fails just as hard as having none.
        file(COPY "${_acpica_subset}" DESTINATION "${_root}")
    endforeach()
    file(GLOB_RECURSE _acpica_staged "${AROS_SDK_INCLUDE_DIR}/acpica/*")
    list(LENGTH _acpica_staged _n_acpica)
    message(STATUS "🧵 AROS-NX: staged ${_n_acpica} vendored ACPICA header(s)")
else()
    message(WARNING
        "arch/all-native/acpica/include/acpica is missing; kernel-kernel will "
        "fail on acpica/actypes.h")
endif()
