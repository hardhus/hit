#!/bin/sh
# Set [workspace.package] version in the root Cargo.toml and refresh Cargo.lock.
set -eu

version="$1"

case "$version" in
    [0-9]*.[0-9]*.[0-9]*) ;;
    *) echo "error: '$version' is not a semver version (X.Y.Z)" >&2; exit 1 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
manifest="$root/Cargo.toml"

# Only the first `version = "..."` line, which belongs to [workspace.package].
awk -v v="$version" '
    !done && /^version = "/ { print "version = \"" v "\""; done = 1; next }
    { print }
' "$manifest" > "$manifest.tmp"
mv "$manifest.tmp" "$manifest"

cargo update --workspace --quiet
echo "workspace version -> $version"
