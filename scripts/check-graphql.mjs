// Checks every query hit sends against GitHub's public GraphQL schema, so a
// misspelled or removed field fails here and not on someone's screen.
//
// usage: node check-graphql.mjs <schema file> <graphql package dir>
//
// Run through `just graphql`, which fetches both.

import { readFileSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";

const [schemaPath, packageDir] = process.argv.slice(2);
const require = createRequire(resolve(packageDir, "package.json"));
const { buildSchema, parse, validate, NoDeprecatedCustomRule } = require("graphql");

const queries = "crates/hit-github/src/graphql";
const fragments = {
  Pr: readFileSync(join(queries, "fragment_pr.graphql"), "utf8"),
  Iss: readFileSync(join(queries, "fragment_issue.graphql"), "utf8"),
};

const schema = buildSchema(readFileSync(schemaPath, "utf8"));
let failed = false;

for (const file of readdirSync(queries).sort()) {
  if (!file.endsWith(".graphql") || file.startsWith("fragment_")) continue;
  const own = readFileSync(join(queries, file), "utf8");
  // Each query brings the fragments it uses, as `nodes::queries` does; every
  // rule applies, including the one against unused fragments.
  const sent = Object.entries(fragments)
    .filter(([name]) => new RegExp(`\\.\\.\\.${name}\\b`).test(own))
    .map(([, text]) => text);
  const document = parse([own, ...sent].join("\n"));
  const errors = validate(schema, document);
  for (const error of errors) {
    console.error(`${file}: ${error.message}`);
    failed = true;
  }
  for (const warning of validate(schema, document, [NoDeprecatedCustomRule])) {
    console.warn(`${file}: warning: ${warning.message}`);
  }
  if (errors.length === 0) console.log(`${file}: ok`);
}

process.exit(failed ? 1 : 0);
