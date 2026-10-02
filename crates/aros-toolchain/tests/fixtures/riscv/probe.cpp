#include <exec/types.h>
#include <vector>

#ifndef RV2_POINTER_BYTES
#error An explicit target pointer width is required
#endif
static_assert(sizeof(APTR) == RV2_POINTER_BYTES, "selected AROS pointer ABI");
extern "C" ULONG rv2_assembly(ULONG value);
extern "C" ULONG rv2_cxx(ULONG value)
{
    std::vector<ULONG> values(3, value);
    /* The opaque assembly call prevents allocation elision. */
    return rv2_assembly((ULONG)values.data()) + values[2];
}
