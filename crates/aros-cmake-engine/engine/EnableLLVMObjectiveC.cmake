# Clang's Objective-C frontend is part of the same compiler binary as C. Keep
# the language opt-in tied to the LLVM lane: GNU cross compilers may be built
# without cc1obj, while the current LLVM profiles can compile the tree's .m
# sources directly. Selecting the already-validated C compiler also prevents
# CMake from accidentally finding the host compiler for a cross build.
if(CMAKE_C_COMPILER_ID MATCHES "Clang")
    if(CMAKE_OBJC_COMPILER AND
       NOT CMAKE_OBJC_COMPILER STREQUAL CMAKE_C_COMPILER)
        message(FATAL_ERROR
            "AROS Objective-C compiler must match the validated C compiler: "
            "'${CMAKE_OBJC_COMPILER}' != '${CMAKE_C_COMPILER}'")
    elseif(NOT CMAKE_OBJC_COMPILER)
        set(CMAKE_OBJC_COMPILER "${CMAKE_C_COMPILER}" CACHE FILEPATH
            "Objective-C compiler for the LLVM AROS target")
    endif()
    enable_language(OBJC)
    if(NOT CMAKE_OBJC_COMPILER STREQUAL CMAKE_C_COMPILER)
        message(FATAL_ERROR
            "CMake changed the AROS Objective-C compiler away from the "
            "validated C compiler: '${CMAKE_OBJC_COMPILER}' != "
            "'${CMAKE_C_COMPILER}'")
    endif()
endif()
