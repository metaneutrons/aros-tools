cmake_minimum_required(VERSION 3.22)
if(NOT ENGINE_DIR OR NOT TEST_BINARY_DIR)
    message(FATAL_ERROR "ENGINE_DIR and fresh TEST_BINARY_DIR are required")
endif()

include("${ENGINE_DIR}/ModuleMacroContract.cmake")
include("${ENGINE_DIR}/ModuleKobjInputs.cmake")
file(MAKE_DIRECTORY "${TEST_BINARY_DIR}")
set(AROS_NATIVE_BUILD_CONTRACT fixture-source-contract)

set(_valid_json [=[{"defname":"mod-a","declaration_line":0,"scoped_declaration_line":12,"included_make_opts":[{"tag":null,"path":"arch/riscv/make.opts"},{"tag":"riscv","path":"arch/riscv/make.opts"}],"user_objects":{"state":"exact","raw":"$(OBJECTS)","words":["second.o","first.o","second.o"],"source":[{"path":"src/module.c","line":1},{"path":"src/module.c","line":1}]},"defname_libs":{"state":"known_empty","raw":null,"source":[]},"user_ldflags":{"state":"unresolved","raw":null,"reason":"dynamic source value","source":[]},"use_libs":{"state":"exact","raw":"foo bar foo","words":["foo","bar","foo"],"source":[{"path":"src/module.c","line":13}]},"kobj_ldflags":{"state":"exact","raw":"$(KOBJ_LDFLAGS)","words":["-Wl,--gc-sections","-Wl,--eh-frame-hdr"],"source":[{"path":"config/riscv.mk","line":5}]},"kernel_kobj_ldscript":{"state":"exact","raw":"arch/riscv/kernel.ld","words":["arch/riscv/kernel.ld"],"source":[{"path":"config/riscv.mk","line":6}]},"funcinstr_libs":{"state":"known_empty","raw":null,"source":[]},"function_instrumentation":{"state":"exact","raw":"$(TARGET_FUNCINSTR)","words":["no"],"source":[{"path":"config/riscv.mk","line":7}]}}]=])

aros_record_module_macro(OWNER mod-a FORM full)
aros_record_module_kobj_inputs(OWNER mod-a JSON "${_valid_json}")
string(JSON _projected_json SET "${_valid_json}" included_configuration_files
    "[{\"tag\":null,\"path\":\"native/config.mk\"}]")
aros_record_module_macro(OWNER mod-config FORM full)
aros_record_module_kobj_inputs(OWNER mod-config JSON "${_projected_json}")
get_property(_stored GLOBAL PROPERTY AROS_MODULE_KOBJ_INPUTS_mod-a)
if(NOT "${_stored}" STREQUAL "${_valid_json}")
    message(FATAL_ERROR "KOBJ metadata JSON was rewritten while recording")
endif()

aros_get_module_kobj_words(_objects mod-a user_objects)
list(LENGTH _objects _object_count)
if(NOT _object_count EQUAL 3)
    message(FATAL_ERROR "Exact KOBJ word count changed: ${_objects}")
endif()
list(GET _objects 0 _first)
list(GET _objects 1 _second)
list(GET _objects 2 _third)
if(NOT _first STREQUAL "second.o" OR NOT _second STREQUAL "first.o" OR
   NOT _third STREQUAL "second.o")
    message(FATAL_ERROR "Exact KOBJ order or duplicate was lost: ${_objects}")
endif()
aros_get_module_kobj_words(_libs mod-a defname_libs)
if(NOT DEFINED _libs OR NOT "${_libs}" STREQUAL "")
    message(FATAL_ERROR "Known-empty KOBJ words did not remain explicitly empty")
endif()
aros_get_module_kobj_words(_uselibs mod-a use_libs)
list(LENGTH _uselibs _uselibs_count)
if(NOT _uselibs_count EQUAL 3)
    message(FATAL_ERROR "Scoped use_libs count changed: ${_uselibs}")
endif()
list(GET _uselibs 0 _uselib_0)
list(GET _uselibs 1 _uselib_1)
list(GET _uselibs 2 _uselib_2)
if(NOT _uselib_0 STREQUAL "foo" OR NOT _uselib_1 STREQUAL "bar" OR
   NOT _uselib_2 STREQUAL "foo")
    message(FATAL_ERROR "Scoped use_libs order or duplicate was lost: ${_uselibs}")
endif()
aros_get_module_kobj_words(_global_flags mod-a kobj_ldflags)
if(NOT "${_global_flags}" STREQUAL "-Wl,--gc-sections;-Wl,--eh-frame-hdr")
    message(FATAL_ERROR "Exact KOBJ global flags changed: ${_global_flags}")
endif()
aros_get_module_kobj_words(_ldscript mod-a kernel_kobj_ldscript)
if(NOT "${_ldscript}" STREQUAL "arch/riscv/kernel.ld")
    message(FATAL_ERROR "Exact KOBJ linker script changed: ${_ldscript}")
endif()
aros_get_module_kobj_words(_funcinstr mod-a funcinstr_libs)
if(NOT DEFINED _funcinstr OR NOT "${_funcinstr}" STREQUAL "")
    message(FATAL_ERROR "Explicitly empty function-instrumentation libraries changed")
endif()
aros_get_module_kobj_words(_function_instrumentation_no mod-a function_instrumentation)
if(NOT "${_function_instrumentation_no}" STREQUAL "no")
    message(FATAL_ERROR "Source-proven negative function instrumentation changed: ${_function_instrumentation_no}")
endif()

string(JSON _yes_json SET "${_valid_json}" function_instrumentation raw "\"yes\"")
string(JSON _yes_json SET "${_yes_json}" function_instrumentation words "[\"yes\"]")
aros_record_module_macro(OWNER mod-yes FORM full)
aros_record_module_kobj_inputs(OWNER mod-yes JSON "${_yes_json}")
aros_get_module_kobj_words(_function_instrumentation_yes mod-yes function_instrumentation)
if(NOT "${_function_instrumentation_yes}" STREQUAL "yes")
    message(FATAL_ERROR "Source-proven positive function instrumentation changed: ${_function_instrumentation_yes}")
endif()

function(_expect_refusal _case _expected _calls)
    set(_script "cmake_minimum_required(VERSION 3.22)\n")
    string(APPEND _script "include([==[${ENGINE_DIR}/ModuleMacroContract.cmake]==])\n")
    string(APPEND _script "include([==[${ENGINE_DIR}/ModuleKobjInputs.cmake]==])\n")
    string(APPEND _script "set(AROS_NATIVE_BUILD_CONTRACT fixture-source-contract)\n")
    string(APPEND _script "${_calls}\n")
    set(_path "${TEST_BINARY_DIR}/${_case}.cmake")
    file(WRITE "${_path}" "${_script}")
    execute_process(COMMAND "${CMAKE_COMMAND}" -P "${_path}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _out ERROR_VARIABLE _err)
    string(REGEX REPLACE "[ \t\r\n]+" "" _diagnostic "${_out}${_err}")
    string(REPLACE " " "" _expected_compact "${_expected}")
    if(_result STREQUAL "0" OR NOT "${_diagnostic}" MATCHES "${_expected_compact}")
        message(FATAL_ERROR "${_case}: expected refusal matching '${_expected}', got ${_result}: ${_out}${_err}")
    endif()
endfunction()

function(_expect_bad_json _case _expected _json)
    set(_calls "aros_record_module_macro(OWNER ${_case} FORM full)\n")
    string(APPEND _calls "aros_record_module_kobj_inputs(OWNER ${_case} JSON [==[${_json}]==])\n")
    _expect_refusal("${_case}" "${_expected}" "${_calls}")
endfunction()

_expect_bad_json(invalid-json "invalid JSON" "{not-json")
_expect_refusal(missing-macro "no registered source"
    "aros_record_module_kobj_inputs(OWNER missing-macro JSON [==[${_valid_json}]==])")

set(_bad "${_valid_json}")
string(JSON _bad SET "${_bad}" invented "true")
_expect_bad_json(top-extra "unexpected object key|missing or unexpected object key" "${_bad}")

string(JSON _bad SET "${_valid_json}" user_objects invented "true")
_expect_bad_json(nested-extra "unexpected object key|missing or unexpected object key" "${_bad}")

string(JSON _bad REMOVE "${_valid_json}" use_libs)
_expect_bad_json(top-missing "missing object key|missing or unexpected object key" "${_bad}")

string(JSON _bad REMOVE "${_valid_json}" function_instrumentation)
_expect_bad_json(function-instrumentation-missing "missing object key|missing or unexpected object key" "${_bad}")

string(JSON _bad SET "${_valid_json}" user_objects state "\"inferred\"")
_expect_bad_json(unknown-state "unknown state" "${_bad}")

string(JSON _bad SET "${_valid_json}" declaration_line "\"zero\"")
_expect_bad_json(line-type "expected NUMBER" "${_bad}")

string(JSON _bad SET "${_valid_json}" scoped_declaration_line 1.5)
_expect_bad_json(line-fraction "zero-based non-negative integer" "${_bad}")

string(JSON _bad SET "${_valid_json}" included_make_opts "\"not-an-array\"")
_expect_bad_json(includes-type "expected ARRAY" "${_bad}")

string(JSON _bad SET "${_projected_json}" included_configuration_files "null")
_expect_bad_json(configuration-null "expected ARRAY" "${_bad}")

string(JSON _bad SET "${_projected_json}" included_configuration_files 0 path "\"../escape.mk\"")
_expect_bad_json(configuration-traversal "safe source-relative path" "${_bad}")

string(JSON _bad SET "${_valid_json}" user_objects words "\"not-an-array\"")
_expect_bad_json(words-type "expected ARRAY" "${_bad}")

string(JSON _bad SET "${_valid_json}" use_libs "\"not-an-object\"")
_expect_bad_json(use-libs-type "expected OBJECT" "${_bad}")

string(JSON _bad SET "${_valid_json}" included_make_opts 0 path "\"../escape/make.opts\"")
_expect_bad_json(makeopts-traversal "safe source-relative path" "${_bad}")

string(JSON _bad SET "${_valid_json}" user_objects source 0 path "\"src/../../escape.c\"")
_expect_bad_json(source-traversal "safe source-relative path" "${_bad}")

string(JSON _bad SET "${_valid_json}" user_objects source 0 path "\"/absolute/path.c\"")
_expect_bad_json(source-absolute "safe source-relative path" "${_bad}")

string(JSON _bad SET "${_valid_json}" included_make_opts 0 path "\"arch//make.opts\"")
_expect_bad_json(source-empty-component "safe source-relative path" "${_bad}")

string(JSON _bad SET "${_valid_json}" user_objects source 0 line 0)
_expect_bad_json(source-line-zero "positive integer" "${_bad}")

string(JSON _bad SET "${_valid_json}" user_objects words 0 "\"unsafe;word\"")
_expect_bad_json(list-unsafe "unsafe CMake list representation" "${_bad}")

# The build and source directory placeholders are expanded; no other `$` is.
string(JSON _bad SET "${_valid_json}" user_objects words 0 [=["${OTHER_DIR}/first.o"]=])
_expect_bad_json(other-placeholder "unsafe CMake list representation" "${_bad}")
string(JSON _placed_json SET "${_valid_json}" user_objects words 0 [=["${AROS_BUILD_DIR}/gen/first.o"]=])
aros_record_module_macro(OWNER mod-placed FORM full)
aros_record_module_kobj_inputs(OWNER mod-placed JSON "${_placed_json}")
_expect_refusal(placeholder-unset "which is not set"
    "aros_record_module_macro(OWNER placeholder-unset FORM full)\naros_record_module_kobj_inputs(OWNER placeholder-unset JSON [==[${_placed_json}]==])\naros_get_module_kobj_words(_objects placeholder-unset user_objects)")
set(AROS_BUILD_DIR "${TEST_BINARY_DIR}/build")
aros_get_module_kobj_words(_placed mod-placed user_objects)
list(GET _placed 0 _placed_first)
if(NOT _placed_first STREQUAL "${AROS_BUILD_DIR}/gen/first.o")
    message(FATAL_ERROR "The build directory placeholder was not expanded: ${_placed}")
endif()
unset(AROS_BUILD_DIR)

string(JSON _bad SET "${_valid_json}" user_objects words "[]")
_expect_bad_json(exact-empty-array "must be nonempty" "${_bad}")

string(JSON _bad SET "${_valid_json}" user_ldflags reason "\"\"")
_expect_bad_json(unresolved-empty-reason "nonempty reason" "${_bad}")

_expect_refusal(abi-only-owner "unsupported source module macro form"
    "aros_record_module_macro(OWNER abi-only-owner FORM abi-only)\naros_record_module_kobj_inputs(OWNER abi-only-owner JSON [==[${_valid_json}]==])")

_expect_refusal(duplicate-registration "duplicate source KOBJ inputs"
    "aros_record_module_macro(OWNER duplicate-registration FORM runtime-only)\naros_record_module_kobj_inputs(OWNER duplicate-registration JSON [==[${_valid_json}]==])\naros_record_module_kobj_inputs(OWNER duplicate-registration JSON [==[${_valid_json}]==])")

_expect_refusal(unresolved-consumption "cannot consume unresolved"
    "aros_record_module_macro(OWNER unresolved-consumption FORM simple)\naros_record_module_kobj_inputs(OWNER unresolved-consumption JSON [==[${_valid_json}]==])\naros_get_module_kobj_words(_flags unresolved-consumption user_ldflags)")

string(JSON _unresolved_json REMOVE "${_valid_json}" function_instrumentation words)
string(JSON _unresolved_json SET "${_unresolved_json}" function_instrumentation state "\"unresolved\"")
string(JSON _unresolved_json SET "${_unresolved_json}" function_instrumentation reason "\"source does not prove a value\"")
_expect_refusal(function-instrumentation-unresolved "cannot consume unresolved"
    "aros_record_module_macro(OWNER function-instrumentation-unresolved FORM full)\naros_record_module_kobj_inputs(OWNER function-instrumentation-unresolved JSON [==[${_unresolved_json}]==])\naros_get_module_kobj_words(_value function-instrumentation-unresolved function_instrumentation)")

_expect_refusal(unknown-getter-field "get requires a safe output variable"
    "aros_record_module_macro(OWNER unknown-getter-field FORM full)\naros_record_module_kobj_inputs(OWNER unknown-getter-field JSON [==[${_valid_json}]==])\naros_get_module_kobj_words(_value unknown-getter-field future_global_input)")

_expect_refusal(missing-metadata-consumption "has no recorded source KOBJ inputs"
    "aros_record_module_macro(OWNER missing-metadata-consumption FORM full)\naros_get_module_kobj_words(_objects missing-metadata-consumption user_objects)")

message(STATUS "KOBJ scoped inputs preserved all eight fields, exact source-proven function-instrumentation yes/no, order and duplicates, global flags, explicit known-empty state, and refused malformed, unresolved, unsafe, missing, or duplicate metadata")
