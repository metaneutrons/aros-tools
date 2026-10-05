# Concrete architecture object effects, not aliases to their module owner.
# The caller must supply source-bound, selected declarations. Unsupported or
# shadowed objects remain errors; no empty compilation target is synthesized.
include_guard(GLOBAL)
include(CMakeParseArguments)

# Dependency and default-link passes may add compilation usage requirements
# after the graph has created these endpoints. Snapshot the final owner state
# at directory end, not only at the point where its sources are split.
function(aros_finalize_arch_object_endpoints)
    get_property(_endpoints DIRECTORY PROPERTY AROS_ARCH_OBJECT_ENDPOINTS)
    foreach(_endpoint IN LISTS _endpoints)
        get_target_property(_owner "${_endpoint}" AROS_ARCH_COMPILATION_OWNER)
        _aros_copy_arch_owner_state("${_endpoint}" "${_owner}")
    endforeach()
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
        if(NOT _name MATCHES "^[a-zA-Z0-9_.+-]+$" OR _name STREQUAL "." OR _name STREQUAL "..")
            message(FATAL_ERROR "Unsafe architecture source basename ${_name}")
        endif()
        set(_matches "")
        foreach(_source IN LISTS _owner_sources)
            if(_source MATCHES "\\$<")
                continue()
            endif()
            get_filename_component(_directory "${_source}" DIRECTORY)
            get_filename_component(_basename "${_source}" NAME_WE)
            if(_directory STREQUAL AS_DIRECTORY AND _basename STREQUAL _name)
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
