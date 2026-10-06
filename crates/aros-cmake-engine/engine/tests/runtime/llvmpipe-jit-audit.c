/* Tools-owned, opt-in observation of actual LLVM11 MCJIT addresses. */
#define DEBUG 1
#include <aros/debug.h>
#include <llvm-c/Core.h>
#include <llvm-c/ExecutionEngine.h>

extern void *__real_LLVMGetPointerToGlobal(LLVMExecutionEngineRef engine,
                                         LLVMValueRef global);
void *__wrap_LLVMGetPointerToGlobal(LLVMExecutionEngineRef engine,
                                   LLVMValueRef global);

void *__wrap_LLVMGetPointerToGlobal(LLVMExecutionEngineRef engine,
                                   LLVMValueRef global)
{
    void *address = __real_LLVMGetPointerToGlobal(engine, global);
    const char *name = LLVMGetValueName(global);
    bug("[llvmpipe-mcjit] function=%s address=%p\n", name ? name : "<unnamed>", address);
    return address;
}
