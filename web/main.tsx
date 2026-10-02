import { createRouter, RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { configureApiClient } from "./api/client";
import { SessionQueryProvider } from "./api/provider";
import { bindQueryClientToSession } from "./api/query-client";
import { onUnauthenticated } from "./lib/session";
import { routeTree } from "./routeTree.gen";
import "./styles.css";

// 戻る・進むの scroll 位置は shell（components/shell/scroll-memory.ts）が戻す。
const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

// 401 は session 失効。QueryClient の cache を捨て、進行中の fetch を止めてから login へ移る。
bindQueryClientToSession();
configureApiClient({ onUnauthorized: () => onUnauthenticated(`${window.location.pathname}${window.location.search}`) });

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <SessionQueryProvider>
        <RouterProvider router={router} />
      </SessionQueryProvider>
    </StrictMode>,
  );
}
