cmake_minimum_required(VERSION 3.22)
project(mesa26-runtime-fixture NONE)

foreach(_required IN ITEMS
        MESA26_RUNTIME_MODULE MESA26_RUNTIME_CASE AROS_MESA_VERSION AROS_SYS_DIR
        MESA26_LOADER_NAME MESA26_PROVIDER_NAME)
    if(NOT DEFINED ${_required})
        message(FATAL_ERROR "fixture requires ${_required}")
    endif()
endforeach()

set(_engine_cmake_list "${CMAKE_CURRENT_LIST_DIR}/../../CMakeLists.txt")
if(NOT EXISTS "${_engine_cmake_list}")
    message(FATAL_ERROR "fixture cannot find production engine CMakeLists.txt")
endif()
file(READ "${_engine_cmake_list}" _engine_cmake_source)
string(FIND "${_engine_cmake_source}"
    "include(\"\${GENERATED_TARGETS_CMAKE}\")" _generated_include_position)
string(FIND "${_engine_cmake_source}"
    "include(\"\${AROS_CMAKE_ENGINE_DIR}/Mesa26Runtime.cmake\")"
    _mesa26_include_position)
string(FIND "${_engine_cmake_source}"
    "aros_configure_mesa26_runtime()" _mesa26_call_position)
string(FIND "${_engine_cmake_source}"
    "include(\"\${AROS_CMAKE_ENGINE_DIR}/RuntimeProbes.cmake\")"
    _runtime_probes_include_position)
if(_generated_include_position LESS 0 OR _mesa26_include_position LESS 0 OR
   _mesa26_call_position LESS 0 OR _runtime_probes_include_position LESS 0 OR
   NOT _generated_include_position LESS _mesa26_include_position OR
   NOT _mesa26_include_position LESS _mesa26_call_position OR
   NOT _mesa26_call_position LESS _runtime_probes_include_position)
    message(FATAL_ERROR
        "production engine must include generated targets, then Mesa26Runtime, "
        "call aros_configure_mesa26_runtime(), and only then include RuntimeProbes")
endif()

add_custom_target(workbench-libs-gl)
set_target_properties(workbench-libs-gl PROPERTIES OUTPUT_NAME "${MESA26_LOADER_NAME}")
if(NOT MESA26_RUNTIME_CASE STREQUAL "missing-provider" AND
   NOT MESA26_RUNTIME_CASE STREQUAL "consumer-missing-provider")
    add_custom_target(mesa3dgl-library)
    set_target_properties(mesa3dgl-library PROPERTIES OUTPUT_NAME "${MESA26_PROVIDER_NAME}")
endif()

set(_engine_dir "${CMAKE_CURRENT_LIST_DIR}/../..")
include("${_engine_dir}/NativeConsumerContract.cmake")

if(MESA26_RUNTIME_CASE MATCHES "^consumer-")
    # Model the exact globally sealed state established by a successful
    # source-binding validation. The contract is deliberately in the fixture
    # binary tree so this probe never writes into the source fixture.
    file(REAL_PATH "${CMAKE_SOURCE_DIR}" AROS_SOURCE_DIR)
    set(_contract "${CMAKE_BINARY_DIR}/native-consumer-contract.json")
    file(WRITE "${_contract}" "fixture native consumer contract\n")
    file(REAL_PATH "${_contract}" AROS_NATIVE_CONSUMER_CONTRACT)
    file(SHA256 "${AROS_NATIVE_CONSUMER_CONTRACT}" AROS_NATIVE_CONSUMER_CONTRACT_SHA256)
    set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED TRUE)
    set(AROS_NATIVE_CONSUMER_PROFILE "consumer-fixture")
    set(AROS_NATIVE_CONSUMER_EXEC_SMP ON)
    set(AROS_NATIVE_CONSUMER_SDK_INCLUDE_RELATIVE "SYS/Developer/include")
    set(AROS_TARGET_PROFILE "consumer-fixture")
    set(AROS_TARGET_CPU "riscv64")
    set(AROS_TARGET_PLATFORM "generic")
    set(AROS_TARGET_TRIPLE "riscv64-unknown-aros")
    set(AROS_TARGET_FAMILY "")
    set(AROS_TARGET_VARIANT "")
    set(AROS_TOOLCHAIN "gnu")
    set(AROS_TARGET_CPU32 "")
    set(GCC_CONFIG_FLOAT_ABI "lp64d")
    set(AROS_ABI_FLAVOUR "generic")
    set(AROS_ABI_PLATFORM_SMP ON)
    set(AROS_ENABLE_MMU ON)
    set(AROS_MESA_VERSION "26.0.0")

    if(NOT MESA26_RUNTIME_CASE STREQUAL "consumer-cached-flag")
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATION_CURRENT TRUE)
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SOURCE_DIR
            "${AROS_SOURCE_DIR}")
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_PATH
            "${AROS_NATIVE_CONSUMER_CONTRACT}")
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SHA256
            "${AROS_NATIVE_CONSUMER_CONTRACT_SHA256}")
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_PROFILE
            "${AROS_TARGET_PROFILE}")
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_ISA "rv64imafdc")
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_ABI "lp64d")
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_CODE_MODEL "medany")
        foreach(_selector IN ITEMS AROS_TARGET_CPU AROS_TARGET_PLATFORM AROS_TARGET_TRIPLE
                AROS_TARGET_FAMILY AROS_TARGET_VARIANT AROS_TOOLCHAIN AROS_TARGET_CPU32
                GCC_CONFIG_FLOAT_ABI AROS_ABI_FLAVOUR AROS_MESA_VERSION)
            set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_${_selector}
                "${${_selector}}")
        endforeach()
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_PLATFORM_SMP ON)
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_USE_MMU ON)
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_EXEC_SMP ON)
        set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SDK_INCLUDE_RELATIVE
            "${AROS_NATIVE_CONSUMER_SDK_INCLUDE_RELATIVE}")
    endif()

    if(MESA26_RUNTIME_CASE STREQUAL "consumer-changed-digest")
        set(AROS_NATIVE_CONSUMER_CONTRACT_SHA256
            "0000000000000000000000000000000000000000000000000000000000000000")
    elseif(MESA26_RUNTIME_CASE STREQUAL "consumer-changed-selector")
        set(AROS_TARGET_PLATFORM "changed-after-validation")
    elseif(MESA26_RUNTIME_CASE STREQUAL "consumer-changed-contract-bytes")
        file(APPEND "${AROS_NATIVE_CONSUMER_CONTRACT}" "changed after validation\n")
    elseif(MESA26_RUNTIME_CASE STREQUAL "consumer-and-build-contract")
        set(AROS_NATIVE_BUILD_CONTRACT "${CMAKE_BINARY_DIR}/native-build-contract.json")
        file(WRITE "${AROS_NATIVE_BUILD_CONTRACT}" "fixture native build contract\n")
    endif()
endif()

include("${MESA26_RUNTIME_MODULE}")
aros_configure_mesa26_runtime()

if(MESA26_RUNTIME_CASE STREQUAL "positive")
    if(NOT TARGET mesa26-runtime-selection)
        message(FATAL_ERROR "Mesa26 runtime selection target was not created")
    endif()
    get_target_property(_loader_dependencies
        workbench-libs-gl MANUALLY_ADDED_DEPENDENCIES)
    foreach(_expected IN ITEMS mesa3dgl-library mesa26-runtime-selection)
        if(NOT _expected IN_LIST _loader_dependencies)
            message(FATAL_ERROR
                "workbench-libs-gl is missing runtime dependency ${_expected}")
        endif()
    endforeach()
elseif(MESA26_RUNTIME_CASE STREQUAL "non26")
    if(TARGET mesa26-runtime-selection)
        message(FATAL_ERROR "non26 Mesa configuration created a Mesa26 target")
    endif()
    get_target_property(_loader_dependencies
        workbench-libs-gl MANUALLY_ADDED_DEPENDENCIES)
    if(_loader_dependencies AND NOT _loader_dependencies STREQUAL "_loader_dependencies-NOTFOUND")
        message(FATAL_ERROR
            "non26 Mesa configuration added loader dependencies: ${_loader_dependencies}")
    endif()
elseif(MESA26_RUNTIME_CASE MATCHES "^consumer-")
    if(TARGET mesa26-runtime-selection)
        message(FATAL_ERROR "Mesa26 consumer configuration created a runtime target")
    endif()
    get_target_property(_loader_dependencies
        workbench-libs-gl MANUALLY_ADDED_DEPENDENCIES)
    if(_loader_dependencies AND NOT _loader_dependencies STREQUAL "_loader_dependencies-NOTFOUND")
        message(FATAL_ERROR
            "Mesa26 consumer configuration added loader dependencies: ${_loader_dependencies}")
    endif()
endif()
