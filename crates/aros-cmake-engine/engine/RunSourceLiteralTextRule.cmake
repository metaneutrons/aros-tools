cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/SourceLiteralTextRules.cmake")
foreach(_required IN ITEMS BINARY_ROOT SOURCE_ROOT FILE FILE_SHA256 OUTPUT PAYLOAD PAYLOAD_SHA256)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "literal text runner requires ${_required}")
    endif()
endforeach()
_aros_source_value_canonical_root("binary root" "${BINARY_ROOT}" _binary_root)
_aros_source_value_canonical_root("source root" "${SOURCE_ROOT}" _source_root)
_aros_literal_text_source_output("${_source_root}" "${FILE}" "${FILE_SHA256}"
    "${_binary_root}" "${OUTPUT}" _source_file _output)
_aros_literal_text_payload("${_binary_root}" "${PAYLOAD}" "${PAYLOAD_SHA256}" _text)
_aros_literal_text_source_output("${_source_root}" "${FILE}" "${FILE_SHA256}"
    "${_binary_root}" "${OUTPUT}" _source_file _output)
_aros_literal_text_publish("${_output}" "${_text}" "${_binary_root}")
_aros_literal_text_source_output("${_source_root}" "${FILE}" "${FILE_SHA256}"
    "${_binary_root}" "${OUTPUT}" _source_file _output)
_aros_literal_text_payload("${_binary_root}" "${PAYLOAD}" "${PAYLOAD_SHA256}" _text_after)
# A successful rebuild must also become fresh after a newer unchanged source
# or engine input. Configuration payload writes retain their timestamps.
file(TOUCH "${_output}")
