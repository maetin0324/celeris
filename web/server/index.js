import { createApp, parseBind } from "./app.js";

try {
  const bind = parseBind(process.env.CELERIS_WEB_BIND);
  const app = createApp({
    bind,
    daemonUrl: process.env.CELERIS_API_URL ?? "http://127.0.0.1:7710",
    daemonTokenFile: process.env.CELERIS_API_TOKEN_FILE,
    chatUploadLimitBytes: process.env.CELERIS_WEB_CHAT_UPLOAD_LIMIT_BYTES,
  });
  const server = app.listen(bind.port, bind.host, () => {
    process.stderr.write(`celeris-web: listening on ${bind.host}:${bind.port}\n`);
  });
  server.on("error", (error) => {
    process.stderr.write(`celeris-web: ${error.message}\n`);
    process.exitCode = 2;
  });
} catch (error) {
  process.stderr.write(`celeris-web: ${error.message}\n`);
  process.exitCode = 2;
}
