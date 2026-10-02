import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { McpCallsView, McpClient, McpClientsView } from "../../api/generated/types";
import { accountKeys } from "../../api/queries/keys";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";

function Calls({ id }: { id: string }) {
  const query = useQuery({
    queryKey: accountKeys.list({ section: "mcp-calls", id }),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<McpCallsView>(`/api/mcp/clients/${encodeURIComponent(id)}/calls`, signal),
  });
  return (
    <FetchFrame query={query}>
      {query.data &&
        (query.data.items.length === 0 ? (
          <p className="text-sm">呼び出し履歴がありません。</p>
        ) : (
          <ul className="text-sm space-y-1" aria-label={`呼び出し ${id}`}>
            {query.data.items.map((call) => (
              <li key={call.id} className="break-words">
                {call.at} {call.tool} {call.ok ? "ok" : `error ${call.error_kind ?? ""}`}（{call.latency_ms} ms）
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
  return (
    <li className="rounded border p-2 min-w-0" aria-label={`MCP クライアント ${client.name}`}>
      <details onToggle={(e) => setOpen(e.currentTarget.open)}>
        <summary className="min-h-11 cursor-pointer break-words">
          {client.name}（{client.revoked_at ? "失効" : "有効"}）
        </summary>
        <p className="text-sm break-words">
          scopes: {(client.scopes ?? []).join(", ") || "なし"} / 最終利用: {client.last_used_at ?? "なし"}
        </p>
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
    <section className="space-y-2 min-w-0" aria-label="MCP クライアント">
      <h2 className="text-lg font-semibold">MCP クライアント</h2>
      <FetchFrame query={query}>
        {query.data && (
          <ul className="space-y-2">
            {query.data.items.map((client) => (
              <ClientCard key={client.id} client={client} />
            ))}
          </ul>
        )}
      </FetchFrame>
    </section>
  );
}
