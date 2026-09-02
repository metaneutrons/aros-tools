# =============================================================================
# Kickstart aggregate
# =============================================================================
#
# The packages and the kickstart ELF themselves are no longer declared here.
# They come from the transpiled `%make_package` and `%link_kickstart` calls in
# generated_targets.cmake, which read the module lists straight out of the
# mmakefiles.
#
# That replaced hand-written lists which were also wrong. The base package was
# declared with 17 modules against the 24 rom/mmakefile.src actually names,
# missing dos64, both filesystem handlers, all five base hidds and debug; a
# system built from it would have been short of pieces with nothing saying so.
#
# What remains here is the aggregate: one target that builds everything the
# bootstrap loads.

get_property(_pkg_targets GLOBAL PROPERTY AROS_PACKAGE_TARGETS)
get_property(_ks_targets GLOBAL PROPERTY AROS_KICKSTART_TARGETS)

set(KICKSTART_DEPS "")
foreach(t IN LISTS _ks_targets _pkg_targets)
    if(TARGET ${t})
        list(APPEND KICKSTART_DEPS ${t})
    endif()
endforeach()

if(KICKSTART_DEPS)
    list(REMOVE_DUPLICATES KICKSTART_DEPS)
    add_custom_target(kickstart DEPENDS ${KICKSTART_DEPS})
    list(LENGTH KICKSTART_DEPS _n_ks)
    message(STATUS "🧩 AROS-NX: kickstart aggregates ${_n_ks} package/link target(s)")
else()
    message(STATUS "🧩 AROS-NX: no kickstart targets for this configuration")
endif()
