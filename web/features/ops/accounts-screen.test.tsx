import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AccountView } from "../../api/generated/types";
import { AccountCard, accountState } from "./accounts-screen";

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
