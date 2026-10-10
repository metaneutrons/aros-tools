cmake_minimum_required(VERSION 3.22)
project(DefaultBuildClosure C)

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
if(NOT DEFINED CLOSURE_CASE)
    set(CLOSURE_CASE classic)
endif()
include("${CMAKE_CURRENT_LIST_DIR}/../../DefaultBuildClosure.cmake")

if(CLOSURE_CASE STREQUAL "native-build-no-root" OR
   CLOSURE_CASE STREQUAL "native-build-with-root")
    set(AROS_NATIVE_BUILD_CONTRACT fixture-build-contract)
    set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED TRUE)
elseif(CLOSURE_CASE STREQUAL "native-consumer-no-root" OR
       CLOSURE_CASE STREQUAL "native-consumer-with-root")
    set(AROS_NATIVE_CONSUMER_CONTRACT fixture-consumer-contract)
    set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED TRUE)
elseif(CLOSURE_CASE STREQUAL "native-unvalidated")
    set(AROS_NATIVE_BUILD_CONTRACT fixture-unvalidated-build-contract)
elseif(CLOSURE_CASE STREQUAL "native-missing-helper")
    set(AROS_NATIVE_CONSUMER_CONTRACT fixture-consumer-contract)
endif()

# Test double for selected-graph admission only. This fixture does not validate
# or attest either source contract or compiler/ABI identity.
if(NOT CLOSURE_CASE STREQUAL "classic" AND
   NOT CLOSURE_CASE STREQUAL "classic-missing-root" AND
   NOT CLOSURE_CASE STREQUAL "native-missing-helper")
    function(_aros_native_source_selection_validated out_validated)
        if(CLOSURE_CASE STREQUAL "native-build-no-root" OR
           CLOSURE_CASE STREQUAL "native-build-with-root" OR
           CLOSURE_CASE STREQUAL "native-consumer-no-root" OR
           CLOSURE_CASE STREQUAL "native-consumer-with-root")
            set(${out_validated} TRUE PARENT_SCOPE)
        else()
            set(${out_validated} FALSE PARENT_SCOPE)
        endif()
    endfunction()
endif()

set(_has_classic_root FALSE)
if(CLOSURE_CASE STREQUAL "classic" OR
   CLOSURE_CASE STREQUAL "native-build-with-root" OR
   CLOSURE_CASE STREQUAL "native-consumer-with-root")
    set(_has_classic_root TRUE)
endif()

if(_has_classic_root)
    file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/default.c"
        "int default_member(void) { return 0; }\n")
    file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/manual.c"
        "int manual_member(void) { return 0; }\n")
    file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/linked.c"
        "int linked_member(void) { return 0; }\n")

    add_custom_target(AROS)
    add_library(default-member STATIC "${CMAKE_CURRENT_BINARY_DIR}/default.c")
    add_library(manual-member STATIC "${CMAKE_CURRENT_BINARY_DIR}/manual.c")
    add_library(linked-member STATIC "${CMAKE_CURRENT_BINARY_DIR}/linked.c")
    target_link_libraries(default-member PRIVATE linked-member)
    add_dependencies(AROS default-member)
else()
    file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/selected.c"
        "int selected_sdk_member(void) { return 0; }\n")
    if(CLOSURE_CASE STREQUAL "native-build-no-root" OR
       CLOSURE_CASE STREQUAL "native-consumer-no-root")
        add_library(selected-sdk-member STATIC "${CMAKE_CURRENT_BINARY_DIR}/selected.c")
    endif()
endif()

aros_limit_all_for_selected_source_graph(AROS)

if(_has_classic_root)
    get_target_property(_default_excluded default-member EXCLUDE_FROM_ALL)
    if(_default_excluded)
        message(FATAL_ERROR "AROS dependency was incorrectly excluded from all")
    endif()
    get_target_property(_manual_excluded manual-member EXCLUDE_FROM_ALL)
    if(NOT _manual_excluded)
        message(FATAL_ERROR "manual target was not excluded from all")
    endif()
    get_target_property(_linked_excluded linked-member EXCLUDE_FROM_ALL)
    if(NOT _linked_excluded)
        message(FATAL_ERROR
            "link-only target should remain excluded until its reachable consumer pulls it")
    endif()

    get_property(_outside GLOBAL PROPERTY AROS_OUTSIDE_DEFAULT_ROOT_TARGETS)
    list(SORT _outside)
    if(NOT _outside STREQUAL "linked-member;manual-member")
        message(FATAL_ERROR "unexpected outside-root inventory: ${_outside}")
    endif()
elseif(CLOSURE_CASE STREQUAL "native-build-no-root" OR
       CLOSURE_CASE STREQUAL "native-consumer-no-root")
    get_target_property(_selected_excluded selected-sdk-member EXCLUDE_FROM_ALL)
    if(_selected_excluded)
        message(FATAL_ERROR "selected native SDK producer was excluded from all")
    endif()
    get_property(_root_set GLOBAL PROPERTY AROS_METAMAKE_DEFAULT_ROOT SET)
    if(_root_set)
        message(FATAL_ERROR "rootless selected graph unexpectedly applied classic closure")
    endif()
endif()
