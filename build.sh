#!/bin/sh
touch crates/tex-core/src/lib.rs crates/tex-core/src/*.rs
cargo build -p tex-cli 2>&1 | grep -E "^error" -A5
exit 0
