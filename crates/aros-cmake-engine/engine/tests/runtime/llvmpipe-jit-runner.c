/* Tools-owned supervisor: success includes CRT exit and ELF unloading. */
#define DEBUG 1
#include <aros/debug.h>
#include <dos/dos.h>
#include <proto/dos.h>

/* LLVM's optimisation passes exceed the normal CLI stack. Keep the workload
 * requirement in this guest fixture, not in the Rust CLI or global defaults. */
#include "llvmpipe-jit-contract.h"

int main(void)
{
    BPTR segment = LoadSeg((CONST_STRPTR)"SYS:Developer/Debug/Tests/graphics/gl/llvmpipe-jit");
    UBYTE arguments[] = "\n";
    LONG result;

    if (!segment)
    {
        bug("[llvmpipe-jit] FAIL: supervisor could not load the probe\n");
        return RETURN_FAIL;
    }

    result = RunCommand(segment, JIT_STACK_BYTES, arguments, 1);
    /* UnLoadSeg exercises the same debug metadata teardown that previously
     * faulted after an otherwise successful pixel readback. */
    UnLoadSeg(segment);
    if (result != 0)
    {
        bug("[llvmpipe-jit] FAIL: probe returned %ld\n", (long)result);
        return RETURN_FAIL;
    }
    bug("=== LLVMPipe LLVM 11 GLSL/JIT PROBE EXIT PASS ===\n");
    return RETURN_OK;
}
