# Architecture MetaMake effects that create metadata, not compilation aliases.
# The caller binds only selected source-owned declarations and runs these after
# phase-one owner targets have been created.
include_guard(GLOBAL)
include(CMakeParseArguments)

function(_aros_arch_metadata_target_name name owner)
    if(NOT "${name}" MATCHES "^[A-Za-z0-9_.+-]+$" OR
       "${name}" STREQUAL "." OR "${name}" STREQUAL "..")
        message(FATAL_ERROR "${owner}: invalid target name '${name}'")
    endif()
endfunction()

function(aros_empty_arch_linklib)
    cmake_parse_arguments(PARSE_ARGV 0 EL "" "NAME;INCLUDE_TARGET" "")
    if(NOT "${EL_UNPARSED_ARGUMENTS}" STREQUAL "" OR
       NOT "${EL_KEYWORDS_MISSING_VALUES}" STREQUAL "" OR
       "${EL_NAME}" STREQUAL "" OR "${EL_INCLUDE_TARGET}" STREQUAL "")
        message(FATAL_ERROR "Incomplete empty architecture linklib declaration")
    endif()
    _aros_arch_metadata_target_name("${EL_NAME}" "Empty architecture linklib")
    if(TARGET "${EL_NAME}")
        message(FATAL_ERROR "Empty architecture linklib ${EL_NAME}: target already exists")
    endif()
    if(NOT TARGET "${EL_INCLUDE_TARGET}")
        message(FATAL_ERROR
            "Empty architecture linklib ${EL_NAME}: include target ${EL_INCLUDE_TARGET} does not exist")
    endif()
    if("${EL_NAME}" STREQUAL "${EL_INCLUDE_TARGET}")
        message(FATAL_ERROR "Empty architecture linklib ${EL_NAME}: cannot depend on itself")
    endif()

    # A no-command custom target is the faithful Make aggregate: it remains a
    # buildable phony owner, but its only effect is the explicit includes edge.
    add_custom_target("${EL_NAME}")
    add_dependencies("${EL_NAME}" "${EL_INCLUDE_TARGET}")
    set_property(TARGET "${EL_NAME}" PROPERTY
        AROS_EMPTY_ARCH_LINKLIB_INCLUDE_TARGET "${EL_INCLUDE_TARGET}")
endfunction()

function(_aros_arch_metadata_component value field owner)
    if(NOT "${value}" MATCHES "^[A-Za-z0-9_.+-]+$" OR
       "${value}" STREQUAL "." OR "${value}" STREQUAL "..")
        message(FATAL_ERROR "${owner}: unsafe ${field} '${value}'")
    endif()
endfunction()

function(_aros_arch_metadata_main_dir value owner out_var)
    if("${value}" STREQUAL "" OR IS_ABSOLUTE "${value}" OR
       "${value}" MATCHES "[;\r\n$]" OR "${value}" MATCHES "\\$<")
        message(FATAL_ERROR "${owner}: MAINDIR must be a safe relative path")
    endif()
    file(TO_CMAKE_PATH "${value}" _main_dir)
    string(REPLACE "/" ";" _parts "${_main_dir}")
    foreach(_part IN LISTS _parts)
        if("${_part}" STREQUAL "" OR "${_part}" STREQUAL "." OR
           "${_part}" STREQUAL "..")
            message(FATAL_ERROR "${owner}: MAINDIR escapes its generated root")
        endif()
    endforeach()
    set("${out_var}" "${_main_dir}" PARENT_SCOPE)
endfunction()

function(_aros_arch_metadata_roots owner out_source out_gen)
    if(NOT DEFINED AROS_SOURCE_DIR OR "${AROS_SOURCE_DIR}" STREQUAL "" OR
       NOT IS_ABSOLUTE "${AROS_SOURCE_DIR}" OR
       NOT DEFINED AROS_GEN_DIR OR "${AROS_GEN_DIR}" STREQUAL "" OR
       NOT IS_ABSOLUTE "${AROS_GEN_DIR}")
        message(FATAL_ERROR
            "${owner}: AROS_SOURCE_DIR and AROS_GEN_DIR must be configured absolute paths")
    endif()
    if("${AROS_SOURCE_DIR}" MATCHES "[;\r\n]" OR "${AROS_GEN_DIR}" MATCHES "[;\r\n]")
        message(FATAL_ERROR "${owner}: configured source/generated roots contain unsafe characters")
    endif()
    file(REAL_PATH "${AROS_SOURCE_DIR}" _source_root)
    if(NOT IS_DIRECTORY "${_source_root}")
        message(FATAL_ERROR "${owner}: AROS_SOURCE_DIR is not a directory")
    endif()
    set(_raw_gen_root "${AROS_GEN_DIR}")
    cmake_path(ABSOLUTE_PATH _raw_gen_root NORMALIZE OUTPUT_VARIABLE _gen_root)
    set("${out_source}" "${_source_root}" PARENT_SCOPE)
    set("${out_gen}" "${_gen_root}" PARENT_SCOPE)
endfunction()

function(_aros_arch_metadata_include_dir out_var raw_dir owner source_root gen_root)
    if("${raw_dir}" STREQUAL "" OR "${raw_dir}" MATCHES "[;\r\n\t ]" OR
       "${raw_dir}" MATCHES "\\$\\{[^}]*\\}" OR "${raw_dir}" MATCHES "\\$<")
        message(FATAL_ERROR "${owner}: invalid or unresolved INCLUDE_DIRS entry '${raw_dir}'")
    endif()
    file(TO_CMAKE_PATH "${raw_dir}" _portable_dir)
    string(REPLACE "/" ";" _parts "${_portable_dir}")
    foreach(_part IN LISTS _parts)
        if("${_part}" STREQUAL "..")
            message(FATAL_ERROR "${owner}: INCLUDE_DIRS entry escapes its allowed root: ${raw_dir}")
        endif()
    endforeach()

    if(IS_ABSOLUTE "${_portable_dir}")
        set(_candidate "${_portable_dir}")
    else()
        set(_candidate "${source_root}/${_portable_dir}")
    endif()
    cmake_path(ABSOLUTE_PATH _candidate NORMALIZE OUTPUT_VARIABLE _resolved)
    cmake_path(IS_PREFIX source_root "${_resolved}" NORMALIZE _under_source)
    cmake_path(IS_PREFIX gen_root "${_resolved}" NORMALIZE _under_gen)
    if(NOT _under_source AND NOT _under_gen)
        message(FATAL_ERROR
            "${owner}: INCLUDE_DIRS entry is outside AROS_SOURCE_DIR and AROS_GEN_DIR: ${raw_dir}")
    endif()

    # Existing source symlinks must not redirect a declared include outside
    # the source tree. Generated include directories may not exist until build.
    if(_under_source AND IS_DIRECTORY "${_resolved}")
        file(REAL_PATH "${_resolved}" _real_dir)
        cmake_path(IS_PREFIX source_root "${_real_dir}" NORMALIZE _real_under_source)
        if(NOT _real_under_source)
            message(FATAL_ERROR "${owner}: INCLUDE_DIRS symlink escapes AROS_SOURCE_DIR: ${raw_dir}")
        endif()
        set(_resolved "${_real_dir}")
    elseif(_under_gen AND IS_DIRECTORY "${_resolved}" AND IS_DIRECTORY "${gen_root}")
        file(REAL_PATH "${gen_root}" _real_gen_root)
        file(REAL_PATH "${_resolved}" _real_dir)
        cmake_path(IS_PREFIX _real_gen_root "${_real_dir}" NORMALIZE _real_under_gen)
        if(NOT _real_under_gen)
            message(FATAL_ERROR "${owner}: INCLUDE_DIRS symlink escapes AROS_GEN_DIR: ${raw_dir}")
        endif()
        set(_resolved "${_real_dir}")
    endif()
    set("${out_var}" "${_resolved}" PARENT_SCOPE)
endfunction()

function(_aros_arch_metadata_bracket_arg out_var value)
    set(_equals "")
    while("${value}" MATCHES "\\]${_equals}\\]")
        string(APPEND _equals "=")
    endwhile()
    set("${out_var}" "[${_equals}[${value}]${_equals}]" PARENT_SCOPE)
endfunction()

function(aros_set_archincludes_endpoint)
    cmake_parse_arguments(PARSE_ARGV 0 AI "" "NAME;MODNAME;MAINDIR;PRIORITY;TAG" "INCLUDE_DIRS;DEFINITIONS")
    if(NOT "${AI_UNPARSED_ARGUMENTS}" STREQUAL "" OR
       NOT "${AI_KEYWORDS_MISSING_VALUES}" STREQUAL "" OR
       "${AI_NAME}" STREQUAL "" OR "${AI_MODNAME}" STREQUAL "" OR
       "${AI_MAINDIR}" STREQUAL "" OR "${AI_PRIORITY}" STREQUAL "" OR
       "${AI_TAG}" STREQUAL "")
        message(FATAL_ERROR "Incomplete architecture include-flag declaration")
    endif()
    _aros_arch_metadata_target_name("${AI_NAME}" "Architecture include-flag endpoint")
    foreach(_field IN ITEMS MODNAME PRIORITY TAG)
        _aros_arch_metadata_component("${AI_${_field}}" "${_field}" "${AI_NAME}")
    endforeach()
    _aros_arch_metadata_main_dir("${AI_MAINDIR}" "${AI_NAME}" _main_dir)
    if(TARGET "${AI_NAME}")
        message(FATAL_ERROR "Architecture include-flag endpoint ${AI_NAME}: target already exists")
    endif()

    _aros_arch_metadata_roots("${AI_NAME}" _source_root _gen_root)
    set(_flags "")
    foreach(_raw_dir IN LISTS AI_INCLUDE_DIRS)
        _aros_arch_metadata_include_dir(_resolved_dir "${_raw_dir}" "${AI_NAME}" "${_source_root}" "${_gen_root}")
        list(APPEND _flags "-I${_resolved_dir}")
    endforeach()
    foreach(_definition IN LISTS AI_DEFINITIONS)
        if(NOT "${_definition}" MATCHES "^[A-Za-z_][A-Za-z0-9_]*(=[A-Za-z0-9_.+-]*)?$")
            message(FATAL_ERROR "${AI_NAME}: invalid preprocessor definition '${_definition}'")
        endif()
        list(APPEND _flags "-D${_definition}")
    endforeach()
    if(_flags)
        string(JOIN " " _include_flags ${_flags})
    else()
        set(_include_flags "")
    endif()
    # `$(ECHO) "%(includes) "` writes one literal trailing space and newline.
    set(_content "${_include_flags} \n")
    if("${_content}" MATCHES "\\$<")
        message(FATAL_ERROR "${AI_NAME}: generator expressions are not valid include paths")
    endif()

    set(_output "${_gen_root}/${_main_dir}/${AI_MODNAME}/include/.${AI_MODNAME}.includeflag.${AI_PRIORITY}.${AI_TAG}")
    cmake_path(ABSOLUTE_PATH _output NORMALIZE OUTPUT_VARIABLE _normalized_output)
    cmake_path(IS_PREFIX _gen_root "${_normalized_output}" NORMALIZE _output_under_gen)
    if(NOT _output_under_gen)
        message(FATAL_ERROR "${AI_NAME}: metadata output escapes AROS_GEN_DIR")
    endif()
    get_property(_claimed GLOBAL PROPERTY AROS_ARCHINCLUDES_OUTPUT_PATHS)
    if(_normalized_output IN_LIST _claimed)
        message(FATAL_ERROR "${AI_NAME}: architecture include-flag output collision at ${_normalized_output}")
    endif()
    list(APPEND _claimed "${_normalized_output}")
    set_property(GLOBAL PROPERTY AROS_ARCHINCLUDES_OUTPUT_PATHS "${_claimed}")

    get_filename_component(_output_dir "${_normalized_output}" DIRECTORY)
    if(IS_DIRECTORY "${_gen_root}")
        file(REAL_PATH "${_gen_root}" _real_gen_root)
        set(_existing_parent "${_output_dir}")
        while(NOT IS_DIRECTORY "${_existing_parent}")
            get_filename_component(_parent "${_existing_parent}" DIRECTORY)
            if("${_parent}" STREQUAL "${_existing_parent}")
                break()
            endif()
            set(_existing_parent "${_parent}")
        endwhile()
        if(IS_DIRECTORY "${_existing_parent}")
            file(REAL_PATH "${_existing_parent}" _real_existing_parent)
            cmake_path(IS_PREFIX _real_gen_root "${_real_existing_parent}" NORMALIZE _output_parent_under_gen)
            if(NOT _output_parent_under_gen)
                message(FATAL_ERROR "${AI_NAME}: metadata output parent escapes AROS_GEN_DIR")
            endif()
        endif()
    endif()
    set(_script "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/${AI_NAME}-write-archincludes.cmake")
    file(MAKE_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles")
    _aros_arch_metadata_bracket_arg(_output_dir_arg "${_output_dir}")
    _aros_arch_metadata_bracket_arg(_output_arg "${_normalized_output}")
    _aros_arch_metadata_bracket_arg(_content_arg "${_content}")
    set(_script_content
        "file(MAKE_DIRECTORY ${_output_dir_arg})\nfile(WRITE ${_output_arg} ${_content_arg})\n")
    file(GENERATE OUTPUT "${_script}" CONTENT "${_script_content}")
    add_custom_command(
        OUTPUT "${_normalized_output}"
        COMMAND "${CMAKE_COMMAND}" -P "${_script}"
        DEPENDS "${_script}"
        VERBATIM
    )
    add_custom_target("${AI_NAME}" DEPENDS "${_normalized_output}")
    set_property(TARGET "${AI_NAME}" PROPERTY AROS_ARCHINCLUDES_OUTPUT "${_normalized_output}")
endfunction()
