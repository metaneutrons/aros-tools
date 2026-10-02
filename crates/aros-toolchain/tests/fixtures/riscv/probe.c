/* Source-owned compiler qualification input, not a runtime/boot test. */
#include <exec/types.h>

#ifndef RV2_POINTER_BYTES
#error An explicit target pointer width is required
#endif
_Static_assert(sizeof(APTR) == RV2_POINTER_BYTES, "selected AROS pointer ABI");
extern ULONG rv2_cxx(ULONG value);
extern ULONG rv2_assembly(ULONG value);
#if RV2_POINTER_BYTES == 8
typedef unsigned __int128 probe_integer;
#elif RV2_POINTER_BYTES == 4
typedef unsigned long long probe_integer;
#else
#error Unsupported probe pointer width
#endif
volatile probe_integer rv2_numerator = 0x123456789abcdef0ULL;
volatile probe_integer rv2_denominator = 17ULL;

int main(void)
{
    probe_integer result = rv2_numerator / rv2_denominator;
    return (int)(rv2_cxx((ULONG)result) + rv2_assembly(3));
}
