# Shared target wiring for genmodule ABI products.

include_guard(GLOBAL)

function(_aros_genmodule_alias alias_target dependency)
    if(NOT TARGET "${alias_target}")
        add_custom_target("${alias_target}")
    endif()
    if(NOT "${alias_target}" STREQUAL "${dependency}")
        add_dependencies("${alias_target}" "${dependency}")
    endif()
endfunction()

# _aros_bind_genmodule_abi_targets(<mmake-id> <includes-target> <fd-target>)
#
# Legacy %build_module makes <mmake>-includes depend on <mmake>-fd. Keep that
# edge on the public alias itself: every client link library already depends
# on the includes alias, so building one directly must materialise both ABI
# headers and the FD without relying on the runtime aggregate.
function(_aros_bind_genmodule_abi_targets mmake_id includes_target fd_target)
    if(NOT mmake_id OR NOT includes_target OR NOT fd_target)
        message(FATAL_ERROR
            "_aros_bind_genmodule_abi_targets: mmake id, includes target and FD target are required")
    endif()
    _aros_genmodule_alias("${mmake_id}-includes" "${includes_target}")
    _aros_genmodule_alias("${mmake_id}-fd" "${fd_target}")
    _aros_genmodule_alias("${mmake_id}-includes" "${fd_target}")
endfunction()
