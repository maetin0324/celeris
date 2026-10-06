import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AccountView } from "../../api/generated/types";
import {
  AccountCard,
  ADAPTER_CHOICES,
  accountState,
  excludedReasonLabel,
  formatRemaining,
  usageTone,
} from "./accounts-screen";

const stats = { done: 1, error: 0, input_tokens: 0, output_tokens: 0, runs: 1 };
const account = (over: Partial<AccountView>): AccountView => ({
  id: "main",
  adapter: "claude-code",
  dir: "/accounts/main",
  in_use: 0,
  logged_in: true,
  login_pending: false,
  stats,
  ...over,
});

describe("accountState", () => {
  it("接続・期限切れ・失敗・休止・除外を文字で返す", () => {
    expect(accountState(account({})).label).toBe("ログイン済み");
    expect(accountState(account({ logged_in: false })).label).toBe("未ログイン");
    expect(accountState(account({ logged_in: false, login_pending: true })).label).toBe("ログイン待ち");
    expect(
      accountState(account({ last_check: { at: "2026-10-04T00:00:00Z", result: "auth_failed", detail: "401" } })),
    ).toMatchObject({ tone: "danger", label: "認証失敗", detail: "401" });
    expect(accountState(account({ cooldown: { reason: "rate limit", until: "2026-10-04T01:00:00Z" } }))).toMatchObject({
      tone: "warning",
      label: "休止中",
    });
    expect(accountState(account({ excluded_reason: "手動で除外" })).label).toBe("除外中");
  });
});

describe("AccountCard", () => {
  const sender = { run: async () => [], pending: false, results: {} };
  const render = (blocked: boolean) =>
    renderToStaticMarkup(
      <ul>
        <AccountCard
          item={account({})}
          max={2}
          fetchedAtMs={0}
          sender={sender}
          blocked={blocked}
          deniedId={blocked ? "denied" : undefined}
          login={undefined}
          setLogin={() => {}}
        />
      </ul>,
    );

  it("状態は文字の badge で、認証情報は有無だけを出す", () => {
    const out = render(false);
    expect(out).toContain('data-slot="badge"');
    expect(out).toContain("ログイン済み");
    expect(out).toContain("保存あり（値は表示しません）");
    expect(out).not.toContain('disabled=""');
  });

  it("権限が無いときは操作を止め、理由へ結び付ける", () => {
    const out = render(true);
    expect(out.match(/disabled=""/g)?.length).toBe(3);
    expect(out.match(/aria-describedby="denied"/g)?.length).toBe(3);
  });
});

describe("使用量（5 時間枠・7 日枠）", () => {
  const sender = { run: async () => [], pending: false, results: {} };
  // 取得時刻を固定し、残り時間はそこからの差で決まることを確かめる。
  const fetchedAt = Date.parse("2026-10-06T00:00:00Z");
  const iso = (ms: number) => new Date(fetchedAt + ms).toISOString();
  const usage = (five: number | null, seven: number | null) => ({
    source: "statusline",
    status: "allowed",
    observed_at: iso(-5 * 60_000),
    five_hour: five === null ? null : { utilization: five, resets_at: iso(2 * 3_600_000 + 15 * 60_000) },
    seven_day: seven === null ? null : { utilization: seven, resets_at: iso(3 * 86_400_000 + 4 * 3_600_000) },
  });
  const render = (item: AccountView) =>
    renderToStaticMarkup(
      <ul>
        <AccountCard
          item={item}
          max={2}
          fetchedAtMs={fetchedAt}
          sender={sender}
          blocked={false}
          deniedId={undefined}
          login={undefined}
          setLogin={() => {}}
        />
      </ul>,
    );
  const meters = (out: string) => [...out.matchAll(/<div role="meter"[^>]*>/g)].map((m) => m[0]);

  it("使用率 0%: 2 本の meter が値と残り時間を持ち、通常の色", () => {
    const out = render(account({ score: 1, usage: usage(0, 0) }));
    const [five, seven] = meters(out);
    expect(five).toContain('aria-label="短期枠（5時間）"');
    expect(five).toContain('aria-valuenow="0"');
    expect(five).toContain('aria-valuemin="0"');
    expect(five).toContain('aria-valuemax="100"');
    expect(five).toContain('aria-valuetext="使用 0% / 残り 100%・リセットまで 2時間15分"');
    expect(seven).toContain('aria-label="長期枠（7日）"');
    expect(seven).toContain('aria-valuetext="使用 0% / 残り 100%・リセットまで 3日4時間"');
    expect(out).toContain("リセットまで 2時間15分");
    expect(out.match(/data-usage-tone="normal"/g)?.length).toBe(2);
    expect(out).toContain("bg-primary");
    expect(out).toContain('style="width:0%"');
    expect(out).toContain("1.00");
  });

  it("中間（45%・75%）: 5h は通常、7d は注意の色と語", () => {
    const out = render(account({ score: 0.42, usage: usage(0.45, 0.75) }));
    expect(out).toContain("使用 45% / 残り 55%");
    expect(out).toContain('style="width:45%"');
    expect(out).toContain('data-usage-tone="warning"');
    expect(out).toContain("bg-warning-foreground");
    expect(out).toContain('aria-valuetext="使用 75% / 残り 25%・注意・リセットまで 3日4時間"');
    expect(out).toContain("（注意）");
    expect(out).toContain("0.42");
    // 観測時刻は source・status と相対時刻付き
    expect(out).toContain("statusline・allowed");
    expect(out).toContain("5 分前");
  });

  it("100% と five_hour_exhausted の除外: 上限間近の色と語、除外理由を日本語で出す", () => {
    const out = render(account({ excluded_reason: "five_hour_exhausted", usage: usage(1, 0.93) }));
    expect(meters(out)[0]).toContain('aria-valuenow="100"');
    expect(out).toContain("使用 100% / 残り 0%");
    expect(out).toContain('style="width:100%"');
    expect(out.match(/data-usage-tone="danger"/g)?.length).toBe(2);
    expect(out).toContain("bg-danger-foreground");
    expect(out).toContain("（上限間近）");
    expect(out).toContain("除外中");
    expect(out).toContain("除外: 短期枠を使い切りました（five_hour_exhausted）");
  });

  it("窓なし・usage なし: meter を出さず「-」", () => {
    const none = render(account({ score: 0.9, usage: { ...usage(null, null) } }));
    expect(meters(none)).toHaveLength(0);
    expect(none.match(/data-usage-window="none"/g)?.length).toBe(3);
    expect(none).toContain("不明");
    expect(none).not.toContain("0%");
    const absent = render(account({}));
    expect(meters(absent)).toHaveLength(0);
    expect(absent.match(/data-usage-window="none"/g)?.length).toBe(3);
  });

  it("opencode-go: 3 本の meter が割合とリセット時刻を持つ", () => {
    const out = render(
      account({
        adapter: "opencode-go",
        usage: {
          source: "opencode-go",
          status: "allowed",
          observed_at: iso(-60_000),
          five_hour: { utilization: 0.1, resets_at: iso(2 * 3_600_000) },
          seven_day: { utilization: 0.5, resets_at: iso(3 * 86_400_000) },
          one_month: { utilization: 0.95, resets_at: iso(20 * 86_400_000) },
        },
      }),
    );
    const [five, seven, month] = meters(out);
    expect(meters(out)).toHaveLength(3);
    expect(five).toContain('aria-valuenow="10"');
    expect(seven).toContain('aria-valuenow="50"');
    expect(month).toContain('aria-label="月間枠（1か月）"');
    expect(month).toContain('aria-valuetext="使用 95% / 残り 5%・上限間近・リセットまで 20日"');
    expect(out).toContain("リセットまで 2時間");
    expect(out).toContain("リセットまで 20日");
    expect(out).toContain("OpenCode Go");
    expect(out).toContain("opencode-go・allowed");
  });

  it("claude: one_month が null の 3 本目は「不明」で 0% にしない", () => {
    const out = render(account({ usage: { ...usage(0.2, 0.3), one_month: null } }));
    expect(meters(out)).toHaveLength(2);
    expect(out.match(/data-usage-window="none"/g)?.length).toBe(1);
    expect(out).toContain("月間枠（1か月）");
    expect(out).toContain("不明");
  });

  it("道具の選択肢と 1 か月枠の除外理由", () => {
    expect(ADAPTER_CHOICES).toContain("opencode-go");
    expect(excludedReasonLabel("one_month_exhausted")).toBe("1 か月の枠切れ（one_month_exhausted）");
  });

  it("段階・残り時間・除外理由の変換", () => {
    expect([0, 0.69, 0.7, 0.89, 0.9, 1].map(usageTone)).toEqual([
      "normal",
      "normal",
      "warning",
      "warning",
      "danger",
      "danger",
    ]);
    expect(formatRemaining(30_000)).toBe("1分未満");
    expect(formatRemaining(45 * 60_000)).toBe("45分");
    expect(formatRemaining(26 * 3_600_000 + 5 * 60_000)).toBe("1日2時間");
    expect(formatRemaining(-1)).toBe("リセット時刻を過ぎました");
    expect(excludedReasonLabel("seven_day_exhausted")).toBe("長期枠を使い切りました（seven_day_exhausted）");
    expect(excludedReasonLabel("手動で除外")).toBe("手動で除外");
  });
});
