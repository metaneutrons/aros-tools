# =============================================================================
# Host tools
# =============================================================================
#
# Some headers are produced by programs that have to run on the build machine:
# flexcat turns a catalog description into #defines, ilbmtoc turns an IFF image
# into a C array. The historic build expects both in $(TOOLDIR) and never says
# who puts them there (config/make.cfg.in:177 for FLEXCAT), so they are built
# here.
#
# They cannot be plain add_executable() targets. This build cross-compiles:
# add_compile_options() has already put -target <cpu>-unknown-elf and
# -ffreestanding on everything in this directory scope, and a host tool needs
# neither. add_custom_command invokes the compiler directly and so inherits
# none of it.

set(AROS_HOST_CC "cc" CACHE STRING "C compiler for tools that run on the build machine")
set(AROS_HOST_TOOL_DIR "${CMAKE_BINARY_DIR}/hosttools")
file(MAKE_DIRECTORY "${AROS_HOST_TOOL_DIR}")

# aros_host_tool(NAME <name> SOURCES <file>... [DEPENDS <file>...]
#                [DEFINES <d>...]
#                [INCLUDES <dir>...] [LIBS <l>...]
#                [RAW_CFLAGS <flag>...] [RAW_LDFLAGS <flag>...])
#
# Builds one host executable in a single compiler call and exports its path as
# AROS_HOST_<NAME> in the caller's scope. Recompiles when a source changes;
# DEPENDS is for headers and other non-compilation inputs which must rebuild the
# executable without being passed to the compiler as translation units.
function(aros_host_tool)
    set(oneValueArgs NAME)
    set(multiValueArgs SOURCES DEPENDS DEFINES INCLUDES LIBS RAW_CFLAGS RAW_LDFLAGS)
    cmake_parse_arguments(HT "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(NOT HT_NAME OR NOT HT_SOURCES)
        message(FATAL_ERROR "aros_host_tool: NAME and SOURCES are required")
    endif()

    set(_exe "${AROS_HOST_TOOL_DIR}/${HT_NAME}")

    set(_flags "")
    foreach(d IN LISTS HT_DEFINES)
        list(APPEND _flags "-D${d}")
    endforeach()
    foreach(i IN LISTS HT_INCLUDES)
        list(APPEND _flags "-I${i}")
    endforeach()
    set(_libs "")
    foreach(l IN LISTS HT_LIBS)
        list(APPEND _libs "-l${l}")
    endforeach()

    # RAW_CFLAGS / RAW_LDFLAGS carry what a discovery step produced verbatim --
    # pkg-config's -I/-L/-l set for libpng, for instance, which cannot be
    # reduced to a bare library name because the header and the library live
    # outside the default search paths on a Homebrew host.
    add_custom_command(
        OUTPUT "${_exe}"
        COMMAND "${AROS_HOST_CC}" -O2 -w ${_flags} ${HT_RAW_CFLAGS} ${HT_SOURCES}
                ${_libs} ${HT_RAW_LDFLAGS} -o "${_exe}"
        DEPENDS ${HT_SOURCES} ${HT_DEPENDS}
        COMMENT "Building host tool ${HT_NAME}"
        VERBATIM)

    string(TOUPPER "${HT_NAME}" _upper)
    set(AROS_HOST_${_upper} "${_exe}" PARENT_SCOPE)
endfunction()

# -----------------------------------------------------------------------------
# flexcat
# -----------------------------------------------------------------------------
#
# Source selection follows what the tree provides for non-Amiga hosts:
# locale_other.c replaces locale.c, openlibs.c opens Amiga libraries and is not
# needed, vastubs.c refuses to compile off m68k, and getft.c is replaced by
# cmake/hosttools/flexcat_getft.c.
file(GLOB _flexcat_all "${CMAKE_SOURCE_DIR}/tools/flexcat/src/*.c")
set(_flexcat_srcs "")
foreach(f IN LISTS _flexcat_all)
    get_filename_component(_n "${f}" NAME)
    if(NOT _n MATCHES "^(locale|openlibs|vastubs|getft)\\.c$")
        list(APPEND _flexcat_srcs "${f}")
    endif()
endforeach()
list(APPEND _flexcat_srcs "${CMAKE_SOURCE_DIR}/cmake/hosttools/flexcat_getft.c")

# FlexCat is a host executable, so probe iconv with exactly the compiler that
# will build it rather than using CMake's target-side checks.  glibc supplies
# iconv from libc, while Apple's SDK requires an explicit -liconv.  Trying the
# unadorned host link first also keeps standalone libiconv hosts supported.
set(_flexcat_iconv_probe_source
    "${AROS_HOST_TOOL_DIR}/flexcat-iconv-probe.c")
set(_flexcat_iconv_probe_binary
    "${AROS_HOST_TOOL_DIR}/flexcat-iconv-probe")
file(WRITE "${_flexcat_iconv_probe_source}" [=[
#include <iconv.h>

int main(void)
{
    return iconv_open("UTF-8", "UTF-8") == (iconv_t)-1;
}
]=])
execute_process(
    COMMAND "${AROS_HOST_CC}" "${_flexcat_iconv_probe_source}"
            -o "${_flexcat_iconv_probe_binary}"
    RESULT_VARIABLE _flexcat_iconv_without_library_result
    ERROR_VARIABLE _flexcat_iconv_without_library_error
    OUTPUT_QUIET)
set(_flexcat_iconv_ldflags "")
if(NOT _flexcat_iconv_without_library_result EQUAL 0)
    execute_process(
        COMMAND "${AROS_HOST_CC}" "${_flexcat_iconv_probe_source}"
                -liconv -o "${_flexcat_iconv_probe_binary}"
        RESULT_VARIABLE _flexcat_iconv_with_library_result
        ERROR_VARIABLE _flexcat_iconv_with_library_error
        OUTPUT_QUIET)
    if(NOT _flexcat_iconv_with_library_result EQUAL 0)
        message(FATAL_ERROR
            "AROS-NX: host FlexCat requires iconv, but ${AROS_HOST_CC} could "
            "not link it either from the default host runtime or with -liconv.\n"
            "Without -liconv:\n${_flexcat_iconv_without_library_error}\n"
            "With -liconv:\n${_flexcat_iconv_with_library_error}")
    endif()
    set(_flexcat_iconv_ldflags -liconv)
endif()

aros_host_tool(NAME flexcat
    SOURCES ${_flexcat_srcs}
    DEFINES _GNU_SOURCE NO_INLINE_STDARG
    INCLUDES "${CMAKE_SOURCE_DIR}/tools/flexcat/src"
    RAW_LDFLAGS ${_flexcat_iconv_ldflags})

# aros_build_catalogs(
#     MMAKE_ID <id> NAME <catalog-name> SUBDIR <installed-subdirectory>
#     DIRECTORY <declaring-directory> SOURCE_DIR <cd/ct-directory>
#     DESTINATION <catalog-root> DESCRIPTION <cd-basename-or-path>
#     SOURCE_DESCRIPTION <sd-basename-or-path>
#     LANGUAGES <language>... [SOURCE <generated-source-path>]
#     [CONSUMERS <compiled-target>...])
#
# One real output is declared for every translated catalog. SOURCE is optional;
# when relative, it is rooted below the declaring directory's generated-tree
# mirror. CONSUMERS are exact compiled targets whose source files include that
# rehomed generated source/header.
function(aros_build_catalogs)
    set(oneValueArgs
        MMAKE_ID NAME SUBDIR DIRECTORY SOURCE_DIR DESTINATION DESCRIPTION
        SOURCE SOURCE_DESCRIPTION)
    set(multiValueArgs LANGUAGES CONSUMERS)
    cmake_parse_arguments(CAT "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    if(CAT_UNPARSED_ARGUMENTS OR CAT_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_build_catalogs: malformed arguments: "
            "${CAT_UNPARSED_ARGUMENTS}${CAT_KEYWORDS_MISSING_VALUES}")
    endif()
    foreach(_required
            MMAKE_ID NAME SUBDIR DIRECTORY SOURCE_DIR DESTINATION DESCRIPTION
            SOURCE_DESCRIPTION)
        if(NOT CAT_${_required})
            message(FATAL_ERROR
                "aros_build_catalogs: ${_required} is required")
        endif()
    endforeach()
    if(NOT CAT_LANGUAGES)
        message(FATAL_ERROR
            "aros_build_catalogs: LANGUAGES must contain at least one language")
    endif()
    if(CAT_CONSUMERS AND (NOT DEFINED CAT_SOURCE OR CAT_SOURCE STREQUAL ""))
        message(FATAL_ERROR
            "aros_build_catalogs: CONSUMERS requires SOURCE")
    endif()
    if(CAT_NAME MATCHES "[/\\\\]" OR CAT_NAME STREQUAL "." OR CAT_NAME STREQUAL "..")
        message(FATAL_ERROR
            "aros_build_catalogs: NAME must be a catalog basename: ${CAT_NAME}")
    endif()
    string(REPLACE "\\" "/" _catalog_subdir "${CAT_SUBDIR}")
    if(IS_ABSOLUTE "${_catalog_subdir}"
            OR _catalog_subdir MATCHES "(^|/)\\.\\.(/|$)")
        message(FATAL_ERROR
            "aros_build_catalogs: SUBDIR must be a contained relative path: ${CAT_SUBDIR}")
    endif()

    foreach(_path_var DIRECTORY SOURCE_DIR)
        if(IS_ABSOLUTE "${CAT_${_path_var}}")
            set(_${_path_var} "${CAT_${_path_var}}")
        else()
            set(_${_path_var} "${CMAKE_SOURCE_DIR}/${CAT_${_path_var}}")
        endif()
        cmake_path(NORMAL_PATH _${_path_var})
    endforeach()
    if(IS_ABSOLUTE "${CAT_DESTINATION}")
        set(_destination "${CAT_DESTINATION}")
    else()
        set(_destination "${CMAKE_BINARY_DIR}/${CAT_DESTINATION}")
    endif()
    cmake_path(NORMAL_PATH _destination)

    set(_description "${CAT_DESCRIPTION}")
    if(NOT _description MATCHES "\\.cd$")
        string(APPEND _description ".cd")
    endif()
    if(NOT IS_ABSOLUTE "${_description}")
        set(_description "${_SOURCE_DIR}/${_description}")
    endif()
    cmake_path(NORMAL_PATH _description)

    set(_source_description "${CAT_SOURCE_DESCRIPTION}")
    if(NOT _source_description MATCHES "\\.sd$")
        string(APPEND _source_description ".sd")
    endif()
    if(NOT IS_ABSOLUTE "${_source_description}")
        set(_source_description "${_SOURCE_DIR}/${_source_description}")
    endif()
    cmake_path(NORMAL_PATH _source_description)
    # Historic builds install FlexCat source descriptions next to the host
    # executable. This CMake build uses them in place from the source tree.
    if(NOT EXISTS "${_source_description}")
        get_filename_component(_sd_name "${_source_description}" NAME)
        set(_bundled_sd
            "${CMAKE_SOURCE_DIR}/tools/flexcat/src/sd/${_sd_name}")
        if(EXISTS "${_bundled_sd}")
            set(_source_description "${_bundled_sd}")
        endif()
    endif()

    if(NOT TARGET "${CAT_MMAKE_ID}")
        add_custom_target("${CAT_MMAKE_ID}")
        aros_gate_arch("${CAT_MMAKE_ID}" "${_DIRECTORY}")
    endif()

    set(_outputs "")
    foreach(_language IN LISTS CAT_LANGUAGES)
        if(_language STREQUAL "" OR _language MATCHES "[/\\\\;]"
                OR _language STREQUAL "." OR _language STREQUAL "..")
            message(FATAL_ERROR
                "aros_build_catalogs: invalid language '${_language}'")
        endif()
        set(_translation "${_SOURCE_DIR}/${_language}.ct")
        set(_output
            "${_destination}/${_language}/${CAT_SUBDIR}/${CAT_NAME}.catalog")
        cmake_path(NORMAL_PATH _translation)
        cmake_path(NORMAL_PATH _output)
        cmake_path(IS_PREFIX _destination "${_output}" NORMALIZE
            _output_is_contained)
        if(NOT _output_is_contained OR _output STREQUAL _destination)
            message(FATAL_ERROR
                "aros_build_catalogs: catalog output escapes DESTINATION: ${_output}")
        endif()
        get_filename_component(_output_dir "${_output}" DIRECTORY)

        set(_conversion "")
        if(_language STREQUAL "polish")
            set(_conversion "iso88592toamigapl")
        elseif(_language STREQUAL "russian")
            set(_conversion "win1251toamiga1251")
        endif()

        string(SHA256 _output_hash "${_output}")
        set(_claim_property "AROS_CATALOG_OUTPUT_CLAIM_${_output_hash}")
        string(JOIN "|" _signature
            "${_description}" "${_translation}" "${_conversion}")
        get_property(_claimed GLOBAL PROPERTY "${_claim_property}" SET)
        if(_claimed)
            get_property(_first_signature GLOBAL PROPERTY "${_claim_property}")
            if(NOT "${_first_signature}" STREQUAL "${_signature}")
                message(FATAL_ERROR
                    "aros_build_catalogs: ${_output} has conflicting producers")
            endif()
        else()
            set_property(GLOBAL PROPERTY "${_claim_property}" "${_signature}")
            add_custom_command(
                OUTPUT "${_output}"
                COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
                COMMAND "${CMAKE_COMMAND}"
                    "-DTOOL=${AROS_HOST_FLEXCAT}"
                    "-DCONVERSION=${_conversion}"
                    "-DDESCRIPTION=${_description}"
                    "-DTRANSLATION=${_translation}"
                    "-DOUTPUT=${_output}"
                    -P "${CMAKE_SOURCE_DIR}/cmake/RunFlexCat.cmake"
                DEPENDS
                    "${AROS_HOST_FLEXCAT}" "${_description}" "${_translation}"
                    "${CMAKE_SOURCE_DIR}/cmake/RunFlexCat.cmake"
                COMMENT "Creating ${CAT_NAME} catalog for ${_language}"
                VERBATIM)
            set_property(GLOBAL APPEND PROPERTY AROS_CATALOG_OUTPUTS "${_output}")
        endif()
        list(APPEND _outputs "${_output}")
    endforeach()

    if(DEFINED CAT_SOURCE AND NOT CAT_SOURCE STREQUAL "")
        if(IS_ABSOLUTE "${CAT_SOURCE}")
            set(_source_output "${CAT_SOURCE}")
        else()
            set(_generated_root "${CMAKE_BINARY_DIR}/gen")
            cmake_path(NORMAL_PATH _generated_root)
            file(RELATIVE_PATH _declaring_rel
                "${CMAKE_SOURCE_DIR}" "${_DIRECTORY}")
            if(_declaring_rel MATCHES "^\\.\\.")
                message(FATAL_ERROR
                    "aros_build_catalogs: relative SOURCE requires DIRECTORY below the source tree")
            endif()
            set(_source_output
                "${_generated_root}/${_declaring_rel}/${CAT_SOURCE}")
        endif()
        cmake_path(NORMAL_PATH _source_output)
        if(NOT IS_ABSOLUTE "${CAT_SOURCE}")
            cmake_path(IS_PREFIX _generated_root "${_source_output}" NORMALIZE
                _source_is_contained)
            if(NOT _source_is_contained OR _source_output STREQUAL _generated_root)
                message(FATAL_ERROR
                    "aros_build_catalogs: relative SOURCE escapes the generated tree: ${CAT_SOURCE}")
            endif()
        endif()
        get_filename_component(_source_output_dir "${_source_output}" DIRECTORY)

        string(SHA256 _source_hash "${_source_output}")
        set(_source_claim "AROS_CATALOG_OUTPUT_CLAIM_${_source_hash}")
        string(JOIN "|" _source_signature
            "${_description}" "${_source_description}")
        get_property(_source_claimed GLOBAL PROPERTY "${_source_claim}" SET)
        if(_source_claimed)
            get_property(_first_signature GLOBAL PROPERTY "${_source_claim}")
            if(NOT "${_first_signature}" STREQUAL "${_source_signature}")
                message(FATAL_ERROR
                    "aros_build_catalogs: ${_source_output} has conflicting producers")
            endif()
        else()
            set_property(GLOBAL PROPERTY "${_source_claim}" "${_source_signature}")
            add_custom_command(
                OUTPUT "${_source_output}"
                COMMAND "${CMAKE_COMMAND}" -E make_directory
                        "${_source_output_dir}"
                COMMAND "${AROS_HOST_FLEXCAT}" "${_description}"
                        "${_source_output}=${_source_description}"
                DEPENDS
                    "${AROS_HOST_FLEXCAT}" "${_description}"
                    "${_source_description}"
                COMMENT "Creating ${CAT_NAME} catalog source ${_source_output}"
                VERBATIM)
            set_property(GLOBAL APPEND PROPERTY AROS_CATALOG_OUTPUTS
                "${_source_output}")
        endif()
        set(_source_helper "aros-catalog-source-${_source_hash}")
        if(NOT TARGET "${_source_helper}")
            add_custom_target("${_source_helper}" DEPENDS "${_source_output}")
        endif()

        # The legacy recipe creates SOURCE beside the consumer's .c file. The
        # CMake build deliberately rehomes it beneath gen/, so make that mirror
        # visible only to the concrete targets that actually compile from the
        # same source directory. -iquote preserves the original quoted-header
        # lookup without leaking a generated strings.h into SDK/system lookup.
        list(REMOVE_DUPLICATES CAT_CONSUMERS)
        foreach(_consumer IN LISTS CAT_CONSUMERS)
            if(NOT TARGET "${_consumer}")
                message(FATAL_ERROR
                    "aros_build_catalogs: SOURCE consumer is not a target: ${_consumer}")
            endif()
            get_target_property(_consumer_type "${_consumer}" TYPE)
            if(NOT _consumer_type MATCHES
                    "^(EXECUTABLE|STATIC_LIBRARY|SHARED_LIBRARY|MODULE_LIBRARY|OBJECT_LIBRARY)$")
                message(FATAL_ERROR
                    "aros_build_catalogs: SOURCE consumer is not compilable: ${_consumer}")
            endif()
            add_dependencies("${_consumer}" "${_source_helper}")
            target_compile_options("${_consumer}" BEFORE PRIVATE
                "-iquote${_source_output_dir}")
        endforeach()
        list(APPEND _outputs "${_source_output}")
    endif()

    list(REMOVE_DUPLICATES _outputs)
    string(JOIN "|" _helper_signature
        "${CAT_MMAKE_ID}" "${_outputs}")
    string(SHA256 _helper_hash "${_helper_signature}")
    set(_helper "aros-catalog-set-${_helper_hash}")
    if(NOT TARGET "${_helper}")
        add_custom_target("${_helper}" DEPENDS ${_outputs})
    endif()
    add_dependencies("${CAT_MMAKE_ID}" "${_helper}")
endfunction()

aros_host_tool(NAME ilbmtoc
    SOURCES "${CMAKE_SOURCE_DIR}/tools/ilbmtoc/ilbmtoc.c")

# -----------------------------------------------------------------------------
# genmodule
# -----------------------------------------------------------------------------
#
# The Rust SDK scan currently emits only a subset of genmodule's products. Full
# modules additionally need their generated start/end sources, while ABI-only
# declarations need the exact config's inline/proto headers, FD and link stubs.
# Build the reference generator as a host executable so those products retain
# the semantics of config/make.tmpl without inheriting the target toolchain.
file(GLOB _genmodule_srcs CONFIGURE_DEPENDS
    "${CMAKE_SOURCE_DIR}/tools/genmodule/*.c")
file(GLOB _genmodule_headers CONFIGURE_DEPENDS
    "${CMAKE_SOURCE_DIR}/tools/genmodule/*.h")
list(SORT _genmodule_srcs)
list(SORT _genmodule_headers)

aros_host_tool(NAME genmodule
    SOURCES ${_genmodule_srcs}
    DEPENDS ${_genmodule_headers}
    INCLUDES "${CMAKE_SOURCE_DIR}/tools/genmodule")

# -----------------------------------------------------------------------------
# ilbmtoicon
# -----------------------------------------------------------------------------
#
# Turns an icon description plus a PNG into an Amiga Workbench .info file, which
# is what %build_icons produces (config/make.tmpl:3117). Unlike the other host
# tools it has external dependencies: libpng and zlib
# (tools/ilbmtoicon/Makefile:9,27).
#
# Discovery is pkg-config first, because on a Homebrew host neither png.h nor
# libpng16 is on the default search path, then CMake's FindPNG/FindZLIB modules.
# The compiler is invoked directly, so imported targets such as PNG::PNG cannot
# be passed as link flags; the module fallback converts its library variables
# to actual paths or -l arguments. Without both dependencies, icon output rules
# stay in the graph and fail with a direct diagnostic when requested.
find_package(PkgConfig QUIET)
if(PKG_CONFIG_FOUND)
    pkg_check_modules(AROS_HOST_PNG QUIET libpng)
    pkg_check_modules(AROS_HOST_ZLIB QUIET zlib)
endif()

set(_host_png_ready FALSE)
if(PKG_CONFIG_FOUND AND AROS_HOST_PNG_FOUND AND AROS_HOST_ZLIB_FOUND)
    # CFLAGS/LDFLAGS retain non-directory flags advertised by the .pc files;
    # rebuilding them from INCLUDE_DIRS/LIBRARIES alone would silently lose
    # those usage requirements.
    set(_png_cflags ${AROS_HOST_PNG_CFLAGS} ${AROS_HOST_ZLIB_CFLAGS})
    set(_png_ldflags ${AROS_HOST_PNG_LDFLAGS} ${AROS_HOST_ZLIB_LDFLAGS})
    set(_host_png_ready TRUE)
else()
    # Force module mode: a package config is allowed to expose only PNG::PNG,
    # which is meaningful to target_link_libraries() but not to our raw `cc`
    # custom command.
    find_package(ZLIB QUIET MODULE)
    find_package(PNG QUIET MODULE)
    if(PNG_FOUND AND ZLIB_FOUND)
        set(_png_cflags ${PNG_DEFINITIONS})
        foreach(d IN LISTS PNG_INCLUDE_DIRS)
            list(APPEND _png_cflags "-I${d}")
        endforeach()

        # FindPNG's list may contain absolute paths, bare system libraries
        # (notably `m` for a static libpng), or optimized/debug selectors.
        # Host tools are always built -O2, so select the non-debug entries.
        set(_png_ldflags "")
        set(_use_library TRUE)
        foreach(l IN LISTS PNG_LIBRARIES)
            if(l STREQUAL "optimized" OR l STREQUAL "general")
                set(_use_library TRUE)
            elseif(l STREQUAL "debug")
                set(_use_library FALSE)
            elseif(_use_library)
                if(IS_ABSOLUTE "${l}" OR l MATCHES "^-" OR l MATCHES "[/\\\\]")
                    list(APPEND _png_ldflags "${l}")
                else()
                    list(APPEND _png_ldflags "-l${l}")
                endif()
            endif()
        endforeach()
        set(_host_png_ready TRUE)
    endif()
endif()

if(_host_png_ready)
    list(REMOVE_DUPLICATES _png_cflags)
    list(REMOVE_DUPLICATES _png_ldflags)
    aros_host_tool(NAME ilbmtoicon
        SOURCES "${CMAKE_SOURCE_DIR}/tools/ilbmtoicon/ilbmtoicon.c"
        RAW_CFLAGS ${_png_cflags}
        RAW_LDFLAGS ${_png_ldflags})
    set(AROS_HOST_HAVE_ILBMTOICON TRUE)
else()
    set(AROS_HOST_HAVE_ILBMTOICON FALSE)
    message(STATUS
        "⏭️  AROS-NX: libpng and/or zlib not found on the build machine; "
        "unavailable icon rules will be reported after target transpilation")
endif()

# -----------------------------------------------------------------------------
# Generated header rules
# -----------------------------------------------------------------------------

# aros_catalog_header(CD <file> SD <file> OUTPUT <file>)
#
# flexcat renders a catalog description through a source description template.
# C_h_aros.sd emits the message ids as #defines, which is what compile-time
# code needs; the .catalog files themselves are a runtime concern.
function(aros_catalog_header)
    set(oneValueArgs CD SD OUTPUT)
    cmake_parse_arguments(CH "" "${oneValueArgs}" "" ${ARGN})

    get_filename_component(_dir "${CH_OUTPUT}" DIRECTORY)
    file(MAKE_DIRECTORY "${_dir}")

    add_custom_command(
        OUTPUT "${CH_OUTPUT}"
        COMMAND "${AROS_HOST_FLEXCAT}" "${CH_CD}" "${CH_OUTPUT}=${CH_SD}"
        DEPENDS "${AROS_HOST_FLEXCAT}" "${CH_CD}" "${CH_SD}"
        COMMENT "Generating ${CH_OUTPUT} from ${CH_CD}"
        VERBATIM)
endfunction()

# aros_script_header(SCRIPT <py> INPUT <file> OUTPUT <file>)
#
# For generators the tree ships as Python. rom/dos/genstrings.py builds the
# error-code index table, a format its own comment calls impossible to express
# in FlexCat.
function(aros_script_header)
    set(oneValueArgs SCRIPT INPUT OUTPUT)
    cmake_parse_arguments(SH "" "${oneValueArgs}" "" ${ARGN})

    find_package(Python3 COMPONENTS Interpreter QUIET)
    if(NOT Python3_EXECUTABLE)
        message(WARNING "python3 not found; cannot generate ${SH_OUTPUT}")
        return()
    endif()

    get_filename_component(_dir "${SH_OUTPUT}" DIRECTORY)
    file(MAKE_DIRECTORY "${_dir}")

    add_custom_command(
        OUTPUT "${SH_OUTPUT}"
        COMMAND "${Python3_EXECUTABLE}" "${SH_SCRIPT}" "${SH_INPUT}" > "${SH_OUTPUT}"
        DEPENDS "${SH_SCRIPT}" "${SH_INPUT}"
        COMMENT "Generating ${SH_OUTPUT} from ${SH_INPUT}"
        VERBATIM)
endfunction()

# aros_ilbm_header(ILBM <file> OUTPUT <file> [FLAGS <f>...])
function(aros_ilbm_header)
    set(oneValueArgs ILBM OUTPUT)
    set(multiValueArgs FLAGS)
    cmake_parse_arguments(IH "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    get_filename_component(_dir "${IH_OUTPUT}" DIRECTORY)
    file(MAKE_DIRECTORY "${_dir}")

    add_custom_command(
        OUTPUT "${IH_OUTPUT}"
        COMMAND "${AROS_HOST_ILBMTOC}" ${IH_FLAGS} "${IH_ILBM}" > "${IH_OUTPUT}"
        DEPENDS "${AROS_HOST_ILBMTOC}" "${IH_ILBM}"
        COMMENT "Generating ${IH_OUTPUT} from ${IH_ILBM}"
        VERBATIM)
endfunction()

# aros_tool_header(TOOL <target> OUTPUT <file> [WORKDIR <dir>] [DEPENDS <f>...])
#
# For a generator that writes to stdout and reads its inputs from the current
# directory rather than from arguments. workbench/libs/muimaster/buildincludes
# is built that way: it walks the class headers next to it and prints one
# combined libraries/mui.h.
function(aros_tool_header)
    set(oneValueArgs TOOL OUTPUT WORKDIR)
    set(multiValueArgs DEPENDS)
    cmake_parse_arguments(TH "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    get_filename_component(_dir "${TH_OUTPUT}" DIRECTORY)
    file(MAKE_DIRECTORY "${_dir}")

    set(_wd "${TH_WORKDIR}")
    if(NOT _wd)
        set(_wd "${CMAKE_SOURCE_DIR}")
    endif()

    add_custom_command(
        OUTPUT "${TH_OUTPUT}"
        COMMAND "${CMAKE_COMMAND}" -E chdir "${_wd}" "${TH_TOOL}" > "${TH_OUTPUT}"
        DEPENDS "${TH_TOOL}" ${TH_DEPENDS}
        COMMENT "Generating ${TH_OUTPUT}"
        VERBATIM)
endfunction()

# -----------------------------------------------------------------------------
# buildincludes: libraries/mui.h
# -----------------------------------------------------------------------------
#
# muimaster does not ship libraries/mui.h; it generates one from mui.h,
# macros.h and every class header (workbench/libs/muimaster/mmakefile.src:463).
# Missing, it was the single largest gap in the build: 215 compile failures,
# and the undeclared identifiers that follow from them.
# Built and run at configure time by GeneratedHeaders.cmake, not declared as a
# build target: its output has to exist before the first compile.
