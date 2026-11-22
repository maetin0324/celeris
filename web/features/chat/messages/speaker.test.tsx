// ADR 2026-11-21-chat-discussion-mode D7・D9: 発言の主体（人・operator・MCP・CoS）と、議論・案の印の表示。
// 守る不変条件: 主体は author・relay の記録から導き、human: でない主体の発言を「あなた」と表示しない。議論・案の発言には印が付き、
// 実行と未指定の発言の表示は mode を足す前と同じ。既存の messages の試験は author・relay・discussion_mode を持つ発言を
// 描かないので、取り次ぎの発言が「あなた」と出る誤りも印の欠落も検出できない。
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ChatMessage } from "../../../api/generated/types";
import { message } from "../data/fixtures.test-support";
import { MessageItem, PendingItem } from "./message-item";
import { discussionModeMark, messageSpeaker } from "./speaker";

const relay = (over: Partial<NonNullable<ChatMessage["relay"]>> = {}): NonNullable<ChatMessage["relay"]> => ({
  relayed_by: "operator:claude-code",
  said_at: "2026-10-05T00:00:00Z",
  scope: {},
  ...over,
});

const html = (m: ChatMessage) => renderToStaticMarkup(<MessageItem message={m} streaming={false} />);

describe("chat_discussion_mode_speaker", () => {
  it("chat_discussion_mode_speaker_distinguishes_human_operator_mcp_and_cos", () => {
    const user = (over: Partial<ChatMessage>) => messageSpeaker(message("m1", 1, { role: "user", ...over }));
    expect(user({ author: "human:web" })).toEqual({ kind: "human", label: "あなた" });
    expect(user({ author: null })).toEqual({ kind: "human", label: "あなた" });
    expect(user({ author: "operator:claude-code", relay: relay() })).toEqual({
      kind: "operator",
      label: "operator claude-code が取り次いだ発言",
    });
    expect(user({ author: "operator:claude-code" })).toEqual({
      kind: "operator",
      label: "operator claude-code の発言",
    });
    expect(user({ author: "mcp:c1", relay: relay({ relayed_by: "mcp:c1" }) })).toEqual({
      kind: "mcp",
      label: "MCP c1 が取り次いだ発言",
    });
    expect(user({ author: "mcp:c1" })).toEqual({ kind: "mcp", label: "MCP c1 の発言" });
    expect(
      user({
        author: "cos:g1",
        relay: relay({ relayed_by: "cos:g1", source: { thread_id: "g1", message_id: "m0", author: "human:web" } }),
      }),
    ).toEqual({ kind: "cos", label: "統括 CoS が取り次いだ人の発言" });
    expect(user({ author: "robot" })).toEqual({ kind: "unknown", label: "記録の主体 robot の発言" });
    expect(messageSpeaker(message("m2", 2, { role: "assistant" }))).toEqual({ kind: "cos", label: "CoS" });
    expect(messageSpeaker(message("m3", 3, { role: "system" }))).toEqual({ kind: "system", label: "システム" });
  });

  it("chat_discussion_mode_speaker_relayed_message_is_not_shown_as_the_person", () => {
    const out = html(
      message("m1", 1, { role: "user", text: "これで進めて", author: "operator:claude-code", relay: relay() }),
    );
    expect(out).toContain('aria-label="operator claude-code が取り次いだ発言"');
    expect(out).toContain('data-speaker="operator"');
    expect(out).not.toContain('aria-label="あなた"');
    const mcp = html(message("m2", 2, { role: "user", text: "x", author: "mcp:c1" }));
    expect(mcp).toContain('data-speaker="mcp"');
    expect(mcp).toContain("MCP c1 の発言");
    const person = html(message("m3", 3, { role: "user", text: "x", author: "human:web" }));
    expect(person).toContain('aria-label="あなた"');
    expect(person).not.toContain("chat-speaker");
  });

  it("chat_discussion_mode_mark_shows_discuss_and_propose", () => {
    const discuss = html(message("m1", 1, { role: "user", text: "批判して", discussion_mode: "discuss" }));
    expect(discuss).toContain('data-slot="chat-discussion-mode"');
    expect(discuss).toContain('data-mode="discuss"');
    expect(discuss).toContain(">議論</span>");
    const propose = html(message("m2", 2, { role: "user", text: "案に", discussion_mode: "propose" }));
    expect(propose).toContain('data-mode="propose"');
    expect(propose).toContain(">案</span>");
    // 取り次ぎの発言でも、主体と印の両方を出す。
    const relayed = html(
      message("m3", 3, {
        role: "user",
        text: "x",
        author: "mcp:c1",
        relay: relay({ relayed_by: "mcp:c1" }),
        discussion_mode: "discuss",
      }),
    );
    expect(relayed).toContain("MCP c1 が取り次いだ発言");
    expect(relayed).toContain('data-mode="discuss"');
    const pending = renderToStaticMarkup(
      <PendingItem
        pending={{
          client_message_id: "c1",
          text: "送信中",
          attachment_ids: [],
          mode: "queue",
          discussion_mode: "propose",
          state: "sending",
        }}
      />,
    );
    expect(pending).toContain('data-mode="propose"');
  });

  it("chat_discussion_mode_mark_absent_for_execute_and_unset", () => {
    expect(discussionModeMark("execute")).toBeUndefined();
    expect(discussionModeMark(null)).toBeUndefined();
    const plain = html(message("m1", 1, { role: "user", text: "直して" }));
    expect(html(message("m1", 1, { role: "user", text: "直して", discussion_mode: "execute" }))).toBe(plain);
    expect(plain).not.toContain("chat-message-meta");
    expect(plain).toContain('aria-label="あなた"');
  });
});
