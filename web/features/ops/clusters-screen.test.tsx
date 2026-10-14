import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { LocalMachineView } from "../../api/generated/types";
import type { ActionResult, ActionTarget } from "../../components/actions/use-action-result";
import {
  LOCAL_SETTINGS_PATH,
  LocalMachineCard,
  localApplyState,
  localSettingsBody,
  localSourceLabel,
  submitLocalSettings,
} from "./clusters-screen";

const local = (over: Partial<LocalMachineView> = {}): LocalMachineView => ({
  id: "local",
  hostname: "celeris-host",
  in_use: 2,
  workspace_root: {
    value: "/local/celeris/data/workspaces",
    source: "settings",
    config_value: "/home/u/.local/celeris/workspaces",
    apply: "new_tasks",
  },
  max_concurrency: { value: 6, source: "config", config_value: 6, apply: "next_tick" },
  scratch_root: "/local/celeris/data/scratch",
  updated_at: "2026-10-14T00:00:00Z",
  ...over,
});

const sender = (results: Record<string, ActionResult> = {}) => ({ run: async () => [], pending: false, results });
const render = (view: LocalMachineView, results: Record<string, ActionResult> = {}, saved?: LocalMachineView) =>
  renderToStaticMarkup(
    <ul>
      <LocalMachineCard local={view} sender={sender(results)} denied={false} saved={saved ?? null} />
    </ul>,
  );

describe("ローカルマシンの行", () => {
  it("実効値と source、設定ファイル値、反映の時期を文字で出す", () => {
    const out = render(local());
    expect(out).toContain("ローカルマシン local");
    expect(out).toContain("local（celeris-host）");
    expect(out).toContain("/local/celeris/data/workspaces（画面で保存した値）");
    expect(out).toContain("設定ファイルの値: /home/u/.local/celeris/workspaces");
    expect(out).toContain("6（設定ファイル）");
    expect(out).toContain("新しい task から反映");
    expect(out).toContain("次の tick から反映");
    expect(out).toContain("ローカルの設定を保存");
    // 保存した上書きを入力の初期値に、無い欄は空欄（placeholder に設定ファイル値）。
    expect(out).toContain('value="/local/celeris/data/workspaces"');
    expect(out).toContain('placeholder="6"');
  });

  it("source の語彙", () => {
    expect(localSourceLabel("cli")).toBe("CLI の --workspace-root");
    expect(localSourceLabel("settings")).toBe("画面で保存した値");
    expect(localSourceLabel("config")).toBe("設定ファイル");
  });
});

describe("保存", () => {
  it("成功: PUT を全置換の本文で送り、変更後の反映の時期を出す", async () => {
    const sent: ActionTarget[] = [];
    const after = local({ max_concurrency: { value: 8, source: "settings", config_value: 6, apply: "next_tick" } });
    const run = async (targets: readonly ActionTarget[]) => {
      sent.push(...targets);
      return [{ id: "local:settings", ok: true, message: "操作が完了しました", response: after }];
    };
    const out = await submitLocalSettings(run, " /local/celeris/data/workspaces ", "8");
    expect(sent).toEqual([
      {
        id: "local:settings",
        path: LOCAL_SETTINGS_PATH,
        method: "PUT",
        body: { workspace_root: "/local/celeris/data/workspaces", max_concurrency: 8 },
      },
    ]);
    expect(out).toEqual({ ok: true, view: after });
    const html = render(local(), {}, after);
    expect(html).toContain('data-testid="local-settings-applied"');
    expect(html).toContain("保存しました。");
    expect(html).toMatch(/作業ディレクトリ: <span[^>]*>新しい task から反映/);
    expect(html).toMatch(/同時実行数: <span[^>]*>次の tick から反映/);
  });

  it("空欄は上書きを消す（null）", () => {
    expect(localSettingsBody("", " ")).toEqual({ body: { workspace_root: null, max_concurrency: null } });
  });

  it("検証エラー: 範囲外の同時実行数は送らず、サーバの 422 は文言を出す", async () => {
    let called = false;
    const run = async () => {
      called = true;
      return [];
    };
    for (const bad of ["0", "257", "1.5", "abc"]) {
      const out = await submitLocalSettings(run, "", bad);
      expect(out).toEqual({ ok: false, error: "同時実行数は 1〜256 の整数で入力してください。" });
    }
    expect(called).toBe(false);
    const rejected = {
      id: "local:settings",
      ok: false,
      status: 422,
      message: "workspace_root must not be inside the releases dir",
    };
    expect(await submitLocalSettings(async () => [rejected], "/home/u/.local/celeris/releases/x", "")).toEqual({
      ok: false,
    });
    const html = render(local(), { "local:settings": rejected });
    expect(html).toContain('role="alert"');
    expect(html).toContain("workspace_root must not be inside the releases dir");
  });

  it("restart_required: CLI の上書きがある欄は再起動が必要と出す", () => {
    expect(localApplyState({ apply: "restart_required", source: "settings" })).toEqual({
      tone: "warning",
      label: "daemon の再起動が必要",
    });
    expect(localApplyState({ apply: "new_tasks", source: "cli" }).label).toBe("daemon の再起動が必要");
    expect(localApplyState({ apply: "new_tasks", source: "config" }).tone).toBe("info");
    const cli = local({
      workspace_root: {
        value: "/scratch/staging",
        source: "cli",
        config_value: "/home/u/.local/celeris/workspaces",
        apply: "new_tasks",
      },
    });
    const html = render(cli, {}, cli);
    expect(html).toContain("/scratch/staging（CLI の --workspace-root）");
    expect(html).toContain("daemon の再起動が必要");
    expect(html).toContain("CLI の上書きを外して daemon");
    // CLI の値は store の上書きではないので入力の初期値にしない。
    expect(html).not.toContain('value="/scratch/staging"');
  });
});
