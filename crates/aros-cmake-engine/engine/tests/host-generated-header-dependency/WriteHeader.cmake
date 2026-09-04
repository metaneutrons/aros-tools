cmake_minimum_required(VERSION 3.22)

if(NOT OUTPUT)
    message(FATAL_ERROR "OUTPUT is required")
endif()
get_filename_component(_directory "${OUTPUT}" DIRECTORY)
file(MAKE_DIRECTORY "${_directory}")
file(WRITE "${OUTPUT}"
    "#ifndef AROS_I386_LIBCALL_H\n"
    "#define AROS_I386_LIBCALL_H\n"
    "#define AROS_FIXTURE_LIBCALL 1\n"
    "#endif\n")
