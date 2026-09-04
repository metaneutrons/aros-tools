include_guard(GLOBAL)

# CMake learned the IS_EXECUTABLE path predicate in 3.29, while AROS-NX's
# supported configure baseline is 3.22.  Keep every host-tool boundary on one
# compatibility contract: all supported CMake versions reject missing paths
# and directories; 3.29+ additionally rejects a file without execute
# permission.  On older CMake releases the first real invocation remains the
# authoritative executability check and already reports its process failure.
function(aros_path_is_executable path output)
    if(NOT EXISTS "${path}" OR IS_DIRECTORY "${path}")
        set(_aros_executable FALSE)
    elseif(CMAKE_VERSION VERSION_GREATER_EQUAL "3.29")
        if(IS_EXECUTABLE "${path}")
            set(_aros_executable TRUE)
        else()
            set(_aros_executable FALSE)
        endif()
    else()
        set(_aros_executable TRUE)
    endif()
    set(${output} "${_aros_executable}" PARENT_SCOPE)
endfunction()
