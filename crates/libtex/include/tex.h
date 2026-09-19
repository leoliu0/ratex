#ifndef RATEX_TEX_H
#define RATEX_TEX_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

typedef struct TexSession tex_session;
typedef struct TexResult tex_result;
typedef struct { const uint8_t *data; size_t len; } tex_bytes;
enum tex_status {
    TEX_SUCCESS = 0, TEX_COMPILATION_ERROR = 1, TEX_INVALID_INPUT = 2,
    TEX_NO_CONVERGENCE = 3, TEX_INTERNAL_ERROR = 4
};

/* Paths are UTF-8 relative project paths with / separators. Inputs are copied.
 * All non-null pointers must be valid for their documented lifetime and size.
 * Null buffers are valid only with length zero. Strings are NOT NUL-terminated.
 * Serialize access to each session; separate sessions may run concurrently.
 * All output views belong to their result until tex_result_free().
 * last_error belongs to the session until its next mutation or destruction.
 * Free handles exactly once; free(NULL) is allowed. Results outlive sessions.
 * Build with the ffi-release Cargo profile for Rust panic containment. */
uint32_t tex_abi_version(void);
tex_session *tex_session_new(void); /* NULL if construction fails */
void tex_session_free(tex_session *session);
uint32_t tex_session_add_file(tex_session *, const uint8_t *name, size_t name_len, const uint8_t *data, size_t data_len);
uint32_t tex_session_remove_file(tex_session *, const uint8_t *name, size_t name_len);
uint32_t tex_session_set_epoch(tex_session *, int64_t epoch); /* -1 = host clock; otherwise UTC Unix seconds */
tex_bytes tex_session_last_error(const tex_session *);
tex_result *tex_compile(const tex_session *, const uint8_t *entry, size_t entry_len);
void tex_result_free(tex_result *);
uint32_t tex_result_status(const tex_result *);
uint32_t tex_result_passes(const tex_result *);
uint32_t tex_result_bibtex_runs(const tex_result *);
tex_bytes tex_result_pdf(const tex_result *);
tex_bytes tex_result_log(const tex_result *);
tex_bytes tex_result_diagnostics(const tex_result *);
size_t tex_result_file_count(const tex_result *);
/* Files are sorted by project-relative name. Out-of-range indices return {NULL,0}. */
tex_bytes tex_result_file_name(const tex_result *, size_t index);
tex_bytes tex_result_file_data(const tex_result *, size_t index);

#ifdef __cplusplus
}
#endif
#endif
