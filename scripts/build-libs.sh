#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

mode="${1:-all}"
case "$mode" in native|wasm|all) ;; *) echo "Usage: $0 [native|wasm|all]" >&2; exit 2 ;; esac
target_dir="${CARGO_TARGET_DIR:-target}"

if [[ "$mode" == wasm || "$mode" == all ]]; then
    bindgen="${WASM_BINDGEN:-$target_dir/libtex-tools/bin/wasm-bindgen}"
    if [[ ! -x "$bindgen" ]]; then bindgen=wasm-bindgen; fi
    if ! command -v "$bindgen" >/dev/null 2>&1; then
        echo "Install wasm-bindgen-cli matching Cargo.lock (see docs/libraries.md)." >&2
        exit 1
    fi
    required=$(python3 -c 'import tomllib; print(next(p["version"] for p in tomllib.load(open("Cargo.lock", "rb"))["package"] if p["name"] == "wasm-bindgen"))')
    if [[ "$("$bindgen" --version)" != "wasm-bindgen $required" ]]; then
        echo "wasm-bindgen-cli must match Cargo.lock: $required" >&2
        exit 1
    fi
fi

if [[ "$mode" == native || "$mode" == all ]]; then
    cargo build --locked --profile ffi-release -p libtex
    mkdir -p "$target_dir/libtex/include"
    cp crates/libtex/include/tex.h "$target_dir/libtex/include/tex.h"
    for name in libtex.so libtex.a libtex.dylib tex.dll tex.dll.lib tex.lib; do
        if [[ -f "$target_dir/ffi-release/$name" ]]; then
            cp "$target_dir/ffi-release/$name" "$target_dir/libtex/$name"
        fi
    done
    echo "Native libraries: $target_dir/libtex/"
fi

if [[ "$mode" == wasm || "$mode" == all ]]; then
    cargo build --locked --release -p tex-wasm --target wasm32-unknown-unknown
    for target in web nodejs; do
        "$bindgen" "$target_dir/wasm32-unknown-unknown/release/tex_wasm.wasm" \
            --target "$target" --out-dir "$target_dir/wasm/$target" --out-name tex
    done
    echo "Browser and Node.js modules: $target_dir/wasm/"
fi
