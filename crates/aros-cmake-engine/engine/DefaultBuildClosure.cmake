include_guard(GLOBAL)

# CMake places every ordinary executable and library in its implicit `all`
# target. MetaMake starts at its configured default target (`AROS` in
# mmake.config) and visits only that target's dependency closure. Every
# translated declaration stays nameable, but unrelated manual tests, disabled
# tools and hosted bridges must not change the upstream default build.

function(_aros_mark_metamake_reachable target_name)
    if(NOT TARGET "${target_name}")
        return()
    endif()
    get_property(_reachable GLOBAL PROPERTY AROS_METAMAKE_REACHABLE_TARGETS)
    if("${target_name}" IN_LIST _reachable)
        return()
    endif()
    list(APPEND _reachable "${target_name}")
    set_property(GLOBAL PROPERTY AROS_METAMAKE_REACHABLE_TARGETS "${_reachable}")

    get_property(_dependencies TARGET "${target_name}"
        PROPERTY MANUALLY_ADDED_DEPENDENCIES)
    foreach(_dependency IN LISTS _dependencies)
        _aros_mark_metamake_reachable("${_dependency}")
    endforeach()
endfunction()

# aros_limit_all_to_metamake_root(<root>)
#
# Apply after all #MM edges have been attached. A target outside the closure
# remains directly buildable; EXCLUDE_FROM_ALL changes only implicit
# selection. Link-only libraries may be marked excluded too, which is harmless:
# CMake still builds them whenever a reachable consumer links them.
function(aros_limit_all_to_metamake_root root_target)
    if(NOT TARGET "${root_target}")
        message(FATAL_ERROR
            "MetaMake default root is not a translated target: ${root_target}")
    endif()

    set_property(GLOBAL PROPERTY AROS_METAMAKE_REACHABLE_TARGETS "")
    _aros_mark_metamake_reachable("${root_target}")
    get_property(_reachable GLOBAL PROPERTY AROS_METAMAKE_REACHABLE_TARGETS)
    get_property(_targets DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)

    set(_excluded "")
    foreach(_target IN LISTS _targets)
        get_target_property(_type "${_target}" TYPE)
        if(NOT _type MATCHES
                "^(EXECUTABLE|STATIC_LIBRARY|SHARED_LIBRARY|MODULE_LIBRARY|OBJECT_LIBRARY)$")
            continue()
        endif()
        if("${_target}" IN_LIST _reachable)
            continue()
        endif()
        set_target_properties("${_target}" PROPERTIES EXCLUDE_FROM_ALL TRUE)
        set_property(TARGET "${_target}" PROPERTY AROS_OUTSIDE_DEFAULT_ROOT TRUE)
        list(APPEND _excluded "${_target}")
    endforeach()

    list(REMOVE_DUPLICATES _excluded)
    list(SORT _excluded)
    set_property(GLOBAL PROPERTY AROS_OUTSIDE_DEFAULT_ROOT_TARGETS "${_excluded}")
endfunction()
