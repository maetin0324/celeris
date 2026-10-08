// ADR 2026-10-08-cos-workspace-files-in-chat D3: CoS の返事の workspace path を添付の link にし、
// 添付の md・text を画面内で開ける。
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ChatAttachment } from "../../../api/generated/types";
import { chatPaths } from "../data/client";
import { message, T } from "../data/fixtures.test-support";
import { MessageItem } from "./message-item";
import { AttachmentList, CHAT_TEXT_PREVIEW_MAX_BYTES, chatPreviewKind } from "./parts";
import { linkWorkspacePaths, workspaceAttachmentFor } from "./workspace-links";

const ABS = "/local/celeris/data/db/cos/threads/T1/workspace";
const files = [
  { path: "artifacts/manaba-monitor-setup.md", attachment_id: "a1" },
  { path: "table.csv", attachment_id: "a2" },
];
const href = chatPaths.attachmentContent;

function attachment(over: Partial<ChatAttachment> = {}): ChatAttachment {
  return {
    id: "a1",
    thread_id: T,
    name: "manaba-monitor-setup.md",
    media_type: "application/octet-stream",
    size_bytes: 120,
    sha256: "x",
    state: "ready",
    download_url: "/api/v1/chat/attachments/a1/content",
    preview_url: null,
    ...over,
  };
}

describe("chat_workspace_links", () => {
  it("chat_workspace_links_absolute_relative_and_code_span_paths_become_links", () => {
    const text = [
      `結論: \`${ABS}/artifacts/manaba-monitor-setup.md\` を開いてください。`,
      "表は table.csv、手順は ./artifacts/manaba-monitor-setup.mdに。",
    ].join("\n");
    const out = linkWorkspacePaths(text, files, href);
    expect(out).toContain(`[${ABS}/artifacts/manaba-monitor-setup.md](${href("a1")})`);
    expect(out).toContain(`[table.csv](${href("a2")})`);
    expect(out).toContain(`[./artifacts/manaba-monitor-setup.md](${href("a1")})に`);
    expect(out.match(/\]\(/g)).toHaveLength(3);
  });

  it("chat_workspace_links_leave_other_names_code_fences_and_links_alone", () => {
    const text = [
      "old-table.csv と /tmp/x/table.csv と table.csv.bak は別物。",
      "`table.csv を読む` は文の一部。",
      "[表](https://example.invalid/table.csv)",
      "```",
      "cat table.csv",
      "```",
    ].join("\n");
    expect(linkWorkspacePaths(text, files, href)).toBe(text);
    expect(linkWorkspacePaths("table.csv", [], href)).toBe("table.csv");
    expect(linkWorkspacePaths("see table.csv.", files, href)).toBe(`see [table.csv](${href("a2")}).`);
  });

  it("chat_workspace_links_retarget_markdown_links_to_workspace_files", () => {
    expect(linkWorkspacePaths(`[手順](${ABS}/artifacts/manaba-monitor-setup.md)`, files, href)).toBe(
      `[手順](${href("a1")})`,
    );
    expect(linkWorkspacePaths("[表](./table.csv)", files, href)).toBe(`[表](${href("a2")})`);
    expect(linkWorkspacePaths("[表](https://example.invalid/table.csv)", files, href)).toBe(
      "[表](https://example.invalid/table.csv)",
    );
  });

  it("chat_workspace_links_href_maps_back_to_attachment", () => {
    expect(workspaceAttachmentFor(href("a2"), files, href)).toBe("a2");
    expect(workspaceAttachmentFor("/api/chat/attachments/zz/content", files, href)).toBeUndefined();
    expect(workspaceAttachmentFor(href("a2"), undefined, href)).toBeUndefined();
  });

  it("chat_workspace_links_reply_renders_link_and_attachment_with_preview_toggle", () => {
    const out = renderToStaticMarkup(
      <MessageItem
        message={message("m2", 2, {
          role: "assistant",
          text: `結論: \`${ABS}/artifacts/manaba-monitor-setup.md\` を見てください。`,
          attachment_ids: ["a1"],
          workspace_files: [files[0] as (typeof files)[number]],
        })}
        streaming={false}
        attachments={{ a1: attachment() }}
      />,
    );
    expect(out).toContain(`href="${href("a1")}"`);
    expect(out).toContain(`>${ABS}/artifacts/manaba-monitor-setup.md</a>`);
    expect(out).toContain('id="chat-attachment-a1"');
    expect(out).toContain('data-preview-kind="markdown"');
    expect(out).toContain("本文をここで見る");
  });
});

describe("chat_attachment_preview", () => {
  it("chat_attachment_preview_kinds_follow_name_and_size", () => {
    expect(chatPreviewKind(attachment())).toBe("markdown");
    expect(chatPreviewKind(attachment({ name: "table.csv" }))).toBe("text");
    expect(chatPreviewKind(attachment({ name: "big.log", size_bytes: CHAT_TEXT_PREVIEW_MAX_BYTES + 1 }))).toBe(
      undefined,
    );
    for (const name of ["page.html", "figure.svg", "paper.pdf", "data.bin", "shot.png"])
      expect(chatPreviewKind(attachment({ name }))).toBe(undefined);
    expect(chatPreviewKind(attachment({ state: "deleted" }))).toBe(undefined);
  });

  it("chat_attachment_preview_open_md_and_text_use_artifact_viewers", () => {
    const out = renderToStaticMarkup(
      <AttachmentList
        ids={["a1", "a2", "a3"]}
        attachments={{
          a1: attachment(),
          a2: attachment({ id: "a2", name: "table.csv" }),
          a3: attachment({ id: "a3", name: "paper.pdf", media_type: "application/pdf" }),
        }}
        open={new Set(["a1", "a2", "a3"])}
        onToggle={() => {}}
      />,
    );
    expect(out).toContain('aria-expanded="true"');
    expect(out).toContain("本文を閉じる");
    expect(out).toContain('id="chat-attachment-a1-body"');
    // text viewer（行番号・折り返し）。pdf は開閉も本文も無く download だけ。
    expect(out).toContain('data-artifact-viewer="text"');
    expect(out).not.toContain('id="chat-attachment-a3-body"');
    expect(out).toContain('download="paper.pdf"');
  });
});
