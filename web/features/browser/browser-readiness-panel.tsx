import { useQuery } from "@tanstack/react-query";
import { apiGet } from "../../api/client";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { DataList } from "../../components/ui/data-list";
import { Section } from "../../components/ui/panel";

// preflight 葉の read-only response。site-policy/grant は生成型を使う。
// readiness は schema の収録前なので、D5 の公開 wire contract をここで定義する。
export type BrowserReadiness = { items: Array<{ status: string; check: string; detail: string }> };
const statusLabels: Record<string, { label: string; tone: BadgeTone }> = {
  OK: { label: "利用可", tone: "success" },
  NG: { label: "対応が必要", tone: "danger" },
  WARN: { label: "要確認", tone: "warning" },
  SKIP: { label: "対象外", tone: "neutral" },
};
export function readinessView(status: string) {
  return statusLabels[status] ?? { label: "未確認", tone: "neutral" as const };
}
export function browserReadinessQuery() {
  return {
    queryKey: ["browser", "readiness"] as const,
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<BrowserReadiness>("/api/browser/readiness", signal),
  };
}
export function BrowserReadinessPanel() {
  const query = useQuery(browserReadinessQuery());
  return (
    <Section
      title="ブラウザ実行の前提を点検"
      description="daemon の設定と接続状態を表示します。対応が必要な項目を確認してください。"
    >
      <div className="space-y-3">
        <Button variant="secondary" disabled={query.isFetching} onClick={() => void query.refetch()}>
          点検結果を再取得
        </Button>
        <FetchFrame query={query} subject="ブラウザ実行の点検結果">
          {query.data?.items.length === 0 ? <p>点検項目はありません。</p> : null}
          <DataList
            items={
              query.data?.items.map((item, index) => ({
                key: `${item.check}-${index}`,
                label: item.check,
                value: (
                  <div className="space-y-1">
                    <Badge tone={readinessView(item.status).tone}>{readinessView(item.status).label}</Badge>
                    <p className="break-all">{item.detail}</p>
                  </div>
                ),
              })) ?? []
            }
          />
        </FetchFrame>
      </div>
    </Section>
  );
}
