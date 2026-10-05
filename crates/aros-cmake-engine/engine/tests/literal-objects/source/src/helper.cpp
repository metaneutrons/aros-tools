#include "value.h"
#include <system-value.h>

#if X != 2
#error "ordered -D/-U arguments were not preserved"
#endif
#ifdef LITERAL_CMAKE_GLOBAL_CXX
#error "CMake global compiler flags must not be injected"
#endif

extern "C" int literal_cpp_value()
{
    return VALUE + SYSTEM_VALUE + X;
}
