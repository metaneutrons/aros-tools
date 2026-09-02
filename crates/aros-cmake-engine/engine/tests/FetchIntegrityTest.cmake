cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-fetch-integrity-${_suffix}")
set(_origin "${_root}/origin")
set(_stage "${_root}/stage")
set(_archive "${_origin}/fixture.tar.gz")
if(DEFINED ENV{AROS_FETCH_BIN} AND NOT "$ENV{AROS_FETCH_BIN}" STREQUAL "")
    set(_fetch "$ENV{AROS_FETCH_BIN}")
else()
    find_program(_fetch NAMES aros-fetch)
endif()
if(NOT EXISTS "${_fetch}" OR IS_DIRECTORY "${_fetch}")
    message(FATAL_ERROR
        "required installed aros-fetch test executable is missing: ${_fetch}")
endif()

file(MAKE_DIRECTORY "${_origin}" "${_stage}/fixture-src")
file(WRITE "${_stage}/fixture-src/value.txt" "trusted payload\n")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E tar czf "${_archive}"
        --format=gnutar fixture-src
    WORKING_DIRECTORY "${_stage}"
    RESULT_VARIABLE _archive_result
    ERROR_VARIABLE _archive_error)
if(NOT _archive_result EQUAL 0)
    message(FATAL_ERROR "could not create integrity fixture: ${_archive_error}")
endif()
file(SHA256 "${_archive}" _sha256)
set(_contract "fixture.tar.gz=sha256:${_sha256}")

function(_run_fetch label result_var log_var)
    execute_process(
        COMMAND ${ARGN}
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set(${result_var} "${_result}" PARENT_SCOPE)
    set(${log_var} "${_stdout}\n${_stderr}" PARENT_SCOPE)
endfunction()

set(_cache "${_root}/cache")
set(_ports "${_root}/ports")
_run_fetch(correct _result _log
    "${_fetch}" -a fixture -s tar.gz -ao "${_origin}"
    -cs "${_contract}" -l "${_cache}" -d "${_ports}" -b "${_ports}")
if(NOT _result EQUAL 0 OR NOT EXISTS "${_ports}/fixture-src/value.txt")
    message(FATAL_ERROR "verified local fetch failed (${_result})\n${_log}")
endif()
if(NOT _log MATCHES "Verified   fixture.tar.gz \\(SHA-256\\)")
    message(FATAL_ERROR "successful fetch did not report verification\n${_log}")
endif()

# A cache hit is never trusted merely because it was fetched successfully once.
file(WRITE "${_cache}/fixture.tar.gz" "tampered\n")
set(_tampered_ports "${_root}/tampered-ports")
_run_fetch(tampered _result _log
    "${_fetch}" -a fixture -s tar.gz -ao "${_origin}"
    -cs "${_contract}" -l "${_cache}" -d "${_tampered_ports}"
    -b "${_tampered_ports}")
if(_result EQUAL 0 OR NOT _log MATCHES "AF0401" OR
   NOT _log MATCHES "SHA-256 mismatch")
    message(FATAL_ERROR "tampered cache was not rejected clearly\n${_log}")
endif()

# Every fallback candidate needs its own exact checksum.
_run_fetch(incomplete _result _log
    "${_fetch}" -a fixture -s "tar.xz tar.gz" -ao "${_origin}"
    -cs "${_contract}" -l "${_root}/incomplete-cache"
    -d "${_root}/incomplete-ports")
if(_result EQUAL 0 OR NOT _log MATCHES "AF0101" OR
   NOT _log MATCHES "does not cover archive candidate 'fixture.tar.xz'")
    message(FATAL_ERROR "incomplete multi-suffix contract was not rejected\n${_log}")
endif()

_run_fetch(malformed _result _log
    "${_fetch}" -a fixture -s tar.gz -ao "${_origin}"
    -cs "fixture.tar.gz=sha256:abc" -l "${_root}/malformed-cache"
    -d "${_root}/malformed-ports")
if(_result EQUAL 0 OR NOT _log MATCHES "invalid SHA-256 for 'fixture.tar.gz'")
    message(FATAL_ERROR "malformed digest was not rejected clearly\n${_log}")
endif()

_run_fetch(duplicate _result _log
    "${_fetch}" -a fixture -s tar.gz -ao "${_origin}"
    -cs "${_contract} ${_contract}" -l "${_root}/duplicate-cache"
    -d "${_root}/duplicate-ports")
if(_result EQUAL 0 OR NOT _log MATCHES "duplicate checksum declaration")
    message(FATAL_ERROR "duplicate digest was not rejected clearly\n${_log}")
endif()

# Offline mode may use a verified cache but must not contact a network origin.
set(_offline_cache "${_root}/offline-cache")
file(MAKE_DIRECTORY "${_offline_cache}")
file(COPY "${_archive}" DESTINATION "${_offline_cache}")
_run_fetch(offline-hit _result _log
    "${CMAKE_COMMAND}" -E env AROS_FETCH_OFFLINE=ON
    "${_fetch}" -a fixture -s tar.gz -ao "https://127.0.0.1:9"
    -cs "${_contract}" -l "${_offline_cache}"
    -d "${_root}/offline-ports" -b "${_root}/offline-ports")
if(NOT _result EQUAL 0)
    message(FATAL_ERROR "verified offline cache hit failed\n${_log}")
endif()

_run_fetch(offline-miss _result _log
    "${CMAKE_COMMAND}" -E env AROS_FETCH_OFFLINE=ON
    "${_fetch}" -a fixture -s tar.gz -ao "https://127.0.0.1:9"
    -cs "${_contract}" -l "${_root}/offline-miss-cache"
    -d "${_root}/offline-miss-ports")
if(_result EQUAL 0 OR NOT _log MATCHES "AF0201" OR
   NOT _log MATCHES "offline cache/local-origin miss")
    message(FATAL_ERROR "offline cache miss did not block the network clearly\n${_log}")
endif()

# Release/CI callers can turn the optional declaration into a hard policy.
_run_fetch(strict _result _log
    "${CMAKE_COMMAND}" -E env AROS_FETCH_REQUIRE_CHECKSUMS=ON
    "${_fetch}" -a fixture -s tar.gz -ao "${_origin}"
    -l "${_root}/strict-cache" -d "${_root}/strict-ports")
if(_result EQUAL 0 OR NOT _log MATCHES "AF0101" OR
   NOT _log MATCHES "checksum contract does not cover")
    message(FATAL_ERROR "strict mode accepted a hashless fetch\n${_log}")
endif()
