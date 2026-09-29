import { createApp, parseBind } from "./app.js";

try {
  const bind = parseBind(process.env.CELERIS_WEB_BIND);
  const app = createApp({ bind });
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
