#!/usr/bin/env bash
# Regenerate the Latin woff2 subsets in this directory from the OFL sources in
# github.com/google/fonts (ofl/<family>/). Requires fonttools and brotli:
#   web/fonts/subset.sh DIR    (DIR holds the downloaded .ttf files)
set -euo pipefail
src="$1"
out="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

latin='U+0020-007E,U+00A0-00FF,U+0131,U+0152-0153,U+02C6,U+02DA,U+02DC,U+00D7,U+2013-2014,U+2018-201A,U+201C-201E,U+2022,U+2026,U+2032-2033,U+2039-203A,U+2044,U+20AC,U+2122,U+2190-2193,U+21B5,U+2212,U+2318,U+2325'

# Pin the optical size and keep only the weights the interface uses.
fonttools varLib.instancer -q "$src/Newsreader[opsz,wght].ttf" opsz=16 wght=400:650 -o "$tmp/newsreader.ttf"
fonttools varLib.instancer -q "$src/Newsreader-Italic[opsz,wght].ttf" opsz=16 wght=400 -o "$tmp/newsreader-italic.ttf"
fonttools varLib.instancer -q "$src/Inter[opsz,wght].ttf" opsz=14 wght=400:650 -o "$tmp/inter.ttf"
fonttools varLib.instancer -q "$src/InstrumentSans[wdth,wght].ttf" wdth=100 wght=400:650 -o "$tmp/instrument-sans.ttf"
fonttools varLib.instancer -q "$src/JetBrainsMono[wght].ttf" wght=400:600 -o "$tmp/jetbrains-mono.ttf"
cp "$src/CourierPrime-Regular.ttf" "$tmp/courier-prime.ttf"

for font in "$tmp"/*.ttf; do
    name="$(basename "$font" .ttf)"
    pyftsubset "$font" --unicodes="$latin" --flavor=woff2 --no-hinting --desubroutinize \
        --layout-features='kern,liga,calt,tnum,case,ss01,cv11,zero,onum,lnum,pnum' \
        --output-file="$out/$name.woff2"
done
