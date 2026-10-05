if(NOT DEFINED FETCH_ROOT OR "${FETCH_ROOT}" STREQUAL "")
    message(FATAL_ERROR "CreateInputs requires FETCH_ROOT")
endif()
file(MAKE_DIRECTORY
    "${FETCH_ROOT}/include/config"
    "${FETCH_ROOT}/builds/unix")

set(_header "#define FIRST original\r\n#cmakedefine FT_CONFIG_OPTION_USE_PNG\r\n#cmakedefine FT_CONFIG_OPTION_USE_BZIP2\r\nkeep=CRLF\r\nliteral=semi;$value")
file(WRITE "${FETCH_ROOT}/include/config/ftoption.h.in" "${_header}")

set(_helper "cross=@CROSS@\nsysroot=@SYSROOT@\nsemi=@SEMI@\n")
file(WRITE "${FETCH_ROOT}/builds/unix/cross-helper.in" "${_helper}")
file(WRITE "${FETCH_ROOT}/include/config/generated.in" "configured=@CONFIG@\n")
file(WRITE "${FETCH_ROOT}/include/config/source-generated.in" "source=@CONFIG@")
file(WRITE "${FETCH_ROOT}/include/config/sdk.in" "sdk=@CONFIG@\n")
string(ASCII 52 20 32 _byte_alignment_probe)
file(WRITE "${FETCH_ROOT}/include/config/aligned.in" "${_byte_alignment_probe}")
string(ASCII 48 160 10 52 20 32 65 66 10 84 65 73 76 _line_search_probe)
file(WRITE "${FETCH_ROOT}/include/config/line-search.in" "${_line_search_probe}")
file(WRITE "${FETCH_ROOT}/.complete" "fixture fetched\n")
