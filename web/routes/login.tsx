import { createFileRoute } from "@tanstack/react-router";
import { type FormEvent, useState } from "react";
import { clearProtectedCaches, safeNextPath } from "../lib/session";

type LoginSearch = { next?: string; error?: string };

// /login（R04）。成功で gateway が署名 cookie を出し、`next`（同一オリジンの絶対パス）へ移る。
// JS が無くても form の POST で動く。daemon には触れない。
export const Route = createFileRoute("/login")({
  validateSearch: (search: Record<string, unknown>): LoginSearch => ({
    next: typeof search.next === "string" ? search.next : undefined,
    error: typeof search.error === "string" ? search.error : undefined,
  }),
  component: LoginPage,
});

function LoginPage() {
  const search = Route.useSearch();
  const next = safeNextPath(search.next);
  const [error, setError] = useState(search.error ? "パスワードが違います" : "");
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setError("");
    const password = String(new FormData(event.currentTarget).get("password") ?? "");
    try {
      const response = await fetch("/login", {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "application/json" },
        body: JSON.stringify({ password, next }),
      });
      if (response.ok) {
        const body = (await response.json()) as { next?: string };
        clearProtectedCaches();
        window.location.assign(safeNextPath(body.next));
        return;
      }
      setError(response.status === 401 ? "パスワードが違います" : `ログインできません（${response.status}）`);
    } catch {
      setError("gateway に接続できません");
    }
    setBusy(false);
  }

  return (
    <main className="mx-auto flex w-full max-w-sm flex-col gap-4 p-6">
      <h1 className="text-xl font-semibold">Celeris にログイン</h1>
      <form method="post" action="/login" onSubmit={submit} className="flex flex-col gap-3">
        <input type="hidden" name="next" value={next} />
        <label htmlFor="password" className="text-sm font-medium">
          パスワード
        </label>
        <input
          id="password"
          name="password"
          type="password"
          autoComplete="current-password"
          required
          // biome-ignore lint/a11y/noAutofocus: login 画面の唯一の入力欄
          autoFocus
          className="min-h-11 w-full rounded border border-neutral-400 px-3 text-base"
        />
        {error ? (
          <p role="alert" className="text-sm text-red-700">
            {error}
          </p>
        ) : null}
        <button
          type="submit"
          disabled={busy}
          className="min-h-11 w-full rounded bg-neutral-900 px-4 text-base font-medium text-white disabled:opacity-60"
        >
          ログイン
        </button>
      </form>
    </main>
  );
}
