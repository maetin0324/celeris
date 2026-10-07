import type http from "node:http";
import { createApp } from "../../server/app.js";

// e2e 用の gateway。loopback の空き port で起こす。dist/ は playwright の webServer の build が作る。
// `port` は再起動の試験（同じ origin で起こし直す）だけが渡す。
export async function startGateway(options: Parameters<typeof createApp>[0] = {}, port = 0) {
  const server: http.Server = createApp({ log: () => {}, ...options }).listen(port, "127.0.0.1");
  await new Promise<void>((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("gateway did not bind TCP");
  return {
    base: `http://127.0.0.1:${address.port}`,
    port: address.port,
    // browser の live proxy（WS upgrade）を付ける試験（browser-gateway.ts）が使う。
    server,
    close: () =>
      new Promise<void>((resolve) => {
        server.close(() => resolve());
        server.closeAllConnections(); // SSE の接続を待たない
      }),
  };
}
