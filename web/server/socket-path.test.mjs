// unix socket の path 長（sun_path 107 byte）: daemon の検査は TMPDIR を workspace 配下の長い path に向ける。
// 試験の socket が TMPDIR の長さに依らず収まること、本体が長すぎる path を明示の error で拒むことを確かめる。
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { createBrowserLive } from "./browser-live.js";
import { assertSocketPath, makeSocketDir, SUN_PATH_MAX } from "./socket-path.js";

const webDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const cleanup = [];
after(() => {
  for (const dir of cleanup) rmSync(dir, { recursive: true, force: true });
});

// 長さ bytes の私有の dir（daemon の runs/exec-tmp-* 相当）を作る。TMPDIR 自体が長ければ /tmp を根にする。
function longTmpDir(bytes) {
  const base = Buffer.byteLength(tmpdir()) + 30 < bytes ? tmpdir() : "/tmp";
  const root = mkdtempSync(path.join(base, "long-tmpdir-"));
  cleanup.push(root);
  const dir = path.join(root, "x".repeat(bytes - Buffer.byteLength(root) - 1));
  mkdirSync(dir, { mode: 0o700 });
  assert.equal(Buffer.byteLength(dir), bytes);
  return dir;
}

test("assertSocketPath accepts 107 bytes and rejects 108 with the path in the message", () => {
  const ok = `/${"a".repeat(SUN_PATH_MAX - 1)}`;
  assert.equal(assertSocketPath(ok), ok);
  const long = `${ok}b`;
  assert.throws(() => assertSocketPath(long), /unix socket path is 108 bytes, longer than sun_path \(107\)/);
});

test("makeSocketDir falls back to /tmp when the base is too long, and the dir is private", () => {
  const base = longTmpDir(110);
  const dir = makeSocketDir("cst-", "device-owner-9999.sock", base);
  cleanup.push(dir);
  assert.equal(path.dirname(dir), "/tmp");
  assert.ok(Buffer.byteLength(path.join(dir, "device-owner-9999.sock")) <= SUN_PATH_MAX);
  assert.equal(statSync(dir).mode & 0o777, 0o700);
  const short = makeSocketDir("cst-", "owner.sock", "/tmp");
  cleanup.push(short);
  assert.equal(path.dirname(short), "/tmp");
});

test("startSocket refuses an owner socket path longer than sun_path with an explicit error", () => {
  const dir = longTmpDir(110);
  const ownerSocket = path.join(dir, "device-owner-8.sock");
  const live = createBrowserLive({ auth: { onLogout() {} }, ownerSocket });
  assert.throws(() => live.startSocket(), /longer than sun_path/);
  assert.equal(existsSync(ownerSocket), false);
});

// 80 byte: owner.sock は収まるが device-owner-<n>.sock は収まらない幅（daemon で止まった長さ）。
// 110 byte: どの socket 名も収まらない。どちらでも子の node --test が宙づりにならず全件通る。
for (const bytes of [80, 110]) {
  test(`browser-live tests pass under a ${bytes}-byte TMPDIR`, () => {
    const dir = longTmpDir(bytes);
    // 親の node --test が置く NODE_TEST_CONTEXT を渡すと、子は結果を親向けの形式で出し stdout が空になる。
    const { NODE_TEST_CONTEXT: _, ...env } = process.env;
    const run = spawnSync(
      process.execPath,
      ["--test", "server/browser-live.test.mjs", "server/browser-live-frames-prod.test.mjs"],
      { cwd: webDir, env: { ...env, TMPDIR: dir }, encoding: "utf8", timeout: 120_000, killSignal: "SIGKILL" },
    );
    assert.equal(run.signal, null, `child timed out:\n${run.stdout}\n${run.stderr}`);
    assert.equal(run.status, 0, `${run.stdout}\n${run.stderr}`);
    assert.match(run.stdout, /ℹ pass [1-9]/);
    assert.match(run.stdout, /ℹ fail 0/);
  });
}
