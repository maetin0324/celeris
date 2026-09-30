import { useQuery } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import type { FormEvent } from "react";
import { apiGet } from "../../api/client";
import type { Graph } from "../../api/generated/types";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { buttonClassName } from "../../components/ui/button";
import { layoutGraph } from "./graph-layout";

export function GraphScreen({ root, depth }: { root?: string; depth?: number }) {
  const router = useRouter();
  const params = new URLSearchParams();
  if (root) params.set("root", root);
  if (depth !== undefined) params.set("depth", String(depth));
  const path = `/api/graph${params.size ? `?${params}` : ""}`;
  const query = useQuery({
    queryKey: ["graph", root ?? "", depth ?? null],
    queryFn: ({ signal }) => apiGet<Graph>(path, signal),
  });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const nextRoot = String(data.get("root") ?? "").trim() || undefined;
    const rawDepth = String(data.get("depth") ?? "").trim();
    const nextDepth = rawDepth === "" ? undefined : Math.max(0, Math.floor(Number(rawDepth)));
    const next = new URLSearchParams();
    if (nextRoot) next.set("root", nextRoot);
    if (nextDepth !== undefined && Number.isFinite(nextDepth)) next.set("depth", String(nextDepth));
    router.history.push(`/graph${next.size ? `?${next}` : ""}`, {});
  }

  const layout = query.data ? layoutGraph(query.data) : null;
  return (
    <div data-screen="/graph" className="flex min-w-0 flex-col gap-4">
      <h1 tabIndex={-1} className="text-xl font-semibold focus:outline-none">
        依存グラフ
      </h1>
      <form method="get" onSubmit={submit} data-testid="graph-filter-form" className="flex flex-wrap items-end gap-3">
        <label className="flex flex-col gap-1">
          root
          <input name="root" defaultValue={root ?? ""} className="min-h-11 w-56 max-w-full rounded border px-3" />
        </label>
        <label className="flex flex-col gap-1">
          depth
          <input
            name="depth"
            type="number"
            min="0"
            defaultValue={depth ?? ""}
            className="min-h-11 w-24 rounded border px-3"
          />
        </label>
        <button className={buttonClassName} type="submit">
          絞り込み
        </button>
      </form>
      <FetchFrame query={query}>
        {layout && (
          <>
            <p data-testid="graph-summary" className="text-sm">
              {layout.nodes.length} ノード / {layout.edges.length} 辺
            </p>
            <div
              data-testid="graph-canvas"
              className="max-w-full overflow-auto rounded border border-neutral-300"
              style={{ maxHeight: "70vh" }}
            >
              <svg
                role="img"
                aria-label="タスクの依存グラフ"
                width={layout.width}
                height={layout.height}
                viewBox={`0 0 ${layout.width} ${layout.height}`}
              >
                {layout.edges.map((edge) => (
                  <line
                    key={`${edge.from}-${edge.to}`}
                    x1={edge.x1}
                    y1={edge.y1}
                    x2={edge.x2}
                    y2={edge.y2}
                    stroke="#737373"
                    strokeWidth="2"
                  />
                ))}
                {layout.nodes.map((node) => (
                  <g key={node.id} data-graph-node={node.id}>
                    <title>{`${node.id}: ${node.title} (${node.status})`}</title>
                    <rect x={node.x} y={node.y} width="200" height="72" rx="8" fill="#fafafa" stroke="#525252" />
                    <text x={node.x + 10} y={node.y + 25} fontSize="13" fontWeight="bold">
                      {node.id}
                    </text>
                    <text x={node.x + 10} y={node.y + 45} fontSize="12">
                      {node.title.length > 24 ? `${node.title.slice(0, 23)}…` : node.title}
                    </text>
                    <text x={node.x + 10} y={node.y + 62} fontSize="11">
                      {node.status}
                    </text>
                  </g>
                ))}
              </svg>
            </div>
          </>
        )}
      </FetchFrame>
    </div>
  );
}
