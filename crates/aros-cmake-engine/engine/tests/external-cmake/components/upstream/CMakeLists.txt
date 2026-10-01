cmake_minimum_required(VERSION 3.22)
project(ExternalComponentFixture C)

set(CMAKE_ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/lib")

add_library(unlisted STATIC unlisted.c)
add_library(not_selected STATIC not_selected.c)
add_library(left STATIC left.c left_extra.c)
add_library(right STATIC right.c)
target_link_libraries(left PRIVATE unlisted)

install(FILES left.h DESTINATION include COMPONENT left-headers)
install(FILES right.h DESTINATION include COMPONENT right-headers)
install(FILES unselected.h DESTINATION include COMPONENT unused-headers)
