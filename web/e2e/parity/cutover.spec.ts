import { execFileSync, spawn } from "node:child_process";
import { readdirSync, rmSync } from "node:fs";
import net from "node:net";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { makeTmpDir } from "../support/tmp-dir";

const webRoot = path.resolve(import.meta.dirname, "../..");

async function freePort() {
  const server = net.createServer();
  await new Promise<void>((resolve, reject) => server.listen(0, "127.0.0.1", resolve).once("error", reject));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("missing TCP port");
  await new Promise<void>((resolve) => server.close(() => resolve()));
  return address.port;
}

test("parity-x: web の配布物が offline install で動く", async () => {
  test.setTimeout(180_000);
  const release = "parity-cutover";
  execFileSync("bash", ["scripts/release.sh"], {
    cwd: webRoot,
    env: { ...process.env, CELERIS_WEB_RELEASE: release },
    stdio: "pipe",
  });
  const archive = path.join(webRoot, "release", `celeris-web-0.1.0-${release}.tar.gz`);
  const unpack = makeTmpDir("celeris-web-release-");
  let child: ReturnType<typeof spawn> | undefined;
  try {
    execFileSync("tar", ["-xzf", archive, "-C", unpack]);
    const bundle = path.join(unpack, readdirSync(unpack)[0]);
    execFileSync("corepack", ["pnpm@12.6.0", "-C", bundle, "install", "--prod", "--offline", "--frozen-lockfile"], {
      stdio: "pipe",
    });
    const port = await freePort();
    const childEnv = { ...process.env };
    delete childEnv.CELERIS_WEB_RELEASE;
    child = spawn(process.execPath, ["server/index.js"], {
      cwd: bundle,
      env: {
        ...childEnv,
        CELERIS_WEB_BIND: `127.0.0.1:${port}`,
        CELERIS_API_URL: "http://127.0.0.1:1",
      },
      stdio: "ignore",
    });
    let health: Response | undefined;
    for (let attempt = 0; attempt < 40; attempt++) {
      try {
        health = await fetch(`http://127.0.0.1:${port}/healthz`);
        break;
      } catch {
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    }
    expect(health?.status).toBe(200);
    expect(await health?.json()).toMatchObject({ name: "celeris-web", release });
  } finally {
    child?.kill("SIGTERM");
    rmSync(unpack, { recursive: true, force: true });
  }
});
