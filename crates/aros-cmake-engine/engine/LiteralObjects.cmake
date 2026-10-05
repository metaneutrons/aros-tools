include_guard(GLOBAL)

# Compile a source whose Make recipe is an audited, finite compiler argv. This
# deliberately bypasses CMake target compile options: only ARGUMENTS is sent to
# the selected compiler, in its declared order, followed by the private
# dependency/output arguments owned here.
function(aros_compile_literal_object)
    cmake_parse_arguments(PARSE_ARGV 0 L "" "NAME;SOURCE;OUTPUT;LANGUAGE" "ARGUMENTS")
    if(L_UNPARSED_ARGUMENTS OR L_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "Literal object: invalid or incomplete arguments")
    endif()
    foreach(key NAME SOURCE OUTPUT LANGUAGE)
        if(NOT DEFINED L_${key} OR "${L_${key}}" STREQUAL "")
            message(FATAL_ERROR "Literal object: missing ${key}")
        endif()
    endforeach()
    if(NOT L_NAME MATCHES "^[A-Za-z0-9][A-Za-z0-9_-]*$" OR
       TARGET "${L_NAME}" OR TARGET "${L_NAME}-compile" OR TARGET "${L_NAME}-check")
        message(FATAL_ERROR "Literal object: unsafe or duplicate owner")
    endif()
    if(NOT L_LANGUAGE STREQUAL "C" AND NOT L_LANGUAGE STREQUAL "CXX")
        message(FATAL_ERROR "Literal object: language must be C or CXX")
    endif()
    if(NOT DEFINED AROS_SOURCE_DIR OR NOT DEFINED AROS_BUILD_DIR OR
       "${AROS_SOURCE_DIR}" STREQUAL "" OR "${AROS_BUILD_DIR}" STREQUAL "")
        message(FATAL_ERROR "Literal object: configured source and build roots are required")
    endif()

    foreach(value IN ITEMS "${L_NAME}" "${L_SOURCE}" "${L_OUTPUT}"
            "${AROS_SOURCE_DIR}" "${AROS_BUILD_DIR}")
        _aros_literal_reject_control_text("${value}" "path or owner")
    endforeach()

    if(L_LANGUAGE STREQUAL "C")
        set(driver "${CMAKE_C_COMPILER}")
        set(compiler_id "${CMAKE_C_COMPILER_ID}")
        set(compiler_arg1 "${CMAKE_C_COMPILER_ARG1}")
        set(compiler_launcher "${CMAKE_C_COMPILER_LAUNCHER}")
    else()
        set(driver "${CMAKE_CXX_COMPILER}")
        set(compiler_id "${CMAKE_CXX_COMPILER_ID}")
        set(compiler_arg1 "${CMAKE_CXX_COMPILER_ARG1}")
        set(compiler_launcher "${CMAKE_CXX_COMPILER_LAUNCHER}")
    endif()
    if(NOT compiler_id MATCHES "^(GNU|Clang|AppleClang)$")
        message(FATAL_ERROR "Literal object: ${L_LANGUAGE} requires an audited GNU or Clang driver")
    endif()
    if(NOT "${compiler_arg1}" STREQUAL "" OR
       NOT "${compiler_launcher}" STREQUAL "")
        message(FATAL_ERROR
            "Literal object: compiler arguments and launchers are not supported")
    endif()
    _aros_literal_reject_control_text("${driver}" "compiler path")
    if(NOT IS_ABSOLUTE "${driver}")
        message(FATAL_ERROR "Literal object: compiler driver must be an absolute regular file")
    endif()
    cmake_path(ABSOLUTE_PATH driver NORMALIZE OUTPUT_VARIABLE driver)
    _aros_literal_check_absolute_chain("${driver}" "compiler driver")
    _aros_literal_assert_regular("${driver}" "compiler driver" TRUE)

    cmake_path(ABSOLUTE_PATH AROS_SOURCE_DIR NORMALIZE OUTPUT_VARIABLE source_root)
    cmake_path(ABSOLUTE_PATH AROS_BUILD_DIR NORMALIZE OUTPUT_VARIABLE build_root)
    _aros_literal_check_root("${source_root}" "source root")
    _aros_literal_check_root("${build_root}" "build root")

    if(NOT IS_ABSOLUTE "${L_SOURCE}")
        message(FATAL_ERROR "Literal object: source must be an absolute path")
    endif()
    cmake_path(ABSOLUTE_PATH L_SOURCE NORMALIZE OUTPUT_VARIABLE source)
    cmake_path(IS_PREFIX source_root "${source}" NORMALIZE source_inside)
    cmake_path(GET source FILENAME source_name)
    if(NOT source_inside OR source STREQUAL source_root OR
       (L_LANGUAGE STREQUAL "C" AND NOT source_name MATCHES "\.c$") OR
       (L_LANGUAGE STREQUAL "CXX" AND NOT source_name MATCHES "\.cpp$"))
        message(FATAL_ERROR "Literal object: source must be a rooted .c or .cpp file for its language")
    endif()
    _aros_literal_check_path_chain("${source}" "${source_root}" "source")
    _aros_literal_assert_regular("${source}" "source" FALSE)
    cmake_path(GET source PARENT_PATH source_directory)

    if(NOT IS_ABSOLUTE "${L_OUTPUT}")
        message(FATAL_ERROR "Literal object: output must be an absolute path")
    endif()
    cmake_path(ABSOLUTE_PATH L_OUTPUT NORMALIZE OUTPUT_VARIABLE output)
    set(gen_root "${build_root}/gen")
    cmake_path(IS_PREFIX gen_root "${output}" NORMALIZE output_inside)
    cmake_path(GET output FILENAME output_name)
    if(NOT output_inside OR output STREQUAL gen_root OR
       NOT output_name MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*\.o$")
        message(FATAL_ERROR "Literal object: output must be one .o below AROS_BUILD_DIR/gen")
    endif()
    _aros_literal_check_path_chain("${output}" "${build_root}" "output")
    _aros_literal_check_optional_regular("${output}" "object output")

    set(depfile "${output}.d")
    _aros_literal_check_path_chain("${depfile}" "${build_root}" "dependency output")
    _aros_literal_check_optional_regular("${depfile}" "dependency output")

    set(_aros_literal_optimization_flags
        -O0 -O1 -O2 -O3 -O4 -O5 -O6 -O7 -Os -Og -Oz -Ofast)
    set(_aros_literal_standard_flags
        -std=c89 -std=c90 -std=c99 -std=c11 -std=c17 -std=c23
        -std=gnu89 -std=gnu90 -std=gnu99 -std=gnu11 -std=gnu17 -std=gnu23
        -std=c++98 -std=c++11 -std=c++14 -std=c++17 -std=c++20 -std=c++23
        -std=gnu++98 -std=gnu++11 -std=gnu++14 -std=gnu++17 -std=gnu++20 -std=gnu++23)
    set(_aros_literal_safe_flags
        -ansi -pedantic -pedantic-errors -w -g -g0 -g1 -g2 -g3 -pipe -pthread
        -fexceptions -fno-exceptions -frtti -fno-rtti -fstrict-aliasing
        -fno-strict-aliasing -fwrapv -fno-common -fcommon -fshort-wchar
        -fvisibility=hidden -fvisibility=default -fno-omit-frame-pointer
        -fomit-frame-pointer -fno-builtin -ffreestanding -fno-stack-protector
        -fstack-protector -fstack-protector-strong -fstack-protector-all
        -ffunction-sections -fdata-sections -fno-function-sections
        -fno-data-sections -fno-use-cxa-atexit -fno-threadsafe-statics
        -fno-asynchronous-unwind-tables -fno-unwind-tables -fno-ident
        -fPIC -fpic -fPIE -fpie -fno-PIC -fno-pic -fno-PIE -fno-pie
        -nostdinc -nostdinc++)

    set(expect_path_for "")
    set(expect_macro_for "")
    foreach(argument IN LISTS L_ARGUMENTS)
        _aros_literal_reject_control_text("${argument}" "compiler argument")
        if(NOT expect_macro_for STREQUAL "")
            if(NOT argument MATCHES "^[A-Za-z_][A-Za-z0-9_]*(=.*)?$" OR
               (expect_macro_for STREQUAL "-U" AND argument MATCHES "="))
                message(FATAL_ERROR "Literal object: ${expect_macro_for} requires one safe macro name")
            endif()
            set(expect_macro_for "")
            continue()
        endif()
        if(NOT expect_path_for STREQUAL "")
            _aros_literal_check_declared_path(
                "${argument}" "${source_root}" "${build_root}"
                "${source_directory}" "${expect_path_for}")
            set(expect_path_for "")
            continue()
        endif()

        if(argument STREQUAL "-I" OR argument STREQUAL "-isystem" OR
           argument STREQUAL "-iquote" OR argument STREQUAL "--sysroot" OR
           argument STREQUAL "-isysroot")
            set(expect_path_for "${argument}")
            continue()
        elseif(argument STREQUAL "-D" OR argument STREQUAL "-U")
            set(expect_macro_for "${argument}")
            continue()
        elseif(argument MATCHES "^-I(.+)$")
            _aros_literal_check_declared_path(
                "${CMAKE_MATCH_1}" "${source_root}" "${build_root}"
                "${source_directory}" "-I")
            continue()
        elseif(argument MATCHES "^--sysroot=(.+)$")
            _aros_literal_check_declared_path(
                "${CMAKE_MATCH_1}" "${source_root}" "${build_root}"
                "${source_directory}" "--sysroot")
            continue()
        endif()

        if(argument STREQUAL "-c" OR argument MATCHES "^-o.+$" OR
           argument STREQUAL "-o" OR argument MATCHES "^@" OR
           argument MATCHES "^--?(specs|sysroot)(=|$)" OR
           argument MATCHES "^-fplugin|^-X(clang|assembler|preprocessor)$|^-W[alp]," OR
           argument MATCHES "^-M(M|D|MD|F|T|Q|P)?$" OR
           argument STREQUAL "-E" OR argument STREQUAL "-S" OR
           argument STREQUAL "-fsyntax-only" OR argument STREQUAL "--")
            message(FATAL_ERROR "Literal object: unsafe compiler role argument '${argument}'")
        endif()
        if(argument MATCHES "^-D[A-Za-z_][A-Za-z0-9_]*(=.*)?$")
            continue()
        elseif(argument MATCHES "^-U[A-Za-z_][A-Za-z0-9_]*$")
            continue()
        elseif(argument IN_LIST _aros_literal_standard_flags)
            continue()
        elseif(argument IN_LIST _aros_literal_optimization_flags)
            continue()
        elseif(argument IN_LIST _aros_literal_safe_flags)
            continue()
        elseif(argument MATCHES "^-W[A-Za-z0-9_][A-Za-z0-9_.=+-]*$")
            continue()
        elseif(argument MATCHES "^-m(32|64|arm|thumb|thumb-interwork|soft-float|hard-float|strict-align|no-strict-align|no-red-zone|red-zone|big-endian|little-endian|no-unaligned-access|unaligned-access|68000|68010|68020|68030|68040|68060|cpu32)$" OR
               argument MATCHES "^-m(float-abi=(soft|softfp|hard)|abi=[A-Za-z0-9_.+-]+|arch=[A-Za-z0-9_.+-]+|cpu=[A-Za-z0-9_.+-]+|tune=[A-Za-z0-9_.+-]+|fpu=[A-Za-z0-9_.+-]+|cmodel=(medany|medlow|small|medium|large|kernel|tiny))$")
            continue()
        endif()
        message(FATAL_ERROR "Literal object: unsafe or unsupported compiler argument '${argument}'")
    endforeach()
    if(NOT expect_path_for STREQUAL "")
        message(FATAL_ERROR "Literal object: ${expect_path_for} requires a declared in-root path")
    endif()
    if(NOT expect_macro_for STREQUAL "")
        message(FATAL_ERROR "Literal object: ${expect_macro_for} requires one safe macro name")
    endif()

    # Share the SDK ownership namespace so a literal output cannot claim an
    # SDK object's generated intermediate, regardless of declaration order.
    string(TOLOWER "${output}" folded_output)
    string(SHA256 output_key "${folded_output}")
    get_property(prior_owner GLOBAL PROPERTY "AROS_SDK_OBJECT_${output_key}")
    if(prior_owner)
        message(FATAL_ERROR "Literal object: output already owned by ${prior_owner}")
    endif()
    set_property(GLOBAL PROPERTY "AROS_SDK_OBJECT_${output_key}" "${L_NAME}")
    set_property(GLOBAL PROPERTY "AROS_LITERAL_OBJECT_${output_key}" "${L_NAME}")

    string(SHA256 command_key "${output}")
    set(command_root "${CMAKE_CURRENT_BINARY_DIR}")
    _aros_literal_reject_control_text("${command_root}" "CMake binary root")
    _aros_literal_check_root("${command_root}" "CMake binary root")
    set(command_file "${command_root}/CMakeFiles/aros-literal-object-${command_key}.cmake")
    _aros_literal_check_path_chain("${command_file}" "${command_root}" "command file")
    _aros_literal_check_optional_regular("${command_file}" "command file")
    cmake_path(GET source PARENT_PATH source_directory)
    set(command_contents "# Generated literal compiler invocation.\n")
    _aros_literal_append_set(command_contents SOURCE "${source}")
    _aros_literal_append_set(command_contents SOURCE_ROOT "${source_root}")
    _aros_literal_append_set(command_contents BUILD_ROOT "${build_root}")
    _aros_literal_append_set(command_contents SOURCE_DIRECTORY "${source_directory}")
    _aros_literal_append_set(command_contents OUTPUT "${output}")
    _aros_literal_append_set(command_contents DEPFILE "${depfile}")
    _aros_literal_append_set(command_contents COMPILER "${driver}")
    _aros_literal_append_set(command_contents LANGUAGE "${L_LANGUAGE}")
    string(APPEND command_contents "set(ARGUMENTS\n")
    foreach(argument IN LISTS L_ARGUMENTS)
        string(APPEND command_contents "    [==[${argument}]==]\n")
    endforeach()
    string(APPEND command_contents ")\n")
    file(MAKE_DIRECTORY "${command_root}/CMakeFiles")
    if(EXISTS "${command_file}")
        file(READ "${command_file}" current_command_contents)
    else()
        set(current_command_contents "")
    endif()
    if(NOT "${current_command_contents}" STREQUAL "${command_contents}")
        file(WRITE "${command_file}" "${command_contents}")
    endif()
    _aros_literal_check_path_chain("${command_file}" "${command_root}" "command file")
    _aros_literal_assert_regular("${command_file}" "command file" FALSE)
    file(SHA256 "${command_file}" command_sha256)

    set(check_target "${L_NAME}-check")
    set(compile_target "${L_NAME}-compile")
    set(runner "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunLiteralObject.cmake")
    add_custom_target("${check_target}"
        COMMAND "${CMAKE_COMMAND}"
            "-DCOMMAND_FILE=${command_file}"
            "-DCOMMAND_ROOT=${command_root}"
            "-DCOMMAND_SHA256=${command_sha256}"
            -DVERIFY_ONLY=ON
            -P "${runner}"
        COMMENT "Checking literal object paths for ${output_name}"
        VERBATIM)
    add_custom_command(OUTPUT "${output}"
        COMMAND "${CMAKE_COMMAND}"
            "-DCOMMAND_FILE=${command_file}"
            "-DCOMMAND_ROOT=${command_root}"
            "-DCOMMAND_SHA256=${command_sha256}"
            -DVERIFY_ONLY=OFF
            -P "${runner}"
        DEPENDS "${source}" "${driver}" "${runner}" "${command_file}"
        DEPFILE "${depfile}"
        COMMENT "Compiling literal object ${output_name}"
        VERBATIM)
    add_custom_target("${compile_target}" DEPENDS "${output}")
    add_dependencies("${compile_target}" "${check_target}")
    add_custom_target("${L_NAME}")
    add_dependencies("${L_NAME}" "${compile_target}")
    set_property(TARGET "${compile_target}" PROPERTY AROS_LITERAL_OBJECT_OUTPUT "${output}")
    set_property(TARGET "${L_NAME}" PROPERTY AROS_LITERAL_OBJECT_COMPILE_TARGETS
        "${compile_target}")
endfunction()

# Bind a finite Make aggregate only to object declarations registered above.
function(aros_literal_object_group)
    cmake_parse_arguments(PARSE_ARGV 0 G "" "NAME" "OBJECTS")
    if(G_UNPARSED_ARGUMENTS OR G_KEYWORDS_MISSING_VALUES OR
       NOT G_NAME MATCHES "^[A-Za-z0-9][A-Za-z0-9_-]*$" OR NOT G_OBJECTS)
        message(FATAL_ERROR "Literal object group: invalid finite aggregate")
    endif()
    if(TARGET "${G_NAME}")
        message(FATAL_ERROR "Literal object group: duplicate owner")
    endif()

    set(owners)
    set(compile_targets)
    foreach(path IN LISTS G_OBJECTS)
        _aros_literal_reject_control_text("${path}" "group object path")
        if(NOT IS_ABSOLUTE "${path}")
            message(FATAL_ERROR "Literal object group: objects must be exact absolute output paths")
        endif()
        cmake_path(ABSOLUTE_PATH path NORMALIZE OUTPUT_VARIABLE object)
        string(TOLOWER "${object}" folded_object)
        string(SHA256 key "${folded_object}")
        get_property(owner GLOBAL PROPERTY "AROS_LITERAL_OBJECT_${key}")
        if(NOT owner OR NOT TARGET "${owner}")
            message(FATAL_ERROR "Literal object group: object has no registered literal producer: ${object}")
        endif()
        get_target_property(_targets "${owner}" AROS_LITERAL_OBJECT_COMPILE_TARGETS)
        if(NOT _targets OR _targets STREQUAL "_targets-NOTFOUND")
            message(FATAL_ERROR "Literal object group: producer has no compile target: ${object}")
        endif()
        set(exact_owner_found FALSE)
        foreach(compile_target IN LISTS _targets)
            get_target_property(owned_object "${compile_target}" AROS_LITERAL_OBJECT_OUTPUT)
            if("${owned_object}" STREQUAL "${object}")
                set(exact_owner_found TRUE)
            endif()
        endforeach()
        if(NOT exact_owner_found)
            message(FATAL_ERROR
                "Literal object group: path is not the exact registered output: ${object}")
        endif()
        list(APPEND owners "${owner}")
        list(APPEND compile_targets ${_targets})
    endforeach()
    list(REMOVE_DUPLICATES owners)
    list(REMOVE_DUPLICATES compile_targets)
    add_custom_target("${G_NAME}")
    add_dependencies("${G_NAME}" ${owners})
    set_property(TARGET "${G_NAME}" PROPERTY AROS_LITERAL_OBJECT_COMPILE_TARGETS
        "${compile_targets}")
endfunction()

function(_aros_literal_reject_control_text value label)
    foreach(character ";" "\n" "\r" "$" "@" "|" "&" "<" ">" "`")
        string(FIND "${value}" "${character}" position)
        if(NOT position EQUAL -1)
            message(FATAL_ERROR "Literal object: unsafe ${label} contains a control character")
        endif()
    endforeach()
    string(FIND "${value}" "]==]" bracket_delimiter)
    string(FIND "${value}" "]===]" extended_bracket_delimiter)
    if(NOT bracket_delimiter EQUAL -1 OR NOT extended_bracket_delimiter EQUAL -1)
        message(FATAL_ERROR "Literal object: unsafe ${label} contains a CMake serialization delimiter")
    endif()
endfunction()

function(_aros_literal_assert_regular path label executable)
    find_program(_aros_literal_test_program NAMES test PATHS /usr/bin /bin NO_DEFAULT_PATH NO_CACHE REQUIRED)
    execute_process(COMMAND "${_aros_literal_test_program}" -f "${path}" RESULT_VARIABLE result)
    if(NOT result EQUAL 0)
        message(FATAL_ERROR "Literal object: ${label} is not a regular file: ${path}")
    endif()
    if(executable)
        execute_process(COMMAND "${_aros_literal_test_program}" -x "${path}" RESULT_VARIABLE result)
        if(NOT result EQUAL 0)
            message(FATAL_ERROR "Literal object: compiler driver is not executable: ${path}")
        endif()
    endif()
endfunction()

function(_aros_literal_check_absolute_chain path label)
    set(cursor "${path}")
    while(TRUE)
        if(IS_SYMLINK "${cursor}")
            message(FATAL_ERROR "Literal object: symlink ${label} path refused: ${cursor}")
        endif()
        cmake_path(GET cursor PARENT_PATH parent)
        if(parent STREQUAL cursor)
            break()
        endif()
        set(cursor "${parent}")
    endwhile()
endfunction()

function(_aros_literal_check_root root label)
    if(NOT IS_ABSOLUTE "${root}" OR NOT IS_DIRECTORY "${root}")
        message(FATAL_ERROR "Literal object: ${label} must be an existing directory")
    endif()
    if(IS_SYMLINK "${root}")
        message(FATAL_ERROR "Literal object: symlink ${label} refused: ${root}")
    endif()
endfunction()

function(_aros_literal_check_path_chain path boundary label)
    cmake_path(IS_PREFIX boundary "${path}" NORMALIZE inside)
    if(NOT inside OR path STREQUAL boundary)
        message(FATAL_ERROR "Literal object: ${label} escapes its declared root")
    endif()
    set(cursor "${path}")
    while(NOT cursor STREQUAL boundary)
        if(IS_SYMLINK "${cursor}")
            message(FATAL_ERROR "Literal object: symlink ${label} path refused: ${cursor}")
        endif()
        if(EXISTS "${cursor}" AND NOT cursor STREQUAL path AND NOT IS_DIRECTORY "${cursor}")
            message(FATAL_ERROR "Literal object: non-directory ${label} parent: ${cursor}")
        endif()
        cmake_path(GET cursor PARENT_PATH parent)
        if(parent STREQUAL cursor)
            message(FATAL_ERROR "Literal object: invalid ${label} root boundary")
        endif()
        set(cursor "${parent}")
    endwhile()
endfunction()

function(_aros_literal_check_optional_regular path label)
    if(IS_SYMLINK "${path}")
        message(FATAL_ERROR "Literal object: symlink ${label} refused: ${path}")
    endif()
    if(EXISTS "${path}")
        _aros_literal_assert_regular("${path}" "${label}" FALSE)
    endif()
endfunction()

function(_aros_literal_check_declared_path raw source_root build_root source_directory role)
    _aros_literal_reject_control_text("${raw}" "${role} path")
    if(IS_ABSOLUTE "${raw}")
        set(path "${raw}")
    else()
        # The direct compiler invocation runs in the declaring source's
        # directory, so relative include/sysroot paths keep that Make role.
        cmake_path(ABSOLUTE_PATH raw BASE_DIRECTORY "${source_directory}"
            NORMALIZE OUTPUT_VARIABLE path)
    endif()
    cmake_path(ABSOLUTE_PATH path NORMALIZE OUTPUT_VARIABLE path)
    cmake_path(IS_PREFIX source_root "${path}" NORMALIZE in_source)
    cmake_path(IS_PREFIX build_root "${path}" NORMALIZE in_build)
    if(NOT in_source AND NOT in_build)
        message(FATAL_ERROR "Literal object: ${role} path is outside declared source/build roots: ${path}")
    endif()
    if(in_source)
        set(boundary "${source_root}")
    else()
        set(boundary "${build_root}")
    endif()
    if(path STREQUAL boundary)
        return()
    endif()
    _aros_literal_check_path_chain("${path}" "${boundary}" "${role}")
    if(EXISTS "${path}" AND NOT IS_DIRECTORY "${path}")
        message(FATAL_ERROR "Literal object: ${role} path is not a directory: ${path}")
    endif()
endfunction()

function(_aros_literal_append_set output_variable name value)
    set(text "${${output_variable}}set(${name} [==[${value}]==])\n")
    set(${output_variable} "${text}" PARENT_SCOPE)
endfunction()
