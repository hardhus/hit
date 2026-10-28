set windows-shell := ["sh", "-cu"]

msrv := "1.90"

# list recipes
default:
    @just --list

# format all crates
fmt:
    cargo fmt --all

# check formatting
fmt-check:
    cargo fmt --all --check

# clippy with warnings as errors
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# run all tests
test:
    cargo test --workspace

# quality gate: fmt-check + lint + test
check: fmt-check lint test

# run hit with arguments
run *args:
    cargo run -q -p hit-cli -- {{args}}

# check the build with the minimum supported rust version
msrv:
    sh scripts/msrv.sh {{msrv}}

# set workspace version (e.g. just bump 0.0.2)
bump version:
    sh scripts/bump.sh {{version}}

# check dependencies for known vulnerabilities (needs cargo-audit)
audit:
    sh scripts/audit.sh
