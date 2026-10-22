import { createRoot } from "react-dom/client";
import "../../../../styles.css";
import { thread } from "../../data/fixtures.test-support";
import { type ThreadApi, ThreadsModel } from "../model";
import { ChatThreads } from "../threads";

const general = thread({ id: "general", title: "統括 CoS", attention_count: 2 });
const project = thread({ id: "human", kind: "project", project_id: "A", title: "作業の相談" });
const api: ThreadApi = {
  list: async () => ({ items: [project, general], next_cursor: null }),
  open: async () => ({ thread: general }),
  read: async () => ({ thread: general }),
  patch: async () => ({ thread: project }),
};
const model = new ThreadsModel(api);
const root = document.getElementById("root");
if (!root) throw new Error("root not found");
createRoot(root).render(
  <main>
    <h1>会話の試験</h1>
    <ChatThreads
      model={model}
      onSelectThread={(id) => {
        document.body.dataset.selected = id;
      }}
    />
  </main>,
);
