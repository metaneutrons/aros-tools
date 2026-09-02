# Shared bridge for board boot cores which still consume legacy-generated
# kernel/exec/task KOBJs. Each configured build compiles this architecture-
# neutral source set with its selected target compiler.

function(aros_add_board_autoinit target)
    if(TARGET "${target}")
        return()
    endif()

    set(_sources
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/functions.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/libraries_nolibs.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/libraries.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/__showerror.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/commandline.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/commandname.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/_programname.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/__stdiowin.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/stdiowin.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/fromwb.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/initexitsets.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/startupvars.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/programentries.c"
        "${CMAKE_SOURCE_DIR}/compiler/autoinit/detach.c")
    add_library("${target}" STATIC ${_sources})
    target_include_directories("${target}" PRIVATE
        "${CMAKE_SOURCE_DIR}/compiler/autoinit"
        "${CMAKE_SOURCE_DIR}/rom/exec")
    target_compile_options("${target}" PRIVATE -fno-stack-protector)
endfunction()
