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
const { buildSchema, parse, specifiedRules, validate, NoDeprecatedCustomRule, NoUnusedFragmentsRule } =
  require("graphql");

const queries = "crates/hit-github/src/graphql";
const fragmentsFile = "fragments.graphql";
const fragments = readFileSync(join(queries, fragmentsFile), "utf8");

// Queries are sent with the whole fragments file, like `nodes::queries` does,
// so a fragment one query does not use is fine.
const rules = specifiedRules.filter((rule) => rule !== NoUnusedFragmentsRule);

const schema = buildSchema(readFileSync(schemaPath, "utf8"));
let failed = false;

for (const file of readdirSync(queries).sort()) {
  if (!file.endsWith(".graphql") || file === fragmentsFile) continue;
  const own = readFileSync(join(queries, file), "utf8");
  const usesFragments = own.includes("...");
  const document = parse(usesFragments ? own + fragments : own);
  const errors = validate(schema, document, rules);
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
