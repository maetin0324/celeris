// src/main.ts を単一の ESM（dist/bundle.mjs）にし、dist/manifest.json を書く（境界 ADR 2026-11-02 (d)(e)）。
// 型検査はしない（tsc --noEmit は別の段）。daemon が読むのは dist の 2 file だけ。
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { isBuiltin } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build, version as esbuildVersion } from "esbuild";

const pkg = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const schemaPath = path.resolve(pkg, "../docs/api/script-host/host-v1.schema.json");
const HOST_MODULE = "celeris:host";
const SCHEME = /^[a-zA-Z][a-zA-Z0-9+.-]*:/;

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

// celeris:host だけを外に残す。node:・bun:・https: などの scheme 付きと Node の組み込み名（fs 等）は
// package が同名でも落とす。式で組み立てる動的 import はここでは見えず、Rust の loader が実行時に拒否する。
const importPolicy = {
  name: "celeris-import-policy",
  setup(pluginBuild) {
    pluginBuild.onResolve({ filter: /.*/ }, (args) => {
      if (args.path === HOST_MODULE) return { path: HOST_MODULE, external: true };
      if (args.kind === "entry-point") return undefined;
      if (isBuiltin(args.path) || SCHEME.test(args.path) || args.path.startsWith("//"))
        return { errors: [{ text: `import of "${args.path}" is not allowed in a Celeris bundle` }] };
      return undefined;
    });
  },
};

function sourceCommit() {
  const git = (...args) => execFileSync("git", args, { cwd: pkg, encoding: "utf8" }).trim();
  const commit = git("rev-parse", "HEAD");
  // 未 commit の変更を含む bundle を、その commit の産物と偽らない。
  const dirty = git("status", "--porcelain", "--", ".", "../docs/api/script-host");
  return dirty ? `${commit}-dirty` : commit;
}

function declaredCapabilities(schema, config) {
  const known = new Set(schema.$defs.Capability.oneOf.map((entry) => entry.const));
  const declared = [...new Set(Object.values(config.domains).flat())].sort();
  const unknown = declared.filter((capability) => !known.has(capability));
  if (unknown.length > 0) throw new Error(`unknown capability in bundle.config.json: ${unknown.join(", ")}`);
  return declared;
}

export async function buildBundle({ entry = "src/main.ts", outDir = "dist", config } = {}) {
  const out = path.resolve(pkg, outDir);
  // 失敗したときに古い bundle と manifest を残さない。
  rmSync(path.join(out, "bundle.mjs"), { force: true });
  rmSync(path.join(out, "manifest.json"), { force: true });

  const schemaBytes = readFileSync(schemaPath);
  const bundleConfig = config ?? JSON.parse(readFileSync(path.join(pkg, "bundle.config.json"), "utf8"));
  const capabilities = declaredCapabilities(JSON.parse(schemaBytes.toString("utf8")), bundleConfig);

  const result = await build({
    absWorkingDir: pkg,
    entryPoints: [entry],
    outfile: path.join(out, "bundle.mjs"),
    bundle: true,
    format: "esm",
    platform: "neutral",
    target: "esnext",
    charset: "utf8",
    legalComments: "none",
    write: false,
    metafile: true,
    logLevel: "silent",
    plugins: [importPolicy],
  });
  if (result.outputFiles.length !== 1) throw new Error(`expected 1 output, got ${result.outputFiles.length}`);
  const imports = Object.values(result.metafile.outputs).flatMap((output) => output.imports.map((i) => i.path));
  const stray = imports.filter((name) => name !== HOST_MODULE);
  if (stray.length > 0) throw new Error(`bundle keeps imports other than ${HOST_MODULE}: ${stray.join(", ")}`);

  const bundle = result.outputFiles[0].contents;
  const typescript = JSON.parse(readFileSync(path.join(pkg, "node_modules/typescript/package.json"), "utf8"));
  const manifest = {
    schema: "celeris.script-manifest/1",
    bundle_sha256: sha256(bundle),
    host_api: bundleConfig.host_api,
    schema_sha256: sha256(schemaBytes),
    toolchain: { typescript: typescript.version, esbuild: esbuildVersion, node: process.versions.node },
    source_commit: sourceCommit(),
    domains: Object.keys(bundleConfig.domains).sort(),
    capabilities,
    checkpoint_versions: bundleConfig.checkpoint_versions,
  };
  mkdirSync(out, { recursive: true });
  writeFileSync(path.join(out, "bundle.mjs"), bundle);
  writeFileSync(path.join(out, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const manifest = await buildBundle();
    console.log(`bundle: dist/bundle.mjs sha256=${manifest.bundle_sha256}`);
  } catch (err) {
    console.error(`bundle: ${err.message}`);
    process.exitCode = 1;
  }
}
