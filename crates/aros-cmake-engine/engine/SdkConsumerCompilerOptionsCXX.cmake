# Included by CMakeCXXInformation after toolchain/platform initialization and
# before ENV{CXXFLAGS} and CMAKE_CXX_FLAGS_INIT seed the cache.
include("${CMAKE_CURRENT_LIST_DIR}/NativeConsumerContract.cmake")
aros_apply_sdk_consumer_compiler_options_for_language(CXX)
