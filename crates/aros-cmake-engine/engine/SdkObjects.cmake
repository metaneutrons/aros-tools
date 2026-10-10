include_guard(GLOBAL)
include("${CMAKE_CURRENT_LIST_DIR}/NativeConsumerContract.cmake")

# Source-declared standalone compilation and exact Developer-library staging.
# No Make intermediate is read: each object has a private CMake compilation
# owner. Language, flags, prerequisites and object sets must come from the
# transpiler's closed source proof, not from a board or filename heuristic.
function(aros_compile_sdk_object)
    cmake_parse_arguments(PARSE_ARGV 0 O "" "NAME;SOURCE;INTERMEDIATE;OUTPUT;LANGUAGE"
        "DEFINES;UNDEFINES;OPTIONS;INCLUDES")
    if(O_UNPARSED_ARGUMENTS OR O_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "SDK object: invalid or incomplete arguments")
    endif()
    foreach(key NAME SOURCE INTERMEDIATE OUTPUT LANGUAGE)
        if(NOT DEFINED O_${key} OR "${O_${key}}" STREQUAL "")
            message(FATAL_ERROR "SDK object: missing ${key}")
        endif()
    endforeach()
    if(NOT O_NAME MATCHES "^[A-Za-z0-9][A-Za-z0-9_-]*$" OR TARGET "${O_NAME}")
        message(FATAL_ERROR "SDK object: unsafe or duplicate owner")
    endif()
    if(NOT O_LANGUAGE STREQUAL "C" AND NOT O_LANGUAGE STREQUAL "CXX")
        message(FATAL_ERROR "SDK object: language must be C or CXX")
    endif()
    if(NOT DEFINED AROS_SOURCE_DIR OR NOT DEFINED AROS_BUILD_DIR OR
       NOT DEFINED AROS_DEVELOPER_LIB_DIR)
        message(FATAL_ERROR "SDK object: configured source/build/Developer roots are required")
    endif()
    cmake_path(ABSOLUTE_PATH O_SOURCE NORMALIZE OUTPUT_VARIABLE source)
    cmake_path(ABSOLUTE_PATH AROS_SOURCE_DIR NORMALIZE OUTPUT_VARIABLE source_root)
    cmake_path(IS_PREFIX source_root "${source}" NORMALIZE source_inside)
    if(NOT source_inside OR NOT EXISTS "${source}" OR IS_DIRECTORY "${source}" OR
       IS_SYMLINK "${source}")
        message(FATAL_ERROR "SDK object: source must be a regular local source file")
    endif()
    file(REAL_PATH "${source}" physical_source)
    file(REAL_PATH "${source_root}" physical_source_root)
    cmake_path(IS_PREFIX physical_source_root "${physical_source}" NORMALIZE physically_inside)
    if(NOT physically_inside)
        message(FATAL_ERROR "SDK object: source escapes the physical source root")
    endif()
    if(O_LANGUAGE STREQUAL "C" AND NOT source MATCHES "\\.c$" OR
       O_LANGUAGE STREQUAL "CXX" AND NOT source MATCHES "\\.cpp$")
        message(FATAL_ERROR "SDK object: source extension disagrees with its declared language")
    endif()
    cmake_path(ABSOLUTE_PATH O_OUTPUT NORMALIZE OUTPUT_VARIABLE output)
    cmake_path(GET output PARENT_PATH output_parent)
    cmake_path(ABSOLUTE_PATH AROS_DEVELOPER_LIB_DIR NORMALIZE OUTPUT_VARIABLE lib_root)
    cmake_path(ABSOLUTE_PATH AROS_BUILD_DIR NORMALIZE OUTPUT_VARIABLE binary_root)
    if(IS_SYMLINK "${binary_root}")
        message(FATAL_ERROR "SDK object: symlink build root refused")
    endif()
    cmake_path(IS_PREFIX binary_root "${lib_root}" NORMALIZE lib_inside)
    cmake_path(GET output FILENAME output_name)
    if(NOT lib_inside OR lib_root STREQUAL binary_root OR
       NOT output_parent STREQUAL lib_root OR
       NOT output_name MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*\\.o$")
        message(FATAL_ERROR "SDK object: output must be one Developer-library object")
    endif()
    cmake_path(ABSOLUTE_PATH O_INTERMEDIATE NORMALIZE OUTPUT_VARIABLE intermediate)
    set(gen_root "${AROS_BUILD_DIR}/gen")
    cmake_path(ABSOLUTE_PATH gen_root NORMALIZE OUTPUT_VARIABLE gen_root)
    cmake_path(IS_PREFIX gen_root "${intermediate}" NORMALIZE gen_inside)
    cmake_path(GET intermediate FILENAME intermediate_name)
    if(NOT gen_inside OR NOT intermediate_name STREQUAL output_name OR
       intermediate STREQUAL output)
        message(FATAL_ERROR "SDK object: intermediate must be the matching generated object")
    endif()
    # CMake output ownership is global, including objects from other mmakefiles.
    # Case-fold before hashing because some hosts use case-insensitive volumes.
    foreach(path "${intermediate}" "${output}")
        string(TOLOWER "${path}" folded_path)
        string(SHA256 key "${folded_path}")
        get_property(prior GLOBAL PROPERTY "AROS_SDK_OBJECT_${key}")
        if(prior)
            message(FATAL_ERROR "SDK object: duplicate output owned by ${prior}")
        endif()
    endforeach()
    foreach(value IN LISTS O_DEFINES O_UNDEFINES)
        if(NOT value MATCHES "^[A-Za-z_][A-Za-z0-9_]*(=[A-Za-z0-9_.,+/-]+)?$")
            message(FATAL_ERROR "SDK object: unsafe preprocessor value")
        endif()
    endforeach()
    foreach(value IN LISTS O_UNDEFINES)
        if(value MATCHES "=")
            message(FATAL_ERROR "SDK object: undefinition cannot have a value")
        endif()
    endforeach()
    foreach(value IN LISTS O_OPTIONS)
        set(safe_options -g -g0 -g1 -g2 -g3 -O0 -O1 -O2 -O3 -Os -Og -Ofast
            -pipe -pthread -fexceptions -fno-exceptions -frtti -fno-rtti
            -fstrict-aliasing -fno-strict-aliasing -fwrapv -fno-common -fcommon
            -fshort-wchar -fvisibility=hidden -fvisibility=default
            -fno-omit-frame-pointer -fomit-frame-pointer -fno-builtin)
        if(NOT value IN_LIST safe_options AND
           NOT value MATCHES "^-std=(c|gnu|c\\+\\+|gnu\\+\\+)(89|90|99|11|14|17|20|23)$" AND
           NOT value MATCHES "^-W[A-Za-z0-9_][A-Za-z0-9_.=-]*$")
            message(FATAL_ERROR "SDK object: unsafe compile option")
        endif()
    endforeach()
    foreach(path IN LISTS O_INCLUDES)
        if(NOT IS_ABSOLUTE "${path}" OR path MATCHES "[;\n\r$]")
            message(FATAL_ERROR "SDK object: include directory must be a resolved absolute path")
        endif()
    endforeach()
    foreach(path "${intermediate}" "${output}")
        string(TOLOWER "${path}" folded_path)
        string(SHA256 key "${folded_path}")
        set_property(GLOBAL PROPERTY "AROS_SDK_OBJECT_${key}" "${O_NAME}")
    endforeach()
    set(compile_target "${O_NAME}-compile")
    add_library("${compile_target}" OBJECT EXCLUDE_FROM_ALL "${source}")
    # A replaced existing output can otherwise look up-to-date to Ninja. Run
    # the path check even on an unchanged build, before compiling or staging.
    set(check_target "${O_NAME}-check")
    add_custom_target("${check_target}"
        COMMAND "${CMAKE_COMMAND}"
            "-DBINARY_ROOT=${binary_root}"
            "-DSOURCE_OBJECT=$<TARGET_OBJECTS:${compile_target}>"
            "-DSOURCE_FILE=${source}"
            "-DSOURCE_ROOT=${source_root}"
            "-DINTERMEDIATE=${intermediate}"
            "-DOUTPUT=${output}"
            "-DLIB_ROOT=${lib_root}"
            -DVERIFY_ONLY=ON
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunSdkObject.cmake"
        VERBATIM)
    add_dependencies("${compile_target}" "${check_target}")
    set_target_properties("${compile_target}" PROPERTIES POSITION_INDEPENDENT_CODE OFF)
    target_compile_definitions("${compile_target}" PRIVATE ${O_DEFINES})
    target_compile_options("${compile_target}" PRIVATE ${O_OPTIONS})
    foreach(value IN LISTS O_UNDEFINES)
        target_compile_options("${compile_target}" PRIVATE "-U${value}")
    endforeach()
    cmake_path(GET source PARENT_PATH source_directory)
    # Make compiles in the declaring source directory. Preserve that quote
    # include role without adding it as a system include or borrowing a sysroot.
    target_compile_options("${compile_target}" PRIVATE "-iquote" "${source_directory}")
    target_include_directories("${compile_target}" PRIVATE ${O_INCLUDES})
    cmake_path(GET intermediate PARENT_PATH intermediate_parent)
    add_custom_command(OUTPUT "${intermediate}" "${output}"
        COMMAND "${CMAKE_COMMAND}"
            "-DBINARY_ROOT=${binary_root}"
            "-DSOURCE_OBJECT=$<TARGET_OBJECTS:${compile_target}>"
            "-DSOURCE_FILE=${source}"
            "-DSOURCE_ROOT=${source_root}"
            "-DINTERMEDIATE=${intermediate}"
            "-DOUTPUT=${output}"
            "-DLIB_ROOT=${lib_root}"
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunSdkObject.cmake"
        DEPENDS "$<TARGET_OBJECTS:${compile_target}>"
            "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunSdkObject.cmake"
        COMMENT "Publishing source-declared SDK object ${output_name}"
        COMMAND_EXPAND_LISTS VERBATIM)
    add_custom_target("${O_NAME}" DEPENDS "${intermediate}" "${output}")
    add_dependencies("${O_NAME}" "${compile_target}")
    add_dependencies("${O_NAME}" "${check_target}")
endfunction()

# Bind a finite ordinary Make aggregate to already registered object outputs.
function(aros_sdk_object_group)
    cmake_parse_arguments(PARSE_ARGV 0 G "" "NAME" "OBJECTS")
    if(G_UNPARSED_ARGUMENTS OR G_KEYWORDS_MISSING_VALUES OR
       NOT G_NAME MATCHES "^[A-Za-z0-9][A-Za-z0-9_-]*$" OR NOT G_OBJECTS)
        message(FATAL_ERROR "SDK object group: invalid finite aggregate")
    endif()
    if(TARGET "${G_NAME}")
        message(FATAL_ERROR "SDK object group: duplicate owner")
    endif()
    set(owners)
    foreach(path IN LISTS G_OBJECTS)
        cmake_path(ABSOLUTE_PATH path NORMALIZE OUTPUT_VARIABLE object)
        string(TOLOWER "${object}" folded_path)
        string(SHA256 key "${folded_path}")
        get_property(owner GLOBAL PROPERTY "AROS_SDK_OBJECT_${key}")
        if(NOT owner OR NOT TARGET "${owner}")
            message(FATAL_ERROR "SDK object group: object has no source-derived producer")
        endif()
        list(APPEND owners "${owner}")
    endforeach()
    add_custom_target("${G_NAME}")
    add_dependencies("${G_NAME}" ${owners})
    list(REMOVE_DUPLICATES owners)
    set(compile_targets)
    foreach(owner IN LISTS owners)
        list(APPEND compile_targets "${owner}-compile")
    endforeach()
    set_property(TARGET "${G_NAME}" PROPERTY AROS_SDK_OBJECT_COMPILE_TARGETS
        "${compile_targets}")
endfunction()

# These names are part of the existing AROS program-link contract, not a
# board-specific object inventory. Resolve them only from registered source
# owners; never fall back to compiling a guessed source in native mode.
function(aros_bind_source_sdk_program_inputs)
    if(AROS_NATIVE_CONSUMER_CONTRACT OR AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED)
        _aros_native_source_selection_validated(_source_selected)
    endif()
    cmake_parse_arguments(PARSE_ARGV 0 B "" "" "REQUIRE")
    if(B_UNPARSED_ARGUMENTS OR B_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "Native program link contract: invalid required roles")
    endif()
    foreach(required IN LISTS B_REQUIRE)
        if(NOT required MATCHES "^(C_STARTUP|C_DETACH|CXX_STARTUP)$")
            message(FATAL_ERROR "Native program link contract: unknown required role")
        endif()
    endforeach()
    foreach(role C_STARTUP C_DETACH CXX_STARTUP)
        if(role STREQUAL "C_STARTUP")
            set(file startup.o)
        elseif(role STREQUAL "C_DETACH")
            set(file detach.o)
        else()
            set(file cxx-startup.o)
        endif()
        set(path "${AROS_DEVELOPER_LIB_DIR}/${file}")
        string(TOLOWER "${path}" folded_path)
        string(SHA256 key "${folded_path}")
        get_property(owner GLOBAL PROPERTY "AROS_SDK_OBJECT_${key}")
        if(NOT owner OR NOT TARGET "${owner}")
            if(role IN_LIST B_REQUIRE)
                message(FATAL_ERROR "Native program link contract lacks source-owned ${file}")
            endif()
            set("AROS_${role}_TARGET" "" PARENT_SCOPE)
            string(TOLOWER "${role}" output_role)
            set("_aros_${output_role}_output" "" PARENT_SCOPE)
            continue()
        endif()
        set("AROS_${role}_TARGET" "${owner}" PARENT_SCOPE)
        string(TOLOWER "${role}" output_role)
        set("_aros_${output_role}_output" "${path}" PARENT_SCOPE)
    endforeach()
endfunction()
