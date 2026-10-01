#!/usr/bin/env bash
# Compare our-engine PDF against the latexmk reference for trust_own/main.tex.
# Usage: scripts/compare_with_latexmk.sh <our.pdf>
# Verdict: PAGES match AND text content matches (pdftotext, whitespace-normalized).
set -u
OURS="${1:?usage: compare_with_latexmk.sh <our.pdf>}"
REF="/home/leo/dd/tex/trust_own/main.pdf"
[ -f "$OURS" ] || { echo "COMPARE: our PDF missing: $OURS"; exit 1; }
[ -f "$REF" ]  || { echo "COMPARE: reference PDF missing: $REF"; exit 1; }
OP=$(pdfinfo "$OURS" 2>/dev/null | awk '/^Pages/{print $2}')
RP=$(pdfinfo "$REF"  2>/dev/null | awk '/^Pages/{print $2}')
echo "COMPARE: pages ours=$OP ref=$RP"
TMP=$(mktemp -d) || exit 1
trap 'rm -rf -- "$TMP"' EXIT
pdftotext "$OURS" "$TMP/ours.txt" 2>/dev/null || { echo "COMPARE: pdftotext failed: $OURS"; exit 1; }
pdftotext "$REF"  "$TMP/ref.txt"  2>/dev/null || { echo "COMPARE: pdftotext failed: $REF"; exit 1; }
norm() { tr -s ' \n' ' ' < "$1" | sed 's/ //g' | md5sum | cut -d' ' -f1; }
OH=$(norm "$TMP/ours.txt"); RH=$(norm "$TMP/ref.txt")
echo "COMPARE: text-hash ours=$OH ref=$RH"
if [ -n "$OP" ] && [ "$OP" = "$RP" ] && [ "$OH" = "$RH" ]; then
  echo "COMPARE: MATCH"
else
  echo "COMPARE: DIFFER"
  diff <(tr -s ' \n' '\n' < "$TMP/ours.txt") <(tr -s ' \n' '\n' < "$TMP/ref.txt") | head -20
  exit 1
fi
