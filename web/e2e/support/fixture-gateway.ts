import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, type FakeDaemonOptions } from "./fake-daemon.mjs";
import { startGateway } from "./gateway";
import { makeTmpDir } from "./tmp-dir";

// 偽 daemon（空き port）に中継する gateway。画面を T1 / R1 / cos / P1 の fixture で描く。
// options は状態の変種（states.ts）の偽 daemon の設定。既定は rich profile のまま。
export async function startFixtureGateway(options: FakeDaemonOptions = {}) {
  const dir = makeTmpDir("celeris-web-a11y-");
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ profile: "rich", ...options, token: FIXTURE_TOKEN });
  const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
  return {
    base: gateway.base,
    daemon,
    async close() {
      await gateway.close();
      await daemon.close();
      rmSync(dir, { recursive: true, force: true });
    },
  };
}
