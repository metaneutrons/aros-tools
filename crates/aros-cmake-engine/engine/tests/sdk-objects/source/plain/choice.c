#if !defined(SDK_PLAIN_PROJECTION) || SDK_PLAIN_PROJECTION != 17
#error plain source-state projection was not applied
#endif
#if defined(_XOPEN_SOURCE) || defined(SDK_XOPEN_PROJECTION) || defined(SDK_CPP_PROJECTION)
#error another declaration's source-state projection leaked into plain C
#endif

#include "quoted-state.h"

#if QUOTED_SOURCE_STATE != 17
#error quoted include resolved to the wrong source-local header
#endif

int sdk_plain_choice_symbol(void)
{
    return SDK_PLAIN_PROJECTION + QUOTED_SOURCE_STATE;
}
