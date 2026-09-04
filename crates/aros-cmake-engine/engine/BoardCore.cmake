# Shared bridge for board boot cores which still consume legacy-generated
# kernel/exec/task KOBJs. Each configured build compiles this architecture-
# neutral source set with its selected target compiler.

function(aros_add_board_autoinit target)
    if(TARGET "${target}")
        return()
    endif()

    set(_sources
        "${AROS_SOURCE_DIR}/compiler/autoinit/functions.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/libraries_nolibs.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/libraries.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/__showerror.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/commandline.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/commandname.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/_programname.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/__stdiowin.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/stdiowin.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/fromwb.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/initexitsets.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/startupvars.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/programentries.c"
        "${AROS_SOURCE_DIR}/compiler/autoinit/detach.c")
    add_library("${target}" STATIC ${_sources})
    target_include_directories("${target}" PRIVATE
        "${AROS_SOURCE_DIR}/compiler/autoinit"
        "${AROS_SOURCE_DIR}/rom/exec")
    target_compile_options("${target}" PRIVATE -fno-stack-protector)
endfunction()
