#!/usr/bin/env bash
# Issue #16 regression gate: build `ratex` from source exactly like
# `cargo install` (Homebrew on Linux, the AUR source package) does -- release
# profile, ThinLTO, embedded package archive -- inside a container with a hard
# memory ceiling and no swap, then smoke the installed binary. A build that
# needs more memory than the ceiling is OOM-killed and the gate fails.
#
# Every rustc and linker process runs under a wrapper that records its peak
# RSS (VmHWM; file-backed pages included) and its peak anonymous RSS, the part
# the kernel cannot reclaim without swap. The largest ones are printed.
#
# Usage: scripts/memory_bounded_build.sh [source-dir]
#   source-dir  checkout to build (default: this repository); mounted read-only
# Environment:
#   RATEX_MEMORY_LIMIT  container memory ceiling, swap disabled (default 2g)
#   RATEX_BUILD_CPUS    container CPUs (default 4)
#   RATEX_BUILD_JOBS    cargo build jobs (default 1)
#   RUST_IMAGE          official Rust image to build in (default rust:1-bookworm)
#   DOCKER              docker command, e.g. "sudo docker" (default docker)
set -euo pipefail

SRC=$(cd "${1:-$(dirname "$0")/..}" && pwd)
LIMIT=${RATEX_MEMORY_LIMIT:-2g}
CPUS=${RATEX_BUILD_CPUS:-4}
JOBS=${RATEX_BUILD_JOBS:-1}
IMAGE=${RUST_IMAGE:-rust:1-bookworm}
read -r -a DOCKER_CMD <<< "${DOCKER:-docker}"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/ratex-memgate.XXXXXX")
trap 'rm -rf -- "$WORK"' EXIT
mkdir -p "$WORK/out" "$WORK/tools"

# RUSTC_WRAPPER (argv: rustc ARGS...) and, through the `memwrap-link` name,
# the linker (argv: ARGS... for `cc`). Appends one JSON line per process to
# $MEMWRAP_LOG. wait4's ru_maxrss would also cover rustc's linker child, so
# the wrapper samples the child's own VmHWM and RssAnon from /proc instead.
cat > "$WORK/tools/memwrap.py" <<'EOF'
#!/usr/bin/env python3
import json, os, sys, threading, time

def status(pid):
    fields = {}
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                key, _, value = line.partition(":")
                if key in ("VmHWM", "RssAnon"):
                    fields[key] = int(value.split()[0])
    except OSError:
        pass
    return fields

def tree(pid):
    pids = [pid]
    for p in pids:
        try:
            for task in os.listdir(f"/proc/{p}/task"):
                with open(f"/proc/{p}/task/{task}/children") as f:
                    pids.extend(int(child) for child in f.read().split())
        except OSError:
            pass
    return pids

link = os.path.basename(sys.argv[0]) == "memwrap-link"
argv = ["cc"] + sys.argv[1:] if link else sys.argv[1:]
name = "?"
if link and "-o" in argv:
    name = "link " + os.path.basename(argv[argv.index("-o") + 1])
elif "--crate-name" in argv:
    name = argv[argv.index("--crate-name") + 1]
    if "--crate-type" in argv:
        name += " " + argv[argv.index("--crate-type") + 1]
child = os.fork()
if child == 0:
    try:
        os.execvp(argv[0], argv)
    finally:
        os._exit(127)
peak = {"hwm": 0, "anon": 0}
done = threading.Event()

def sample():
    while not done.is_set():
        for pid in tree(child):
            fields = status(pid)
            # For the linker the real work happens in cc's ld child.
            if pid == child or link:
                peak["hwm"] = max(peak["hwm"], fields.get("VmHWM", 0))
            peak["anon"] = max(peak["anon"], fields.get("RssAnon", 0))
        time.sleep(0.02)

threading.Thread(target=sample, daemon=True).start()
_, code, _ = os.wait4(child, 0)
done.set()
if name != "?" and not name.startswith("___"):
    with open(os.environ["MEMWRAP_LOG"], "a") as f:
        f.write(json.dumps({"name": name, "hwm": peak["hwm"], "anon": peak["anon"]}) + "\n")
code = os.waitstatus_to_exitcode(code)
sys.exit(code if code >= 0 else 128 - code)
EOF
chmod +x "$WORK/tools/memwrap.py"
ln -s memwrap.py "$WORK/tools/memwrap-link"

# Runs inside the container. The build tree, registry and toolchain temp files
# live in the container layer (disk), not tmpfs, so only process memory and
# reclaimable page cache count against the ceiling.
INNER=$(cat <<'EOF'
set -u
host=$(rustc -vV | sed -n 's/^host: //p' | tr '[:lower:]-' '[:upper:]_')
export "CARGO_TARGET_${host}_LINKER=/tools/memwrap-link"
export RUSTC_WRAPPER=/tools/memwrap.py MEMWRAP_LOG=/tmp/memwrap.jsonl
cargo install --locked --path /src/crates/tex-cli --bin ratex --root /out
rc=$?
python3 - <<'PY' >&2
import json
procs = [json.loads(line) for line in open("/tmp/memwrap.jsonl")]
procs.sort(key=lambda p: -p["hwm"])
print(f"memory gate: {len(procs)} rustc/linker processes; largest peak RSS (anonymous peak):")
for p in procs[:8]:
    print(f"  {p['hwm'] / 1024:7.0f} MiB ({p['anon'] / 1024:5.0f} MiB anon)  {p['name']}")
PY
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

echo "memory gate: building $SRC with memory=$LIMIT (no swap), cpus=$CPUS, jobs=$JOBS, image=$IMAGE" >&2
start=$(date +%s)
set +e
"${DOCKER_CMD[@]}" run --rm \
  --memory="$LIMIT" --memory-swap="$LIMIT" --cpus="$CPUS" \
  --user "$(id -u):$(id -g)" \
  -e HOME=/tmp -e CARGO_HOME=/tmp/cargo -e CARGO_TARGET_DIR=/tmp/target \
  -e CARGO_BUILD_JOBS="$JOBS" -e CARGO_TERM_COLOR=always \
  -v "$SRC:/src:ro" -v "$WORK/out:/out" -v "$WORK/tools:/tools:ro" \
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
