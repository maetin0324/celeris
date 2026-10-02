import type { ReactNode } from "react";

// 画面の中の枠（P2-02）。見出しは h2 以下にし、h1 は画面の見出しだけにする。
export function Panel({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <section aria-label={title} className="rounded border border-neutral-300 p-4">
      <h2 className="text-base font-semibold">{title}</h2>
      {children ? <div className="mt-2 text-sm text-neutral-700">{children}</div> : null}
    </section>
  );
}
