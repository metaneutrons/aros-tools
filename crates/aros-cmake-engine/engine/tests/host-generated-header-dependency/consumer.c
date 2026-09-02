#include <aros/i386/libcall.h>

#if AROS_FIXTURE_LIBCALL != 1
#error "generated companion header contract is not active"
#endif

int companion_fixture(void)
{
    return AROS_FIXTURE_LIBCALL;
}
