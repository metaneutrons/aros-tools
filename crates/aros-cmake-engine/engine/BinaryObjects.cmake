# A flat binary wrapped as a relocatable object.
#
# config/make.tmpl:1552 (%rule_link_binary) links the given objects at a fixed
# text address with no ELF wrapper, then re-links the raw image with
# `ld -r --format binary`, which gives the consumer `_binary_<name>_start`,
# `_binary_<name>_end` and `_binary_<name>_size`.
#
# The kernel's SMP trampoline is the case that matters: rom/kernel copies the
# image to low memory to start the other cores, and referenced
# `_binary_smpbootstrap_start` and `_binary_smpbootstrap_size` as dangling
# externals until this existed. The bootstrap loader forgives exactly one
# undefined symbol, SysBase (bootstrap/elfloader.c:157).
#
# Both link steps are ld.lld invoked directly, matching the reference, which
# uses $(KERNEL_LD) then $(AROS_LD).

include_guard(GLOBAL)
include(CMakeParseArguments)

# aros_link_binary_object(NAME <n> OUTPUT <path> DIRECTORY <dir>
#                         SOURCES <basenames...> START <addr>
#                         [LDFLAGS <flags...>] CONSUMER <target> [ARCH_TAG <t>])
function(aros_link_binary_object)
    set(oneValueArgs NAME OUTPUT DIRECTORY START CONSUMER ARCH_TAG)
    set(multiValueArgs SOURCES LDFLAGS)
    cmake_parse_arguments(BO "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

    foreach(_required NAME OUTPUT DIRECTORY SOURCES CONSUMER)
        if(NOT BO_${_required})
            message(FATAL_ERROR
                "aros_link_binary_object: ${_required} is required")
        endif()
    endforeach()
    # Tested for emptiness, not truth: the usual start address is 0, and
    # if(NOT "0") is true in CMake.
    if(BO_START STREQUAL "")
        message(FATAL_ERROR "aros_link_binary_object: START is required")
    endif()
    if(BO_ARCH_TAG AND NOT BO_ARCH_TAG IN_LIST AROS_ARCH_INCLUDE_TAGS)
        # Another architecture's trampoline. Declared for every architecture,
        # built for this one.
        return()
    endif()
    # A standalone-linked consumer has no target yet: its link is created by
    # aros_finalize_standalone_links, after every binary object is declared. Its
    # object library carries the name instead.
    set(_consumer_objects "${BO_CONSUMER}-objs")
    if(NOT TARGET "${BO_CONSUMER}" AND NOT TARGET "${_consumer_objects}")
        set_property(GLOBAL APPEND PROPERTY AROS_BINARY_OBJECT_GAPS
            "${BO_NAME}: consumer ${BO_CONSUMER} is not a target here")
        return()
    endif()
    if(NOT AROS_LLD_BIN)
        message(FATAL_ERROR
            "aros_link_binary_object(${BO_NAME}): AROS_LLD_BIN is required")
    endif()

    # No LANGUAGE: the inputs are a C source (files=) or an assembler one
    # (asmfiles=, or an object a sibling %rule_assemble_multi built), and the
    # declaration does not distinguish them. aros_resolve_sources' default lane
    # tries .c, .cpp, .S and .s, which is the same set.
    aros_resolve_sources(_sources "${BO_DIRECTORY}"
        MMAKE_ID "${BO_CONSUMER}-binary-${BO_NAME}"
        SOURCES ${BO_SOURCES})
    if(NOT _sources)
        set_property(GLOBAL APPEND PROPERTY AROS_BINARY_OBJECT_GAPS
            "${BO_NAME}: no source resolved in ${BO_DIRECTORY}")
        return()
    endif()
    aros_mark_preprocessed_asm(${_sources})

    # One object library per image, so the flat link consumes real objects and
    # the sources are compiled with this configuration's own flags.
    set(_objects_target "${BO_CONSUMER}-binary-${BO_NAME}-objects")
    if(TARGET "${_objects_target}")
        return()
    endif()
    add_library("${_objects_target}" OBJECT ${_sources})
    set_target_properties("${_objects_target}" PROPERTIES LINKER_LANGUAGE C)
    # A binary object inherits the consumer directory's architecture. Without
    # the gate, the PC bootstrap's vesa blob remained an implicit `all` member
    # in ARM builds even though the standalone consumer itself was foreign.
    aros_gate_arch("${_objects_target}" "${BO_DIRECTORY}")
    aros_apply_includes("${_objects_target}" MODULE_DIR "${BO_DIRECTORY}")
    # The image is linked with the consumer's architecture -- the vesa blob is
    # `-m elf_i386` -- so it has to be compiled for it too. The declaration
    # states only the link flag, and the consumer holds the rest.
    get_property(_consumer_isa GLOBAL PROPERTY
        "AROS_ISA_OPTIONS_${BO_CONSUMER}")
    if(_consumer_isa)
        target_compile_options("${_objects_target}" PRIVATE ${_consumer_isa})
    endif()
    get_property(_host_headers GLOBAL PROPERTY AROS_HOST_HEADER_TARGETS)
    if(_consumer_isa AND _host_headers)
        add_dependencies("${_objects_target}" ${_host_headers})
    endif()

    get_filename_component(_out_dir "${BO_OUTPUT}" DIRECTORY)
    # The second link derives the symbol names from the input path as written,
    # so it runs in the directory holding the image and names it bare. That is
    # the `cd` in the reference recipe, and the reason the raw image is not
    # written straight to _out_dir under a different name.
    set(_image_dir "${CMAKE_BINARY_DIR}/gen/binary/${BO_NAME}")
    add_custom_command(
        OUTPUT "${BO_OUTPUT}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_image_dir}" "${_out_dir}"
        # --image-base is a deviation from config/make.tmpl:1567, which passes
        # only --entry, --oformat binary and -Ttext. ld.lld refuses a text
        # address below its default image base for the target:
        #
        #   ld.lld: error: section '.text' address (0x0) is smaller than image
        #   base (0x200000); specify --image-base
        #
        # The reference recipe was written for GNU ld, which has no such base.
        # Setting it to the same address as -Ttext keeps the flat image exactly
        # where the declaration asks for it.
        # -z norelro is the second lld-specific addition, for the same reason
        # as --image-base: lld lays out a RELRO segment even for a flat image,
        # and at a fixed low text address it overlaps .data --
        #
        #   ld.lld: error: section .relro_padding virtual address range
        #   overlaps with .data
        #
        # A raw binary has no dynamic loader to enforce RELRO, so there is
        # nothing to lose by switching it off.
        COMMAND "${AROS_LLD_BIN}" ${BO_LDFLAGS}
            "--entry=${BO_START}" --oformat binary "-Ttext=${BO_START}"
            "--image-base=${BO_START}" -z norelro
            -o "${_image_dir}/${BO_NAME}"
            "$<TARGET_OBJECTS:${_objects_target}>"
        COMMAND "${CMAKE_COMMAND}" -E chdir "${_image_dir}"
            "${AROS_LLD_BIN}" ${BO_LDFLAGS} -r --format binary "${BO_NAME}"
            -o "${BO_OUTPUT}"
        DEPENDS "${_objects_target}"
        COMMENT "Wrapping ${BO_NAME} as a relocatable object"
        COMMAND_EXPAND_LISTS
        VERBATIM)

    # The consumer links it as an ordinary external object, which is what
    # $(wildcard $(OBJDIR)/arch/*.o) achieves in the reference.
    set_source_files_properties("${BO_OUTPUT}" PROPERTIES
        EXTERNAL_OBJECT TRUE GENERATED TRUE)
    # Recorded for every consumer, and attached directly only when the consumer
    # is a real target.
    set_property(GLOBAL APPEND PROPERTY
        "AROS_BINARY_OBJECTS_FOR_${BO_CONSUMER}" "${BO_OUTPUT}")
    if(TARGET "${BO_CONSUMER}")
        target_sources("${BO_CONSUMER}" PRIVATE "${BO_OUTPUT}")
    endif()

    # A kickstart member's objects were mirrored while its target was created,
    # which is before this runs, so the wrapped binary has to be registered here
    # too or the kickstart object loses _binary_<name>_start.
    get_property(_mirrored GLOBAL PROPERTY
        "AROS_KICKSTART_OBJECTS_${BO_CONSUMER}")
    if(_mirrored)
        set_property(GLOBAL APPEND PROPERTY
            "AROS_KICKSTART_EXTOBJS_${BO_CONSUMER}" "${BO_OUTPUT}")
    endif()
endfunction()

# aros_report_binary_object_gaps()
#
# Declarations this configuration could not build. Written out rather than
# implied: each one is a wrapped binary the reference produces and we do not.
function(aros_report_binary_object_gaps)
    get_property(_gaps GLOBAL PROPERTY AROS_BINARY_OBJECT_GAPS)
    set(_report "${CMAKE_BINARY_DIR}/generated_targets.binary-object-gaps.txt")
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
        "⚠️  ${_count} %rule_link_binary declaration(s) not built here -> ${_report}")
endfunction()
