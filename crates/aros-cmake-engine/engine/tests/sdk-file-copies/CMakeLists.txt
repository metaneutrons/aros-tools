cmake_minimum_required(VERSION 3.22)
project(SdkFileCopiesFixture NONE)

foreach(_required IN ITEMS AROS_ENGINE_DIR AROS_SDK_COPY_FIXTURE_DIR
        AROS_SDK_COPY_PYTHON)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "SDK file-copy fixture requires ${_required}")
    endif()
endforeach()

set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}/source tree")
if(AROS_SDK_COPY_PROBE STREQUAL "source-destination-overlap")
    set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}")
endif()
set(_local_root "${AROS_SOURCE_DIR}/fd")
set(_foreign_root "${CMAKE_BINARY_DIR}/foreign")
set(_fd_root "${CMAKE_BINARY_DIR}/SYS/Developer/SDK/fd")
set(AROS_DEVELOPER_FD_DIR "${_fd_root}")
set(_destination "${_fd_root}/serial")
set(AROS_DEVELOPER_BIN_DIR "${CMAKE_BINARY_DIR}/SYS/Developer/bin")
set(AROS_DEVELOPER_MAN1_DIR "${CMAKE_BINARY_DIR}/SYS/Developer/man/man1")
if(AROS_SDK_COPY_DESTINATION_KIND STREQUAL "bin")
    set(_destination "${AROS_DEVELOPER_BIN_DIR}")
elseif(AROS_SDK_COPY_DESTINATION_KIND STREQUAL "man1")
    set(_destination "${AROS_DEVELOPER_MAN1_DIR}")
elseif(AROS_SDK_COPY_DESTINATION_KIND)
    message(FATAL_ERROR "unknown fixture destination kind")
endif()
if(AROS_SDK_COPY_PROBE STREQUAL "destination-subdir")
    string(APPEND _destination "/not-exact")
endif()
set(_fetch_root "${CMAKE_BINARY_DIR}/Ports/serialized fd")
set(_fetch_source "${_fetch_root}/includes")
set(_fetch_stamp "${_fetch_root}/.complete")

file(MAKE_DIRECTORY "${_local_root}" "${_foreign_root}" "${_fd_root}")
if(NOT EXISTS "${_local_root}/local-a.fd")
    file(WRITE "${_local_root}/local-a.fd" "local-a;dollar=$value\r\n")
endif()
if(NOT EXISTS "${_local_root}/local-b.fd")
    file(WRITE "${_local_root}/local-b.fd" "local-b bytes\n")
endif()
if(AROS_SDK_COPY_DESTINATION_KIND STREQUAL "bin")
    file(CHMOD "${_local_root}/local-a.fd" PERMISSIONS
        OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
    file(CHMOD "${_local_root}/local-b.fd" PERMISSIONS OWNER_READ OWNER_WRITE GROUP_READ WORLD_READ)
endif()
if(NOT EXISTS "${_foreign_root}/foreign.fd")
    file(WRITE "${_foreign_root}/foreign.fd" "foreign source\n")
endif()

if(AROS_SDK_COPY_PROBE STREQUAL "source-destination-overlap")
    file(WRITE "${CMAKE_BINARY_DIR}/probe.fd" "overlap source\n")
endif()

if(AROS_SDK_COPY_PROBE STREQUAL "fetch-stamp-outside")
    set(_fetch_stamp "${CMAKE_BINARY_DIR}/outside-stamp")
endif()

if(AROS_SDK_COPY_PROBE)
    add_custom_target(fetch-sdk)
else()
    add_custom_command(
        OUTPUT "${_fetch_stamp}"
        COMMAND "${CMAKE_COMMAND}"
            "-DFETCH_ROOT=${_fetch_root}"
            "-DPYTHON=${AROS_SDK_COPY_PYTHON}"
            "-DFIXTURE_DIR=${AROS_SDK_COPY_FIXTURE_DIR}"
            -P "${AROS_SDK_COPY_FIXTURE_DIR}/CreateFetched.cmake"
        DEPENDS
            "${AROS_SDK_COPY_FIXTURE_DIR}/CreateFetched.cmake"
            "${AROS_SDK_COPY_FIXTURE_DIR}/WriteBytes.py"
        COMMENT "Fetching SDK file-copy inputs"
        VERBATIM)
    add_custom_target(fetch-sdk DEPENDS "${_fetch_stamp}")
endif()
set_property(TARGET fetch-sdk PROPERTY AROS_FETCH_DESTINATION "${_fetch_root}")
set_property(TARGET fetch-sdk PROPERTY AROS_FETCH_COMPLETION_STAMP "${_fetch_stamp}")
# A %fetch without destination= registers "." (rom/hidds/pci does). It names
# the build root and can never own an SDK copy's fetch directory, so the
# ownership scan must resolve it instead of refusing the whole configuration.
add_custom_target(fetch-relative)
set_property(TARGET fetch-relative PROPERTY AROS_FETCH_DESTINATION ".")
set_property(GLOBAL PROPERTY AROS_FETCH_TARGETS fetch-sdk fetch-relative)

include("${AROS_ENGINE_DIR}/SdkFileCopies.cmake")

if(AROS_SDK_COPY_PROBE)
    set(_name "probe")
    set(_source "${_local_root}")
    set(_probe_files local-a.fd)
    if(AROS_SDK_COPY_PROBE STREQUAL "duplicate-scalar")
        aros_stage_sdk_files(
            NAME probe NAME repeated SOURCE "${_source}"
            DESTINATION "${_destination}" FILES ${_probe_files})
    elseif(AROS_SDK_COPY_PROBE STREQUAL "duplicate-fetch")
        aros_stage_sdk_files(
            NAME probe SOURCE "${_source}" DESTINATION "${_destination}"
            FETCH fetch-sdk FETCH fetch-sdk FILES ${_probe_files})
    elseif(AROS_SDK_COPY_PROBE STREQUAL "fetch-stamp-outside")
        aros_stage_sdk_files(
            NAME probe SOURCE "${_fetch_source}" DESTINATION "${_destination}"
            FETCH fetch-sdk FILES fetched-a.fd)
    elseif(AROS_SDK_COPY_PROBE STREQUAL "traversal")
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${_destination}" FILES ../local-a.fd)
    elseif(AROS_SDK_COPY_PROBE STREQUAL "wildcard")
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${_destination}" FILES "*.fd")
    elseif(AROS_SDK_COPY_PROBE STREQUAL "option-basename")
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${_destination}" FILES -leading.fd)
    elseif(AROS_SDK_COPY_PROBE STREQUAL "control-basename")
        set(_bad_basename "bad\nname.fd")
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${_destination}" FILES "${_bad_basename}")
    elseif(AROS_SDK_COPY_PROBE STREQUAL "semicolon-basename")
        set(_bad_basename "one;two.fd")
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${_destination}" FILES "${_bad_basename}")
    elseif(AROS_SDK_COPY_PROBE STREQUAL "duplicate-files")
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${_destination}" FILES local-a.fd local-a.fd)
    elseif(AROS_SDK_COPY_PROBE STREQUAL "outside-source")
        aros_stage_sdk_files(NAME probe SOURCE "${_foreign_root}"
            DESTINATION "${_destination}" FILES foreign.fd)
    elseif(AROS_SDK_COPY_PROBE STREQUAL "destination-subdir")
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${_destination}" FILES local-a.fd)
    elseif(AROS_SDK_COPY_PROBE STREQUAL "outside-destination")
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${CMAKE_BINARY_DIR}/outside" FILES local-a.fd)
    elseif(AROS_SDK_COPY_PROBE STREQUAL "source-destination-overlap")
        aros_stage_sdk_files(NAME probe SOURCE "${CMAKE_BINARY_DIR}"
            DESTINATION "${_destination}" FILES probe.fd)
    elseif(AROS_SDK_COPY_PROBE STREQUAL "symlink-source")
        file(WRITE "${CMAKE_BINARY_DIR}/outside-source.fd" "external\n")
        file(CREATE_LINK "${CMAKE_BINARY_DIR}/outside-source.fd"
            "${_local_root}/linked.fd" SYMBOLIC RESULT _link_result)
        if(NOT _link_result STREQUAL "0")
            message(FATAL_ERROR "could not create source symlink: ${_link_result}")
        endif()
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${_destination}" FILES linked.fd)
    elseif(AROS_SDK_COPY_PROBE STREQUAL "fifo-source")
        find_program(_mkfifo NAMES mkfifo PATHS /usr/bin /bin NO_DEFAULT_PATH REQUIRED)
        execute_process(COMMAND "${_mkfifo}" "${_local_root}/pipe.fd"
            RESULT_VARIABLE _mkfifo_result)
        if(NOT _mkfifo_result EQUAL 0)
            message(FATAL_ERROR "could not create source FIFO")
        endif()
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${_destination}" FILES pipe.fd)
    elseif(AROS_SDK_COPY_PROBE STREQUAL "symlink-output")
        file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/outside")
        file(CREATE_LINK "${CMAKE_BINARY_DIR}/outside" "${_fd_root}/redirect"
            SYMBOLIC RESULT _link_result)
        if(NOT _link_result STREQUAL "0")
            message(FATAL_ERROR "could not create output symlink: ${_link_result}")
        endif()
        set(_destination "${_fd_root}/redirect")
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${_destination}" FILES local-a.fd)
    elseif(AROS_SDK_COPY_PROBE STREQUAL "duplicate-output")
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${_destination}" FILES local-a.fd)
        aros_stage_sdk_files(NAME probe SOURCE "${_source}"
            DESTINATION "${_destination}" FILES local-a.fd)
    else()
        message(FATAL_ERROR "unknown SDK file-copy probe ${AROS_SDK_COPY_PROBE}")
    endif()
    return()
endif()

aros_stage_sdk_files(
    NAME sdk-file-copy-products
    SOURCE "${_local_root}"
    DESTINATION "${_destination}"
    FILES local-a.fd local-b.fd)
aros_stage_sdk_files(
    NAME sdk-file-copy-products
    SOURCE "${_fetch_source}"
    DESTINATION "${_destination}"
    FETCH fetch-sdk
    FILES fetched-a.fd fetched-b.fd)
add_custom_target(all-sdk-files)
add_dependencies(all-sdk-files sdk-file-copy-products)
