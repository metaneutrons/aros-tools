#if !defined(_XOPEN_SOURCE) || _XOPEN_SOURCE != 700
#error explicit XOPEN source-state projection was not applied
#endif
#if !defined(SDK_XOPEN_PROJECTION) || SDK_XOPEN_PROJECTION != 29
#error XOPEN declaration definition was not applied
#endif
#if defined(SDK_PLAIN_PROJECTION) || defined(SDK_CPP_PROJECTION)
#error another declaration's source-state projection leaked into XOPEN C
#endif

#include "quoted-state.h"

#if QUOTED_SOURCE_STATE != 29
#error quoted include resolved to the wrong source-local header
#endif

int sdk_xopen_choice_symbol(void)
{
    return SDK_XOPEN_PROJECTION + QUOTED_SOURCE_STATE;
}
