import { describe, expect, it } from "vitest";
import { notificationBody, shouldNotify } from "./notification-rule";

describe("notification-rule", () => {
  const view = { unread: 2, events: 3, by_kind: { bad_news: 1 } };
  it("未読の出来事が前に見た値より増えたときだけ知らせる", () => {
    expect(shouldNotify(view, null)).toBe(true);
    expect(shouldNotify(view, 3)).toBe(false);
    expect(shouldNotify(view, 2)).toBe(true);
    expect(shouldNotify({ unread: 0, events: 0, by_kind: {} }, null)).toBe(false);
    expect(shouldNotify(null, null)).toBe(false);
  });
  it("本文は件数だけで、悪い知らせを先に出す", () => {
    expect(notificationBody(view)).toBe("悪い知らせ 1 件 / 未読の通知 2 件");
    expect(notificationBody({ unread: 1, events: 1, by_kind: {} })).toBe("未読の通知 1 件");
  });
});
