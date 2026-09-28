import type { BrowserRun } from "~/celeris/types";
import { Badge } from "~/components/ui/badge";
import { buttonClass } from "~/components/ui/button";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import { safeBrowserLiveUrl } from "~/lib/browser";

export function BrowserRunsPanel({ runs, activeRunIds }: { runs: BrowserRun[]; activeRunIds: string[] }) {
  if (runs.length === 0) return null;
  return (
    <section aria-label="Browser sessions" data-testid="browser-runs">
      <Card>
        <CardHeader title="ブラウザ" />
        <CardBody className="space-y-4">
          {runs.map((run) => {
            const running = run.state === "RUNNING" && activeRunIds.includes(run.run_id);
            const href = running ? safeBrowserLiveUrl(run.live_view_url) : null;
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
                  <>
                    <a
                      href={href}
                      target="_blank"
                      rel="noopener noreferrer"
                      className={buttonClass({ variant: "secondary", size: "sm" })}
                    >
                      Open Browser Live View
                    </a>
                    <p className="text-xs text-fg-subtle">
                      管理者用ダッシュボードで上記のセッションを選択してください。他の実行も表示されることがあります。
                    </p>
                  </>
                ) : running ? (
                  <p className="text-sm text-fg-muted">
                    Live View は未設定です。管理者が HTTPS ダッシュボードを設定すると開けます。
                  </p>
                ) : (
                  <p className="text-sm text-fg-muted">Live View はブラウザ実行中のみ利用できます。</p>
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
