#ifndef RUNTIME_RUNNER_STUB_DOS_DOS_H
#define RUNTIME_RUNNER_STUB_DOS_DOS_H

#include <stdint.h>

typedef uintptr_t BPTR;
typedef uint32_t ULONG;
typedef int32_t LONG;
typedef unsigned char UBYTE;
typedef const UBYTE *CONST_STRPTR;
typedef UBYTE *STRPTR;

#define RETURN_OK 0
#define RETURN_FAIL 20

BPTR LoadSeg(CONST_STRPTR name);
LONG RunCommand(BPTR segment, ULONG stack_size, STRPTR arguments,
    ULONG argument_length);
void UnLoadSeg(BPTR segment);

#endif
