#include "tex.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
    const char *name = "main.tex";
    const char *source = "\\documentclass{article}\n"
        "\\usepackage{fontspec}\n"
        "\\setmainfont{Latin Modern Roman}\n"
        "\\begin{document}\n"
        "Native font selection in libtex C ABI: \\textbf{Bold glyphs} and \\textit{italic shapes}.\n"
        "\\end{document}\n";
    assert(tex_abi_version() == 1);
    tex_session *session = tex_session_new();
    assert(session);
    assert(tex_session_set_epoch(session, 1700000000) == TEX_SUCCESS);
    assert(tex_session_add_file(session, (const uint8_t *)name, strlen(name),
        (const uint8_t *)source, strlen(source)) == TEX_SUCCESS);
    assert(tex_session_add_file(session, NULL, 1, NULL, 0) == TEX_INVALID_INPUT);
    assert(tex_session_last_error(session).len > 0);
    tex_result *result = tex_compile(session, (const uint8_t *)name, strlen(name));
    assert(result);
    tex_session_free(session); /* result owns all its bytes */
    if (tex_result_status(result) != TEX_SUCCESS) {
        tex_bytes diagnostic = tex_result_diagnostics(result);
        fwrite(diagnostic.data, 1, diagnostic.len, stderr);
        tex_bytes log = tex_result_log(result);
        fwrite(log.data, 1, log.len, stderr);
        return 1;
    }
    tex_bytes pdf = tex_result_pdf(result);
    assert(pdf.len > 1000 && memcmp(pdf.data, "%PDF-", 5) == 0);
    assert(tex_result_passes(result) >= 2);
    assert(tex_result_file_count(result) >= 3);
    if (argc > 1) {
        FILE *output = fopen(argv[1], "wb");
        assert(output);
        assert(fwrite(pdf.data, 1, pdf.len, output) == pdf.len);
        assert(fclose(output) == 0);
    }
    printf("C ABI: native-font PDF %zu bytes, %u passes\n", pdf.len, tex_result_passes(result));
    tex_result_free(result);
    tex_result_free(NULL);
    tex_session_free(NULL);
    return 0;
}
