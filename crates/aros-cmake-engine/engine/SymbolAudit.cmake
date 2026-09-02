# =============================================================================
# Symbol audit: does a built module have any chance of loading?
# =============================================================================
#
# Every link here is `ld.lld -r`, partial, so a missing link library or a
# missing stub never fails a link. It produces a relocatable object with
# dangling externals that fails when AROS loads it. A green build therefore says
# nothing about loadability, and until this target existed nothing measured it.
#
# Not part of `all`: it walks every built artefact with llvm-nm, which is only
# meaningful once a build has actually produced them.
#
#   ninja symbol-audit            measure, and fail if a pinned number rose
#   ninja symbol-audit-baseline   re-pin deliberately after an intended change

find_program(AROS_AUDIT_PYTHON3 NAMES python3)
set(AROS_SYMBOL_AUDIT_SCRIPT "${CMAKE_SOURCE_DIR}/scripts/symbols/audit-symbols.py")
# Written by aros-genmodule at configure time. A relocatable module leaves its
# library bases undefined on purpose, so without this list the audit conflates
# "the loader will fill this in" with "nothing provides this": 1882 of 9268
# references were library bases.
set(AROS_SYMBOL_AUDIT_LIBBASES "${CMAKE_BINARY_DIR}/symbol-audit/libbases.txt")
set(AROS_SYMBOL_AUDIT_BASELINE
    "${CMAKE_SOURCE_DIR}/scripts/symbols/baseline-${AROS_TARGET_PLATFORM}-${AROS_TARGET_CPU}.json")

# llvm-nm has to be one that understands the target objects. Searched rather
# than derived: deriving it from CMAKE_C_COMPILER's directory produced
# /usr/bin/llvm-nm, which does not exist, and the failure surfaced as a Python
# traceback from inside the script.
get_filename_component(_cc_dir "${CMAKE_C_COMPILER}" DIRECTORY)
find_program(AROS_AUDIT_NM
    NAMES llvm-nm
    HINTS "${AROS_CROSS_TOOLCHAIN_ROOT}/bin" "${_cc_dir}"
          "/opt/homebrew/opt/llvm/bin" "/usr/local/opt/llvm/bin")
set(_audit_nm "${AROS_AUDIT_NM}")

# Which artefacts are loaded together, from aros_make_package() and
# aros_link_kickstart(). Written with file(GENERATE) because the member paths
# are $<TARGET_FILE:...> generator expressions.
set(AROS_SYMBOL_AUDIT_LOAD_SETS "${CMAKE_BINARY_DIR}/symbol-audit/load-sets.txt")
get_property(_aros_load_sets GLOBAL PROPERTY AROS_LOAD_SETS)
if(_aros_load_sets)
    string(REPLACE ";" "\n" _aros_load_sets_body "${_aros_load_sets}")
    file(GENERATE OUTPUT "${AROS_SYMBOL_AUDIT_LOAD_SETS}"
        CONTENT "${_aros_load_sets_body}\n")
    list(LENGTH _aros_load_sets _aros_n_load_sets)
    message(STATUS "🧷 AROS-NX: ${_aros_n_load_sets} load set(s) recorded for the symbol audit")
else()
    file(GENERATE OUTPUT "${AROS_SYMBOL_AUDIT_LOAD_SETS}" CONTENT "")
endif()

# Which artefacts this configuration actually produces.
#
# Walking SYS/ alone counts orphans: module output names have changed over time
# (SYS/Libs/muimaster.library today, SYS/Libs/workbench-libs-muimaster.library
# from an earlier scheme), and nothing removes the old file. Both were being
# measured, so the same module was counted twice and its stale copy, linked
# before the default link set existed, dominated the report.
#
# The set is every EXECUTABLE target, which is exactly the set of AROS
# artefacts: host tools are never plain executable targets
# (cmake/HostTools.cmake:11) and the kickstart is a custom command.
set(AROS_SYMBOL_AUDIT_ARTEFACTS "${CMAKE_BINARY_DIR}/symbol-audit/artefacts.txt")
_aros_collect_targets("${CMAKE_SOURCE_DIR}" _aros_audit_all_targets)
set(_aros_audit_artefacts "")
foreach(_aros_audit_target IN LISTS _aros_audit_all_targets)
    get_target_property(_aros_audit_type "${_aros_audit_target}" TYPE)
    if(_aros_audit_type STREQUAL "EXECUTABLE")
        list(APPEND _aros_audit_artefacts
            "$<TARGET_FILE:${_aros_audit_target}>")
    endif()
endforeach()
if(_aros_audit_artefacts)
    list(JOIN _aros_audit_artefacts "\n" _aros_audit_artefacts_body)
    file(GENERATE OUTPUT "${AROS_SYMBOL_AUDIT_ARTEFACTS}"
        CONTENT "${_aros_audit_artefacts_body}\n")
    list(LENGTH _aros_audit_artefacts _aros_audit_count)
    message(STATUS
        "🧾 AROS-NX: ${_aros_audit_count} artefact(s) declared for the symbol audit")
else()
    file(GENERATE OUTPUT "${AROS_SYMBOL_AUDIT_ARTEFACTS}" CONTENT "")
endif()

if(AROS_AUDIT_PYTHON3 AND AROS_AUDIT_NM AND EXISTS "${AROS_SYMBOL_AUDIT_SCRIPT}")
    add_custom_target(symbol-audit
        COMMAND "${AROS_AUDIT_PYTHON3}" -B "${AROS_SYMBOL_AUDIT_SCRIPT}"
                --root "${CMAKE_BINARY_DIR}/SYS"
                --nm "${_audit_nm}"
                --report-dir "${CMAKE_BINARY_DIR}/symbol-audit"
                --libbases "${AROS_SYMBOL_AUDIT_LIBBASES}"
                --load-sets "${AROS_SYMBOL_AUDIT_LOAD_SETS}"
                --artefacts "${AROS_SYMBOL_AUDIT_ARTEFACTS}"
                --baseline "${AROS_SYMBOL_AUDIT_BASELINE}"
        COMMENT "Auditing undefined symbols in the built modules"
        USES_TERMINAL
        VERBATIM)
    add_custom_target(symbol-audit-baseline
        COMMAND "${AROS_AUDIT_PYTHON3}" -B "${AROS_SYMBOL_AUDIT_SCRIPT}"
                --root "${CMAKE_BINARY_DIR}/SYS"
                --nm "${_audit_nm}"
                --report-dir "${CMAKE_BINARY_DIR}/symbol-audit"
                --libbases "${AROS_SYMBOL_AUDIT_LIBBASES}"
                --load-sets "${AROS_SYMBOL_AUDIT_LOAD_SETS}"
                --artefacts "${AROS_SYMBOL_AUDIT_ARTEFACTS}"
                --baseline "${AROS_SYMBOL_AUDIT_BASELINE}"
                --update-baseline
        COMMENT "Re-pinning the symbol audit baseline"
        USES_TERMINAL
        VERBATIM)
else()
    message(STATUS
        "⏭️  AROS-NX: symbol audit unavailable (python3, llvm-nm or the script is missing)")
endif()
