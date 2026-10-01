#include "left.h"
#include "right.h"

int external_component_consumer(void)
{
    return component_left() + component_right();
}
