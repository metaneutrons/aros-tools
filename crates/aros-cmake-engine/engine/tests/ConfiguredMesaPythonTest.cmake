cmake_minimum_required(VERSION 3.24)
get_filename_component(_engine "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${CMAKE_CURRENT_BINARY_DIR}/mesa python binding ${_suffix}")
set(_source "${_root}/source")
file(MAKE_DIRECTORY "${_source}")

set(_fixture_cmake [=[
cmake_minimum_required(VERSION 3.24)
project(MesaPythonBinding LANGUAGES C CXX)
include("${ENGINE}/PythonGenerators.cmake")
include("${ENGINE}/ConfiguredMesaSources.cmake")

set(AROS_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}")
add_custom_target(fixture-fetch)
set(_fetch_destination "${CMAKE_BINARY_DIR}/fetched")
set(_fetch_stamp "${_fetch_destination}/.fixture-fetched")
set_property(TARGET fixture-fetch PROPERTY
    AROS_FETCH_DESTINATION "${_fetch_destination}")
set_property(TARGET fixture-fetch PROPERTY
    AROS_FETCH_COMPLETION_STAMP "${_fetch_stamp}")

set(_source_root "${_fetch_destination}/mesa-26.0.0")
set(_producer_build_root "${CMAKE_BINARY_DIR}/gen/mesa-26.0.0")
set(_declared_build_root "${_producer_build_root}")
if(CASE STREQUAL "relative-root")
    set(_declared_build_root "mesa-26.0.0")
endif()
aros_generate_python_outputs(
    OWNER mesa-python-owner
    SOURCE_ROOT "${_source_root}"
    BUILD_ROOT "${_declared_build_root}"
    FETCH_TARGET fixture-fetch
    SOURCE_INPUTS
        src/compiler/nir/meson_builtins.py
        src/gallium/auxiliary/mesa_layout.py
    JOB
        SCRIPT src/compiler/nir/meson_builtins.py
        OUTPUT src/compiler/nir/mesa_builtins.c
        ARGUMENTS --kind c
    JOB
        SCRIPT src/gallium/auxiliary/mesa_layout.py
        OUTPUT src/gallium/auxiliary/mesa_layout.cpp
        ARGUMENTS --kind cxx)

get_target_property(_outputs mesa-python-owner AROS_PYTHON_OUTPUTS)
set(_expected_c
    "${_producer_build_root}/src/compiler/nir/mesa_builtins.c")
set(_expected_cxx
    "${_producer_build_root}/src/gallium/auxiliary/mesa_layout.cpp")
if(NOT _outputs STREQUAL "${_expected_c};${_expected_cxx}")
    message(FATAL_ERROR
        "actual Python producer did not retain both expected C/CXX outputs: ${_outputs}")
endif()
foreach(_output IN LISTS _outputs)
    if(EXISTS "${_output}")
        message(FATAL_ERROR "cold Python producer unexpectedly ran: ${_output}")
    endif()
endforeach()

set(_selected_outputs "${_outputs}")
if(CASE STREQUAL "shared-root")
    aros_generate_python_outputs(
        OWNER mesa-other-owner SOURCE_ROOT "${_source_root}"
        BUILD_ROOT "${_declared_build_root}" FETCH_TARGET fixture-fetch
        JOB SCRIPT src/other.py OUTPUT src/shared.c)
    list(APPEND _selected_outputs "${_producer_build_root}/src/shared.c")
endif()
add_library(selected OBJECT EXCLUDE_FROM_ALL ${_selected_outputs})
add_library(ordinary OBJECT EXCLUDE_FROM_ALL ${_outputs})
set_property(TARGET selected PROPERTY COMPILE_OPTIONS "-DKEEP_TARGET_OPTION=1")
set_source_files_properties("${_expected_c}" TARGET_DIRECTORY selected
    PROPERTIES COMPILE_OPTIONS "-DKEEP_C_OPTION=1")
set_source_files_properties("${_expected_cxx}" TARGET_DIRECTORY selected
    PROPERTIES COMPILE_OPTIONS "-DKEEP_CXX_OPTION=1")
get_target_property(_target_options_before selected COMPILE_OPTIONS)
get_source_file_property(_c_options_before "${_expected_c}"
    TARGET_DIRECTORY selected COMPILE_OPTIONS)
get_source_file_property(_cxx_options_before "${_expected_cxx}"
    TARGET_DIRECTORY selected COMPILE_OPTIONS)

set(_owner mesa-python-owner)
set(_make_gendir "/measured/build/bin/target/gen")
set(_make_root "${_make_gendir}/workbench/libs/mesa/26.0.0")
if(CASE STREQUAL "wrong-owner")
    add_custom_target(other-python-owner)
    set(_owner other-python-owner)
elseif(CASE STREQUAL "missing-registration")
    string(SHA256 _key "${_expected_c}")
    set_property(GLOBAL PROPERTY "AROS_PYTHON_OUTPUT_OWNER_${_key}" "")
elseif(CASE STREQUAL "invented-output")
    set(_invented "${_producer_build_root}/src/nir/invented.c")
    set_source_files_properties("${_invented}" PROPERTIES GENERATED TRUE)
    target_sources(selected PRIVATE "${_invented}")
elseif(CASE STREQUAL "changed-producer-root")
    string(SHA256 _key "${_expected_c}")
    set_property(GLOBAL PROPERTY "AROS_PYTHON_OUTPUT_BUILD_ROOT_${_key}"
        "${_producer_build_root}/changed")
elseif(CASE STREQUAL "changed-producer-relative")
    string(SHA256 _key "${_expected_cxx}")
    set_property(GLOBAL PROPERTY "AROS_PYTHON_OUTPUT_RELATIVE_${_key}"
        "src/gallium/auxiliary/other.cpp")
elseif(CASE STREQUAL "escaping-nominal-root")
    set(_make_root "${CMAKE_BINARY_DIR}/outside-measured-gendir")
endif()

if(CASE STREQUAL "rebound-context")
    _aros_bind_mesa_python_context(selected "${_owner}" "${_producer_build_root}"
        "${_make_root}" "${_make_gendir}")
    set(_make_root "${_make_gendir}/workbench/libs/mesa/26.0.1")
    _aros_bind_mesa_python_context(selected "${_owner}" "${_producer_build_root}"
        "${_make_root}" "${_make_gendir}")
else()
    _aros_bind_mesa_python_context(selected "${_owner}" "${_declared_build_root}"
        "${_make_root}" "${_make_gendir}")
    if(CASE MATCHES "^(good|relative-root|shared-root)$")
        # An identical second binding must be a no-op for both metadata and
        # source compile options.
        _aros_bind_mesa_python_context(selected "${_owner}" "${_declared_build_root}"
            "${_make_root}" "${_make_gendir}")
        if(CASE STREQUAL "shared-root")
            _aros_bind_mesa_python_context(selected mesa-other-owner "${_declared_build_root}"
                "${_make_root}" "${_make_gendir}")
        endif()

        get_target_property(_bound selected AROS_MESA_PYTHON_BOUND_SOURCES)
        if(NOT _bound STREQUAL "${_selected_outputs}")
            message(FATAL_ERROR "Python binding lost or duplicated concrete sources: ${_bound}")
        endif()
        get_target_property(_target_options_after selected COMPILE_OPTIONS)
        get_source_file_property(_c_options_after "${_expected_c}"
            TARGET_DIRECTORY selected COMPILE_OPTIONS)
        get_source_file_property(_cxx_options_after "${_expected_cxx}"
            TARGET_DIRECTORY selected COMPILE_OPTIONS)
        if(NOT "${_target_options_after}" STREQUAL "${_target_options_before}" OR
           NOT "${_c_options_after}" STREQUAL "${_c_options_before}" OR
           NOT "${_cxx_options_after}" STREQUAL "${_cxx_options_before}")
            message(FATAL_ERROR "Python binding changed COMPILE_OPTIONS")
        endif()

        foreach(_source IN LISTS _selected_outputs)
            if(_source STREQUAL _expected_c)
                set(_relative "src/compiler/nir/mesa_builtins.c")
                set(_language C)
            elseif(_source STREQUAL _expected_cxx)
                set(_relative "src/gallium/auxiliary/mesa_layout.cpp")
                set(_language CXX)
            else()
                set(_relative "src/shared.c")
                set(_language C)
            endif()
            get_source_file_property(_actual_language "${_source}"
                TARGET_DIRECTORY selected LANGUAGE)
            if(NOT _actual_language STREQUAL _language)
                message(FATAL_ERROR
                    "cold output ${_source} has language ${_actual_language}, expected ${_language}")
            endif()
            string(SHA256 _binding "selected|${_source}")
            get_property(_nominal_set GLOBAL PROPERTY
                "AROS_MESA_PYTHON_NOMINAL_${_binding}" SET)
            get_property(_nominal GLOBAL PROPERTY
                "AROS_MESA_PYTHON_NOMINAL_${_binding}")
            set(_expected_nominal "${_make_root}/${_relative}")
            if(NOT _nominal_set OR NOT _nominal STREQUAL _expected_nominal)
                message(FATAL_ERROR
                    "Python source lost exact root-relative nominal mapping: ${_source} -> ${_nominal}")
            endif()
            string(SHA256 _ordinary_binding "ordinary|${_source}")
            get_property(_ordinary_nominal_set GLOBAL PROPERTY
                "AROS_MESA_PYTHON_NOMINAL_${_ordinary_binding}" SET)
            if(_ordinary_nominal_set)
                message(FATAL_ERROR "Python nominal binding leaked to an ordinary target")
            endif()
        endforeach()
        get_property(_ordinary_bound_set TARGET ordinary
            PROPERTY AROS_MESA_PYTHON_BOUND_SOURCES SET)
        if(_ordinary_bound_set)
            message(FATAL_ERROR "Python bound-source state leaked to an ordinary target")
        endif()
    endif()
endif()
]=])
file(WRITE "${_source}/CMakeLists.txt" "${_fixture_cmake}")

foreach(_case IN ITEMS good relative-root shared-root wrong-owner missing-registration invented-output
        changed-producer-root changed-producer-relative escaping-nominal-root
        rebound-context)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -G Ninja -S "${_source}" -B "${_root}/${_case}"
            "-DENGINE=${_engine}" "-DCASE=${_case}"
        RESULT_VARIABLE _status OUTPUT_VARIABLE _out ERROR_VARIABLE _err)
    if(_case MATCHES "^(good|relative-root|shared-root)$")
        if(NOT _status EQUAL 0)
            message(FATAL_ERROR "Mesa Python binding fixture failed: ${_out}${_err}")
        endif()
        continue()
    endif()

    if(_case STREQUAL "wrong-owner" OR _case STREQUAL "missing-registration" OR
       _case STREQUAL "invented-output")
        set(_reason "different or missing producer")
    elseif(_case STREQUAL "changed-producer-root" OR
           _case STREQUAL "changed-producer-relative")
        set(_reason "inconsistent producer coordinates")
    elseif(_case STREQUAL "escaping-nominal-root")
        set(_reason "leaves measured GENDIR")
    else()
        set(_reason "changed after binding")
    endif()
    if(_status EQUAL 0 OR NOT "${_out}${_err}" MATCHES "${_reason}")
        message(FATAL_ERROR
            "Mesa Python binding ${_case} missed expected refusal '${_reason}': ${_out}${_err}")
    endif()
endforeach()
