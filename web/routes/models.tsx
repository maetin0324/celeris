import { createFileRoute } from "@tanstack/react-router";
import { ModelsScreen } from "../features/ops/models-screen";

// /models。LLM source ごとのモデル一覧と上書き。
export const Route = createFileRoute("/models")({
  validateSearch: (search: Record<string, unknown>) => ({
    source: typeof search.source === "string" ? search.source : undefined,
  }),
  component: ModelsScreen,
});
