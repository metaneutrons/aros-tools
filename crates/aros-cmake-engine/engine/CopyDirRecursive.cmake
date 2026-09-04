cmake_minimum_required(VERSION 3.22)

# Build-time implementation of MetaMake's %copy_dir_recursive primitive.
#
# The legacy cpy-dir-rec.py script overlays rather than cleans destinations,
# copies only changed files, and always filters build/control metadata.  Keep
# those semantics here so independent recursive-copy declarations can safely
# target nested destination trees in the same parallel build.
foreach(_aros_copy_dir_var IN ITEMS
        AROS_COPY_DIR_SOURCE
        AROS_COPY_DIR_DESTINATION
        AROS_COPY_DIR_STAMP)
    if(NOT DEFINED ${_aros_copy_dir_var} OR
       "${${_aros_copy_dir_var}}" STREQUAL "")
        message(FATAL_ERROR
            "CopyDirRecursive.cmake requires ${_aros_copy_dir_var}")
    endif()
endforeach()

if(NOT IS_DIRECTORY "${AROS_COPY_DIR_SOURCE}")
    message(FATAL_ERROR
        "%copy_dir_recursive source directory is unavailable: "
        "${AROS_COPY_DIR_SOURCE}")
endif()

file(MAKE_DIRECTORY "${AROS_COPY_DIR_DESTINATION}")
file(COPY "${AROS_COPY_DIR_SOURCE}/"
    DESTINATION "${AROS_COPY_DIR_DESTINATION}"
    PATTERN ".cvsignore" EXCLUDE
    PATTERN "mmakefile.src" EXCLUDE
    PATTERN "mmakefile" EXCLUDE
    PATTERN "CVS" EXCLUDE
    PATTERN ".svn" EXCLUDE
    PATTERN ".git*" EXCLUDE)
file(TOUCH "${AROS_COPY_DIR_STAMP}")
