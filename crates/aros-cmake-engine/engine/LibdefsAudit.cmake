include_guard(GLOBAL)

# Registers one concrete MetaMake declaration where a broad Rust bootstrap
# header can shadow the exact upstream genmodule header.
function(aros_register_libdefs_audit identity rust_libdefs reference_libdefs)
    foreach(_value IN ITEMS "${identity}" "${rust_libdefs}" "${reference_libdefs}")
        if(_value MATCHES "[\t\r\n;]")
            message(FATAL_ERROR
                "libdefs audit fields may not contain tabs, newlines or semicolons: ${_value}")
        endif()
    endforeach()
    set_property(GLOBAL APPEND PROPERTY AROS_LIBDEFS_AUDIT_RECORDS
        "${identity}\t${rust_libdefs}\t${reference_libdefs}")
    set_property(GLOBAL APPEND PROPERTY AROS_LIBDEFS_AUDIT_REFERENCES
        "${reference_libdefs}")
endfunction()

# Adds a fail-closed gate after every shadowing pair has been registered.
# Reference outputs are dependencies, so stale files are rebuilt
# before they are compared. The target itself always runs: the Rust bootstrap
# files are configure-time products and deliberately have no Ninja producer.
function(aros_add_libdefs_audit_target)
    get_property(_records GLOBAL PROPERTY AROS_LIBDEFS_AUDIT_RECORDS)
    get_property(_references GLOBAL PROPERTY AROS_LIBDEFS_AUDIT_REFERENCES)
    if(NOT _records)
        return()
    endif()
    list(SORT _records)
    list(REMOVE_DUPLICATES _references)
    set(_manifest "${CMAKE_BINARY_DIR}/verify/functions-count-audit.manifest")
    set(_report "${CMAKE_BINARY_DIR}/verify/functions-count-audit.txt")
    string(REPLACE ";" "\n" _content "${_records}")
    file(GENERATE OUTPUT "${_manifest}" CONTENT "${_content}\n")

    add_custom_target(functions-count-audit
        COMMAND "${CMAKE_COMMAND}"
            -DAROS_LIBDEFS_AUDIT_MANIFEST=${_manifest}
            -DAROS_LIBDEFS_AUDIT_REPORT=${_report}
            -P "${CMAKE_SOURCE_DIR}/cmake/RunLibdefsAudit.cmake"
        DEPENDS ${_references}
        COMMENT "Comparing Rust and upstream genmodule FUNCTIONS_COUNT values"
        VERBATIM)
    if(TARGET verify)
        add_dependencies(verify functions-count-audit)
    endif()
endfunction()
