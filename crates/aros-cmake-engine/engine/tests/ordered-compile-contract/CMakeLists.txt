cmake_minimum_required(VERSION 3.22)
project(OrderedCompileContract C CXX)
include("${CMAKE_CURRENT_LIST_DIR}/../../OrderedCompileContract.cmake")
add_library(ordered_c OBJECT probe.c)
add_library(ordered_cxx OBJECT probe.cpp)
add_library(ordered_all OBJECT probe.c)
add_library(ordered_all_cxx OBJECT probe.cpp)
if(CASE STREQUAL "raw-counter")
    target_compile_options(ordered_c PRIVATE -DORDER=1 -UORDER -DORDER=1)
    return()
elseif(CASE STREQUAL "unknown-target")
    _aros_target_ordered_compile_contract(absent ALL -DORDER=1)
elseif(CASE STREQUAL "unknown-language")
    _aros_target_ordered_compile_contract(ordered_c ASM -DORDER=1)
elseif(CASE STREQUAL "empty")
    _aros_target_ordered_compile_contract(ordered_c ALL "")
elseif(CASE STREQUAL "no-arguments")
    _aros_target_ordered_compile_contract(ordered_c ALL)
elseif(CASE STREQUAL "semicolon")
    _aros_target_ordered_compile_contract(ordered_c ALL "-DORDER=1;ignored")
elseif(CASE STREQUAL "expression")
    _aros_target_ordered_compile_contract(ordered_c ALL "$<CONFIG:Debug>")
elseif(CASE STREQUAL "response-file")
    _aros_target_ordered_compile_contract(ordered_c ALL "@flags.txt")
elseif(CASE STREQUAL "duplicate-language")
    _aros_target_ordered_compile_contract(ordered_c C -DORDER=1)
    _aros_target_ordered_compile_contract(ordered_c C -UORDER -DORDER=1)
elseif(CASE STREQUAL "all-then-c")
    _aros_target_ordered_compile_contract(ordered_c ALL -DORDER=1)
    _aros_target_ordered_compile_contract(ordered_c C -UORDER -DORDER=1)
elseif(CASE STREQUAL "c-then-all")
    _aros_target_ordered_compile_contract(ordered_c C -DORDER=1)
    _aros_target_ordered_compile_contract(ordered_c ALL -UORDER -DORDER=1)
elseif(NOT CASE STREQUAL "valid")
    message(FATAL_ERROR "Unknown fixture case")
endif()
_aros_target_ordered_compile_contract(ordered_all ALL
    -DORDER=1 -UORDER -DORDER=1 "-DVALUE=\"a b\"" "-DAPOSTROPHE=\"a'b\""
    -DLANGUAGE_C=1)
_aros_target_ordered_compile_contract(ordered_all_cxx ALL
    -DORDER=1 -UORDER -DORDER=1 "-DVALUE=\"a b\"" "-DAPOSTROPHE=\"a'b\""
    -DLANGUAGE_CXX=1)
foreach(_target IN ITEMS ordered_c ordered_cxx)
    # Cross-language sentinels would fail both sources if the language gate
    # leaked; source code checks the selected value independently.
    _aros_target_ordered_compile_contract("${_target}" C
        -DORDER=1 -UORDER -DORDER=1 "-DVALUE=\"a b\"" "-DAPOSTROPHE=\"a'b\""
        -DLANGUAGE_C=1)
    _aros_target_ordered_compile_contract("${_target}" CXX
        -DORDER=1 -UORDER -DORDER=1 "-DVALUE=\"a b\"" "-DAPOSTROPHE=\"a'b\""
        -DLANGUAGE_CXX=1)
endforeach()
