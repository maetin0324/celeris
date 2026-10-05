import { useQuery } from "@tanstack/react-query";
import { Link, useRouter } from "@tanstack/react-router";
import { type FormEvent, type RefObject, useEffect, useRef, useState } from "react";
import { apiGet } from "../../api/client";
import type { Graph } from "../../api/generated/types";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import type { BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { StatusBadge, statusView } from "../../components/ui/status-badge";
import { type GraphLayout, layoutGraph, NODE_HEIGHT, NODE_WIDTH } from "./graph-layout";

const control = "min-h-11 rounded-md border border-input bg-surface px-3 text-body text-foreground";

// 節点の左端の帯。StatusBadge と同じ状態→tone の対応（statusView）で、tone の前景 token を使う。
// 色だけで状態を伝えないよう、節点の中には StatusBadge（文字の状態）も置く。
const toneEdge: Readonly<Record<BadgeTone, string>> = {
  success: "border-l-success-foreground",
  warning: "border-l-warning-foreground",
  danger: "border-l-danger-foreground",
  info: "border-l-info-foreground",
  neutral: "border-l-neutral-foreground",
  running: "border-l-running-foreground",
};

export function GraphScreen({ root, depth }: { root?: string; depth?: number }) {
  const router = useRouter();
  const formRef = useRef<HTMLFormElement>(null);
  useEffect(() => {
    const form = formRef.current;
    if (!form) return;
    const rootInput = form.elements.namedItem("root");
    const depthInput = form.elements.namedItem("depth");
    if (rootInput instanceof HTMLInputElement) rootInput.value = root ?? "";
    if (depthInput instanceof HTMLInputElement) depthInput.value = depth === undefined ? "" : String(depth);
  }, [root, depth]);
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
    <ScreenFrame title="依存グラフ" route="/graph">
      <form
        ref={formRef}
        method="get"
        onSubmit={submit}
        data-testid="graph-filter-form"
        aria-label="グラフの絞り込み"
        className="flex flex-wrap items-end gap-3"
      >
        <label className="flex min-w-0 flex-col gap-1 text-label font-medium text-foreground">
          起点の task id（root）
          <input
            name="root"
            aria-label="root"
            defaultValue={root ?? ""}
            className={`${control} w-56 max-w-full font-mono`}
          />
        </label>
        <label className="flex flex-col gap-1 text-label font-medium text-foreground">
          深さ（depth）
          <input
            name="depth"
            type="number"
            min="0"
            inputMode="numeric"
            aria-label="depth"
            defaultValue={depth ?? ""}
            className={`${control} w-24 tabular-nums`}
          />
        </label>
        <Button type="submit">絞り込み</Button>
      </form>
      <FetchFrame query={query}>{layout && <GraphCanvas layout={layout} />}</FetchFrame>
    </ScreenFrame>
  );
}

type HorizontalScroll = { overflow: boolean; atStart: boolean; atEnd: boolean };

// 枠が横に溢れているか、左右の端にいるか。狭い幅で横に続く graph の手がかり（注記と端の影）に使う。
function useHorizontalScroll(ref: RefObject<HTMLElement | null>): HorizontalScroll {
  const [state, setState] = useState<HorizontalScroll>({ overflow: false, atStart: true, atEnd: true });
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => {
      const max = el.scrollWidth - el.clientWidth;
      const next = { overflow: max > 1, atStart: el.scrollLeft <= 1, atEnd: el.scrollLeft >= max - 1 };
      setState((prev) =>
        prev.overflow === next.overflow && prev.atStart === next.atStart && prev.atEnd === next.atEnd ? prev : next,
      );
    };
    update();
    el.addEventListener("scroll", update, { passive: true });
    const observer = new ResizeObserver(update);
    observer.observe(el);
    if (el.firstElementChild) observer.observe(el.firstElementChild);
    return () => {
      el.removeEventListener("scroll", update);
      observer.disconnect();
    };
  }, [ref]);
  return state;
}

function GraphCanvas({ layout }: { layout: GraphLayout }) {
  const canvasRef = useRef<HTMLDivElement>(null);
  const scroll = useHorizontalScroll(canvasRef);
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <p data-testid="graph-summary" className="text-label text-muted-foreground tabular-nums">
        {layout.nodes.length} ノード / {layout.edges.length} 辺
      </p>
      {/* 狭い幅では graph が枠の右へ続いても見た目に手がかりが無かった（fix-r6 narrow）。溢れている間は注記を出し、
          まだ見えていない側の端に影を置く。 */}
      {scroll.overflow ? (
        <p id="graph-scroll-hint" data-testid="graph-scroll-hint" className="text-label text-muted-foreground">
          横に続きます。枠を左右に動かすと残りの task が見えます。
        </p>
      ) : null}
      <div className="relative min-w-0">
        {/* 横に広いグラフはこの区画の中だけで scroll させ、page には横 scroll を出さない。 */}
        <div
          ref={canvasRef}
          aria-describedby={scroll.overflow ? "graph-scroll-hint" : undefined}
          data-testid="graph-canvas"
          className="min-w-0 max-w-full overflow-auto overscroll-contain rounded-lg border border-border bg-muted"
          style={{ maxHeight: "70vh" }}
        >
          <div className="relative" style={{ width: layout.width, height: layout.height }}>
            <svg
              aria-hidden="true"
              className="pointer-events-none absolute inset-0"
              width={layout.width}
              height={layout.height}
              viewBox={`0 0 ${layout.width} ${layout.height}`}
            >
              <defs>
                <marker
                  id="graph-arrow"
                  viewBox="0 0 8 8"
                  refX="8"
                  refY="4"
                  markerWidth="8"
                  markerHeight="8"
                  orient="auto-start-reverse"
                >
                  <path d="M0 0 L8 4 L0 8 z" className="fill-input" />
                </marker>
              </defs>
              {layout.edges.map((edge) => (
                <line
                  key={`${edge.from}-${edge.to}`}
                  x1={edge.x1}
                  y1={edge.y1}
                  x2={edge.x2}
                  y2={edge.y2}
                  className="stroke-input"
                  strokeWidth="1.5"
                  markerEnd="url(#graph-arrow)"
                />
              ))}
            </svg>
            <ul aria-label="タスクの依存グラフ">
              {layout.nodes.map((node) => (
                <li key={node.id} className="absolute" style={{ left: node.x, top: node.y, width: NODE_WIDTH }}>
                  <Link
                    to="/tasks/$id"
                    params={{ id: node.id }}
                    data-graph-node={node.id}
                    data-status={node.status}
                    className={`flex flex-col gap-1 overflow-hidden rounded-md border border-l-4 border-border bg-surface px-3 py-2 text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring ${toneEdge[statusView(node.status).tone]}`}
                    style={{ height: NODE_HEIGHT }}
                  >
                    <span className="truncate font-mono text-label text-muted-foreground" title={node.id}>
                      {node.id}
                    </span>
                    <span className="truncate text-label font-medium" title={node.title}>
                      {node.title}
                    </span>
                    <StatusBadge status={node.status} className="break-normal whitespace-nowrap" />
                  </Link>
                </li>
              ))}
            </ul>
          </div>
        </div>
        {scroll.overflow && !scroll.atStart ? (
          <div
            aria-hidden="true"
            data-testid="graph-edge-start"
            className="pointer-events-none absolute inset-y-px left-px w-6 rounded-l-lg bg-linear-to-r from-foreground/15 to-transparent"
          />
        ) : null}
        {scroll.overflow && !scroll.atEnd ? (
          <div
            aria-hidden="true"
            data-testid="graph-edge-end"
            className="pointer-events-none absolute inset-y-px right-px w-6 rounded-r-lg bg-linear-to-l from-foreground/15 to-transparent"
          />
        ) : null}
      </div>
    </div>
  );
}
