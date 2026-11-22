// ADR 2026-11-21-chat-discussion-mode D5・D6・D9: 吹き出しの編集・撤回の操作（自分の発言だけ）と「編集済み（rev n）」「撤回済み」・
// 前の版、409 の再読込、案・要約の引用の状態と失効の警告・引いた発言への link。外部ネットワークには出ない。
import { isValidElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ChatCitationView, ChatMessage } from "../../../api/generated/types";
import { detail, event, message, page, T } from "../data/fixtures.test-support";
import { chatReducer, initialChatState } from "../data/reducer";
import { REVISION_CONFLICT_TEXT } from "../data/revision";
import { CITATION_STALE_TEXT, CitationList } from "./citations";
import { MessageItem } from "./message-item";
import { MessageList } from "./message-list";
import { type MessageRevisionActions, RevisionEntry, RevisionFeedback, revisionNoteText } from "./revision";
import { citedMessageHref, fileCitations } from "./workspace-links";

const ME = "human:web";
const SUMMARY = "discussion/summary/s.json";
const PROPOSAL = "discussion/proposal/p.json";

function actions(viewer: string | null = ME): MessageRevisionActions {
  return { viewer, revise: vi.fn(async () => ({ kind: "error", detail: "x" }) as const), reload: vi.fn() };
}

const mine = (over: Partial<ChatMessage> = {}) => message("m1", 1, { text: "仮説 A は誤り", author: ME, ...over });

function cite(path: string, messageId: string, state: ChatCitationView["state"], thread = T): ChatCitationView {
  return { path, citation: { thread_id: thread, message_id: messageId, revision: 1, sha256: "a".repeat(64) }, state };
}

function reply(citations: ChatCitationView[]): ChatMessage {
  return message("m2", 2, {
    role: "assistant",
    text: "要約と案を添えました",
    citations,
    workspace_files: [
      { path: SUMMARY, attachment_id: "a1" },
      { path: PROPOSAL, attachment_id: "a2" },
    ],
  });
}

const render = (m: ChatMessage, revision?: MessageRevisionActions) =>
  renderToStaticMarkup(<MessageItem message={m} streaming={false} revision={revision} />);

const buttons = (html: string) => [...html.matchAll(/<button[^>]*>([^<]*)</g)].map((match) => match[1]);

describe("chat_message_revision_actions", () => {
  it("chat_message_revision_own_message_shows_edit_and_retract", () => {
    const html = render(mine(), actions());
    expect(html).toContain('data-slot="chat-revision-controls"');
    expect(buttons(html)).toEqual(["編集", "撤回"]);
    // 表示している人がまだ分からなくても、人の発言には出す（daemon が書いた人かを確かめる）。
    expect(buttons(render(mine(), actions(null)))).toEqual(["編集", "撤回"]);
    // 届く前の発言は編集だけ。
    expect(buttons(render(mine({ state: "queued" }), actions()))).toEqual(["編集"]);
  });

  it("chat_message_revision_cos_relay_and_other_people_show_no_actions", () => {
    const cases: ChatMessage[] = [
      message("m2", 2, { role: "assistant", text: "CoS の返事" }),
      message("m3", 3, { role: "system", text: "記録" }),
      mine({ author: "operator:claude-code" }),
      mine({ author: "mcp:c1" }),
      mine({ author: "cos:t0", relay: { relayed_by: "cos:t0", said_at: "t", scope: { task_ids: [] } } }),
      mine({ relay: { relayed_by: "operator:claude-code", said_at: "t", scope: { task_ids: [] } } }),
      mine({ author: "human:alice" }),
      mine({ author: null }),
      mine({ state: "running" }),
    ];
    for (const m of cases) {
      const html = render(m, actions());
      expect(html, `${m.role} ${m.author}`).not.toContain("chat-revision-controls");
      expect(buttons(html)).not.toContain("編集");
      expect(buttons(html)).not.toContain("撤回");
    }
    // 操作の口を渡さない一覧（他の画面）にも出ない。
    expect(render(mine())).not.toContain("chat-revision-controls");
  });

  it("chat_message_revision_list_hides_actions_on_other_people_once_viewer_is_known", () => {
    const state = [
      { type: "snapshot", detail: detail(), messages: page([mine({ author: "human:alice" })], "10") },
      { type: "pending_add", clientMessageId: "c-1", text: "自分の発言" },
      {
        type: "pending_ack",
        clientMessageId: "c-1",
        message: message("m4", 4, { text: "自分の発言", author: ME, client_message_id: "c-1" }),
      },
    ].reduce((s, a) => chatReducer(s, a as Parameters<typeof chatReducer>[1]), initialChatState(T));
    const html = renderToStaticMarkup(
      <MessageList state={state} revision={{ revise: actions().revise, reload: vi.fn() }} />,
    );
    // 自分（m4）にだけ操作が出る。
    expect(html.match(/data-slot="chat-revision-controls"/g)).toHaveLength(1);
    const own = html.slice(html.indexOf('id="chat-message-m4"'));
    expect(own).toContain("chat-revision-controls");
  });
});

describe("chat_message_revision_display", () => {
  it("chat_message_revision_edited_shows_rev_and_previous_versions_button", () => {
    const m = mine({ text: "仮説 A は条件付きで誤り", revision: 3, edited: { by: ME, at: "t" } });
    expect(revisionNoteText(m)).toBe("編集済み（rev 3）");
    const html = render(m, actions());
    expect(html).toContain("編集済み（rev 3）");
    expect(buttons(html)).toContain("前の版を見る");
  });

  it("chat_message_revision_retracted_shows_note_and_no_actions", () => {
    const m = mine({ text: "", revision: 2, retracted: { by: ME, at: "t", reason: "前提が違った" } });
    const html = render(m, actions());
    expect(html).toContain("撤回済み（理由: 前提が違った）");
    expect(html).toContain('data-revision-state="retracted"');
    expect(html).not.toContain("chat-revision-controls");
    expect(buttons(html)).toContain("前の版を見る");
  });

  it("chat_message_revision_unchanged_message_has_no_note_or_history", () => {
    const html = render(mine(), actions());
    expect(html).not.toContain("chat-revision-note");
    expect(html).not.toContain("前の版");
  });

  it("chat_message_revision_previous_version_shows_the_old_body", () => {
    const html = renderToStaticMarkup(
      <RevisionEntry
        at="2026-11-22T00:00:00Z"
        data={{
          message_id: "m1",
          kind: "edit",
          revision: 2,
          previous_revision: 1,
          previous_text: "仮説 A は誤り",
          previous_sha256: "a".repeat(64),
          by: ME,
        }}
      />,
    );
    expect(html).toContain("rev 1");
    expect(html).toContain("仮説 A は誤り");
    expect(html).toContain("human:web が編集");
  });

  it("chat_message_revision_conflict_asks_to_reload", () => {
    const onReload = vi.fn();
    const element = RevisionFeedback({ outcome: { kind: "conflict", detail: "409" }, onReload });
    const html = renderToStaticMarkup(element);
    expect(html).toContain(REVISION_CONFLICT_TEXT);
    expect(buttons(html)).toContain("再読込");
    // 再読込の button が渡した reload を呼ぶ。
    const action = (element as ReactElement<{ action: ReactElement<{ onClick: () => void }> }>).props.action;
    expect(isValidElement(action)).toBe(true);
    action.props.onClick();
    expect(onReload).toHaveBeenCalledTimes(1);
  });
});

describe("chat_citation_display", () => {
  it("chat_citation_files_group_by_path_and_stale_when_any_is_not_current", () => {
    const entries = fileCitations(
      reply([cite(SUMMARY, "m1", "current"), cite(PROPOSAL, "m1", "current"), cite(PROPOSAL, "m3", "edited")]),
    );
    expect(entries.map((entry) => [entry.path, entry.attachmentId, entry.citations.length, entry.stale])).toEqual([
      [SUMMARY, "a1", 1, false],
      [PROPOSAL, "a2", 2, true],
    ]);
  });

  it("chat_citation_all_current_has_no_stale_warning", () => {
    const html = render(reply([cite(SUMMARY, "m1", "current")]));
    expect(html).toContain('data-slot="chat-citations"');
    expect(html).toContain('data-stale="false"');
    expect(html).toContain("最新");
    expect(html).not.toContain(CITATION_STALE_TEXT);
  });

  it("chat_citation_edited_retracted_missing_show_state_and_stale_warning", () => {
    for (const [state, label] of [
      ["edited", "編集された"],
      ["retracted", "撤回された"],
      ["missing", "見つからない"],
    ] as const) {
      const html = render(reply([cite(SUMMARY, "m1", "current"), cite(SUMMARY, "m3", state)]));
      expect(html, state).toContain(`data-state="${state}"`);
      expect(html, state).toContain(label);
      expect(html, state).toContain(CITATION_STALE_TEXT);
      expect(html, state).toContain('data-slot="chat-citation-stale"');
    }
  });

  it("chat_citation_links_move_to_the_cited_message", () => {
    expect(citedMessageHref({ thread_id: T, message_id: "m1" }, T)).toBe("#chat-message-m1");
    expect(citedMessageHref({ thread_id: "g/1", message_id: "m9" }, T)).toBe("/?thread=g%2F1#chat-message-m9");
    const html = renderToStaticMarkup(
      <CitationList message={reply([cite(SUMMARY, "m1", "current"), cite(SUMMARY, "m9", "current", "g1")])} />,
    );
    expect(html).toContain('href="#chat-message-m1"');
    expect(html).toContain('href="/?thread=g1#chat-message-m9"');
    expect(html).toContain("（統括の会話）");
    // 移る先の吹き出しが同じ id を持つ。
    expect(render(mine())).toContain('id="chat-message-m1"');
  });

  it("chat_citation_reply_without_citations_is_unchanged", () => {
    const html = render(message("m2", 2, { role: "assistant", text: "ふつうの返事" }));
    expect(html).not.toContain("chat-citations");
  });

  it("chat_citation_sse_retract_turns_the_shown_summary_stale", () => {
    const state = [
      { type: "snapshot", detail: detail(), messages: page([mine(), reply([cite(SUMMARY, "m1", "current")])], "10") },
      {
        type: "event",
        event: event(
          11,
          "message_revised",
          {
            message_id: "m1",
            kind: "retract",
            revision: 2,
            previous_revision: 1,
            previous_text: "仮説 A は誤り",
            previous_sha256: "a".repeat(64),
            by: ME,
            reason: "",
          },
          { message_id: "m1" },
        ),
      },
    ].reduce((s, a) => chatReducer(s, a as Parameters<typeof chatReducer>[1]), initialChatState(T));
    const shown = state.messages.m2;
    expect(shown).toBeDefined();
    const html = render(shown as ChatMessage);
    expect(html).toContain('data-state="retracted"');
    expect(html).toContain(CITATION_STALE_TEXT);
  });
});
