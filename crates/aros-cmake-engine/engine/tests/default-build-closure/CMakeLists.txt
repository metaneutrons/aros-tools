cmake_minimum_required(VERSION 3.22)
project(DefaultBuildClosure C)

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
include("${CMAKE_CURRENT_LIST_DIR}/../../DefaultBuildClosure.cmake")

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

aros_limit_all_to_metamake_root(AROS)

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
