#!/usr/bin/env bash
# Full-document visual + text comparison: ours vs /tmp/ref_clean/main.pdf
# Usage: scripts/diff_vs_ref.sh [our.pdf] [dpi]
set -u
OURS="${1:-/tmp/work1/main.pdf}"
REF=/tmp/ref_clean/main.pdf
DPI="${2:-100}"
TMP=$(mktemp -d)
OP=$(pdfinfo "$OURS" | awk '/^Pages/{print $2}')
RP=$(pdfinfo "$REF" | awk '/^Pages/{print $2}')
echo "PIXDIFF: pages ours=$OP ref=$RP"
total=0; bad=0
N=$((OP>RP?RP:OP))
for i in $(seq 1 "$N"); do
  pdftoppm -r "$DPI" -f $i -l $i -gray -png "$OURS" "$TMP/o" >/dev/null 2>&1
  pdftoppm -r "$DPI" -f $i -l $i -gray -png "$REF" "$TMP/r" >/dev/null 2>&1
  o=$(ls "$TMP"/o*-*.png 2>/dev/null | head -1); r=$(ls "$TMP"/r*-*.png 2>/dev/null | head -1)
  [ -z "$o" ] || [ -z "$r" ] && continue
  python3 - "$o" "$r" "$i" <<'EOF'
import sys
try:
    from PIL import Image
except ImportError:
    sys.exit(3)
a=Image.open(sys.argv[1]).convert("L"); b=Image.open(sys.argv[2]).convert("L")
if a.size!=b.size: b=b.resize(a.size)
pa, pb = a.tobytes(), b.tobytes()
diff = sum(1 for x,y in zip(pa,pb) if abs(x-y) > 40)
print(f"{sys.argv[3]} {diff} {len(pa)}")
EOF
  read p d t <<< "$(python3 - "$o" "$r" "$i" <<'EOF'
import sys
from PIL import Image
a=Image.open(sys.argv[1]).convert("L"); b=Image.open(sys.argv[2]).convert("L")
if a.size!=b.size: b=b.resize(a.size)
pa, pb = a.tobytes(), b.tobytes()
diff = sum(1 for x,y in zip(pa,pb) if abs(x-y) > 40)
print(diff, len(pa))
EOF
)"
  total=$((total+t)); bad=$((bad+d))
  awk -v d="$d" -v t="$t" 'BEGIN{ if (t>0 && d/t>0.02) printf "PIXDIFF: page %s: %.2f%% pixels differ\n", '"$i"', 100*d/t }'
  rm -f "$TMP"/o*-*.png "$TMP"/r*-*.png
done
echo "PIXDIFF: total differing pixels: $bad / $total = $(python3 -c "print(f'{100*$bad/$total:.3f}%' if $total else 'n/a')")"
# text comparison
pdftotext "$OURS" "$TMP/o.txt"; pdftotext "$REF" "$TMP/r.txt"
norm() { tr -s ' \n' ' ' < "$1" | sed 's/ //g' | md5sum | cut -d' ' -f1; }
echo "TEXTDIFF: hash ours=$(norm "$TMP/o.txt") ref=$(norm "$TMP/r.txt")"
[ "$(norm "$TMP/o.txt")" = "$(norm "$TMP/r.txt")" ] && echo "TEXTDIFF: MATCH" || echo "TEXTDIFF: DIFFER"
rm -rf "$TMP"
