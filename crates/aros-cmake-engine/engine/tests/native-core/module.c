#if defined(NATIVE_CORE_TEST_GENERATED_HEADER)
#include "native-core-generated.h"
#endif

extern int external_symbol(void);
extern int library_symbol(void);
extern int builtin_symbol(void);

int core_entry(void) {
    int result = external_symbol() + library_symbol() + builtin_symbol();
#if defined(NATIVE_CORE_TEST_GENERATED_HEADER)
    result += NATIVE_CORE_GENERATED_VALUE;
#endif
    return result;
}
