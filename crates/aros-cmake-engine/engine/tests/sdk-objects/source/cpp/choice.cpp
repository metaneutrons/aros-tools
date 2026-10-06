#if !defined(SDK_CPP_PROJECTION) || SDK_CPP_PROJECTION != 43
#error C++ declaration definition was not applied
#endif
#if defined(_XOPEN_SOURCE) || defined(SDK_PLAIN_PROJECTION) || defined(SDK_XOPEN_PROJECTION)
#error another declaration's source-state projection leaked into C++
#endif

#include "quoted-state.h"

#if QUOTED_SOURCE_STATE != 43
#error quoted include resolved to the wrong source-local header
#endif

extern "C" int sdk_cpp_choice_symbol()
{
    return SDK_CPP_PROJECTION + QUOTED_SOURCE_STATE;
}
