import { createFileRoute } from "@tanstack/react-router";

// /login の空の枠。form と認証の境界は P1-06 で入れる。
export const Route = createFileRoute("/login")({
  component: LoginPage,
});

function LoginPage() {
  return (
    <main className="mx-auto max-w-sm p-6">
      <h1 className="text-xl font-semibold">Celeris にログイン</h1>
    </main>
  );
}
