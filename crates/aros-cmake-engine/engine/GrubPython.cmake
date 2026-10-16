# Shared, closed host-Python selection for the GRUB builder and build runner.
# Selecting a compatible fallback is not itself release qualification.
include_guard(GLOBAL)

function(_aros_grub_python_require_file path)
    if(NOT IS_ABSOLUTE "${path}")
        message(FATAL_ERROR "GRUB Python path must be absolute")
    endif()
    foreach(_needle IN ITEMS ";" "\"" "\n" "\r" "$" "[" "]" "\\")
        string(FIND "${path}" "${_needle}" _position)
        if(NOT _position EQUAL -1)
            message(FATAL_ERROR "GRUB Python path contains an unsafe value")
        endif()
    endforeach()
    execute_process(COMMAND /bin/test -f "${path}" RESULT_VARIABLE _regular)
    execute_process(COMMAND /bin/test -x "${path}" RESULT_VARIABLE _executable)
    if(NOT "${_regular}" STREQUAL "0" OR NOT "${_executable}" STREQUAL "0")
        message(FATAL_ERROR "GRUB Python is not a regular executable: ${path}")
    endif()
endfunction()

# Explicit arguments allow isolated probes without modifying global installs.
# Production callers use only the independently shared fixed policy below.
function(_aros_grub_python_choose preferred fallbacks output_path output_selection)
    if(EXISTS "${preferred}" OR IS_SYMLINK "${preferred}")
        _aros_grub_python_require_file("${preferred}")
        set(_selected "${preferred}")
        set(_selection preferred)
    else()
        set(_selected "")
        foreach(_candidate IN LISTS fallbacks)
            if(EXISTS "${_candidate}" OR IS_SYMLINK "${_candidate}")
                _aros_grub_python_require_file("${_candidate}")
                set(_selected "${_candidate}")
                break()
            endif()
        endforeach()
        if(_selected STREQUAL "")
            message(FATAL_ERROR "GRUB requires Python 3.14 or an installed compatible python3")
        endif()
        set(_selection fallback)
    endif()
    set(${output_path} "${_selected}" PARENT_SCOPE)
    set(${output_selection} "${_selection}" PARENT_SCOPE)
endfunction()

function(_aros_grub_python_select output_path output_selection)
    # Never inherit PATH, Python3_EXECUTABLE or a generated contract as discovery
    # authority. An unusable installed candidate fails; it is not skipped.
    _aros_grub_python_choose("/opt/homebrew/opt/python@3.14/bin/python3.14"
        "/opt/homebrew/bin/python3;/usr/bin/python3" _path _selection)
    set(${output_path} "${_path}" PARENT_SCOPE)
    set(${output_selection} "${_selection}" PARENT_SCOPE)
endfunction()

function(_aros_grub_python_identity path output_real output_sha256)
    _aros_grub_python_require_file("${path}")
    file(REAL_PATH "${path}" _real)
    _aros_grub_python_require_file("${_real}")
    file(SHA256 "${_real}" _sha256)
    set(${output_real} "${_real}" PARENT_SCOPE)
    set(${output_sha256} "${_sha256}" PARENT_SCOPE)
endfunction()

function(_aros_grub_python_probe path output_version output_real output_sha256)
    _aros_grub_python_identity("${path}" _real_before _sha_before)
    # This is the supported host-runtime floor, not a claim that a version probe
    # qualifies every GRUB product. -I/-S isolate the probe from Python user code.
    execute_process(COMMAND "${path}" -I -S -c
        "import codecs, optparse, re, sys; assert sys.version_info.major == 3 and sys.version_info >= (3, 11) and sys.version_info.releaselevel == 'final'; print('%d.%d.%d' % sys.version_info[:3])"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _version ERROR_VARIABLE _stderr
        TIMEOUT 10)
    string(STRIP "${_version}" _version)
    if(NOT "${_result}" STREQUAL "0" OR
       NOT _version MATCHES "^3\\.[0-9]+\\.[0-9]+$" OR _version VERSION_LESS "3.11")
        message(FATAL_ERROR "GRUB Python probe failed; a stable compatible Python >= 3.11 is required: ${path}")
    endif()
    _aros_grub_python_identity("${path}" _real_after _sha_after)
    if(NOT _real_before STREQUAL _real_after OR NOT _sha_before STREQUAL _sha_after)
        message(FATAL_ERROR "GRUB Python changed during its version probe; rerun CMake")
    endif()
    set(${output_version} "${_version}" PARENT_SCOPE)
    set(${output_real} "${_real_after}" PARENT_SCOPE)
    set(${output_sha256} "${_sha_after}" PARENT_SCOPE)
endfunction()

function(_aros_grub_python_check_selection_version selection version)
    if(selection STREQUAL "preferred")
        if(NOT version MATCHES "^3\\.14\\.[0-9]+$")
            message(FATAL_ERROR "GRUB preferred Python path does not provide Python 3.14")
        endif()
    elseif(NOT selection STREQUAL "fallback")
        message(FATAL_ERROR "GRUB Python selection kind is invalid")
    endif()
endfunction()

function(_aros_grub_python_revalidate path selection recorded_path recorded_selection
        recorded_real recorded_version recorded_sha256)
    _aros_grub_python_identity("${path}" _real _sha256)
    if(NOT recorded_path STREQUAL path OR NOT recorded_selection STREQUAL selection OR
       NOT recorded_real STREQUAL _real OR NOT recorded_sha256 STREQUAL _sha256)
        message(FATAL_ERROR "GRUB2 runner Python selection changed or was substituted; rerun CMake")
    endif()
    # Refuse changed bytes before executing the interpreter again.
    _aros_grub_python_probe("${path}" _version _real _sha256)
    _aros_grub_python_check_selection_version("${selection}" "${_version}")
    if(NOT recorded_version STREQUAL _version)
        message(FATAL_ERROR "GRUB2 runner Python version changed; rerun CMake")
    endif()
endfunction()

function(_aros_grub_python_notice path version selection)
    if(selection STREQUAL "fallback")
        message(WARNING
            "GRUB: preferred Python 3.14 is not installed; using ${path} (${version}). "
            "This fallback is recorded, but does not by itself qualify release outputs.")
    endif()
endfunction()
