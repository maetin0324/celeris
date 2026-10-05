import { Link, useLocation, useRouter } from "@tanstack/react-router";
import { Dialog } from "radix-ui";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { badgeView } from "../../api/queries/badges";
import type { ConnectionState } from "../../api/realtime/connection-state";
import { useConnectionState, useRealtimeSubscription } from "../../api/realtime/use-realtime";
import { NotificationsWatcher } from "../../features/reports/notifications-watcher";
import { formatAbsolute, formatRelative, useLastHello } from "../../lib/time";
import { Badge, type BadgeTone } from "../ui/badge";
import { buttonVariants } from "../ui/button";
import { Icon } from "../ui/icon";
import { mobileOtherItems, mobileTabs, navGroups, navItems } from "./nav-items";
import { installScrollMemory } from "./scroll-memory";
import { useShellServerState } from "./use-server-state";

type NavBadgeView = ReturnType<typeof badgeView>;

// root の shell（P2-02）。ナビ・ヘッダ・Console の置き場・接続状態・outlet を持つ。
// daemon の状態では mount を変えない（server state を待つ Suspense や条件付きの描画を置かない）。
// md 以上は幅 --spacing-nav の左列、md 未満は上の帯と「メニュー」で開く一覧（DESIGN.md「幅ごとの規則」）。
export function Shell({ children }: { children: ReactNode }) {
  const router = useRouter();
  const [menuOpen, setMenuOpen] = useState(false);
  const menuButton = useRef<HTMLButtonElement>(null);
  // 画面下のタブの「その他」のシート。遷移で閉じたときは focus を「その他」へ戻さず、遷移先の見出しへ移す。
  const [moreOpen, setMoreOpen] = useState(false);
  const moreOpenRef = useRef(false);
  moreOpenRef.current = moreOpen;
  const moreClosedByNavigation = useRef(false);
  // shell は root の認証 gate を通ったときだけ mount される。
  const server = useShellServerState(true);
  const badges: Record<string, { view: NavBadgeView; tone: BadgeTone }> = {
    "/inbox": { view: badgeView(server.inboxBadge, "受信箱"), tone: "warning" },
    "/notifications": { view: badgeView(server.notificationsBadge, "未読の通知"), tone: "info" },
  };

  useRealtimeSubscription();
  const realtime = useConnectionState();
  const connection = connectionView(server.down, realtime);
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
        if (moreOpenRef.current) {
          moreClosedByNavigation.current = true;
          setMoreOpen(false);
        }
        requestAnimationFrame(() => {
          const heading = document.querySelector<HTMLElement>("#main h1");
          heading?.focus({ preventScroll: true });
        });
      }),
    [router],
  );

  // 開いた一覧は Escape で閉じ、focus を「メニュー」へ戻す。
  useEffect(() => {
    if (!menuOpen) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setMenuOpen(false);
      menuButton.current?.focus();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [menuOpen]);

  return (
    <div data-shell className="flex min-h-dvh flex-col bg-background text-foreground md:flex-row">
      {/* skip link。Tab の最初の行き先で、focus したときだけ画面の上に出る（隠れている間も 44px の大きさは保つ）。
          JS を使わない素の fragment link で、押すと main（tabIndex=-1）へ focus が移る。 */}
      <a
        href="#main"
        data-skip-link
        className="absolute top-2 left-4 z-20 inline-flex min-h-11 -translate-y-24 items-center rounded-md border border-border bg-surface px-4 text-body font-medium text-primary shadow-popover focus:translate-y-0"
      >
        本文へ移動
      </a>
      <NotificationsWatcher unread={server.notificationsUnread} />
      <header className="relative flex flex-wrap items-center gap-2 border-b border-border bg-background px-4 py-1 md:sticky md:top-0 md:h-dvh md:w-nav md:shrink-0 md:flex-col md:flex-nowrap md:items-stretch md:gap-3 md:overflow-y-auto md:border-r md:border-b-0 md:px-3 md:py-4">
        <Link
          to="/"
          className="inline-flex min-h-11 min-w-11 items-center rounded-md text-section font-semibold text-foreground md:px-3"
        >
          Celeris
        </Link>
        <p data-connection={connection.state} role="status" className="min-w-0 md:px-3">
          <Badge tone={connection.tone}>接続状態: {connection.label}</Badge>
        </p>
        <button
          ref={menuButton}
          type="button"
          className={`${buttonVariants({ variant: "secondary", size: "sm" })} ml-auto md:hidden`}
          aria-expanded={menuOpen}
          aria-controls="shell-nav"
          onClick={() => setMenuOpen((open) => !open)}
        >
          <Icon name="menu" size="sm" />
          メニュー
        </button>
        <nav
          id="shell-nav"
          aria-label="主要"
          className={`${menuOpen ? "block" : "hidden"} absolute inset-x-0 top-full z-10 border-b border-border bg-surface p-2 shadow-popover md:static md:block md:border-0 md:bg-transparent md:p-0 md:shadow-flat`}
        >
          {navGroups.map((group) => (
            <div key={group.key} className="mt-2 first:mt-0 md:mt-4">
              <p id={`shell-nav-${group.key}`} className="px-3 py-1 text-label font-medium text-muted-foreground">
                {group.label}
              </p>
              <ul aria-labelledby={`shell-nav-${group.key}`} className="grid grid-cols-2 gap-1 md:grid-cols-1">
                {navItems
                  .filter((item) => item.group === group.key)
                  .map((item) => (
                    <li key={item.to} className="min-w-0">
                      <Link
                        to={item.to}
                        activeOptions={{ exact: item.to === "/" }}
                        activeProps={{ "aria-current": "page", className: "bg-accent font-semibold" }}
                        className="flex min-h-11 min-w-0 items-center justify-between gap-2 rounded-md px-3 text-body text-foreground hover:bg-accent"
                      >
                        <span className="min-w-0 break-words">{item.label}</span>
                        <NavBadge badge={badges[item.to]} />
                      </Link>
                    </li>
                  ))}
              </ul>
            </div>
          ))}
        </nav>
        {lastHello && (
          <p data-server-time className="hidden px-3 text-label text-muted-foreground md:mt-auto md:block">
            最終受信{" "}
            <time dateTime={lastHello} data-absolute>
              {formatAbsolute(lastHello)}
            </time>{" "}
            (<span data-relative>{formatRelative(lastHello)}</span>)
          </p>
        )}
      </header>
      {/* md 未満は画面下の固定タブ（MobileTabBar）が覆う分だけ列の下を空け、本文と Console の置き場を隠さない。 */}
      <div className="flex min-w-0 flex-1 flex-col bg-surface pb-shell-inset">
        {server.down && (
          <p
            role="alert"
            data-celeris-down
            className="border-b border-border bg-danger px-4 py-2 text-label font-medium text-danger-foreground md:px-6"
          >
            celeris に接続できません。復旧すると自動で再取得します。
          </p>
        )}
        <main id="main" tabIndex={-1} className="min-w-0 flex-1 p-4 focus:outline-none md:p-6">
          {children}
        </main>
        <aside data-console-slot aria-label="Console" className="border-t border-border" />
      </div>
      <MobileTabBar
        badges={badges}
        notificationsUnread={(server.notificationsBadge ?? 0) > 0}
        open={moreOpen}
        onOpenChange={setMoreOpen}
        closedByNavigation={moreClosedByNavigation}
      />
    </div>
  );
}

// 接続状態の語と tone。health が落ちていれば realtime の状態より優先して「切断」とする。
// 色だけに頼らず、必ず文言（接続状態: …）で伝える。
const connectionViews: Readonly<Record<ConnectionState | "down", { label: string; tone: BadgeTone }>> = {
  down: { label: "切断", tone: "danger" },
  open: { label: "接続済み", tone: "success" },
  // 「接続を確認中」は 360 で header を 2 段に折っていた（fix-r6 narrow）。他の語と同じ 3〜4 字に揃える。
  connecting: { label: "確認中", tone: "neutral" },
  reconnecting: { label: "再接続中", tone: "warning" },
  unauthorized: { label: "認証が必要", tone: "danger" },
  closed: { label: "未接続", tone: "neutral" },
};

function connectionView(down: boolean, realtime: ConnectionState) {
  const state = down ? "down" : realtime;
  return { state, ...connectionViews[state] };
}

function NavBadge({ badge }: { badge: { view: NavBadgeView; tone: BadgeTone } | undefined }) {
  if (!badge?.view) return null;
  // 件数が不明（?）のときは成功色や注意色にせず neutral にする（DESIGN.md「Badge」）。
  const tone = badge.view.text === "?" ? "neutral" : badge.tone;
  // shrink-0 は子の span に付け、Slot の連結で Badge の class に足す。className で渡すと cn が
  // tailwind-merge の初期化を起動直後の描画で走らせ、最初の遷移の click を待たせる（衝突する class は無い）。
  return (
    <Badge asChild data-badge tone={tone} role="img" aria-label={badge.view.label}>
      <span className="shrink-0">{badge.view.text}</span>
    </Badge>
  );
}

const tabClass =
  "relative flex min-h-11 min-w-0 flex-col items-center justify-center gap-1 text-label font-medium whitespace-nowrap hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring";

// md 未満の画面下の固定タブ（旧 GUI の MobileTabBar）。主要 4 つと「その他」。「その他」は残りの項目を下からのシートで出す。
// シートは Radix Dialog（focus trap・Escape・aria-modal・閉じた後に「その他」へ focus を戻す）。
function MobileTabBar({
  badges,
  notificationsUnread,
  open,
  onOpenChange,
  closedByNavigation,
}: {
  badges: Record<string, { view: NavBadgeView; tone: BadgeTone }>;
  notificationsUnread: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  closedByNavigation: { current: boolean };
}) {
  const pathname = useLocation({ select: (location) => location.pathname });
  const otherActive = mobileOtherItems.some((item) => pathname === item.to || pathname.startsWith(`${item.to}/`));
  return (
    <nav
      aria-label="主要（モバイル）"
      data-testid="mobile-tabbar"
      className="fixed inset-x-0 bottom-0 z-30 grid h-shell-inset grid-cols-5 border-t border-border bg-surface md:hidden"
      style={{ paddingBottom: "env(safe-area-inset-bottom)" }}
    >
      {mobileTabs.map((tab) => (
        <Link
          key={tab.to}
          to={tab.to}
          activeOptions={{ exact: tab.to === "/" }}
          activeProps={{ "aria-current": "page", className: "text-primary" }}
          inactiveProps={{ className: "text-muted-foreground" }}
          className={tabClass}
        >
          <Icon name={tab.icon} />
          <span>{tab.label}</span>
          {badges[tab.to] ? (
            <span className="absolute top-1 left-1/2 ml-2">
              <NavBadge badge={badges[tab.to]} />
            </span>
          ) : null}
        </Link>
      ))}
      <Dialog.Root open={open} onOpenChange={onOpenChange}>
        <Dialog.Trigger
          data-active={otherActive ? "true" : undefined}
          className={`${tabClass} ${open || otherActive ? "text-primary" : "text-muted-foreground"}`}
        >
          <Icon name="more-horizontal" />
          <span>その他</span>
          {notificationsUnread && !otherActive ? (
            <span aria-hidden="true" className="absolute top-2 left-1/2 ml-3 size-2 rounded-full bg-info-foreground" />
          ) : null}
        </Dialog.Trigger>
        <Dialog.Portal>
          <Dialog.Overlay className="fixed inset-0 bg-overlay transition-opacity duration-150 motion-reduce:transition-none" />
          <Dialog.Content
            aria-describedby={undefined}
            // Radix は背後を aria-hidden にするが aria-modal は付けない。読み上げにモーダルであることを明示する。
            aria-modal="true"
            data-testid="mobile-more-sheet"
            onCloseAutoFocus={(event) => {
              // 遷移で閉じたときは shell が遷移先の見出しへ focus を移す。
              if (!closedByNavigation.current) return;
              closedByNavigation.current = false;
              event.preventDefault();
            }}
            className="fixed inset-x-0 bottom-0 max-h-2/3 overflow-y-auto overscroll-contain rounded-t-lg border-t border-border bg-surface px-4 pt-4 text-foreground shadow-dialog"
            style={{ paddingBottom: "max(1rem, env(safe-area-inset-bottom))" }}
          >
            <div className="flex min-w-0 items-center justify-between gap-2">
              <Dialog.Title className="text-section font-semibold">その他</Dialog.Title>
              <Dialog.Close
                aria-label="閉じる"
                className="inline-flex min-h-11 min-w-11 shrink-0 items-center justify-center rounded-md text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
              >
                <Icon name="close" />
              </Dialog.Close>
            </div>
            {navGroups.map((group) => {
              const items = mobileOtherItems.filter((item) => item.group === group.key);
              if (items.length === 0) return null;
              return (
                <div key={group.key} className="mt-3">
                  <p id={`mobile-more-${group.key}`} className="py-1 text-label font-medium text-muted-foreground">
                    {group.label}
                  </p>
                  <ul aria-labelledby={`mobile-more-${group.key}`} className="grid grid-cols-2 gap-2">
                    {items.map((item) => (
                      <li key={item.to} className="min-w-0">
                        <Link
                          to={item.to}
                          activeProps={{ "aria-current": "page", className: "bg-accent font-semibold" }}
                          className="flex min-h-11 min-w-0 items-center justify-between gap-2 rounded-md border border-border px-3 text-body text-foreground hover:bg-accent"
                        >
                          <span className="min-w-0 break-words">{item.label}</span>
                          <NavBadge badge={badges[item.to]} />
                        </Link>
                      </li>
                    ))}
                  </ul>
                </div>
              );
            })}
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    </nav>
  );
}
