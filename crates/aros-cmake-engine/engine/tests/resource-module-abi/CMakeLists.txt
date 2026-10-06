cmake_minimum_required(VERSION 3.22)
project(ResourceModuleAbiContract C)

foreach(_required AROS_SOURCE_DIR AROS_RUST_TOOLS_DIR AROS_HOST_GENMODULE
        AROS_TEST_ENGINE_DIR AROS_TEST_DEVICE_BASE_CONFIG
        AROS_TEST_DEVICE_OVERRIDE_CONFIG)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "${_required} is required")
    endif()
endforeach()

# Load the production engine without its broad configure-time SDK scan. This
# fixture exercises only explicit genmodule declaration ownership.
set(_bootstrap_stub_dir "${CMAKE_CURRENT_BINARY_DIR}/bootstrap-stub")
file(MAKE_DIRECTORY "${_bootstrap_stub_dir}")
file(WRITE "${_bootstrap_stub_dir}/BootstrapSDK.cmake"
    "function(aros_bootstrap_sdk_includes)\nendfunction()\n")
list(INSERT CMAKE_MODULE_PATH 0 "${_bootstrap_stub_dir}")

set(AROS_TARGET_CPU riscv)
set(AROS_TARGET_PLATFORM esp32p4)
include("${AROS_TEST_ENGINE_DIR}/AROS.cmake")

set(_kernel_dir "${AROS_SOURCE_DIR}/rom/kernel")
set(_kernel_config "${_kernel_dir}/kernel.conf")
set(_filesystem_dir "${AROS_SOURCE_DIR}/rom/filesystem")
set(_filesystem_config "${_filesystem_dir}/FileSystem.conf")
set(_card_dir "${AROS_SOURCE_DIR}/rom/card")
set(_card_config "${_card_dir}/card.conf")
set(_flashdisk_dir "${AROS_SOURCE_DIR}/arch/riscv-esp32p4/flashdisk")
set(_flashdisk_config "${_flashdisk_dir}/flashdisk.conf")
if(NOT EXISTS "${_kernel_config}")
    message(FATAL_ERROR "P4 kernel config is missing: ${_kernel_config}")
endif()
if(NOT EXISTS "${_filesystem_config}")
    message(FATAL_ERROR "P4 FileSystem config is missing: ${_filesystem_config}")
endif()
if(NOT EXISTS "${_card_config}")
    message(FATAL_ERROR "P4 card config is missing: ${_card_config}")
endif()
if(NOT EXISTS "${_flashdisk_config}")
    message(FATAL_ERROR "P4 flashdisk config is missing: ${_flashdisk_config}")
endif()

# This id models the source-owned full %build_module ABI contract. It generates
# no runtime target and is built only through the public includes endpoint.
aros_set_module_config(probe-kernel "${_kernel_config}")
aros_set_module_abi(probe-kernel)
aros_module_scaffolding(_kernel_sources _kernel_scaffold
    MODTYPE resource
    TARGET kernel
    MMAKE_ID probe-kernel
    DIRECTORY "${_kernel_dir}")

# Same resource config, but runtime-only source ownership: no public ABI aliases
# may be inferred from the resource type or config alone.
aros_set_module_config(probe-runtime "${_kernel_config}")
aros_module_scaffolding(_runtime_sources _runtime_scaffold
    MODTYPE resource
    TARGET kernel
    MMAKE_ID probe-runtime
    DIRECTORY "${_kernel_dir}")

# The real FileSystem resource uses %build_module with options noincludes and
# an empty functionlist. It owns an includes endpoint and libdefs output, but
# genmodule emits neither an FD nor headers for this configuration.
aros_set_module_config(kernel-filesystem "${_filesystem_config}")
aros_set_module_abi(kernel-filesystem)
aros_module_scaffolding(_filesystem_sources _filesystem_scaffold
    MODTYPE resource
    TARGET FileSystem
    MMAKE_ID kernel-filesystem
    DIRECTORY "${_filesystem_dir}")

# A second, unmarked declaration of the same resource config exercises the
# runtime-only path and guards against inferring ABI aliases from type/config.
aros_set_module_config(probe-filesystem-runtime "${_filesystem_config}")
aros_module_scaffolding(_filesystem_runtime_sources _filesystem_runtime_scaffold
    MODTYPE resource
    TARGET FileSystem
    MMAKE_ID probe-filesystem-runtime
    DIRECTORY "${_filesystem_dir}")

# The card module's source declares modname=card while card.conf overrides its
# generated API name with `includename cardres`.
aros_set_module_config(kernel-cardres "${_card_config}")
aros_set_module_abi(kernel-cardres)
aros_module_scaffolding(_card_sources _card_scaffold
    MODTYPE resource
    TARGET card
    MMAKE_ID kernel-cardres
    DIRECTORY "${_card_dir}")

# The real P4 flashdisk device uses config defaults plus its validated
# BeginIO/AbortIO pair; it has no explicit functionlist section.
aros_set_module_config(kernel-flashdisk "${_flashdisk_config}")
aros_set_module_abi(kernel-flashdisk)
aros_module_scaffolding(_flashdisk_sources _flashdisk_scaffold
    MODTYPE device
    TARGET flashdisk
    MMAKE_ID kernel-flashdisk
    DIRECTORY "${_flashdisk_dir}")

# The base config intentionally has AUTO includes resolved before the override
# adds a cdef and functionlist. The base decision must remain noincludes.
aros_set_module_config(probe-synthetic-device "${AROS_TEST_DEVICE_BASE_CONFIG}")
aros_set_module_config_override(
    probe-synthetic-device "${AROS_TEST_DEVICE_OVERRIDE_CONFIG}")
aros_set_module_abi(probe-synthetic-device)
aros_module_scaffolding(_synthetic_device_sources _synthetic_device_scaffold
    MODTYPE device
    TARGET SyntheticDevice
    MMAKE_ID probe-synthetic-device
    DIRECTORY "${_flashdisk_dir}")

foreach(_target probe-kernel-includes probe-kernel-fd
        probe-kernel-includes-generated probe-runtime-genmodfiles-generated
        kernel-filesystem-includes kernel-filesystem-includes-generated
        kernel-filesystem-libdefs-generated
        probe-filesystem-runtime-genmodfiles-generated
        kernel-cardres-includes kernel-cardres-fd
        kernel-cardres-includes-generated kernel-cardres-fd-generated
        kernel-cardres-libdefs-generated
        kernel-flashdisk-includes kernel-flashdisk-fd
        kernel-flashdisk-includes-generated kernel-flashdisk-fd-generated
        kernel-flashdisk-libdefs-generated
        probe-synthetic-device-includes
        probe-synthetic-device-includes-generated
        probe-synthetic-device-libdefs-generated)
    if(NOT TARGET "${_target}")
        message(FATAL_ERROR "expected genmodule declaration target ${_target}")
    endif()
endforeach()
foreach(_target probe-synthetic-device-fd probe-synthetic-device-fd-generated)
    if(TARGET "${_target}")
        message(FATAL_ERROR
            "base noincludes decision was lost across device override: ${_target}")
    endif()
endforeach()
foreach(_target kernel-filesystem-fd kernel-filesystem-fd-generated)
    if(TARGET "${_target}")
        message(FATAL_ERROR
            "noincludes FileSystem unexpectedly owns an FD target ${_target}")
    endif()
endforeach()
foreach(_target probe-runtime-includes probe-runtime-fd includes-kernel includes-all
        probe-filesystem-runtime-includes probe-filesystem-runtime-fd)
    if(TARGET "${_target}")
        message(FATAL_ERROR
            "runtime-only resource unexpectedly owns public ABI alias ${_target}")
    endif()
endforeach()

get_property(_filesystem_includes_dependencies
    TARGET kernel-filesystem-includes-generated
    PROPERTY MANUALLY_ADDED_DEPENDENCIES)
if(NOT "kernel-filesystem-libdefs-generated" IN_LIST _filesystem_includes_dependencies)
    message(FATAL_ERROR
        "FileSystem includes endpoint does not depend on its libdefs producer: "
        "${_filesystem_includes_dependencies}")
endif()
if("kernel-filesystem-fd-generated" IN_LIST _filesystem_includes_dependencies)
    message(FATAL_ERROR
        "noincludes FileSystem endpoint acquired an unsupported FD dependency")
endif()

get_property(_card_includes_dependencies
    TARGET kernel-cardres-includes-generated
    PROPERTY MANUALLY_ADDED_DEPENDENCIES)
foreach(_required kernel-cardres-fd-generated kernel-cardres-libdefs-generated)
    if(NOT _required IN_LIST _card_includes_dependencies)
        message(FATAL_ERROR
            "cardres includes endpoint does not depend on ${_required}: "
            "${_card_includes_dependencies}")
    endif()
endforeach()

get_property(_flashdisk_includes_dependencies
    TARGET kernel-flashdisk-includes-generated
    PROPERTY MANUALLY_ADDED_DEPENDENCIES)
foreach(_required kernel-flashdisk-fd-generated kernel-flashdisk-libdefs-generated)
    if(NOT _required IN_LIST _flashdisk_includes_dependencies)
        message(FATAL_ERROR
            "flashdisk ABI endpoint does not depend on ${_required}: "
            "${_flashdisk_includes_dependencies}")
    endif()
endforeach()

get_property(_synthetic_device_includes_dependencies
    TARGET probe-synthetic-device-includes-generated
    PROPERTY MANUALLY_ADDED_DEPENDENCIES)
if(NOT "probe-synthetic-device-libdefs-generated"
       IN_LIST _synthetic_device_includes_dependencies)
    message(FATAL_ERROR
        "synthetic noincludes endpoint lost its libdefs dependency: "
        "${_synthetic_device_includes_dependencies}")
endif()
if("probe-synthetic-device-fd-generated"
   IN_LIST _synthetic_device_includes_dependencies)
    message(FATAL_ERROR
        "synthetic noincludes endpoint acquired an FD dependency")
endif()
