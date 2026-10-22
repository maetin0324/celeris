import { chmodSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

// Linux の sun_path は 108 byte（末尾の NUL を含む）。path は 107 byte まで。
export const SUN_PATH_MAX = 107;

// unix socket の path が sun_path に収まらなければ明示の error を投げる。
// 収まらない path で listen すると EINVAL になり、何の path が悪いかが分かりにくい。
export function assertSocketPath(socketPath) {
  const bytes = Buffer.byteLength(socketPath);
  if (bytes > SUN_PATH_MAX)
    throw new Error(`unix socket path is ${bytes} bytes, longer than sun_path (${SUN_PATH_MAX}): ${socketPath}`);
  return socketPath;
}

// 試験の socket を置く私有の dir を作る。base（既定は TMPDIR）の下だと longestName が sun_path に
// 収まらないときは /tmp 直下に作る。daemon の検査は TMPDIR を workspace 配下の長い path に向ける。
export function makeSocketDir(prefix, longestName, base = tmpdir()) {
  const fits = (root) => Buffer.byteLength(path.join(root, `${prefix}XXXXXX`, longestName)) <= SUN_PATH_MAX;
  const root = fits(base) ? base : "/tmp";
  if (!fits(root)) throw new Error(`no directory fits a unix socket named ${longestName}`);
  const dir = mkdtempSync(path.join(root, prefix));
  chmodSync(dir, 0o700);
  return dir;
}
