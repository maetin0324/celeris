import { Link, useRouter } from "@tanstack/react-router";
import { type ReactNode, useEffect, useState } from "react";
import { badgeView } from "../../api/queries/badges";
import { useRealtimeSubscription } from "../../api/realtime/use-realtime";
import { formatAbsolute, formatRelative, useLastHello } from "../../lib/time";
import { buttonClassName } from "../ui/button";
import { navItems } from "./nav-items";
import { installScrollMemory } from "./scroll-memory";
import { useShellServerState } from "./use-server-state";

// root の shell（P2-02）。ナビ・ヘッダ・Console の置き場・接続状態・outlet を持つ。
// daemon の状態では mount を変えない（server state を待つ Suspense や条件付きの描画を置かない）。
export function Shell({ children }: { children: ReactNode }) {
  const router = useRouter();
  const [menuOpen, setMenuOpen] = useState(false);
  // shell は root の認証 gate を通ったときだけ mount される。
  const server = useShellServerState(true);
  const badges: Record<string, ReturnType<typeof badgeView>> = {
    "/inbox": badgeView(server.inboxBadge, "受信箱"),
    "/approvals": badgeView(server.approvalsBadge, "承認待ち"),
  };

  useRealtimeSubscription();
  const lastHello = useLastHello();
  const [, tick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => tick((n) => n + 1), 10_000);
    return () => clearInterval(id);
  }, []);

  useEffect(() => installScrollMemory(router), [router]);

  // 遷移が終わったら画面の見出しへ focus を移す（S4）。
  useEffect(
    () =>
      router.subscribe("onResolved", ({ pathChanged }) => {
        if (!pathChanged) return;
        setMenuOpen(false);
        requestAnimationFrame(() => {
          const heading = document.querySelector<HTMLElement>("#main h1");
          heading?.focus({ preventScroll: true });
        });
      }),
    [router],
  );

  return (
    <div data-shell className="flex min-h-dvh flex-col md:flex-row">
      <header className="flex items-center justify-between gap-2 border-b border-neutral-300 px-3 py-1 md:w-56 md:flex-col md:items-stretch md:justify-start md:border-r md:border-b-0 md:py-3">
        <Link to="/" className="inline-flex min-h-11 min-w-11 items-center text-lg font-semibold">
          Celeris
        </Link>
        <button
          type="button"
          className={`${buttonClassName} md:hidden`}
          aria-expanded={menuOpen}
          aria-controls="shell-nav"
          onClick={() => setMenuOpen((open) => !open)}
        >
          メニュー
        </button>
        <nav
          id="shell-nav"
          aria-label="主要"
          className={`${menuOpen ? "block" : "hidden"} absolute inset-x-0 top-12 z-10 border-b border-neutral-300 bg-white md:static md:block md:border-0`}
        >
          <ul className="grid grid-cols-2 gap-1 p-2 md:grid-cols-1 md:p-0">
            {navItems.map((item) => (
              <li key={item.to}>
                <Link
                  to={item.to}
                  activeOptions={{ exact: item.to === "/" }}
                  activeProps={{ "aria-current": "page", className: "bg-neutral-200 font-semibold" }}
                  className="flex min-h-11 items-center rounded px-3 hover:bg-neutral-100"
                >
                  {item.label}
                  <NavBadge badge={badges[item.to]} />
                </Link>
              </li>
            ))}
          </ul>
        </nav>
        <p data-connection role="status" className="hidden text-xs text-neutral-600 md:block">
          接続状態: 未確認
        </p>
        {lastHello && (
          <p data-server-time className="hidden text-xs text-neutral-600 md:block">
            最終受信{" "}
            <time dateTime={lastHello} data-absolute>
              {formatAbsolute(lastHello)}
            </time>{" "}
            (<span data-relative>{formatRelative(lastHello)}</span>)
          </p>
        )}
      </header>
      <div className="flex min-w-0 flex-1 flex-col">
        {server.down && (
          <p
            role="alert"
            data-celeris-down
            className="border-b border-red-300 bg-red-50 px-4 py-2 text-sm text-red-900"
          >
            celeris に接続できません。復旧すると自動で再取得します。
          </p>
        )}
        <main id="main" className="min-w-0 flex-1 p-4">
          {children}
        </main>
        <aside data-console-slot aria-label="Console" className="border-t border-neutral-300" />
      </div>
    </div>
  );
}

function NavBadge({ badge }: { badge: ReturnType<typeof badgeView> | undefined }) {
  if (!badge) return null;
  return (
    <span data-badge className="ml-2 rounded bg-neutral-200 px-1.5 text-xs" role="img" aria-label={badge.label}>
      {badge.text}
    </span>
  );
}
