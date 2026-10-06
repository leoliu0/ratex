#!/usr/bin/env bash
# Build TeXres Online, the static in-browser editor (web/), into
# $CARGO_TARGET_DIR/web-dist (default target/web-dist) or the given directory.
# Requires the WebAssembly toolchain of docs/libraries.md, Python 3.14+ and Node 22+.
set -euo pipefail
cd "$(dirname "$0")/.."

target_dir="${CARGO_TARGET_DIR:-target}"
dist="${1:-$target_dir/web-dist}"

bindgen="${WASM_BINDGEN:-$target_dir/libtex-tools/bin/wasm-bindgen}"
if [[ ! -x "$bindgen" ]]; then bindgen=wasm-bindgen; fi
required=$(python3 -c 'import tomllib; print(next(p["version"] for p in tomllib.load(open("Cargo.lock", "rb"))["package"] if p["name"] == "wasm-bindgen"))')
if [[ "$("$bindgen" --version 2>/dev/null)" != "wasm-bindgen $required" ]]; then
    echo "Install wasm-bindgen-cli $required (see docs/libraries.md)." >&2
    exit 1
fi

# The module embeds the engine, the pdfLaTeX format and the package index;
# package chunks and the other formats become separate files.
messages="$target_dir/web-cargo-messages.json"
cargo build --locked --release -p tex-wasm --target wasm32-unknown-unknown --features lazy-assets \
    --message-format=json-render-diagnostics ${CARGO_BUILD_JOBS:+-j "$CARGO_BUILD_JOBS"} >"$messages"
kpse_out=$(python3 - "$messages" <<'EOF'
import json, sys
dirs = [m["out_dir"] for m in map(json.loads, open(sys.argv[1]))
        if m.get("reason") == "build-script-executed" and "/crates/tex-kpse#" in m["package_id"]]
if len(dirs) != 1:
    sys.exit(f"expected one tex-kpse build script run, found {dirs}")
print(dirs[0])
EOF
)
rm -f "$messages"

wasm_out="$target_dir/web-wasm"
"$bindgen" "$target_dir/wasm32-unknown-unknown/release/tex_wasm.wasm" --target web \
    --out-dir "$wasm_out" --out-name tex --remove-name-section --remove-producers-section

mkdir -p "$dist"
python3 scripts/web_assets.py --kpse-out "$kpse_out" --formats crates/tex-cli/assets \
    --dist "$dist" --manifest "$target_dir/web-assets.json"

(cd web && npm ci --no-audit --no-fund --loglevel=error)
node web/build.mjs --dist "$dist" --wasm "$wasm_out" --assets "$target_dir/web-assets.json"
echo "TeXres Online: $dist (serve it with any static file server, e.g. python3 -m http.server -d $dist)"
