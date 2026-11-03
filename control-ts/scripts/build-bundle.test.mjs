import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { INPUTS_FILE, buildBundle, inputsSha256 } from "./build-bundle.mjs";

const pkg = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const work = mkdtempSync(path.join(process.env.TMPDIR ?? tmpdir(), "control-ts-bundle-"));
after(() => rmSync(work, { recursive: true, force: true }));

let serial = 0;
function fixture(source) {
  const dir = path.join(work, `f${serial++}`);
  mkdirSync(dir);
  const entry = path.join(dir, "main.ts");
  writeFileSync(entry, source);
  return { entry, outDir: path.join(dir, "out") };
}

const sha256 = (file) => createHash("sha256").update(readFileSync(file)).digest("hex");

test("manifest records the hash of the bundle and of the schema", async () => {
  const { outDir } = fixture("");
  const manifest = await buildBundle({ outDir });
  const onDisk = JSON.parse(readFileSync(path.join(outDir, "manifest.json"), "utf8"));
  assert.deepEqual(onDisk, manifest);
  assert.equal(manifest.schema, "celeris.script-manifest/1");
  assert.equal(manifest.bundle_sha256, sha256(path.join(outDir, "bundle.mjs")));
  assert.equal(manifest.schema_sha256, sha256(path.resolve(pkg, "../docs/api/script-host/host-v1.schema.json")));
  assert.equal(manifest.host_api, "^1");
  assert.deepEqual(manifest.domains, ["cos_digest", "host_selftest", "lane_rules", "route_priority"]);
  assert.deepEqual(manifest.capabilities, ["cos_digest:list_runs", "cos_digest:read_run", "selftest:echo_task"]);
  assert.deepEqual(manifest.checkpoint_versions, []);
  assert.equal(manifest.toolchain.typescript, "7.0.2");
  assert.equal(manifest.toolchain.esbuild, "0.28.2");
  assert.equal(manifest.toolchain.node, process.versions.node);
  assert.match(manifest.source_commit, /^[0-9a-f]{40}(-dirty)?$/);
});

test("the same source gives the same bundle bytes", async () => {
  const first = await buildBundle({ outDir: fixture("").outDir });
  const second = await buildBundle({ outDir: fixture("").outDir });
  assert.equal(first.bundle_sha256, second.bundle_sha256);
});

test("the bundle is one ESM file that imports only celeris:host", async () => {
  const { outDir } = fixture("");
  await buildBundle({ outDir });
  const text = readFileSync(path.join(outDir, "bundle.mjs"), "utf8");
  const imports = [...text.matchAll(/^import .* from "([^"]+)";$/gm)].map((match) => match[1]);
  // 領域ごとの import は esbuild が 1 文にまとめないので、文の数でなく import 先の集合を見る。
  assert.deepEqual([...new Set(imports)], ["celeris:host"]);
  assert.match(text, /__celerisDispatch/);
});

const rejected = [
  ["node: static import", 'import { readFileSync } from "node:fs"; console.log(readFileSync);', "node:fs"],
  ["bare builtin name", 'import { readFileSync } from "fs"; console.log(readFileSync);', "fs"],
  ["node: dynamic import", 'await import("node:child_process");', "node:child_process"],
  ["node: require", 'require("node:os");', "node:os"],
  ["bun: import", 'import "bun:sqlite";', "bun:sqlite"],
  ["https: import", 'import "https://example.invalid/x.js";', "https://example.invalid/x.js"],
  ["protocol-relative import", 'import "//example.invalid/x.js";', "//example.invalid/x.js"],
];
for (const [name, source, specifier] of rejected) {
  test(`bundling fails on ${name} and leaves no output`, async () => {
    const { entry, outDir } = fixture(source);
    await assert.rejects(buildBundle({ entry, outDir }), (err) => {
      assert.ok(err.message.includes(`import of "${specifier}" is not allowed`), err.message);
      return true;
    });
    assert.equal(existsSync(path.join(outDir, "bundle.mjs")), false);
    assert.equal(existsSync(path.join(outDir, "manifest.json")), false);
    assert.equal(existsSync(path.join(outDir, INPUTS_FILE)), false);
  });
}

test("bundling fails on a capability the schema does not know", async () => {
  const { entry, outDir } = fixture("");
  const config = { host_api: "^1", domains: { x: ["write:everything"] }, checkpoint_versions: [] };
  await assert.rejects(buildBundle({ entry, outDir, config }), /unknown capability.*write:everything/);
});

// 付記 S5-9 (2): dist に source の内容 hash を書く。試験の support（Rust）が同じ値を計算して古い dist を見分ける。
test("dist records the content hash of the control-ts sources", async () => {
  const { outDir } = fixture("");
  await buildBundle({ outDir });
  assert.equal(readFileSync(path.join(outDir, INPUTS_FILE), "utf8"), `${inputsSha256(pkg)}\n`);
});

// Rust の control_ts_inputs_sha256 と同じ値（crates/celeris/tests/script_compare_route_priority.rs の固定値と同じ）。
test("the inputs hash covers src/ and the three package files in path order", () => {
  const root = path.join(work, `inputs${serial++}`);
  const files = {
    "src/main.ts": "export {};\n",
    "src/domains/x.ts": "export const x = 1;\n",
    "bundle.config.json": "{}\n",
    "package.json": '{"name":"t"}\n',
    "pnpm-lock.yaml": "lockfileVersion: 9\n",
  };
  for (const [rel, text] of Object.entries(files)) {
    mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
    writeFileSync(path.join(root, rel), text);
  }
  assert.equal(inputsSha256(root), "6027c7bbb74688d319212a529039e042018e8f2d2645ef70e939e17676c5e00c");
  writeFileSync(path.join(root, "src/domains/x.ts"), "export const x = 2;\n");
  assert.notEqual(inputsSha256(root), "6027c7bbb74688d319212a529039e042018e8f2d2645ef70e939e17676c5e00c");
});
