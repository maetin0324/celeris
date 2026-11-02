import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { HOST_FUNCTIONS, renderHostTypes } from "./gen-host-types.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const pkg = path.resolve(here, "..");

test("the committed host.d.ts matches the schema", () => {
  execFileSync(process.execPath, ["scripts/gen-host-types.mjs", "--check"], { cwd: pkg, stdio: "pipe" });
});

test("--check fails when the generated file differs from the schema", () => {
  const schema = JSON.parse(readFileSync(path.resolve(pkg, "../docs/api/script-host/host-v1.schema.json"), "utf8"));
  const current = readFileSync(path.join(pkg, "src/host.d.ts"), "utf8");
  assert.equal(renderHostTypes(schema), current);
  schema.$defs.Added = { type: "string" };
  assert.notEqual(renderHostTypes(schema), current);
});

// tsconfig の skipLibCheck は .d.ts の中の誤りを隠すので、生成物だけは検査を外して型検査する。
test("the generated host.d.ts is valid TypeScript", () => {
  const tsc = path.join(pkg, "node_modules/.bin/tsc");
  const args = ["--ignoreConfig", "--noEmit", "--skipLibCheck", "false", "--target", "esnext", "--lib", "esnext", "--types", "", "src/host.d.ts"];
  execFileSync(tsc, args, { cwd: pkg, stdio: "pipe" });
});

test("schema shapes map to the types of the value table", () => {
  const out = renderHostTypes({
    $defs: {
      Nullable: { type: ["string", "null"] },
      Tagged: {
        oneOf: [
          { type: "object", properties: { kind: { type: "string", const: "a" } }, required: ["kind"] },
          { type: "object", properties: { kind: { type: "string", const: "b" }, n: { type: "integer" } }, required: ["kind", "n"] },
        ],
      },
      Unit: { type: "string", enum: ["low", "high"] },
      Ref: { type: "object", properties: { u: { $ref: "#/$defs/Unit" } }, required: ["u"] },
    },
  });
  assert.match(out, /export type Nullable = string \| null;/);
  assert.match(out, /"kind": "a";[\s\S]*"kind": "b";\n {2}"n": number;/);
  assert.match(out, /export type Unit = "low" \| "high";/);
  assert.match(out, /"u": Unit;/);
});

test("declared functions have the same names as the exports of host.js", () => {
  const names = (text) =>
    [...text.matchAll(/^export (?:async )?(?:function|class|const) (\w+)/gm)].map((match) => match[1]).sort();
  const hostJs = readFileSync(path.resolve(pkg, "../crates/script-runtime/src/host.js"), "utf8");
  assert.deepEqual(names(HOST_FUNCTIONS), names(hostJs));
});
