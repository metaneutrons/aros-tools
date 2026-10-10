#include <namespace_order.h>
#include <generated_order.h>
#include "private_order.h"

#if AROS_NAMESPACE_ID != EXPECTED_NAMESPACE_ID
#error runtime namespace mismatch
#endif
#if AROS_GENERATED_ID != EXPECTED_GENERATED_ID
#error generated include root precedence mismatch
#endif
#if AROS_PRIVATE_ID != EXPECTED_PRIVATE_ID
#error generated private include precedence mismatch
#endif

int aros_runtime_header_namespace_probe;
