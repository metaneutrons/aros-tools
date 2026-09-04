cmake_minimum_required(VERSION 3.22)
project(aros_smbios_validation C)

if(NOT DEFINED AROS_SOURCE_DIR)
    message(FATAL_ERROR "AROS_SOURCE_DIR is required")
endif()

add_executable(smbios-validation main.c)
target_include_directories(smbios-validation PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}/include"
    "${AROS_SOURCE_DIR}/compiler/include")
