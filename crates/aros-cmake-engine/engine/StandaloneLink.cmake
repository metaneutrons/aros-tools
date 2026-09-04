# A program that is not an AROS module: a standalone executable linked through
# the compiler driver, at a fixed address, with its own linker script.
#
# The PC bootstrap is the case. Its own declaration says why it cannot go
# through the ordinary path (arch/all-pc/bootstrap/mmakefile.src:25): the AROS
# triple links via collect-aros, which emits relocatable modules and ignores the
# linker script. So it states `<mmake>_LINK` explicitly, which
# config/make.tmpl:342 offers for exactly this, and
# config/make.tmpl:1117 passes to the link.
#
# Our own link rule is globally `ld.lld -r` (cmake/AROS.cmake:244) and CMake has
# no per-target link rule, so this is a custom command. That is not a workaround
# for CMake: the reference has a distinct recipe here too.
#
# Two phases, because the wrapped vesa binary is attached to the program after
# every program target exists:
#
#   aros_declare_standalone_link   at program creation, with the object library
#   aros_finalize_standalone_links from CMakeLists.txt, after the generated file

include_guard(GLOBAL)
include(CMakeParseArguments)

# Which of a declaration's driver link options carries a linker script. Its
# presence is what makes a program standalone; nothing else in the tree links
# one.
function(aros_standalone_link_wanted out_var)
    set(${out_var} FALSE PARENT_SCOPE)
    foreach(_option IN LISTS ARGN)
        if(_option MATCHES "^-Wl,-T,")
            set(${out_var} TRUE PARENT_SCOPE)
            return()
        endif()
    endforeach()
endfunction()

# aros_declare_standalone_link(NAME <mmake> OBJECTS <object-library>
#     OUTPUT <path> USELIBS <names...> LINK_OPTIONS <opts...>
#     DRIVER_LINK_OPTIONS <opts...> ISA_LINK_OPTIONS <opts...>)
function(aros_declare_standalone_link)
    set(oneValueArgs NAME OBJECTS OUTPUT)
    set(multiValueArgs USELIBS LINK_OPTIONS DRIVER_LINK_OPTIONS ISA_LINK_OPTIONS)
    cmake_parse_arguments(SL "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    foreach(_required NAME OBJECTS OUTPUT)
        if(NOT SL_${_required})
            message(FATAL_ERROR
                "aros_declare_standalone_link: ${_required} is required")
        endif()
    endforeach()
    set_property(GLOBAL APPEND PROPERTY AROS_STANDALONE_LINKS "${SL_NAME}")
    foreach(_field OBJECTS OUTPUT USELIBS LINK_OPTIONS DRIVER_LINK_OPTIONS
            ISA_LINK_OPTIONS)
        set_property(GLOBAL PROPERTY
            "AROS_STANDALONE_${_field}_${SL_NAME}" "${SL_${_field}}")
    endforeach()
endfunction()

# The archive targets a `-L<dir> -l<name>` pair names.
#
# Resolved by output directory and output name, not by the graph's uselib
# resolution: the bootstrap wants the 32-bit flavour, and both flavours publish
# the same archive name, so the graph's preference for the native one is wrong
# here. The `-L` in the declaration is what distinguishes them.
function(_aros_standalone_archive_targets out_var directories names)
    set(_targets "")
    _aros_collect_targets("${CMAKE_SOURCE_DIR}" _all)
    foreach(_target IN LISTS _all)
        get_target_property(_type "${_target}" TYPE)
        if(NOT _type STREQUAL "STATIC_LIBRARY")
            continue()
        endif()
        get_target_property(_name "${_target}" OUTPUT_NAME)
        if(NOT _name OR _name STREQUAL "_name-NOTFOUND")
            set(_name "${_target}")
        endif()
        if(NOT _name IN_LIST names)
            continue()
        endif()
        get_target_property(_dir "${_target}" ARCHIVE_OUTPUT_DIRECTORY)
        if(NOT _dir OR _dir STREQUAL "_dir-NOTFOUND")
            continue()
        endif()
        cmake_path(NORMAL_PATH _dir)
        foreach(_wanted IN LISTS directories)
            cmake_path(NORMAL_PATH _wanted)
            if(_dir STREQUAL _wanted AND NOT _target IN_LIST _targets)
                list(APPEND _targets "${_target}")
            endif()
        endforeach()
    endforeach()
    set(${out_var} "${_targets}" PARENT_SCOPE)
endfunction()

# aros_finalize_standalone_links()
function(aros_finalize_standalone_links)
    get_property(_names GLOBAL PROPERTY AROS_STANDALONE_LINKS)
    if(NOT _names)
        return()
    endif()
    list(REMOVE_DUPLICATES _names)
    set(_gaps "")

    foreach(_name IN LISTS _names)
        foreach(_field OBJECTS OUTPUT USELIBS LINK_OPTIONS DRIVER_LINK_OPTIONS
                ISA_LINK_OPTIONS)
            get_property(_${_field} GLOBAL PROPERTY
                "AROS_STANDALONE_${_field}_${_name}")
        endforeach()
        if(NOT TARGET "${_OBJECTS}")
            list(APPEND _gaps "${_name}: object library ${_OBJECTS} is missing")
            continue()
        endif()
        get_target_property(_foreign_arch "${_OBJECTS}" AROS_FOREIGN_ARCH)

        # A standalone link is the only thing in the tree compiling for a
        # second architecture, so it is the only consumer of a host-tool
        # header: the bootstrap's sources reach aros/i386/libcall.h, which
        # arch/i386-all/include/aros/cpu.h:148 asks for and gencall_i386
        # writes. Ordered here rather than at the object library's creation,
        # because the header declarations are read after every target.
        get_property(_host_headers GLOBAL PROPERTY AROS_HOST_HEADER_TARGETS)
        if(_host_headers)
            add_dependencies("${_OBJECTS}" ${_host_headers})
        endif()

        # `-L` directories from the declaration decide which flavour of an
        # archive a `-l` name refers to.
        set(_dirs "")
        foreach(_option IN LISTS _LINK_OPTIONS)
            if(_option MATCHES "^-L(.+)$")
                list(APPEND _dirs "${CMAKE_MATCH_1}")
            endif()
        endforeach()
        _aros_standalone_archive_targets(_archives "${_dirs}" "${_USELIBS}")
        set(_missing "")
        foreach(_lib IN LISTS _USELIBS)
            set(_found FALSE)
            foreach(_archive IN LISTS _archives)
                get_target_property(_out "${_archive}" OUTPUT_NAME)
                if(_out STREQUAL "${_lib}")
                    set(_found TRUE)
                endif()
            endforeach()
            if(NOT _found)
                list(APPEND _missing "${_lib}")
            endif()
        endforeach()
        if(_missing)
            list(APPEND _gaps
                "${_name}: no archive in ${_dirs} for ${_missing}")
        endif()

        # Objects the build wrapped and attached to this program, which
        # $<TARGET_OBJECTS:> does not carry.
        get_property(_external GLOBAL PROPERTY
            "AROS_BINARY_OBJECTS_FOR_${_name}")

        set(_lib_args "")
        foreach(_lib IN LISTS _USELIBS)
            list(APPEND _lib_args "-l${_lib}")
        endforeach()

        get_filename_component(_out_dir "${_OUTPUT}" DIRECTORY)
        # A `-Wl,-Map,<path>` writes beside the executable and the linker does
        # not create the directory for it.
        set(_needed_dirs "${_out_dir}")
        foreach(_option IN LISTS _DRIVER_LINK_OPTIONS)
            if(_option MATCHES "^-Wl,-Map,(.+)$")
                get_filename_component(_map_dir "${CMAKE_MATCH_1}" DIRECTORY)
                if(_map_dir AND NOT _map_dir IN_LIST _needed_dirs)
                    list(APPEND _needed_dirs "${_map_dir}")
                endif()
            endif()
        endforeach()
        add_custom_command(
            OUTPUT "${_OUTPUT}"
            COMMAND "${CMAKE_COMMAND}" -E make_directory ${_needed_dirs}
            # Selecting the prefix-owned linker is a host-specific addition,
            # not in the reference recipe. Driving clang for an i386-linux
            # triple on a macOS host otherwise picks the host linker, which
            # rejects every GNU option the declaration passes:
            #
            #   ld: unknown options: --hash-style=gnu --eh-frame-hdr
            #   -dynamic-linker -N -Map -T
            #
            # Clang 11 predates --ld-path, but accepts an absolute linker via
            # -fuse-ld=<path>. Naming the prefix-owned ld.lld explicitly is the
            # same deterministic choice the module rule makes
            # (cmake/AROS.cmake:236); it does not depend on PATH.
            # -no-pie is the third host-toolchain addition. clang defaults to
            # PIE for a linux triple, and a position-independent image cannot
            # be what a linker script places at a fixed address:
            #
            #   ld.lld: error: relocation R_386_32 cannot be used against
            #   symbol 'scr_Width'; recompile with -fPIC
            #
            # The reference never states it because its driver defaults differ.
            COMMAND "${CMAKE_C_COMPILER}" "-fuse-ld=${AROS_LLD_BIN}" -no-pie
                ${_ISA_LINK_OPTIONS}
                "$<TARGET_OBJECTS:${_OBJECTS}>" ${_external}
                ${_DRIVER_LINK_OPTIONS} ${_LINK_OPTIONS} ${_lib_args}
                -o "${_OUTPUT}"
            DEPENDS "${_OBJECTS}" ${_external} ${_archives}
            COMMENT "Standalone link ${_name}"
            COMMAND_EXPAND_LISTS
            VERBATIM)
        if(NOT TARGET "${_name}")
            if(_foreign_arch)
                add_custom_target("${_name}" DEPENDS "${_OUTPUT}")
                set_property(TARGET "${_name}" PROPERTY AROS_FOREIGN_ARCH TRUE)
            else()
                add_custom_target("${_name}" ALL DEPENDS "${_OUTPUT}")
            endif()
        else()
            if(_foreign_arch)
                add_custom_target("${_name}-standalone" DEPENDS "${_OUTPUT}")
                set_property(TARGET "${_name}-standalone" PROPERTY
                    AROS_FOREIGN_ARCH TRUE)
            else()
                add_custom_target("${_name}-standalone" ALL DEPENDS "${_OUTPUT}")
            endif()
        endif()
    endforeach()

    set(_report "${CMAKE_BINARY_DIR}/generated_targets.standalone-link-gaps.txt")
    if(_gaps)
        list(SORT _gaps)
        string(REPLACE ";" "\n" _body "${_gaps}")
        file(WRITE "${_report}" "${_body}\n")
        list(LENGTH _gaps _count)
        message(STATUS
            "⚠️  ${_count} standalone link gap(s) -> ${_report}")
    else()
        file(REMOVE "${_report}")
    endif()
endfunction()
