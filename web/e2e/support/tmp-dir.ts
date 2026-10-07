import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

// e2e の一時 dir の作成と後片付け（ADR 2026-10-07-build-tmp-hygiene D3）。
// spec は mkdtempSync を直接呼ばずここを使う。作った dir は process 終了時にも必ず消す。
const created = new Set<string>();
let hooked = false;

/** `<tmpdir>/<prefix>XXXXXX` を作り、終了時の削除に登録する。 */
export function makeTmpDir(prefix: string): string {
  const dir = mkdtempSync(path.join(tmpdir(), prefix));
  created.add(dir);
  if (!hooked) {
    hooked = true;
    process.on("exit", cleanupTmpDirs);
  }
  return dir;
}

/** 1 つの dir を消す（afterAll 向け）。登録済みでなくても消す。 */
export function removeTmpDir(dir: string): void {
  rmSync(dir, { recursive: true, force: true });
  created.delete(dir);
}

/** 登録済みの dir をすべて消す。 */
export function cleanupTmpDirs(): void {
  for (const dir of [...created]) removeTmpDir(dir);
}
