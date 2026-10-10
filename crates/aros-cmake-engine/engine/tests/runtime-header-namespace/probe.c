#include <namespace_order.h>
#include <generated_order.h>
#include <private_angle_order.h>
#include "private_order.h"
#include "module_order.h"

#if AROS_NAMESPACE_ID != EXPECTED_NAMESPACE_ID
#error runtime namespace mismatch
#endif
#if AROS_GENERATED_ID != EXPECTED_GENERATED_ID
#error generated include root precedence mismatch
#endif
#if AROS_PRIVATE_ID != EXPECTED_PRIVATE_ID
#error generated private include precedence mismatch
#endif
#if AROS_PRIVATE_ANGLE_ID != EXPECTED_PRIVATE_ANGLE_ID
#error implicit private angle namespace mismatch
#endif
#if AROS_MODULE_PRIVATE_ID != EXPECTED_MODULE_PRIVATE_ID
#error quoted module fallback mismatch
#endif

int aros_runtime_header_namespace_probe;
