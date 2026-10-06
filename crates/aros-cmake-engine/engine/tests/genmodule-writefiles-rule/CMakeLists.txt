cmake_minimum_required(VERSION 3.24)
project(GenmoduleWritefilesRuleFixture NONE)

foreach(_required AROS_SOURCE_DIR AROS_ENGINE_DIR AROS_HOST_GENMODULE)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "${_required} is required")
    endif()
endforeach()

if(NOT DEFINED AROS_WRITEFILES_PROBE)
    set(AROS_WRITEFILES_PROBE positive)
endif()

# AROS.cmake owns the production endpoint. Stub only the broad SDK bootstrap:
# this project(NONE) fixture needs neither an AROS compiler nor a sysroot.
set(_bootstrap_stub_dir "${CMAKE_BINARY_DIR}/bootstrap-stub")
file(MAKE_DIRECTORY "${_bootstrap_stub_dir}")
file(WRITE "${_bootstrap_stub_dir}/BootstrapSDK.cmake"
    "function(aros_bootstrap_sdk_includes)\nendfunction()\n")
list(INSERT CMAKE_MODULE_PATH 0 "${_bootstrap_stub_dir}")
set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED TRUE)
set(AROS_CMAKE_ENGINE_DIR "${AROS_ENGINE_DIR}")
include("${AROS_ENGINE_DIR}/AROS.cmake")

set(_config_relative "workbench/libs/gl/gl.conf")
set(_config "${AROS_SOURCE_DIR}/${_config_relative}")
cmake_path(NORMAL_PATH _config)

if(AROS_WRITEFILES_PROBE STREQUAL "source-parent-symlink")
    # Keep the target in the same source tree: confinement alone must not admit
    # replacing a declared directory with an alias after the Rust scan.
    set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}/source-parent-probe")
    file(MAKE_DIRECTORY "${AROS_SOURCE_DIR}/real")
    file(COPY "${_config}" DESTINATION "${AROS_SOURCE_DIR}/real")
    file(CREATE_LINK "${AROS_SOURCE_DIR}/real" "${AROS_SOURCE_DIR}/alias"
        SYMBOLIC RESULT _symlink_result)
    if(NOT _symlink_result STREQUAL "0")
        message(FATAL_ERROR "could not create source parent symlink: ${_symlink_result}")
    endif()
    set(_request_config "alias/gl.conf")
elseif(AROS_WRITEFILES_PROBE STREQUAL "traversal")
    set(_request_config "workbench/libs/../libs/gl/gl.conf")
elseif(AROS_WRITEFILES_PROBE STREQUAL "missing-config")
    set(_request_config "workbench/libs/gl/missing.conf")
else()
    set(_request_config "${_config_relative}")
endif()

if(AROS_WRITEFILES_PROBE STREQUAL "output-symlink")
    string(SHA256 _signature "${_config}|gl|library")
    string(SUBSTRING "${_signature}" 0 16 _short_hash)
    set(_writer_parent "${CMAKE_BINARY_DIR}/genmodule/linklibs/${_short_hash}")
    file(MAKE_DIRECTORY "${_writer_parent}")
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/outside-output")
    file(CREATE_LINK "${CMAKE_BINARY_DIR}/outside-output"
        "${_writer_parent}/gen" SYMBOLIC RESULT _symlink_result)
    if(NOT _symlink_result STREQUAL "0")
        message(FATAL_ERROR "could not create output redirect symlink: ${_symlink_result}")
    endif()
endif()

aros_genmodule_writefiles_stamp(
    NAME gl-stubs
    CONFIG "${_request_config}"
    MODULE gl
    MODTYPE library)

if(AROS_WRITEFILES_PROBE STREQUAL "duplicate")
    aros_genmodule_writefiles_stamp(
        NAME gl-stubs
        CONFIG "${_request_config}"
        MODULE gl
        MODTYPE library)
    message(FATAL_ERROR "duplicate writefiles target unexpectedly succeeded")
elseif(AROS_WRITEFILES_PROBE STREQUAL "positive")
    get_property(_stamp_dependencies TARGET gl-stubs
        PROPERTY MANUALLY_ADDED_DEPENDENCIES)
    list(LENGTH _stamp_dependencies _stamp_dependency_count)
    if(NOT _stamp_dependency_count EQUAL 1)
        message(FATAL_ERROR
            "writefiles endpoint must depend on one shared writer: ${_stamp_dependencies}")
    endif()
    list(GET _stamp_dependencies 0 _stamp_writer)

    set(_normal_marker
        "@AROS_GENMODULE|normal|stackstubs,regcallstubs,autoinit,getlibbase|gl|library|${_config}")
    set(_relative_marker
        "@AROS_GENMODULE|rel|stackstubs,regcallstubs,autoinit,getlibbase|gl|library|${_config}")
    _aros_genmodule_linklib_sources(_normal_sources _normal_writer
        _normal_include _normal_config "${AROS_SOURCE_DIR}" "${_normal_marker}")
    _aros_genmodule_linklib_sources(_relative_sources _relative_writer
        _relative_include _relative_config "${AROS_SOURCE_DIR}" "${_relative_marker}")
    if(NOT _stamp_writer STREQUAL _normal_writer OR
       NOT _stamp_writer STREQUAL _relative_writer)
        message(FATAL_ERROR
            "writefiles endpoint, normal marker, and relative marker use different writers: "
            "${_stamp_writer}; ${_normal_writer}; ${_relative_writer}")
    endif()
    if(NOT _normal_sources OR NOT _relative_sources)
        message(FATAL_ERROR
            "normal/relative generated-source marker resolved to no sources")
    endif()

    list(GET _normal_sources 0 _repair_source)
    file(WRITE "${CMAKE_BINARY_DIR}/writefiles-declared-source.txt"
        "${_repair_source}\n")
endif()
