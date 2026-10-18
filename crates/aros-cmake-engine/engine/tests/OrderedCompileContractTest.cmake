cmake_minimum_required(VERSION 3.22)
find_program(_clang NAMES clang REQUIRED)
find_program(_clangxx NAMES clang++ REQUIRED)
find_program(_ninja NAMES ninja REQUIRED)
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "$ENV{TMPDIR}")
if(NOT _root)
    set(_root /tmp)
endif()
set(_root "${_root}/aros ordered compile ${_suffix}")

function(_check_flag_sequence build target expected)
    file(READ "${build}/compile_commands.json" _json)
    string(JSON _count LENGTH "${_json}")
    math(EXPR _last "${_count} - 1")
    set(_matched 0)
    foreach(_index RANGE 0 ${_last})
        string(JSON _command GET "${_json}" ${_index} command)
        if(NOT _command MATCHES "CMakeFiles/${target}[.]dir/")
            continue()
        endif()
        math(EXPR _matched "${_matched} + 1")
        separate_arguments(_argv UNIX_COMMAND "${_command}")
        set(_observed "")
        foreach(_argument IN LISTS _argv)
            if(_argument STREQUAL "-DORDER=1" OR _argument STREQUAL "-UORDER")
                list(APPEND _observed "${_argument}")
            endif()
        endforeach()
        list(JOIN _observed " " _observed)
        if(NOT _observed STREQUAL expected)
            message(FATAL_ERROR "${target} emitted wrong flag order: ${_observed}")
        endif()
    endforeach()
    if(NOT _matched EQUAL 1)
        message(FATAL_ERROR "${target} needs exactly one compile command, found ${_matched}")
    endif()
endfunction()

foreach(_case IN ITEMS valid raw-counter unknown-target unknown-language
        empty no-arguments semicolon expression response-file duplicate-language
        all-then-c c-then-all)
    set(_build "${_root}/${_case}")
    execute_process(COMMAND "${CMAKE_COMMAND}"
        -S "${CMAKE_CURRENT_LIST_DIR}/ordered-compile-contract" -B "${_build}"
        -G Ninja "-DCASE=${_case}"
        "-DCMAKE_C_COMPILER:FILEPATH=${_clang}"
        "-DCMAKE_CXX_COMPILER:FILEPATH=${_clangxx}"
        "-DCMAKE_MAKE_PROGRAM:FILEPATH=${_ninja}"
        -DCMAKE_EXPORT_COMPILE_COMMANDS=ON
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr
        TIMEOUT 30)
    if(_case STREQUAL "valid" OR _case STREQUAL "raw-counter")
        if(NOT "${_result}" STREQUAL "0")
            message(FATAL_ERROR "${_case} configure failed: ${_stdout}${_stderr}")
        endif()
        if(_case STREQUAL "valid")
            set(_build_targets ordered_c ordered_cxx ordered_all ordered_all_cxx)
            foreach(_target IN LISTS _build_targets)
                _check_flag_sequence("${_build}" "${_target}"
                    "-DORDER=1 -UORDER -DORDER=1")
            endforeach()
        else()
            # A missing option set must not masquerade as the expected
            # de-duplication failure: inspect the command as well as the error.
            _check_flag_sequence("${_build}" ordered_c "-DORDER=1 -UORDER")
            set(_build_targets ordered_c)
        endif()
        execute_process(COMMAND "${CMAKE_COMMAND}" --build "${_build}"
            --target ${_build_targets} --parallel 1
            RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr
            TIMEOUT 30)
        if(_case STREQUAL "valid")
            if(NOT "${_result}" STREQUAL "0")
                message(FATAL_ERROR "ordered compile failed: ${_stdout}${_stderr}")
            endif()
        elseif("${_result}" STREQUAL "0" OR NOT "${_stdout}${_stderr}" MATCHES
                "ordered contract lost final definition")
            message(FATAL_ERROR "raw duplicate counterprobe failed incorrectly: ${_stdout}${_stderr}")
        endif()
    else()
        if(_case STREQUAL "unknown-target")
            set(_expected "requires an existing target")
        elseif(_case STREQUAL "unknown-language")
            set(_expected "language must be ALL, C or CXX")
        elseif(_case STREQUAL "no-arguments")
            set(_expected "requires nonempty arguments")
        elseif(_case STREQUAL "duplicate-language" OR _case STREQUAL "all-then-c" OR
               _case STREQUAL "c-then-all")
            set(_expected "must not overlap languages")
        else()
            set(_expected "contains an unsafe argument")
        endif()
        if("${_result}" STREQUAL "0" OR NOT "${_stdout}${_stderr}" MATCHES "${_expected}")
            message(FATAL_ERROR "${_case} was not rejected correctly: ${_stdout}${_stderr}")
        endif()
    endif()
endforeach()
message(STATUS "Ordered C/CXX contracts and duplicate/unsafe-input counterprobes passed; retained ${_root}")
