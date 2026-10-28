#!/bin/sh
# Check the workspace with the minimum supported Rust version.
set -eu

msrv="$1"

if ! rustup toolchain list | grep -q "^$msrv"; then
    echo "warning: toolchain $msrv is not installed (rustup toolchain install $msrv)" >&2
    exit 0
fi

cargo "+$msrv" check --workspace --all-targets
