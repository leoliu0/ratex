#!/bin/bash
# Build target/release/texres with profile-guided optimization.
#
#   scripts/build_pgo.sh
#
# 1. build an instrumented texres into target/pgo/instrumented,
# 2. run it over the benchmark corpus (scripts/bench/corpus: cold builds with
#    every engine, biber and BibTeX included),
# 3. merge the profiles and rebuild target/release/texres with them.
#
# The engine is a large token interpreter whose hot paths are spread over
# many big functions; without a profile the compiler cannot tell hot from cold
# code and the hot path misses the instruction cache several times per
# hundred instructions. With a profile one pdfLaTeX pass over the tikz_pgfplots
# benchmark document takes about a quarter fewer cycles (PERFORMANCE.md). The
# profile comes from other documents than the one measured; no profile is
# stored in the repository, so it always matches the sources it was taken from.
#
# Needs llvm-profdata of the LLVM that rustc uses: `rustup component add
# llvm-tools`, or the distribution's llvm package.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
pgo=${PGO_DIR:-$root/target/pgo}
corpus=$root/scripts/bench/corpus

profdata=$(command -v llvm-profdata || true)
if [ -z "$profdata" ]; then
    profdata=$(find "$(rustc --print sysroot)" -name llvm-profdata -type f 2>/dev/null | head -n 1)
fi
if [ -z "$profdata" ]; then
    echo "build_pgo.sh: llvm-profdata not found (rustup component add llvm-tools)" >&2
    exit 1
fi

# RUSTFLAGS replaces the target rustflags of .cargo/config.toml, so repeat its
# link argument.
case $(uname -s) in
    Darwin) link="-C link-arg=-Wl,-export_dynamic" ;;
    *) link="-C link-arg=-Wl,--export-dynamic" ;;
esac

rm -rf "$pgo/raw" "$pgo/merged.profdata" "$pgo/work"
mkdir -p "$pgo/raw" "$pgo/work"

echo "== instrumented build"
RUSTFLAGS="${RUSTFLAGS:-} $link -Cprofile-generate=$pgo/raw" \
    CARGO_TARGET_DIR="$pgo/instrumented" \
    cargo build --release --locked -p tex-cli --bin texres
texres=$pgo/instrumented/release/texres

echo "== training run over $corpus"
export TZ=UTC LC_ALL=C.UTF-8 SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1
export TEX_RS_CACHE_DIR="$pgo/work/cache"
for doc in "$corpus"/*/; do
    name=$(basename "$doc")
    case $name in
        xelatex_*) engine=-xelatex ;;
        lualatex_*) engine=-lualatex ;;
        *) engine=-pdf ;;
    esac
    mkdir -p "$pgo/work/$name"
    cp -r "$doc". "$pgo/work/$name/"
    echo "   $name"
    # A document that fails to build still trains the paths up to the failure.
    (cd "$pgo/work/$name" && "$texres" "$engine" -interaction=nonstopmode main.tex >/dev/null 2>&1) || true
done

echo "== merge"
"$profdata" merge -o "$pgo/merged.profdata" "$pgo/raw"/*.profraw

echo "== optimized build"
RUSTFLAGS="${RUSTFLAGS:-} $link -Cprofile-use=$pgo/merged.profdata" \
    cargo build --release --locked -p tex-cli --bin texres
echo "built ${CARGO_TARGET_DIR:-$root/target}/release/texres"
