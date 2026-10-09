import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AccountView } from "../../api/generated/types";
import type { ActionResult, ActionTarget } from "../../components/actions/use-action-result";
import {
  AccountCard,
  ADAPTER_CHOICES,
  accountLimitTarget,
  accountState,
  excludedReasonLabel,
  formatRemaining,
  limitSourceLabel,
  parseMaxRunsInput,
  submitAccountLimit,
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
  effective_max_runs: 2,
  max_runs_source: "default",
  implicit: false,
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
          item={account({ max_runs: 3, effective_max_runs: 3, max_runs_source: "account" })}
          defaultMax={2}
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

  it("権限が無いときは操作を止め、理由へ結び付ける（確認・ログイン・削除・上限の保存・既定に戻す）", () => {
    const out = render(true);
    expect(out.match(/disabled=""/g)?.length).toBe(5);
    expect(out.match(/aria-describedby="denied"/g)?.length).toBe(5);
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
          defaultMax={2}
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

type Run = (targets: readonly ActionTarget[]) => Promise<ActionResult[]>;

/** 送った要求を記録し、決めた結果を返す run。 */
function fakeRun(result: Omit<ActionResult, "id"> | null) {
  const sent: ActionTarget[] = [];
  const run: Run = async (targets) => {
    sent.push(...targets);
    return result ? targets.map((t) => ({ id: t.id, ...result })) : [];
  };
  return { run, sent };
}

const implicitAccount = account({
  adapter: "implicit",
  id: "opencode-qwen",
  dir: null,
  implicit: true,
  provider: "opencode-qwen",
  in_use: 1,
  max_runs: null,
  effective_max_runs: 1,
  max_runs_source: "default",
});

describe("アカウントごとの上限（PATCH /accounts/{adapter}/{id}）", () => {
  const renderCard = (item: AccountView, results: Record<string, ActionResult> = {}) =>
    renderToStaticMarkup(
      <ul>
        <AccountCard
          item={item}
          defaultMax={2}
          fetchedAtMs={0}
          sender={{ run: async () => [], pending: false, results }}
          blocked={false}
          deniedId={undefined}
          login={undefined}
          setLogin={() => {}}
        />
      </ul>,
    );

  it("入力の検査: 1〜32 の整数だけを通す", () => {
    expect(parseMaxRunsInput("1")).toEqual({ value: 1 });
    expect(parseMaxRunsInput(" 32 ")).toEqual({ value: 32 });
    for (const bad of ["", "0", "33", "-1", "1.5", "abc", "1e1"]) {
      expect(parseMaxRunsInput(bad)).toEqual({ error: "上限は 1〜32 の整数で入力してください。" });
    }
  });

  it("保存: 数値を PATCH /api/accounts/{adapter}/{id} に送る", async () => {
    const { run, sent } = fakeRun({ ok: true, message: "操作が完了しました" });
    expect(await submitAccountLimit(run, account({}), "3")).toEqual({ ok: true });
    expect(sent).toEqual([
      { id: "limit:claude-code:main", path: "/api/accounts/claude-code/main", method: "PATCH", body: { max_runs: 3 } },
    ]);
  });

  it("adapter と id はパスの各部分としてエンコードする", () => {
    expect(accountLimitTarget(account({ adapter: "custom/adapter", id: "local source?#" }), 2)).toMatchObject({
      path: "/api/accounts/custom%2Fadapter/local%20source%3F%23",
      method: "PATCH",
      body: { max_runs: 2 },
    });
  });

  it("既定に戻す: max_runs に null を送る", async () => {
    const { run, sent } = fakeRun({ ok: true, message: "操作が完了しました" });
    const item = account({ adapter: "codex", id: "b", max_runs: 4, effective_max_runs: 4, max_runs_source: "account" });
    expect(await submitAccountLimit(run, item, null)).toEqual({ ok: true });
    expect(sent[0]).toMatchObject({ path: "/api/accounts/codex/b", method: "PATCH", body: { max_runs: null } });
  });

  it("検査に落ちた値は送らずに文言を返す", async () => {
    const { run, sent } = fakeRun({ ok: true, message: "" });
    expect(await submitAccountLimit(run, account({}), "0")).toEqual({
      ok: false,
      error: "上限は 1〜32 の整数で入力してください。",
    });
    expect(await submitAccountLimit(run, account({}), "33")).toMatchObject({ ok: false });
    expect(sent).toHaveLength(0);
  });

  it("API のエラーは項目の文言にし、403 は権限の案内に任せる", async () => {
    const failed = fakeRun({ ok: false, status: 404, message: "account not found" });
    expect(await submitAccountLimit(failed.run, account({}), "3")).toEqual({
      ok: false,
      error: "保存できませんでした: account not found",
    });
    const denied = fakeRun({ ok: false, status: 403, message: "forbidden" });
    expect(await submitAccountLimit(denied.run, account({}), "3")).toEqual({ ok: false });
  });

  it("画面: 実行中 / 実効上限・上書きの有無・入力欄・保存と既定に戻す", () => {
    const out = renderCard(account({ in_use: 1, max_runs: 3, effective_max_runs: 3, max_runs_source: "account" }));
    expect(out).toContain("1 / 実効上限 3（このアカウントに保存した上限）");
    expect(out).toContain('aria-label="上限 main"');
    expect(out).toContain("同時実行数の上限（1〜32）");
    expect(out).toMatch(/<input[^>]*type="number"[^>]*min="1"[^>]*max="32"[^>]*value="3"/);
    expect(out).toContain("上限を保存");
    expect(out).toContain("「既定に戻す」で保存値を消します。");
    expect(out).toMatch(/<button[^>]*>既定に戻す<\/button>/);
    expect(out).not.toMatch(/<button[^>]*disabled=""[^>]*>既定に戻す/);
  });

  it("画面: 上書きが無いときは既定を示し、既定に戻すは押せない", () => {
    const out = renderCard(account({}));
    expect(out).toContain("0 / 実効上限 2（既定（max_runs_per_account = 2））");
    expect(out).toContain("今は上書きしていません。");
    expect(out).toMatch(/<button[^>]*disabled=""[^>]*>既定に戻す<\/button>/);
  });

  it("画面: 保存の失敗（API エラー）を入力欄に結び付けて出す", () => {
    const out = renderCard(account({}), {
      "limit:claude-code:main": { id: "limit:claude-code:main", ok: false, status: 400, message: "bad max_runs" },
    });
    expect(out).toContain("保存できませんでした: bad max_runs");
    expect(out).toMatch(/<input[^>]*aria-invalid="true"/);
  });

  it("暗黙アカウント: 印を出し、ログイン・確認・削除を出さず、同じ欄で上限を編集する", async () => {
    const out = renderCard(implicitAccount);
    expect(out).toContain('data-implicit="true"');
    expect(out).toContain(">暗黙アカウント</span>");
    expect(out).toContain("アカウントを持たない実行枠 opencode-qwen");
    expect(out).toContain("1 / 実効上限 1（既定（max_runs_per_account = 2））");
    expect(out).toContain("上限を保存");
    expect(out).not.toContain("ログイン開始");
    expect(out).not.toContain(">確認</button>");
    expect(out).not.toContain(">削除</button>");
    expect(out).not.toContain("使用量");
    expect(accountLimitTarget(implicitAccount, 2)).toEqual({
      id: "limit:implicit:opencode-qwen",
      path: "/api/accounts/implicit/opencode-qwen",
      method: "PATCH",
      body: { max_runs: 2 },
    });
    const { run, sent } = fakeRun({ ok: true, message: "操作が完了しました" });
    expect(await submitAccountLimit(run, implicitAccount, "2")).toEqual({ ok: true });
    expect(await submitAccountLimit(run, implicitAccount, null)).toEqual({ ok: true });
    expect(sent.map((t) => [t.path, t.body])).toEqual([
      ["/api/accounts/implicit/opencode-qwen", { max_runs: 2 }],
      ["/api/accounts/implicit/opencode-qwen", { max_runs: null }],
    ]);
  });

  it("暗黙アカウント: 保存値があるときは既定に戻せる", () => {
    const out = renderCard({ ...implicitAccount, max_runs: 4, effective_max_runs: 4, max_runs_source: "account" });
    expect(out).toContain("1 / 実効上限 4（このアカウントに保存した上限）");
    expect(out).toMatch(/<input[^>]*type="number"[^>]*value="4"/);
    expect(out).toContain("「既定に戻す」で保存値を消します。");
    expect(out).not.toMatch(/<button[^>]*disabled=""[^>]*>既定に戻す/);
  });

  it("実効上限の出どころの文言", () => {
    expect(limitSourceLabel({ max_runs_source: "account" }, 2)).toBe("このアカウントに保存した上限");
    expect(limitSourceLabel({ max_runs_source: "default" }, 5)).toBe("既定（max_runs_per_account = 5）");
    expect(limitSourceLabel({ max_runs_source: "other" }, 2)).toBe("other");
  });
});
