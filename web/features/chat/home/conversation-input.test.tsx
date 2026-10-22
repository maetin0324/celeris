import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ChatThread } from "../../../api/generated/types";
import { COS_COMPOSER_PLACEHOLDER } from "../composer/chat-composer";
import { thread } from "../data/fixtures.test-support";
import { initialChatState } from "../data/reducer";
import type { ChatSession, ChatSessionSnapshot } from "../data/session";
import { ConversationInput, LEGACY_READ_ONLY_NOTICE } from "./conversation-input";

function render(kind: ChatThread["kind"]): string {
  const chat = initialChatState("t1");
  chat.thread = thread({ kind });
  const snapshot: ChatSessionSnapshot = { chat, stream: "open", loading: false, loadError: undefined };
  return renderToStaticMarkup(
    <ConversationInput threadId="t1" thread={chat.thread} snapshot={snapshot} session={{} as ChatSession} />,
  );
}

// ADR 2026-10-22-cos-fixed-threads D1: legacy threads are read-only in the home too.
describe("chat_home_conversation_input", () => {
  it("chat_home_legacy_thread_shows_a_read_only_notice_instead_of_the_composer", () => {
    const html = render("legacy");
    expect(html).toContain(LEGACY_READ_ONLY_NOTICE);
    expect(html).toContain("data-legacy-read-only");
    expect(html).not.toContain("CoS へのメッセージ");
    expect(html).not.toContain("送信");
  });

  it("chat_home_cos_threads_keep_the_composer", () => {
    for (const kind of ["general", "project"] as const) {
      const html = render(kind);
      expect(html).toContain(`placeholder="${COS_COMPOSER_PLACEHOLDER}"`);
      expect(html).toContain("送信");
      expect(html).not.toContain("data-legacy-read-only");
    }
  });
});
