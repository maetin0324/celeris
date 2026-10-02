import { createFileRoute, redirect } from "@tanstack/react-router";

// R06 /org/secretary。現行と同じく /org/cos へ 302 相当で移す（fetch を待たない）。
export const Route = createFileRoute("/org/secretary")({
  beforeLoad: () => {
    throw redirect({ to: "/org/$id", params: { id: "cos" }, replace: true });
  },
});
