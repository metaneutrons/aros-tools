/*
 * Standalone guest probe for the AROS Gallium llvmpipe GLSL/JIT path.
 *
 * Set the global AROS variable SYS/Gallium.default to "llvmpipe" before
 * launching this program.  The probe creates a small window, renders one
 * GLSL triangle, and verifies a pixel from the rendered result.  It exits
 * immediately; no input or interactive wait is required.
 */

#define DEBUG 1
#include <aros/debug.h>

#include <exec/types.h>
#include <exec/tasks.h>
#include <intuition/intuition.h>
#include <proto/exec.h>
#include <proto/intuition.h>

#include <GL/gla.h>

#include <stdio.h>
#include <stdarg.h>
#include <string.h>
#include "llvmpipe-jit-contract.h"

#define PROBE_WIDTH  96
#define PROBE_HEIGHT 96
#define PIXEL_TOLERANCE 8

CONST_STRPTR version = (CONST_STRPTR)"$VER: llvmpipe-jit 1.0 (2026) AROS";

static GLAContext context = NULL;
static struct Window *window = NULL;

static PFNGLCREATESHADERPROC            pglCreateShader;
static PFNGLSHADERSOURCEPROC            pglShaderSource;
static PFNGLCOMPILESHADERPROC           pglCompileShader;
static PFNGLGETSHADERIVPROC             pglGetShaderiv;
static PFNGLGETSHADERINFOLOGPROC        pglGetShaderInfoLog;
static PFNGLCREATEPROGRAMPROC           pglCreateProgram;
static PFNGLATTACHSHADERPROC            pglAttachShader;
static PFNGLBINDATTRIBLOCATIONPROC      pglBindAttribLocation;
static PFNGLLINKPROGRAMPROC             pglLinkProgram;
static PFNGLGETPROGRAMIVPROC            pglGetProgramiv;
static PFNGLGETPROGRAMINFOLOGPROC       pglGetProgramInfoLog;
static PFNGLUSEPROGRAMPROC              pglUseProgram;
static PFNGLDELETEPROGRAMPROC           pglDeleteProgram;
static PFNGLDELETESHADERPROC            pglDeleteShader;
static PFNGLENABLEVERTEXATTRIBARRAYPROC pglEnableVertexAttribArray;
static PFNGLDISABLEVERTEXATTRIBARRAYPROC pglDisableVertexAttribArray;
static PFNGLVERTEXATTRIBPOINTERPROC     pglVertexAttribPointer;

static const GLchar *vertex_source =
    "attribute vec2 a_position;\n"
    "void main() {\n"
    "    gl_Position = vec4(a_position, 0.0, 1.0);\n"
    "}\n";

static const GLchar *fragment_source =
    "void main() {\n"
    "    gl_FragColor = vec4(0.25, 0.50, 0.75, 1.0);\n"
    "}\n";

static void out(const char *format, ...)
{
    char buffer[512];
    va_list arguments;

    va_start(arguments, format);
    vsnprintf(buffer, sizeof(buffer), format, arguments);
    va_end(arguments);

    printf("%s", buffer);
    fflush(stdout);
    bug("%s", buffer);
}

static int load_gl_procs(void)
{
#define LOAD_GL_PROC(variable, type, name) \
    do { \
        variable = (type)glAGetProcAddress((const GLubyte *)name); \
        if (!(variable)) { \
            out("[llvmpipe-jit] FAIL: missing GL entry point %s\n", name); \
            return 0; \
        } \
    } while (0)

    LOAD_GL_PROC(pglCreateShader, PFNGLCREATESHADERPROC, "glCreateShader");
    LOAD_GL_PROC(pglShaderSource, PFNGLSHADERSOURCEPROC, "glShaderSource");
    LOAD_GL_PROC(pglCompileShader, PFNGLCOMPILESHADERPROC, "glCompileShader");
    LOAD_GL_PROC(pglGetShaderiv, PFNGLGETSHADERIVPROC, "glGetShaderiv");
    LOAD_GL_PROC(pglGetShaderInfoLog, PFNGLGETSHADERINFOLOGPROC, "glGetShaderInfoLog");
    LOAD_GL_PROC(pglCreateProgram, PFNGLCREATEPROGRAMPROC, "glCreateProgram");
    LOAD_GL_PROC(pglAttachShader, PFNGLATTACHSHADERPROC, "glAttachShader");
    LOAD_GL_PROC(pglBindAttribLocation, PFNGLBINDATTRIBLOCATIONPROC, "glBindAttribLocation");
    LOAD_GL_PROC(pglLinkProgram, PFNGLLINKPROGRAMPROC, "glLinkProgram");
    LOAD_GL_PROC(pglGetProgramiv, PFNGLGETPROGRAMIVPROC, "glGetProgramiv");
    LOAD_GL_PROC(pglGetProgramInfoLog, PFNGLGETPROGRAMINFOLOGPROC, "glGetProgramInfoLog");
    LOAD_GL_PROC(pglUseProgram, PFNGLUSEPROGRAMPROC, "glUseProgram");
    LOAD_GL_PROC(pglDeleteProgram, PFNGLDELETEPROGRAMPROC, "glDeleteProgram");
    LOAD_GL_PROC(pglDeleteShader, PFNGLDELETESHADERPROC, "glDeleteShader");
    LOAD_GL_PROC(pglEnableVertexAttribArray, PFNGLENABLEVERTEXATTRIBARRAYPROC,
                 "glEnableVertexAttribArray");
    LOAD_GL_PROC(pglDisableVertexAttribArray, PFNGLDISABLEVERTEXATTRIBARRAYPROC,
                 "glDisableVertexAttribArray");
    LOAD_GL_PROC(pglVertexAttribPointer, PFNGLVERTEXATTRIBPOINTERPROC,
                 "glVertexAttribPointer");

#undef LOAD_GL_PROC
    return 1;
}

static void print_shader_log(const char *label, GLuint object, int is_program)
{
    GLchar log[512];
    GLsizei length = 0;

    log[0] = '\0';
    if (is_program)
        pglGetProgramInfoLog(object, (GLsizei)sizeof(log), &length, log);
    else
        pglGetShaderInfoLog(object, (GLsizei)sizeof(log), &length, log);

    if (length > 0)
        out("[llvmpipe-jit] %s log: %s\n", label, (const char *)log);
}

static GLuint compile_shader(GLenum type, const GLchar *source, const char *label)
{
    GLuint shader;
    GLint compiled = GL_FALSE;

    shader = pglCreateShader(type);
    if (!shader)
    {
        out("[llvmpipe-jit] FAIL: glCreateShader failed for %s\n", label);
        return 0;
    }

    pglShaderSource(shader, 1, &source, NULL);
    pglCompileShader(shader);
    pglGetShaderiv(shader, GL_COMPILE_STATUS, &compiled);
    if (compiled != GL_TRUE)
    {
        out("[llvmpipe-jit] FAIL: %s shader compilation failed\n", label);
        print_shader_log(label, shader, 0);
        pglDeleteShader(shader);
        return 0;
    }

    out("[llvmpipe-jit] %s shader compiled\n", label);
    return shader;
}

static int renderer_is_expected(const GLubyte *renderer)
{
    const char *name = (const char *)renderer;

    if (!name || !strstr(name, "llvmpipe"))
    {
        out("[llvmpipe-jit] FAIL: GL_RENDERER does not contain llvmpipe\n");
        return 0;
    }
    if (!strstr(name, "LLVM 11.0.0,"))
    {
        out("[llvmpipe-jit] FAIL: GL_RENDERER does not identify LLVM 11.0.0\n");
        return 0;
    }
    return 1;
}

static int pixel_matches(const GLubyte pixel[4])
{
    static const GLint expected[4] = { 64, 128, 191, 255 };
    GLint i;

    for (i = 0; i < 4; ++i)
    {
        GLint difference = (GLint)pixel[i] - expected[i];
        if (difference < 0)
            difference = -difference;
        if (difference > PIXEL_TOLERANCE)
            return 0;
    }
    return 1;
}

int main(void)
{
    struct Screen *pubscreen = NULL;
    struct TagItem attributes[10];
    int attribute_count = 0;
    GLuint vertex_shader = 0;
    GLuint fragment_shader = 0;
    GLuint program = 0;
    GLint linked = GL_FALSE;
    GLenum error;
    const GLubyte *vendor;
    const GLubyte *renderer;
    const GLubyte *gl_version;
    GLubyte pixel[4] = { 0, 0, 0, 0 };
    static const GLfloat triangle[] = {
        -1.0f, -1.0f,
         1.0f, -1.0f,
         0.0f,  1.0f
    };
    int result = 1;

    out("[llvmpipe-jit] starting; SYS/Gallium.default must be llvmpipe\n");
    {
        struct Task *task = FindTask(NULL);
        out("[llvmpipe-jit] stack lower=%p upper=%p local=%p bytes=%lu\n",
            task->tc_SPLower, task->tc_SPUpper, &result,
            (unsigned long)((char *)task->tc_SPUpper - (char *)task->tc_SPLower));
        if ((char *)task->tc_SPUpper - (char *)task->tc_SPLower < (long)JIT_STACK_BYTES)
        {
            out("[llvmpipe-jit] FAIL: run the probe through llvmpipe-jit-runner; LLVM requires the fixture's 1 MiB stack\n");
            return 1;
        }
    }

    pubscreen = LockPubScreen(NULL);
    if (!pubscreen)
    {
        out("[llvmpipe-jit] FAIL: LockPubScreen failed\n");
        goto cleanup;
    }

    window = OpenWindowTags(NULL,
        WA_Title,         (IPTR)"llvmpipe-jit probe",
        WA_PubScreen,     (IPTR)pubscreen,
        WA_Left,          16,
        WA_Top,           48,
        WA_InnerWidth,    PROBE_WIDTH,
        WA_InnerHeight,   PROBE_HEIGHT,
        WA_Activate,      TRUE,
        WA_SimpleRefresh, TRUE,
        WA_NoCareRefresh, TRUE,
        WA_IDCMP,         0,
        TAG_DONE);
    UnlockPubScreen(NULL, pubscreen);
    pubscreen = NULL;

    if (!window)
    {
        out("[llvmpipe-jit] FAIL: OpenWindowTags failed\n");
        goto cleanup;
    }

    attributes[attribute_count].ti_Tag = GLA_Window;
    attributes[attribute_count++].ti_Data = (IPTR)window;
    attributes[attribute_count].ti_Tag = GLA_Left;
    attributes[attribute_count++].ti_Data = window->BorderLeft;
    attributes[attribute_count].ti_Tag = GLA_Top;
    attributes[attribute_count++].ti_Data = window->BorderTop;
    attributes[attribute_count].ti_Tag = GLA_Bottom;
    attributes[attribute_count++].ti_Data = window->BorderBottom;
    attributes[attribute_count].ti_Tag = GLA_Right;
    attributes[attribute_count++].ti_Data = window->BorderRight;
    attributes[attribute_count].ti_Tag = GLA_DoubleBuf;
    attributes[attribute_count++].ti_Data = GL_TRUE;
    attributes[attribute_count].ti_Tag = GLA_RGBMode;
    attributes[attribute_count++].ti_Data = GL_TRUE;
    attributes[attribute_count].ti_Tag = GLA_NoStencil;
    attributes[attribute_count++].ti_Data = GL_TRUE;
    attributes[attribute_count].ti_Tag = GLA_NoAccum;
    attributes[attribute_count++].ti_Data = GL_TRUE;
    attributes[attribute_count].ti_Tag = TAG_DONE;

    context = glACreateContext(attributes);
    if (!context)
    {
        out("[llvmpipe-jit] FAIL: glACreateContext failed\n");
        goto cleanup;
    }
    glAMakeCurrent(context);

    vendor = glGetString(GL_VENDOR);
    renderer = glGetString(GL_RENDERER);
    gl_version = glGetString(GL_VERSION);
    out("[llvmpipe-jit] GL_VENDOR:   %s\n",
        vendor ? (const char *)vendor : "<null>");
    out("[llvmpipe-jit] GL_RENDERER: %s\n",
        renderer ? (const char *)renderer : "<null>");
    out("[llvmpipe-jit] GL_VERSION:  %s\n",
        gl_version ? (const char *)gl_version : "<null>");

    if (!vendor || !gl_version)
    {
        out("[llvmpipe-jit] FAIL: GL identity query returned null\n");
        goto cleanup;
    }

    if (!renderer_is_expected(renderer))
        goto cleanup;

    if (!load_gl_procs())
        goto cleanup;

    vertex_shader = compile_shader(GL_VERTEX_SHADER, vertex_source, "vertex");
    if (!vertex_shader)
        goto cleanup;

    fragment_shader = compile_shader(GL_FRAGMENT_SHADER, fragment_source, "fragment");
    if (!fragment_shader)
        goto cleanup;

    program = pglCreateProgram();
    if (!program)
    {
        out("[llvmpipe-jit] FAIL: glCreateProgram failed\n");
        goto cleanup;
    }

    pglAttachShader(program, vertex_shader);
    pglAttachShader(program, fragment_shader);
    pglBindAttribLocation(program, 0, "a_position");
    pglLinkProgram(program);
    pglGetProgramiv(program, GL_LINK_STATUS, &linked);
    if (linked != GL_TRUE)
    {
        out("[llvmpipe-jit] FAIL: shader program link failed\n");
        print_shader_log("program", program, 1);
        goto cleanup;
    }
    out("[llvmpipe-jit] shader program linked\n");

    glViewport(0, 0, PROBE_WIDTH, PROBE_HEIGHT);
    glClearColor(0.0f, 0.0f, 0.0f, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT);
    pglUseProgram(program);
    pglEnableVertexAttribArray(0);
    pglVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, triangle);
    glDrawArrays(GL_TRIANGLES, 0, 3);
    pglDisableVertexAttribArray(0);
    glFinish();
    glReadPixels(PROBE_WIDTH / 2, PROBE_HEIGHT / 2, 1, 1,
                 GL_RGBA, GL_UNSIGNED_BYTE, pixel);
    glASwapBuffers(context);

    error = glGetError();
    if (error != GL_NO_ERROR)
    {
        out("[llvmpipe-jit] FAIL: glGetError after draw/readback = 0x%04x\n",
            (unsigned int)error);
        goto cleanup;
    }

    out("[llvmpipe-jit] center RGBA: %u %u %u %u; expected 64 128 191 255 +/- %d\n",
        (unsigned int)pixel[0], (unsigned int)pixel[1],
        (unsigned int)pixel[2], (unsigned int)pixel[3], PIXEL_TOLERANCE);
    if (!pixel_matches(pixel))
    {
        out("[llvmpipe-jit] FAIL: rendered center pixel does not match shader color\n");
        goto cleanup;
    }

    result = 0;

cleanup:
    if (pubscreen)
        UnlockPubScreen(NULL, pubscreen);
    if (context)
    {
        if (pglUseProgram)
            pglUseProgram(0);
        if (program && pglDeleteProgram)
            pglDeleteProgram(program);
        if (vertex_shader && pglDeleteShader)
            pglDeleteShader(vertex_shader);
        if (fragment_shader && pglDeleteShader)
            pglDeleteShader(fragment_shader);
        glADestroyContext(context);
    }
    if (window)
        CloseWindow(window);
    if (result == 0)
        out("=== LLVMPipe LLVM 11 GLSL/JIT PROBE PASS ===\n");
    else
        out("=== LLVMPipe LLVM 11 GLSL/JIT PROBE FAIL ===\n");

    return result;
}
