# =============================================================================
# Explicit M0 build roots
# =============================================================================
#
# These opt-in targets assemble only roots CMake can prove exist and record
# every missing or deliberately unproven edge next to the generated graph.
# Neither target is part of `all`.

include_guard(GLOBAL)

if(TARGET aros-core OR TARGET aros-distribution OR TARGET aros-root-report)
    message(FATAL_ERROR
        "AROS M0 root target name collision: aros-core, aros-distribution, "
        "and aros-root-report are reserved")
endif()

set(_aros_core_deps "")
set(_aros_distribution_deps "")
set(_aros_root_missing "")
set(_aros_root_optional_missing "")
set(_aros_historic_root_lines "")

macro(_aros_root_require list_var purpose target_name)
    if(TARGET "${target_name}")
        list(APPEND ${list_var} "${target_name}")
    else()
        list(APPEND _aros_root_missing "${purpose}: ${target_name}")
    endif()
endmacro()

macro(_aros_root_optional list_var purpose target_name)
    if(TARGET "${target_name}")
        list(APPEND ${list_var} "${target_name}")
    else()
        list(APPEND _aros_root_optional_missing "${purpose}: ${target_name}")
    endif()
endmacro()

macro(_aros_root_expect_edge purpose parent_name child_name)
    if(NOT TARGET "${parent_name}")
        list(APPEND _aros_root_missing
            "${purpose}: ${parent_name} -> ${child_name} (parent target missing)")
    elseif(NOT TARGET "${child_name}")
        list(APPEND _aros_root_missing
            "${purpose}: ${parent_name} -> ${child_name} (dependency target missing)")
    else()
        get_property(_aros_parent_deps
            TARGET "${parent_name}" PROPERTY MANUALLY_ADDED_DEPENDENCIES)
        if(NOT "${child_name}" IN_LIST _aros_parent_deps)
            list(APPEND _aros_root_missing
                "${purpose}: ${parent_name} -> ${child_name} (edge not attached)")
        endif()
    endif()
endmacro()

# Boot-core roots. The PC kernel is the %link_kickstart output itself; both
# Raspi ports additionally have a platform kernel meta target for core.elf.
if(AROS_TARGET_PLATFORM STREQUAL "pc" AND AROS_TARGET_CPU STREQUAL "x86_64")
    set(_aros_platform_kernel "kernel-pc-x86_64-kernel")
elseif(AROS_TARGET_PLATFORM STREQUAL "raspi"
       AND AROS_TARGET_CPU STREQUAL "arm")
    set(_aros_platform_kernel "kernel-raspi-arm")
elseif(AROS_TARGET_PLATFORM STREQUAL "raspi"
       AND AROS_TARGET_CPU STREQUAL "aarch64")
    set(_aros_platform_kernel "kernel-raspi-aarch64")
else()
    # Keep an unsupported configuration diagnosable without pretending that a
    # generic spelling is authoritative for every architecture.
    set(_aros_platform_kernel
        "kernel-${AROS_TARGET_PLATFORM}-${AROS_TARGET_CPU}")
    list(APPEND _aros_root_missing
        "unsupported M0 CPU/platform pair: ${AROS_TARGET_CPU}/${AROS_TARGET_PLATFORM}")
endif()

_aros_root_require(_aros_core_deps
    "configured kickstart/package aggregate" "kickstart")
_aros_root_require(_aros_core_deps
    "platform kernel root" "${_aros_platform_kernel}")
_aros_root_optional(_aros_core_deps
    "common kernel module meta root" "kernel-modules")
list(REMOVE_DUPLICATES _aros_core_deps)

add_custom_target(aros-core)
if(_aros_core_deps)
    add_dependencies(aros-core ${_aros_core_deps})
endif()

# Distribution roots. `AROS` remains outside aros-core because its historic
# closure also contains demos, external software, and the full Workbench. The
# compound distribution name is selected explicitly so the configured M0
# profile remains visible and auditable in the generated graph.
set(_aros_distribution_root
    "distfiles-${AROS_TARGET_PLATFORM}-${AROS_TARGET_CPU}")
_aros_root_require(_aros_distribution_deps
    "legacy system root" "AROS")
_aros_root_require(_aros_distribution_deps
    "legacy Workbench root" "workbench-complete")
_aros_root_require(_aros_distribution_deps
    "CPU/platform distribution root" "${_aros_distribution_root}")
list(REMOVE_DUPLICATES _aros_distribution_deps)

add_custom_target(aros-distribution)
add_dependencies(aros-distribution aros-core)
if(_aros_distribution_deps)
    add_dependencies(aros-distribution ${_aros_distribution_deps})
endif()

# Inventory the lower-level inputs on which these aggregates rely. The target
# lists are populated only for the active architecture by aros_make_package()
# and aros_link_kickstart(); icon declarations are target-agnostic, so report
# their count but let workbench-complete select the configured icon-set roots.
get_property(_aros_package_targets GLOBAL PROPERTY AROS_PACKAGE_TARGETS)
get_property(_aros_kickstart_targets GLOBAL PROPERTY AROS_KICKSTART_TARGETS)
get_property(_aros_icon_targets GLOBAL PROPERTY AROS_ICON_TARGETS)
get_property(_aros_fetch_targets GLOBAL PROPERTY AROS_FETCH_TARGETS)

foreach(_list_name IN ITEMS
        _aros_package_targets _aros_kickstart_targets _aros_icon_targets
        _aros_fetch_targets)
    list(REMOVE_DUPLICATES ${_list_name})
    list(SORT ${_list_name})
endforeach()

list(LENGTH _aros_package_targets _aros_package_count)
list(LENGTH _aros_kickstart_targets _aros_kickstart_count)
list(LENGTH _aros_icon_targets _aros_icon_count)
list(LENGTH _aros_fetch_targets _aros_fetch_count)

# The legacy Workbench root asks for this family root, but the current tree has
# only its more specific `...-additional-icons-aros*` descendants. Do not add a
# second guessed edge; make the missing historic edge explicit instead.
set(_aros_additional_icons_root
    "iconset-${AROS_TARGET_ICONSET}-additional-icons")

# Re-check the historic entry edges against the configured CMake target graph.
# This turns scanner/parser regressions into concrete report entries instead of
# relying on a permanent statement about which source files are discovered.
set(_aros_legacy_system_root
    "AROS-${AROS_TARGET_PLATFORM}-${AROS_TARGET_CPU}")
set(_aros_legacy_variant_root
    "${_aros_legacy_system_root}-${AROS_TARGET_VARIANT}")
set(_aros_legacy_complete_root
    "AROS-complete-${AROS_TARGET_PLATFORM}-${AROS_TARGET_CPU}")
_aros_root_expect_edge("historic cleanup edge" "AROS" "clean-errors")
_aros_root_expect_edge("historic configured system edge"
    "AROS" "${_aros_legacy_system_root}")
_aros_root_expect_edge("historic configured variant edge"
    "AROS" "${_aros_legacy_variant_root}")
_aros_root_expect_edge("historic toolchain edge"
    "AROS" "toolchain-linklibs")
_aros_root_expect_edge("historic Workbench edge"
    "AROS" "workbench-complete")
_aros_root_expect_edge("historic complete-system edge"
    "AROS-complete" "${_aros_legacy_complete_root}")
_aros_root_expect_edge("historic test edge" "AROS-complete" "test")
_aros_root_expect_edge("historic configured distribution edge"
    "distfiles" "${_aros_distribution_root}")
_aros_root_expect_edge("legacy Workbench icon edge"
    "workbench-complete" "${_aros_additional_icons_root}")

foreach(_root IN ITEMS AROS AROS-complete distfiles)
    if(TARGET "${_root}")
        list(APPEND _aros_historic_root_lines "  ${_root}: present")
        get_property(_root_deps
            TARGET "${_root}" PROPERTY MANUALLY_ADDED_DEPENDENCIES)
        list(REMOVE_DUPLICATES _root_deps)
        list(SORT _root_deps)
        if(_root_deps)
            foreach(_dep IN LISTS _root_deps)
                list(APPEND _aros_historic_root_lines "    -> ${_dep}")
            endforeach()
        else()
            list(APPEND _aros_historic_root_lines
                "    -> no configured dependency")
        endif()
    else()
        list(APPEND _aros_historic_root_lines "  ${_root}: missing")
    endif()
endforeach()

list(SORT _aros_core_deps)
list(SORT _aros_distribution_deps)
list(SORT _aros_root_missing)
list(SORT _aros_root_optional_missing)
list(LENGTH _aros_core_deps _aros_core_count)
list(LENGTH _aros_distribution_deps _aros_distribution_count)
list(LENGTH _aros_root_missing _aros_missing_count)

set(_aros_root_report
    "${CMAKE_BINARY_DIR}/generated_targets.root-semantics.txt")
file(WRITE "${_aros_root_report}"
    "AROS M0 root semantics\n"
    "cpu: ${AROS_TARGET_CPU}\n"
    "platform: ${AROS_TARGET_PLATFORM}\n"
    "legacy platform: ${AROS_TARGET_LEGACY_PLATFORM}\n"
    "iconset: ${AROS_TARGET_ICONSET}\n\n"
    "aros-core dependencies (${_aros_core_count}):\n")
foreach(_dep IN LISTS _aros_core_deps)
    file(APPEND "${_aros_root_report}" "  ${_dep}\n")
endforeach()
file(APPEND "${_aros_root_report}"
    "\naros-distribution dependencies, in addition to aros-core "
    "(${_aros_distribution_count}):\n")
foreach(_dep IN LISTS _aros_distribution_deps)
    file(APPEND "${_aros_root_report}" "  ${_dep}\n")
endforeach()

file(APPEND "${_aros_root_report}"
    "\nhistoric meta-root targets and configured dependencies:\n")
foreach(_line IN LISTS _aros_historic_root_lines)
    file(APPEND "${_aros_root_report}" "${_line}\n")
endforeach()

file(APPEND "${_aros_root_report}"
    "\nconfigured lower-level inventory:\n"
    "  package targets: ${_aros_package_count}\n"
    "  kickstart link targets: ${_aros_kickstart_count}\n"
    "  icon declaration targets: ${_aros_icon_count}\n"
    "  network fetch targets deliberately not added: ${_aros_fetch_count}\n"
    "\nmissing required targets or known historic edges:\n")
if(_aros_root_missing)
    foreach(_gap IN LISTS _aros_root_missing)
        file(APPEND "${_aros_root_report}" "  ${_gap}\n")
    endforeach()
else()
    file(APPEND "${_aros_root_report}" "  none detected by target existence\n")
endif()

file(APPEND "${_aros_root_report}" "\noptional roots not present:\n")
if(_aros_root_optional_missing)
    foreach(_gap IN LISTS _aros_root_optional_missing)
        file(APPEND "${_aros_root_report}" "  ${_gap}\n")
    endforeach()
else()
    file(APPEND "${_aros_root_report}" "  none\n")
endif()

file(APPEND "${_aros_root_report}"
    "\nunproven semantics (M0):\n"
    "  historic meta-root target discovery is checked above, but target "
    "existence does not prove that direct Make recipes have concrete CMake "
    "output rules\n"
    "  the selected legacy distribution root is an aggregate, not proof of a "
    "bootable image\n")
if(AROS_TARGET_PLATFORM STREQUAL "pc")
    file(APPEND "${_aros_root_report}"
        "  PC ISO/bootloader assembly remains outside the proven M0 closure\n")
elseif(AROS_TARGET_PLATFORM STREQUAL "raspi")
    file(APPEND "${_aros_root_report}"
        "  Raspi core.elf, image, config.txt, firmware, and copy recipes remain "
        "outside the proven M0 closure\n")
endif()

add_custom_target(aros-root-report
    COMMAND "${CMAKE_COMMAND}" -E cat "${_aros_root_report}"
    COMMENT "Showing the explicit M0 root selection and known gaps"
    VERBATIM)

message(STATUS
    "AROS M0 roots: aros-core selects ${_aros_core_count} existing target(s); "
    "aros-distribution adds ${_aros_distribution_count}; "
    "${_aros_missing_count} required edge(s) missing -> ${_aros_root_report}")
if(_aros_root_missing)
    message(WARNING
        "AROS M0 aggregate roots are intentionally incomplete; see "
        "${_aros_root_report}")
endif()
