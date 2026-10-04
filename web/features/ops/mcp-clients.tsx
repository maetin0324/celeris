import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { McpCallsView, McpClient, McpClientsView } from "../../api/generated/types";
import { accountKeys } from "../../api/queries/keys";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { DataList } from "../../components/ui/data-list";
import { formatAbsolute } from "../../lib/time";

/** client の状態。失効を先に見る。badge には必ずこの文字を出す。 */
export function mcpClientState(client: Pick<McpClient, "revoked_at" | "last_used_at">): {
  tone: BadgeTone;
  label: string;
} {
  if (client.revoked_at) return { tone: "danger", label: "失効" };
  return { tone: "success", label: "有効" };
}

function Calls({ id }: { id: string }) {
  const query = useQuery({
    queryKey: accountKeys.list({ section: "mcp-calls", id }),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<McpCallsView>(`/api/mcp/clients/${encodeURIComponent(id)}/calls`, signal),
  });
  return (
    <FetchFrame query={query} subject="呼び出し履歴">
      {query.data &&
        (query.data.items.length === 0 ? (
          <p className="text-label text-muted-foreground">呼び出し履歴がありません。</p>
        ) : (
          <ul className="min-w-0 divide-y divide-border text-label text-foreground" aria-label={`呼び出し ${id}`}>
            {query.data.items.map((call) => (
              <li key={call.id} className="flex min-w-0 flex-wrap items-center gap-2 py-2">
                <Badge tone={call.ok ? "success" : "danger"}>{call.ok ? "成功" : "失敗"}</Badge>
                <span className="min-w-0 break-words">
                  {call.tool} {call.ok ? "ok" : `error ${call.error_kind ?? ""}`}（{call.latency_ms} ms）・
                  {formatAbsolute(call.at)}
                </span>
              </li>
            ))}
          </ul>
        ))}
    </FetchFrame>
  );
}

/** calls は開いたときだけ取りに行く。 */
function ClientCard({ client }: { client: McpClient }) {
  const [open, setOpen] = useState(false);
  const state = mcpClientState(client);
  return (
    <li
      className="min-w-0 rounded-lg border border-border bg-surface p-4"
      aria-label={`MCP クライアント ${client.name}`}
    >
      <details className="min-w-0 space-y-3" onToggle={(e) => setOpen(e.currentTarget.open)}>
        <summary className="flex min-h-11 min-w-0 cursor-pointer flex-wrap items-center gap-2 break-words text-body font-semibold text-foreground">
          {client.name}
          {/* 読み上げと文字列検索では「name（状態）」と一続きに読ませ、見た目は badge にする。 */}
          <span className="sr-only">（</span>
          <Badge tone={state.tone} data-state={state.label}>
            {state.label}
          </Badge>
          <span className="sr-only">）</span>
        </summary>
        <DataList
          items={[
            { key: "scopes", label: "scopes", value: (client.scopes ?? []).join(", ") || "なし" },
            {
              key: "last_used",
              label: "最終利用",
              value: client.last_used_at ? formatAbsolute(client.last_used_at) : "まだ使われていません",
            },
            { key: "created", label: "作成", value: formatAbsolute(client.created_at) },
            ...(client.revoked_at
              ? [{ key: "revoked", label: "失効日時", value: formatAbsolute(client.revoked_at) }]
              : []),
          ]}
        />
        {open && <Calls id={client.id} />}
      </details>
    </li>
  );
}

export function McpClientsSection() {
  const query = useQuery({
    queryKey: accountKeys.list({ section: "mcp-clients" }),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<McpClientsView>("/api/mcp/clients", signal),
  });
  return (
    <section className="min-w-0 space-y-3" aria-label="MCP クライアント">
      <h2 className="text-section font-semibold text-foreground">MCP クライアント</h2>
      <p className="text-label text-muted-foreground">
        外部の道具から Celeris を使う client です。名前を開くと scopes・最終利用・呼び出し履歴を見られます。token
        の値は表示しません。
      </p>
      <FetchFrame query={query} subject="MCP クライアント">
        {query.data &&
          (query.data.items.length === 0 ? (
            <p className="text-body text-muted-foreground">MCP クライアントはありません。</p>
          ) : (
            <ul className="grid min-w-0 gap-3 xl:grid-cols-2">
              {query.data.items.map((client) => (
                <ClientCard key={client.id} client={client} />
              ))}
            </ul>
          ))}
      </FetchFrame>
    </section>
  );
}
