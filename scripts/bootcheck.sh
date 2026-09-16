#!/usr/bin/env bash
# Boot-state regression check. Run after EVERY engine landing.
# Usage: scripts/bootcheck.sh [label]
# Prints: build status, error count, furthest file:line reached.
set -u
LABEL="${1:-unnamed}"
cd "$(dirname "$0")/.."
if ! cargo build -p tex-cli --bin pdflatex --offline; then
  echo "BOOTCHECK[$LABEL]: BUILD FAILED" >&2
  exit 1
fi
WORK=$(mktemp -d /tmp/tex-bootcheck.XXXXXX) || {
  echo "BOOTCHECK[$LABEL]: cannot create temporary workspace" >&2
  exit 1
}
LOG="$WORK/process.log"
cat > "$WORK/hello.tex" <<'EOF'
\documentclass{article}
\begin{document}
Boot-state smoke test: $a^2+b^2=c^2$.
\end{document}
EOF
FMT=target/debug/pdflatex.fmt
BACKUP=""
restore_format() {
  if [ -n "$BACKUP" ] && [ -f "$BACKUP" ]; then
    mv -- "$BACKUP" "$FMT"
  fi
}
cleanup() {
  restore_format
  rm -rf -- "$WORK"
}
if [ -f "$FMT" ]; then
  BACKUP=$(mktemp /tmp/bootcheck-fmt.XXXXXX)
  mv -- "$FMT" "$BACKUP"
fi
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
timeout 120 ./target/debug/pdflatex -interaction=nonstopmode -halt-on-error \
  -output-directory "$WORK" "$WORK/hello.tex" > "$LOG" 2>&1
EC=$?
restore_format
trap - EXIT HUP INT TERM
ERRS=$(grep -cE '^! ' "$LOG" || true)
LASTERR=$(grep -E '^! ' "$LOG" | tail -1 || true)
echo "BOOTCHECK[$LABEL]: errors=$ERRS"
echo "BOOTCHECK[$LABEL]: last: $LASTERR"
# Full pass = exit 0 and no error lines and a PDF written.
if [ "$EC" -eq 0 ] && [ "$ERRS" -eq 0 ] && [ -f "$WORK/hello.pdf" ]; then
  echo "BOOTCHECK[$LABEL]: PASS"
  cleanup
else
  echo "BOOTCHECK[$LABEL]: NOT PASSING (hang if exit=124)"
  tail -40 "$LOG"
  cleanup
  exit 1
fi
