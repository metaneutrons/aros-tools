cmake_minimum_required(VERSION 3.22)
project(ArchQuoteIncludesFixture LANGUAGES C ASM)
list(INSERT CMAKE_MODULE_PATH 0 "${CMAKE_CURRENT_LIST_DIR}/cmake")
set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}/source inputs")
set(_arch "${AROS_SOURCE_DIR}/arch/all-pc/probe")
set(_extra "${AROS_SOURCE_DIR}/extra quotes")
set(_angle "${AROS_SOURCE_DIR}/angle")
set(_ordinary "${AROS_SOURCE_DIR}/ordinary")
file(MAKE_DIRECTORY "${_arch}" "${_extra}" "${_angle}" "${_ordinary}")
file(MAKE_DIRECTORY "${AROS_SOURCE_DIR}/compiler/startup")
# The engine registers these excluded startup targets even though the fixture
# builds only objects. They are never compiled or used as search-path evidence.
file(WRITE "${AROS_SOURCE_DIR}/compiler/startup/startup.c" "int unused_startup;\n")
file(WRITE "${AROS_SOURCE_DIR}/compiler/startup/detach.c" "int unused_detach;\n")
file(WRITE "${_extra}/selected.h" "#define SELECTED 11\n")
file(WRITE "${_ordinary}/selected.h" "#define SELECTED 22\n")
file(WRITE "${_angle}/selected.h" "#define SELECTED 33\n")
file(WRITE "${_arch}/special.c" [=[
#include "selected.h"
#if SELECTED != 11
#error wrong explicit quote include
#endif
#undef SELECTED
#include <selected.h>
#if SELECTED != 33
#error extra quote path polluted angle lookup
#endif
#ifndef EXPECT_CODEGEN
#error source codegen flags were lost
#endif
int special;
]=])
file(WRITE "${_arch}/special-asm.S" [=[
#include "selected.h"
#if SELECTED != 11
#error wrong explicit quote include
#endif
.text
]=])
set(_regular "${AROS_SOURCE_DIR}/regular.c")
file(WRITE "${_regular}" [=[
#include "selected.h"
#if SELECTED != 22
#error explicit quote path leaked to an ordinary source
#endif
int regular;
]=])
set(_shared "${AROS_SOURCE_DIR}/shared.c")
file(WRITE "${_shared}" [=[
#include "shared.h"
#if SHARED != EXPECT_SHARED
#error shared source inherited another target's fallback
#endif
int shared;
]=])
include("${CMAKE_CURRENT_LIST_DIR}/../../AROS.cmake")
aros_set_arch_source_options(
    "pc|arch/all-pc/probe|special|-iquote${_extra}"
    "pc|arch/all-pc/probe|special|-DEXPECT_CODEGEN=1"
    "pc|arch/all-pc/probe|special-asm|-iquote${_extra}")
if(QUOTE_TEST_CASE STREQUAL conflict)
    aros_set_arch_source_options(
        "pc|arch/all-pc/probe|special|-iquote${_extra}"
        "pc|arch/all-pc/probe|special|-iquote${_ordinary}")
endif()
aros_resolve_arch_sources(_resolved _dropped "${AROS_SOURCE_DIR}"
    ARCH_SOURCES "pc|arch/all-pc/probe|special,special-asm")
aros_mark_preprocessed_asm(${_resolved})
foreach(_lane IN ITEMS a b)
    set(_fallback "${AROS_SOURCE_DIR}/fallback-${_lane}")
    file(MAKE_DIRECTORY "${_fallback}")
    if(_lane STREQUAL a)
        set(_expected 41)
    else()
        set(_expected 42)
    endif()
    file(WRITE "${_fallback}/shared.h" "#define SHARED ${_expected}\n")
    add_library("probe-${_lane}" OBJECT ${_resolved} "${_regular}" "${_shared}")
    target_compile_definitions("probe-${_lane}" PRIVATE "EXPECT_SHARED=${_expected}")
    _aros_add_genmodule_quote_dirs("probe-${_lane}" "${_ordinary}")
    aros_apply_includes("probe-${_lane}" INCLUDES "${_angle}")
    # A late dependency path must remain in this target's deferred list.
    _aros_add_genmodule_quote_dirs("probe-${_lane}" "${_fallback}")
    if(QUOTE_TEST_CASE STREQUAL target-first)
        target_compile_options("probe-${_lane}" BEFORE PRIVATE "-iquote${_ordinary}")
    endif()
endforeach()
