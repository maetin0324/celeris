import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { apiGet } from "../../api/client";
import type { HarnessView, LlmSourcesView, LlmSourceView } from "../../api/generated/types";
import { harnessKeys, sourceKeys } from "../../api/queries/keys";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { DataList } from "../../components/ui/data-list";
import { Section } from "../../components/ui/panel";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";

const sourceUrl = (id: string) => `/sources/${encodeURIComponent(id)}`;

export function reachabilityLabel(reachable: boolean | null | undefined): string {
  return reachable === true ? "到達可能" : reachable === false ? "到達不可" : "不明";
}

export function deploymentModelId(deploymentId: string): string {
  return deploymentId.split("/").slice(1).join("/");
}

export function SourcesScreen() {
  const query = useQuery({
    queryKey: sourceKeys.list(),
    queryFn: ({ signal }) => apiGet<LlmSourcesView>("/api/llm/sources", signal),
  });
  return (
    <ScreenFrame
      title="LLM source"
      route="/sources"
      breadcrumb={[{ label: "LLM source" }]}
      description="接続先と稼働状況を確認します。変更は設定で行います。"
    >
      <FetchFrame query={query} subject="LLM source">
        {query.data && (
          <Section title={`source 一覧（${query.data.sources.length}）`}>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>source</TableHead>
                  <TableHead>種別</TableHead>
                  <TableHead>到達性</TableHead>
                  <TableHead>deployments</TableHead>
                  <TableHead>accounts</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {query.data.sources.map((s) => (
                  <TableRow key={s.id}>
                    <TableCell>
                      <Link className="underline" to={sourceUrl(s.id)}>
                        {s.id}
                      </Link>
                    </TableCell>
                    <TableCell>{s.kind}</TableCell>
                    <TableCell>
                      <Badge tone={s.reachable === true ? "success" : s.reachable === false ? "warning" : "neutral"}>
                        {reachabilityLabel(s.reachable)}
                      </Badge>
                    </TableCell>
                    <TableCell>{s.deployments?.length ?? 0}</TableCell>
                    <TableCell>{s.accounts.length}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </Section>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}

export function SourceDetailScreen() {
  const { sourceId } = useParams({ from: "/sources/$sourceId" });
  const query = useQuery({
    queryKey: [...sourceKeys.all, "detail", sourceId],
    queryFn: ({ signal }) => apiGet<LlmSourceView>(`/api/llm/sources/${encodeURIComponent(sourceId)}`, signal),
  });
  return (
    <ScreenFrame
      title={sourceId}
      route={sourceUrl(sourceId)}
      breadcrumb={[{ label: "LLM source", link: { to: "/sources" } }, { label: sourceId }]}
      description="source のアカウントと model deployment を表示します。変更は設定で行います。"
    >
      <FetchFrame query={query} subject="LLM source">
        {query.data && (
          <div className="space-y-4">
            <Section title="概要">
              <DataList
                items={[
                  { key: "kind", label: "種別", value: query.data.kind },
                  {
                    key: "reachable",
                    label: "到達性",
                    value:
                      query.data.reachable === true
                        ? "到達可能"
                        : query.data.reachable === false
                          ? `到達不可${query.data.unreachable_reason ? `: ${query.data.unreachable_reason}` : ""}`
                          : "不明",
                  },
                  { key: "accounts", label: "accounts", value: query.data.accounts.length },
                  { key: "deployments", label: "deployments", value: query.data.deployments?.length ?? 0 },
                ]}
              />
            </Section>
            <Section title={`accounts（${query.data.accounts.length}）`}>
              {query.data.accounts.length ? (
                <ul className="space-y-2">
                  {query.data.accounts.map((a) => (
                    <li key={a.id}>
                      <Link className="underline" to="/accounts" hash={`${sourceId}-${a.id}`}>
                        {a.id}
                      </Link>{" "}
                      — {a.logged_in ? "ログイン済み" : "未ログイン"}
                    </li>
                  ))}
                </ul>
              ) : (
                <p>アカウントはありません。</p>
              )}
            </Section>
            <Section title={`models（${query.data.deployments?.length ?? 0}）`}>
              {query.data.deployments?.length ? (
                <ul className="space-y-2">
                  {query.data.deployments.map((d) => (
                    <li key={d.deployment_id}>
                      {deploymentModelId(d.deployment_id)} — {d.reachability}
                    </li>
                  ))}
                </ul>
              ) : (
                <p>deployment はありません。</p>
              )}
            </Section>
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}

export function HarnessesScreen() {
  const query = useQuery({
    queryKey: harnessKeys.list(),
    queryFn: ({ signal }) => apiGet<HarnessView[]>("/api/harnesses", signal),
  });
  return (
    <ScreenFrame
      title="Harness"
      route="/harnesses"
      breadcrumb={[{ label: "Harness" }]}
      description="登録済み harness の能力・適格性を確認します。変更は設定で行います。"
    >
      <FetchFrame query={query} subject="harness">
        {query.data && (
          <div className="space-y-4">
            {query.data.map((h) => (
              <Section key={h.id} title={h.id} description={`種別: ${h.kind}`}>
                <div className="space-y-3">
                  <DataList
                    items={[
                      { key: "api", label: "API formats", value: h.api_formats.join(", ") || "なし" },
                      { key: "sources", label: "対応 source 種別", value: h.source_kinds.join(", ") || "なし" },
                      { key: "capabilities", label: "能力", value: h.capabilities.join(", ") || "なし" },
                      {
                        key: "defaults",
                        label: "execution class defaults",
                        value: h.execution_class_defaults.join(", ") || "なし",
                      },
                    ]}
                  />
                  <h3 className="font-semibold">適格性ルール</h3>
                  {h.rules.length ? (
                    <ul className="list-disc pl-5">
                      {h.rules.map((r) => (
                        <li
                          key={`${r.genre ?? ""}:${r.role ?? ""}:${r.execution_class ?? ""}:${r.required_capabilities.join(",")}`}
                        >
                          {[r.genre, r.role, r.execution_class].filter(Boolean).join(" / ") || "全般"} — 必要能力:{" "}
                          {r.required_capabilities.join(", ") || "なし"}
                          {r.requires_repo != null ? ` / repository ${r.requires_repo ? "必須" : "不要"}` : ""}
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <p>ルールはありません。</p>
                  )}
                  <h3 className="font-semibold">既定 route candidates</h3>
                  <ul>
                    {h.route_defaults.map((c) => (
                      <li key={`${c.source_id}/${c.model_id}`}>
                        {c.source_id} / {c.model_id}
                      </li>
                    ))}
                  </ul>
                </div>
              </Section>
            ))}
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
