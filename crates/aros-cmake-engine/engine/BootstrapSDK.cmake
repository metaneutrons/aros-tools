# AROS-NX SDK Header Bootstrap

include("${CMAKE_CURRENT_LIST_DIR}/Executable.cmake")

# aros_generate_asm_header(<sdk_inc> <geninc>)
#
# Compiles compiler/include/asm.c to assembly and turns the .ascii strings it
# contains into aros/<cpu>/asm.h, in both include roots.
#
# The token differs by toolchain: GNU as emits .asciz, LLVM emits .ascii
# (see compiler/include/mmakefile.src:160).
function(aros_generate_asm_header sdk_inc geninc)
    set(_src "${CMAKE_SOURCE_DIR}/compiler/include/asm.c")
    if(NOT EXISTS "${_src}")
        return()
    endif()

    set(_asm "${CMAKE_BINARY_DIR}/asm.s")
    set(_incs
        "-I${CMAKE_SOURCE_DIR}/compiler/include"
        "-I${CMAKE_SOURCE_DIR}/arch/all-native/include"
        "-I${geninc}"
        "-I${sdk_inc}"
        "-I${CMAKE_SOURCE_DIR}/rom/exec"
        "-I${CMAKE_SOURCE_DIR}/rom/kernel"
    )
    # The architecture directories that apply, same selection as the includes.
    foreach(d "${AROS_TARGET_CPU}-${AROS_TARGET_PLATFORM}" "all-${AROS_TARGET_PLATFORM}"
              "${AROS_TARGET_CPU}-all" "all-native")
        foreach(m exec kernel)
            if(IS_DIRECTORY "${CMAKE_SOURCE_DIR}/arch/${d}/${m}")
                list(APPEND _incs "-I${CMAKE_SOURCE_DIR}/arch/${d}/${m}")
            endif()
        endforeach()
    endforeach()

    if(AROS_CROSS_TOOLCHAIN_ROOT AND AROS_TARGET_TRIPLE)
        set(_compiler_target "${AROS_TARGET_TRIPLE}")
    else()
        set(_compiler_target "${AROS_TARGET_CPU}-unknown-elf")
    endif()
    execute_process(
        COMMAND "${CMAKE_C_COMPILER}"
                -target "${_compiler_target}"
                -ffreestanding -fno-builtin
                ${_incs}
                -S "${_src}" -o "${_asm}"
        RESULT_VARIABLE _res
        ERROR_VARIABLE _err
        OUTPUT_QUIET
    )
    if(NOT _res EQUAL 0 OR NOT EXISTS "${_asm}")
        message(FATAL_ERROR
            "AROS-NX could not generate required aros/${AROS_TARGET_CPU}/asm.h "
            "with ${CMAKE_C_COMPILER} (exit ${_res}).\n${_err}")
    endif()

    file(STRINGS "${_asm}" _lines REGEX "\\.(ascii|asciz)")
    set(_out "")
    foreach(line IN LISTS _lines)
        # Take the first quoted field and drop the `$` markers, as the
        # reference's `cut -d'"' -f2 | sed 's/\$//g'` does.
        if(line MATCHES "\"([^\"]*)\"")
            set(_text "${CMAKE_MATCH_1}")
            string(REPLACE "$" "" _text "${_text}")
            string(APPEND _out "${_text}\n")
        endif()
    endforeach()

    if(_out STREQUAL "")
        message(FATAL_ERROR
            "AROS-NX generated no definitions for required "
            "aros/${AROS_TARGET_CPU}/asm.h from ${_asm}")
    endif()

    foreach(root "${sdk_inc}" "${geninc}")
        file(WRITE "${root}/aros/${AROS_TARGET_CPU}/asm.h" "${_out}")
    endforeach()
    string(REGEX MATCHALL "\n" _nl "${_out}")
    list(LENGTH _nl _n)
    message(STATUS "🔧 AROS-NX: generated aros/${AROS_TARGET_CPU}/asm.h (${_n} lines)")
endfunction()

function(aros_bootstrap_sdk_includes)
    set(SDK_INC "${CMAKE_BINARY_DIR}/SDK/include")
    set(GEN_INC "${CMAKE_BINARY_DIR}/GENINCDIR")
    file(MAKE_DIRECTORY "${SDK_INC}/aros")

    # 1. Copy core system headers from compiler/include/
    file(COPY "${CMAKE_SOURCE_DIR}/compiler/include/"
         DESTINATION "${SDK_INC}"
    )
    # The historic compiler-includes target publishes this architecture
    # dispatcher to both include roots.  Keep the generated root in sync too:
    # an existing build tree may still contain a same-named header staged by a
    # formerly unfiltered foreign architecture, and GENINCDIR is searched
    # before the freshly bootstrapped SDK.
    file(MAKE_DIRECTORY "${GEN_INC}/asm")
    file(COPY_FILE
        "${CMAKE_SOURCE_DIR}/compiler/include/asm/cpu.h"
        "${GEN_INC}/asm/cpu.h"
        ONLY_IF_DIFFERENT)
    if(EXISTS "${SDK_INC}/exec/execbase.inc")
        file(COPY_FILE "${SDK_INC}/exec/execbase.inc"
            "${SDK_INC}/exec/execbase.h" ONLY_IF_DIFFERENT)
    endif()

    # 2. Copy AROS support headers into aros/
    file(COPY "${CMAKE_SOURCE_DIR}/compiler/arossupport/include/"
         DESTINATION "${SDK_INC}/aros"
    )
    if(EXISTS "${CMAKE_SOURCE_DIR}/compiler/autoinit/autoinit.h")
        file(COPY_FILE "${CMAKE_SOURCE_DIR}/compiler/autoinit/autoinit.h"
            "${SDK_INC}/aros/autoinit.h" ONLY_IF_DIFFERENT)
    endif()

    # 3. Copy CRT and POSIX headers (stdio.h, stdlib.h, string.h, alloca.h, unistd.h, etc.)
    if(EXISTS "${CMAKE_SOURCE_DIR}/compiler/crt/stdc/include/aros/stdc/")
        file(COPY "${CMAKE_SOURCE_DIR}/compiler/crt/stdc/include/aros/stdc/"
             DESTINATION "${SDK_INC}"
        )
    endif()
    if(EXISTS "${CMAKE_SOURCE_DIR}/compiler/crt/posixc/include/aros/posixc/")
        file(COPY "${CMAKE_SOURCE_DIR}/compiler/crt/posixc/include/aros/posixc/"
             DESTINATION "${SDK_INC}"
        )
    endif()
    if(EXISTS "${CMAKE_SOURCE_DIR}/compiler/crt/posixc/include/")
        file(COPY "${CMAKE_SOURCE_DIR}/compiler/crt/posixc/include/"
             DESTINATION "${SDK_INC}"
        )
    endif()
    if(EXISTS "${CMAKE_SOURCE_DIR}/compiler/crt/stdc/include/")
        file(COPY "${CMAKE_SOURCE_DIR}/compiler/crt/stdc/include/"
             DESTINATION "${SDK_INC}"
        )
    endif()

    # 4. Copy Architecture-specific headers into their expected subdirectories
    if(EXISTS "${CMAKE_SOURCE_DIR}/arch/x86_64-all/include/aros/")
        file(COPY "${CMAKE_SOURCE_DIR}/arch/x86_64-all/include/aros/"
             DESTINATION "${SDK_INC}/aros/x86_64"
        )
    endif()
    if(EXISTS "${CMAKE_SOURCE_DIR}/arch/i386-all/include/aros/")
        file(COPY "${CMAKE_SOURCE_DIR}/arch/i386-all/include/aros/"
             DESTINATION "${SDK_INC}/aros/i386"
        )
    endif()

    if(EXISTS "${CMAKE_SOURCE_DIR}/arch/aarch64-all/include/aros/")
        file(COPY "${CMAKE_SOURCE_DIR}/arch/aarch64-all/include/aros/"
             DESTINATION "${SDK_INC}/aros/aarch64"
        )
    endif()

    if(EXISTS "${CMAKE_SOURCE_DIR}/arch/arm-all/include/aros/")
        file(COPY "${CMAKE_SOURCE_DIR}/arch/arm-all/include/aros/"
             DESTINATION "${SDK_INC}/aros/arm"
        )
    endif()
    if(EXISTS "${CMAKE_SOURCE_DIR}/arch/arm-all/include/aros-armel/")
        file(COPY "${CMAKE_SOURCE_DIR}/arch/arm-all/include/aros-armel/"
             DESTINATION "${SDK_INC}/aros/arm"
        )
    endif()

    if(EXISTS "${CMAKE_SOURCE_DIR}/arch/riscv64-all/include/aros/")
        file(COPY "${CMAKE_SOURCE_DIR}/arch/riscv64-all/include/aros/"
             DESTINATION "${SDK_INC}/aros/riscv64"
        )
    endif()

    # IRQ types header
    if(AROS_TARGET_CPU STREQUAL "x86_64" OR AROS_TARGET_CPU STREQUAL "i386")
        if(EXISTS "${CMAKE_SOURCE_DIR}/arch/i386-all/include/irqtypes.h")
            file(COPY_FILE
                "${CMAKE_SOURCE_DIR}/arch/i386-all/include/irqtypes.h"
                "${SDK_INC}/aros/irqtypes.h" ONLY_IF_DIFFERENT)
        endif()
    else()
        file(WRITE "${SDK_INC}/aros/irqtypes.h"
"#ifndef AROS_IRQTYPES_H\n#define AROS_IRQTYPES_H\n#define IRQTYPE_STANDARD (1 << 0)\n#endif\n"
        )
    endif()

    # 5. Boost is staged later from the compiler/boost %fetch input.
    # Do not copy the host's headers here: that made macOS and Linux SDKs
    # silently differ before the target's ports-includes closure had run.

    # 6. Generate aros/config.h
    #
    # The flavour follows the target, as configure derives it from the platform
    # case: `pc)` at configure:10727 sets aros_flavour="standalone" for both
    # i386 and x86_64, `r*pi)` at :11213 sets it for arm and aarch64, and
    # `opensbi)` at :11305 selects the same standalone flavour for RISC-V.
    # AROS_FLAVOUR_NATIVE, which this file used to state for every target, is
    # what configure picks for classic Amiga-like ports, and it is wrong for all
    # three presets here.
    #
    # It is not a cosmetic difference. Whole function bodies are inside
    # `#if (AROS_FLAVOUR & AROS_FLAVOUR_STANDALONE)`: with NATIVE the mask is
    # 1 & 2 = 0, so rom/exec/superstate.c compiled down to `return NULL` and
    # `core_APIC_Probe` (arch/all-pc/kernel/apic.c:41) freed its descriptor and
    # returned NULL, which made ictl_Initialize panic with "Failed to allocate
    # APIC descriptor". arch/x86_64-all/kernel/cpu_init.c:26 silently skipped
    # the XSAVE/AVX context path for the same reason.
    #
    # A platform this does not know must not inherit a flavour by accident.
    if(AROS_TARGET_PLATFORM STREQUAL "pc" OR
       AROS_TARGET_PLATFORM STREQUAL "raspi" OR
       AROS_TARGET_PLATFORM STREQUAL "opensbi")
        set(_aros_flavour "AROS_FLAVOUR_STANDALONE")
    else()
        message(FATAL_ERROR
            "No AROS_FLAVOUR is known for platform '${AROS_TARGET_PLATFORM}'. "
            "configure derives it per platform case (see configure:10727 for pc, "
            ":11213 for raspi, and :11305 for opensbi); add this one rather than "
            "letting it default.")
    endif()

    # 15 of the 20 values config/config.h.in substitutes are still missing from
    # this file, and a missing macro is silently zero in `#if`. See OPEN-POINTS
    # point 35 for the list and what each one would change.
    set(CONFIG_H "${SDK_INC}/aros/config.h")
    # This header is consumed below, during the same configure pass, by
    # aros_generate_asm_header().  file(GENERATE) defers the write until the
    # generation phase and therefore makes a pristine build depend on a stale
    # config.h from an earlier configure.  The contents contain no generator
    # expressions, so publish them immediately and deterministically.
    file(WRITE "${CONFIG_H}"
"/* AROS-NX v0.1.0: Auto-generated aros/config.h */
#ifndef AROS_CONFIG_H
#define AROS_CONFIG_H

#define AROS_FLAVOUR_NATIVE             1
#define AROS_FLAVOUR_STANDALONE         2
#define AROS_FLAVOUR_EMULATION          4
#define AROS_FLAVOUR_LINKLIB            8
#define AROS_FLAVOUR_BINCOMPAT          16
#define AROS_FLAVOUR                    ${_aros_flavour}
#define AROS_DOS_PACKETS                1
#define AROS_AMIGAOS_COMPLIANCE         1

#define AROS_NOMINAL_WIDTH              640
#define AROS_NOMINAL_HEIGHT             480
#define AROS_NOMINAL_DEPTH              8

#define AROS_SERIAL_DEBUG               1
#define AROS_MODULES_DEBUG              1

#endif /* AROS_CONFIG_H */
")

    # 7. Run pure-Rust aros-genmodule to generate proto/, clib/, defines/,
    #    interface/ and the per-module <mod>_libdefs.h.
    #
    #    Public headers go to the shared SDK; <mod>_libdefs.h goes to a
    #    per-module directory under AROS_GEN_DIR, because it is module-private
    #    (LIBBASE, LIBBASETYPE, the cdefprivate block) and 26 .conf stems occur
    #    more than once in the tree. aros_apply_includes() puts the matching
    #    directory on each target's include path.
    set(AROS_GEN_DIR "${CMAKE_BINARY_DIR}/gen")

    # Older builds wrote <mod>_libdefs.h into the shared SDK. Those copies would
    # win over the per-module ones, because directory-level include paths are
    # searched before target-level ones. Remove them so the move takes effect in
    # an existing build tree, not only in a fresh one.
    file(GLOB _stale_libdefs "${SDK_INC}/*_libdefs.h")
    if(_stale_libdefs)
        list(LENGTH _stale_libdefs _n_stale)
        message(STATUS "🧹 AROS-NX: removing ${_n_stale} stale <mod>_libdefs.h from the shared SDK")
        file(REMOVE ${_stale_libdefs})
    endif()
    # genmodule takes the list space-separated, CMake stores it with semicolons.
    string(REPLACE ";" " " _arch_dirs_arg "${AROS_ARCH_SOURCE_DIRS}")
    if(DEFINED AROS_GENMODULE_BIN)
        aros_path_is_executable("${AROS_GENMODULE_BIN}" _aros_genmodule_executable)
    else()
        set(_aros_genmodule_executable FALSE)
    endif()
    if(NOT _aros_genmodule_executable)
        message(FATAL_ERROR
            "AROS-NX requires executable aros-genmodule at "
            "${AROS_GENMODULE_BIN}. Install a complete aros-tools release, or "
            "set AROS_RUST_TOOLS_DIR / AROS_GENMODULE_BIN explicitly.")
    endif()
    execute_process(
        COMMAND "${AROS_GENMODULE_BIN}"
                "--scan-dir" "${CMAKE_SOURCE_DIR}"
                "--output-inc" "${SDK_INC}"
                "--output-gen" "${AROS_GEN_DIR}"
                # The library bases this tree declares, for `ninja symbol-audit`.
                # A relocatable module leaves them undefined on purpose, and the
                # audit needs the list to tell that apart from a real gap.
                "--output-libbases" "${CMAKE_BINARY_DIR}/symbol-audit/libbases.txt"
                "--arch-dirs" "${_arch_dirs_arg}"
        RESULT_VARIABLE GENMODULE_RES
        ERROR_VARIABLE _aros_genmodule_error
    )
    if(NOT GENMODULE_RES EQUAL 0)
        message(FATAL_ERROR
            "AROS-NX SDK generator failed (${GENMODULE_RES}).\n"
            "${_aros_genmodule_error}")
    endif()

    # 8. aros/<cpu>/asm.h, needed by every assembly source.
    #
    # Not a copy but a code generation step: compiler/include/asm.c is compiled
    # to assembly, and the .ascii strings it emits are the header's contents.
    # This mirrors compiler/include/mmakefile.src:170. Without it every .s/.S
    # file fails at `#include <aros/<cpu>/asm.h>`, which is what the
    # architecture-specific source overrides consist of.
    aros_generate_asm_header("${SDK_INC}" "${CMAKE_BINARY_DIR}/GENINCDIR")

    message(STATUS "✅ AROS-NX SDK include tree populated at: ${SDK_INC}")
endfunction()
