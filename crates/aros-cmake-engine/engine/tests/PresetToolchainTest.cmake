cmake_minimum_required(VERSION 3.22)

get_filename_component(_root "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)
file(READ "${_root}/CMakePresets.json" _presets)
string(JSON _count LENGTH "${_presets}" configurePresets)
if(_count LESS 1)
    message(FATAL_ERROR "CMakePresets.json has no configure presets")
endif()

math(EXPR _last "${_count} - 1")
foreach(_index RANGE 0 ${_last})
    string(JSON _name GET "${_presets}" configurePresets ${_index} name)
    string(JSON _toolchain ERROR_VARIABLE _error
        GET "${_presets}" configurePresets ${_index} cacheVariables AROS_TOOLCHAIN)
    if(_error OR NOT _toolchain STREQUAL "llvm")
        message(FATAL_ERROR
            "configure preset ${_name} must pin AROS_TOOLCHAIN=llvm; "
            "otherwise GCC and Clang hosts generate different MetaMake graphs")
    endif()
    string(JSON _platform GET "${_presets}"
        configurePresets ${_index} cacheVariables AROS_TARGET_PLATFORM)
    string(JSON _bootloader ERROR_VARIABLE _error
        GET "${_presets}" configurePresets ${_index}
        cacheVariables AROS_TARGET_BOOTLOADER)
    if(_platform STREQUAL "pc")
        set(_expected_bootloader "grub2gfx")
    else()
        set(_expected_bootloader "")
    endif()
    if(_error OR NOT "${_bootloader}" STREQUAL "${_expected_bootloader}")
        message(FATAL_ERROR
            "configure preset ${_name} must pin "
            "AROS_TARGET_BOOTLOADER=${_expected_bootloader}; target coverage "
            "must not infer which legacy bootloader lane is active")
    endif()
    set(_compilers CMAKE_C_COMPILER CMAKE_CXX_COMPILER CMAKE_ASM_COMPILER)
    set(_expected clang clang++ clang)
    foreach(_compiler _want IN ZIP_LISTS _compilers _expected)
        string(JSON _value ERROR_VARIABLE _error
            GET "${_presets}" configurePresets ${_index} cacheVariables ${_compiler})
        if(_error OR NOT "${_value}" STREQUAL "${_want}")
            message(FATAL_ERROR
                "configure preset ${_name} must pin ${_compiler}=${_want}; "
                "the direct build cannot feed LLVM target flags to a host GCC")
        endif()
    endforeach()
endforeach()

message(STATUS "configure presets pin Clang, target toolchain and bootloader independently of the host")
