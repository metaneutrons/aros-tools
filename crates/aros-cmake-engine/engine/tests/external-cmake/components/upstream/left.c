#include "left.h"
#include "right.h"

#ifndef EXTERNAL_COMPONENT_FIXTURE
#error "external COMPILE_DEFINES were not applied"
#endif

int component_left(void)
{
    return 41 + component_right();
}
