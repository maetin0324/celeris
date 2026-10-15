import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import type { SecretView } from "../../api/generated/types";
import { inboxItemsQuery } from "../../api/queries/inbox-notifications";
import { Badge } from "../../components/ui/badge";
import { serverNowMs } from "../../lib/time";
import { type CredentialPhase, credentialWaitRows, PHASE_LABEL, VERIFY_GRACE_MS } from "./credential-wait-model";

const TONE: Record<CredentialPhase, "warning" | "info" | "danger"> = {
  awaiting_key: "warning",
  verifying: "info",
  reregister: "danger",
};

/**
 * 登録を待つ work unit の一覧（secrets section の先頭）。受信箱の credential 待ち item から導く。
 * 登録後は「検証中」、成功して item が消えたら「続行しました」、誤キーなら「再登録待ち」。
 * 登録は連携の有効化だけで、測定の予算の承認ではない。
 */
export function CredentialWaitsPanel({
  secrets,
  saved,
  focusSecret,
}: {
  secrets: readonly SecretView[];
  /** この画面で保存に成功した secret id と、その結果（結果の同一性で「新しい保存」を見分ける）。 */
  saved: ReadonlyArray<{ id: string; result: object }>;
  focusSecret?: string;
}) {
  const inbox = useQuery({ ...inboxItemsQuery({ kind: "credential_wait" }), refetchInterval: 5000 });
  const [savedAt, setSavedAt] = useState<Record<string, number>>({});
  const [tick, setTick] = useState(0);
  const seenResults = useRef(new Map<string, object>());
  useEffect(() => {
    const fresh: Record<string, number> = {};
    for (const { id, result } of saved) {
      if (seenResults.current.get(id) !== result) {
        seenResults.current.set(id, result);
        fresh[id] = serverNowMs();
      }
    }
    if (Object.keys(fresh).length === 0) return;
    setSavedAt((prev) => ({ ...prev, ...fresh }));
    const timer = setTimeout(() => setTick((n) => n + 1), VERIFY_GRACE_MS + 50);
    return () => clearTimeout(timer);
  }, [saved]);
  void tick;
  const now = serverNowMs();
  const agoMs = Object.fromEntries(Object.entries(savedAt).map(([id, at]) => [id, now - at]));
  const rows = credentialWaitRows(inbox.data?.items ?? [], secrets, agoMs);

  // 見えていた待ちが消えた = 検証に成功して続行した。
  const prevIds = useRef<Set<string>>(new Set());
  const [resumed, setResumed] = useState<string[]>([]);
  useEffect(() => {
    if (!inbox.data) return;
    const current = new Set(rows.map((r) => r.secretId));
    const gone = [...prevIds.current].filter((id) => !current.has(id));
    prevIds.current = current;
    if (gone.length > 0) setResumed((prev) => [...new Set([...prev, ...gone])]);
  }, [inbox.data, rows]);
  const resumedNow = resumed.filter((id) => !rows.some((r) => r.secretId === id));

  if (rows.length === 0 && resumedNow.length === 0) return null;
  return (
    <section className="min-w-0 space-y-2" aria-label="登録待ちの work unit">
      <h3 className="text-body font-semibold text-foreground">登録待ちの work unit（{rows.length}）</h3>
      <ul className="grid min-w-0 gap-2">
        {rows.map((row) => (
          <li
            key={row.item.id}
            data-credential-wait={row.secretId}
            data-phase={row.phase}
            className={`min-w-0 space-y-1 rounded-lg border border-border bg-surface p-3 text-label text-foreground${
              row.secretId === focusSecret ? " outline-2 outline-ring" : ""
            }`}
          >
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <a className="inline-flex min-h-11 items-center break-all underline" href={`#secret-${row.secretId}`}>
                {row.secretId}
              </a>
              <Badge tone={TONE[row.phase]}>{PHASE_LABEL[row.phase]}</Badge>
            </div>
            <p className="break-words">
              待っている task:{" "}
              {row.tasks.length === 0
                ? "なし"
                : row.tasks.map((task, i) => (
                    <span key={task.id}>
                      {i > 0 && "、"}
                      <Link className="underline" to="/tasks/$id" params={{ id: task.id }}>
                        {task.title}
                      </Link>
                    </span>
                  ))}
              （{row.units.length} 件の work unit だけ。他の unit・兄弟は止まりません）
            </p>
            <p className="break-words text-muted-foreground">
              {row.phase === "reregister"
                ? "登録されたキーは使えませんでした。正しいキーで置き換えると、もう一度検証して自動で続行します。"
                : "登録すると検証のあと、待っている work unit が自動で続行されます。登録は連携の有効化だけで、測定の予算の承認ではありません。"}
            </p>
          </li>
        ))}
      </ul>
      {resumedNow.map((id) => (
        <p key={id} role="status" data-credential-resumed={id} className="text-label text-success-foreground">
          {id}: 検証に成功し、待っていた work unit は続行しました。
        </p>
      ))}
    </section>
  );
}
