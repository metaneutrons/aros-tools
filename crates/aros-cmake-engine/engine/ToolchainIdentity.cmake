# Prevent a configured build tree from silently changing its immutable AROS
# cross-toolchain. CMake caches compiler paths, probe results and ABI details;
# reusing them with another prefix or release would make the build impure even
# when both toolchains happen to target the same CPU.
function(aros_lock_build_tree_toolchain)
    if(NOT AROS_CROSS_TOOLCHAIN_ROOT)
        return()
    endif()

    foreach(_required IN ITEMS AROS_TARGET_PROFILE AROS_TARGET_TRIPLE)
        if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
            message(FATAL_ERROR
                "Locked AROS toolchain did not define ${_required}")
        endif()
    endforeach()

    if(DEFINED AROS_CROSS_TOOLCHAIN_PROFILE)
        set(_identity_toolchain_profile "${AROS_CROSS_TOOLCHAIN_PROFILE}")
    else()
        # Compatibility for direct CMake consumers that predate a separate
        # compiler-profile namespace.
        set(_identity_toolchain_profile "${AROS_TARGET_PROFILE}")
    endif()
    if(NOT _identity_toolchain_profile STREQUAL "${AROS_TARGET_PROFILE}")
        if(NOT _identity_toolchain_profile MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*$")
            message(FATAL_ERROR "Build-tree compiler profile is empty or unsafe")
        endif()
    endif()

    if(AROS_CROSS_TOOLCHAIN_QUALIFICATION STREQUAL "local-byte-verified")
        if(NOT AROS_TOOLCHAIN STREQUAL "gnu" OR
           NOT "${AROS_CROSS_TOOLCHAIN_RELEASE_ID}" STREQUAL "")
            message(FATAL_ERROR "Local GNU build identity cannot claim a release")
        endif()
        foreach(_digest IN ITEMS AROS_CROSS_TOOLCHAIN_LOCAL_SHA256
                AROS_CROSS_TOOLCHAIN_TREE_SHA256)
            string(LENGTH "${${_digest}}" _length)
            if(NOT _length EQUAL 64 OR NOT "${${_digest}}" MATCHES "^[0-9a-f]+$")
                message(FATAL_ERROR "Local GNU build identity requires verified ${_digest}")
            endif()
        endforeach()
        set(_identity
            "schema=2\n"
            "qualification=local-byte-verified\n"
            "root=${AROS_CROSS_TOOLCHAIN_ROOT}\n"
            "descriptor_sha256=${AROS_CROSS_TOOLCHAIN_LOCAL_SHA256}\n"
            "target_profile=${AROS_TARGET_PROFILE}\n"
            "target_triple=${AROS_TARGET_TRIPLE}\n"
            "tree_sha256=${AROS_CROSS_TOOLCHAIN_TREE_SHA256}\n")
    else()
        if(DEFINED AROS_CROSS_TOOLCHAIN_QUALIFICATION AND
           NOT "${AROS_CROSS_TOOLCHAIN_QUALIFICATION}" STREQUAL "")
            message(FATAL_ERROR "Unsupported build-tree toolchain qualification")
        endif()
        if(NOT AROS_CROSS_TOOLCHAIN_RELEASE_ID)
            message(FATAL_ERROR
                "Locked AROS toolchain did not define AROS_CROSS_TOOLCHAIN_RELEASE_ID")
        endif()
        # Retain the existing released/producer-manifest stamp bytes.
        set(_identity
        "schema=1\n"
        "root=${AROS_CROSS_TOOLCHAIN_ROOT}\n"
        "release_id=${AROS_CROSS_TOOLCHAIN_RELEASE_ID}\n"
        "target_profile=${AROS_TARGET_PROFILE}\n"
        "target_triple=${AROS_TARGET_TRIPLE}\n"
        "tree_sha256=${AROS_CROSS_TOOLCHAIN_TREE_SHA256}\n")
    endif()
    if(NOT _identity_toolchain_profile STREQUAL "${AROS_TARGET_PROFILE}")
        list(APPEND _identity
            "toolchain_profile=${_identity_toolchain_profile}\n")
    endif()
    string(JOIN "" _identity ${_identity})
    set(_stamp "${CMAKE_BINARY_DIR}/.aros-toolchain-id")

    if(IS_SYMLINK "${_stamp}" OR IS_DIRECTORY "${_stamp}")
        message(FATAL_ERROR "Build-tree toolchain identity must be a regular unlinked file")
    endif()
    if(EXISTS "${_stamp}")
        if(UNIX)
            # A pinned system `test`, as every other execution-time file-kind
            # check in the engine uses, so an earlier PATH entry cannot make the
            # check pass for a FIFO, device or hardlinked stamp.
            find_program(_aros_identity_test NAMES test PATHS /usr/bin /bin
                NO_DEFAULT_PATH NO_CACHE REQUIRED)
            execute_process(COMMAND "${_aros_identity_test}" -f "${_stamp}"
                RESULT_VARIABLE _regular OUTPUT_QUIET ERROR_QUIET TIMEOUT 5)
            if(NOT _regular EQUAL 0)
                message(FATAL_ERROR "Build-tree toolchain identity must be a regular unlinked file")
            endif()
        endif()
        file(SIZE "${_stamp}" _stamp_size)
        if(_stamp_size GREATER 8192)
            message(FATAL_ERROR "Build-tree toolchain identity exceeds its size limit")
        endif()
        file(READ "${_stamp}" _configured_identity LIMIT 8192)
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
