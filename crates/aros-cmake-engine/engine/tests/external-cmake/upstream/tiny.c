#include "tiny.h"
#include "fixture-abi.h"

int external_tiny(void)
{
    return EXTERNAL_TINY_VALUE + FIXTURE_ABI_SIZE;
}
