# =============================================================================
# Workbench icon generation
# =============================================================================
#
# The historic %build_icons macro (config/make.tmpl:3117) creates one .info
# file per icon base name.  A declaration's mmake id is a phony aggregate, and
# several declarations may extend the same id.  Keep those two concepts
# separate here: aros_declare_icon_target() owns the outer target, while
# aros_build_icons() adds one uniquely named helper per declaration.

include_guard(GLOBAL)

set(AROS_TARGET_ICONSET "Gorilla" CACHE STRING
    "Icon set installed into the active AROS system tree")

# Initialise reporting state before generated_targets.cmake is included.  A
# reconfigure starts a new CMake process, and the global include guard prevents
# a subdirectory from resetting claims collected earlier in the same process.
set_property(GLOBAL PROPERTY AROS_ICON_TARGETS "")
set_property(GLOBAL PROPERTY AROS_ICON_OUTPUTS "")
set_property(GLOBAL PROPERTY AROS_ICON_OUTPUT_CONFLICTS "")
set_property(GLOBAL PROPERTY AROS_ICON_MISSING_INPUTS "")
set_property(GLOBAL PROPERTY AROS_ICON_MISSING_HOST_TOOL "")

# Turn a source directory accepted by the generated file into an absolute path.
function(_aros_icon_source_dir out_var directory)
    if(IS_ABSOLUTE "${directory}")
        set(_absolute "${directory}")
    else()
        set(_absolute "${CMAKE_SOURCE_DIR}/${directory}")
    endif()
    cmake_path(NORMAL_PATH _absolute)
    set(${out_var} "${_absolute}" PARENT_SCOPE)
endfunction()

# Turn an icon destination into an absolute build-tree path.  Resolved Make
# directory variables already arrive absolute; accepting a relative path keeps
# the public helper useful for hand-written declarations too.
function(_aros_icon_destination_dir out_var directory)
    if(IS_ABSOLUTE "${directory}")
        set(_absolute "${directory}")
    else()
        set(_absolute "${CMAKE_BINARY_DIR}/${directory}")
    endif()
    cmake_path(NORMAL_PATH _absolute)
    set(${out_var} "${_absolute}" PARENT_SCOPE)
endfunction()

# aros_declare_icon_target(MMAKE_ID <id> DIRECTORY <source-directory>)
#
# Declares the outer phony target exactly once.  This is intentionally separate
# from aros_build_icons(): an unresolved or configuration-empty declaration is
# still a real mmake target and must remain in the dependency graph.
function(aros_declare_icon_target)
    set(oneValueArgs MMAKE_ID DIRECTORY)
    cmake_parse_arguments(ARG "" "${oneValueArgs}" "" ${ARGN})

    if(ARG_UNPARSED_ARGUMENTS OR ARG_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_declare_icon_target: malformed arguments: "
            "${ARG_UNPARSED_ARGUMENTS}${ARG_KEYWORDS_MISSING_VALUES}")
    endif()
    if(NOT ARG_MMAKE_ID OR NOT ARG_DIRECTORY)
        message(FATAL_ERROR
            "aros_declare_icon_target: MMAKE_ID and DIRECTORY are required")
    endif()

    _aros_icon_source_dir(_source_dir "${ARG_DIRECTORY}")

    if(NOT TARGET "${ARG_MMAKE_ID}")
        # No ALL: icon families are selected through the declarative mmake
        # graph, and building every installed icon set would make several of
        # them race to write the active SYS tree.
        add_custom_target("${ARG_MMAKE_ID}")
    endif()

    # Use the same architecture policy as compiled targets.  Foreign targets
    # stay nameable but are excluded from the default build and reported.
    aros_gate_arch("${ARG_MMAKE_ID}" "${_source_dir}")

    set_property(GLOBAL APPEND PROPERTY AROS_ICON_TARGETS "${ARG_MMAKE_ID}")
endfunction()

# aros_build_icons(
#     MMAKE_ID <id>
#     DIRECTORY <source-directory>
#     DESTINATION <output-directory>
#     [FORMAT <extension>]
#     [ICONSET <configured-icon-set>]
#     ICONS <base-name>...
#     [IMAGES <shared-image>...])
#
# With no IMAGES, icon X uses X.<FORMAT>.  With IMAGES, every icon in the
# declaration uses that same one- or two-image list.  ilbmtoicon accepts at most
# two images.  Every image is made absolute separately; Make's original rule
# relies on VPATH to repair the second word of a multi-image variable.
function(aros_build_icons)
    set(oneValueArgs MMAKE_ID DIRECTORY DESTINATION FORMAT ICONSET)
    set(multiValueArgs ICONS IMAGES)
    cmake_parse_arguments(ARG "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(ARG_UNPARSED_ARGUMENTS OR ARG_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_build_icons: malformed arguments: "
            "${ARG_UNPARSED_ARGUMENTS}${ARG_KEYWORDS_MISSING_VALUES}")
    endif()
    if(NOT ARG_MMAKE_ID OR NOT ARG_DIRECTORY OR NOT ARG_DESTINATION)
        message(FATAL_ERROR
            "aros_build_icons: MMAKE_ID, DIRECTORY and DESTINATION are required")
    endif()

    if(NOT ARG_FORMAT)
        set(ARG_FORMAT "png")
    endif()

    list(LENGTH ARG_IMAGES _image_count)
    if(_image_count GREATER 2)
        message(FATAL_ERROR
            "aros_build_icons: ${ARG_MMAKE_ID} passes ${_image_count} images; "
            "ilbmtoicon accepts at most two")
    endif()

    _aros_icon_source_dir(_source_dir "${ARG_DIRECTORY}")
    _aros_icon_destination_dir(_destination_dir "${ARG_DESTINATION}")

    # Be robust when called without a preceding declaration.  Do not redeclare
    # an existing target: every resolved condition variant extends the same
    # outer target, and repeating aros_gate_arch() would duplicate the foreign-
    # architecture report (or gate a concrete/meta target a second time).
    if(NOT TARGET "${ARG_MMAKE_ID}")
        aros_declare_icon_target(
            MMAKE_ID "${ARG_MMAKE_ID}"
            DIRECTORY "${_source_dir}")
    endif()

    # Only calls explicitly tagged with an icon set are gated.  Generic program
    # icons and preset copies remain available for every configuration.
    if(ARG_ICONSET AND NOT "${ARG_ICONSET}" STREQUAL "${AROS_TARGET_ICONSET}")
        return()
    endif()

    # Keep the output graph even when the host tool is unavailable.  Such a
    # rule fails with a direct diagnostic when requested, while configure and
    # unrelated build targets remain usable.
    set(_have_tool FALSE)
    if(AROS_HOST_HAVE_ILBMTOICON AND AROS_HOST_ILBMTOICON)
        set(_have_tool TRUE)
    else()
        set_property(GLOBAL APPEND PROPERTY AROS_ICON_MISSING_HOST_TOOL
            "${ARG_MMAKE_ID}: ${_source_dir} -> ${_destination_dir}")
    endif()

    set(_outputs "")
    foreach(_icon IN LISTS ARG_ICONS)
        if(_icon STREQUAL "")
            continue()
        endif()

        # Do not use NAME_WE: valid icon bases include dots, for example
        # pci-mediator.hidd.
        set(_description "${_source_dir}/${_icon}.info.src")
        set(_output "${_destination_dir}/${_icon}.info")
        set(_image_paths "")

        if(ARG_IMAGES)
            foreach(_image IN LISTS ARG_IMAGES)
                if(IS_ABSOLUTE "${_image}")
                    set(_image_path "${_image}")
                else()
                    set(_image_path "${_source_dir}/${_image}")
                endif()
                cmake_path(NORMAL_PATH _image_path)
                list(APPEND _image_paths "${_image_path}")
            endforeach()
        else()
            set(_image_path "${_source_dir}/${_icon}.${ARG_FORMAT}")
            cmake_path(NORMAL_PATH _image_path)
            list(APPEND _image_paths "${_image_path}")
        endif()

        foreach(_input IN ITEMS "${_description}" ${_image_paths})
            if(NOT EXISTS "${_input}")
                set_property(GLOBAL APPEND PROPERTY AROS_ICON_MISSING_INPUTS
                    "${ARG_MMAKE_ID}: ${_input}")
            endif()
        endforeach()

        # CMake/Ninja permit only one producer for an OUTPUT.  Record the first
        # declaration and let later targets depend on its rule.  This mirrors
        # Make's first-satisfiable pattern rule and handles the two WBRename
        # declarations without a duplicate-output generation error.
        string(SHA256 _output_hash "${_output}")
        set(_claim_property "AROS_ICON_OUTPUT_CLAIM_${_output_hash}")
        set(_claim_info_property "AROS_ICON_OUTPUT_INFO_${_output_hash}")

        string(JOIN "|" _input_signature "${_description}" ${_image_paths})
        string(SHA256 _claim_signature "${_input_signature}")
        get_property(_already_claimed GLOBAL PROPERTY "${_claim_property}" SET)

        if(_already_claimed)
            get_property(_first_signature GLOBAL PROPERTY "${_claim_property}")
            if(NOT "${_first_signature}" STREQUAL "${_claim_signature}")
                get_property(_first_info GLOBAL PROPERTY "${_claim_info_property}")
                string(JOIN ", " _current_images ${_image_paths})
                set(_conflict
                    "${_output}: keeping ${_first_info}, ignoring ${ARG_MMAKE_ID} [description=${_description}, images=${_current_images}]")
                set_property(GLOBAL APPEND PROPERTY AROS_ICON_OUTPUT_CONFLICTS
                    "${_conflict}")
            endif()
        else()
            string(JOIN ", " _image_info ${_image_paths})
            set_property(GLOBAL PROPERTY "${_claim_property}" "${_claim_signature}")
            set_property(GLOBAL PROPERTY "${_claim_info_property}"
                "${ARG_MMAKE_ID} [description=${_description}, images=${_image_info}]")

            set(_depends "${_description}" ${_image_paths})
            if(_have_tool)
                list(PREPEND _depends "${AROS_HOST_ILBMTOICON}")
                add_custom_command(
                    OUTPUT "${_output}"
                    COMMAND "${CMAKE_COMMAND}" -E make_directory
                            "${_destination_dir}"
                    COMMAND "${AROS_HOST_ILBMTOICON}"
                            "${_description}" ${_image_paths} "${_output}"
                    DEPENDS ${_depends}
                    COMMENT "Creating icon ${_output}"
                    VERBATIM
                    COMMAND_EXPAND_LISTS)
            else()
                add_custom_command(
                    OUTPUT "${_output}"
                    COMMAND "${CMAKE_COMMAND}" -E echo
                            "Cannot create ${_output}: host ilbmtoicon is unavailable (libpng/zlib missing)"
                    COMMAND "${CMAKE_COMMAND}" -E false
                    DEPENDS ${_depends}
                    COMMENT "Icon generation unavailable for ${_output}"
                    VERBATIM
                    COMMAND_EXPAND_LISTS)
            endif()

            set_property(GLOBAL APPEND PROPERTY AROS_ICON_OUTPUTS "${_output}")
        endif()

        # A later declaration that reuses this output still depends on the
        # first producer through the common output file.
        list(APPEND _outputs "${_output}")
    endforeach()

    if(NOT _outputs)
        return()
    endif()
    list(REMOVE_DUPLICATES _outputs)

    # The helper is stable across reconfigure and unique per declaration.  It
    # lets a second declaration extend an existing outer target, which cannot
    # itself be given additional file dependencies after creation.
    string(JOIN "|" _declaration_signature
        "${ARG_MMAKE_ID}" "${_source_dir}" "${_destination_dir}"
        "${ARG_FORMAT}" "${ARG_ICONSET}" "${ARG_ICONS}" "${ARG_IMAGES}")
    string(SHA256 _helper_hash "${_declaration_signature}")
    set(_helper_target "aros-icon-set-${_helper_hash}")

    if(NOT TARGET "${_helper_target}")
        add_custom_target("${_helper_target}" DEPENDS ${_outputs})
    endif()
    add_dependencies("${ARG_MMAKE_ID}" "${_helper_target}")
endfunction()
