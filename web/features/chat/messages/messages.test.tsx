import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ChatAttachment, ChatEvent } from "../../../api/generated/types";
import { Markdown, parseBlocks } from "../../../components/content/markdown";
import { card, delta, detail, event, message, page, run, T, thread } from "../data/fixtures.test-support";
import { type ChatAction, type ChatState, chatReducer, initialChatState, selectTimeline } from "../data/reducer";
import { attachmentIdsOf, fetchOlder } from "./history";
import {
  appendedKeys,
  type FollowState,
  followReducer,
  formatBytes,
  groupTools,
  initialFollowState,
  isAtBottom,
  nextAnnouncement,
  scrollTopAfterPrepend,
  statusLabel,
} from "./logic";
import { MessageItem } from "./message-item";
import { MessageList } from "./message-list";
import { AttachmentList, JumpToLatest, StatusLine, ToolCall } from "./parts";

function reduce(actions: (ChatAction | ChatEvent)[], start: ChatState = initialChatState(T)): ChatState {
  return actions.reduce<ChatState>(
    (state, action) => chatReducer(state, "thread_id" in action ? { type: "event", event: action } : action),
    start,
  );
}

function snapshot(items = [message("m1", 1, { text: "こんにちは" })], eventId = "10"): ChatAction {
  return { type: "snapshot", detail: detail({ thread: thread() }), messages: page(items, eventId) };
}

const tool = (over: Record<string, unknown> = {}) => ({
  call_id: "c1",
  name: "Bash",
  summary: "cargo test",
  detail: "running 3 tests\nok",
  error: false,
  state: "completed" as const,
  truncated: false,
  ...over,
});

function attachment(over: Partial<ChatAttachment> = {}): ChatAttachment {
  return {
    id: "a1",
    thread_id: T,
    name: "report.pdf",
    media_type: "application/pdf",
    size_bytes: 2048,
    sha256: "x",
    state: "ready",
    download_url: "/api/v1/chat/attachments/a1/content",
    preview_url: null,
    ...over,
  };
}

const count = (haystack: string, needle: string) => haystack.split(needle).length - 1;

describe("chat_messages_markdown", () => {
  it("chat_messages_markdown_sanitizes_html_and_unsafe_links", () => {
    const out = renderToStaticMarkup(
      <Markdown
        source={
          '<script>alert(1)</script>\n\n[x](javascript:alert(1)) [ok](https://example.com) <img src=x onerror="y">'
        }
      />,
    );
    expect(out).not.toContain("<script>");
    expect(out).not.toContain("<img");
    expect(out).toContain("&lt;script&gt;");
    expect(out).not.toContain("javascript:");
    expect(out).toContain('href="https://example.com"');
    expect(out).toContain('rel="noopener noreferrer"');
  });

  it("chat_messages_markdown_code_block_has_copy_and_keeps_blank_lines", () => {
    const source = '前\n\n```rust\nfn main() {\n\n  println!("<b>");\n}\n```\n\n後';
    const blocks = parseBlocks(source);
    expect(blocks.map((b) => b.kind)).toEqual(["text", "code", "text"]);
    const out = renderToStaticMarkup(<Markdown source={source} />);
    expect(out).toContain('aria-label="コードをコピー"');
    expect(out).toContain('aria-label="コード（rust）"');
    expect(out).toContain("min-h-11 min-w-11");
    expect(out).toContain("&lt;b&gt;");
    expect(out).toContain("fn main() {\n\n  println!");
  });

  it("chat_messages_markdown_unclosed_fence_while_streaming_is_code", () => {
    expect(parseBlocks("```ts\nconst a = 1;").map((b) => b.kind)).toEqual(["code"]);
  });

  it("chat_messages_markdown_table", () => {
    const out = renderToStaticMarkup(
      <Markdown source={"| 名前 | 状態 |\n| --- | :---: |\n| a | **ok** |\n| b | `x` |"} />,
    );
    expect(out).toContain("<table");
    expect(count(out, "<th ")).toBe(2);
    expect(count(out, "<td ")).toBe(4);
    expect(out).toContain('scope="col"');
    expect(out).toContain("<strong>ok</strong>");
    expect(out).toContain("overflow-x-auto");
  });

  it("chat_messages_markdown_keeps_existing_paragraph_and_heading_output", () => {
    const out = renderToStaticMarkup(<Markdown source={"# 見出し\n\n本文 [a](/tasks/1)"} />);
    expect(out).toContain('<h3 class="font-semibold">見出し</h3>');
    expect(out).toContain('<a href="/tasks/1" rel="noopener noreferrer" class="underline break-all">a</a>');
    const target = renderToStaticMarkup(<Markdown source="[a](/tasks/1)" links="target" />);
    expect(target).toContain("inline-flex min-h-11");
  });
});

describe("chat_messages_bubbles", () => {
  it("chat_messages_bubbles_user_text_is_plain_and_assistant_is_markdown", () => {
    const user = renderToStaticMarkup(
      <MessageItem message={message("m1", 1, { role: "user", text: "**太字にしない**" })} streaming={false} />,
    );
    expect(user).toContain('data-role="user"');
    expect(user).toContain("**太字にしない**");
    const assistant = renderToStaticMarkup(
      <MessageItem message={message("m2", 2, { role: "assistant", text: "**太字**" })} streaming={false} />,
    );
    expect(assistant).toContain('data-role="assistant"');
    expect(assistant).toContain("<strong>太字</strong>");
    expect(assistant).toContain('aria-label="CoS"');
  });

  it("chat_messages_bubbles_card_slot_gets_message_cards", () => {
    const render = vi.fn((cards: { title: string }[]) => (
      <span>{`カード ${cards.map((c) => c.title).join(",")}`}</span>
    ));
    const out = renderToStaticMarkup(
      <MessageItem
        message={message("m2", 2, { role: "assistant", text: "t", cards: [card({ title: "承認" })] })}
        streaming={false}
        renderCards={render}
      />,
    );
    expect(render).toHaveBeenCalledTimes(1);
    expect(out).toContain('data-slot="chat-cards"');
    expect(out).toContain("カード 承認");
  });

  it("chat_messages_bubbles_failed_state_is_shown", () => {
    const out = renderToStaticMarkup(
      <MessageItem
        message={message("m2", 2, { role: "assistant", text: "途中", state: "failed" })}
        streaming={false}
      />,
    );
    expect(out).toContain("失敗");
  });
});

describe("chat_messages_tool", () => {
  it("chat_messages_tool_collapsed_shows_name_summary_outcome_without_detail", () => {
    const out = renderToStaticMarkup(<ToolCall tool={{ ...tool(), run_id: "r1", message_id: null }} />);
    expect(out).toContain("Bash");
    expect(out).toContain("cargo test");
    expect(out).toContain("成功");
    expect(out).toContain('aria-expanded="false"');
    expect(out).toContain("min-h-11");
    expect(out).not.toContain("running 3 tests");
  });

  it("chat_messages_tool_expanded_renders_detail_and_failure", () => {
    const out = renderToStaticMarkup(
      <ToolCall
        tool={{ ...tool({ error: true, state: "failed", truncated: true }), run_id: null, message_id: null }}
        defaultOpen
      />,
    );
    expect(out).toContain('aria-expanded="true"');
    expect(out).toContain("running 3 tests");
    expect(out).toContain("失敗");
    expect(out).toContain('data-state="failed"');
    expect(out).toContain("省略");
  });

  it("chat_messages_tool_without_detail_is_not_a_button", () => {
    const out = renderToStaticMarkup(
      <ToolCall tool={{ ...tool({ detail: null, state: "running" }), run_id: null, message_id: null }} />,
    );
    expect(out).not.toContain("<button");
    expect(out).toContain("実行中");
  });

  it("chat_messages_tool_grouped_by_message_then_run_then_loose", () => {
    const state = reduce([
      snapshot([message("m1", 1), message("m2", 2, { role: "assistant", run_id: "r1", state: "running" })]),
      event(11, "tool", tool({ call_id: "a" }), { run_id: "r1", message_id: "m2" }),
      event(12, "tool", tool({ call_id: "b" }), { run_id: "r1" }),
      event(13, "tool", tool({ call_id: "c" }), { run_id: "r9" }),
    ]);
    const groups = groupTools(state);
    expect(groups.byMessage.m2?.map((t) => t.call_id)).toEqual(["a", "b"]);
    expect(groups.loose.map((t) => t.call_id)).toEqual(["c"]);
  });
});

describe("chat_messages_streaming", () => {
  it("chat_messages_streaming_draft_then_final_message_is_shown_once", () => {
    const streaming = reduce([snapshot(), delta(11, "m2", 0, "途中の"), delta(12, "m2", 9, "本文")]);
    const during = renderToStaticMarkup(<MessageList state={streaming} />);
    expect(count(during, "途中の本文")).toBe(1);
    expect(during).toContain('aria-busy="true"');
    expect(during).toContain('data-slot="chat-caret"');

    const done = reduce(
      [event(13, "message", { message: message("m2", 2, { role: "assistant", text: "途中の本文", run_id: "r1" }) })],
      streaming,
    );
    const after = renderToStaticMarkup(<MessageList state={done} />);
    expect(count(after, "途中の本文")).toBe(1);
    expect(after).not.toContain('data-slot="chat-caret"');
    expect(selectTimeline(done).filter((i) => i.key === "m:m2")).toHaveLength(1);
  });

  it("chat_messages_streaming_pending_and_acked_user_message_not_duplicated", () => {
    const pending = reduce([snapshot(), { type: "pending_add", clientMessageId: "c1", text: "送る" }]);
    expect(count(renderToStaticMarkup(<MessageList state={pending} />), "送る")).toBe(1);
    const acked = reduce(
      [event(11, "message", { message: message("m3", 3, { text: "送る", client_message_id: "c1" }) })],
      pending,
    );
    const out = renderToStaticMarkup(<MessageList state={acked} />);
    expect(count(out, "送る")).toBe(1);
    expect(out).not.toContain("送信中");
  });

  it("chat_messages_streaming_thinking_shows_label_or_public_summary", () => {
    expect(statusLabel({ phase: "thinking", summary: "" })).toBe("考え中");
    expect(statusLabel({ phase: "working", summary: " 試験を実行中 " })).toBe("試験を実行中");
    const state = reduce([
      snapshot(),
      event(11, "run", { run: run("r1", "running") }, { run_id: "r1" }),
      event(12, "status", { phase: "thinking", summary: "" }, { run_id: "r1" }),
    ]);
    const out = renderToStaticMarkup(<MessageList state={state} />);
    expect(out).toContain('data-slot="chat-status"');
    expect(out).toContain("考え中");
    const ended = reduce([event(13, "run", { run: run("r1", "completed") }, { run_id: "r1" })], state);
    expect(renderToStaticMarkup(<MessageList state={ended} />)).not.toContain("考え中");
    expect(renderToStaticMarkup(<StatusLine status={{ phase: "working", summary: "" }} />)).toContain("作業中");
  });
});

describe("chat_messages_attachments", () => {
  it("chat_messages_attachments_image_preview_and_file_download", () => {
    const out = renderToStaticMarkup(
      <AttachmentList
        ids={["a1", "a2", "a3"]}
        attachments={{
          a1: attachment(),
          a2: attachment({
            id: "a2",
            name: "図.png",
            media_type: "image/png",
            preview_url: "/api/v1/chat/attachments/a2/preview",
          }),
        }}
      />,
    );
    expect(out).toContain("report.pdf");
    expect(out).toContain("2.0 KB");
    expect(out).toContain('href="/api/v1/chat/attachments/a1/content"');
    expect(out).toContain('download="report.pdf"');
    expect(out).toContain('src="/api/v1/chat/attachments/a2/preview"');
    expect(out).toContain('alt="図.png"');
    expect(out).toContain("添付を読み込み中");
    expect(count(out, "min-h-11")).toBe(3);
  });

  it("chat_messages_attachments_ids_and_format", () => {
    const state = reduce([
      snapshot([message("m1", 1, { attachment_ids: ["a1", "a2"] }), message("m2", 2, { attachment_ids: ["a1"] })]),
    ]);
    expect(attachmentIdsOf(state)).toEqual(["a1", "a2"]);
    expect(formatBytes(10)).toBe("10 B");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
  });
});

describe("chat_messages_follow", () => {
  const items = (keys: string[]) => keys.map((key) => ({ key, kind: "message" as const }));

  it("chat_messages_follow_at_bottom_threshold", () => {
    expect(isAtBottom({ scrollTop: 552, scrollHeight: 1000, clientHeight: 400 })).toBe(true);
    expect(isAtBottom({ scrollTop: 500, scrollHeight: 1000, clientHeight: 400 })).toBe(false);
  });

  it("chat_messages_follow_tracks_new_items_only_at_bottom", () => {
    let state: FollowState = followReducer(initialFollowState(), { type: "timeline", items: items(["m:1"]) }).state;
    const atBottom = followReducer(state, { type: "timeline", items: items(["m:1", "m:2"]) });
    expect(atBottom.scrollToBottom).toBe(true);
    expect(atBottom.state.unread).toBe(0);

    state = followReducer(atBottom.state, { type: "scrolled", atBottom: false }).state;
    const away = followReducer(state, { type: "timeline", items: items(["m:1", "m:2", "m:3", "m:4"]) });
    expect(away.scrollToBottom).toBe(false);
    expect(away.state.unread).toBe(2);
    // 本文が伸びただけ（key 不変）では未読は増えず、位置も動かさない。
    const grown = followReducer(away.state, { type: "timeline", items: items(["m:1", "m:2", "m:3", "m:4"]) });
    expect(grown.scrollToBottom).toBe(false);
    expect(grown.state.unread).toBe(2);

    const jumped = followReducer(grown.state, { type: "jumped" });
    expect(jumped.scrollToBottom).toBe(true);
    expect(jumped.state).toMatchObject({ atBottom: true, unread: 0 });
  });

  it("chat_messages_follow_prepended_history_is_not_unread", () => {
    let state = followReducer(initialFollowState(), { type: "timeline", items: items(["m:5", "m:6"]) }).state;
    state = followReducer(state, { type: "scrolled", atBottom: false }).state;
    const prepended = followReducer(state, { type: "timeline", items: items(["m:3", "m:4", "m:5", "m:6"]) });
    expect(prepended.state.unread).toBe(0);
    expect(prepended.scrollToBottom).toBe(false);
    expect(appendedKeys(["m:5", "m:6"], ["m:3", "m:4", "m:5", "m:6", "m:7"])).toEqual(["m:7"]);
  });

  it("chat_messages_follow_own_send_jumps_to_bottom", () => {
    let state = followReducer(initialFollowState(), { type: "timeline", items: items(["m:1"]) }).state;
    state = followReducer(state, { type: "scrolled", atBottom: false }).state;
    const sent = followReducer(state, {
      type: "timeline",
      items: [...items(["m:1"]), { key: "p:c1", kind: "pending" }],
    });
    expect(sent.scrollToBottom).toBe(true);
    expect(sent.state.atBottom).toBe(true);
  });

  it("chat_messages_follow_prepend_keeps_position", () => {
    expect(scrollTopAfterPrepend({ scrollTop: 10, scrollHeight: 1000, clientHeight: 400 }, 1600)).toBe(610);
  });

  it("chat_messages_follow_jump_button_shows_unread", () => {
    const out = renderToStaticMarkup(<JumpToLatest unread={3} onClick={() => {}} />);
    expect(out).toContain('aria-label="最新へ（未読 3 件）"');
    expect(out).toContain("min-h-11 min-w-11");
    expect(renderToStaticMarkup(<JumpToLatest unread={0} onClick={() => {}} />)).toContain('aria-label="最新へ"');
  });
});

describe("chat_messages_history", () => {
  it("chat_messages_history_fetches_before_oldest_seq", async () => {
    const state = reduce([snapshot([message("m5", 5), message("m6", 6)])]);
    const list = vi.fn(async () => ({ ...page([message("m3", 3), message("m4", 4)], "10"), next_before_seq: 3 }));
    const result = await fetchOlder(state, list, 2);
    expect(list).toHaveBeenCalledWith(T, { before_seq: 5, limit: 2 }, undefined);
    expect(result.hasOlder).toBe(true);
    const next = chatReducer(state, result.action as ChatAction);
    expect(selectTimeline(next).map((i) => i.key)).toEqual(["m:m3", "m:m4", "m:m5", "m:m6"]);
  });

  it("chat_messages_history_stops_at_first_page", async () => {
    const list = vi.fn();
    expect(await fetchOlder(reduce([snapshot([message("m1", 1)])]), list)).toEqual({ action: null, hasOlder: false });
    expect(list).not.toHaveBeenCalled();
    const last = vi.fn(async () => page([message("m2", 2)], "10"));
    expect((await fetchOlder(reduce([snapshot([message("m3", 3)])]), last)).hasOlder).toBe(false);
  });

  it("chat_messages_history_button_when_older_exists", () => {
    const out = renderToStaticMarkup(
      <MessageList state={reduce([snapshot()])} hasOlder onLoadOlder={async () => {}} />,
    );
    expect(out).toContain("以前のメッセージ");
  });
});

describe("chat_messages_live_region", () => {
  it("chat_messages_live_region_reads_settled_replies_only_once", () => {
    const base = reduce([snapshot([message("m1", 1, { role: "assistant", text: "既存" })])]);
    let announce = nextAnnouncement(null, selectTimeline(base));
    expect(announce.text).toBe("");

    const streaming = reduce([delta(11, "m2", 0, "abc"), delta(12, "m2", 3, "def")], base);
    const during = nextAnnouncement(announce, selectTimeline(streaming));
    expect(during).toBe(announce);

    const running = reduce(
      [
        event(13, "message", {
          message: message("m2", 2, { role: "assistant", text: "abcdef", state: "running", run_id: "r1" }),
        }),
      ],
      streaming,
    );
    expect(nextAnnouncement(announce, selectTimeline(running))).toBe(announce);

    const done = reduce(
      [
        event(14, "message", {
          message: message("m2", 2, { role: "assistant", text: "abcdef", state: "completed", run_id: "r1" }),
        }),
      ],
      running,
    );
    announce = nextAnnouncement(announce, selectTimeline(done));
    expect(announce.text).toBe("CoS: abcdef");
    expect(nextAnnouncement(announce, selectTimeline(done))).toBe(announce);
  });

  it("chat_messages_live_region_is_separate_from_the_list", () => {
    const out = renderToStaticMarkup(<MessageList state={reduce([snapshot()])} />);
    expect(count(out, "aria-live")).toBe(1);
    expect(out).toContain('aria-live="polite" aria-atomic="true" class="sr-only"');
    expect(out).toContain('aria-label="会話"');
    expect(out).not.toContain('role="log"');
  });
});
