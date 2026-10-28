import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { message, thread } from "../data/fixtures.test-support";
import { EscalationSourceLink, ThreadBar } from "./chat-home";

// ADR 2026-10-28-single-human-window-general-cos D5: the home is the general CoS; a project
// thread is for reading and offers the way back; a card links to its project thread.
describe("chat_home_single_window", () => {
  it("chat_home_general_thread_has_no_thread_list_or_back_link", () => {
    const html = renderToStaticMarkup(
      <ThreadBar thread={thread({ kind: "general", title: "統括 CoS" })} onBackToGeneral={() => {}} />,
    );
    expect(html).toContain("統括 CoS");
    expect(html).not.toContain("統括 CoS に戻る");
    expect(html).not.toContain("会話一覧");
  });

  it("chat_home_project_thread_is_read_only_guidance_with_back_to_general", () => {
    const html = renderToStaticMarkup(
      <ThreadBar
        thread={thread({ kind: "project", project_id: "A", title: "CoS: 案件A" })}
        onBackToGeneral={() => {}}
      />,
    );
    expect(html).toContain('data-thread-bar="project"');
    expect(html).toContain("CoS: 案件A");
    expect(html).toContain("読む用です");
    expect(html).toContain("統括 CoS に戻る");
  });

  it("chat_home_escalation_card_links_to_the_project_thread_and_shows_resolution", () => {
    const escalation = {
      item_id: "i1",
      source_kind: "decision",
      source_key: "decision-d1",
      source_thread_id: "pt 1",
      project_id: "A",
      task_id: "t1",
      state: "escalated",
      open: true,
    };
    const open = renderToStaticMarkup(<EscalationSourceLink message={message("m1", 1, { escalation })} />);
    expect(open).toContain('href="/?thread=pt%201"');
    expect(open).toContain("元のスレッドを開く");
    expect(open).toContain('data-escalation-state="open"');
    const closed = renderToStaticMarkup(
      <EscalationSourceLink
        message={message("m1", 1, { escalation: { ...escalation, state: "resolved", open: false } })}
      />,
    );
    expect(closed).toContain('data-escalation-state="resolved"');
    expect(closed).toContain("解消しました");
    expect(renderToStaticMarkup(<EscalationSourceLink message={message("m2", 2)} />)).toBe("");
  });
});
