import { createRoot } from "react-dom/client";
import "../../../styles.css";
import { InboxIntegrationActions, TaskIntegrationActions } from "../task-integration";
import { inboxItem, integration } from "./task-integration-data";

function App() {
  return (
    <main className="grid min-w-0 gap-4 p-4">
      <h1>取り込み状態の確認</h1>
      {(["unintegrated", "integrated", "discarded", "held"] as const).map((state) => (
        <section key={state} aria-label={state}>
          <TaskIntegrationActions
            taskId="T 1"
            integration={{ ...integration, state }}
            promotion={state === "integrated" ? { status: "promotion_pending" } : undefined}
          />
        </section>
      ))}
      <section aria-label="受信箱">
        <InboxIntegrationActions item={inboxItem} />
      </section>
    </main>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("root missing");
createRoot(root).render(<App />);
