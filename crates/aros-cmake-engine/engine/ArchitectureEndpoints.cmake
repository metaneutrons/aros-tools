# Concrete architecture object effects, not aliases to their module owner.
# The caller must supply source-bound, selected declarations. Unsupported or
# shadowed objects remain errors; no empty compilation target is synthesized.
include_guard(GLOBAL)
include(CMakeParseArguments)

# Architecture source lists may contain source-owned subdirectories such as
# math/e_fmod. Keep the relative stem exact; do not flatten it or admit parent
# traversal when binding the selected compiler source.
function(_aros_arch_source_stem directory name output_directory output_name)
    if(NOT name MATCHES "^[a-zA-Z0-9_./+-]+$" OR IS_ABSOLUTE "${name}" OR
       name MATCHES "(^|/)\\.\\.?(/|$)" OR name MATCHES "//" OR name MATCHES "/$")
        message(FATAL_ERROR "Unsafe architecture source basename ${name}")
    endif()
    get_filename_component(_relative_directory "${name}" DIRECTORY)
    get_filename_component(_basename "${name}" NAME)
    if(_relative_directory STREQUAL "")
        set(_selected_directory "${directory}")
    else()
        set(_selected_directory "${directory}/${_relative_directory}")
    endif()
    set(${output_directory} "${_selected_directory}" PARENT_SCOPE)
    set(${output_name} "${_basename}" PARENT_SCOPE)
endfunction()

# Dependency and default-link passes may add compilation usage requirements
# after the graph has created these endpoints. Snapshot the final owner state
# at directory end, not only at the point where its sources are split.
function(aros_finalize_arch_object_endpoints)
    get_property(_endpoints DIRECTORY PROPERTY AROS_ARCH_OBJECT_ENDPOINTS)
    foreach(_endpoint IN LISTS _endpoints)
        get_target_property(_owner "${_endpoint}" AROS_ARCH_COMPILATION_OWNER)
        _aros_copy_arch_owner_state("${_endpoint}" "${_owner}")
        get_target_property(_client "${_endpoint}" AROS_ARCH_CLIENT_OBJECTS)
        if(_client)
            get_target_property(_dependencies "${_owner}" MANUALLY_ADDED_DEPENDENCIES)
            get_target_property(_leaves "${_owner}" AROS_ARCH_CLIENT_LEAVES)
            foreach(_dependency IN LISTS _dependencies)
                if(NOT _dependency STREQUAL "_dependencies-NOTFOUND" AND
                   NOT _dependency IN_LIST _leaves AND NOT _dependency STREQUAL _endpoint)
                    add_dependencies("${_endpoint}" "${_dependency}")
                endif()
            endforeach()
        endif()
    endforeach()
endfunction()

# A static archive cannot own the compile edges used by its own prerequisite:
# CMake propagates archive prerequisites to every directly compiled object.
# Move the complete ordered source set to one object owner and leave the
# archive as an ordered, archive-only consumer. Final compile state and all
# prerequisites except the source-proved object leaves are copied at directory
# end. This preserves generic/architecture/generated member order.
function(_aros_prepare_arch_client_objects archive output)
    get_target_property(_objects "${archive}" AROS_ARCH_CLIENT_OBJECT_OWNER)
    if(_objects AND NOT _objects STREQUAL "_objects-NOTFOUND")
        set(${output} "${_objects}" PARENT_SCOPE)
        return()
    endif()
    get_target_property(_sources "${archive}" SOURCES)
    if(NOT _sources OR _sources STREQUAL "_sources-NOTFOUND")
        message(FATAL_ERROR "Architecture client-linklib: archive has no compiler sources")
    endif()
    string(SHA256 _key "${archive}")
    set(_objects "aros-arch-client-objects-${_key}")
    add_library("${_objects}" OBJECT EXCLUDE_FROM_ALL ${_sources})
    _aros_copy_arch_owner_state("${_objects}" "${archive}")
    set_property(TARGET "${_objects}" PROPERTY AROS_ARCH_COMPILATION_OWNER "${archive}")
    set_property(TARGET "${_objects}" PROPERTY AROS_ARCH_CLIENT_OBJECTS TRUE)
    set_property(TARGET "${archive}" PROPERTY AROS_ARCH_CLIENT_OBJECT_OWNER "${_objects}")
    set_property(TARGET "${archive}" PROPERTY AROS_ARCH_CLIENT_ORIGINAL_SOURCES "${_sources}")
    set_property(TARGET "${archive}" PROPERTY SOURCES "$<TARGET_OBJECTS:${_objects}>")
    set_property(DIRECTORY APPEND PROPERTY AROS_ARCH_OBJECT_ENDPOINTS "${_objects}")
    get_property(_scheduled DIRECTORY PROPERTY AROS_ARCH_OBJECT_FINALIZER_SCHEDULED)
    if(NOT _scheduled)
        set_property(DIRECTORY PROPERTY AROS_ARCH_OBJECT_FINALIZER_SCHEDULED TRUE)
        cmake_language(DEFER CALL aros_finalize_arch_object_endpoints)
    endif()
    set(${output} "${_objects}" PARENT_SCOPE)
endfunction()

function(_aros_copy_arch_owner_state endpoint owner)
    foreach(_property IN ITEMS INCLUDE_DIRECTORIES SYSTEM_INCLUDE_DIRECTORIES
            COMPILE_DEFINITIONS COMPILE_OPTIONS COMPILE_FLAGS COMPILE_FEATURES
            C_STANDARD C_STANDARD_REQUIRED C_EXTENSIONS
            CXX_STANDARD CXX_STANDARD_REQUIRED CXX_EXTENSIONS
            OBJC_STANDARD OBJC_STANDARD_REQUIRED OBJC_EXTENSIONS
            OBJCXX_STANDARD OBJCXX_STANDARD_REQUIRED OBJCXX_EXTENSIONS
            INTERPROCEDURAL_OPTIMIZATION PRECOMPILE_HEADERS
            PRECOMPILE_HEADERS_REUSE_FROM DISABLE_PRECOMPILE_HEADERS
            POSITION_INDEPENDENT_CODE AROS_NO_POSIXC_HEADERS
            AROS_QUOTE_FALLBACK_OPTIONS AROS_DEFER_QUOTE_OPTIONS LINK_LIBRARIES)
        get_target_property(_value "${owner}" "${_property}")
        if(NOT "${_value}" STREQUAL "_value-NOTFOUND")
            set_property(TARGET "${endpoint}" PROPERTY "${_property}" "${_value}")
        endif()
    endforeach()
endfunction()

# Resolve only sources already selected on the concrete owner. A missing or
# shadowed declaration cannot turn into an empty successful object endpoint.
function(aros_bind_arch_source_endpoint)
    cmake_parse_arguments(PARSE_ARGV 0 AS "" "ENDPOINT;OWNER;INCLUDE_OWNER;DIRECTORY" "BASENAMES")
    if(AS_UNPARSED_ARGUMENTS OR AS_KEYWORDS_MISSING_VALUES OR
       "${AS_DIRECTORY}" STREQUAL "" OR "${AS_BASENAMES}" STREQUAL "" OR NOT IS_ABSOLUTE "${AS_DIRECTORY}" OR
       NOT IS_DIRECTORY "${AS_DIRECTORY}" OR NOT TARGET "${AS_OWNER}")
        message(FATAL_ERROR "Incomplete architecture source endpoint binding")
    endif()
    get_target_property(_owner_sources "${AS_OWNER}" SOURCES)
    set(_selected "")
    foreach(_name IN LISTS AS_BASENAMES)
        _aros_arch_source_stem("${AS_DIRECTORY}" "${_name}" _expected_directory _expected_basename)
        set(_matches "")
        foreach(_source IN LISTS _owner_sources)
            if(_source MATCHES "\\$<")
                continue()
            endif()
            get_filename_component(_directory "${_source}" DIRECTORY)
            get_filename_component(_basename "${_source}" NAME_WE)
            if(_directory STREQUAL _expected_directory AND _basename STREQUAL _expected_basename)
                list(APPEND _matches "${_source}")
            endif()
        endforeach()
        list(LENGTH _matches _count)
        if(NOT _count EQUAL 1)
            message(FATAL_ERROR "Architecture endpoint ${AS_ENDPOINT}: ${_name} has no unique selected source")
        endif()
        list(APPEND _selected ${_matches})
    endforeach()
    aros_bind_arch_object_endpoint(ENDPOINT "${AS_ENDPOINT}" OWNER "${AS_OWNER}"
        INCLUDE_OWNER "${AS_INCLUDE_OWNER}" SOURCES ${_selected})
endfunction()

# Bind a source-proved `%build_archspecific -linklib` leaf to only the selected
# architecture objects in the concrete client archives. Depending on a whole
# static archive here would cycle when that archive itself depends on this
# MetaMake leaf. The library builder applies basename replacement; this
# endpoint independently proves the source selection before exposing object
# dependencies to the build graph.
function(aros_bind_arch_linklib_source_endpoint)
    cmake_parse_arguments(PARSE_ARGV 0 AL
        "" "ENDPOINT;INCLUDE_OWNER;DIRECTORY"
        "ARCHIVE_TARGETS;BASENAMES")
    if(AL_UNPARSED_ARGUMENTS OR AL_KEYWORDS_MISSING_VALUES OR
       "${AL_ENDPOINT}" STREQUAL "" OR
       "${AL_INCLUDE_OWNER}" STREQUAL "" OR
       "${AL_DIRECTORY}" STREQUAL "" OR
       "${AL_ARCHIVE_TARGETS}" STREQUAL "" OR
       "${AL_BASENAMES}" STREQUAL "" OR
       NOT IS_ABSOLUTE "${AL_DIRECTORY}" OR
       NOT IS_DIRECTORY "${AL_DIRECTORY}" OR
       TARGET "${AL_ENDPOINT}" OR NOT TARGET "${AL_INCLUDE_OWNER}")
        message(FATAL_ERROR "Incomplete architecture client-linklib endpoint binding")
    endif()

    set(_bound_archives "")
    set(_arch_object_dependencies "")
    set(_object_contract "")
    set(_object_count 0)
    foreach(_archive IN LISTS AL_ARCHIVE_TARGETS)
        if(NOT TARGET "${_archive}")
            continue()
        endif()
        get_target_property(_archive_type "${_archive}" TYPE)
        if(NOT _archive_type STREQUAL "STATIC_LIBRARY")
            message(FATAL_ERROR
                "Architecture client-linklib endpoint ${AL_ENDPOINT}: ${_archive} is not an archive")
        endif()
        get_target_property(_archive_sources "${_archive}" AROS_ARCH_CLIENT_ORIGINAL_SOURCES)
        if(_archive_sources STREQUAL "_archive_sources-NOTFOUND")
            get_target_property(_archive_sources "${_archive}" SOURCES)
        endif()
        get_target_property(_generic_sources "${_archive}" AROS_GENERIC_LINKLIB_SOURCES)
        if(_generic_sources STREQUAL "_generic_sources-NOTFOUND")
            set(_generic_sources "")
        endif()

        foreach(_name IN LISTS AL_BASENAMES)
            _aros_arch_source_stem("${AL_DIRECTORY}" "${_name}" _expected_directory _expected_basename)
            set(_matches "")
            foreach(_source IN LISTS _archive_sources)
                if(_source MATCHES "\\$<")
                    continue()
                endif()
                get_filename_component(_directory "${_source}" DIRECTORY)
                get_filename_component(_basename "${_source}" NAME_WE)
                if(_directory STREQUAL _expected_directory AND _basename STREQUAL _expected_basename)
                    list(APPEND _matches "${_source}")
                endif()
            endforeach()
            list(LENGTH _matches _count)
            if(NOT _count EQUAL 1)
                message(FATAL_ERROR
                    "Architecture client-linklib endpoint ${AL_ENDPOINT}: ${_name} has no unique selected source in ${_archive}")
            endif()
            list(GET _matches 0 _selected_source)
            if(NOT IS_ABSOLUTE "${_selected_source}" OR
               NOT EXISTS "${_selected_source}" OR
               IS_DIRECTORY "${_selected_source}")
                message(FATAL_ERROR
                    "Architecture client-linklib endpoint ${AL_ENDPOINT}: unbound source ${_selected_source}")
            endif()
            get_source_file_property(_external "${_selected_source}" EXTERNAL_OBJECT)
            if(_external)
                message(FATAL_ERROR
                    "Architecture client-linklib endpoint ${AL_ENDPOINT}: external object is not a source effect")
            endif()

            if(NOT CMAKE_C_OUTPUT_EXTENSION)
                message(FATAL_ERROR
                    "Architecture client-linklib endpoint ${AL_ENDPOINT}: C object extension is unavailable")
            endif()
            get_filename_component(_source_filename "${_selected_source}" NAME)
            string(REPLACE "." "[.]" _source_pattern "${_source_filename}")
            string(REPLACE "+" "[+]" _source_pattern "${_source_pattern}")
            string(REPLACE "." "[.]" _object_extension_pattern
                "${CMAKE_C_OUTPUT_EXTENSION}")
            # CMake appends CMAKE_C_OUTPUT_EXTENSION to the full source
            # filename in a target's object path (for example mathinit.c.o).
            # The generated build-time contract rejects zero or multiple
            # compiler objects, including on incremental endpoint builds.
            set(_object_pattern
                ".*/${_source_pattern}${_object_extension_pattern}$")
            _aros_prepare_arch_client_objects("${_archive}" _object_owner)
            add_dependencies("${_object_owner}" "${AL_INCLUDE_OWNER}")
            set_property(TARGET "${_archive}" APPEND PROPERTY AROS_ARCH_CLIENT_LEAVES "${AL_ENDPOINT}")
            list(APPEND _arch_object_dependencies
                "$<FILTER:$<TARGET_OBJECTS:${_object_owner}>,INCLUDE,${_object_pattern}>")
            string(APPEND _object_contract
                "set(OBJECT_SET_${_object_count} [==[$<FILTER:$<TARGET_OBJECTS:${_object_owner}>,INCLUDE,${_object_pattern}>]==])\n")
            math(EXPR _object_count "${_object_count} + 1")

            foreach(_generic IN LISTS _generic_sources)
                get_filename_component(_generic_basename "${_generic}" NAME_WE)
                if(_generic_basename STREQUAL _name AND
                   _generic IN_LIST _archive_sources)
                    message(FATAL_ERROR
                        "Architecture client-linklib endpoint ${AL_ENDPOINT}: generic ${_name} was not replaced in ${_archive}")
                endif()
            endforeach()
        endforeach()
        list(APPEND _bound_archives "${_archive}")
    endforeach()

    if(NOT _bound_archives)
        message(FATAL_ERROR
            "Architecture client-linklib endpoint ${AL_ENDPOINT}: no selected normal or relative client archive exists")
    endif()
    list(REMOVE_DUPLICATES _bound_archives)
    if(NOT _arch_object_dependencies)
        message(FATAL_ERROR
            "Architecture client-linklib endpoint ${AL_ENDPOINT}: no selected archive objects")
    endif()
    string(SHA256 _contract_key "${AL_ENDPOINT}")
    set(_contract_path "${CMAKE_CURRENT_BINARY_DIR}/CMakeFiles/aros-arch-linklib-${_contract_key}.cmake")
    string(APPEND _object_contract "set(EXPECTED_OBJECT_SETS ${_object_count})\n")
    file(GENERATE OUTPUT "${_contract_path}" CONTENT "${_object_contract}")
    add_custom_target("${AL_ENDPOINT}"
        COMMAND "${CMAKE_COMMAND}" "-DCONTRACT=${_contract_path}"
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/VerifyArchLinklibObjects.cmake"
        DEPENDS ${_arch_object_dependencies} VERBATIM)
    add_dependencies("${AL_ENDPOINT}" "${AL_INCLUDE_OWNER}")
endfunction()

function(aros_bind_arch_object_endpoint)
    cmake_parse_arguments(PARSE_ARGV 0 AE "" "ENDPOINT;OWNER;INCLUDE_OWNER" "SOURCES")
    if(AE_UNPARSED_ARGUMENTS OR AE_KEYWORDS_MISSING_VALUES OR
       "${AE_ENDPOINT}" STREQUAL "" OR "${AE_OWNER}" STREQUAL "" OR
       "${AE_INCLUDE_OWNER}" STREQUAL "" OR "${AE_SOURCES}" STREQUAL "")
        message(FATAL_ERROR "Incomplete architecture object binding")
    endif()
    if(TARGET "${AE_ENDPOINT}" OR NOT TARGET "${AE_OWNER}" OR
       NOT TARGET "${AE_INCLUDE_OWNER}" OR AE_ENDPOINT STREQUAL AE_OWNER)
        message(FATAL_ERROR "Architecture endpoint ${AE_ENDPOINT}: conflicting or missing owner")
    endif()
    get_target_property(_type "${AE_OWNER}" TYPE)
    if(NOT _type MATCHES "^(EXECUTABLE|STATIC_LIBRARY|OBJECT_LIBRARY|SHARED_LIBRARY|MODULE_LIBRARY)$")
        message(FATAL_ERROR "Architecture endpoint ${AE_ENDPOINT}: owner is not a compilation target")
    endif()
    get_target_property(_owner_sources "${AE_OWNER}" SOURCES)
    set(_unique "")
    foreach(_source IN LISTS AE_SOURCES)
        if(_source MATCHES "\\$<" OR NOT IS_ABSOLUTE "${_source}" OR
           NOT EXISTS "${_source}" OR IS_DIRECTORY "${_source}" OR
           NOT _source IN_LIST _owner_sources OR _source IN_LIST _unique)
            message(FATAL_ERROR "Architecture endpoint ${AE_ENDPOINT}: unbound or duplicate source ${_source}")
        endif()
        get_source_file_property(_external "${_source}" EXTERNAL_OBJECT)
        if(_external)
            message(FATAL_ERROR "Architecture endpoint ${AE_ENDPOINT}: external object is not a source effect")
        endif()
        list(APPEND _unique "${_source}")
    endforeach()

    add_library("${AE_ENDPOINT}" OBJECT EXCLUDE_FROM_ALL ${_unique})
    _aros_copy_arch_owner_state("${AE_ENDPOINT}" "${AE_OWNER}")
    add_dependencies("${AE_ENDPOINT}" "${AE_INCLUDE_OWNER}")
    list(REMOVE_ITEM _owner_sources ${_unique})
    list(APPEND _owner_sources "$<TARGET_OBJECTS:${AE_ENDPOINT}>")
    set_property(TARGET "${AE_OWNER}" PROPERTY SOURCES "${_owner_sources}")
    set_property(TARGET "${AE_ENDPOINT}" PROPERTY AROS_ARCH_COMPILATION_OWNER "${AE_OWNER}")
    get_property(_scheduled DIRECTORY PROPERTY AROS_ARCH_OBJECT_FINALIZER_SCHEDULED)
    set_property(DIRECTORY APPEND PROPERTY AROS_ARCH_OBJECT_ENDPOINTS "${AE_ENDPOINT}")
    if(NOT _scheduled)
        set_property(DIRECTORY PROPERTY AROS_ARCH_OBJECT_FINALIZER_SCHEDULED TRUE)
        cmake_language(DEFER CALL aros_finalize_arch_object_endpoints)
    endif()
endfunction()
