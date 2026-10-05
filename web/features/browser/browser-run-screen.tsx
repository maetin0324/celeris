import { useQuery } from "@tanstack/react-query";
import { Notice } from "../../components/ui/notice";
import { Section } from "../../components/ui/panel";
import {
  BrowserGatewayError,
  browserRunsQuery,
  browserWaitsQuery,
  type OwnerSession,
  ownerSessionQuery,
} from "./browser-query";
import { BrowserWaitsList } from "./browser-waits-panel";
import { ControlBar } from "./control-bar";
import { LiveEvents } from "./live-events";
import { LiveViewFrame, liveViewState } from "./live-view-frame";
import { OwnerSessionNotice } from "./owner-session-notice";

// `/browser/runs/$taskId/$runId`（D3.1・D3.6）。狭幅でも 状態→ボタン（control bar）→Live View→待ち→イベント の順に積む。

const UNAVAILABLE: OwnerSession = { available: false, isOwner: false, csrfToken: null };

function useOwner(): OwnerSession | undefined {
  const owner = useQuery({ ...ownerSessionQuery(), retry: false });
  if (owner.data) return owner.data;
  if (owner.error instanceof BrowserGatewayError && owner.error.code === "owner_unavailable") return UNAVAILABLE;
  return undefined;
}

export function BrowserRunScreen({ taskId, runId }: { taskId: string; runId: string }) {
  const owner = useOwner();
  const isOwner = owner?.isOwner === true;
  const runs = useQuery({ ...browserRunsQuery(taskId), enabled: isOwner });
  const waits = useQuery(browserWaitsQuery(taskId));
  const run = runs.data?.items.find((item) => item.run_id === runId);
  const runWaits = (waits.data?.items ?? []).filter((wait) => wait.run_id === runId);
  const authInterval = runWaits.some((wait) => wait.state === "pending" && wait.reason === "waiting_for_auth");
  const ownerReason = owner === undefined ? null : !owner.available ? "本人確認を利用できません。" : null;
  const disabledReason = !isOwner
    ? (ownerReason ?? "本人として登録したセッションでだけ操作できます。")
    : authInterval
      ? "認証を扱っている間は、すべての操作を止めています。"
      : null;
  const live = liveViewState({
    taskId,
    runId,
    run: run ?? { state: "RUNNING", live_path: undefined, live: undefined },
    owner,
    authInterval,
  });

  return (
    <div className="flex flex-col gap-4" data-testid="browser-run-screen">
      <OwnerSessionNotice owner={owner} />
      {isOwner && runs.isError ? (
        <Notice tone="danger" title="実行を取得できません">
          Celeris に接続できるかを確かめ、画面を読み込み直してください。
        </Notice>
      ) : null}
      {isOwner && runs.isSuccess && !run ? (
        <Notice tone="warning" title="この実行は見つかりません">
          task {taskId} に run {runId} の記録がありません。
        </Notice>
      ) : null}
      {run ? (
        <ControlBar
          ids={{ taskId, runId, sessionId: run.session_id }}
          csrf={owner?.csrfToken ?? null}
          canOperate={isOwner && !authInterval}
          disabledReason={disabledReason}
        />
      ) : (
        <section
          aria-label="操作状態"
          className="rounded-lg border-2 border-border bg-surface p-4"
          data-testid="browser-control-bar"
        >
          <p className="text-section font-semibold">操作状態を確認できません</p>
          {disabledReason ? <p className="text-label">{disabledReason}</p> : null}
        </section>
      )}
      {/* 広い幅では Live View と 待ち・イベント を 2 列に分ける。DOM の順（読み上げ・狭幅の順）は変えない。 */}
      <div className="flex flex-col gap-4 lg:grid lg:grid-cols-2 lg:items-start">
        <LiveViewFrame state={live} taskLabel={taskId} />
        <div className="flex flex-col gap-4">
          <div id="browser-waits" data-testid="browser-run-waits">
            {runWaits.length === 0 ? (
              <Section title="待ち">
                <p className="text-label text-muted-foreground">この実行に人の対応待ちはありません。</p>
              </Section>
            ) : (
              <BrowserWaitsList waits={runWaits} owner={owner} />
            )}
          </div>
          <LiveEvents taskId={taskId} runId={runId} />
        </div>
      </div>
    </div>
  );
}
