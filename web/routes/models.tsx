import { createFileRoute } from "@tanstack/react-router";
import { ModelsScreen } from "../features/ops/models-screen";

// /models。LLM source ごとのモデル一覧と上書き。
export const Route = createFileRoute("/models")({ component: ModelsScreen });
