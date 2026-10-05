import { describe, expect, it } from "vitest";
import type { Notice } from "../../api/generated/types";
import { isNoticeKind, noticeKindView, noticeLinks } from "./notice-view";

const base: Notice = {
  id: "N1",
  kind: "task_done",
  group_key: "task_done:N1",
  title: "完了",
  summary: "",
  count: 1,
  first_at: "2026-10-01T00:00:00Z",
  last_at: "2026-10-01T00:00:00Z",
};

describe("notice-view", () => {
  it("種類は生成型の値だけを受ける", () => {
    expect(isNoticeKind("bad_news")).toBe(true);
    expect(isNoticeKind("inbox_new")).toBe(false);
    expect(isNoticeKind(undefined)).toBe(false);
    expect(noticeKindView("bad_news").tone).toBe("danger");
  });

  it("links を先に、target と task_id を後に、同じ href は 1 つにまとめる", () => {
    const links = noticeLinks({
      ...base,
      links: [{ label: "タスク", href: "/tasks/T1" }],
      target: { kind: "task", id: "T1" },
      task_id: "T1",
    });
    expect(links).toEqual([{ href: "/tasks/T1", label: "タスク" }]);
  });

  it("報告の target は /reports?report=<id>、外部や protocol-relative の href は捨てる", () => {
    const links = noticeLinks({
      ...base,
      kind: "report",
      links: [
        { label: "外", href: "https://example.invalid/" },
        { label: "外2", href: "//example.invalid/" },
      ],
      target: { kind: "report", id: "R 1" },
    });
    expect(links).toEqual([{ href: "/reports?report=R%201", label: "報告の本文" }]);
    expect(noticeLinks({ ...base, kind: "report" })).toEqual([{ href: "/reports", label: "報告の一覧" }]);
  });
});
