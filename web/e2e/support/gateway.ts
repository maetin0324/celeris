import type http from "node:http";
import { createApp } from "../../server/app.js";

// e2e 用の gateway。loopback の空き port で起こす。dist/ は playwright の webServer の build が作る。
export async function startGateway(options: Parameters<typeof createApp>[0] = {}) {
  const server: http.Server = createApp({ log: () => {}, ...options }).listen(0, "127.0.0.1");
  await new Promise<void>((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("gateway did not bind TCP");
  return {
    base: `http://127.0.0.1:${address.port}`,
    port: address.port,
    close: () => new Promise<void>((resolve) => server.close(() => resolve())),
  };
}
