import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { ApiError, apiGet } from "../../api/client";
import { projectKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { Notice } from "../../components/ui/notice";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { ProjectSection } from "./project-detail-view";

// /projects/:id/docs/maintenance: 監査は指摘の一覧、整理案は操作の表で読ませる。JSON は details の中で直す。
// 承認・適用・採用は ConfirmDialog を通す（FRONTEND_CONTRACT 規則 6）。409 は文書リポジトリなしとして文書画面へ案内する。

type AuditDocument = { path?: string; title?: string; category?: string; findings?: string[] };
type PlanAction = {
  operation?: string;
  path?: string;
  paths?: string[];
  destination?: string;
  alternatives?: string[];
};
type Maintenance = {
  audit?: { revision?: string; documents?: AuditDocument[]; conventions?: string[] } & Record<string, unknown>;
  proposal?: { actions?: PlanAction[]; rationale?: string[] } & Record<string, unknown>;
  policy?: { mode?: string } & Record<string, unknown>;
  saved_report?: unknown;
};

const operationLabels: Record<string, string> = {
  rewrite: "書き直す",
  move: "移動",
  archive: "保管へ移す",
  delete: "削除",
  merge: "統合",
  index: "索引を作る",
  choose_canonical: "正本を選ぶ",
};
const modeLabels: Record<string, string> = { observe: "観察のみ", conservative: "控えめに整理", managed: "管理する" };
const input =
  "box-border min-h-32 w-full max-w-full rounded-md border border-input bg-surface p-2 font-mono text-label";
const navLink = "inline-flex min-h-11 min-w-11 items-center text-primary underline";

function actionTarget(action: PlanAction): string {
  const from = action.paths?.join("、") ?? action.path ?? "—";
  if (action.destination) return `${from} → ${action.destination}`;
  if (action.alternatives?.length) return `${from}（候補: ${action.alternatives.join("、")}）`;
  return from;
}

function JsonDetails({ summary, value }: { summary: string; value: unknown }) {
  return (
    <details>
      <summary className="min-h-11 cursor-pointer py-2 text-label text-muted-foreground">{summary}</summary>
      <pre className="max-w-full overflow-auto whitespace-pre-wrap rounded-md border border-border p-2 text-label">
        {JSON.stringify(value, null, 2)}
      </pre>
    </details>
  );
}

function AuditFindings({ audit }: { audit: Maintenance["audit"] }) {
  const flagged = (audit?.documents ?? []).filter((doc) => (doc.findings ?? []).length > 0);
  if (flagged.length === 0) return <p className="text-muted-foreground">指摘はありません。</p>;
  return (
    <ul className="space-y-2" data-testid="docs-audit-findings">
      {flagged.map((doc) => (
        <li key={doc.path} className="min-w-0 rounded-md border border-border p-2 break-words">
          <p className="font-medium">{doc.title || doc.path}</p>
          {doc.title ? <p className="text-label text-muted-foreground break-all">{doc.path}</p> : null}
          <ul className="list-disc ps-6">
            {(doc.findings ?? []).map((finding) => (
              <li key={finding}>{finding}</li>
            ))}
          </ul>
        </li>
      ))}
    </ul>
  );
}

function PlanTable({ actions }: { actions: PlanAction[] }) {
  if (actions.length === 0) return <p className="text-muted-foreground">整理する文書はありません。</p>;
  return (
    <Table data-testid="docs-plan-actions">
      <TableHeader>
        <TableRow>
          <TableHead>操作</TableHead>
          <TableHead>対象</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {actions.map((action, index) => (
          <TableRow key={`${action.operation}-${action.path ?? index}`}>
            <TableCell className="whitespace-nowrap">
              {operationLabels[action.operation ?? ""] ?? action.operation ?? "未確認"}
            </TableCell>
            <TableCell className="break-all">{actionTarget(action)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

export function ProjectDocsMaintenanceScreen({ projectId }: { projectId: string }) {
  const base = `/api/projects/${encodeURIComponent(projectId)}/docs/maintenance`;
  const key = projectKeys.docs(projectId, "maintenance");
  const query = useQuery({
    queryKey: key,
    queryFn: async ({ signal }) => {
      try {
        return await apiGet<Maintenance>(base, signal);
      } catch (error) {
        if (error instanceof ApiError && error.status === 409) return null;
        throw error;
      }
    },
  });
  const sender = useActionResult(key);
  const [plan, setPlan] = useState("");
  const [policy, setPolicy] = useState("");
  const [parseError, setParseError] = useState<{ field: "plan" | "policy"; message: string } | null>(null);
  function body(op: string): Record<string, unknown> | null {
    const out: Record<string, unknown> = { op };
    const field = op === "adopt" ? "policy" : "plan";
    try {
      if (op === "approve" || op === "apply") out.plan = JSON.parse(plan || JSON.stringify(query.data?.proposal));
      if (op === "adopt") out.policy = JSON.parse(policy || JSON.stringify(query.data?.policy));
    } catch {
      setParseError({ field, message: "JSON として読めません。括弧と引用符を確かめてください。" });
      return null;
    }
    setParseError(null);
    return out;
  }
  async function run(op: string) {
    const payload = body(op);
    if (!payload) throw new Error("JSON として読めないため送っていません。欄のエラーを確かめてください。");
    const [outcome] = await sender.run([{ id: op, path: base, body: payload }]);
    if (outcome && !outcome.ok) throw new Error("送信に失敗しました。画面上部の結果を確かめてください。");
  }
  const result = Object.values(sender.results).at(-1);
  const taskId =
    result?.ok && result.response && typeof result.response === "object" && "task_id" in result.response
      ? String(result.response.task_id)
      : null;
  const actions = query.data?.proposal?.actions ?? [];
  const fileCount = new Set(actions.flatMap((a) => a.paths ?? (a.path ? [a.path] : []))).size;
  const followUp = "完了すると「結果のタスクを開く」が出ます。そのタスクで差分を確認します";
  return (
    <ScreenFrame title={`文書の保守 ${projectId}`} route="/projects/:id/docs/maintenance">
      <nav aria-label="文書の頁" className="flex flex-wrap gap-3">
        <Link className={navLink} to="/projects/$id/docs" params={{ id: projectId }}>
          ← 文書
        </Link>
        <Link className={navLink} to="/projects/$id" params={{ id: projectId }}>
          案件詳細
        </Link>
      </nav>
      <p className="text-muted-foreground">監査結果と整理案を確認してから実行します。</p>
      <ActionResultView result={result} />
      {taskId && (
        <Link className={navLink} to="/tasks/$id" params={{ id: taskId }}>
          結果のタスクを開く
        </Link>
      )}
      {result?.ok && <JsonDetails summary="送信結果の JSON" value={result.response} />}
      <FetchFrame query={query} subject="文書の保守情報">
        {query.data === null ? (
          <Notice title="文書リポジトリがありません">
            <p>先に文書画面で文書リポジトリを用意してください。</p>
            <Link className={navLink} to="/projects/$id/docs" params={{ id: projectId }}>
              文書を用意する
            </Link>
          </Notice>
        ) : query.data ? (
          <div className="min-w-0 space-y-4" data-testid="docs-maintenance">
            <ProjectSection
              title="監査結果"
              testId="docs-audit"
              description={`文書 ${query.data.audit?.documents?.length ?? 0} 件のうち指摘のあるもの。`}
            >
              <AuditFindings audit={query.data.audit} />
              <Button disabled={sender.pending} onClick={() => void run("audit").catch(() => undefined)}>
                監査結果を保存
              </Button>
              <JsonDetails summary="監査結果の JSON" value={query.data.audit} />
              {query.data.saved_report != null && (
                <JsonDetails summary="保存済み監査レポート" value={query.data.saved_report} />
              )}
            </ProjectSection>
            <ProjectSection title="整理案" testId="docs-plan" description="承認してから適用します。">
              <PlanTable actions={actions} />
              {(query.data.proposal?.rationale ?? []).length > 0 && (
                <ul className="list-disc ps-6 text-label text-muted-foreground">
                  {(query.data.proposal?.rationale ?? []).map((reason) => (
                    <li key={reason}>{reason}</li>
                  ))}
                </ul>
              )}
              <details>
                <summary className="min-h-11 cursor-pointer py-2 text-label text-muted-foreground">
                  整理案の JSON を直す
                </summary>
                <label className="block">
                  整理案 JSON
                  <textarea
                    className={input}
                    value={plan || JSON.stringify(query.data.proposal, null, 2)}
                    aria-invalid={parseError?.field === "plan" || undefined}
                    aria-describedby={parseError?.field === "plan" ? "docs-plan-error" : undefined}
                    onChange={(e) => setPlan(e.target.value)}
                  />
                </label>
              </details>
              {parseError?.field === "plan" && (
                <p id="docs-plan-error" role="alert" className="text-danger-foreground">
                  {parseError.message}
                </p>
              )}
              <div className="flex flex-wrap gap-2">
                <ConfirmDialog
                  trigger={<Button disabled={sender.pending}>整理案を承認</Button>}
                  title="整理案を承認しますか"
                  target={`整理案（${actions.length} 操作・${fileCount} file）`}
                  consequence="承認した案だけが後で適用できるようになります。文書はまだ変わりません。"
                  reversibility="承認は適用するまで文書に影響しません。案を直して承認し直せます"
                  followUp={followUp}
                  confirmLabel="整理案を承認する"
                  onConfirm={() => run("approve")}
                />
                <ConfirmDialog
                  trigger={
                    <Button variant="primary" disabled={sender.pending}>
                      承認済み案を適用
                    </Button>
                  }
                  title="整理案を適用しますか"
                  target={`承認済みの整理案（${actions.length} 操作・${fileCount} file）`}
                  consequence="文書の移動・削除・統合を行う review タスクを作ります。"
                  reversibility="文書リポジトリの履歴から戻せます。review タスクで差し戻すこともできます"
                  followUp={followUp}
                  confirmLabel="整理案を適用する"
                  onConfirm={() => run("apply")}
                />
              </div>
            </ProjectSection>
            <ProjectSection
              title="文書管理ポリシー"
              testId="docs-policy"
              description={`現在の方針: ${modeLabels[query.data.policy?.mode ?? ""] ?? query.data.policy?.mode ?? "未確認"}`}
            >
              <details>
                <summary className="min-h-11 cursor-pointer py-2 text-label text-muted-foreground">
                  ポリシーの JSON を直す
                </summary>
                <label className="block">
                  ポリシー JSON
                  <textarea
                    className={input}
                    value={policy || JSON.stringify(query.data.policy, null, 2)}
                    aria-invalid={parseError?.field === "policy" || undefined}
                    aria-describedby={parseError?.field === "policy" ? "docs-policy-error" : undefined}
                    onChange={(e) => setPolicy(e.target.value)}
                  />
                </label>
              </details>
              {parseError?.field === "policy" && (
                <p id="docs-policy-error" role="alert" className="text-danger-foreground">
                  {parseError.message}
                </p>
              )}
              <ConfirmDialog
                trigger={<Button disabled={sender.pending}>ポリシーを採用</Button>}
                title="ポリシーを採用しますか"
                target="この案件の文書管理ポリシー"
                consequence="以後の監査と整理案はこのポリシーで作られます。"
                reversibility="前のポリシーの JSON を採用し直せば戻ります"
                followUp="この画面の監査結果と整理案が新しい方針で作り直されます"
                confirmLabel="ポリシーを採用する"
                onConfirm={() => run("adopt")}
              />
            </ProjectSection>
          </div>
        ) : null}
      </FetchFrame>
    </ScreenFrame>
  );
}
