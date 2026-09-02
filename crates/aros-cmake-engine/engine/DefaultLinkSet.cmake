# The default link set the target compiler's spec file appends to every link.
#
# Our link rule is `ld.lld -r` invoked directly (cmake/AROS.cmake:244), so no
# compiler driver applies the spec for us. That spec is where the library bases
# come from:
#
#   config/elf-specs.in:19
#       *lib: %(autolib) %{!nostdc:%{!noposixc:-lposixc} -lstdcio -lstdc}
#             %{!nosysbase:-lexec} %{nostdc:-lstdc.static}
#   compiler/autoinit/auto
#       *autolib: -lmui -lamiga ... -loop -llibinit -lautoinit
#
# lib<mod>.a carries <mod>_autoinit.c, whose AROS_LIBSET
# (compiler/include/aros/symbolsets.h:118) defines the module's library base;
# libexec.a carries `struct ExecBase *SysBase` (rom/exec/exec_autoinit.c:22).
# Modules link with -nostartfiles (configure.in:3468), which suppresses
# *startfile: only, so a module receives the same set a program does.
#
# The transpiler reads and resolves the spec; this file only applies what it
# resolved. Both halves are deliberately dumb about which libraries exist.

include_guard(GLOBAL)

# aros_set_default_link_set(<item>...)
#
# Called by generated_targets.cmake. Each item is
# `<name>|<archive target>|<absent switches>|<present switches>`, switch lists
# comma-separated, in spec order with duplicates preserved.
function(aros_set_default_link_set)
    set_property(GLOBAL PROPERTY AROS_DEFAULT_LINK_SET "${ARGN}")
endfunction()

# Reads generated_targets.spec-switches.txt into <out-prefix>_<mmake> variables.
function(_aros_read_spec_switches out_var)
    set(_manifest "${CMAKE_BINARY_DIR}/generated_targets.spec-switches.txt")
    set(_entries "")
    if(EXISTS "${_manifest}")
        file(STRINGS "${_manifest}" _lines)
        foreach(_line IN LISTS _lines)
            if(_line STREQUAL "")
                continue()
            endif()
            string(REPLACE "\t" ";" _fields "${_line}")
            list(POP_FRONT _fields _mmake)
            if(_mmake AND _fields)
                list(APPEND _entries "${_mmake}=${_fields}")
            endif()
        endforeach()
    endif()
    set(${out_var} "${_entries}" PARENT_SCOPE)
endfunction()

# The checked-in external-driver spec uses `nostdc` to select stdc.static. The
# native AROS GCC and Clang drivers express the same runtime choice with
# `-static`: dynamic posixc/stdcio/stdc are omitted and stdc.static is added.
# Convert that driver fact into the condition spelling used by the parsed spec.
function(_aros_runtime_selection_switches out_var)
    set(_switches ${ARGN})
    if("static" IN_LIST _switches AND NOT "nostdc" IN_LIST _switches)
        list(APPEND _switches "nostdc")
    endif()
    set(${out_var} "${_switches}" PARENT_SCOPE)
endfunction()

# Collects every target in this directory and below.
function(_aros_collect_targets directory out_var)
    get_property(_targets DIRECTORY "${directory}" PROPERTY BUILDSYSTEM_TARGETS)
    get_property(_subdirs DIRECTORY "${directory}" PROPERTY SUBDIRECTORIES)
    foreach(_subdir IN LISTS _subdirs)
        _aros_collect_targets("${_subdir}" _nested)
        list(APPEND _targets ${_nested})
    endforeach()
    set(${out_var} "${_targets}" PARENT_SCOPE)
endfunction()

# aros_default_link_set_files(<out-files> <out-deps> [<switch>...])
#
# The resolved set as archive paths, in spec order, for a link that is a custom
# command rather than a target. The kickstart link needs exactly this: the
# reference links it with $(TARGET_CC) (config/make.tmpl:3904), so the spec
# applies there too, with -nosysbase among its LDFLAGS.
#
# Without it a kickstart image is left asking for LibNextTagItem (libamiga),
# __includelibrarieshandling (libautoinit), set_call_libfuncs (liblibinit) and
# the C runtime, because those archives are deliberately excluded from each
# member's own object (config/make.tmpl:2752).
function(aros_default_link_set_files out_files out_deps)
    get_property(_set GLOBAL PROPERTY AROS_DEFAULT_LINK_SET)
    _aros_runtime_selection_switches(_selection_switches ${ARGN})
    set(_files "")
    set(_deps "")
    foreach(_spec IN LISTS _set)
        string(REPLACE "|" ";" _fields "${_spec}")
        list(LENGTH _fields _field_count)
        if(NOT _field_count EQUAL 4)
            continue()
        endif()
        list(GET _fields 1 _archive)
        list(GET _fields 2 _absent)
        list(GET _fields 3 _present)
        # The switch lists are comma-separated inside the record, because the
        # record itself is pipe-separated. Without this a two-switch item is one
        # string that matches nothing: -lposixc requires both nostdc and
        # noposixc to be absent, and stayed in every link.
        string(REPLACE "," ";" _absent "${_absent}")
        string(REPLACE "," ";" _present "${_present}")
        set(_wanted TRUE)
        foreach(_switch IN LISTS _absent)
            if(_switch IN_LIST _selection_switches)
                set(_wanted FALSE)
            endif()
        endforeach()
        foreach(_switch IN LISTS _present)
            if(NOT _switch IN_LIST _selection_switches)
                set(_wanted FALSE)
            endif()
        endforeach()
        if(NOT _wanted OR NOT TARGET "${_archive}")
            continue()
        endif()
        # Duplicates are kept in the file list, as the spec names -lamiga twice
        # and order decides resolution, but an input edge is needed only once.
        list(APPEND _files "$<TARGET_FILE:${_archive}>")
        if(NOT _archive IN_LIST _deps)
            list(APPEND _deps "${_archive}")
        endif()
    endforeach()
    set(${out_files} "${_files}" PARENT_SCOPE)
    set(${out_deps} "${_deps}" PARENT_SCOPE)
endfunction()

# aros_apply_default_link_set()
#
# Appends the resolved set to every AROS artefact. Every add_executable() in
# this build is an AROS artefact: host tools are ExternalProject-style builds
# (cmake/HostTools.cmake:11), never plain executable targets, and the kickstart
# is a custom command rather than a target.
#
# No --start-group here. The compiler driver passes these archives in spec
# order without one, and a group changes which members are pulled.
function(aros_apply_default_link_set)
    get_property(_set GLOBAL PROPERTY AROS_DEFAULT_LINK_SET)
    if(NOT _set)
        message(FATAL_ERROR
            "aros_apply_default_link_set: the transpiler declared no default "
            "link set; every AROS link would be missing its library bases")
    endif()

    _aros_read_spec_switches(_switch_entries)
    _aros_collect_targets("${CMAKE_SOURCE_DIR}" _all_targets)

    set(_artefacts "")
    foreach(_target IN LISTS _all_targets)
        get_target_property(_type "${_target}" TYPE)
        if(_type STREQUAL "EXECUTABLE")
            list(APPEND _artefacts "${_target}")
        endif()
    endforeach()

    set(_missing_archives "")
    set(_applied 0)
    foreach(_target IN LISTS _artefacts)
        # A declaration's own -nostdc/-noposixc/-nosysbase suppress part of the
        # set, and each exists because it would otherwise link against itself.
        set(_switches "")
        foreach(_entry IN LISTS _switch_entries)
            if(_entry MATCHES "^${_target}=(.*)$")
                set(_switches "${CMAKE_MATCH_1}")
                break()
            endif()
        endforeach()
        _aros_runtime_selection_switches(_selection_switches ${_switches})

        set(_items "")
        set(_seen "")
        foreach(_spec IN LISTS _set)
            string(REPLACE "|" ";" _fields "${_spec}")
            list(LENGTH _fields _field_count)
            if(NOT _field_count EQUAL 4)
                message(FATAL_ERROR
                    "aros_apply_default_link_set: malformed item '${_spec}'")
            endif()
            list(GET _fields 0 _name)
            list(GET _fields 1 _archive)
            list(GET _fields 2 _absent)
            list(GET _fields 3 _present)
            # Comma-separated inside the record; see the note above.
            string(REPLACE "," ";" _absent "${_absent}")
            string(REPLACE "," ";" _present "${_present}")

            set(_wanted TRUE)
            foreach(_switch IN LISTS _absent)
                if(_switch IN_LIST _selection_switches)
                    set(_wanted FALSE)
                endif()
            endforeach()
            foreach(_switch IN LISTS _present)
                if(NOT _switch IN_LIST _selection_switches)
                    set(_wanted FALSE)
                endif()
            endforeach()
            if(NOT _wanted)
                continue()
            endif()
            if(NOT TARGET "${_archive}")
                if(NOT _archive IN_LIST _missing_archives)
                    list(APPEND _missing_archives "${_archive}")
                endif()
                continue()
            endif()
            # A module never links its own client archive: the archive's
            # autoinit object defines the very base the module implements.
            if(_archive STREQUAL "${_target}-linklib")
                continue()
            endif()
            if(_archive IN_LIST _seen)
                # compiler/autoinit/auto names -lamiga twice and archive order
                # decides resolution. target_link_libraries() would collapse a
                # repeated target, so the repeat goes in as its archive path;
                # the dependency edge is already established by the first.
                list(APPEND _items "$<TARGET_FILE:${_archive}>")
            else()
                list(APPEND _items "${_archive}")
                list(APPEND _seen "${_archive}")
            endif()
        endforeach()

        if(_items)
            target_link_libraries("${_target}" PRIVATE ${_items})
            math(EXPR _applied "${_applied} + 1")
        endif()
    endforeach()

    if(_missing_archives)
        list(SORT _missing_archives)
        string(REPLACE ";" "\n" _missing_text "${_missing_archives}")
        file(WRITE
            "${CMAKE_BINARY_DIR}/generated_targets.default-link-set-missing.txt"
            "${_missing_text}\n")
        list(LENGTH _missing_archives _missing_count)
        message(STATUS
            "⚠️  ${_missing_count} default link set archive(s) are not built in "
            "this configuration -> "
            "${CMAKE_BINARY_DIR}/generated_targets.default-link-set-missing.txt")
    else()
        file(REMOVE
            "${CMAKE_BINARY_DIR}/generated_targets.default-link-set-missing.txt")
    endif()
    message(STATUS "🔗 default link set applied to ${_applied} artefact(s)")
endfunction()
