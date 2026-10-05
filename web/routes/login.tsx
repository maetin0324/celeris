import { createFileRoute } from "@tanstack/react-router";
import { type FormEvent, useRef, useState } from "react";
import { Button } from "../components/ui/button";
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
  const passwordRef = useRef<HTMLInputElement>(null);

  // 失敗したら理由を alert で読み上げ、直すべき欄（唯一の入力欄）へ focus を戻して打ち直せるようにする。
  function fail(message: string) {
    setError(message);
    setBusy(false);
    const input = passwordRef.current;
    if (input) {
      input.focus();
      input.select();
    }
  }

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
      fail(
        response.status === 401
          ? "パスワードが違います"
          : `ログインできません（${response.status}）。時間をおいて再度お試しください`,
      );
    } catch {
      fail("gateway に接続できません。gateway が起動しているか確かめてください");
    }
  }

  return (
    <main className="mx-auto flex w-full max-w-sm flex-col gap-4 p-6">
      <h1 className="text-title font-semibold text-foreground">Celeris にログイン</h1>
      {/* 戻り先を先に示す（失敗して打ち直す間も、ログイン後にどこへ戻るかが分かるように）。 */}
      <p className="text-label text-muted-foreground">
        {next === "/" ? (
          "ログイン後はホームを開きます。"
        ) : (
          <>
            ログイン後に <code className="break-all font-mono text-foreground">{next}</code> へ戻ります。
          </>
        )}
      </p>
      <form
        method="post"
        action="/login"
        onSubmit={submit}
        aria-busy={busy}
        className="flex flex-col gap-3 rounded-lg border border-border bg-surface p-4"
      >
        <input type="hidden" name="next" value={next} />
        <label htmlFor="password" className="text-label font-medium text-foreground">
          パスワード
        </label>
        <input
          ref={passwordRef}
          id="password"
          name="password"
          type="password"
          autoComplete="current-password"
          required
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? "login-error" : undefined}
          // biome-ignore lint/a11y/noAutofocus: login 画面の唯一の入力欄
          autoFocus
          className="min-h-11 w-full rounded-md border border-input bg-surface px-3 text-body text-foreground"
        />
        {error ? (
          <p
            id="login-error"
            role="alert"
            className="rounded-md border border-border bg-danger px-3 py-2 text-label text-danger-foreground"
          >
            {error}
          </p>
        ) : null}
        <Button type="submit" variant="primary" disabled={busy} className="w-full">
          {busy ? "ログイン中…" : "ログイン"}
        </Button>
      </form>
    </main>
  );
}
