import { createRequestHandler } from "@react-router/express";
import express from "express";
import { RouterContextProvider } from "react-router";
import { startOwnerControlSocket } from "../app/browser-owner.server";
import { expressCsrfGuard } from "../app/middleware/security.server";

/** React Router のハンドラ（本番は build/server/index.js に入る。開発は Vite の ssrLoadModule で読む）。 */
export const app: express.Express = express();
app.disable("x-powered-by");

// ADR-0080 D6: `celerisctl browser owner-session approve <challenge>` を受ける Unix control socket
// （`CELERIS_GUI_OWNER_SOCKET` が設定されたときだけ。0700 directory / 0600 socket）。
startOwnerControlSocket();

// 変更系の CSRF 検査（403）。React Router の組み込み検査（400）より前に置く（docs/adr/0005 D1）。
app.use((req, res, next) => expressCsrfGuard(req, res, next));

app.use(
  createRequestHandler({
    build: () => import("virtual:react-router/server-build"),
    getLoadContext() {
      return new RouterContextProvider();
    },
  }),
);
