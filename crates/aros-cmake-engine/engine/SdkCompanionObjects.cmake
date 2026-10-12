include_guard(GLOBAL)
include("${CMAKE_CURRENT_LIST_DIR}/LiteralObjects.cmake")

# A companion uses the same authenticated driver but its own explicit ABI,
# never the primary CMake target's inherited command-line flags.
function(aros_compile_sdk_companion_object)
    cmake_parse_arguments(PARSE_ARGV 0 O "" "NAME;SOURCE;INTERMEDIATE;OUTPUT;LANGUAGE;COMPANION_CPU"
        "DEFINES;UNDEFINES;OPTIONS;INCLUDES")
    if(O_UNPARSED_ARGUMENTS OR O_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "SDK companion object: invalid arguments")
    endif()
    set(arguments ${O_OPTIONS})
    foreach(value IN LISTS O_DEFINES)
        list(APPEND arguments "-D${value}")
    endforeach()
    foreach(value IN LISTS O_UNDEFINES)
        list(APPEND arguments "-U${value}")
    endforeach()
    foreach(path IN LISTS O_INCLUDES)
        list(APPEND arguments "-I${path}")
    endforeach()
    _aros_sdk_companion_role(role_options library elf_class elf_machine
        "${O_COMPANION_CPU}" "${O_LANGUAGE}" "${arguments}")
    set(library_root "${AROS_TARGET_SYSROOT}/${library}")
    cmake_path(ABSOLUTE_PATH library_root NORMALIZE OUTPUT_VARIABLE library_root)
    cmake_path(ABSOLUTE_PATH O_OUTPUT NORMALIZE OUTPUT_VARIABLE output)
    cmake_path(GET output PARENT_PATH parent)
    cmake_path(GET output FILENAME basename)
    if(NOT parent STREQUAL library_root OR NOT basename MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*[.]o$")
        message(FATAL_ERROR "SDK companion object: output differs from the declared library root")
    endif()
    string(SHA256 identity "${output}")
    set(compiled "${AROS_BUILD_DIR}/gen/sdk-companion/${identity}/${basename}")
    foreach(path "${O_INTERMEDIATE}" "${output}")
        _aros_literal_check_path_chain("${path}" "${AROS_BUILD_DIR}" "SDK companion output")
        _aros_literal_check_optional_regular("${path}" "SDK companion output")
        string(TOLOWER "${path}" folded)
        string(SHA256 key "${folded}")
        get_property(prior GLOBAL PROPERTY "AROS_SDK_OBJECT_${key}")
        if(prior)
            message(FATAL_ERROR "SDK companion object: duplicate ownership")
        endif()
        set_property(GLOBAL PROPERTY "AROS_SDK_OBJECT_${key}" "${O_NAME}")
    endforeach()
    cmake_path(GET O_SOURCE PARENT_PATH source_directory)
    list(APPEND arguments -iquote "${source_directory}")
    aros_compile_literal_object(NAME "${O_NAME}-compile" SOURCE "${O_SOURCE}"
        OUTPUT "${compiled}" LANGUAGE "${O_LANGUAGE}" COMPANION_CPU "${O_COMPANION_CPU}"
        ARGUMENTS ${arguments})
    set(check "${O_NAME}-check")
    set(staging_arguments
        "-DBINARY_ROOT=${AROS_BUILD_DIR}" "-DSOURCE_ROOT=${AROS_SOURCE_DIR}"
        "-DSOURCE_FILE=${O_SOURCE}" "-DSOURCE_OBJECT=${compiled}"
        "-DINTERMEDIATE=${O_INTERMEDIATE}" "-DOUTPUT=${output}" "-DLIB_ROOT=${library_root}"
        "-DEXPECTED_ELF_CLASS=${elf_class}" "-DEXPECTED_ELF_MACHINE=${elf_machine}")
    add_custom_target("${check}"
        COMMAND "${CMAKE_COMMAND}" ${staging_arguments} -DVERIFY_ONLY=ON
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunSdkObject.cmake" VERBATIM)
    add_dependencies("${O_NAME}-compile" "${check}")
    add_custom_command(OUTPUT "${O_INTERMEDIATE}" "${output}"
        COMMAND "${CMAKE_COMMAND}" ${staging_arguments}
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunSdkObject.cmake"
        DEPENDS "${compiled}" "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunSdkObject.cmake"
        VERBATIM)
    add_custom_target("${O_NAME}" DEPENDS "${O_INTERMEDIATE}" "${output}")
    add_dependencies("${O_NAME}" "${O_NAME}-compile" "${check}")
endfunction()
