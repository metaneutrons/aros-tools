#include <namespace_order.h>
#include <generated_order.h>
#include <private_angle_order.h>
#include "private_order.h"
#include "module_order.h"

static_assert(AROS_NAMESPACE_ID == EXPECTED_NAMESPACE_ID,
              "runtime namespace mismatch");
static_assert(AROS_GENERATED_ID == EXPECTED_GENERATED_ID,
              "generated include root precedence mismatch");
static_assert(AROS_PRIVATE_ID == EXPECTED_PRIVATE_ID,
              "generated private include precedence mismatch");
static_assert(AROS_PRIVATE_ANGLE_ID == EXPECTED_PRIVATE_ANGLE_ID,
              "implicit private angle namespace mismatch");
static_assert(AROS_MODULE_PRIVATE_ID == EXPECTED_MODULE_PRIVATE_ID,
              "quoted module fallback mismatch");

int aros_runtime_header_namespace_probe;
