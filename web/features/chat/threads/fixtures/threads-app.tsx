import { createRoot } from "react-dom/client";
import "../../../../styles.css";
import { thread } from "../../data/fixtures.test-support";
import { type ThreadApi, ThreadsModel } from "../model";
import { ChatThreads } from "../threads";

const inbox = thread({ id: "inbox", kind: "inbox", title: "受信箱" });
const human = thread({ id: "human", title: "作業の相談" });
const api: ThreadApi = {
  list: async () => ({ items: [human, inbox], next_cursor: null }),
  create: async () => ({ thread: human }),
  patch: async () => ({ thread: human }),
};
const model = new ThreadsModel(api);
const root = document.getElementById("root");
if (!root) throw new Error("root not found");
createRoot(root).render(
  <main>
    <h1>会話の試験</h1>
    <ChatThreads
      model={model}
      inboxWaitingCount={2}
      onSelectThread={(id) => {
        document.body.dataset.selected = id;
      }}
    />
  </main>,
);
