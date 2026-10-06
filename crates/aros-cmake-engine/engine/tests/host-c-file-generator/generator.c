#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int make_path(char *buffer, size_t capacity, const char *directory,
                     const char *name)
{
    int length = snprintf(buffer, capacity, "%s/%s", directory, name);
    return length >= 0 && (size_t)length < capacity;
}

static int write_file(const char *path, const char *contents)
{
    FILE *file = fopen(path, "wb");
    if (file == NULL)
        return 1;
    size_t length = strlen(contents);
    int failed = fwrite(contents, 1, length, file) != length;
    if (fclose(file) != 0)
        failed = 1;
    return failed;
}

int main(int argc, char **argv)
{
    char input_path[4096];
    char output_path[4096];
    char extra_path[4096];
    char input[128];
    const char *run_log = getenv("HCF_TEST_RUN_LOG");

    if (argc != 4 || !make_path(input_path, sizeof(input_path), argv[1], "payload.txt") ||
        !make_path(output_path, sizeof(output_path), argv[2], argv[3]) ||
        !make_path(extra_path, sizeof(extra_path), argv[2], "unexpected.bin"))
        return 2;
    if (run_log != NULL) {
        FILE *log = fopen(run_log, "ab");
        if (log == NULL)
            return 3;
        fputs("run\n", log);
        fclose(log);
    }

    FILE *file = fopen(input_path, "rb");
    if (file == NULL)
        return 4;
    if (fgets(input, sizeof(input), file) == NULL) {
        fclose(file);
        return 5;
    }
    fclose(file);
    input[strcspn(input, "\r\n")] = '\0';

    if (strcmp(input, "NO_OUTPUT") == 0)
        return 0;
    if (strcmp(input, "FAIL") == 0) {
        (void)write_file(output_path, "partial output\n");
        return 7;
    }
    char output[256];
    int length = snprintf(output, sizeof(output), "generated:%s\n", input);
    if (length < 0 || (size_t)length >= sizeof(output) ||
        write_file(output_path, output) != 0)
        return 8;
    if (strcmp(input, "EXTRA_OUTPUT") == 0 &&
        write_file(extra_path, "unexpected\n") != 0)
        return 9;
    return 0;
}
