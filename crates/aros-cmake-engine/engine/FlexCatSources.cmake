# =============================================================================
# Safe paired hand-written FlexCat source rules
# =============================================================================
#
# A small set of historic MUI preference modules writes a generated locale.c
# and locale.h through a literal FlexCat recipe rather than %build_catalogs.
# The transpiler admits only that bounded rule shape and calls the two helpers
# below in separate phases: declaration registers the generated C product
# before aros_resolve_sources() probes the source tree; binding adds exact
# compile-target dependencies after the targets exist.

function(_aros_flexcat_source_path out_var value label)
    if(NOT value OR value MATCHES ";")
        message(FATAL_ERROR
            "aros_declare_flexcat_sources: ${label} is required and may not contain ';'")
    endif()
    if(IS_ABSOLUTE "${value}")
        set(_path "${value}")
    else()
        set(_path "${CMAKE_SOURCE_DIR}/${value}")
    endif()
    cmake_path(NORMAL_PATH _path)
    cmake_path(ABSOLUTE_PATH CMAKE_SOURCE_DIR NORMALIZE OUTPUT_VARIABLE _source_root)
    cmake_path(IS_PREFIX _source_root "${_path}" NORMALIZE _contained)
    if(NOT _contained OR _path STREQUAL _source_root OR NOT EXISTS "${_path}")
        message(FATAL_ERROR
            "aros_declare_flexcat_sources: ${label} must be an existing source-tree file: ${value}")
    endif()
    set(${out_var} "${_path}" PARENT_SCOPE)
endfunction()

function(_aros_flexcat_leaf_name value label)
    if(NOT value OR value MATCHES "[/\\\\;$]" OR value STREQUAL "." OR value STREQUAL "..")
        message(FATAL_ERROR
            "aros_declare_flexcat_sources: ${label} must be a safe generated filename: ${value}")
    endif()
endfunction()

# aros_declare_flexcat_header(
#   OWNER <mmake-owner> DIRECTORY <source-root-relative-directory>
#   HEADER <generated-h> DESCRIPTION <pot> HEADER_TEMPLATE <sd>)
#
# Historic OpenURL uses this exact one-output recipe shape. The owner is an
# ordinary #MM prerequisite of the compiled program, so aros_add_target_dependency
# consumes AROS_GENERATED_INCLUDE_DIRECTORY below when it attaches that edge.
function(aros_declare_flexcat_header)
    set(oneValueArgs OWNER DIRECTORY HEADER DESCRIPTION HEADER_TEMPLATE)
    cmake_parse_arguments(FCH "" "${oneValueArgs}" "" ${ARGN})
    if(FCH_UNPARSED_ARGUMENTS OR FCH_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_declare_flexcat_header: malformed arguments: "
            "${FCH_UNPARSED_ARGUMENTS}${FCH_KEYWORDS_MISSING_VALUES}")
    endif()
    foreach(_required OWNER DIRECTORY HEADER DESCRIPTION HEADER_TEMPLATE)
        if(NOT FCH_${_required})
            message(FATAL_ERROR
                "aros_declare_flexcat_header: ${_required} is required")
        endif()
    endforeach()
    if(FCH_OWNER MATCHES "[^A-Za-z0-9_.+-]" OR
       FCH_OWNER STREQUAL "." OR FCH_OWNER STREQUAL "..")
        message(FATAL_ERROR
            "aros_declare_flexcat_header: OWNER is not a safe target name: ${FCH_OWNER}")
    endif()
    if(TARGET "${FCH_OWNER}")
        message(FATAL_ERROR
            "aros_declare_flexcat_header: OWNER already exists: ${FCH_OWNER}")
    endif()
    _aros_flexcat_leaf_name("${FCH_HEADER}" HEADER)

    if(IS_ABSOLUTE "${FCH_DIRECTORY}")
        set(_directory "${FCH_DIRECTORY}")
    else()
        set(_directory "${CMAKE_SOURCE_DIR}/${FCH_DIRECTORY}")
    endif()
    cmake_path(NORMAL_PATH _directory)
    cmake_path(ABSOLUTE_PATH CMAKE_SOURCE_DIR NORMALIZE OUTPUT_VARIABLE _source_root)
    cmake_path(IS_PREFIX _source_root "${_directory}" NORMALIZE _contained)
    if(NOT _contained OR _directory STREQUAL _source_root OR
       NOT IS_DIRECTORY "${_directory}")
        message(FATAL_ERROR
            "aros_declare_flexcat_header: DIRECTORY must be an existing source-tree directory: ${FCH_DIRECTORY}")
    endif()
    _aros_flexcat_source_path(_description "${FCH_DESCRIPTION}" DESCRIPTION)
    _aros_flexcat_source_path(_header_template
        "${FCH_HEADER_TEMPLATE}" HEADER_TEMPLATE)

    file(RELATIVE_PATH _declaring_rel "${_source_root}" "${_directory}")
    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR NORMALIZE OUTPUT_VARIABLE _binary_root)
    set(_generated_dir "${_binary_root}/gen/${_declaring_rel}")
    cmake_path(NORMAL_PATH _generated_dir)
    cmake_path(IS_PREFIX _binary_root "${_generated_dir}" NORMALIZE _generated_contained)
    if(NOT _generated_contained OR _generated_dir STREQUAL _binary_root)
        message(FATAL_ERROR
            "aros_declare_flexcat_header: generated directory escapes the build tree")
    endif()
    file(MAKE_DIRECTORY "${_generated_dir}")
    set(_output "${_generated_dir}/${FCH_HEADER}")
    set(_runner "${CMAKE_SOURCE_DIR}/cmake/RunFlexCat.cmake")
    add_custom_command(
        OUTPUT "${_output}"
        COMMAND "${CMAKE_COMMAND}"
            "-DTOOL=${AROS_HOST_FLEXCAT}"
            "-DDESCRIPTION=${_description}"
            "-DSOURCE_DESCRIPTION=${_header_template}"
            "-DOUTPUT=${_output}"
            -P "${_runner}"
        DEPENDS "${AROS_HOST_FLEXCAT}" "${_description}"
            "${_header_template}" "${_runner}"
        COMMENT "Creating FlexCat header ${FCH_HEADER}"
        VERBATIM)
    add_custom_target("${FCH_OWNER}" DEPENDS "${_output}")
    aros_gate_arch("${FCH_OWNER}" "${_directory}")
    set_property(TARGET "${FCH_OWNER}" PROPERTY
        AROS_GENERATED_INCLUDE_DIRECTORY "${_generated_dir}")
endfunction()

# aros_declare_flexcat_sources(
#   OWNER <mmake-owner> DIRECTORY <source-root-relative-directory>
#   SOURCE <generated-c> HEADER <generated-h>
#   DESCRIPTION <pot> HEADER_TEMPLATE <sd> SOURCE_TEMPLATE <sd>
#   [CATALOG_DESTINATION <build-tree-dir> CATALOG_NAME <basename>
#    CATALOG_SOURCE_DIR <source-directory-relative-path>
#    LANGUAGES <po-language>...])
function(aros_declare_flexcat_sources)
    set(oneValueArgs
        OWNER DIRECTORY SOURCE HEADER DESCRIPTION HEADER_TEMPLATE SOURCE_TEMPLATE
        CATALOG_DESTINATION CATALOG_NAME CATALOG_SOURCE_DIR)
    set(multiValueArgs LANGUAGES)
    cmake_parse_arguments(FCS "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(FCS_UNPARSED_ARGUMENTS OR FCS_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_declare_flexcat_sources: malformed arguments: "
            "${FCS_UNPARSED_ARGUMENTS}${FCS_KEYWORDS_MISSING_VALUES}")
    endif()
    foreach(_required OWNER DIRECTORY SOURCE HEADER DESCRIPTION HEADER_TEMPLATE SOURCE_TEMPLATE)
        if(NOT FCS_${_required})
            message(FATAL_ERROR "aros_declare_flexcat_sources: ${_required} is required")
        endif()
    endforeach()
    if(FCS_OWNER MATCHES "[^A-Za-z0-9_.+-]" OR FCS_OWNER STREQUAL "." OR FCS_OWNER STREQUAL "..")
        message(FATAL_ERROR "aros_declare_flexcat_sources: OWNER is not a safe target name: ${FCS_OWNER}")
    endif()
    _aros_flexcat_leaf_name("${FCS_SOURCE}" SOURCE)
    _aros_flexcat_leaf_name("${FCS_HEADER}" HEADER)
    if(FCS_SOURCE STREQUAL FCS_HEADER)
        message(FATAL_ERROR "aros_declare_flexcat_sources: SOURCE and HEADER must differ")
    endif()
    if((FCS_CATALOG_DESTINATION AND (NOT FCS_CATALOG_NAME OR NOT FCS_CATALOG_SOURCE_DIR)) OR
       (FCS_CATALOG_NAME AND (NOT FCS_CATALOG_DESTINATION OR NOT FCS_CATALOG_SOURCE_DIR)) OR
       (FCS_CATALOG_SOURCE_DIR AND (NOT FCS_CATALOG_DESTINATION OR NOT FCS_CATALOG_NAME)))
        message(FATAL_ERROR
            "aros_declare_flexcat_sources: catalog destination, name and source directory must be paired")
    endif()
    if(FCS_LANGUAGES AND NOT FCS_CATALOG_NAME)
        message(FATAL_ERROR "aros_declare_flexcat_sources: LANGUAGES requires catalog outputs")
    endif()

    if(IS_ABSOLUTE "${FCS_DIRECTORY}")
        set(_directory "${FCS_DIRECTORY}")
    else()
        set(_directory "${CMAKE_SOURCE_DIR}/${FCS_DIRECTORY}")
    endif()
    cmake_path(NORMAL_PATH _directory)
    cmake_path(ABSOLUTE_PATH CMAKE_SOURCE_DIR NORMALIZE OUTPUT_VARIABLE _source_root)
    cmake_path(IS_PREFIX _source_root "${_directory}" NORMALIZE _directory_contained)
    if(NOT _directory_contained OR _directory STREQUAL _source_root OR
       NOT IS_DIRECTORY "${_directory}")
        message(FATAL_ERROR
            "aros_declare_flexcat_sources: DIRECTORY must be an existing source-tree directory: ${FCS_DIRECTORY}")
    endif()
    _aros_flexcat_source_path(_description "${FCS_DESCRIPTION}" DESCRIPTION)
    _aros_flexcat_source_path(_header_template "${FCS_HEADER_TEMPLATE}" HEADER_TEMPLATE)
    _aros_flexcat_source_path(_source_template "${FCS_SOURCE_TEMPLATE}" SOURCE_TEMPLATE)

    file(RELATIVE_PATH _declaring_rel "${_source_root}" "${_directory}")
    if(_declaring_rel MATCHES "^\\.\\.")
        message(FATAL_ERROR "aros_declare_flexcat_sources: DIRECTORY escapes the source tree")
    endif()
    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR NORMALIZE OUTPUT_VARIABLE _binary_root)
    set(_generated_dir "${_binary_root}/gen/${_declaring_rel}")
    cmake_path(NORMAL_PATH _generated_dir)
    cmake_path(IS_PREFIX _binary_root "${_generated_dir}" NORMALIZE _generated_contained)
    if(NOT _generated_contained OR _generated_dir STREQUAL _binary_root)
        message(FATAL_ERROR "aros_declare_flexcat_sources: generated directory escapes the build tree")
    endif()
    set(_source_output "${_generated_dir}/${FCS_SOURCE}")
    set(_header_output "${_generated_dir}/${FCS_HEADER}")
    set(_nominal_source "${_directory}/${FCS_SOURCE}")
    cmake_path(NORMAL_PATH _nominal_source)

    string(SHA256 _source_key "${_nominal_source}")
    get_property(_registered GLOBAL PROPERTY "AROS_FLEXCAT_GENERATED_SOURCE_${_source_key}")
    if(_registered)
        message(FATAL_ERROR
            "aros_declare_flexcat_sources: generated source already has an owner: ${_nominal_source}")
    endif()
    if(TARGET "${FCS_OWNER}")
        message(FATAL_ERROR
            "aros_declare_flexcat_sources: OWNER already exists: ${FCS_OWNER}")
    endif()

    set(_runner "${CMAKE_SOURCE_DIR}/cmake/RunFlexCat.cmake")
    add_custom_command(
        OUTPUT "${_header_output}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_generated_dir}"
        COMMAND "${CMAKE_COMMAND}"
            "-DTOOL=${AROS_HOST_FLEXCAT}"
            "-DDESCRIPTION=${_description}"
            "-DSOURCE_DESCRIPTION=${_header_template}"
            "-DOUTPUT=${_header_output}"
            -P "${_runner}"
        DEPENDS "${AROS_HOST_FLEXCAT}" "${_description}" "${_header_template}" "${_runner}"
        COMMENT "Creating FlexCat header ${FCS_HEADER}"
        VERBATIM)
    add_custom_command(
        OUTPUT "${_source_output}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_generated_dir}"
        COMMAND "${CMAKE_COMMAND}"
            "-DTOOL=${AROS_HOST_FLEXCAT}"
            "-DDESCRIPTION=${_description}"
            "-DSOURCE_DESCRIPTION=${_source_template}"
            "-DOUTPUT=${_source_output}"
            -P "${_runner}"
        DEPENDS "${AROS_HOST_FLEXCAT}" "${_description}" "${_source_template}" "${_runner}"
        COMMENT "Creating FlexCat source ${FCS_SOURCE}"
        VERBATIM)

    set(_outputs "${_header_output}" "${_source_output}")
    if(FCS_CATALOG_NAME)
        _aros_flexcat_leaf_name("${FCS_CATALOG_NAME}" CATALOG_NAME)
        if(IS_ABSOLUTE "${FCS_CATALOG_DESTINATION}")
            set(_catalog_destination "${FCS_CATALOG_DESTINATION}")
        else()
            set(_catalog_destination "${_binary_root}/${FCS_CATALOG_DESTINATION}")
        endif()
        cmake_path(NORMAL_PATH _catalog_destination)
        cmake_path(IS_PREFIX _binary_root "${_catalog_destination}" NORMALIZE _catalog_root_contained)
        if(NOT _catalog_root_contained OR _catalog_destination STREQUAL _binary_root)
            message(FATAL_ERROR
                "aros_declare_flexcat_sources: CATALOG_DESTINATION must be below the build tree: ${FCS_CATALOG_DESTINATION}")
        endif()
        foreach(_language IN LISTS FCS_LANGUAGES)
            if(NOT _language MATCHES "^[A-Za-z0-9_-]+$")
                message(FATAL_ERROR "aros_declare_flexcat_sources: invalid PO language: ${_language}")
            endif()
            set(_translation "${_directory}/${FCS_CATALOG_SOURCE_DIR}/${_language}.po")
            cmake_path(NORMAL_PATH _translation)
            cmake_path(IS_PREFIX _directory "${_translation}" NORMALIZE _translation_contained)
            if(NOT _translation_contained)
                message(FATAL_ERROR
                    "aros_declare_flexcat_sources: CATALOG_SOURCE_DIR escapes DIRECTORY")
            endif()
            if(NOT EXISTS "${_translation}")
                message(FATAL_ERROR
                    "aros_declare_flexcat_sources: missing PO input for ${_language}: ${_translation}")
            endif()
            set(_catalog_output
                "${_catalog_destination}/${_language}/${FCS_CATALOG_NAME}.catalog")
            cmake_path(NORMAL_PATH _catalog_output)
            cmake_path(IS_PREFIX _catalog_destination "${_catalog_output}" NORMALIZE _catalog_contained)
            if(NOT _catalog_contained)
                message(FATAL_ERROR "aros_declare_flexcat_sources: catalog output escapes destination")
            endif()
            get_filename_component(_catalog_output_dir "${_catalog_output}" DIRECTORY)
            add_custom_command(
                OUTPUT "${_catalog_output}"
                COMMAND "${CMAKE_COMMAND}" -E make_directory "${_catalog_output_dir}"
                COMMAND "${CMAKE_COMMAND}"
                    "-DTOOL=${AROS_HOST_FLEXCAT}"
                    "-DPOFILE=${_translation}"
                    "-DOUTPUT=${_catalog_output}"
                    -P "${_runner}"
                DEPENDS "${AROS_HOST_FLEXCAT}" "${_translation}" "${_runner}"
                COMMENT "Creating ${FCS_CATALOG_NAME} catalog for ${_language}"
                VERBATIM)
            list(APPEND _outputs "${_catalog_output}")
        endforeach()
    endif()

    add_custom_target("${FCS_OWNER}" DEPENDS ${_outputs})
    aros_gate_arch("${FCS_OWNER}" "${_directory}")
    set_property(TARGET "${FCS_OWNER}" PROPERTY
        AROS_FLEXCAT_GENERATED_DIRECTORY "${_generated_dir}")
    set_property(GLOBAL PROPERTY
        "AROS_FLEXCAT_GENERATED_SOURCE_${_source_key}" "${_source_output}")
    set_property(GLOBAL PROPERTY
        "AROS_FLEXCAT_GENERATED_SOURCE_OWNER_${_source_key}" "${FCS_OWNER}")
endfunction()

# aros_bind_flexcat_source_consumers(OWNER <mmake-owner>
#                                    CONSUMERS <compiled-target>...)
function(aros_bind_flexcat_source_consumers)
    set(oneValueArgs OWNER)
    set(multiValueArgs CONSUMERS)
    cmake_parse_arguments(FCSB "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})
    if(FCSB_UNPARSED_ARGUMENTS OR FCSB_KEYWORDS_MISSING_VALUES OR
       NOT FCSB_OWNER OR NOT FCSB_CONSUMERS)
        message(FATAL_ERROR
            "aros_bind_flexcat_source_consumers: OWNER and CONSUMERS are required")
    endif()
    if(NOT TARGET "${FCSB_OWNER}")
        message(FATAL_ERROR
            "aros_bind_flexcat_source_consumers: owner target is missing: ${FCSB_OWNER}")
    endif()
    get_property(_generated_dir TARGET "${FCSB_OWNER}" PROPERTY
        AROS_FLEXCAT_GENERATED_DIRECTORY)
    if(NOT _generated_dir)
        message(FATAL_ERROR
            "aros_bind_flexcat_source_consumers: owner is not a FlexCat source target: ${FCSB_OWNER}")
    endif()
    list(REMOVE_DUPLICATES FCSB_CONSUMERS)
    foreach(_consumer IN LISTS FCSB_CONSUMERS)
        if(NOT TARGET "${_consumer}")
            message(FATAL_ERROR
                "aros_bind_flexcat_source_consumers: consumer target is missing: ${_consumer}")
        endif()
        get_target_property(_consumer_type "${_consumer}" TYPE)
        if(NOT _consumer_type MATCHES
                "^(EXECUTABLE|STATIC_LIBRARY|SHARED_LIBRARY|MODULE_LIBRARY|OBJECT_LIBRARY)$")
            message(FATAL_ERROR
                "aros_bind_flexcat_source_consumers: consumer is not compilable: ${_consumer}")
        endif()
        add_dependencies("${_consumer}" "${FCSB_OWNER}")
        target_compile_options("${_consumer}" BEFORE PRIVATE "-iquote${_generated_dir}")
    endforeach()
endfunction()
