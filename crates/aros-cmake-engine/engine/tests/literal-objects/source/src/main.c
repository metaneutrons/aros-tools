#include "value.h"
#include <system-value.h>

#if X != 2
#error "ordered -D/-U arguments were not preserved"
#endif
#ifdef LITERAL_CMAKE_GLOBAL_C
#error "CMake global compiler flags must not be injected"
#endif

int literal_c_value(void)
{
    return VALUE + SYSTEM_VALUE + X;
}
