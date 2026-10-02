import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { apiGet } from "../../api/client";
import { projectKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";

type Maintenance = { audit?: unknown; proposal?: unknown; policy?: unknown; saved_report?: unknown };
export function ProjectDocsMaintenanceScreen({ projectId }: { projectId: string }) {
  const base = `/api/projects/${encodeURIComponent(projectId)}/docs/maintenance`;
  const key = projectKeys.docs(projectId, "maintenance");
  const query = useQuery({ queryKey: key, queryFn: ({ signal }) => apiGet<Maintenance>(base, signal) });
  const sender = useActionResult(key);
  const [plan, setPlan] = useState("");
  const [policy, setPolicy] = useState("");
  async function run(op: string) {
    const body: Record<string, unknown> = { op };
    try {
      if (op === "approve" || op === "apply") body.plan = JSON.parse(plan || JSON.stringify(query.data?.proposal));
      if (op === "adopt") body.policy = JSON.parse(policy || JSON.stringify(query.data?.policy));
    } catch {
      return;
    }
    await sender.run([{ id: op, path: base, body }]);
  }
  const result = Object.values(sender.results).at(-1);
  const taskId =
    result?.ok && result.response && typeof result.response === "object" && "task_id" in result.response
      ? String(result.response.task_id)
      : null;
  return (
    <ScreenFrame title={`文書の保守 ${projectId}`} route="/projects/:id/docs/maintenance">
      <Link className="min-h-11 underline" to="/projects/$id/docs" params={{ id: projectId }}>
        ← 文書
      </Link>
      <p>監査結果と整理案を確認してから実行します。</p>
      <ActionResultView result={result} />
      {taskId && (
        <Link className="underline" to="/tasks/$id" params={{ id: taskId }}>
          結果のタスクを開く
        </Link>
      )}
      {result?.ok && (
        <pre className="max-w-full overflow-auto whitespace-pre-wrap">{JSON.stringify(result.response, null, 2)}</pre>
      )}
      <FetchFrame query={query}>
        {query.data && (
          <div className="min-w-0 space-y-4" data-testid="docs-maintenance">
            <section>
              <h2>監査結果</h2>
              <Button disabled={sender.pending} onClick={() => void run("audit")}>
                監査結果を保存
              </Button>
              <pre className="max-w-full overflow-auto whitespace-pre-wrap">
                {JSON.stringify(query.data.audit, null, 2)}
              </pre>
            </section>
            {query.data.saved_report != null && (
              <details>
                <summary className="min-h-11">保存済み監査レポート</summary>
                <pre className="max-w-full overflow-auto whitespace-pre-wrap">
                  {JSON.stringify(query.data.saved_report, null, 2)}
                </pre>
              </details>
            )}
            <section className="space-y-2">
              <h2>整理案</h2>
              <label className="block">
                整理案 JSON
                <textarea
                  className="box-border min-h-44 w-full max-w-full rounded border p-2"
                  value={plan || JSON.stringify(query.data.proposal, null, 2)}
                  onChange={(e) => setPlan(e.target.value)}
                />
              </label>
              <div className="flex flex-wrap gap-2">
                <Button disabled={sender.pending} onClick={() => void run("approve")}>
                  整理案を承認
                </Button>
                <Button disabled={sender.pending} onClick={() => void run("apply")}>
                  承認済み案を適用
                </Button>
              </div>
            </section>
            <section className="space-y-2">
              <h2>文書管理ポリシー</h2>
              <label className="block">
                ポリシー JSON
                <textarea
                  className="box-border min-h-32 w-full max-w-full rounded border p-2"
                  value={policy || JSON.stringify(query.data.policy, null, 2)}
                  onChange={(e) => setPolicy(e.target.value)}
                />
              </label>
              <Button disabled={sender.pending} onClick={() => void run("adopt")}>
                ポリシーを採用
              </Button>
            </section>
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
