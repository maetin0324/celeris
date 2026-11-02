// scripts/dev/test-affected.sh が control-ts の検査を選んで走らせることを小さな git fixture で確かめる
// （web の選択は scripts/dev/tests/test-affected-list.sh が確かめる）。偽の pnpm を使い、cargo の試験は走らせない。
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";

const script = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../scripts/dev/test-affected.sh");
const work = mkdtempSync(path.join(process.env.TMPDIR ?? tmpdir(), "control-ts-affected-"));
after(() => rmSync(work, { recursive: true, force: true }));

const write = (rel, text) => {
  const file = path.join(work, rel);
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, text);
  return file;
};
const git = (...args) => execFileSync("git", args, { cwd: work, encoding: "utf8" }).trim();

write("Cargo.toml", '[workspace]\nmembers = ["crates/a"]\nresolver = "2"\n');
write("crates/a/Cargo.toml", '[package]\nname = "a"\nversion = "0.1.0"\nedition = "2021"\n');
write("crates/a/src/lib.rs", "");
write(
  "control-ts/package.json",
  JSON.stringify({ scripts: { typecheck: "tsc --noEmit", test: "vitest run" }, packageManager: "pnpm@1.0.0" }),
);
write("docs/api/script-host/host-v1.schema.json", "{}\n");
write("docs/api/v1/api-v1.schema.json", "{}\n");
write(".gitignore", "node_modules\ntarget\n");
write("scripts/dev/check-health.sh", "#!/usr/bin/env bash\nexit 0\n");
// 偽の pnpm は install で偽の tsc と vitest を置く。呼ばれた順を CALLS に残す。
const calls = path.join(work, ".calls");
const fake = write(
  ".bin/pnpm",
  `#!/bin/sh
echo "pnpm $*" >>"$CALLS"
mkdir -p node_modules/.bin
: >node_modules/.modules.yaml
printf '#!/bin/sh\\necho "tsc $*" >>"$CALLS"\\nexit "\${FAKE_TSC_RC:-0}"\\n' >node_modules/.bin/tsc
printf '#!/bin/sh\\necho "vitest $*" >>"$CALLS"\\n' >node_modules/.bin/vitest
chmod +x node_modules/.bin/tsc node_modules/.bin/vitest
`,
);
chmodSync(fake, 0o755);
git("init", "-q", "-b", "main", ".");
git("config", "user.email", "t@example.com");
git("config", "user.name", "t");
git("add", "-A");
git("commit", "-q", "-m", "base");
const base = git("rev-parse", "HEAD");

function run(args, extraEnv = {}) {
  return spawnSync("bash", [script, ...args], {
    cwd: work,
    encoding: "utf8",
    env: { ...process.env, CELERIS_WU_BASE: base, CALLS: calls, PATH: `${path.dirname(fake)}:${process.env.PATH}`, ...extraEnv },
  });
}
const reset = () => {
  git("reset", "-q", "--hard", base);
  git("clean", "-qfd", "-e", ".calls");
  rmSync(calls, { force: true });
};

test("--list selects control-ts on a change under control-ts/", () => {
  reset();
  write("control-ts/src/x.ts", "");
  assert.equal(run(["--list"]).stdout, "control-ts: typecheck+test (理由: control-ts/ の変更)\n");
});

test("--list selects control-ts on a change of the host schema", () => {
  reset();
  write("docs/api/script-host/host-v1.schema.json", '{"x":1}\n');
  const lines = run(["--list"]).stdout.trim().split("\n");
  assert.ok(lines.includes("control-ts: typecheck+test (理由: docs/api/script-host/ の変更（生成型の元の schema）)"), `${lines}`);
});

test("--list does not select control-ts on other changes", () => {
  reset();
  write("crates/a/src/lib.rs", "// x\n");
  write("docs/api/v1/api-v1.schema.json", '{"x":1}\n');
  assert.doesNotMatch(run(["--list"]).stdout, /control-ts/);
});

test("a run installs, typechecks and tests control-ts", () => {
  reset();
  write("control-ts/src/x.ts", "");
  const result = run([]);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(
    readFileSync(calls, "utf8"),
    "pnpm install --offline --frozen-lockfile\ntsc --noEmit\nvitest run\n",
  );
});

test("a failed typecheck fails test-affected before the unit tests", () => {
  reset();
  write("control-ts/src/x.ts", "");
  const result = run([], { FAKE_TSC_RC: "2" });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /control-ts typecheck が失敗した \(exit 2\)/);
  assert.equal(readFileSync(calls, "utf8").includes("vitest"), false);
});
