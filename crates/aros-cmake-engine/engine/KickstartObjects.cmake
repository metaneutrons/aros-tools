# A kickstart member is a different artefact from the loadable module.
#
# config/make.tmpl:2743 builds it with its own rule, and every difference from
# the module link matters:
#
#   $(KOBJ) : $(OBJS) $(ENDOBJS)
#       $(AROS_LD) -Ur $(KOBJ_LDFLAGS) $(KERNEL_KOBJ_LDSCRIPT) -o $@ $^ \
#           $(USER_LDFLAGS) -L$(AROS_LIB) $(addprefix -l,$(LINKLIBS))
#       $(OBJCOPY) $@ $(FILTBASES) `... -L <set list symbols>`
#
#   * $(AROS_LD) directly, not the compiler driver, so the spec's default link
#     set does not apply here at all. A kickstart member linked with it carries
#     the same archive members as its neighbours, and the joint link fails with
#     `duplicate symbol: LibNextTagItem`.
#   * KAUTOLIB is the kobj's own, much smaller default set: dos intuition
#     layers graphics oop utility expansion keymap (make.tmpl:2743).
#   * KLIB -- hiddstubs amiga arossupport autoinit libinit -- is filtered out
#     of the declaration's uselibs (make.tmpl:2752).
#   * objcopy -L makes the library bases local, and with them every
#     __*_LIST__, __*_END__ and __aros_lib* symbol. That localisation is what
#     lets several members be linked into one image; without it the set lists
#     collide (`duplicate symbol: set_call_libfuncs`).
#
#   * KERNEL_KOBJ_LDSCRIPT orders the module's sections so its Resident tag is
#     the module head and its End marker -- the rt_EndSkip the romtag scanner
#     leaps to -- the module tail. Without it the kickstart link merges all the
#     tags into one block and all the End markers into a block behind it, the
#     first tag's leap skips every other module, and the boot ends in
#     `exec.library is not found`. The transpiler reads the value from
#     config/make.cfg.in and declares it with aros_set_kickstart_kobj_ldscript.

include_guard(GLOBAL)
include(CMakeParseArguments)

# Found here rather than reused from SymbolAudit.cmake, which is included after
# the generated targets.
find_program(AROS_KICKSTART_PYTHON3 NAMES python3)
get_filename_component(_aros_kickstart_cc_dir "${CMAKE_C_COMPILER}" DIRECTORY)
find_program(AROS_KICKSTART_NM
    NAMES llvm-nm
    HINTS "${AROS_CROSS_TOOLCHAIN_ROOT}/bin" "${_aros_kickstart_cc_dir}"
          "/opt/homebrew/opt/llvm/bin" "/usr/local/opt/llvm/bin")
set(AROS_KICKSTART_LOCALISE_SCRIPT
    "${CMAKE_SOURCE_DIR}/scripts/kickstart/localise-symbols.py")

# The kobj's own default library set, by name (config/make.tmpl:2743).
set(AROS_KICKSTART_AUTOLIBS
    dos intuition layers graphics oop utility expansion keymap)

# Never linked into a kickstart member (config/make.tmpl:2752).
set(AROS_KICKSTART_EXCLUDED_LIBS
    hiddstubs amiga arossupport autoinit libinit)

# Made file-local so members can be linked together (config/make.tmpl:2746).
set(AROS_KICKSTART_LOCAL_BASES
    DOSBase IntuitionBase LayersBase GfxBase OOPBase
    UtilityBase ExpansionBase KeymapBase KernelBase)

# aros_set_kickstart_kobj_ldscript(<token>...)
#
# The `-T <script>` KERNEL_KOBJ_LDSCRIPT names, as the transpiler read it from
# config/make.cfg.in. Stored rather than applied, because the member objects are
# built on request, later.
function(aros_set_kickstart_kobj_ldscript)
    set_property(GLOBAL PROPERTY AROS_KICKSTART_KOBJ_LDSCRIPT "${ARGN}")
endfunction()

# _aros_kickstart_archive_map(<out-var>)
#
# `<name>;<target>` pairs for every static library in the build, keyed by the
# archive base name a uselib refers to. Built once and cached in a global
# property, because it walks every target.
function(_aros_kickstart_archive_map out_var)
    get_property(_map GLOBAL PROPERTY AROS_KICKSTART_ARCHIVE_MAP)
    if(_map)
        set(${out_var} "${_map}" PARENT_SCOPE)
        return()
    endif()
    _aros_collect_targets("${CMAKE_SOURCE_DIR}" _all)
    foreach(_target IN LISTS _all)
        get_target_property(_type "${_target}" TYPE)
        if(NOT _type STREQUAL "STATIC_LIBRARY")
            continue()
        endif()
        get_target_property(_name "${_target}" OUTPUT_NAME)
        if(NOT _name OR _name STREQUAL "_name-NOTFOUND")
            set(_name "${_target}")
        endif()
        list(APPEND _map "${_name}" "${_target}")
    endforeach()
    set_property(GLOBAL PROPERTY AROS_KICKSTART_ARCHIVE_MAP "${_map}")
    set(${out_var} "${_map}" PARENT_SCOPE)
endfunction()

# aros_register_kickstart_member(<module-target> <objects-target> <uselibs>)
#
# Called by a module builder that has made its objects reusable. Recorded rather
# than acted on, because the kickstart declaration that needs the second
# artefact is emitted after every module.
function(aros_register_kickstart_member module objects uselibs)
    set_property(GLOBAL PROPERTY "AROS_KICKSTART_OBJECTS_${module}" "${objects}")
    set_property(GLOBAL PROPERTY "AROS_KICKSTART_USELIBS_${module}" "${uselibs}")
endfunction()

# aros_kickstart_member_object(<out-var> <module-target>)
#
# The member's kobj, built on first request and reused afterwards. Sets
# <out-var> to the object path, or to the module's own artefact when the module
# did not make its objects reusable -- which is the old behaviour, and is
# reported.
function(aros_kickstart_member_object out_var module)
    set(_kobj "${CMAKE_BINARY_DIR}/gen/kobj/${module}.o")
    # Not "<module>-kobj": that is a MetaMake target name the transpiler already
    # emits as a phony, so the existence check matched it and this function
    # returned a path nothing built.
    set(_kobj_target "${module}-kickstart-object")
    if(TARGET "${_kobj_target}")
        set(${out_var} "${_kobj}" PARENT_SCOPE)
        return()
    endif()

    get_property(_objects GLOBAL PROPERTY "AROS_KICKSTART_OBJECTS_${module}")
    if(NOT _objects OR NOT TARGET "${_objects}")
        set_property(GLOBAL APPEND PROPERTY AROS_KICKSTART_GAPS
            "${module}: objects not reusable, linked as a whole module instead")
        set(${out_var} "$<TARGET_FILE:${module}>" PARENT_SCOPE)
        return()
    endif()
    if(NOT AROS_LLD_BIN OR NOT CMAKE_OBJCOPY OR NOT AROS_KICKSTART_NM
       OR NOT AROS_KICKSTART_PYTHON3 OR NOT EXISTS "${AROS_KICKSTART_LOCALISE_SCRIPT}")
        message(FATAL_ERROR
            "aros_kickstart_member_object(${module}): needs ld.lld, objcopy, "
            "llvm-nm, python3 and ${AROS_KICKSTART_LOCALISE_SCRIPT}")
    endif()

    # Resolved to archive targets, not to `-l<name>`: an archive whose
    # declaration was never an explicit `-l` consumer keeps its target-derived
    # file name, so `-lstdc.static` finds nothing while
    # $<TARGET_FILE:linklibs-stdc-static> is exact.
    _aros_kickstart_archive_map(_by_name)
    get_property(_uselibs GLOBAL PROPERTY "AROS_KICKSTART_USELIBS_${module}")
    set(_lib_args "")
    set(_lib_deps "")
    set(_unresolved "")
    foreach(_lib IN LISTS _uselibs AROS_KICKSTART_AUTOLIBS)
        if(_lib IN_LIST AROS_KICKSTART_EXCLUDED_LIBS)
            continue()
        endif()
        list(FIND _by_name "${_lib}" _index)
        if(_index LESS 0)
            if(NOT _lib IN_LIST _unresolved)
                list(APPEND _unresolved "${_lib}")
            endif()
            continue()
        endif()
        math(EXPR _index "${_index} + 1")
        list(GET _by_name ${_index} _archive)
        if(_archive IN_LIST _lib_deps)
            continue()
        endif()
        list(APPEND _lib_deps "${_archive}")
        list(APPEND _lib_args "$<TARGET_FILE:${_archive}>")
    endforeach()
    # The declaration's own link options stay in their `-L`/`-l` form, as the
    # reference passes them, so the SDK library directory has to be on the
    # search path too -- that is the `-L$(AROS_LIB)` of make.tmpl:2758. Any
    # `-l<name>` the archive map knows also becomes a real input edge, so the
    # archive exists before this links.
    get_property(_external_objects GLOBAL PROPERTY
        "AROS_KICKSTART_EXTOBJS_${module}")
    get_property(_ldopts GLOBAL PROPERTY "AROS_KICKSTART_LDOPTS_${module}")
    if(_ldopts)
        list(PREPEND _ldopts "-L" "${AROS_DEVELOPER_LIB_DIR}")
        foreach(_option IN LISTS _ldopts)
            if(_option MATCHES "^-l(.+)$")
                set(_wanted "${CMAKE_MATCH_1}")
                list(FIND _by_name "${_wanted}" _index)
                if(_index GREATER_EQUAL 0)
                    math(EXPR _index "${_index} + 1")
                    list(GET _by_name ${_index} _archive)
                    if(NOT _archive IN_LIST _lib_deps)
                        list(APPEND _lib_deps "${_archive}")
                    endif()
                endif()
            endif()
        endforeach()
    endif()
    if(_unresolved)
        set_property(GLOBAL APPEND PROPERTY AROS_KICKSTART_GAPS
            "${module}: no archive for ${_unresolved}")
    endif()

    set(_localise "")
    foreach(_base IN LISTS AROS_KICKSTART_LOCAL_BASES)
        list(APPEND _localise "--localize-symbol=${_base}")
    endforeach()

    # The set lists are named per module, so they cannot be listed here: the
    # reference reads them back out of the linked object with nm. The script
    # does the same, then applies both sets of -L in one objcopy call.
    # The section-ordering script, and the script file as a real input edge so
    # every member relinks when it changes.
    get_property(_kobj_ldscript GLOBAL PROPERTY AROS_KICKSTART_KOBJ_LDSCRIPT)
    set(_ldscript_deps "")
    if(_kobj_ldscript)
        list(GET _kobj_ldscript -1 _ldscript_file)
        if(EXISTS "${_ldscript_file}")
            list(APPEND _ldscript_deps "${_ldscript_file}")
        else()
            set_property(GLOBAL APPEND PROPERTY AROS_KICKSTART_GAPS
                "${module}: section-ordering script ${_ldscript_file} is missing")
        endif()
    endif()

    add_custom_command(
        OUTPUT "${_kobj}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory
            "${CMAKE_BINARY_DIR}/gen/kobj"
        # Through aros-collect, because the reference's `-Ur` here is not just
        # "relocatable": it is collect-aros's mode that also builds the symbol
        # sets (collect-aros.c:188). The member's own `-T` ordering script goes
        # to the first pass, exactly as KERNEL_KOBJ_LDSCRIPT does there, and the
        # generated set script to the second.
        COMMAND "${AROS_COLLECT_BIN}" --ld "${AROS_LLD_BIN}"
            --report "${CMAKE_BINARY_DIR}/gen/kobj/${module}.sets.txt"
            -- -r ${_kobj_ldscript} -o "${_kobj}"
            "$<TARGET_OBJECTS:${_objects}>" ${_external_objects}
            ${_ldopts} ${_lib_args}
        COMMAND "${AROS_KICKSTART_PYTHON3}" -B
            "${AROS_KICKSTART_LOCALISE_SCRIPT}"
            "${CMAKE_OBJCOPY}" "${AROS_KICKSTART_NM}" "${_kobj}" ${_localise}
        DEPENDS "${_objects}" ${_external_objects} ${_lib_deps}
            ${_ldscript_deps}
            "${AROS_KICKSTART_LOCALISE_SCRIPT}"
        COMMENT "Kickstart object ${module}"
        COMMAND_EXPAND_LISTS
        VERBATIM)
    add_custom_target("${_kobj_target}" DEPENDS "${_kobj}")
    set(${out_var} "${_kobj}" PARENT_SCOPE)
endfunction()

# aros_report_kickstart_gaps()
function(aros_report_kickstart_gaps)
    get_property(_gaps GLOBAL PROPERTY AROS_KICKSTART_GAPS)
    set(_report "${CMAKE_BINARY_DIR}/generated_targets.kickstart-gaps.txt")
    if(NOT _gaps)
        file(REMOVE "${_report}")
        return()
    endif()
    list(REMOVE_DUPLICATES _gaps)
    list(SORT _gaps)
    string(REPLACE ";" "\n" _body "${_gaps}")
    file(WRITE "${_report}" "${_body}\n")
    list(LENGTH _gaps _count)
    message(STATUS
        "⚠️  ${_count} kickstart member(s) could not get their own object -> ${_report}")
endfunction()
