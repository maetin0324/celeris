import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "./fake-daemon.mjs";
import { startGateway } from "./gateway";

// 偽 daemon（空き port）に中継する gateway。画面を T1 / R1 / cos / P1 の fixture で描く。
export async function startFixtureGateway() {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-a11y-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
  return {
    base: gateway.base,
    async close() {
      await gateway.close();
      await daemon.close();
      rmSync(dir, { recursive: true, force: true });
    },
  };
}
