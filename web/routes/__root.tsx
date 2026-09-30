import { createRootRoute, Outlet, redirect } from "@tanstack/react-router";
import { fetchSession, safeNextPath } from "../lib/session";

// 全画面の共通の枠。shell（ナビ・server state）は Phase 2（P2-02）で足す。
// 認証の境界は gateway のローカル session だけで決める（daemon の health を条件にしない）。
export const Route = createRootRoute({
  beforeLoad: async ({ location }) => {
    if (location.pathname === "/login") return;
    const session = await fetchSession();
    if (!session.authenticated) throw redirect({ to: "/login", search: { next: safeNextPath(location.href) } });
  },
  component: RootLayout,
});

function RootLayout() {
  return (
    <div className="min-h-dvh bg-white text-neutral-900">
      <Outlet />
    </div>
  );
}
