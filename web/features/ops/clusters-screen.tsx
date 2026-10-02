import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { ClusterConnectStart, Clusters, ClusterView } from "../../api/generated/types";
import { clusterKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";

type Sender = ReturnType<typeof useActionResult>;
const inputClass = "block w-full min-h-11 rounded border p-2";
type Starts = Record<string, ClusterConnectStart>;

function ClusterCard({
  item,
  sender,
  start,
  setStart,
}: {
  item: ClusterView;
  sender: Sender;
  start: ClusterConnectStart | undefined;
  setStart: (id: string, value: ClusterConnectStart | null) => void;
}) {
  const [code, setCode] = useState("");
  const [workDir, setWorkDir] = useState(item.work_dir ?? "");
  const base = `/api/clusters/${encodeURIComponent(item.id)}`;
  // サーバの connect_pending か、手元の needs_code があれば、取り直しを跨いでコード入力欄を残す。
  const awaiting = Boolean(item.connect_pending) || start?.kind === "needs_code";
  return (
    <li className="min-w-0 rounded border p-3 space-y-2" aria-label={`クラスタ ${item.id}`}>
      <h3 className="font-semibold break-words">{item.id}</h3>
      <p className="text-sm break-words">
        {item.host} / auth {item.auth ?? "manual"} / {item.connected ? "接続中" : "未接続"}
        {item.work_dir ? ` / 作業ディレクトリ ${item.work_dir}（${item.work_dir_source ?? "config"}）` : ""}
      </p>
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={sender.pending}
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
        <section className="rounded border p-2 space-y-2" aria-label={`接続コード ${item.id}`}>
          <p role="status">コード待ち{start?.prompt ? `: ${start.prompt}` : ""}</p>
          <label className="block">
            接続コード
            <input
              className={inputClass}
              type="password"
              autoComplete="off"
              value={code}
              onChange={(e) => setCode(e.target.value)}
            />
          </label>
          <div className="flex flex-wrap gap-2">
            <Button
              disabled={sender.pending || code.trim() === ""}
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
              disabled={sender.pending}
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
          <input className={inputClass} value={workDir} onChange={(e) => setWorkDir(e.target.value)} />
        </label>
        <div className="flex flex-wrap gap-2">
          <Button
            disabled={sender.pending || workDir.trim() === ""}
            onClick={() =>
              void sender.run([
                { id: `wd:${item.id}`, path: `${base}/settings`, method: "PUT", body: { work_dir: workDir.trim() } },
              ])
            }
          >
            作業ディレクトリを保存
          </Button>
          <Button
            disabled={sender.pending}
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
  return (
    <ScreenFrame title="クラスタ" route="/clusters">
      <FetchFrame query={query}>
        {query.data && (
          <section className="space-y-2 min-w-0" aria-label="クラスタ一覧">
            <h2 className="text-lg font-semibold">クラスタ一覧（{query.data.items.length}）</h2>
            {query.data.items.length === 0 ? (
              <p>クラスタがありません。</p>
            ) : (
              <ul className="space-y-3">
                {query.data.items.map((item) => (
                  <ClusterCard key={item.id} item={item} sender={sender} start={starts[item.id]} setStart={setStart} />
                ))}
              </ul>
            )}
          </section>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
