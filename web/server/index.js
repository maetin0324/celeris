import { createApp, parseBind } from "./app.js";

try {
  const bind = parseBind(process.env.CELERIS_WEB_BIND);
  const app = createApp({
    bind,
    daemonUrl: process.env.CELERIS_API_URL ?? "http://127.0.0.1:7710",
    daemonTokenFile: process.env.CELERIS_API_TOKEN_FILE,
  });
  const ownerServer = app.locals.browserLive.startSocket();
  const server = app.listen(bind.port, bind.host, () => {
    process.stderr.write(`celeris-web: listening on ${bind.host}:${bind.port}\n`);
  });
  server.on("upgrade", app.locals.browserLiveUpgrade);
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
