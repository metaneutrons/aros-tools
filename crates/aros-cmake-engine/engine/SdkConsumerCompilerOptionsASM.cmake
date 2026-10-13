# Included by CMakeASMInformation after toolchain/platform initialization and
# before ENV{ASMFLAGS} and CMAKE_ASM_FLAGS_INIT seed the cache.
include("${CMAKE_CURRENT_LIST_DIR}/NativeConsumerContract.cmake")
aros_apply_sdk_consumer_compiler_options_for_language(ASM)
