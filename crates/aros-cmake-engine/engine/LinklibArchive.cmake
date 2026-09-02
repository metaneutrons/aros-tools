# Where a link-library target keeps its archive.
#
# Used by AhiBuild.cmake and ConfigureBuild.cmake, which both link in-tree
# link libraries into a private build, and by cmake/tests/configure-build,
# which includes ConfigureBuild.cmake on its own.
include_guard(GLOBAL)

# The archive a link-library target actually writes.
#
# A link library is called `liblinklibs-<mmake>.a` in the build root only while
# nothing names it: the transpiler promotes it to
# `SYS/Developer/lib/lib<name>.a` as soon as a consumer does. Both spellings
# are live at once -- linklibs-libm is private today while linklibs-amiga and
# linklibs-mui are canonical -- so no filename pattern is right for every link
# library, and the target is the only thing that knows. Two consumers pinned a
# pattern instead and each one broke: OPEN-POINTS 42 (AHI) and 44
# (WirelessManager).
function(aros_linklib_archive_path target out_var)
    if(NOT TARGET "${target}")
        message(FATAL_ERROR
            "aros_linklib_archive_path: no such target ${target}")
    endif()
    get_target_property(_linklib_type "${target}" TYPE)
    if(NOT _linklib_type STREQUAL "STATIC_LIBRARY")
        message(FATAL_ERROR
            "aros_linklib_archive_path: ${target} is a ${_linklib_type}, not an archive")
    endif()
    get_target_property(_linklib_imported "${target}" IMPORTED)
    if(_linklib_imported)
        # An imported archive carries its own location; nothing derives it.
        get_target_property(_linklib_location "${target}" IMPORTED_LOCATION)
        if(NOT _linklib_location OR _linklib_location STREQUAL "_linklib_location-NOTFOUND")
            message(FATAL_ERROR
                "aros_linklib_archive_path: imported ${target} has no IMPORTED_LOCATION")
        endif()
        set(${out_var} "${_linklib_location}" PARENT_SCOPE)
        return()
    endif()
    get_target_property(_linklib_name "${target}" OUTPUT_NAME)
    if(NOT _linklib_name OR _linklib_name STREQUAL "_linklib_name-NOTFOUND")
        set(_linklib_name "${target}")
    endif()
    get_target_property(_linklib_dir "${target}" ARCHIVE_OUTPUT_DIRECTORY)
    if(NOT _linklib_dir OR _linklib_dir STREQUAL "_linklib_dir-NOTFOUND")
        # CMake's own default for a target declared in the top-level list.
        set(_linklib_dir "${CMAKE_CURRENT_BINARY_DIR}")
    endif()
    set(${out_var}
        "${_linklib_dir}/${CMAKE_STATIC_LIBRARY_PREFIX}${_linklib_name}${CMAKE_STATIC_LIBRARY_SUFFIX}"
        PARENT_SCOPE)
endfunction()
