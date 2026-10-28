#!/bin/sh
# Check dependencies against the RustSec advisory database.
set -eu

if ! cargo audit --version >/dev/null 2>&1; then
    echo "warning: cargo-audit is not installed (cargo install cargo-audit --locked)" >&2
    exit 0
fi

cargo audit
