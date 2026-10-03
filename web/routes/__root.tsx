import { createRootRoute, Outlet, redirect, useRouterState } from "@tanstack/react-router";
import { NotFound } from "../components/shell/not-found";
import { Shell } from "../components/shell/shell";
import { fetchSession, safeNextPath } from "../lib/session";

// 全画面の共通の枠。認証の境界は gateway のローカル session だけで決める（daemon の health を条件にしない）。
// shell は daemon の状態で mount を変えない。server state を待つ Suspense をここに置かない（P2-02）。
export const Route = createRootRoute({
  beforeLoad: async ({ location }) => {
    if (location.pathname === "/login") return;
    const session = await fetchSession();
    if (!session.authenticated) throw redirect({ to: "/login", search: { next: safeNextPath(location.href) } });
  },
  component: RootLayout,
  notFoundComponent: NotFound,
});

function RootLayout() {
  const isLogin = useRouterState({ select: (state) => state.location.pathname === "/login" });
  return (
    <div className="min-h-dvh bg-background text-foreground">
      {isLogin ? (
        <Outlet />
      ) : (
        <Shell>
          <Outlet />
        </Shell>
      )}
    </div>
  );
}
