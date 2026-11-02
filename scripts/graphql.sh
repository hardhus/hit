#!/bin/sh
# Validates hit's GraphQL queries against GitHub's current public schema.
set -eu

work=target/graphql-check
mkdir -p "$work"

curl -fsSL --retry 3 -o "$work/schema.graphql" \
  https://docs.github.com/public/fpt/schema.docs.graphql

if [ ! -d "$work/node_modules/graphql" ]; then
  npm install --silent --no-audit --no-fund --prefix "$work" graphql@16.10.0
fi

node scripts/check-graphql.mjs "$work/schema.graphql" "$work/node_modules/graphql"
