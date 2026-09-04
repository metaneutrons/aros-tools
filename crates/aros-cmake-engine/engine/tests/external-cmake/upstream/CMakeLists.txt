cmake_minimum_required(VERSION 3.22)
project(ExternalTiny C)

add_library(tiny STATIC tiny.c)
target_include_directories(tiny PUBLIC
    "$<BUILD_INTERFACE:${CMAKE_CURRENT_LIST_DIR}>"
    "$<INSTALL_INTERFACE:include>")

install(TARGETS tiny ARCHIVE DESTINATION lib)
install(FILES tiny.h DESTINATION include)
install(FILES metadata.txt DESTINATION share/tiny)
