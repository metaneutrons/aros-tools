/*
 * Host replacement for tools/flexcat/src/getft.c.
 *
 * flexcat ships host variants of its other AmigaOS-specific parts
 * (locale_other.c, FlexCat_cat_other.h) but not of getft.c, which reads a
 * file's timestamp through AllocDosObject/Lock/Examine. Only flexcat's
 * MODIFIED option uses it, to skip work when nothing changed; stat() gives the
 * same answer on the host.
 *
 * Kept out of the vendored tree so tools/flexcat stays a clean upstream copy.
 */

#include <sys/stat.h>
#include <stdint.h>

typedef int32_t int32;

int32 getft(char *filename)
{
    struct stat st;

    if (stat(filename, &st) != 0)
        return -1;

    return (int32)st.st_mtime;
}
