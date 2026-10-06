# Native and WebAssembly libraries

Both interfaces compile a project entirely in memory using the bundled LaTeX
format, packages and fonts. They run up to five TeX passes, with the same BibTeX
and Biber engines used by `texres`. PDF serialization includes fonts and images.
No TeX Live installation, subprocess, temporary document directory, or asset
download is needed at runtime (except by the `lazy-assets` WebAssembly build
below, whose host supplies package files on demand).

The pass count reports work actually performed: automatic convergence does not
force a redundant pass when no auxiliary state requires a rerun.

## Build

Install Rust, a C compiler, and Python 3.11+. For WebAssembly, install Clang/LLD
and the `wasm32-unknown-unknown` standard library:

```sh
# Rustup installations:
rustup target add wasm32-unknown-unknown
# Arch Linux's system Rust instead uses: sudo pacman -S rust-wasm clang lld

# Match the wasm-bindgen version recorded in Cargo.lock:
cargo install wasm-bindgen-cli --version 0.2.122 --locked --root target/libtex-tools

./scripts/build-libs.sh       # both targets
./scripts/build-libs.sh native
./scripts/build-libs.sh wasm
```

Native output is staged in `target/libtex/`: `libtex.so` and `libtex.a` on Linux,
plus `include/tex.h`. macOS produces `libtex.dylib`/`libtex.a`; Windows uses the
platform's DLL/static-library names. The `ffi-release` profile retains unwinding
so the C entry points can turn Rust panics into errors. The CLI release profile
is unchanged. Do not use the ordinary aborting `release` profile when embedding
the C library in a host that needs panic containment.

The raw module is
`target/wasm32-unknown-unknown/release/tex_wasm.wasm`. Generated `.wasm`, JavaScript,
and TypeScript declarations are in `target/wasm/web/` and `target/wasm/nodejs/`.
Use these generated packages, whose JavaScript supplies the required imports.
All package assets are embedded; expect a substantial module download. The build
script also accepts `CARGO_TARGET_DIR` and `WASM_BINDGEN` overrides.

The `lazy-assets` feature of `tex-wasm` instead leaves the package archive and
the XeLaTeX/LuaLaTeX formats out of the module (about 24 MB instead of over
700 MB; the pdfLaTeX format and the package index stay inside). The archive's
independently compressed 128 KiB chunks become separate files, and the host
returns them synchronously from `setChunkLoader`, as `scripts/build-web.sh`
and TeXres Online do.

## C API

See [`tex.h`](../crates/libtex/include/tex.h) for the ABI and ownership contract,
and [`smoke.c`](../crates/libtex/examples/smoke.c) for a complete example.

```sh
cc crates/libtex/examples/smoke.c -Itarget/libtex/include \
  -Ltarget/libtex -Wl,-rpath,"$PWD/target/libtex" -ltex -o target/libtex/smoke
target/libtex/smoke target/libtex/hello.pdf

# Linux static-link example:
cc crates/libtex/examples/smoke.c -Itarget/libtex/include \
  target/libtex/libtex.a -ldl -lpthread -lm -o target/libtex/smoke-static
target/libtex/smoke-static
```

Create a session, add named byte buffers, then call `tex_compile(session, entry,
entry_len)`. Inspect the result status before reading its PDF. Result buffers
remain valid until `tex_result_free`, even if the session is freed first. Input
buffers are copied. All strings use UTF-8 and explicit byte lengths, without
NUL terminators. Serialize use of a session; independent sessions can compile
on independent native threads.

## JavaScript API

Browser ES module:

```js
import init, { TexSession } from './web/tex.js';
await init();
const session = new TexSession();
session.addFile('main.tex', new TextEncoder().encode(String.raw`
\documentclass{article}
\begin{document}Hello from libtex.\end{document}
`));
const result = session.compile('main.tex');
if (result.status !== 0) throw new Error(result.diagnostics || result.log);
const pdf = result.pdf; // independent Uint8Array copy
const aux = result.file('main.aux');
console.log(result.fileNames, result.passes, result.bibtexRuns, result.biberRuns);
result.free();
session.free();
```

Node.js uses `const { TexSession } = require('./nodejs/tex.js')` without the
browser initialization call. Use a Node version providing `globalThis.crypto`
(Node 22+ recommended). Compilation is synchronous; run the browser module in
a Web Worker when the interface needs to stay responsive.

`addFile(name, bytes)`, `removeFile(name)`, and `setEpoch(seconds)` throw on invalid
arguments. `compile(entry)` returns a result for ordinary document errors.
`setEpoch(undefined)` restores the host clock. Copies returned by result getters
remain valid after `.free()`. A Wasm trap is an internal failure: discard that
Wasm instance rather than attempting to reuse it.

`compileWith(entry, engine, synctex)` takes `"auto"`, `"pdflatex"`, `"xelatex"`
or `"lualatex"`; with `synctex` true, the result also holds
`<job>.synctex.gz`. `result.engine` names the engine that produced the result.
A `lazy-assets` module needs `setChunkLoader(index => Uint8Array | undefined)`
and `setFormatLoader(name => Uint8Array | undefined)` before compiling. Both
loaders must answer synchronously (in a browser, from memory or a synchronous
request inside a Web Worker); a missing chunk reads as a missing package file.

## Results and project rules

| Status | Meaning |
| --- | --- |
| 0 | Success; PDF and generated files available |
| 1 | TeX, bibliography, or PDF generation error |
| 2 | Invalid entry path, missing entry, or invalid project |
| 3 | Auxiliary files did not converge within five passes |
| 4 | Internal runtime error |

`tex.h`'s `enum tex_status` names the statuses above.

The library chooses the engine from the entry file before the first pass: a
`% !TeX program = …`, `% !TeX TS-program = …`, or `%&…` directive in the leading
comment lines selects pdfTeX, XeTeX, or LuaTeX; otherwise loading `luatexja`,
`luacode`, or `luatextra`, or using `\directlua`, in the preamble selects the
LuaTeX-compatible mode; loading `fontspec`, `xeCJK`, `ctex` (or a `ctex` class),
`unicode-math`, or `polyglossia` selects XeTeX, as `texmk` does; anything else
uses pdfTeX. If a
pass fails with a log message stating that another engine is required (a
package asking for XeTeX or LuaTeX moves a pdfTeX run to XeTeX; one that needs
LuaTeX moves a pdfTeX or XeTeX run to LuaTeX), the
compilation restarts with that engine; each engine is tried at most once, each
with its own five-pass limit.

Projects use relative paths with `/` separators. The entry file's parent is the
working directory; includes resolve with ordinary TeX project semantics. Inputs
and generated files override the bundled package files. Each compile starts
fresh from the session's current inputs; generated files are retained only in
its result. Add a previous result's auxiliary files explicitly if desired.

Both interfaces expose PDF bytes, accumulated logs, rendered diagnostics,
generated files, TeX pass count, and BibTeX and Biber run counts. biblatex's
default `backend=biber` runs the built-in Biber whenever its control file or
datasources change, as `texmk` does. The default clock is the host clock; set
UTC Unix seconds for reproducible dates. Shell tools, interactive terminal
input, and operating-system file access are unavailable through the library
API; WebAssembly builds also cannot fetch URL datasources. Native `fontspec`/`xeCJK` selection runs on the XeTeX engine with the same
bundled faces and shaping path as the CLI. Supply custom font files with
`add_file` and select them by project-relative `Path`; a separate session
cannot access those files. See the [font capabilities and engine limits](../README.md#fonts-and-unicode-in-the-source-build).

## TeXres Online

`web/` is a single-user editor that runs entirely in the browser: projects in
IndexedDB, a CodeMirror 6 editor, pdf.js preview, SyncTeX in both directions,
zip import/export, and the `lazy-assets` module in a Web Worker. Package chunks
and the XeLaTeX format are downloaded when a document first needs them and are
kept in the Cache API; a service worker keeps the page usable offline.
LuaLaTeX is not offered: luaotfload needs a writable cache directory, which
the WebAssembly build lacks.

```sh
# Requires the WebAssembly toolchain above, Python 3.14+ and Node 22+.
./scripts/build-web.sh                 # writes target/web-dist/
python3 -m http.server -d target/web-dist 8000
```

Any static file server works. `node web/test/e2e.mjs --dist target/web-dist
--texres target/release/texres` drives the site in headless Chromium
(`--chromium PATH`, default `/usr/bin/chromium`): it builds a biblatex article
with a figure, an edit, an error, and a XeLaTeX document, requires PDFs
byte-identical to native `texres`, checks SyncTeX, zip round trips and reload
persistence, and prints the bytes downloaded in each phase.

## Verification

```sh
cargo test --workspace
./scripts/build-libs.sh
# Run the C smoke example above first to enable the PDF parity comparison.
node scripts/test-wasm.cjs
node scripts/test-wasm-web.mjs
```

The Rust integration suite covers nested inputs, cross-references, bibliography,
images, native font selection, errors, nonconvergence, and session isolation.
The C, Node.js Wasm, and web-target Wasm smoke programs compile real native-font
documents. The Node.js and web-target checks compare PDF bytes with the native
C output when `target/libtex/hello.pdf` exists.
