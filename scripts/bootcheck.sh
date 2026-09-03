#!/usr/bin/env bash
# Boot-state regression check. Run after EVERY engine landing.
# Usage: scripts/bootcheck.sh [label]
# Prints: build status, error count, furthest file:line reached.
set -u
LABEL="${1:-unnamed}"
cd "$(dirname "$0")/.."
cargo build -p tex-cli --bin pdflatex --offline 2>&1 | grep -E '^error' -A4 && { echo "BOOTCHECK[$LABEL]: BUILD FAILED"; exit 1; }
FMT=target/debug/pdflatex.fmt
[ -f "$FMT" ] && mv "$FMT" /tmp/bootcheck.fmt.bak
timeout 120 ./target/debug/pdflatex -output-directory /tmp/textest /tmp/textest/hello.tex > /tmp/bootcheck.log 2>&1
EC=$?
[ -f /tmp/bootcheck.fmt.bak ] && mv /tmp/bootcheck.fmt.bak "$FMT"
ERRS=$(grep -cE '^! ' /tmp/bootcheck.log)
FURTHEST=$(grep -oE '\(/usr/share/texmf-dist/tex/[^ ]+' /tmp/bootcheck.log | tail -1)
LASTERR=$(grep -E '^! ' /tmp/bootcheck.log | tail -1)
echo "BOOTCHECK[$LABEL]: errors=$ERRS"
echo "BOOTCHECK[$LABEL]: last: $LASTERR"
# Full pass = exit 0 and no error lines and a PDF written.
if [ "$EC" -eq 0 ] && [ "$ERRS" -eq 0 ] && [ -f /tmp/textest/hello.pdf ]; then
  echo "BOOTCHECK[$LABEL]: PASS"
else
  echo "BOOTCHECK[$LABEL]: NOT PASSING (hang if exit=124)"
fi
