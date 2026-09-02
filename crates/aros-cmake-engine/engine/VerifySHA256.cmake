cmake_minimum_required(VERSION 3.22)

if(NOT DEFINED FILE OR NOT DEFINED EXPECTED)
    message(FATAL_ERROR "VerifySHA256.cmake requires FILE and EXPECTED")
endif()
if(NOT EXISTS "${FILE}" OR IS_DIRECTORY "${FILE}")
    message(FATAL_ERROR "SHA-256 input does not exist: ${FILE}")
endif()
string(LENGTH "${EXPECTED}" EXPECTED_LENGTH)
if(NOT EXPECTED_LENGTH EQUAL 64 OR
   NOT EXPECTED MATCHES "^[0-9A-Fa-f]+$")
    message(FATAL_ERROR "Invalid expected SHA-256: ${EXPECTED}")
endif()

file(SHA256 "${FILE}" ACTUAL)
string(TOLOWER "${EXPECTED}" EXPECTED_LOWER)
if(NOT ACTUAL STREQUAL EXPECTED_LOWER)
    message(FATAL_ERROR
        "SHA-256 mismatch for ${FILE}: expected ${EXPECTED_LOWER}, got ${ACTUAL}")
endif()
