#!/usr/bin/env bash
# Issue #16 regression gate: build `ratex` from source exactly like
# `cargo install` (Homebrew on Linux) does -- release profile, ThinLTO,
# embedded package archive -- inside a container with a hard memory ceiling
# and no swap, then smoke the installed binary. A build that needs more memory
# than the ceiling is OOM-killed and the gate fails.
#
# Usage: scripts/memory_bounded_build.sh [source-dir]
#   source-dir  checkout to build (default: this repository); mounted read-only
# Environment:
#   RATEX_MEMORY_LIMIT  container memory ceiling, swap disabled (default 6g)
#   RATEX_BUILD_CPUS    CPUs and cargo jobs, mirroring a 4-vCPU host (default 4)
#   RUST_IMAGE          official Rust image to build in (default rust:1-bookworm)
#   DOCKER              docker command, e.g. "sudo docker" (default docker)
set -euo pipefail

SRC=$(cd "${1:-$(dirname "$0")/..}" && pwd)
LIMIT=${RATEX_MEMORY_LIMIT:-6g}
CPUS=${RATEX_BUILD_CPUS:-4}
IMAGE=${RUST_IMAGE:-rust:1-bookworm}
read -r -a DOCKER_CMD <<< "${DOCKER:-docker}"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/ratex-memgate.XXXXXX")
trap 'rm -rf -- "$WORK"' EXIT
mkdir -p "$WORK/out"

# Runs inside the container. The build tree, registry and toolchain temp files
# live in the container layer (disk), not tmpfs, so only process memory and
# reclaimable page cache count against the ceiling.
INNER=$(cat <<'EOF'
set -u
python3 - <<'PY'
import resource, subprocess, sys
rc = subprocess.call(["cargo", "install", "--locked", "--path", "/src/crates/tex-cli",
                      "--bin", "ratex", "--root", "/out"])
peak = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1048576
print(f"memory gate: largest single build process peak RSS {peak:.2f} GiB", file=sys.stderr)
sys.exit(rc if rc >= 0 else 128 - rc)
PY
rc=$?
if [ -r /sys/fs/cgroup/memory.peak ]; then
  awk '{printf "memory gate: container cgroup memory.peak %.2f GiB (includes page cache)\n", $1 / 1073741824}' \
    /sys/fs/cgroup/memory.peak >&2
fi
ooms=$(awk '$1 == "oom_kill" { print $2 }' /sys/fs/cgroup/memory.events 2>/dev/null || echo 0)
if [ "${ooms:-0}" -gt 0 ]; then
  echo "memory gate: ${ooms} build process(es) OOM-killed by the memory ceiling" >&2
  exit 137
fi
exit "$rc"
EOF
)

echo "memory gate: building $SRC with memory=$LIMIT (no swap), cpus=$CPUS, image=$IMAGE" >&2
start=$(date +%s)
set +e
"${DOCKER_CMD[@]}" run --rm \
  --memory="$LIMIT" --memory-swap="$LIMIT" --cpus="$CPUS" \
  --user "$(id -u):$(id -g)" \
  -e HOME=/tmp -e CARGO_HOME=/tmp/cargo -e CARGO_TARGET_DIR=/tmp/target \
  -e CARGO_BUILD_JOBS="$CPUS" -e CARGO_TERM_COLOR=always \
  -v "$SRC:/src:ro" -v "$WORK/out:/out" \
  "$IMAGE" bash -c "$INNER"
rc=$?
set -e
echo "memory gate: build finished in $(( $(date +%s) - start ))s with exit $rc" >&2
if [ "$rc" -eq 137 ]; then
  echo "::error title=Memory-bounded build::building ratex exceeded the $LIMIT memory ceiling and was OOM-killed (issue #16 regression)" >&2
  exit 1
elif [ "$rc" -ne 0 ]; then
  echo "::error title=Memory-bounded build::cargo install of ratex failed with exit $rc" >&2
  exit 1
fi

RATEX="$WORK/out/bin/ratex"
"$RATEX" --version
cat > "$WORK/smoke.tex" <<'EOF'
\documentclass{article}
\usepackage{amsmath}
\usepackage{hyperref}
\begin{document}
\section{Memory gate}\label{sec:gate}
\begin{align}
  e^{i\pi} + 1 &= 0 \label{eq:euler}
\end{align}
Section~\ref{sec:gate} holds equation~\eqref{eq:euler}; see \url{https://github.com/leoliu0/ratex}.
\end{document}
EOF
(cd "$WORK" && "$RATEX" --cache-directory "$WORK/cache" smoke.tex)
if [ "$(head -c 5 "$WORK/smoke.pdf" 2>/dev/null)" != "%PDF-" ]; then
  echo "::error title=Memory-bounded build::installed ratex did not produce smoke.pdf" >&2
  exit 1
fi
echo "memory gate: PASS ($(wc -c < "$WORK/smoke.pdf") byte PDF)" >&2
