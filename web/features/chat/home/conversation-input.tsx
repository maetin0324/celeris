import type { ChatThread } from "../../../api/generated/types";
import { ChatComposer, type ChatComposerProps, COS_COMPOSER_PLACEHOLDER } from "../composer/chat-composer";

/** ADR 2026-10-22-cos-fixed-threads D1: 旧会話（legacy）は読むだけ。daemon も 409 で入力を断る。 */
export const LEGACY_READ_ONLY_NOTICE =
  "この会話は移行前のもので、読むだけです。続きは統括 CoS か案件の CoS で話してください。";

/** The input of a conversation: the composer for CoS threads, a notice for read-only legacy threads. */
export function ConversationInput({
  thread,
  ...composer
}: { thread: ChatThread | null | undefined } & ChatComposerProps) {
  if (thread?.kind === "legacy") {
    return (
      <p role="note" data-legacy-read-only className="border-t border-border p-3 text-body text-muted-foreground">
        {LEGACY_READ_ONLY_NOTICE}
      </p>
    );
  }
  return <ChatComposer {...composer} placeholder={thread ? COS_COMPOSER_PLACEHOLDER : undefined} />;
}
