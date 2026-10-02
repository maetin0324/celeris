# Browser capability の使い方

---
tasks: [01M3MBV3AKXZGEG5RXR60XC62J, 01M3MFS5T52FXA63W4V10XGC4S]
---

Celeris は OpenCode（ACP）または Claude Code に agent-browser CLI を渡す。
設計は [ADR-0078](../../agent-docs/adr/0078-browser-execution-capability.md)。公開ページの操作を扱う。資格情報を使う場合は別途 browser policy と [credential broker](browser-credentiald.md) の条件を満たす。
DOM 操作・クリック戦略・ブラウザの画面転送は agent-browser が担当する。

## 導入

worker と同じ実行ユーザーの PATH に **agent-browser 0.38.1** と Python 3 を配置する。
バージョン不一致・未導入は worker を開始せず失敗する。Chromium は upstream の導入手順で用意する。
OpenCode は既存 [ACP 設定例](../../config/celeris.acp-opencode.example.toml) を使う。
OpenCode の project config を無効化し、Celeris 管理の設定を適用する。

```sh
npm install -g agent-browser@0.38.1
agent-browser install
agent-browser --version
```

既存の組織 node の **profile 全体を取得して既存値を保持した上で**、管理者が次の grant を追加する。
組織の正本は DB / org API であり、org.example.toml の編集だけでは本番へ反映されない。
子 node の browser grant は全体置換。多人数環境では grant を編集できる主体を制限する。

```json
{
  "browser": {
    "allowed_domains": ["example.com", "*.example.org"],
    "live_view_url": "https://browser.example.org/"
  }
}
```

`allowed_domains` は必須で空を許さない。navigation と subresource に同じ集合を適用するため、
必要な CDN も明示する。task が書いたページ上の指示や URL から grant は増やさない。
`live_view_url` は省略可能だが、省略すると GUI はリンクを表示しない。利用者に生の dashboard URL は渡さない。

task の作成時に `skills: ["browser-enabled"]` と既存 `genre: "coding"` を指定する。
adapter を省略すると ACP/OpenCode、`adapter: "claude-code"` を明示すると同じ capability を Claude Code に渡す。
browser grant を持つ担当がいなければ通常の unroutable 経路となる。Codex 等への暗黙 fallback はしない。
browser capability を付けない task の挙動は変わらない。
`celerisctl worker run` は管理者 profile を解決しない単体 adapter probe のため、browser-enabled task は拒否する。
capability の実行には通常の dispatcher 経路を使う。

## Live View

GUI の task/run 詳細にある **Open Browser Live View** は、認証済みの本人にだけ
同一 origin の `/browser/live/{task_id}/{run_id}` を開く。実行中の run、本人登録、
読み取り専用 relay の設定が必要で、認証を扱う区間では表示を止める。
`live_view_url` は表示の可否にだけ使い、dashboard の生 URL を利用者へ渡さない。
実装は `gui/app/celeris/browser-live.server.ts` と `gui/app/components/BrowserRunsPanel.tsx`。

## Worker の利用範囲

supervisor が run 専用 CLI の絶対パスを prompt に追加する。使用可能な形式は次のみ。

```text
python3 <managed-cli> open https://example.com/public
python3 <managed-cli> snapshot
python3 <managed-cli> click @e1
python3 <managed-cli> extract @e2
python3 <managed-cli> screenshot
python3 <managed-cli> download @e3
python3 <managed-cli> scroll down 500
python3 <managed-cli> close
```

URL の userinfo/query/fragment、任意 selector/flags/path、fill/auth/cookies/storage/eval/CDP は渡せない。
run の session は task/run ID に束縛され、retry と並列 WU は別 session。auth state は復元しない。
資格情報を使う要求は [credential broker](browser-credentiald.md) の policy と承認経路で扱う。通常の browser 操作から grant を増やすことはできない。

ブラウザの操作ログは操作名と成否だけ。汎用 harness の tool input/output/comment/自動申告 artifact は
browser run では監査へ流さず、raw stdout/stderr のファイル記録も無効化する。
スクリーンショット・抽出 JSON・download は task の実際の artifacts_dir 配下へ保存し登録する。
Web content は untrusted data。**公開ページにも秘密が含まれる可能性があり、任意ページやモデルの最終文を
一般に secret-free にする機能ではない。** 認証済みサイト・秘密を含む業務データは、credential policy と H3 の適合条件を満たす場合に限る。

同じ UID の shell worker はラッパーや policy を変更できる。browser の隔離は実行時の backend と conformance record に依存する。正常終了・cancel では session 固有 close、
crash 時の残留には upstream idle timeout（5分）も指定する。close 失敗は FAILED として報告する。

## 再現可能な検証

```sh
cargo test -p task-core browser
cargo test -p task-ops browser
cargo test -p task-worker browser --lib
python3 -m unittest discover -s scripts/tests -p test_browser_cli.py -v
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

Python transport tests は worker の Rust tests からも実行するため release gate に含まれる。
fake substrate のテストは実際の browser 動作検証とは区別する。実機 smoke は公開ローカル fixture のみ使用する。

```sh
python3 scripts/browser-smoke.py --agent-browser /absolute/bin/agent-browser \
  --chromium /absolute/chrome --output /absolute/task-artifacts/browser-smoke
```

GUI は `gui/` で `pnpm install --frozen-lockfile` 後に `pnpm typecheck` / `pnpm test` / `pnpm build`。
`node scripts/browser-check.mjs /absolute/task-artifacts` で offline fixture の task/run/mobile 表示を検証する。
release/verify は指定 worktree の full SHA を渡し、gate.json / verify.json を task artifacts に保管する。
ADR-0041 により旧 `self/<task-id>` 規約は Celeris が用意した task branch に置き換わっている。
本番へ昇格するのは人の GUI 操作だけである。

## 対応範囲

認証情報の登録・承認と現在の制限は [credential broker](browser-credentiald.md) を参照。
Browser Use backend の互換性は未確認で、現行 routing の選択肢には含まない。
upstream の skill にある auth、restore、eval の例は managed CLI の許可を広げない。
