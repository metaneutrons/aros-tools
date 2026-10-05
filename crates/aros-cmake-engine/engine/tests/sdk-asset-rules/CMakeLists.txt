cmake_minimum_required(VERSION 3.22)
project(SdkAssetRulesFixture C)

if(AROS_SDK_ASSET_PROBE MATCHES "^mutate-(configured-|runtime-)")
    set(CMAKE_BUILD_TYPE Release)
endif()

foreach(_required IN ITEMS AROS_ENGINE_DIR AROS_SDK_ASSET_FIXTURE_DIR)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "SDK asset fixture requires ${_required}")
    endif()
endforeach()

set(_developer "${CMAKE_BINARY_DIR}/SYS/Developer")
set(AROS_DEVELOPER_FD_DIR "${_developer}/SDK/fd")
set(AROS_DEVELOPER_BIN_DIR "${_developer}/bin")
set(AROS_DEVELOPER_MAN1_DIR "${_developer}/man/man1")
set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}/source tree")
set(_source "${AROS_SOURCE_DIR}/sdk")
file(MAKE_DIRECTORY "${_source}" "${AROS_DEVELOPER_FD_DIR}")
file(COPY_FILE "${AROS_SDK_ASSET_FIXTURE_DIR}/sdk-script" "${_source}/sdk-script"
    ONLY_IF_DIFFERENT)
file(CHMOD "${_source}/sdk-script" PERMISSIONS
    OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)

include("${AROS_ENGINE_DIR}/SdkAssetRules.cmake")

if(AROS_SDK_ASSET_PROBE STREQUAL "symlink-root")
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/outside-bin")
    file(REMOVE "${AROS_DEVELOPER_BIN_DIR}")
    file(CREATE_LINK "${CMAKE_BINARY_DIR}/outside-bin" "${AROS_DEVELOPER_BIN_DIR}"
        SYMBOLIC RESULT _link_result)
    if(NOT _link_result STREQUAL "0")
        message(FATAL_ERROR "could not create bin root symlink: ${_link_result}")
    endif()
endif()

if(AROS_SDK_ASSET_PROBE STREQUAL "unregistered-producer")
    add_custom_target(fake-producer)
    aros_sdk_asset_rule(NAME alias INPUT "${AROS_DEVELOPER_BIN_DIR}/unregistered"
        PRODUCER fake-producer OUTPUT "${AROS_DEVELOPER_BIN_DIR}/alias")
    return()
elseif(AROS_SDK_ASSET_PROBE STREQUAL "missing-producer")
    aros_sdk_asset_rule(NAME alias INPUT "${AROS_DEVELOPER_BIN_DIR}/missing"
        PRODUCER missing-producer OUTPUT "${AROS_DEVELOPER_BIN_DIR}/alias")
    return()
elseif(AROS_SDK_ASSET_PROBE STREQUAL "output-escape")
    aros_sdk_asset_rule(NAME escape TEXT_HEX 41
        OUTPUT "${AROS_DEVELOPER_BIN_DIR}/../escaped")
    return()
elseif(AROS_SDK_ASSET_PROBE STREQUAL "input-escape")
    add_custom_target(fake-producer)
    aros_sdk_asset_rule(NAME escape INPUT "${AROS_DEVELOPER_BIN_DIR}/../outside"
        PRODUCER fake-producer OUTPUT "${AROS_DEVELOPER_BIN_DIR}/alias")
    return()
elseif(AROS_SDK_ASSET_PROBE STREQUAL "output-overlap")
    aros_sdk_asset_rule(NAME overlap TEXT_HEX 41
        OUTPUT "${AROS_DEVELOPER_BIN_DIR}/nested/overlap")
    return()
elseif(AROS_SDK_ASSET_PROBE STREQUAL "text-control")
    aros_sdk_asset_rule(NAME control TEXT_HEX 410a42
        OUTPUT "${AROS_DEVELOPER_MAN1_DIR}/control.1")
    return()
elseif(AROS_SDK_ASSET_PROBE STREQUAL "text-operation-conflict")
    aros_sdk_asset_rule(NAME conflict INPUT "${AROS_DEVELOPER_BIN_DIR}/source"
        PRODUCER fake-producer TEXT_HEX 41 OUTPUT "${AROS_DEVELOPER_BIN_DIR}/conflict")
    return()
elseif(AROS_SDK_ASSET_PROBE STREQUAL "same-producer")
    add_custom_target(producer)
    _aros_register_sdk_asset_output(producer "${AROS_DEVELOPER_BIN_DIR}/source")
    aros_sdk_asset_rule(NAME producer INPUT "${AROS_DEVELOPER_BIN_DIR}/source"
        PRODUCER producer OUTPUT "${AROS_DEVELOPER_BIN_DIR}/alias")
    return()
elseif(AROS_SDK_ASSET_PROBE STREQUAL "duplicate-keyword")
    aros_sdk_asset_rule(NAME first NAME second TEXT_HEX 41
        OUTPUT "${AROS_DEVELOPER_MAN1_DIR}/duplicate.1")
    return()
elseif(AROS_SDK_ASSET_PROBE STREQUAL "overlap-bin-contains-man")
    set(AROS_DEVELOPER_BIN_DIR "${_developer}/shared")
    set(AROS_DEVELOPER_MAN1_DIR "${_developer}/shared/man1")
    aros_sdk_asset_rule(NAME overlap TEXT_HEX 41
        OUTPUT "${AROS_DEVELOPER_MAN1_DIR}/overlap.1")
    return()
elseif(AROS_SDK_ASSET_PROBE STREQUAL "overlap-man-contains-bin")
    set(AROS_DEVELOPER_MAN1_DIR "${_developer}/shared")
    set(AROS_DEVELOPER_BIN_DIR "${_developer}/shared/bin")
    aros_sdk_asset_rule(NAME overlap TEXT_HEX 41
        OUTPUT "${AROS_DEVELOPER_MAN1_DIR}/overlap.1")
    return()
elseif(AROS_SDK_ASSET_PROBE STREQUAL "case-collision")
    aros_sdk_asset_rule(NAME lower TEXT_HEX 41
        OUTPUT "${AROS_DEVELOPER_MAN1_DIR}/same-name")
    aros_sdk_asset_rule(NAME upper TEXT_HEX 42
        OUTPUT "${AROS_DEVELOPER_MAN1_DIR}/SAME-NAME")
    return()
endif()

if(AROS_SDK_ASSET_PROBE STREQUAL "ambiguous-program")
    add_custom_target(other-program)
    _aros_register_sdk_asset_output(other-program "${AROS_DEVELOPER_BIN_DIR}/registered-program")
    add_executable(registered-program "${AROS_SDK_ASSET_FIXTURE_DIR}/program.c")
    set_target_properties(registered-program PROPERTIES
        RUNTIME_OUTPUT_DIRECTORY "${AROS_DEVELOPER_BIN_DIR}"
        OUTPUT_NAME "registered-program")
    aros_register_sdk_program_file(NAME registered-program
        OUTPUT "${AROS_DEVELOPER_BIN_DIR}/registered-program")
    return()
endif()

aros_stage_sdk_files(NAME script-producer SOURCE "${_source}"
    DESTINATION "${AROS_DEVELOPER_BIN_DIR}" FILES sdk-script)
aros_sdk_asset_rule(NAME sdk-script-alias
    INPUT "${AROS_DEVELOPER_BIN_DIR}/sdk-script"
    PRODUCER script-producer OUTPUT "${AROS_DEVELOPER_BIN_DIR}/sdk-script-alias")

set(_program_source "${CMAKE_BINARY_DIR}/program.c")
if(AROS_SDK_ASSET_PROBE STREQUAL "mutate-producer-after-registration")
    file(WRITE "${_program_source}" "int main(void) { return 17; }\n")
else()
    file(COPY_FILE "${AROS_SDK_ASSET_FIXTURE_DIR}/program.c" "${_program_source}"
        ONLY_IF_DIFFERENT)
endif()
add_executable(program-target "${_program_source}")
set_target_properties(program-target PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${AROS_DEVELOPER_BIN_DIR}"
    OUTPUT_NAME "registered-program"
    PREFIX ""
    SUFFIX "")
aros_register_sdk_program_file(NAME program-target
    OUTPUT "${AROS_DEVELOPER_BIN_DIR}/registered-program")
if(AROS_SDK_ASSET_PROBE STREQUAL "mutate-producer-after-registration")
    set_property(TARGET program-target PROPERTY OUTPUT_NAME "mutated-program")
elseif(AROS_SDK_ASSET_PROBE STREQUAL "mutate-configured-directory")
    set_property(TARGET program-target PROPERTY RUNTIME_OUTPUT_DIRECTORY_RELEASE "${CMAKE_BINARY_DIR}/elsewhere")
elseif(AROS_SDK_ASSET_PROBE STREQUAL "mutate-configured-name")
    set_property(TARGET program-target PROPERTY OUTPUT_NAME_RELEASE "other")
elseif(AROS_SDK_ASSET_PROBE STREQUAL "mutate-runtime-name")
    set_property(TARGET program-target PROPERTY RUNTIME_OUTPUT_NAME "other")
elseif(AROS_SDK_ASSET_PROBE STREQUAL "mutate-runtime-configured-name")
    set_property(TARGET program-target PROPERTY RUNTIME_OUTPUT_NAME_RELEASE "other")
elseif(AROS_SDK_ASSET_PROBE STREQUAL "mutate-configured-postfix")
    set_property(TARGET program-target PROPERTY RELEASE_POSTFIX "-other")
endif()
aros_sdk_asset_rule(NAME program-alias
    INPUT "${AROS_DEVELOPER_BIN_DIR}/registered-program"
    PRODUCER program-target OUTPUT "${AROS_DEVELOPER_BIN_DIR}/program-alias")

aros_sdk_asset_rule(NAME sdk-man-page
    OUTPUT "${AROS_DEVELOPER_MAN1_DIR}/sdk-assets.1"
    TEXT_HEX 53444b206173736574206d616e75616c)

add_custom_target(all-sdk-assets)
add_dependencies(all-sdk-assets sdk-script-alias program-alias sdk-man-page)
