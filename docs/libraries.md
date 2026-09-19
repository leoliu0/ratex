# Native and WebAssembly libraries

Both interfaces compile a project entirely in memory using the bundled LaTeX
format, packages and fonts. They run up to five TeX passes, with the same BibTeX
engine used by `ratex`. PDF serialization includes fonts and images. No TeX Live
installation, subprocess, temporary document directory, or asset download is
needed at runtime.

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
console.log(result.fileNames, result.passes, result.bibtexRuns);
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

## Results and project rules

| Status | Meaning |
| --- | --- |
| 0 | Success; PDF and generated files available |
| 1 | TeX, bibliography, or PDF generation error |
| 2 | Invalid entry path, missing entry, or invalid project |
| 3 | Auxiliary files did not converge within five passes |
| 4 | Internal runtime error |

Projects use relative paths with `/` separators. The entry file's parent is the
working directory; includes resolve with ordinary TeX project semantics. Inputs
and generated files override the bundled package files. Each compile starts
fresh from the session's current inputs; generated files are retained only in
its result. Add a previous result's auxiliary files explicitly if desired.

Both interfaces expose PDF bytes, accumulated logs, rendered diagnostics,
generated files, TeX pass count, and BibTeX run count. The default clock is the
host clock; set UTC Unix seconds for reproducible dates. Shell tools, Biber,
interactive terminal input, and operating-system file access are unavailable
through the library API. This is the existing pdfLaTeX-compatible engine, with
its existing TeX compatibility limits.

## Verification

```sh
cargo test --workspace
./scripts/build-libs.sh
# Run the C smoke example above first to enable the PDF parity comparison.
node scripts/test-wasm.cjs
node scripts/test-wasm-web.mjs
```

The Rust integration suite covers nested inputs, cross-references, bibliography
generation, images/fonts, errors, nonconvergence, and isolation. The Wasm test
executes real document builds and compares the basic PDF with the native C
example when `target/libtex/hello.pdf` exists.
