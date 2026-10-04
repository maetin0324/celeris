import { createRoot } from "react-dom/client";
import "../../../styles.css";
import { ArtifactPreview } from "../artifact-preview";

const root = document.getElementById("root");
if (!root) throw new Error("fixture root がありません");
createRoot(root).render(
  <main className="flex flex-col gap-4 p-4">
    <h1 className="text-title">成果物プレビュー</h1>
    <ArtifactPreview taskId="T1" idx={0} name="報告.md" />
    <ArtifactPreview taskId="T1" idx={1} name="添付.zip" />
  </main>,
);
