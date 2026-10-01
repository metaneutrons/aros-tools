#include <aros/debug.h>
#include <dos/dos.h>

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define TEST_SEGMENT ((BPTR)0x1234)

static const char *test_case(void)
{
    const char *value = getenv("RUNNER_TEST_CASE");
    return value ? value : "";
}

BPTR LoadSeg(CONST_STRPTR name)
{
    printf("LOAD path=%s\n", name ? (const char *)name : "(null)");
    fflush(stdout);
    if (strcmp(test_case(), "load-failure") == 0)
        return (BPTR)0;
    return TEST_SEGMENT;
}

LONG RunCommand(BPTR segment, ULONG stack_size, STRPTR arguments,
    ULONG argument_length)
{
    const char *argument_bytes = arguments && arguments[0] == '\n' &&
        arguments[1] == '\0' ? "0a00" : "unexpected";
    const char *result_text = getenv("RUNNER_TEST_RESULT");
    char *end = NULL;
    long result = result_text ? strtol(result_text, &end, 10) : 0;

    if (!result_text || end == result_text || *end != '\0')
        result = 0;

    printf("RUN segment=%llu stack=%llu args=%s length=%d\n",
        (unsigned long long)segment, (unsigned long long)stack_size,
        argument_bytes, (int)argument_length);
    fflush(stdout);
    return (LONG)result;
}

void UnLoadSeg(BPTR segment)
{
    printf("UNLOAD segment=%llu\n", (unsigned long long)segment);
    fflush(stdout);
}

void bug(const char *format, ...)
{
    va_list arguments;

    va_start(arguments, format);
    fputs("BUG:", stdout);
    vprintf(format, arguments);
    va_end(arguments);
    fflush(stdout);
}
