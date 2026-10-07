#!/usr/bin/env bash
# Build texres with profile-guided optimization.
#
#   scripts/build_pgo.sh [OUTPUT]      # default: target/pgo/texres
#
# 1. builds an instrumented texres,
# 2. runs a cold build of every document of the training
#    corpus (scripts/bench/corpus, plus a variant of scripts/bench/corpus100
#    with other text and data: gen_corpus100.py --seed-offset; documents that
#    come out identical to the measured ones are left out, so no benchmark
#    input is trained on) to record which code the engine runs and how its
#    branches go,
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
# Both builds link with fat LTO in one codegen unit, so the optimizer sees the
# whole interpreter at once when it inlines and lays out the profiled code
# (about 4% fewer cycles than thin LTO, see PERFORMANCE.md). The settings must
# be the same in both builds: they enter cargo's symbol hashes, and a profile
# recorded under other symbol names would silently go unused. The plain
# release profile keeps thin LTO, which needs far less memory and time.
build() { # build <target-dir> <rustc flag>
    cargo build --release --locked -p tex-cli --bin texres \
        --target "$triple" --target-dir "$1" \
        --config 'profile.release.lto="fat"' \
        --config 'profile.release.codegen-units=1' \
        --config "target.$triple.rustflags=['$2']"
}

echo "==> instrumented build"
build "$work/gen" "-Cprofile-generate=$work/raw"

echo "==> training run over the benchmark corpora"
bin=$work/gen/$triple/release/texres
# One cold build per document: scripts/bench/corpus (except the LuaLaTeX
# document) and the documents of a corpus100 variant, whose packages (beamer
# themes, siunitx tables, CJK, KOMA, memoir, TikZ, indexes, bibliographies,
# fontspec/luaotfload node processing, ...) the eight-document corpus alone
# leaves out of the profile. The variant has the measured documents' structure
# but other text and data; any document identical to its measured counterpart
# is skipped. Instrumented processes merge into the same raw profile files, so
# documents run in parallel.
# A LuaLaTeX build starts by building luaotfload's font names database; to keep
# the training independent of the machine's fonts, that database holds the TeX
# Live fonts only: OSFONTDIR names no system font directory and the
# configuration's location-precedence leaves only the texmf trees.
rm -rf "$work/train100"
python3 scripts/bench/gen_corpus100.py --out "$work/train100" --seed-offset 1000003 >/dev/null
mkdir -p "$work/run/no-os-fonts" "$work/run/xdg/luaotfload"
printf '[db]\n    location-precedence = texmf\n' >"$work/run/xdg/luaotfload/luaotfload.conf"
train() { # train <source dir> <engine flag>
    doc=$(basename "$1")
    fonts=()
    if [ "$2" = -lualatex ]; then
        fonts=(OSFONTDIR="$work/run/no-os-fonts" XDG_CONFIG_HOME="$work/run/xdg")
    fi
    rm -rf "$work/run/$doc" "$work/run/$doc.cache"
    cp -r "$1" "$work/run/$doc"
    mkdir -p "$work/run/$doc.cache"
    (cd "$work/run/$doc" &&
        env "${fonts[@]}" TEX_RS_CACHE_DIR="$work/run/$doc.cache" TZ=UTC LC_ALL=C.UTF-8 \
        SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 \
        "$bin" "$2" main.tex >/dev/null 2>&1) ||
        echo "    $doc: exit status $? (the profile still counts what ran)"
    echo "    $doc"
}
export -f train
export work bin
{
    for dir in scripts/bench/corpus/*/; do
        case $(basename "$dir") in
            lualatex_fontspec) ;;
            xelatex_fontspec) printf '%s\0%s\0' "$dir" -xelatex ;;
            *) printf '%s\0%s\0' "$dir" -pdf ;;
        esac
    done
    WORK="$work" python3 - <<'EOF'
import filecmp, json, os, sys
from pathlib import Path
train = Path(os.environ["WORK"]) / "train100"
flags = {"pdf": "-pdf", "xe": "-xelatex", "lua": "-lualatex"}
for name, doc in sorted(json.load(open(train / "manifest.json")).items()):
    if doc["engine"] not in flags or doc["main"] != "main.tex":
        continue
    measured = Path("scripts/bench/corpus100") / name / "main.tex"
    if measured.is_file() and filecmp.cmp(train / name / "main.tex", measured, shallow=False):
        continue
    sys.stdout.write(f"{train / name}/\0{flags[doc['engine']]}\0")
EOF
} | xargs -0 -n 2 -P "${PGO_TRAIN_JOBS:-8}" bash -c 'train "$0" "$1"'
"$profdata" merge -o "$work/merged.profdata" "$work/raw"

echo "==> optimized build"
build "$target_dir" "-Cprofile-use=$work/merged.profdata"

mkdir -p "$(dirname "$out")"
cp "$target_dir/$triple/release/texres" "$out"
echo "wrote $out"
