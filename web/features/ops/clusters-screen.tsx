import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { ClusterConnectStart, Clusters, ClusterView } from "../../api/generated/types";
import { clusterKeys } from "../../api/queries/keys";
import { type ActionResult, ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { DataList } from "../../components/ui/data-list";
import { Section } from "../../components/ui/panel";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { formatAbsolute, formatRelative } from "../../lib/time";

type Sender = ReturnType<typeof useActionResult>;
// 入力欄の枠は --color-input（styles.css の @layer base）に任せ、ここでは色を指定しない。
const inputClass = "mt-1 block w-full min-h-11 rounded border p-2";
type Starts = Record<string, ClusterConnectStart>;

/** 接続の状態。色だけに頼らず、必ず文字のラベルを出す。 */
export function clusterConnection(item: ClusterView, awaiting: boolean): { tone: BadgeTone; label: string } {
  if (awaiting) return { tone: "warning", label: "コード入力待ち" };
  if (item.tunnel_login_needed) return { tone: "warning", label: "ログインが必要" };
  if (item.connected === true) return { tone: "success", label: "接続中" };
  if (item.connected === false) return { tone: "neutral", label: "未接続" };
  return { tone: "neutral", label: "不明" };
}

/** 失敗理由: 最後の切断の原因と、転送の最後のエラーを文字で並べる。無ければ null。 */
export function clusterFailure(item: ClusterView): string | null {
  const reasons: string[] = [];
  const cause = item.stats?.last_24h.last_lost_cause ?? item.stats?.since_start?.last_lost_cause;
  if (cause) reasons.push(`切断: ${cause}`);
  for (const f of item.tunnel_forwards ?? []) {
    if (f.last_error) reasons.push(`転送 ${f.listen}: ${f.last_error}`);
  }
  if (item.cooldown_until) reasons.push(`待機中（${formatAbsolute(item.cooldown_until)} まで）`);
  return reasons.length ? reasons.join(" / ") : null;
}

/** 403 の結果があれば、その理由の文。操作の無効化に使う。 */
export function deniedReason(results: Record<string, ActionResult>): string | null {
  const denied = Object.values(results).find((r) => r.status === 403);
  if (!denied) return null;
  return `権限がありません（403）。この画面の操作は無効にしました。理由: ${denied.message}`;
}

function Time({ value }: { value: string | null | undefined }) {
  if (!value) return <span className="text-muted-foreground">記録なし</span>;
  return (
    <time dateTime={value} title={formatAbsolute(value)}>
      {formatRelative(value)}
    </time>
  );
}

function StatusTable({
  items,
  awaiting,
  checkedAt,
}: {
  items: ClusterView[];
  awaiting: Set<string>;
  checkedAt: number;
}) {
  return (
    <Table aria-label="クラスタの状態">
      <TableHeader>
        <TableRow>
          <TableHead>クラスタ</TableHead>
          <TableHead>接続</TableHead>
          <TableHead>最終確認</TableHead>
          <TableHead>最後の切断</TableHead>
          <TableHead>失敗理由</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {items.map((item) => {
          const state = clusterConnection(item, awaiting.has(item.id));
          const failure = clusterFailure(item);
          const lost = item.stats?.last_24h.last_lost_at ?? item.stats?.since_start?.last_lost_at;
          return (
            <TableRow key={item.id} data-testid={`cluster-row-${item.id}`}>
              <TableCell className="font-medium break-all">{item.id}</TableCell>
              <TableCell>
                <Badge tone={state.tone} className="whitespace-nowrap">
                  {state.label}
                </Badge>
              </TableCell>
              <TableCell className="whitespace-nowrap">
                <Time value={new Date(checkedAt).toISOString()} />
              </TableCell>
              <TableCell className="whitespace-nowrap">
                <Time value={lost} />
              </TableCell>
              <TableCell className="min-w-48 break-words">
                {failure ?? <span className="text-muted-foreground">なし</span>}
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

function MobileStatusList({
  items,
  awaiting,
  checkedAt,
}: {
  items: ClusterView[];
  awaiting: Set<string>;
  checkedAt: number;
}) {
  return (
    <ul className="space-y-3 sm:hidden" aria-label="クラスタの状態">
      {items.map((item) => {
        const state = clusterConnection(item, awaiting.has(item.id));
        const failure = clusterFailure(item);
        const lost = item.stats?.last_24h.last_lost_at ?? item.stats?.since_start?.last_lost_at;
        return (
          <li key={item.id} className="min-w-0 space-y-2 rounded-lg border border-border bg-surface p-3">
            <div className="flex flex-wrap items-center gap-2">
              <strong className="min-w-0 break-all">{item.id}</strong>
              <Badge tone={state.tone}>{state.label}</Badge>
            </div>
            <p className="break-words">
              <span className="text-muted-foreground">失敗理由: </span>
              {failure ?? "なし"}
            </p>
            <p className="text-label text-muted-foreground">
              最終確認: <Time value={new Date(checkedAt).toISOString()} /> ／ 最後の切断: <Time value={lost} />
            </p>
          </li>
        );
      })}
    </ul>
  );
}

function ClusterCard({
  item,
  sender,
  start,
  setStart,
  denied,
}: {
  item: ClusterView;
  sender: Sender;
  start: ClusterConnectStart | undefined;
  setStart: (id: string, value: ClusterConnectStart | null) => void;
  denied: boolean;
}) {
  const [code, setCode] = useState("");
  const [workDir, setWorkDir] = useState(item.work_dir ?? "");
  const base = `/api/clusters/${encodeURIComponent(item.id)}`;
  // サーバの connect_pending か、手元の needs_code があれば、取り直しを跨いでコード入力欄を残す。
  const awaiting = Boolean(item.connect_pending) || start?.kind === "needs_code";
  const state = clusterConnection(item, awaiting);
  const locked = sender.pending || denied;
  return (
    <li className="min-w-0 space-y-3 rounded-lg border border-border bg-surface p-4" aria-label={`クラスタ ${item.id}`}>
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="min-w-0 break-all text-body font-semibold text-foreground">{item.id}</h3>
        <Badge tone={state.tone}>{state.label}</Badge>
      </div>
      <DataList
        items={[
          { label: "接続先", value: <span className="break-all">{item.host}</span> },
          { label: "認証", value: item.auth ?? "manual" },
          {
            label: "使用中",
            value: `${item.in_use ?? 0} / ${item.concurrency}`,
          },
          {
            label: "作業ディレクトリの値",
            value: item.work_dir ? (
              <span className="break-all">{`${item.work_dir}（${item.work_dir_source ?? "config"}）`}</span>
            ) : (
              <span className="text-muted-foreground">設定なし</span>
            ),
          },
        ]}
      />
      <div className="flex flex-wrap gap-2">
        <Button
          variant="primary"
          disabled={locked}
          onClick={() =>
            void sender.run([{ id: `connect:${item.id}`, path: `${base}/connect`, body: {} }]).then((out) => {
              const res = out[0]?.response as ClusterConnectStart | undefined;
              if (out[0]?.ok && res) setStart(item.id, res.kind === "needs_code" ? res : null);
            })
          }
        >
          接続
        </Button>
      </div>
      <ActionResultView result={sender.results[`connect:${item.id}`]} />
      {awaiting && (
        <section className="space-y-2 rounded border border-border p-3" aria-label={`接続コード ${item.id}`}>
          <p role="status">コード待ち{start?.prompt ? `: ${start.prompt}` : ""}</p>
          <label className="block">
            接続コード
            <input
              className={inputClass}
              type="password"
              autoComplete="off"
              value={code}
              disabled={denied}
              onChange={(e) => setCode(e.target.value)}
            />
          </label>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="primary"
              disabled={locked || code.trim() === ""}
              onClick={() => {
                const sent = code.trim();
                setCode("");
                void sender
                  .run([{ id: `code:${item.id}`, path: `${base}/connect/code`, body: { code: sent } }])
                  .then((out) => {
                    if (out[0]?.ok) setStart(item.id, null);
                  });
              }}
            >
              コードを送る
            </Button>
            <Button
              disabled={locked}
              onClick={() =>
                void sender
                  .run([{ id: `cancel:${item.id}`, path: `${base}/connect`, method: "DELETE" }])
                  .then((out) => {
                    if (out[0]?.ok) setStart(item.id, null);
                  })
              }
            >
              接続を取り消す
            </Button>
          </div>
          <ActionResultView result={sender.results[`code:${item.id}`]} />
          <ActionResultView result={sender.results[`cancel:${item.id}`]} />
        </section>
      )}
      <div className="space-y-2">
        <label className="block">
          作業ディレクトリ
          <input
            className={inputClass}
            value={workDir}
            disabled={denied}
            onChange={(e) => setWorkDir(e.target.value)}
          />
        </label>
        <div className="flex flex-wrap gap-2">
          <Button
            disabled={locked || workDir.trim() === ""}
            onClick={() =>
              void sender.run([
                { id: `wd:${item.id}`, path: `${base}/settings`, method: "PUT", body: { work_dir: workDir.trim() } },
              ])
            }
          >
            作業ディレクトリを保存
          </Button>
          <Button
            disabled={locked}
            onClick={() =>
              void sender
                .run([{ id: `wd:${item.id}`, path: `${base}/settings`, method: "PUT", body: { work_dir: null } }])
                .then((out) => {
                  if (out[0]?.ok) setWorkDir("");
                })
            }
          >
            上書きを消す
          </Button>
        </div>
        <ActionResultView result={sender.results[`wd:${item.id}`]} />
      </div>
    </li>
  );
}

export function ClustersScreen() {
  const query = useQuery({
    queryKey: clusterKeys.list(),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<Clusters>("/api/clusters", signal),
  });
  const sender = useActionResult(clusterKeys.all);
  const [starts, setStarts] = useState<Starts>({});
  const setStart = (id: string, value: ClusterConnectStart | null) =>
    setStarts((prev) => {
      const next = { ...prev };
      if (value) next[id] = value;
      else delete next[id];
      return next;
    });
  const denied = deniedReason(sender.results);
  const items = query.data?.items ?? [];
  const awaiting = new Set(
    items.filter((c) => c.connect_pending || starts[c.id]?.kind === "needs_code").map((c) => c.id),
  );
  return (
    <ScreenFrame title="クラスタ" route="/clusters">
      <FetchFrame query={query}>
        {query.data && (
          <div className="min-w-0 space-y-6">
            {denied && (
              <p
                role="alert"
                data-testid="clusters-denied"
                className="rounded border border-border bg-danger p-3 text-danger-foreground"
              >
                {denied}
              </p>
            )}
            <section className="min-w-0 space-y-3" aria-label="クラスタ一覧">
              <h2 className="text-section font-semibold text-foreground">クラスタ一覧（{items.length}）</h2>
              {items.length === 0 ? (
                <p>クラスタがありません。</p>
              ) : (
                <>
                  <p className="text-label text-muted-foreground">
                    最終確認はこの画面が状態を取得した時刻です。失敗理由は最後の切断の原因と転送のエラーです。
                  </p>
                  <MobileStatusList items={items} awaiting={awaiting} checkedAt={query.dataUpdatedAt} />
                  <div className="hidden sm:block">
                    <StatusTable items={items} awaiting={awaiting} checkedAt={query.dataUpdatedAt} />
                  </div>
                </>
              )}
            </section>
            {items.length > 0 && (
              <Section title="操作" level={2} description="接続と作業ディレクトリの上書きをクラスタごとに行います。">
                <ul className="space-y-3">
                  {items.map((item) => (
                    <ClusterCard
                      key={item.id}
                      item={item}
                      sender={sender}
                      start={starts[item.id]}
                      setStart={setStart}
                      denied={denied !== null}
                    />
                  ))}
                </ul>
              </Section>
            )}
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
