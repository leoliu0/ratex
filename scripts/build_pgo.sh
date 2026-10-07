#!/usr/bin/env bash
# Build texres with profile-guided optimization.
#
#   scripts/build_pgo.sh [OUTPUT]      # default: target/pgo/texres
#
# 1. builds an instrumented texres,
# 2. runs it over the benchmark corpus (scripts/bench/corpus; every document
#    except the LuaLaTeX one, whose cold start is dominated by building a font
#    database) to record which code the engine runs and how its branches go,
# 3. rebuilds texres with that profile.
#
# The TeX engine is a large interpreter loop; laid out with the profile, the
# hot paths stop competing for the instruction cache and branch predictor
# (see PERFORMANCE.md, "Profile-guided build"). Behaviour is unchanged.
# The profile is tied to the sources it was recorded with and is rebuilt
# every time, so nothing generated here is meant to be committed.
#
# Needs the `llvm-profdata` that matches rustc's LLVM: either the one from
# `rustup component add llvm-tools` or one on PATH.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

out=${1:-target/pgo/texres}
# The instrumented build and the profile live in PGO_DIR; the optimized build
# goes to the normal target directory (under its <host triple>/ subdirectory).
work=${PGO_DIR:-$root/target/pgo}
target_dir=${CARGO_TARGET_DIR:-$root/target}
triple=$(rustc -vV | sed -n 's/^host: //p')
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-$(getconf _NPROCESSORS_ONLN)}

profdata=$(find "$(rustc --print sysroot)/lib/rustlib" -name llvm-profdata -type f 2>/dev/null | head -n 1)
profdata=${profdata:-$(command -v llvm-profdata || true)}
[ -n "$profdata" ] || { echo "llvm-profdata not found (rustup component add llvm-tools)" >&2; exit 1; }

rm -rf "$work/raw" "$work/merged.profdata"
mkdir -p "$work/raw" "$work/run"

# RUSTFLAGS would replace the per-target flags of .cargo/config.toml (the
# dynamic export of symbols for Lua C modules), so the profile flags go in as
# one more target-specific flag list, which cargo concatenates with them.
# --target keeps build scripts and proc macros out of the instrumentation.
build() { # build <target-dir> <rustc flag>
    cargo build --release --locked -p tex-cli --bin texres \
        --target "$triple" --target-dir "$1" \
        --config "target.$triple.rustflags=['$2']"
}

echo "==> instrumented build"
build "$work/gen" "-Cprofile-generate=$work/raw"

echo "==> training run over the benchmark corpus"
bin=$work/gen/$triple/release/texres
for dir in scripts/bench/corpus/*/; do
    doc=$(basename "$dir")
    case $doc in
        lualatex_fontspec) continue ;;
        xelatex_fontspec) engine=-xelatex ;;
        *) engine=-pdf ;;
    esac
    echo "    $doc"
    rm -rf "$work/run/$doc"
    cp -r "$dir" "$work/run/$doc"
    mkdir -p "$work/run/$doc.cache"
    (cd "$work/run/$doc" &&
        TEX_RS_CACHE_DIR="$work/run/$doc.cache" TZ=UTC LC_ALL=C.UTF-8 \
        SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 \
        "$bin" "$engine" main.tex >/dev/null 2>&1) ||
        echo "    (exit status $?; the profile still counts what ran)"
done
"$profdata" merge -o "$work/merged.profdata" "$work/raw"

echo "==> optimized build"
build "$target_dir" "-Cprofile-use=$work/merged.profdata"

mkdir -p "$(dirname "$out")"
cp "$target_dir/$triple/release/texres" "$out"
echo "wrote $out"
