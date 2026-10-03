import { createRoot } from "react-dom/client";
import "../../../styles.css";
import { Button } from "../button";
import { ConfirmDialog } from "../confirm-dialog";
import { Drawer } from "../drawer";

let complete: (() => void) | undefined;
let fail: (() => void) | undefined;
let calls = 0;
Object.assign(window, {
  resolveOverlay: () => complete?.(),
  rejectOverlay: () => fail?.(),
});

function App() {
  return (
    <main className="p-4">
      <h1 className="text-title">Overlay 確認</h1>
      <div className="flex flex-wrap gap-2">
        <Drawer trigger={<Button>詳細を開く</Button>} title="タスクの詳細" description="対象の状態を確認します">
          <Button>パネル内の操作</Button>
        </Drawer>
        <ConfirmDialog
          trigger={<Button>削除を確認</Button>}
          title="タスクを削除"
          target="タスク A"
          consequence="タスク A とその実行履歴を削除します。"
          reversibility="元に戻せません。"
          followUp="タスク一覧で確認できます。"
          confirmLabel="タスク A を削除"
          onConfirm={() => {
            calls += 1;
            const output = document.getElementById("calls");
            if (output) output.textContent = String(calls);
            return new Promise<void>((resolve, reject) => {
              complete = resolve;
              fail = () => reject(new Error("削除できませんでした。再確認してください。"));
            });
          }}
        />
      </div>
      <p>
        送信回数: <output id="calls">0</output>
      </p>
      <div className="flex gap-2">
        <Button onClick={() => complete?.()}>送信を成功させる</Button>
        <Button onClick={() => fail?.()}>送信を失敗させる</Button>
      </div>
    </main>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("fixture root がありません");
createRoot(root).render(<App />);
