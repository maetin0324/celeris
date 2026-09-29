import { createRequestHandler } from "@react-router/express";
import express from "express";
import { RouterContextProvider } from "react-router";
import { startOwnerControlSocket } from "../app/browser-owner.server";
import {
  attachLiveViewUpgrade,
  handleLiveViewUpgrade,
  liveViewRelayMiddleware,
} from "../app/celeris/browser-live-relay.server";
import { expressCsrfGuard } from "../app/middleware/security.server";

/** React Router のハンドラ（本番は build/server/index.js に入る。開発は Vite の ssrLoadModule で読む）。 */
export const app: express.Express = express();
app.disable("x-powered-by");

// ADR-0080 D6: `celerisctl browser owner-session approve <challenge>` を受ける Unix control socket
// （`CELERIS_GUI_OWNER_SOCKET` が設定されたときだけ。0700 directory / 0600 socket）。
startOwnerControlSocket();

// ADR-0080 D6: Live View の読み取り専用 relay（`CELERIS_GUI_LIVE_VIEW_UPSTREAM` が設定されたときだけ）。
// dashboard の inline script を動かすため React Router の root middleware（nonce CSP）より前に置く。
// GET 以外は relay の中で全て拒否する（403 live_view_action_denied / 405）ので CSRF 検査より前でよい。
app.use((req, res, next) => liveViewRelayMiddleware(req, res, next));

/** `server.js` が http.Server の `upgrade` に付ける（Live View の WebSocket relay）。 */
export { attachLiveViewUpgrade, handleLiveViewUpgrade };

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
