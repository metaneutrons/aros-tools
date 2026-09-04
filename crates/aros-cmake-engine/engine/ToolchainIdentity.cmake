# Prevent a configured build tree from silently changing its immutable AROS
# cross-toolchain. CMake caches compiler paths, probe results and ABI details;
# reusing them with another prefix or release would make the build impure even
# when both toolchains happen to target the same CPU.
function(aros_lock_build_tree_toolchain)
    if(NOT AROS_CROSS_TOOLCHAIN_ROOT)
        return()
    endif()

    foreach(_required IN ITEMS
            AROS_TARGET_PROFILE AROS_TARGET_TRIPLE
            AROS_CROSS_TOOLCHAIN_RELEASE_ID)
        if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
            message(FATAL_ERROR
                "Locked AROS toolchain did not define ${_required}")
        endif()
    endforeach()

    set(_identity
        "schema=1\n"
        "root=${AROS_CROSS_TOOLCHAIN_ROOT}\n"
        "release_id=${AROS_CROSS_TOOLCHAIN_RELEASE_ID}\n"
        "target_profile=${AROS_TARGET_PROFILE}\n"
        "target_triple=${AROS_TARGET_TRIPLE}\n"
        "tree_sha256=${AROS_CROSS_TOOLCHAIN_TREE_SHA256}\n")
    string(JOIN "" _identity ${_identity})
    set(_stamp "${CMAKE_BINARY_DIR}/.aros-toolchain-id")

    if(EXISTS "${_stamp}")
        file(READ "${_stamp}" _configured_identity)
        if(NOT _configured_identity STREQUAL _identity)
            message(FATAL_ERROR
                "This build tree already belongs to a different AROS toolchain. "
                "Use a fresh build directory instead of mixing compiler state.\n"
                "Configured identity:\n${_configured_identity}"
                "Requested identity:\n${_identity}")
        endif()
    else()
        file(WRITE "${_stamp}" "${_identity}")
    endif()
endfunction()
