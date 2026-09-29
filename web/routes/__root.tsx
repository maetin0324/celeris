import { createRootRoute, Outlet } from "@tanstack/react-router";

// 全画面の共通の枠。shell（ナビ・server state）は Phase 2（P2-02）で足す。
export const Route = createRootRoute({
  component: RootLayout,
});

function RootLayout() {
  return (
    <div className="min-h-dvh bg-white text-neutral-900">
      <Outlet />
    </div>
  );
}
