import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type {
  ClusterConnectStart,
  Clusters,
  ClusterView,
  LocalMachineView,
  LocalSettingsPutBody,
  SettingSource,
} from "../../api/generated/types";
import { clusterKeys } from "../../api/queries/keys";
import {
  type ActionResult,
  ActionResultView,
  type ActionTarget,
  useActionResult,
} from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { DataList } from "../../components/ui/data-list";
import { Section } from "../../components/ui/panel";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { formatAbsolute, formatRelative } from "../../lib/time";
import { nextTab, type TerminalTab, type TerminalTarget, targetLabel } from "./terminal-model";
import { TerminalPanel } from "./terminal-panel";

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

// ---- ローカルマシン（ADR 2026-10-14-local-machine-settings D1〜D5） ----

export const LOCAL_SETTINGS_PATH = "/api/clusters/local/settings";
const MAX_CONCURRENCY_MIN = 1;
const MAX_CONCURRENCY_MAX = 256;

/** 実効値がどこから来たか（応答の `source`）。 */
export function localSourceLabel(source: SettingSource): string {
  switch (source) {
    case "cli":
      return "CLI の --workspace-root";
    case "settings":
      return "画面で保存した値";
    case "config":
      return "設定ファイル";
  }
}

/**
 * 反映の時期（応答の `apply`）。CLI の上書きがある欄は store の値が効かないので、保存しても
 * CLI の上書きを外して daemon を再起動するまで効かない（ADR D3）。
 */
export function localApplyState(setting: { apply: string; source: SettingSource }): { tone: BadgeTone; label: string } {
  if (setting.source === "cli") return { tone: "warning", label: "daemon の再起動が必要" };
  switch (setting.apply) {
    case "new_tasks":
      return { tone: "info", label: "新しい task から反映（既存 task・走行中 run の作業場所は動かさない）" };
    case "next_tick":
      return { tone: "info", label: "次の tick から反映（走行中 run は止めない）" };
    default:
      return { tone: "neutral", label: `反映の時期: ${setting.apply}` };
  }
}

/**
 * 入力から PUT の本文を組む。PUT は全置換なので、空欄は「上書きなし（設定ファイルの値）」= null。
 * 同時実行数は 1〜256 の整数だけを通す（path の検証はサーバの 422 に任せる）。
 */
export function localSettingsBody(
  workspaceRoot: string,
  maxConcurrency: string,
): { body: LocalSettingsPutBody } | { error: string } {
  const root = workspaceRoot.trim();
  const text = maxConcurrency.trim();
  let max: number | null = null;
  if (text !== "") {
    const value = Number(text);
    if (!/^\d+$/.test(text) || value < MAX_CONCURRENCY_MIN || value > MAX_CONCURRENCY_MAX)
      return { error: `同時実行数は ${MAX_CONCURRENCY_MIN}〜${MAX_CONCURRENCY_MAX} の整数で入力してください。` };
    max = value;
  }
  return { body: { workspace_root: root === "" ? null : root, max_concurrency: max } };
}

export function localSettingsTarget(body: LocalSettingsPutBody): ActionTarget {
  return { id: "local:settings", path: LOCAL_SETTINGS_PATH, method: "PUT", body };
}

/** 保存する。入力の誤りは送らずに返し、サーバの拒否（422 など）は文言を返す。成功なら変更後の view を返す。 */
export async function submitLocalSettings(
  run: Sender["run"],
  workspaceRoot: string,
  maxConcurrency: string,
): Promise<{ ok: true; view: LocalMachineView } | { ok: false; error?: string }> {
  const built = localSettingsBody(workspaceRoot, maxConcurrency);
  if ("error" in built) return { ok: false, error: built.error };
  const [result] = await run([localSettingsTarget(built.body)]);
  if (!result) return { ok: false };
  if (result.ok) return { ok: true, view: result.response as LocalMachineView };
  return { ok: false };
}

function localTitle(local: LocalMachineView): string {
  return local.hostname ? `${local.id}（${local.hostname}）` : local.id;
}

const localState: { tone: BadgeTone; label: string } = { tone: "success", label: "このマシン" };

function LocalStatusRow({ local, checkedAt }: { local: LocalMachineView; checkedAt: number }) {
  return (
    <TableRow data-testid={`cluster-row-${local.id}`}>
      <TableCell className="font-medium break-all">{localTitle(local)}</TableCell>
      <TableCell>
        <Badge tone={localState.tone} className="whitespace-nowrap">
          {localState.label}
        </Badge>
      </TableCell>
      <TableCell className="whitespace-nowrap">
        <Time value={new Date(checkedAt).toISOString()} />
      </TableCell>
      <TableCell className="whitespace-nowrap">
        <span className="text-muted-foreground">該当なし</span>
      </TableCell>
      <TableCell className="min-w-48 break-words">
        <span className="text-muted-foreground">なし</span>
      </TableCell>
    </TableRow>
  );
}

function LocalStatusItem({ local, checkedAt }: { local: LocalMachineView; checkedAt: number }) {
  return (
    <li className="min-w-0 space-y-2 rounded-lg border border-border bg-surface p-3">
      <div className="flex flex-wrap items-center gap-2">
        <strong className="min-w-0 break-all">{localTitle(local)}</strong>
        <Badge tone={localState.tone}>{localState.label}</Badge>
      </div>
      <p className="break-words">
        <span className="text-muted-foreground">失敗理由: </span>なし
      </p>
      <p className="text-label text-muted-foreground">
        最終確認: <Time value={new Date(checkedAt).toISOString()} />
      </p>
    </li>
  );
}

function SettingValue({ value, source, config }: { value: string; source: SettingSource; config: string }) {
  return (
    <span className="break-all">
      {`${value}（${localSourceLabel(source)}）`}
      {source !== "config" && (
        <span className="block text-label text-muted-foreground">{`設定ファイルの値: ${config}`}</span>
      )}
    </span>
  );
}

function ApplyBadge({ setting }: { setting: { apply: string; source: SettingSource } }) {
  const state = localApplyState(setting);
  return <Badge tone={state.tone}>{state.label}</Badge>;
}

/** 保存の後に、各欄がいつ効くかを文で出す。 */
function SavedNotice({ view }: { view: LocalMachineView }) {
  return (
    <div role="status" data-testid="local-settings-applied" className="space-y-1 rounded border border-border p-3">
      <p>保存しました。</p>
      <p>
        作業ディレクトリ: <ApplyBadge setting={view.workspace_root} />
      </p>
      <p>
        同時実行数: <ApplyBadge setting={view.max_concurrency} />
      </p>
      {view.workspace_root.source === "cli" && (
        <p>
          CLI の --workspace-root が優先しています。保存した作業ディレクトリは、CLI の上書きを外して daemon
          を再起動した後に効きます。
        </p>
      )}
    </div>
  );
}

export function LocalMachineCard({
  local,
  sender,
  denied,
  saved: savedInitial = null,
  onOpenTerminal,
}: {
  local: LocalMachineView;
  sender: Sender;
  denied: boolean;
  saved?: LocalMachineView | null;
  onOpenTerminal?: () => void;
}) {
  const wr = local.workspace_root;
  const mc = local.max_concurrency;
  const [workspaceRoot, setWorkspaceRoot] = useState(wr.source === "settings" ? wr.value : "");
  const [maxConcurrency, setMaxConcurrency] = useState(mc.source === "settings" ? String(mc.value) : "");
  const [inputError, setInputError] = useState<string | null>(null);
  const [saved, setSaved] = useState<LocalMachineView | null>(savedInitial);
  const locked = sender.pending || denied;
  const result = sender.results["local:settings"];
  const save = (root: string, max: string) => {
    setInputError(null);
    setSaved(null);
    void submitLocalSettings(sender.run, root, max).then((out) => {
      if (out.ok) {
        setSaved(out.view);
        setWorkspaceRoot(out.view.workspace_root.source === "settings" ? out.view.workspace_root.value : "");
        setMaxConcurrency(out.view.max_concurrency.source === "settings" ? String(out.view.max_concurrency.value) : "");
      } else if (out.error) setInputError(out.error);
    });
  };
  return (
    <li
      className="min-w-0 space-y-3 rounded-lg border border-border bg-surface p-4"
      aria-label={`ローカルマシン ${local.id}`}
    >
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="min-w-0 break-all text-body font-semibold text-foreground">{localTitle(local)}</h3>
        <Badge tone={localState.tone}>{localState.label}</Badge>
        {onOpenTerminal && (
          <Button onClick={onOpenTerminal} aria-label="このマシンの端末を開く">
            端末を開く
          </Button>
        )}
      </div>
      <DataList
        items={[
          { label: "使用中", value: `${local.in_use ?? 0} / ${mc.value}` },
          {
            label: "作業ディレクトリの値",
            value: <SettingValue value={wr.value} source={wr.source} config={wr.config_value} />,
          },
          { label: "作業ディレクトリの反映", value: <ApplyBadge setting={wr} /> },
          {
            label: "同時実行数の値",
            value: <SettingValue value={String(mc.value)} source={mc.source} config={String(mc.config_value)} />,
          },
          { label: "同時実行数の反映", value: <ApplyBadge setting={mc} /> },
          {
            label: "scratch の root",
            value: local.scratch_root ? (
              <span className="break-all">{`${local.scratch_root}（設定ファイル。画面からは変えない）`}</span>
            ) : (
              <span className="text-muted-foreground">設定なし</span>
            ),
          },
          {
            label: "最終更新",
            value: <Time value={local.updated_at} />,
          },
        ]}
      />
      {wr.source === "cli" && (
        <p role="note" className="rounded border border-border bg-warning p-3 text-warning-foreground">
          CLI の --workspace-root が優先しています。保存した作業ディレクトリは、CLI の上書きを外して daemon
          を再起動した後に効きます。
        </p>
      )}
      <form
        noValidate
        className="space-y-2"
        aria-label="ローカルの設定"
        onSubmit={(event) => {
          event.preventDefault();
          save(workspaceRoot, maxConcurrency);
        }}
      >
        <label className="block">
          作業ディレクトリ（workspace_root）
          <input
            className={inputClass}
            value={workspaceRoot}
            placeholder={wr.config_value}
            disabled={denied}
            onChange={(e) => setWorkspaceRoot(e.target.value)}
          />
        </label>
        <label className="block">
          同時実行数（max_concurrency）
          <input
            className={inputClass}
            inputMode="numeric"
            value={maxConcurrency}
            placeholder={String(mc.config_value)}
            disabled={denied}
            aria-invalid={inputError ? true : undefined}
            onChange={(e) => setMaxConcurrency(e.target.value)}
          />
        </label>
        <p className="text-label text-muted-foreground">
          空欄は設定ファイルの値に戻します。作業ディレクトリは新しい task から使い、既存 task と走行中 run
          の作業場所は動かしません。
        </p>
        <div className="flex flex-wrap gap-2">
          <Button type="submit" variant="primary" disabled={locked}>
            ローカルの設定を保存
          </Button>
          <Button type="button" disabled={locked} onClick={() => save("", "")}>
            上書きを消す
          </Button>
        </div>
        {inputError && (
          <p role="alert" className="text-danger-foreground">
            {inputError}
          </p>
        )}
        {result && !result.ok && <ActionResultView result={result} />}
        {saved && <SavedNotice view={saved} />}
      </form>
    </li>
  );
}

function StatusTable({
  local,
  items,
  awaiting,
  checkedAt,
}: {
  local: LocalMachineView;
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
        <LocalStatusRow local={local} checkedAt={checkedAt} />
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
  local,
  items,
  awaiting,
  checkedAt,
}: {
  local: LocalMachineView;
  items: ClusterView[];
  awaiting: Set<string>;
  checkedAt: number;
}) {
  return (
    <ul className="space-y-3 sm:hidden" aria-label="クラスタの状態">
      <LocalStatusItem local={local} checkedAt={checkedAt} />
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
  onOpenTerminal,
}: {
  item: ClusterView;
  sender: Sender;
  start: ClusterConnectStart | undefined;
  setStart: (id: string, value: ClusterConnectStart | null) => void;
  denied: boolean;
  onOpenTerminal?: () => void;
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
        {onOpenTerminal && (
          <Button
            disabled={item.connected !== true || !item.work_dir}
            onClick={onOpenTerminal}
            aria-label={`${item.id} の端末を開く`}
          >
            端末を開く
          </Button>
        )}
      </div>
      {onOpenTerminal && item.connected !== true && (
        <p className="text-label text-muted-foreground">
          端末は ssh 接続（ControlMaster）があるときだけ開けます。先に「接続」を押してください。
        </p>
      )}
      {onOpenTerminal && item.connected === true && !item.work_dir && (
        <p className="text-label text-muted-foreground">端末を開くには作業ディレクトリを設定してください。</p>
      )}
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
  const [tabs, setTabs] = useState<TerminalTab[]>([]);
  const [tabSeq, setTabSeq] = useState(0);
  const openTerminal = (target: TerminalTarget, label: string) => {
    setTabs((prev) => nextTab(prev, target, label, tabSeq + 1));
    setTabSeq((n) => n + 1);
  };
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
              <h2 className="text-section font-semibold text-foreground">クラスタ一覧（{items.length + 1}）</h2>
              <p className="text-label text-muted-foreground">
                先頭の行は celeris
                が動いているこのマシンです。最終確認はこの画面が状態を取得した時刻です。失敗理由は最後の切断の原因と転送のエラーです。
              </p>
              <MobileStatusList
                local={query.data.local}
                items={items}
                awaiting={awaiting}
                checkedAt={query.dataUpdatedAt}
              />
              <div className="hidden sm:block">
                <StatusTable
                  local={query.data.local}
                  items={items}
                  awaiting={awaiting}
                  checkedAt={query.dataUpdatedAt}
                />
              </div>
            </section>
            <Section
              title="操作"
              level={2}
              description="このマシンの作業ディレクトリと同時実行数、各クラスタの接続と作業ディレクトリの上書きを行います。"
            >
              <ul className="space-y-3">
                <LocalMachineCard
                  local={query.data.local}
                  sender={sender}
                  denied={denied !== null}
                  onOpenTerminal={() => openTerminal("local", targetLabel("local", query.data?.local.hostname))}
                />
                {items.map((item) => (
                  <ClusterCard
                    key={item.id}
                    item={item}
                    sender={sender}
                    start={starts[item.id]}
                    setStart={setStart}
                    denied={denied !== null}
                    onOpenTerminal={() => openTerminal(`cluster:${item.id}`, targetLabel(`cluster:${item.id}`))}
                  />
                ))}
              </ul>
            </Section>
            <TerminalPanel tabs={tabs} onCloseTab={(key) => setTabs((prev) => prev.filter((t) => t.key !== key))} />
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
