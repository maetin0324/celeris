import { useQuery } from "@tanstack/react-query";
import { apiGet } from "../../api/client";
import type { RoutingEvidenceList, RoutingStageList } from "../../api/generated/types";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Section } from "../../components/ui/panel";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";

export function RoutingView({ stages, evidence }: { stages: RoutingStageList; evidence: RoutingEvidenceList }) {
  return (
    <div className="min-w-0 space-y-4">
      <Section
        title={`段階（${stages.stages.length}）`}
        description="genre と harness ごとの現在段階、根拠の版、直近の変更を表示します。enforce への承認は celerisctl または API で行います。"
      >
        {stages.stages.length === 0 ? (
          <p>段階の記録はありません。</p>
        ) : (
          <Table aria-label="routing 段階">
            <TableHeader>
              <TableRow>
                <TableHead>genre</TableHead>
                <TableHead>harness</TableHead>
                <TableHead>段階</TableHead>
                <TableHead>開始</TableHead>
                <TableHead>根拠版</TableHead>
                <TableHead>直近の変更</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {stages.stages.map((s) => (
                <TableRow key={`${s.genre}:${s.harness_id}`}>
                  <TableCell className="break-all">{s.genre}</TableCell>
                  <TableCell className="break-all">{s.harness_id}</TableCell>
                  <TableCell>
                    <Badge tone={s.stage === "enforce" ? "success" : "neutral"}>
                      {s.stage === "enforce" ? "enforce" : s.stage === "shadow" ? "shadow" : "未確認"}
                    </Badge>
                  </TableCell>
                  <TableCell className="break-all">{s.since ?? "不明"}</TableCell>
                  <TableCell className="break-all">
                    {s.evidence_version ?? "不明"}
                    {s.latest_evidence_version && s.latest_evidence_version !== s.evidence_version
                      ? `（最新: ${s.latest_evidence_version}）`
                      : ""}
                  </TableCell>
                  <TableCell className="break-words">
                    {s.last_change ? (
                      <>
                        {s.last_change.ts} / {s.last_change.actor} / {s.last_change.from ?? "開始"} → {s.last_change.to}
                        : {s.last_change.reason}
                        <div className="text-label text-muted-foreground">
                          guardrail 窓:{" "}
                          {typeof s.last_change.window === "object" && s.last_change.window !== null
                            ? Object.entries(s.last_change.window)
                                .map(([key, value]) => `${key} ${String(value)}`)
                                .join("・") || "不明"
                            : String(s.last_change.window ?? "不明")}
                        </div>
                      </>
                    ) : (
                      "記録なし"
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </Section>
      <Section
        title={`根拠表（${evidence.groups.reduce((n, g) => n + g.rows.length, 0)} 行）`}
        description="区間は評価結果の信頼区間です。選択の再計算は行いません。"
      >
        {evidence.groups.length === 0 ? (
          <p>根拠表はありません。</p>
        ) : (
          <div className="space-y-4">
            {evidence.groups.map((g) => (
              <section key={`${g.genre}:${g.harness_id}`} className="min-w-0">
                <h3 className="mb-2 font-semibold break-words">
                  {g.genre} × {g.harness_id}{" "}
                  <span className="text-label text-muted-foreground">根拠版 {g.evidence_version ?? "不明"}</span>
                </h3>
                <Table aria-label={`${g.genre} ${g.harness_id} 根拠表`}>
                  <TableHeader>
                    <TableRow>
                      <TableHead>source / model</TableHead>
                      <TableHead>合格率（区間）</TableHead>
                      <TableHead>件数</TableHead>
                      <TableHead>timeout</TableHead>
                      <TableHead>中央値</TableHead>
                      <TableHead>適格</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {g.rows.map((r) => (
                      <TableRow key={`${r.source_id}:${r.model_id}`}>
                        <TableCell className="break-all">
                          {r.source_id} / {r.model_id}
                          <div className="text-label text-muted-foreground">harness {r.harness_version}</div>
                        </TableCell>
                        <TableCell className="tabular-nums">
                          {r.pass}/{r.n} ({r.pass_lower.toFixed(3)}–{r.pass_upper.toFixed(3)})
                        </TableCell>
                        <TableCell className="tabular-nums">{r.n}</TableCell>
                        <TableCell className="tabular-nums">{(r.timeout_rate * 100).toFixed(1)}%</TableCell>
                        <TableCell className="tabular-nums">
                          {r.wall_p90_s == null ? "—" : `${r.wall_p90_s}s`} /{" "}
                          {r.usd_per_pass == null ? "—" : `$${r.usd_per_pass}`}
                        </TableCell>
                        <TableCell>{r.qualifies ? "適格" : "未適格"}</TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </section>
            ))}
          </div>
        )}
      </Section>
    </div>
  );
}

export function RoutingScreen() {
  const stages = useQuery({
    queryKey: ["routing", "stages"],
    queryFn: ({ signal }) => apiGet<RoutingStageList>("/api/v1/routing/stages", signal),
  });
  const evidence = useQuery({
    queryKey: ["routing", "evidence"],
    queryFn: ({ signal }) => apiGet<RoutingEvidenceList>("/api/v1/routing/evidence", signal),
  });
  const data = stages.data && evidence.data ? { stages: stages.data, evidence: evidence.data } : undefined;
  const query = {
    data,
    isPending: stages.isPending || evidence.isPending,
    isFetching: stages.isFetching || evidence.isFetching,
    isError: stages.isError || evidence.isError,
    error: stages.error ?? evidence.error,
    dataUpdatedAt: Math.min(stages.dataUpdatedAt || 0, evidence.dataUpdatedAt || 0),
    refetch: async () => {
      await Promise.all([stages.refetch(), evidence.refetch()]);
    },
  };
  return (
    <ScreenFrame
      title="Source・model routing"
      route="/routing"
      breadcrumb={[{ label: "Routing" }]}
      description="段階と評価根拠を確認します。段階を enforce に進める操作は celerisctl または API で人が承認します。"
    >
      <FetchFrame query={query} subject="routing">
        {data && <RoutingView stages={data.stages} evidence={data.evidence} />}
      </FetchFrame>
    </ScreenFrame>
  );
}
