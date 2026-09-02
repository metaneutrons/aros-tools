include(CMakeParseArguments)

# aros_bind_python_output_consumers(
#     OWNER <python-generator-owner>
#     CONSUMERS <compile-targets...>)
#
# Binds an already declared Python-output owner to compile targets. This is a
# separate operation because generated sources must be registered before a
# concrete target is declared, while that target can only be named as a
# dependency afterwards.
function(aros_bind_python_output_consumers)
    set(oneValueArgs OWNER)
    set(multiValueArgs CONSUMERS)
    cmake_parse_arguments(PB "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})
    if(PB_UNPARSED_ARGUMENTS OR PB_KEYWORDS_MISSING_VALUES OR
       NOT PB_OWNER OR NOT PB_CONSUMERS)
        message(FATAL_ERROR
            "aros_bind_python_output_consumers requires OWNER and CONSUMERS")
    endif()
    if(NOT PB_OWNER MATCHES "^[A-Za-z0-9_.+-]+$" OR
       NOT TARGET "${PB_OWNER}")
        message(FATAL_ERROR
            "${PB_OWNER}: missing Python-generator owner target")
    endif()
    get_target_property(_is_python_owner "${PB_OWNER}"
        AROS_PYTHON_OUTPUT_OWNER)
    if(NOT _is_python_owner)
        message(FATAL_ERROR
            "${PB_OWNER}: target is not a Python-generator owner")
    endif()

    list(REMOVE_DUPLICATES PB_CONSUMERS)
    foreach(_consumer IN LISTS PB_CONSUMERS)
        if(NOT TARGET "${_consumer}")
            message(FATAL_ERROR
                "${PB_OWNER}: missing Python-generator consumer ${_consumer}")
        endif()
        get_target_property(_consumer_type "${_consumer}" TYPE)
        if(NOT _consumer_type MATCHES
           "^(EXECUTABLE|STATIC_LIBRARY|SHARED_LIBRARY|MODULE_LIBRARY|OBJECT_LIBRARY)$")
            message(FATAL_ERROR
                "${PB_OWNER}: Python-generator consumer ${_consumer} does not compile")
        endif()
        add_dependencies("${_consumer}" "${PB_OWNER}")
    endforeach()
endfunction()

# aros_generate_python_outputs(
#     OWNER <target>
#     SOURCE_ROOT <fetched-source-root>
#     BUILD_ROOT <private-generated-root>
#     FETCH_TARGET <fetch-owner>
#     [DRIVER_SCRIPT <repository-owned-adapter>]
#     [PACKAGE_FETCH_TARGETS <fetch-owners...>
#      PACKAGE_SOURCE_ROOTS <unpacked-roots...>
#      PACKAGE_PYTHON_PATHS <root-relative-import-paths...>]
#     [SOURCE_INPUTS <source-relative-files...>]
#     [CONSUMERS <compile-targets...>]
#     JOB
#       SCRIPT <source-relative-python-file>
#       OUTPUT <build-relative-product>
#       [ARGUMENTS <generator-arguments...>]
#     [JOB ...])
#
# Declares one output-tracked command per Python/stdout generator and groups all
# products under OWNER.  The source scripts and shared inputs are side effects
# of FETCH_TARGET, so the fetch completion stamp is the only file dependency:
# naming not-yet-unpacked files directly would make a fresh Ninja graph fail
# before the fetch rule can create them.  The runner verifies those inputs once
# the stamp is current and replaces each product only after Python succeeds.
function(aros_generate_python_outputs)
    set(_raw_arguments ${ARGN})
    list(FIND _raw_arguments "JOB" _first_job)
    if(_first_job LESS 0)
        message(FATAL_ERROR
            "aros_generate_python_outputs requires at least one JOB")
    endif()

    list(SUBLIST _raw_arguments 0 ${_first_job} _common_arguments)
    set(oneValueArgs OWNER SOURCE_ROOT BUILD_ROOT FETCH_TARGET DRIVER_SCRIPT)
    set(multiValueArgs SOURCE_INPUTS CONSUMERS PACKAGE_FETCH_TARGETS
        PACKAGE_SOURCE_ROOTS PACKAGE_PYTHON_PATHS)
    cmake_parse_arguments(PG "" "${oneValueArgs}" "${multiValueArgs}"
        ${_common_arguments})
    if(PG_UNPARSED_ARGUMENTS OR PG_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_generate_python_outputs received malformed common arguments")
    endif()
    foreach(_required IN ITEMS OWNER SOURCE_ROOT BUILD_ROOT FETCH_TARGET)
        if(NOT PG_${_required})
            message(FATAL_ERROR
                "aros_generate_python_outputs requires ${_required}")
        endif()
    endforeach()
    foreach(_name IN ITEMS PG_OWNER PG_FETCH_TARGET)
        if(NOT "${${_name}}" MATCHES "^[A-Za-z0-9_.+-]+$")
            message(FATAL_ERROR
                "${PG_OWNER}: invalid Python-generator target name '${${_name}}'")
        endif()
    endforeach()
    if(TARGET "${PG_OWNER}")
        message(FATAL_ERROR
            "${PG_OWNER}: Python-generator owner target was already declared")
    endif()
    if(NOT TARGET "${PG_FETCH_TARGET}")
        message(FATAL_ERROR
            "${PG_OWNER}: missing Python-generator fetch target ${PG_FETCH_TARGET}")
    endif()

    if(PG_DRIVER_SCRIPT)
        if("${PG_DRIVER_SCRIPT}" MATCHES "[;\"$\\\r\n]")
            message(FATAL_ERROR
                "${PG_OWNER}: unsafe Python-generator driver path '${PG_DRIVER_SCRIPT}'")
        endif()
        cmake_path(ABSOLUTE_PATH CMAKE_SOURCE_DIR NORMALIZE
            OUTPUT_VARIABLE _repository_root)
        cmake_path(ABSOLUTE_PATH PG_DRIVER_SCRIPT
            BASE_DIRECTORY "${_repository_root}" NORMALIZE
            OUTPUT_VARIABLE _driver)
        cmake_path(IS_PREFIX _repository_root "${_driver}" NORMALIZE
            _driver_is_owned)
        if(NOT _driver_is_owned OR _driver STREQUAL _repository_root OR
           NOT EXISTS "${_driver}" OR IS_DIRECTORY "${_driver}")
            message(FATAL_ERROR
                "${PG_OWNER}: driver is missing or outside the source tree: ${_driver}")
        endif()
    else()
        set(_driver "")
    endif()

    get_property(_fetch_destination TARGET "${PG_FETCH_TARGET}"
        PROPERTY AROS_FETCH_DESTINATION)
    get_property(_fetch_stamp TARGET "${PG_FETCH_TARGET}"
        PROPERTY AROS_FETCH_COMPLETION_STAMP)
    if(NOT _fetch_destination OR NOT _fetch_stamp)
        message(FATAL_ERROR
            "${PG_OWNER}: ${PG_FETCH_TARGET} is not a complete fetch owner")
    endif()

    # Python is a host tool even in a cross build. Resolve and execute it while
    # configuring, so a missing or unusable interpreter never becomes a late,
    # opaque custom-command failure.
    find_package(Python3 COMPONENTS Interpreter QUIET)
    if(NOT Python3_Interpreter_FOUND OR NOT Python3_EXECUTABLE)
        message(FATAL_ERROR
            "${PG_OWNER}: a working Python 3 interpreter is required; install python3 or set Python3_EXECUTABLE")
    endif()
    execute_process(
        COMMAND "${Python3_EXECUTABLE}" -c
            "import sys; raise SystemExit(0 if sys.version_info.major == 3 else 1)"
        RESULT_VARIABLE _python_probe_result
        OUTPUT_QUIET
        ERROR_QUIET
        TIMEOUT 10)
    if(NOT "${_python_probe_result}" STREQUAL "0")
        message(FATAL_ERROR
            "${PG_OWNER}: Python3_EXECUTABLE is not a usable Python 3 interpreter: ${Python3_EXECUTABLE}")
    endif()

    foreach(_path_var IN ITEMS PG_SOURCE_ROOT PG_BUILD_ROOT
            _fetch_destination _fetch_stamp)
        if("${${_path_var}}" MATCHES "[;\"$\\\r\n]")
            message(FATAL_ERROR
                "${PG_OWNER}: unsafe Python-generator path '${${_path_var}}'")
        endif()
    endforeach()
    cmake_path(ABSOLUTE_PATH _fetch_destination
        BASE_DIRECTORY "${CMAKE_BINARY_DIR}" NORMALIZE
        OUTPUT_VARIABLE _fetch_destination)
    cmake_path(ABSOLUTE_PATH PG_SOURCE_ROOT
        BASE_DIRECTORY "${_fetch_destination}" NORMALIZE
        OUTPUT_VARIABLE _source_root)
    cmake_path(IS_PREFIX _fetch_destination "${_source_root}" NORMALIZE
        _source_is_fetched)
    if(NOT _source_is_fetched OR _source_root STREQUAL _fetch_destination)
        message(FATAL_ERROR
            "${PG_OWNER}: SOURCE_ROOT must be a private child of the fetch destination: ${_source_root}")
    endif()

    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR NORMALIZE
        OUTPUT_VARIABLE _binary_root)
    set(_generated_root "${_binary_root}/gen")
    cmake_path(NORMAL_PATH _generated_root)
    cmake_path(ABSOLUTE_PATH PG_BUILD_ROOT
        BASE_DIRECTORY "${_generated_root}" NORMALIZE
        OUTPUT_VARIABLE _build_root)
    cmake_path(IS_PREFIX _generated_root "${_build_root}" NORMALIZE
        _build_is_generated)
    if(NOT _build_is_generated OR _build_root STREQUAL _generated_root)
        message(FATAL_ERROR
            "${PG_OWNER}: BUILD_ROOT must be a private child of ${_generated_root}: ${_build_root}")
    endif()
    cmake_path(IS_PREFIX _source_root "${_build_root}" NORMALIZE
        _source_contains_build)
    cmake_path(IS_PREFIX _build_root "${_source_root}" NORMALIZE
        _build_contains_source)
    if(_source_contains_build OR _build_contains_source)
        message(FATAL_ERROR
            "${PG_OWNER}: SOURCE_ROOT and BUILD_ROOT must not overlap")
    endif()

    # Optional pure-Python dependencies remain fetched private inputs. They are
    # never installed into or resolved from the host Python.
    list(LENGTH PG_PACKAGE_FETCH_TARGETS _package_count)
    foreach(_package_list IN ITEMS PG_PACKAGE_SOURCE_ROOTS PG_PACKAGE_PYTHON_PATHS)
        list(LENGTH ${_package_list} _package_list_length)
        if(NOT _package_list_length EQUAL _package_count)
            message(FATAL_ERROR
                "${PG_OWNER}: Python package declaration lists differ in length")
        endif()
        set(_package_count ${_package_list_length})
    endforeach()

    set(_package_fetch_targets "")
    set(_package_fetch_stamps "")
    set(_package_source_roots "")
    set(_package_python_paths "")
    if(_package_count GREATER 0)
        math(EXPR _last_package "${_package_count} - 1")
        foreach(_index RANGE 0 ${_last_package})
            list(GET PG_PACKAGE_FETCH_TARGETS ${_index} _package_fetch_target)
            list(GET PG_PACKAGE_SOURCE_ROOTS ${_index} _raw_package_root)
            list(GET PG_PACKAGE_PYTHON_PATHS ${_index} _raw_python_path)
            if(NOT _package_fetch_target MATCHES "^[A-Za-z0-9_.+-]+$" OR
               NOT TARGET "${_package_fetch_target}")
                message(FATAL_ERROR
                    "${PG_OWNER}: missing Python package fetch target ${_package_fetch_target}")
            endif()
            get_property(_package_destination TARGET "${_package_fetch_target}"
                PROPERTY AROS_FETCH_DESTINATION)
            get_property(_package_stamp TARGET "${_package_fetch_target}"
                PROPERTY AROS_FETCH_COMPLETION_STAMP)
            if(NOT _package_destination OR NOT _package_stamp)
                message(FATAL_ERROR
                    "${PG_OWNER}: ${_package_fetch_target} is not a complete fetch owner")
            endif()
            foreach(_package_path IN ITEMS _raw_package_root
                    _raw_python_path _package_destination
                    _package_stamp)
                if("${${_package_path}}" MATCHES "[;\"$\\\r\n]")
                    message(FATAL_ERROR
                        "${PG_OWNER}: unsafe Python package path '${${_package_path}}'")
                endif()
            endforeach()
            cmake_path(ABSOLUTE_PATH _package_destination
                BASE_DIRECTORY "${CMAKE_BINARY_DIR}" NORMALIZE
                OUTPUT_VARIABLE _package_destination)
            cmake_path(ABSOLUTE_PATH _raw_package_root
                BASE_DIRECTORY "${_package_destination}" NORMALIZE
                OUTPUT_VARIABLE _package_root)
            cmake_path(IS_PREFIX _package_destination "${_package_root}" NORMALIZE
                _package_root_is_fetched)
            if(NOT _package_root_is_fetched OR
               _package_root STREQUAL _package_destination)
                message(FATAL_ERROR
                    "${PG_OWNER}: Python package root escapes its fetch destination: ${_package_root}")
            endif()
            cmake_path(ABSOLUTE_PATH _raw_python_path
                BASE_DIRECTORY "${_package_root}" NORMALIZE
                OUTPUT_VARIABLE _python_path)
            cmake_path(IS_PREFIX _package_root "${_python_path}" NORMALIZE
                _python_path_is_owned)
            if(NOT _python_path_is_owned)
                message(FATAL_ERROR
                    "${PG_OWNER}: Python import path escapes its package root: ${_python_path}")
            endif()

            list(APPEND _package_fetch_targets "${_package_fetch_target}")
            list(APPEND _package_fetch_stamps "${_package_stamp}")
            list(APPEND _package_source_roots "${_package_root}")
            list(APPEND _package_python_paths "${_python_path}")
        endforeach()
    endif()

    # The repository-owned Mesa adapter needs working host Flex and Bison
    # executables. Their versions are deliberately not package-pinned here;
    # the upstream recipe does not carry such a constraint.
    if(_driver)
        find_program(_python_generator_flex NAMES flex
            PATHS /opt/homebrew/opt/flex/bin /usr/local/opt/flex/bin
            NO_DEFAULT_PATH)
        if(NOT _python_generator_flex)
            find_program(_python_generator_flex NAMES flex)
        endif()
        find_program(_python_generator_bison NAMES bison
            PATHS /opt/homebrew/opt/bison/bin /usr/local/opt/bison/bin
            NO_DEFAULT_PATH)
        if(NOT _python_generator_bison)
            find_program(_python_generator_bison NAMES bison)
        endif()
        if(NOT _python_generator_flex OR NOT _python_generator_bison)
            message(FATAL_ERROR
                "${PG_OWNER}: working Flex and Bison host tools are required")
        endif()
        execute_process(COMMAND "${_python_generator_flex}" --version
            OUTPUT_VARIABLE _flex_version ERROR_VARIABLE _flex_version_error
            RESULT_VARIABLE _flex_version_result OUTPUT_STRIP_TRAILING_WHITESPACE
            TIMEOUT 10)
        execute_process(COMMAND "${_python_generator_bison}" --version
            OUTPUT_VARIABLE _bison_version ERROR_VARIABLE _bison_version_error
            RESULT_VARIABLE _bison_version_result OUTPUT_STRIP_TRAILING_WHITESPACE
            TIMEOUT 10)
        if(NOT "${_flex_version_result}" STREQUAL "0" OR
           NOT "${_bison_version_result}" STREQUAL "0")
            message(FATAL_ERROR
                "${PG_OWNER}: Flex or Bison is not executable (got '${_flex_version}' / '${_flex_version_error}' and '${_bison_version}' / '${_bison_version_error}')")
        endif()
    endif()

    # Keep fetched compile sources stable across cold and warm configures. On a
    # cold configure they need proxy translation units because the archive is
    # still absent; after fetch, resolving the same stems directly would change
    # object identities and archive members merely because CMake was rerun.
    get_property(_stable_source_roots GLOBAL PROPERTY
        AROS_STABLE_PORT_SOURCE_ROOTS)
    list(APPEND _stable_source_roots "${_source_root}")
    list(REMOVE_DUPLICATES _stable_source_roots)
    set_property(GLOBAL PROPERTY AROS_STABLE_PORT_SOURCE_ROOTS
        "${_stable_source_roots}")

    set(_source_inputs "")
    foreach(_raw_input IN LISTS PG_SOURCE_INPUTS)
        if("${_raw_input}" MATCHES "[;\"$\\\r\n]")
            message(FATAL_ERROR
                "${PG_OWNER}: unsafe Python-generator source input '${_raw_input}'")
        endif()
        set(_source_input "${_raw_input}")
        cmake_path(ABSOLUTE_PATH _source_input
            BASE_DIRECTORY "${_source_root}" NORMALIZE
            OUTPUT_VARIABLE _source_input)
        cmake_path(IS_PREFIX _source_root "${_source_input}" NORMALIZE
            _input_is_owned)
        if(NOT _input_is_owned OR _source_input STREQUAL _source_root)
            message(FATAL_ERROR
                "${PG_OWNER}: SOURCE_INPUT escapes SOURCE_ROOT: ${_source_input}")
        endif()
        list(APPEND _source_inputs "${_source_input}")
    endforeach()
    list(REMOVE_DUPLICATES _source_inputs)

    set(_runner "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunPythonGenerator.cmake")
    list(LENGTH _source_inputs _source_input_count)
    set(_source_input_definitions
        "-DSOURCE_INPUT_COUNT=${_source_input_count}")
    if(_source_input_count GREATER 0)
        math(EXPR _last_source_input "${_source_input_count} - 1")
        foreach(_index RANGE 0 ${_last_source_input})
            list(GET _source_inputs ${_index} _source_input)
            list(APPEND _source_input_definitions
                "-DSOURCE_INPUT_${_index}=${_source_input}")
        endforeach()
    endif()
    set(_package_definitions "-DPACKAGE_COUNT=${_package_count}")
    if(_package_count GREATER 0)
        math(EXPR _last_package "${_package_count} - 1")
        foreach(_index RANGE 0 ${_last_package})
            list(GET _package_source_roots ${_index} _package_root)
            list(GET _package_python_paths ${_index} _python_path)
            list(APPEND _package_definitions
                "-DPACKAGE_SOURCE_ROOT_${_index}=${_package_root}"
                "-DPACKAGE_PYTHON_PATH_${_index}=${_python_path}")
        endforeach()
    endif()

    set(_outputs "")
    list(LENGTH _raw_arguments _argument_count)
    set(_job_marker ${_first_job})
    while(_job_marker LESS _argument_count)
        math(EXPR _job_start "${_job_marker} + 1")
        if(_job_start GREATER_EQUAL _argument_count)
            message(FATAL_ERROR
                "${PG_OWNER}: empty Python-generator JOB")
        endif()
        list(SUBLIST _raw_arguments ${_job_start} -1 _job_tail)
        list(FIND _job_tail "JOB" _relative_next_job)
        if(_relative_next_job LESS 0)
            math(EXPR _job_length "${_argument_count} - ${_job_start}")
            set(_next_job ${_argument_count})
        else()
            set(_job_length ${_relative_next_job})
            math(EXPR _next_job "${_job_start} + ${_relative_next_job}")
        endif()
        if(_job_length EQUAL 0)
            message(FATAL_ERROR
                "${PG_OWNER}: empty Python-generator JOB")
        endif()
        list(SUBLIST _raw_arguments ${_job_start} ${_job_length}
            _job_arguments)
        cmake_parse_arguments(PJ "" "SCRIPT;OUTPUT" "ARGUMENTS"
            ${_job_arguments})
        if(PJ_UNPARSED_ARGUMENTS OR PJ_KEYWORDS_MISSING_VALUES OR
           NOT PJ_SCRIPT OR NOT PJ_OUTPUT)
            message(FATAL_ERROR
                "${PG_OWNER}: malformed Python-generator JOB; SCRIPT and OUTPUT are required")
        endif()

        foreach(_path_var IN ITEMS PJ_SCRIPT PJ_OUTPUT)
            if("${${_path_var}}" MATCHES "[;\"$\\\r\n]")
                message(FATAL_ERROR
                    "${PG_OWNER}: unsafe Python-generator job path '${${_path_var}}'")
            endif()
        endforeach()
        set(_script "${PJ_SCRIPT}")
        cmake_path(ABSOLUTE_PATH _script
            BASE_DIRECTORY "${_source_root}" NORMALIZE
            OUTPUT_VARIABLE _script)
        cmake_path(IS_PREFIX _source_root "${_script}" NORMALIZE
            _script_is_owned)
        if(NOT _script_is_owned OR _script STREQUAL _source_root)
            message(FATAL_ERROR
                "${PG_OWNER}: generator SCRIPT escapes SOURCE_ROOT: ${_script}")
        endif()

        set(_output "${PJ_OUTPUT}")
        cmake_path(ABSOLUTE_PATH _output
            BASE_DIRECTORY "${_build_root}" NORMALIZE
            OUTPUT_VARIABLE _output)
        cmake_path(IS_PREFIX _build_root "${_output}" NORMALIZE
            _output_is_owned)
        if(NOT _output_is_owned OR _output STREQUAL _build_root)
            message(FATAL_ERROR
                "${PG_OWNER}: generator OUTPUT escapes BUILD_ROOT: ${_output}")
        endif()
        if(_output IN_LIST _outputs)
            message(FATAL_ERROR
                "${PG_OWNER}: duplicate Python-generator OUTPUT: ${_output}")
        endif()
        string(SHA256 _output_key "${_output}")
        get_property(_previous_owner GLOBAL PROPERTY
            "AROS_PYTHON_OUTPUT_OWNER_${_output_key}")
        if(_previous_owner)
            message(FATAL_ERROR
                "${PG_OWNER}: ${_output} is already owned by ${_previous_owner}")
        endif()

        foreach(_argument IN LISTS PJ_ARGUMENTS)
            if("${_argument}" MATCHES "[;$\\\r\n]")
                message(FATAL_ERROR
                    "${PG_OWNER}: unsafe Python-generator argument '${_argument}'")
            endif()
        endforeach()

        list(LENGTH PJ_ARGUMENTS _generator_argument_count)
        set(_generator_argument_definitions
            "-DGENERATOR_ARGUMENT_COUNT=${_generator_argument_count}")
        if(_generator_argument_count GREATER 0)
            math(EXPR _last_generator_argument
                "${_generator_argument_count} - 1")
            foreach(_index RANGE 0 ${_last_generator_argument})
                list(GET PJ_ARGUMENTS ${_index} _argument)
                list(APPEND _generator_argument_definitions
                    "-DGENERATOR_ARGUMENT_${_index}=${_argument}")
            endforeach()
        endif()

        add_custom_command(
            OUTPUT "${_output}"
            COMMAND "${CMAKE_COMMAND}"
                "-DOWNER=${PG_OWNER}"
                "-DPYTHON_EXECUTABLE=${Python3_EXECUTABLE}"
                "-DSOURCE_ROOT=${_source_root}"
                "-DBUILD_ROOT=${_build_root}"
                "-DDRIVER_SCRIPT=${_driver}"
                "-DFLEX_EXECUTABLE=${_python_generator_flex}"
                "-DBISON_EXECUTABLE=${_python_generator_bison}"
                "-DGENERATOR_SCRIPT=${_script}"
                "-DOUTPUT=${_output}"
                ${_source_input_definitions}
                ${_package_definitions}
                ${_generator_argument_definitions}
                -P "${_runner}"
            DEPENDS "${_fetch_stamp}" ${_package_fetch_stamps}
                "${_runner}" ${_driver}
            COMMENT "Generating ${_output} with a capability-checked host generator"
            VERBATIM)

        list(APPEND _outputs "${_output}")
        set_property(GLOBAL PROPERTY
            "AROS_PYTHON_OUTPUT_OWNER_${_output_key}" "${PG_OWNER}")
        set(_job_marker ${_next_job})
    endwhile()

    add_custom_target("${PG_OWNER}" DEPENDS ${_outputs})
    add_dependencies("${PG_OWNER}" "${PG_FETCH_TARGET}")
    if(_package_fetch_targets)
        add_dependencies("${PG_OWNER}" ${_package_fetch_targets})
    endif()
    set_property(TARGET "${PG_OWNER}" PROPERTY
        AROS_PYTHON_OUTPUT_OWNER TRUE)
    set_property(TARGET "${PG_OWNER}" PROPERTY
        AROS_PYTHON_OUTPUTS "${_outputs}")

    if(PG_CONSUMERS)
        aros_bind_python_output_consumers(
            OWNER "${PG_OWNER}"
            CONSUMERS ${PG_CONSUMERS})
    endif()
endfunction()

# aros_generate_intree_script_outputs(
#     OWNER <target> SCRIPT <path> OUTPUTS <paths...>
#     [STDOUT] [WORKING_DIRECTORY <path>]
#     [ARGUMENTS <words...>] [DEPENDS <paths...>]
#     [DEPENDENCY_TARGETS <targets...>] [CONSUMERS <targets...>])
#
# An exact Make rule whose recipe runs Python to produce files under $(GENDIR).
# The script can live in-tree or below a capability-checked `%fetch`
# destination. arch/all-pc/udis86/mmakefile.src:26 is the in-tree case:
#
#     $(GENDIR)/$(CURDIR)/libudis86/itab.c: $(OPTABLE) \
#                $(SRCDIR)/$(CURDIR)/scripts/ud_itab.py \
#                $(SRCDIR)/$(CURDIR)/scripts/ud_opcode.py | $(GENDIR)/...
#         $(PYTHON) $(SRCDIR)/$(CURDIR)/scripts/ud_itab.py $(OPTABLE) $(GENDIR)/...
#
# This is deliberately not aros_generate_python_outputs. That function models
# complete package-defined output groups. This one preserves an individual,
# already-declared GNU Make recipe and depends on its existing fetch target when
# the files are external; it neither downloads nor privately pins anything.
#
# What it shares with that function is the part that matters to consumers: each
# output is registered under AROS_PYTHON_OUTPUT_OWNER_<hash>, which is what
# aros_resolve_sources consults before it probes the filesystem, so a declared
# source that does not exist at configure time still resolves.
function(aros_generate_intree_script_outputs)
    set(options STDOUT)
    set(oneValueArgs OWNER SCRIPT WORKING_DIRECTORY)
    set(multiValueArgs ARGUMENTS OUTPUTS DEPENDS DEPENDENCY_TARGETS CONSUMERS)
    cmake_parse_arguments(IG "${options}" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})
    if(IG_UNPARSED_ARGUMENTS OR IG_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_generate_intree_script_outputs: unknown or valueless "
            "arguments: ${IG_UNPARSED_ARGUMENTS}${IG_KEYWORDS_MISSING_VALUES}")
    endif()
    foreach(_required OWNER SCRIPT OUTPUTS)
        if(NOT IG_${_required})
            message(FATAL_ERROR
                "aros_generate_intree_script_outputs: ${_required} is required")
        endif()
    endforeach()
    if(NOT IG_OWNER MATCHES "^[A-Za-z0-9_.+-]+$")
        message(FATAL_ERROR "${IG_OWNER}: not a usable target name")
    endif()
    if(TARGET "${IG_OWNER}")
        message(FATAL_ERROR "${IG_OWNER}: owner target already exists")
    endif()
    foreach(_dependency_target IN LISTS IG_DEPENDENCY_TARGETS)
        if(NOT TARGET "${_dependency_target}")
            message(FATAL_ERROR
                "${IG_OWNER}: dependency target ${_dependency_target} does not exist")
        endif()
    endforeach()
    if(NOT EXISTS "${IG_SCRIPT}" AND NOT IG_DEPENDENCY_TARGETS)
        message(FATAL_ERROR "${IG_OWNER}: no generator script ${IG_SCRIPT}")
    endif()
    if(IG_STDOUT)
        list(LENGTH IG_OUTPUTS _output_count)
        if(NOT _output_count EQUAL 1)
            message(FATAL_ERROR
                "${IG_OWNER}: STDOUT recipes require exactly one output")
        endif()
    endif()

    find_package(Python3 COMPONENTS Interpreter QUIET)
    if(NOT Python3_Interpreter_FOUND OR NOT Python3_EXECUTABLE)
        message(FATAL_ERROR
            "${IG_OWNER}: a working Python 3 interpreter is required; install "
            "python3 or set Python3_EXECUTABLE")
    endif()

    foreach(_word IN LISTS IG_ARGUMENTS)
        if("${_word}" MATCHES "[;$\\\r\n]")
            message(FATAL_ERROR "${IG_OWNER}: unsafe argument '${_word}'")
        endif()
    endforeach()
    set(_file_dependencies "")
    if(EXISTS "${IG_SCRIPT}")
        list(APPEND _file_dependencies "${IG_SCRIPT}")
    endif()
    foreach(_dependency IN LISTS IG_DEPENDS)
        if(EXISTS "${_dependency}")
            list(APPEND _file_dependencies "${_dependency}")
        elseif(NOT IG_DEPENDENCY_TARGETS)
            message(FATAL_ERROR
                "${IG_OWNER}: prerequisite ${_dependency} does not exist")
        endif()
    endforeach()
    if(IG_WORKING_DIRECTORY
            AND NOT IS_DIRECTORY "${IG_WORKING_DIRECTORY}"
            AND NOT IG_DEPENDENCY_TARGETS)
        message(FATAL_ERROR
            "${IG_OWNER}: working directory ${IG_WORKING_DIRECTORY} does not exist")
    endif()

    # Every output must land in the build tree, and no two generators may claim
    # the same file.
    set(_directories "")
    foreach(_output IN LISTS IG_OUTPUTS)
        cmake_path(IS_PREFIX CMAKE_BINARY_DIR "${_output}" NORMALIZE _owned)
        if(NOT _owned)
            message(FATAL_ERROR
                "${IG_OWNER}: output ${_output} is outside the build tree")
        endif()
        string(SHA256 _output_key "${_output}")
        get_property(_previous GLOBAL PROPERTY
            "AROS_PYTHON_OUTPUT_OWNER_${_output_key}")
        if(_previous)
            message(FATAL_ERROR
                "${IG_OWNER}: ${_output} is already owned by ${_previous}")
        endif()
        get_filename_component(_directory "${_output}" DIRECTORY)
        if(NOT _directory IN_LIST _directories)
            list(APPEND _directories "${_directory}")
        endif()
    endforeach()

    if(IG_STDOUT)
        set(_runner_arguments
            "-DRUN_OWNER=${IG_OWNER}"
            "-DRUN_PYTHON=${Python3_EXECUTABLE}"
            "-DRUN_SCRIPT=${IG_SCRIPT}"
            "-DRUN_OUTPUT=${IG_OUTPUTS}"
            "-DRUN_BUILD_ROOT=${CMAKE_BINARY_DIR}")
        if(IG_WORKING_DIRECTORY)
            list(APPEND _runner_arguments
                "-DRUN_WORKING_DIRECTORY=${IG_WORKING_DIRECTORY}")
        endif()
        list(LENGTH IG_ARGUMENTS _argument_count)
        list(APPEND _runner_arguments "-DRUN_ARGUMENT_COUNT=${_argument_count}")
        set(_argument_index 0)
        foreach(_argument IN LISTS IG_ARGUMENTS)
            list(APPEND _runner_arguments
                "-DRUN_ARGUMENT_${_argument_index}=${_argument}")
            math(EXPR _argument_index "${_argument_index} + 1")
        endforeach()
        add_custom_command(
            OUTPUT ${IG_OUTPUTS}
            COMMAND "${CMAKE_COMMAND}" ${_runner_arguments}
                -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunPythonStdout.cmake"
            DEPENDS ${_file_dependencies} ${IG_DEPENDENCY_TARGETS}
            COMMENT "Generating ${IG_OWNER} with ${IG_SCRIPT}"
            VERBATIM)
    else()
        if(IG_WORKING_DIRECTORY)
            set(_direct_working_directory "${IG_WORKING_DIRECTORY}")
        else()
            set(_direct_working_directory "${CMAKE_CURRENT_BINARY_DIR}")
        endif()
        add_custom_command(
            OUTPUT ${IG_OUTPUTS}
            COMMAND "${CMAKE_COMMAND}" -E make_directory ${_directories}
            COMMAND "${Python3_EXECUTABLE}" "${IG_SCRIPT}" ${IG_ARGUMENTS}
            DEPENDS ${_file_dependencies} ${IG_DEPENDENCY_TARGETS}
            WORKING_DIRECTORY "${_direct_working_directory}"
            COMMENT "Generating ${IG_OWNER} with ${IG_SCRIPT}"
            VERBATIM)
    endif()

    foreach(_output IN LISTS IG_OUTPUTS)
        string(SHA256 _output_key "${_output}")
        set_property(GLOBAL PROPERTY
            "AROS_PYTHON_OUTPUT_OWNER_${_output_key}" "${IG_OWNER}")
    endforeach()

    add_custom_target("${IG_OWNER}" DEPENDS ${IG_OUTPUTS})
    set_property(TARGET "${IG_OWNER}" PROPERTY AROS_PYTHON_OUTPUT_OWNER TRUE)
    set_property(TARGET "${IG_OWNER}" PROPERTY AROS_PYTHON_OUTPUTS "${IG_OUTPUTS}")

    if(IG_CONSUMERS)
        aros_bind_python_output_consumers(
            OWNER "${IG_OWNER}" CONSUMERS ${IG_CONSUMERS})
    endif()
endfunction()
