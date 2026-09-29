import type { BrowserRun } from "~/celeris/types";
import { Badge } from "~/components/ui/badge";
import { buttonClass } from "~/components/ui/button";
import { Card, CardBody, CardHeader } from "~/components/ui/card";

/**
 * ADR-0080 D6: Live View への導線は、サーバが本人の session について認可した同一 origin の
 * `/browser/live/{task_id}/{run_id}` だけ。raw `live_view_url`（全 session が見える dashboard）は href にしない
 * （loader が値を消してから渡す）。
 */
export type LiveViewState =
  | { state: "link"; href: string }
  | {
      state: "disabled";
      reason:
        | "owner_unavailable"
        | "not_owner"
        | "not_running"
        | "not_configured"
        | "auth_interval"
        | "relay_unavailable";
    };

const DISABLED_TEXT: Record<Extract<LiveViewState, { state: "disabled" }>["reason"], string> = {
  owner_unavailable: "Live View は、パスワード認証を有効にした単一所有者の Celeris でだけ使えます。",
  not_owner: "Live View は本人として登録したセッションでだけ開けます。",
  not_running: "Live View はブラウザ実行中のみ利用できます。",
  not_configured: "Live View は未設定です。",
  auth_interval: "認証を扱う区間のため Live View は止めています。",
  relay_unavailable: "Live View は利用できません（本人専用の読み取り中継が設定されていません）。",
};

/** `/browser/live/...` の同一 origin の相対パスだけを href として受け付ける。 */
function safeLivePath(href: string): string | null {
  return /^\/browser\/live\/[0-9A-Za-z_-]{1,64}\/[0-9A-Za-z_-]{1,64}$/.test(href) ? href : null;
}

export function BrowserRunsPanel({
  runs,
  liveViews,
}: {
  runs: BrowserRun[];
  liveViews: Record<string, LiveViewState>;
}) {
  if (runs.length === 0) return null;
  return (
    <section aria-label="Browser sessions" data-testid="browser-runs">
      <Card>
        <CardHeader title="ブラウザ" />
        <CardBody className="space-y-4">
          {runs.map((run) => {
            const live: LiveViewState = liveViews[run.run_id] ?? { state: "disabled", reason: "not_running" };
            const href = live.state === "link" ? safeLivePath(live.href) : null;
            return (
              <div key={run.run_id} className="space-y-2 break-words">
                <Badge tone={run.state === "FAILED" ? "danger" : "neutral"}>{run.state}</Badge>
                <p className="text-sm text-fg-muted">
                  実行: <span className="font-mono">{run.run_id}</span>
                </p>
                <p className="text-sm text-fg-muted">
                  セッション: <span className="font-mono">{run.session_id}</span>
                </p>
                {href ? (
                  <a
                    href={href}
                    target="_blank"
                    rel="noopener noreferrer"
                    className={buttonClass({ variant: "secondary", size: "sm" })}
                  >
                    Open Browser Live View
                  </a>
                ) : (
                  <p className="text-sm text-fg-muted">
                    {live.state === "disabled" ? DISABLED_TEXT[live.reason] : DISABLED_TEXT.not_running}
                  </p>
                )}
              </div>
            );
          })}
          <p className="text-xs text-fg-subtle">
            操作の記録はタイムライン、スクリーンショットと抽出結果は成果物で確認できます。
          </p>
        </CardBody>
      </Card>
    </section>
  );
}
