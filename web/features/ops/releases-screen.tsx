import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { ApiError, apiGet, apiMutate } from "../../api/client";
import type { ReleaseItem, ReleasePromoteAccepted, Releases } from "../../api/generated/types";
import { releaseKeys } from "../../api/queries/keys";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { DataList } from "../../components/ui/data-list";
import { Section } from "../../components/ui/panel";
import { StatusBadge } from "../../components/ui/status-badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { formatAbsolute, formatRelative } from "../../lib/time";
import {
  canPromote,
  judgePromotion,
  type PromotionOutcome,
  type PromotionTrack,
  pollDelayMs,
  promotionActionLabel,
  promotionConfirmCopy,
  promotionStatus,
} from "./releases-promotion";

const releasesQuery = {
  queryKey: releaseKeys.list(),
  queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<Releases>("/api/releases", signal),
} as const;

type Tracked = { track: PromotionTrack; outcome: PromotionOutcome; reconnecting: boolean };

/** 昇格の追跡。結果が確定するまで GET /releases を回し、接続が切れても backoff で続ける。 */
function usePromotionTracker() {
  const queryClient = useQueryClient();
  const [tracked, setTracked] = useState<Tracked | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
  // 403 を受けたら、この画面の昇格・巻き戻しを全て無効にし、理由を出す。
  const [denied, setDenied] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const generation = useRef(0);
  const track = tracked?.track;
  const settled = tracked !== null && tracked.outcome.state !== "pending";

  useEffect(() => {
    if (!track || settled) return;
    const mine = generation.current;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let failures = 0;
    const tick = async () => {
      let delay: number;
      try {
        const data = await apiGet<Releases>("/api/releases", controller.signal);
        if (controller.signal.aborted || mine !== generation.current) return;
        failures = 0;
        queryClient.setQueryData(releaseKeys.list(), data);
        const outcome = judgePromotion(
          data.items.find((item) => item.sha12 === track.sha12),
          track,
        );
        setTracked({ track, outcome, reconnecting: false });
        if (outcome.state !== "pending") return;
        delay = pollDelayMs(0);
      } catch {
        if (controller.signal.aborted || mine !== generation.current) return;
        failures += 1;
        setTracked((previous) => (previous ? { ...previous, reconnecting: true } : previous));
        delay = pollDelayMs(failures);
      }
      timer = setTimeout(() => void tick(), delay);
    };
    timer = setTimeout(() => void tick(), pollDelayMs(0));
    return () => {
      controller.abort();
      if (timer) clearTimeout(timer);
    };
  }, [track, settled, queryClient]);

  async function start(item: ReleaseItem) {
    if (starting || (tracked && tracked.outcome.state === "pending")) return;
    generation.current += 1;
    setStarting(true);
    setStartError(null);
    const base = { sha12: item.sha12, baselinePromotedAt: item.promoted_at ?? null };
    const pressedAt = new Date().toISOString();
    try {
      const accepted = await apiMutate<ReleasePromoteAccepted>(
        "POST",
        `/api/releases/${encodeURIComponent(item.sha12)}/promote`,
        {},
      );
      const next: PromotionTrack = { ...base, startedAt: accepted?.started_at ?? pressedAt };
      setTracked({ track: next, outcome: { state: "pending", detail: "昇格を起動しました" }, reconnecting: false });
    } catch (error) {
      if (error instanceof ApiError && (error.kind === "network" || error.kind === "timeout")) {
        // 応答を受けられなかった。起動したかは分からないので、状態を見て確かめる。
        setTracked({
          track: { ...base, startedAt: pressedAt },
          outcome: { state: "pending", detail: "起動したか確認しています" },
          reconnecting: true,
        });
      } else {
        const body = error instanceof ApiError ? error.body : undefined;
        const detail =
          body && typeof body === "object"
            ? ((body as Record<string, unknown>).error ?? (body as Record<string, unknown>).message)
            : body;
        const reason = typeof detail === "string" ? detail : "昇格を起動できませんでした";
        if (error instanceof ApiError && error.status === 403) setDenied(reason);
        else setStartError(reason);
        setTracked(null);
      }
    } finally {
      setStarting(false);
    }
  }
  return { tracked, startError, denied, starting, start };
}

function Time({ value }: { value: string | null | undefined }) {
  if (!value) return <span className="text-muted-foreground">記録なし</span>;
  return (
    <time dateTime={value} title={formatAbsolute(value)}>
      {formatRelative(value)}
    </time>
  );
}

/** 昇格の結果。色だけに頼らず、StatusBadge の文字（実行中・完了・失敗）と文で示す。 */
function PromotionStatus({ tracked, startError }: { tracked: Tracked | null; startError: string | null }) {
  if (startError)
    return (
      <p role="alert" className="rounded border border-border bg-danger p-3 text-danger-foreground">
        昇格を起動できませんでした: {startError}
      </p>
    );
  if (!tracked) return null;
  const { outcome, track, reconnecting } = tracked;
  const badge = <StatusBadge status={promotionStatus(outcome)} className="shrink-0" />;
  if (outcome.state === "pending")
    return (
      <p role="status" data-testid="promote-pending" className="flex flex-wrap items-center gap-2 break-words">
        {badge}
        <span className="min-w-0">
          {track.sha12} を昇格中です（結果を確認するまで完了ではありません）。{outcome.detail}
          {reconnecting && " 接続を待っています。再接続を試みています。"}
        </span>
      </p>
    );
  if (outcome.state === "failed")
    return (
      <p
        role="alert"
        data-testid="promote-failed"
        className="flex flex-wrap items-center gap-2 break-words rounded border border-border p-3"
      >
        {badge}
        <span className="min-w-0">
          {track.sha12} の昇格に失敗しました: {outcome.error}
        </span>
      </p>
    );
  return (
    <p role="status" data-testid="promote-succeeded" className="flex flex-wrap items-center gap-2 break-words">
      {badge}
      <span className="min-w-0">{track.sha12} の昇格が完了しました。</span>
    </p>
  );
}

function ReleaseState({ item }: { item: ReleaseItem }) {
  return (
    <div className="mt-1 flex flex-wrap gap-1">
      {item.is_current && <Badge tone="success">current</Badge>}
      {item.is_previous && <Badge tone="neutral">previous</Badge>}
      {item.promoting && <StatusBadge status="running" />}
      <Badge tone={item.gate_ok ? "success" : "warning"}>{item.gate_ok ? "gate 通過" : "gate 未通過"}</Badge>
    </div>
  );
}

/** 昇格できない理由。ボタンを無効にするときは必ず文字で添える。 */
function blockedReason(item: ReleaseItem, denied: boolean, busy: boolean): string | null {
  if (denied) return "権限がありません";
  if (item.is_current) return "稼働中の版です";
  if (item.promoting || busy) return "昇格中は操作できません";
  if (!item.gate_ok) return "gate を通っていません";
  if (item.problem) return "問題があります";
  return null;
}

function PromoteAction({
  item,
  current,
  busy,
  denied,
  onPromote,
}: {
  item: ReleaseItem;
  current: string | null | undefined;
  busy: boolean;
  denied: boolean;
  onPromote: (item: ReleaseItem) => Promise<void>;
}) {
  const label = promotionActionLabel(item);
  const reason = blockedReason(item, denied, busy) ?? (canPromote(item) ? null : "昇格できない状態です");
  const copy = promotionConfirmCopy(item, current);
  // 昇格が始まると reason が付くが、確認表示は閉じるまで同じ要素のまま置いておく（focus を失わないため）。
  return (
    <div className="space-y-1">
      <ConfirmDialog
        trigger={
          <Button size="sm" disabled={reason !== null}>
            {label}
          </Button>
        }
        title={copy.title}
        target={copy.target}
        consequence={copy.consequence}
        reversibility={copy.reversibility}
        followUp="この画面の「昇格の結果」に進行中・完了・失敗が出ます。"
        confirmLabel={label}
        onConfirm={() => onPromote(item)}
      />
      {reason && <p className="text-label text-muted-foreground">{reason}</p>}
    </div>
  );
}

function ReleaseTable({
  items,
  current,
  busy,
  denied,
  onPromote,
}: {
  items: ReleaseItem[];
  current: string | null | undefined;
  busy: boolean;
  denied: boolean;
  onPromote: (item: ReleaseItem) => Promise<void>;
}) {
  return (
    <Table aria-label="リリースの一覧">
      <TableHeader>
        <TableRow>
          <TableHead>版・状態</TableHead>
          <TableHead>操作</TableHead>
          <TableHead>ビルド</TableHead>
          <TableHead>昇格</TableHead>
          <TableHead>変更</TableHead>
          <TableHead>問題・直近の失敗</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {items.map((item) => {
          const failed = item.promote_failed && !item.promoting ? item.promote_failed : null;
          return (
            <TableRow key={item.sha12} data-testid={`release-${item.sha12}`}>
              <TableCell>
                <code className="font-mono whitespace-nowrap">{item.sha12}</code>
                {item.ref && <p className="min-w-32 text-label text-muted-foreground break-all">ref {item.ref}</p>}
                <ReleaseState item={item} />
              </TableCell>
              <TableCell className="min-w-36">
                <PromoteAction item={item} current={current} busy={busy} denied={denied} onPromote={onPromote} />
              </TableCell>
              <TableCell className="whitespace-nowrap">
                <Time value={item.built_at} />
              </TableCell>
              <TableCell className="whitespace-nowrap">
                <Time value={item.promoted_at} />
              </TableCell>
              <TableCell className="whitespace-nowrap">
                {item.changes ? (
                  `${item.changes.commit_count} commit / ${item.changes.file_count} file`
                ) : (
                  <span className="text-muted-foreground">記録なし</span>
                )}
              </TableCell>
              <TableCell className="min-w-48 break-words">
                {item.problem && <p>問題: {item.problem}</p>}
                {failed && (
                  <p>
                    直近の昇格の失敗（{formatAbsolute(failed.failed_at)}）: {failed.error}
                  </p>
                )}
                {!item.problem && !failed && <span className="text-muted-foreground">なし</span>}
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

function MobileReleaseList({
  items,
  current,
  busy,
  denied,
  onPromote,
}: {
  items: ReleaseItem[];
  current: string | null | undefined;
  busy: boolean;
  denied: boolean;
  onPromote: (item: ReleaseItem) => Promise<void>;
}) {
  return (
    <ul className="space-y-3 sm:hidden" aria-label="リリースの一覧">
      {items.map((item) => {
        const failed = item.promote_failed && !item.promoting ? item.promote_failed : null;
        return (
          <li
            key={item.sha12}
            data-testid={`mobile-release-${item.sha12}`}
            className="min-w-0 space-y-3 rounded-lg border border-border bg-surface p-3"
          >
            <div>
              <code className="font-mono break-all">{item.sha12}</code>
              {item.ref && <p className="text-label text-muted-foreground break-all">ref {item.ref}</p>}
              <ReleaseState item={item} />
            </div>
            <div className="break-words">
              <p className="font-medium">問題・直近の失敗</p>
              {item.problem && <p>問題: {item.problem}</p>}
              {failed && (
                <p>
                  直近の昇格の失敗（{formatAbsolute(failed.failed_at)}）: {failed.error}
                </p>
              )}
              {!item.problem && !failed && <p className="text-muted-foreground">なし</p>}
            </div>
            <PromoteAction item={item} current={current} busy={busy} denied={denied} onPromote={onPromote} />
            <p className="text-label text-muted-foreground">
              ビルド: <Time value={item.built_at} /> ／ 昇格: <Time value={item.promoted_at} />
            </p>
            <p className="text-label text-muted-foreground">
              変更:{" "}
              {item.changes ? `${item.changes.commit_count} commit / ${item.changes.file_count} file` : "記録なし"}
            </p>
          </li>
        );
      })}
    </ul>
  );
}

export function ReleasesScreen() {
  const query = useQuery(releasesQuery);
  const promotion = usePromotionTracker();
  const busy = promotion.starting || promotion.tracked?.outcome.state === "pending";
  const data = query.data;
  return (
    <ScreenFrame title="リリース" route="/releases">
      <FetchFrame query={query}>
        {data && (
          <div className="min-w-0 space-y-6">
            {promotion.denied && (
              <p
                role="alert"
                data-testid="releases-denied"
                className="rounded border border-border bg-danger p-3 text-danger-foreground"
              >
                権限がありません（403）。この画面の昇格・巻き戻しは無効にしました。理由: {promotion.denied}
              </p>
            )}
            <Section
              title="稼働中の版"
              description="本番の daemon が今動かしている版と、昇格・巻き戻しの基準になる版です。"
            >
              <DataList
                className="mt-2"
                items={[
                  { label: "稼働中", value: `${data.running.release}（${data.running.role}）` },
                  { label: "current", value: data.current ?? "なし" },
                  { label: "previous", value: data.previous ?? "なし" },
                ]}
              />
            </Section>
            <Section title="昇格の結果" aria-live="polite">
              <div className="mt-2">
                {promotion.tracked || promotion.startError ? (
                  <PromotionStatus tracked={promotion.tracked} startError={promotion.startError} />
                ) : (
                  <p className="text-label text-muted-foreground">この画面からの昇格はまだありません。</p>
                )}
              </div>
            </Section>
            <Section
              title={`リリースの一覧（${data.items.length}）`}
              description="昇格・巻き戻しは確認を挟みます。前の版（previous）を昇格すると巻き戻しになります。"
            >
              <div className="mt-2">
                {data.items.length === 0 ? (
                  <p>リリースはありません。</p>
                ) : (
                  <>
                    <MobileReleaseList
                      items={data.items}
                      current={data.current}
                      busy={busy}
                      denied={promotion.denied !== null}
                      onPromote={(item) => promotion.start(item)}
                    />
                    <div className="hidden sm:block">
                      <ReleaseTable
                        items={data.items}
                        current={data.current}
                        busy={busy}
                        denied={promotion.denied !== null}
                        onPromote={(item) => promotion.start(item)}
                      />
                    </div>
                  </>
                )}
              </div>
            </Section>
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
