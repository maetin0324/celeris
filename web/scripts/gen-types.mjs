import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const schemaPath = path.join(root, "docs/api/v1/api-v1.schema.json");
const output = path.join(root, "web/api/generated");
const schema = JSON.parse(readFileSync(schemaPath, "utf8"));

function ts(node) {
  if (node.$ref) return node.$ref.split("/").at(-1);
  if (Object.hasOwn(node, "const")) return JSON.stringify(node.const);
  if (node.enum) return node.enum.map((item) => JSON.stringify(item)).join(" | ");
  if (node.anyOf || node.oneOf) return (node.anyOf ?? node.oneOf).map(ts).join(" | ");
  if (node.allOf) return node.allOf.map((part) => `(${ts(part)})`).join(" & ");
  if (Array.isArray(node.type)) return node.type.map((type) => ts({ ...node, type })).join(" | ");
  if (node.type === "array") return `Array<${ts(node.items ?? {})}>`;
  if (node.type === "object" || node.properties) {
    const required = new Set(node.required ?? []);
    const fields = Object.entries(node.properties ?? {}).map(
      ([key, value]) => `  ${JSON.stringify(key)}${required.has(key) ? "" : "?"}: ${ts(value)};`,
    );
    if (node.additionalProperties && typeof node.additionalProperties === "object")
      fields.push(`  [key: string]: ${ts(node.additionalProperties)};`);
    return `{\n${fields.join("\n")}\n}`;
  }
  if (node.type === "string") return "string";
  if (node.type === "integer" || node.type === "number") return "number";
  if (node.type === "boolean") return "boolean";
  if (node.type === "null") return "null";
  return "unknown";
}

const types = `// Generated from docs/api/v1/api-v1.schema.json by web/scripts/gen-types.mjs. Do not edit.\n// CoS run credential routes: POST /cos/operations, GET /cos/operations/{o}, POST /cos/threads/{t}/checkpoint.\n${Object.entries(
  schema.$defs,
)
  .map(([key, value]) => `export type ${key} = ${ts(value)};`)
  .join("\n\n")}\n`;
const outputs = [
  ["types.ts", types],
  ["schema.json", `${JSON.stringify(schema, null, 2)}\n`],
];
let stale = false;
for (const [name, contents] of outputs) {
  const target = path.join(output, name);
  if (process.argv.includes("--check")) {
    if (!existsSync(target) || readFileSync(target, "utf8") !== contents) {
      console.error(`gen-types: stale ${target}`);
      stale = true;
    }
  } else writeFileSync(target, contents);
}
if (stale) process.exitCode = 1;
