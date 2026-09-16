#!/bin/sh
set -eu

# Let Cargo's dependency tracking decide what needs rebuilding and preserve
# its real exit status so automation cannot report a failed build as success.
exec cargo build -p tex-cli "$@"
