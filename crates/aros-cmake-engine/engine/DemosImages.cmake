include_guard(GLOBAL)

function(aros_add_demos_images)
    if(NOT TARGET demos-demowin)
        return()
    endif()
    if(TARGET demos-images-setup)
        message(FATAL_ERROR "demos-images-setup already has a producer")
    endif()
    get_target_property(_type demos-demowin TYPE)
    if(NOT _type STREQUAL "EXECUTABLE")
        message(FATAL_ERROR "demos-demowin is no longer an executable")
    endif()

    set(_source_dir "${AROS_SOURCE_DIR}/developer/demos")
    set(_images_dir "${_source_dir}/images")
    set(_makefile "${_images_dir}/mmakefile")
    set(_generator "${_images_dir}/datfilt.awk")
    set(_consumer "${_source_dir}/demowin.c")
    foreach(_source IN ITEMS "${_makefile}" "${_generator}" "${_consumer}")
        if(NOT EXISTS "${_source}" OR IS_DIRECTORY "${_source}" OR
           IS_SYMLINK "${_source}")
            message(FATAL_ERROR "demos image producer requires a regular source: ${_source}")
        endif()
    endforeach()
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${_makefile}")
    file(STRINGS "${_makefile}" _images_rule REGEX "^IMAGES[ \t]*:=")
    string(REGEX REPLACE "[ \t]+" " " _images_rule "${_images_rule}")
    if(NOT _images_rule STREQUAL
       "IMAGES := ArrowUp ArrowDown ArrowLeft ArrowRight ImageButton")
        message(FATAL_ERROR "demos image inventory differs from the reviewed Make rule")
    endif()
    file(STRINGS "${_makefile}" _setup_rule REGEX "^demos-images-setup[ \t]*:")
    string(REGEX REPLACE "[ \t]+" " " _setup_rule "${_setup_rule}")
    if(NOT _setup_rule STREQUAL "demos-images-setup : $(IMAGEFILES)")
        message(FATAL_ERROR "demos image setup differs from the reviewed Make rule")
    endif()

    find_program(AROS_DEMOS_AWK NAMES gawk awk)
    if(NOT AROS_DEMOS_AWK)
        message(FATAL_ERROR "demos images require a host awk executable")
    endif()
    set(_output_dir "${CMAKE_BINARY_DIR}/developer/demos/images")
    set(_outputs "")
    file(READ "${_consumer}" _consumer_source)
    foreach(_stem IN ITEMS ArrowUp ArrowDown ArrowLeft ArrowRight ImageButton)
        foreach(_variant IN ITEMS 0 1)
            set(_name "${_stem}${_variant}")
            set(_input "${_images_dir}/${_name}.dat")
            set(_output "${_output_dir}/${_name}.h")
            if(NOT EXISTS "${_input}" OR IS_DIRECTORY "${_input}" OR
               IS_SYMLINK "${_input}")
                message(FATAL_ERROR "demos image source is missing or unsafe: ${_input}")
            endif()
            string(FIND "${_consumer_source}" "#include \"images/${_name}.h\"" _include)
            if(_include LESS 0)
                message(FATAL_ERROR "demos image consumer changed: ${_name}.h")
            endif()
            add_custom_command(
                OUTPUT "${_output}"
                COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
                COMMAND "${AROS_DEMOS_AWK}" -f "${_generator}" "${_input}"
                WORKING_DIRECTORY "${_output_dir}"
                DEPENDS "${_input}" "${_generator}" "${_makefile}"
                COMMENT "Generating demos image ${_name}.h"
                VERBATIM)
            list(APPEND _outputs "${_output}")
        endforeach()
    endforeach()
    add_custom_target(demos-images-setup DEPENDS ${_outputs})
    add_dependencies(demos-demowin demos-images-setup)
    target_include_directories(demos-demowin PRIVATE
        "${CMAKE_BINARY_DIR}/developer/demos")
endfunction()
