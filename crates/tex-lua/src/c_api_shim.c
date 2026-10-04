#include <setjmp.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct lua_State lua_State;
typedef int (*lua_CFunction)(lua_State *state);
typedef double lua_Number;
typedef int64_t lua_Integer;
typedef uint64_t lua_Unsigned;
typedef void (*lua_Hook)(lua_State *state, void *record);
typedef intptr_t lua_KContext;
typedef int (*lua_KFunction)(lua_State *state, int status, lua_KContext context);
typedef struct luaL_Reg {
    const char *name;
    lua_CFunction function;
} luaL_Reg;
typedef const char *(*lua_Reader)(lua_State *state, void *data, size_t *size);
typedef int (*lua_Writer)(
    lua_State *state,
    const void *bytes,
    size_t size,
    void *data
);
typedef void *(*lua_Alloc)(
    void *userdata,
    void *pointer,
    size_t old_size,
    size_t new_size
);

const char lua_ident[] = "$LuaVersion: Lua 5.3-compatible TeXres $";

extern const char *lua_pushlstring(lua_State *, const char *, size_t);
extern void lua_concat(lua_State *, int);
extern void luaL_where(lua_State *, int);
extern int tex_lua_capture_error(lua_State *);
extern int lua_type(lua_State *, int);
extern lua_CFunction tex_lua_getpanic(lua_State *);
extern const char *lua_typename(lua_State *, int);
extern const char *lua_tolstring(lua_State *, int, size_t *);
extern lua_Number lua_tonumberx(lua_State *, int, int *);
extern lua_Integer lua_tointegerx(lua_State *, int, int *);
extern int lua_checkstack(lua_State *, int);
extern void *luaL_testudata(lua_State *, int, const char *);
extern const lua_Number *lua_version(lua_State *);
extern int luaL_getsubtable(lua_State *, int, const char *);
extern int lua_getfield(lua_State *, int, const char *);
extern int lua_toboolean(lua_State *, int);
extern void lua_settop(lua_State *, int);
extern void lua_createtable(lua_State *, int, int);
extern void luaL_setfuncs(lua_State *, const luaL_Reg *, int);
extern void lua_pushcclosure(lua_State *, lua_CFunction, int);
extern const char *lua_pushstring(lua_State *, const char *);
extern void lua_pushvalue(lua_State *, int);
extern void lua_setfield(lua_State *, int, const char *);
extern void lua_rotate(lua_State *, int, int);
extern void lua_setglobal(lua_State *, const char *);
extern int luaL_getmetafield(lua_State *, int, const char *);
extern int lua_getmetatable(lua_State *, int);
extern int lua_setmetatable(lua_State *, int);
extern void *lua_touserdata(lua_State *, int);
extern int lua_rawequal(lua_State *, int, int);
void luaL_checkstack(lua_State *, int, const char *);
extern int lua_absindex(lua_State *, int);
extern int lua_isstring(lua_State *, int);
extern const void *lua_topointer(lua_State *, int);
const char *lua_pushfstring(lua_State *, const char *, ...);
int luaL_error(lua_State *, const char *, ...);
extern int tex_lua_yieldk_impl(
    lua_State *,
    int,
    lua_KContext,
    lua_KFunction
);
extern int tex_lua_callk_impl(
    lua_State *,
    int,
    int,
    lua_KContext,
    lua_KFunction
);
extern int tex_lua_pcallk_impl(
    lua_State *,
    int,
    int,
    int,
    lua_KContext,
    lua_KFunction
);
extern int tex_lua_arith_impl(lua_State *, int);
extern int tex_lua_concat_impl(lua_State *, int);
extern int tex_lua_len_impl(lua_State *, int);
extern int tex_lua_compare_impl(lua_State *, int, int, int, int *);
extern int tex_lua_getglobal_impl(lua_State *, const char *);
extern int tex_lua_gettable_impl(lua_State *, int);
extern int tex_lua_geti_impl(lua_State *, int, lua_Integer);
extern int tex_lua_setglobal_impl(lua_State *, const char *);
extern int tex_lua_settable_impl(lua_State *, int);
extern int tex_lua_seti_impl(lua_State *, int, lua_Integer);
extern int tex_lua_next_impl(lua_State *, int, int *);
extern int lua_rawget(lua_State *, int);
extern void lua_pushnil(lua_State *);
extern int lua_isnumber(lua_State *, int);
extern int lua_isinteger(lua_State *, int);
extern int lua_gettop(lua_State *);
extern void lua_copy(lua_State *, int, int);

#define LUA_IDSIZE 60
typedef struct lua_Debug {
    int event;
    const char *name;
    const char *namewhat;
    const char *what;
    const char *source;
    int currentline;
    int linedefined;
    int lastlinedefined;
    unsigned char nups;
    unsigned char nparams;
    char isvararg;
    char istailcall;
    char short_src[LUA_IDSIZE];
    void *i_ci;
} lua_Debug;
extern int lua_getstack(lua_State *, int, lua_Debug *);
extern int lua_getinfo(lua_State *, const char *, lua_Debug *);


void *tex_lua_default_alloc(
    void *userdata,
    void *pointer,
    size_t old_size,
    size_t new_size
) {
    (void)userdata;
    (void)old_size;
    if (new_size == 0) {
        free(pointer);
        return NULL;
    }
    return realloc(pointer, new_size);
}
#define LUA_ERRRUN 2
#define LUA_YIELD 1
#define LUA_ERRMEM 4
#define LUA_REGISTRYINDEX (-1001000)
#define LUA_TNONE (-1)
#define LUA_TNIL 0
#define LUA_TBOOLEAN 1
#define LUA_TLIGHTUSERDATA 2
#define LUA_TNUMBER 3
#define LUA_TSTRING 4
#define LUA_TTABLE 5

struct tex_lua_jump_frame {
    jmp_buf environment;
    struct tex_lua_jump_frame *previous;
};

#if defined(_MSC_VER)
#define THREAD_LOCAL __declspec(thread)
#else
#define THREAD_LOCAL _Thread_local
#endif

static THREAD_LOCAL struct tex_lua_jump_frame *tex_lua_active_jump;
int tex_lua_invoke_c(lua_CFunction function, lua_State *state, int *result) {
    struct tex_lua_jump_frame frame;
    frame.previous = tex_lua_active_jump;
    tex_lua_active_jump = &frame;
    int status = setjmp(frame.environment);
    if (status == 0) {
        *result = function(state);
        tex_lua_active_jump = frame.previous;
        return 0;
    }
    tex_lua_active_jump = frame.previous;
    return status;
}

int tex_lua_invoke_k(
    lua_KFunction continuation,
    lua_State *state,
    int status,
    lua_KContext context,
    int *result
) {
    struct tex_lua_jump_frame frame;
    frame.previous = tex_lua_active_jump;
    tex_lua_active_jump = &frame;
    int jump_status = setjmp(frame.environment);
    if (jump_status == 0) {
        *result = continuation(state, status, context);
        tex_lua_active_jump = frame.previous;
        return 0;
    }
    tex_lua_active_jump = frame.previous;
    return jump_status;
}

int tex_lua_invoke_hook(lua_Hook hook, lua_State *state, void *record) {
    struct tex_lua_jump_frame frame;
    frame.previous = tex_lua_active_jump;
    tex_lua_active_jump = &frame;
    int status = setjmp(frame.environment);
    if (status == 0) {
        hook(state, record);
        tex_lua_active_jump = frame.previous;
        return 0;
    }
    tex_lua_active_jump = frame.previous;
    return status;
}

int tex_lua_invoke_reader(
    lua_Reader reader,
    lua_State *state,
    void *data,
    const char **result,
    size_t *length
) {
    struct tex_lua_jump_frame frame;
    frame.previous = tex_lua_active_jump;
    tex_lua_active_jump = &frame;
    int status = setjmp(frame.environment);
    if (status == 0) {
        *result = reader(state, data, length);
        tex_lua_active_jump = frame.previous;
        return 0;
    }
    tex_lua_active_jump = frame.previous;
    return status;
}

int tex_lua_invoke_writer(
    lua_Writer writer,
    lua_State *state,
    const void *bytes,
    size_t length,
    void *data,
    int *result
) {
    struct tex_lua_jump_frame frame;
    frame.previous = tex_lua_active_jump;
    tex_lua_active_jump = &frame;
    int status = setjmp(frame.environment);
    if (status == 0) {
        *result = writer(state, bytes, length, data);
        tex_lua_active_jump = frame.previous;
        return 0;
    }
    tex_lua_active_jump = frame.previous;
    return status;
}

int lua_error(lua_State *state) {
    tex_lua_capture_error(state);
    if (tex_lua_active_jump != NULL) {
        longjmp(tex_lua_active_jump->environment, LUA_ERRRUN);
    }
    lua_CFunction panic_function = tex_lua_getpanic(state);
    if (panic_function != NULL) panic_function(state);
    abort();
}

int tex_lua_raise_error(lua_State *state) {
    return lua_error(state);
}

int lua_yieldk(
    lua_State *state,
    int nresults,
    lua_KContext context,
    lua_KFunction continuation
) {
    int status = tex_lua_yieldk_impl(state, nresults, context, continuation);
    /* Like lua_yieldk (ldo.c), a yield that is not allowed raises an error. */
    if (status != LUA_YIELD) lua_error(state);
    if (tex_lua_active_jump != NULL) {
        longjmp(tex_lua_active_jump->environment, LUA_YIELD);
    }
    return status;
}

void lua_callk(
    lua_State *state,
    int nargs,
    int nresults,
    lua_KContext context,
    lua_KFunction continuation
) {
    int status = tex_lua_callk_impl(state, nargs, nresults, context, continuation);
    if (status == LUA_YIELD && tex_lua_active_jump != NULL) {
        longjmp(tex_lua_active_jump->environment, LUA_YIELD);
    }
    if (status != 0) lua_error(state);
}

int lua_pcallk(
    lua_State *state,
    int nargs,
    int nresults,
    int error_function,
    lua_KContext context,
    lua_KFunction continuation
) {
    int status = tex_lua_pcallk_impl(
        state,
        nargs,
        nresults,
        error_function,
        context,
        continuation
    );
    if (status == LUA_YIELD && tex_lua_active_jump != NULL) {
        longjmp(tex_lua_active_jump->environment, LUA_YIELD);
    }
    return status;
}

void lua_arith(lua_State *state, int operation) {
    if (tex_lua_arith_impl(state, operation) != 0) lua_error(state);
}

void lua_concat(lua_State *state, int count) {
    if (tex_lua_concat_impl(state, count) != 0) lua_error(state);
}

void lua_len(lua_State *state, int index) {
    if (tex_lua_len_impl(state, index) != 0) lua_error(state);
}

int lua_compare(lua_State *state, int left, int right, int operation) {
    int result = 0;
    if (tex_lua_compare_impl(state, left, right, operation, &result) != 0) {
        lua_error(state);
    }
    return result;
}

lua_Integer luaL_len(lua_State *state, int index) {
    lua_len(state, index);
    int valid = 0;
    lua_Integer length = lua_tointegerx(state, -1, &valid);
    lua_settop(state, -2);
    if (!valid) luaL_error(state, "object length is not an integer");
    return length;
}

void luaL_requiref(
    lua_State *state,
    const char *module_name,
    lua_CFunction open_function,
    int global
) {
    luaL_getsubtable(state, LUA_REGISTRYINDEX, "_LOADED");
    lua_getfield(state, -1, module_name);
    if (!lua_toboolean(state, -1)) {
        lua_settop(state, -2);
        lua_pushcclosure(state, open_function, 0);
        lua_pushstring(state, module_name);
        lua_callk(state, 1, 1, 0, NULL);
        lua_pushvalue(state, -1);
        lua_setfield(state, -3, module_name);
    }
    lua_rotate(state, -2, -1);
    lua_settop(state, -2);
    if (global) {
        lua_pushvalue(state, -1);
        lua_setglobal(state, module_name);
    }
}

int lua_getglobal(lua_State *state, const char *name) {
    if (tex_lua_getglobal_impl(state, name) != 0) lua_error(state);
    return lua_type(state, -1);
}

int lua_gettable(lua_State *state, int index) {
    if (tex_lua_gettable_impl(state, index) != 0) lua_error(state);
    return lua_type(state, -1);
}

int lua_getfield(lua_State *state, int index, const char *key) {
    int absolute = lua_absindex(state, index);
    lua_pushstring(state, key);
    return lua_gettable(state, absolute);
}

int lua_geti(lua_State *state, int index, lua_Integer key) {
    if (tex_lua_geti_impl(state, index, key) != 0) lua_error(state);
    return lua_type(state, -1);
}

void lua_setglobal(lua_State *state, const char *name) {
    if (tex_lua_setglobal_impl(state, name) != 0) lua_error(state);
}

void lua_settable(lua_State *state, int index) {
    if (tex_lua_settable_impl(state, index) != 0) lua_error(state);
}

void lua_setfield(lua_State *state, int index, const char *key) {
    int absolute = lua_absindex(state, index);
    lua_pushstring(state, key);
    lua_rotate(state, -2, 1);
    lua_settable(state, absolute);
}

void lua_seti(lua_State *state, int index, lua_Integer key) {
    if (tex_lua_seti_impl(state, index, key) != 0) lua_error(state);
}

int lua_next(lua_State *state, int index) {
    int result = 0;
    if (tex_lua_next_impl(state, index, &result) != 0) lua_error(state);
    return result;
}

int luaL_newmetatable(lua_State *state, const char *name) {
    lua_getfield(state, LUA_REGISTRYINDEX, name);
    if (lua_type(state, -1) != 0) return 0;
    lua_settop(state, -2);
    lua_createtable(state, 0, 2);
    lua_pushstring(state, name);
    lua_setfield(state, -2, "__name");
    lua_pushvalue(state, -1);
    lua_setfield(state, LUA_REGISTRYINDEX, name);
    return 1;
}

void luaL_setmetatable(lua_State *state, const char *name) {
    lua_getfield(state, LUA_REGISTRYINDEX, name);
    lua_setmetatable(state, -2);
}

void *luaL_testudata(lua_State *state, int argument, const char *name) {
    void *pointer = lua_touserdata(state, argument);
    if (pointer == NULL || !lua_getmetatable(state, argument)) return NULL;
    lua_getfield(state, LUA_REGISTRYINDEX, name);
    int equal = lua_rawequal(state, -1, -2);
    lua_settop(state, -3);
    return equal ? pointer : NULL;
}

void luaL_setfuncs(lua_State *state, const luaL_Reg *functions, int upvalue_count) {
    luaL_checkstack(state, upvalue_count, "too many upvalues");
    for (; functions != NULL && functions->name != NULL; functions++) {
        for (int index = 0; index < upvalue_count; index++) {
            lua_pushvalue(state, -upvalue_count);
        }
        lua_pushcclosure(state, functions->function, upvalue_count);
        lua_setfield(state, -(upvalue_count + 2), functions->name);
    }
    lua_settop(state, -(upvalue_count + 1));
}

int luaL_getsubtable(lua_State *state, int index, const char *field) {
    int absolute = lua_absindex(state, index);
    if (lua_getfield(state, absolute, field) == LUA_TTABLE) return 1;
    lua_settop(state, -2);
    lua_createtable(state, 0, 0);
    lua_pushvalue(state, -1);
    lua_setfield(state, absolute, field);
    return 0;
}

void luaL_pushmodule(lua_State *state, const char *module_name, int size_hint) {
    luaL_getsubtable(state, LUA_REGISTRYINDEX, "_LOADED");
    if (lua_getfield(state, -1, module_name) != LUA_TTABLE) {
        lua_settop(state, -2);
        lua_createtable(state, 0, size_hint);
        lua_pushvalue(state, -1);
        lua_setfield(state, -3, module_name);
    }
    lua_rotate(state, -2, -1);
    lua_settop(state, -2);
}

void luaL_openlib(
    lua_State *state,
    const char *library_name,
    const luaL_Reg *functions,
    int upvalue_count
) {
    if (library_name != NULL) {
        luaL_pushmodule(state, library_name, 0);
        lua_rotate(state, -(upvalue_count + 1), 1);
    }

    if (functions != NULL) {
        luaL_setfuncs(state, functions, upvalue_count);
    } else {
        lua_settop(state, -(upvalue_count + 1));
    }
}
/* The lauxlib functions below follow lauxlib.c (Lua 5.3.6). */

int luaL_getmetafield(lua_State *state, int object_index, const char *event) {
    if (!lua_getmetatable(state, object_index)) return LUA_TNIL;
    lua_pushstring(state, event);
    int kind = lua_rawget(state, -2);
    if (kind == LUA_TNIL) {
        lua_settop(state, -3);
    } else {
        lua_rotate(state, -2, -1);
        lua_settop(state, -2);
    }
    return kind;
}

int luaL_callmeta(lua_State *state, int object_index, const char *event) {
    int absolute = lua_absindex(state, object_index);
    if (luaL_getmetafield(state, absolute, event) == LUA_TNIL) return 0;
    lua_pushvalue(state, absolute);
    lua_callk(state, 1, 1, 0, NULL);
    return 1;
}

const char *luaL_tolstring(lua_State *state, int index, size_t *length) {
    if (luaL_callmeta(state, index, "__tostring")) {
        if (!lua_isstring(state, -1)) {
            luaL_error(state, "'__tostring' must return a string");
        }
    } else {
        switch (lua_type(state, index)) {
            case LUA_TNUMBER:
                if (lua_isinteger(state, index)) {
                    lua_pushfstring(state, "%I", lua_tointegerx(state, index, NULL));
                } else {
                    lua_pushfstring(state, "%f", lua_tonumberx(state, index, NULL));
                }
                break;
            case LUA_TSTRING:
                lua_pushvalue(state, index);
                break;
            case LUA_TBOOLEAN:
                lua_pushstring(state, lua_toboolean(state, index) ? "true" : "false");
                break;
            case LUA_TNIL:
                lua_pushstring(state, "nil");
                break;
            default: {
                int kind = luaL_getmetafield(state, index, "__name");
                const char *name = kind == LUA_TSTRING
                    ? lua_tolstring(state, -1, NULL)
                    : lua_typename(state, lua_type(state, index));
                lua_pushfstring(state, "%s: %p", name, (void *)lua_topointer(state, index));
                if (kind != LUA_TNIL) {
                    lua_rotate(state, -2, -1);
                    lua_settop(state, -2);
                }
                break;
            }
        }
    }
    return lua_tolstring(state, -1, length);
}

/* Search package.loaded (two levels deep) for the value at 'object_index'. */
static int find_field(lua_State *state, int object_index, int level) {
    if (level == 0 || lua_type(state, -1) != LUA_TTABLE) return 0;
    lua_pushnil(state);
    while (lua_next(state, -2)) {
        if (lua_type(state, -2) == LUA_TSTRING) {
            if (lua_rawequal(state, object_index, -1)) {
                lua_settop(state, -2);
                return 1;
            } else if (find_field(state, object_index, level - 1)) {
                /* stack: key table name; remove the table, join "key.name" */
                lua_rotate(state, -2, -1);
                lua_settop(state, -2);
                lua_pushstring(state, ".");
                lua_rotate(state, -2, 1);
                lua_concat(state, 3);
                return 1;
            }
        }
        lua_settop(state, -2);
    }
    return 0;
}

static int push_global_function_name(lua_State *state, lua_Debug *record) {
    int top = lua_gettop(state);
    lua_getinfo(state, "f", record);
    lua_getfield(state, LUA_REGISTRYINDEX, "_LOADED");
    if (find_field(state, top + 1, 2)) {
        const char *name = lua_tolstring(state, -1, NULL);
        if (strncmp(name, "_G.", 3) == 0) {
            lua_pushstring(state, name + 3);
            lua_rotate(state, -2, -1);
            lua_settop(state, -2);
        }
        lua_copy(state, -1, top + 1);
        lua_settop(state, -3);
        return 1;
    }
    lua_settop(state, top);
    return 0;
}

static void push_function_name(lua_State *state, lua_Debug *record) {
    if (push_global_function_name(state, record)) {
        lua_pushfstring(state, "function '%s'", lua_tolstring(state, -1, NULL));
        lua_rotate(state, -2, -1);
        lua_settop(state, -2);
    } else if (*record->namewhat != '\0') {
        lua_pushfstring(state, "%s '%s'", record->namewhat, record->name);
    } else if (*record->what == 'm') {
        lua_pushstring(state, "main chunk");
    } else if (*record->what != 'C') {
        lua_pushfstring(state, "function <%s:%d>", record->short_src, record->linedefined);
    } else {
        lua_pushstring(state, "?");
    }
}

static int last_level(lua_State *state) {
    lua_Debug record;
    int low = 1;
    int high = 1;
    while (lua_getstack(state, high, &record)) {
        low = high;
        high *= 2;
    }
    while (low < high) {
        int middle = (low + high) / 2;
        if (lua_getstack(state, middle, &record)) low = middle + 1;
        else high = middle;
    }
    return high - 1;
}

#define TRACEBACK_LEVELS1 10
#define TRACEBACK_LEVELS2 11

void luaL_traceback(lua_State *state, lua_State *source, const char *message, int level) {
    lua_Debug record;
    int top = lua_gettop(state);
    int last = last_level(source);
    int first_part = last - level > TRACEBACK_LEVELS1 + TRACEBACK_LEVELS2 ? TRACEBACK_LEVELS1 : -1;
    if (message != NULL) lua_pushfstring(state, "%s\n", message);
    luaL_checkstack(state, 10, NULL);
    lua_pushstring(state, "stack traceback:");
    while (lua_getstack(source, level++, &record)) {
        if (first_part-- == 0) {
            lua_pushstring(state, "\n\t...");
            level = last - TRACEBACK_LEVELS2 + 1;
        } else {
            lua_getinfo(source, "Slnt", &record);
            lua_pushfstring(state, "\n\t%s:", record.short_src);
            if (record.currentline > 0) lua_pushfstring(state, "%d:", record.currentline);
            lua_pushstring(state, " in ");
            push_function_name(state, &record);
            if (record.istailcall) lua_pushstring(state, "\n\t(...tail calls...)");
            lua_concat(state, lua_gettop(state) - top);
        }
    }
    lua_concat(state, lua_gettop(state) - top);
}

struct text_buffer {
    char *data;
    size_t length;
    size_t capacity;
};

static int reserve(struct text_buffer *buffer, size_t additional) {
    if (additional > SIZE_MAX - buffer->length - 1) return 0;
    size_t required = buffer->length + additional + 1;
    if (required <= buffer->capacity) return 1;
    size_t capacity = buffer->capacity ? buffer->capacity : 64;
    while (capacity < required) {
        if (capacity > SIZE_MAX / 2) { capacity = required; break; }
        capacity *= 2;
    }
    char *data = (char *)realloc(buffer->data, capacity);
    if (data == NULL) return 0;
    buffer->data = data;
    buffer->capacity = capacity;
    return 1;
}

static int append_bytes(struct text_buffer *buffer, const char *data, size_t length) {
    if (!reserve(buffer, length)) return 0;
    memcpy(buffer->data + buffer->length, data, length);
    buffer->length += length;
    buffer->data[buffer->length] = '\0';
    return 1;
}

static int append_printf(struct text_buffer *buffer, const char *format, ...) {
    char local[128];
    va_list arguments;
    va_start(arguments, format);
    int length = vsnprintf(local, sizeof(local), format, arguments);
    va_end(arguments);
    if (length < 0) return 0;
    if ((size_t)length < sizeof(local)) return append_bytes(buffer, local, (size_t)length);
    char *dynamic = (char *)malloc((size_t)length + 1);
    if (dynamic == NULL) return 0;
    va_start(arguments, format);
    vsnprintf(dynamic, (size_t)length + 1, format, arguments);
    va_end(arguments);
    int ok = append_bytes(buffer, dynamic, (size_t)length);
    free(dynamic);
    return ok;
}

/* luaO_utf8esc: write the UTF-8 bytes of 'code' at the end of 'buffer' (8 bytes). */
static int utf8_escape(char *buffer, unsigned long code) {
    int count = 1;
    if (code < 0x80) {
        buffer[7] = (char)code;
    } else {
        unsigned int first_byte_max = 0x3f;
        do {
            buffer[8 - (count++)] = (char)(0x80 | (code & 0x3f));
            code >>= 6;
            first_byte_max >>= 1;
        } while (code > first_byte_max);
        buffer[8 - count] = (char)((~first_byte_max << 1) | code);
    }
    return count;
}

static char *format_lua_string(
    const char *format,
    va_list arguments,
    size_t *length,
    char *invalid_specifier
) {
    struct text_buffer output = {0};
    if (format == NULL) format = "(null)";
    for (const char *cursor = format; *cursor != '\0'; cursor++) {
        if (*cursor != '%') {
            if (!append_bytes(&output, cursor, 1)) goto failure;
            continue;
        }
        cursor++;
        if (*cursor == '\0') {
            *invalid_specifier = '%';
            goto failure;
        }
        switch (*cursor) {
            case '%': if (!append_bytes(&output, "%", 1)) goto failure; break;
            case 's': {
                const char *value = va_arg(arguments, const char *);
                if (value == NULL) value = "(null)";
                if (!append_bytes(&output, value, strlen(value))) goto failure;
                break;
            }
            case 'c': {
                unsigned char byte = (unsigned char)va_arg(arguments, int);
                if (byte >= 0x20 && byte < 0x7f) {
                    if (!append_bytes(&output, (const char *)&byte, 1)) goto failure;
                } else if (!append_printf(&output, "<\\%d>", byte)) {
                    goto failure;
                }
                break;
            }
            case 'd': if (!append_printf(&output, "%d", va_arg(arguments, int))) goto failure; break;
            case 'I': if (!append_printf(&output, "%lld", (long long)va_arg(arguments, lua_Integer))) goto failure; break;
            case 'U': {
                /* luaO_utf8esc: the argument is a 'long' code point */
                char utf8[8];
                int count = utf8_escape(utf8, (unsigned long)va_arg(arguments, long));
                if (!append_bytes(&output, utf8 + sizeof(utf8) - count, (size_t)count)) goto failure;
                break;
            }
            case 'f': {
                /* luaO_tostring: "%.14g", plus ".0" when it looks like an integer */
                char number[64];
                int count = snprintf(number, sizeof(number), "%.14g", va_arg(arguments, lua_Number));
                if (count < 0 || (size_t)count >= sizeof(number) - 2) goto failure;
                if (number[strspn(number, "-0123456789")] == '\0') {
                    number[count++] = '.';
                    number[count++] = '0';
                }
                if (!append_bytes(&output, number, (size_t)count)) goto failure;
                break;
            }
            case 'p': if (!append_printf(&output, "%p", va_arg(arguments, void *))) goto failure; break;
            default:
                *invalid_specifier = *cursor;
                goto failure;
        }
    }
    if (output.data == NULL) {
        output.data = (char *)calloc(1, 1);
        if (output.data == NULL) return NULL;
    }
    *length = output.length;
    return output.data;

failure:
    free(output.data);
    return NULL;
}

const char *lua_pushvfstring(lua_State *state, const char *format, va_list arguments) {
    va_list copy;
    va_copy(copy, arguments);
    size_t length = 0;
    char invalid_specifier = '\0';
    char *text = format_lua_string(format, copy, &length, &invalid_specifier);
    va_end(copy);
    if (text == NULL) {
        if (invalid_specifier != '\0') {
            char message[64];
            int count = snprintf(
                message,
                sizeof(message),
                "invalid option '%%%c' to 'lua_pushfstring'",
                invalid_specifier
            );
            size_t message_length = count > 0 ? (size_t)count : 0;
            lua_pushlstring(state, message, message_length);
            lua_error(state);
            return NULL;
        }
        lua_pushlstring(state, "not enough memory", 17);
        tex_lua_capture_error(state);
        if (tex_lua_active_jump != NULL) {
            longjmp(tex_lua_active_jump->environment, LUA_ERRMEM);
        }
        return NULL;
    }
    const char *result = lua_pushlstring(state, text, length);
    free(text);
    return result;
}

const char *lua_pushfstring(lua_State *state, const char *format, ...) {
    va_list arguments;
    va_start(arguments, format);
    const char *result = lua_pushvfstring(state, format, arguments);
    va_end(arguments);
    return result;
}

int luaL_error(lua_State *state, const char *format, ...) {
    luaL_where(state, 1);
    va_list arguments;
    va_start(arguments, format);
    lua_pushvfstring(state, format, arguments);
    va_end(arguments);
    lua_concat(state, 2);
    return lua_error(state);
}

int luaL_argerror(lua_State *state, int argument, const char *message) {
    lua_Debug record;
    if (!lua_getstack(state, 0, &record)) {
        return luaL_error(state, "bad argument #%d (%s)", argument, message);
    }
    lua_getinfo(state, "n", &record);
    if (strcmp(record.namewhat, "method") == 0) {
        argument--;
        if (argument == 0) {
            return luaL_error(state, "calling '%s' on bad self (%s)", record.name, message);
        }
    }
    if (record.name == NULL) {
        record.name = push_global_function_name(state, &record)
            ? lua_tolstring(state, -1, NULL)
            : "?";
    }
    return luaL_error(state, "bad argument #%d to '%s' (%s)", argument, record.name, message);
}

static int type_error(lua_State *state, int argument, const char *expected) {
    const char *actual;
    if (luaL_getmetafield(state, argument, "__name") == LUA_TSTRING) {
        actual = lua_tolstring(state, -1, NULL);
    } else if (lua_type(state, argument) == LUA_TLIGHTUSERDATA) {
        actual = "light userdata";
    } else {
        actual = lua_typename(state, lua_type(state, argument));
    }
    const char *message = lua_pushfstring(state, "%s expected, got %s", expected, actual);
    return luaL_argerror(state, argument, message);
}

static void tag_error(lua_State *state, int argument, int tag) {
    type_error(state, argument, lua_typename(state, tag));
}

const char *luaL_checklstring(lua_State *state, int argument, size_t *length) {
    const char *value = lua_tolstring(state, argument, length);
    if (value == NULL) tag_error(state, argument, LUA_TSTRING);
    return value;
}

const char *luaL_optlstring(
    lua_State *state,
    int argument,
    const char *default_value,
    size_t *length
) {
    if (lua_type(state, argument) <= 0) {
        if (length != NULL) *length = default_value == NULL ? 0 : strlen(default_value);
        return default_value;
    }
    return luaL_checklstring(state, argument, length);
}

lua_Number luaL_checknumber(lua_State *state, int argument) {
    int valid = 0;
    lua_Number value = lua_tonumberx(state, argument, &valid);
    if (!valid) tag_error(state, argument, LUA_TNUMBER);
    return value;
}

lua_Number luaL_optnumber(lua_State *state, int argument, lua_Number default_value) {
    return lua_type(state, argument) <= 0
        ? default_value
        : luaL_checknumber(state, argument);
}

lua_Integer luaL_checkinteger(lua_State *state, int argument) {
    int valid = 0;
    lua_Integer value = lua_tointegerx(state, argument, &valid);
    if (!valid) {
        if (lua_isnumber(state, argument)) {
            luaL_argerror(state, argument, "number has no integer representation");
        } else {
            tag_error(state, argument, LUA_TNUMBER);
        }
    }
    return value;
}

lua_Integer luaL_optinteger(lua_State *state, int argument, lua_Integer default_value) {
    return lua_type(state, argument) <= 0
        ? default_value
        : luaL_checkinteger(state, argument);
}

void luaL_checktype(lua_State *state, int argument, int expected) {
    if (lua_type(state, argument) != expected) tag_error(state, argument, expected);
}

void luaL_checkany(lua_State *state, int argument) {
    if (lua_type(state, argument) == LUA_TNONE) {
        luaL_argerror(state, argument, "value expected");
    }
}

void luaL_checkstack(lua_State *state, int size, const char *message) {
    if (!lua_checkstack(state, size)) {
        if (message != NULL) luaL_error(state, "stack overflow (%s)", message);
        else luaL_error(state, "stack overflow");
    }
}

void *luaL_checkudata(lua_State *state, int argument, const char *name) {
    void *value = luaL_testudata(state, argument, name);
    if (value == NULL) type_error(state, argument, name);
    return value;
}

int luaL_checkoption(
    lua_State *state,
    int argument,
    const char *default_value,
    const char *const options[]
) {
    const char *value = default_value != NULL
        ? luaL_optlstring(state, argument, default_value, NULL)
        : luaL_checklstring(state, argument, NULL);
    for (int index = 0; options[index] != NULL; index++) {
        if (strcmp(value, options[index]) == 0) return index;
    }
    return luaL_argerror(
        state,
        argument,
        lua_pushfstring(state, "invalid option '%s'", value)
    );
}

void luaL_checkversion_(lua_State *state, lua_Number version, size_t numeric_sizes) {
    const size_t expected_sizes = sizeof(lua_Integer) * 16 + sizeof(lua_Number);
    const lua_Number *actual = lua_version(state);
    const lua_Number *core = lua_version(NULL);
    if (numeric_sizes != expected_sizes) {
        luaL_error(state, "core and library have incompatible numeric types");
    }
    if (actual != core) {
        luaL_error(state, "multiple Lua VMs detected");
    }
    if (actual == NULL || *actual != version) {
        luaL_error(
            state,
            "version mismatch: app. needs %f, Lua core provides %f",
            version,
            actual == NULL ? 0.0 : *actual
        );
    }
}
