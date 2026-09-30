import type { ReactNode } from "react";
import { Panel } from "../ui/panel";

// 画面の見出しと枠（P2-02）。h1 は tabIndex=-1 で、遷移後の focus 先になる（S4）。中身は各画面の Phase で足す。
export function ScreenFrame({ title, route, children }: { title: string; route: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col gap-4" data-screen={route}>
      <h1 tabIndex={-1} className="text-xl font-semibold break-words focus:outline-none">
        {title}
      </h1>
      {children ?? <Panel title="準備中">この画面の中身は後の Phase で入ります。</Panel>}
    </div>
  );
}
