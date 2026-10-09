import { useRouter } from "@tanstack/react-router";
import { useState } from "react";
import { Button } from "../../components/ui/button";
import { locateDecision } from "./decision-model";

/**
 * チャットのカードから決定の詳細（`/tasks/<出した task>#decision-<id>`）へ移る。カードは出した task を持たないので、
 * 押したときに `GET /decisions` から引く（回答済みで受信箱から消えた決定も辿れる）。
 */
export function DecisionLocateButton({ decisionId }: { decisionId: string }) {
  const router = useRouter({ warn: false });
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  async function onClick() {
    setPending(true);
    setMessage(null);
    try {
      const href = await locateDecision(decisionId);
      if (!href) setMessage("この決定は見つかりません。");
      else if (router) void router.navigate({ href });
      else window.location.assign(href);
    } catch {
      setMessage("決定の場所を読めませんでした。時間をおいて試してください。");
    } finally {
      setPending(false);
    }
  }
  return (
    <span className="inline-flex min-w-0 flex-wrap items-center gap-2">
      <Button variant="ghost" size="sm" disabled={pending} onClick={() => void onClick()}>
        決定の詳細・答えを変える
      </Button>
      {message ? (
        <span role="alert" className="text-label text-danger-foreground">
          {message}
        </span>
      ) : null}
    </span>
  );
}
