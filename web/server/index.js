import { createApp, parseBind } from "./app.js";

try {
  // ADR 2026-10-27-api-principals D5: BFF は web 専用 token だけを持つ。旧い共有 token の env は起動エラー。
  if (process.env.CELERIS_API_TOKEN_FILE !== undefined) {
    throw new Error("CELERIS_API_TOKEN_FILE was removed; set CELERIS_WEB_API_TOKEN_FILE to the human:web token file");
  }
  const bind = parseBind(process.env.CELERIS_WEB_BIND);
  const app = createApp({
    bind,
    daemonUrl: process.env.CELERIS_API_URL ?? "http://127.0.0.1:7710",
    daemonTokenFile: process.env.CELERIS_WEB_API_TOKEN_FILE,
    chatUploadLimitBytes: process.env.CELERIS_WEB_CHAT_UPLOAD_LIMIT_BYTES,
  });
  const ownerServer = app.locals.browserLive.startSocket();
  const server = app.listen(bind.port, bind.host, () => {
    process.stderr.write(`celeris-web: listening on ${bind.host}:${bind.port}\n`);
  });
  server.on("upgrade", app.locals.browserLiveUpgrade);
  // ADR 2026-10-27-web-terminal D2: 停止（release の昇格を含む）では端末を閉じ、終了を記録してから終わる。
  process.once("SIGTERM", () => {
    app.locals.terminal.shutdown().finally(() => process.exit(0));
  });
  server.on("error", (error) => {
    process.stderr.write(`celeris-web: ${error.message}\n`);
    ownerServer?.close();
    process.exitCode = 2;
  });
  ownerServer?.on("error", (error) => {
    process.stderr.write(`celeris-web: owner socket: ${error.message}\n`);
    server.close();
    process.exitCode = 2;
  });
} catch (error) {
  process.stderr.write(`celeris-web: ${error.message}\n`);
  process.exitCode = 2;
}
