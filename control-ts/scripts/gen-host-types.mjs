// docs/api/script-host/host-v1.schema.json から src/host.d.ts を作る（web/scripts/gen-types.mjs と同じ方式）。
//   node scripts/gen-host-types.mjs           生成物を書く
//   node scripts/gen-host-types.mjs --check   生成物が古ければ exit 1
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const pkg = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const schemaPath = path.resolve(pkg, "../docs/api/script-host/host-v1.schema.json");
const outputPath = path.join(pkg, "src/host.d.ts");

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

// schema は DTO だけを持つ。op の関数の形は crates/script-runtime/src/host.js の export に合わせてここに書く
// （export の名前の一致は scripts/gen-host-types.test.mjs が確かめる）。
export const HOST_FUNCTIONS = `export type RequestContext = { token: string };

export const HOST_API_VERSION: string;

export class HostError extends Error {
  readonly code: HostErrorCode;
  constructor(code: HostErrorCode, message: string);
}

export function hostVersion(): string;

export function selftestEcho(ctx: RequestContext, request: SelftestEchoRequest): Promise<SelftestEchoReply>;

export function cosDigestListRuns(ctx: RequestContext, request: CosDigestListRunsRequest): Promise<CosDigestListRunsReply>;

export function cosDigestReadRun(ctx: RequestContext, request: CosDigestReadRunRequest): Promise<CosDigestInput>;`;

export function renderHostTypes(schema) {
  // schema の HostError は TS では上の class で表す（code と message を持つ）。同じ名前の type を出さない。
  const types = Object.entries(schema.$defs)
    .filter(([key]) => key !== "HostError")
    .map(([key, value]) => `export type ${key} = ${ts(value)};`);
  const body = [...types, HOST_FUNCTIONS].join("\n\n");
  return `// Generated from docs/api/script-host/host-v1.schema.json by control-ts/scripts/gen-host-types.mjs. Do not edit.\ndeclare module "celeris:host" {\n${body}\n}\n`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const contents = renderHostTypes(JSON.parse(readFileSync(schemaPath, "utf8")));
  if (process.argv.includes("--check")) {
    if (!existsSync(outputPath) || readFileSync(outputPath, "utf8") !== contents) {
      console.error(`gen-host-types: stale ${outputPath}`);
      process.exitCode = 1;
    }
  } else writeFileSync(outputPath, contents);
}
