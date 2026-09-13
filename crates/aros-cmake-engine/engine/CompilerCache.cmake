# AROS compiler-cache launcher contract.
#
# Rust resolves the policy and exact executable once, then passes it through
# AROS_COMPILER_CACHE_MODE and AROS_COMPILER_CACHE_EXECUTABLE. This module must
# never rediscover sccache or ccache: a second PATH lookup could select a
# different backend than the frontend recorded for the build.

macro(_aros_clear_compiler_cache_launchers)
    # CMake supports the compiler-launcher target property for C and C++, but
    # not for ASM. Keep removing a legacy ASM cache variable so an earlier
    # experimental configure cannot leave misleading state behind; it is never
    # set as an active launcher by this module.
    foreach(_aros_language C CXX ASM)
        unset(CMAKE_${_aros_language}_COMPILER_LAUNCHER CACHE)
        unset(CMAKE_${_aros_language}_COMPILER_LAUNCHER)
    endforeach()
endmacro()

macro(_aros_set_compiler_cache_launcher _aros_executable)
    # CMAKE_<LANG>_COMPILER_LAUNCHER is implemented by CMake only for C and
    # C++. Do not fabricate support for ASM through an ignored variable or the
    # internal RULE_LAUNCH_COMPILE escape hatch. Assembly therefore remains a
    # deterministic direct invocation until CMake exposes a supported
    # language-specific launcher mechanism.
    foreach(_aros_language C CXX)
        set(CMAKE_${_aros_language}_COMPILER_LAUNCHER
            "${_aros_executable}"
            CACHE FILEPATH "AROS frontend-selected compiler launcher" FORCE)
    endforeach()
endmacro()

if(NOT DEFINED AROS_COMPILER_CACHE_MODE)
    # Direct engine consumers predate the frontend policy hand-off. They never
    # get a hidden PATH-based launcher: absent policy means explicitly off.
    set(AROS_COMPILER_CACHE_MODE "off")
endif()

if(AROS_COMPILER_CACHE_MODE STREQUAL "off")
    _aros_clear_compiler_cache_launchers()
    unset(AROS_COMPILER_CACHE_EXECUTABLE CACHE)
    unset(AROS_COMPILER_CACHE_EXECUTABLE)
    message(STATUS "AROS: Compiler cache launcher disabled by frontend policy")
elseif(AROS_COMPILER_CACHE_MODE STREQUAL "sccache" OR
       AROS_COMPILER_CACHE_MODE STREQUAL "ccache")
    if(NOT DEFINED AROS_COMPILER_CACHE_EXECUTABLE OR
       AROS_COMPILER_CACHE_EXECUTABLE STREQUAL "")
        message(FATAL_ERROR
            "AROS_COMPILER_CACHE_EXECUTABLE is required when AROS_COMPILER_CACHE_MODE is ${AROS_COMPILER_CACHE_MODE}")
    endif()
    if(NOT IS_ABSOLUTE "${AROS_COMPILER_CACHE_EXECUTABLE}")
        message(FATAL_ERROR
            "AROS_COMPILER_CACHE_EXECUTABLE must be absolute, got '${AROS_COMPILER_CACHE_EXECUTABLE}'")
    endif()
    if(NOT EXISTS "${AROS_COMPILER_CACHE_EXECUTABLE}")
        message(FATAL_ERROR
            "AROS frontend-selected compiler cache executable does not exist: ${AROS_COMPILER_CACHE_EXECUTABLE}")
    endif()
    _aros_set_compiler_cache_launcher("${AROS_COMPILER_CACHE_EXECUTABLE}")
    message(STATUS
        "AROS: Compiler cache launcher selected by frontend (${AROS_COMPILER_CACHE_MODE}) -> ${AROS_COMPILER_CACHE_EXECUTABLE}")
else()
    message(FATAL_ERROR
        "invalid AROS_COMPILER_CACHE_MODE '${AROS_COMPILER_CACHE_MODE}'; expected off, sccache, or ccache")
endif()
