---
title: 旧 gui: concurrency 入力欄を外す最小変更
tasks: [01M4D7RVKXY9A967ZXDADWVHQJ]
status: done
updated: 2026-10-08
---
# 旧 gui: concurrency 入力欄を外す（完了 2026-10-08 15:22 UTC）

## 変更
- providers.tsx: 追加フォームの concurrency 入力、カードの concurrency 表示、編集フォームの concurrency 入力を削除（in_use 表示は残す）。
- providers-admin.server.ts: ProviderCreateInput / ProviderPatchInput から `concurrency` を削除、build* 関数の読み取り・送信を削除。
- routing-source-state.ts: 変更なし。`concurrency: "同時実行の上限"` は LLM source の routing 除外理由（`excluded_reason`）の表示ラベルで、provider の `[[providers]].concurrency` ではないため（objective の「など」候補は検討のうえ非対象）。
- types.ts: `corepack pnpm@11.27.0 -C gui gen:types` で再生成（schema 側が api WU で更新済み。`git diff --exit-code` で幂等を確認）。
- test/unit/{providers-admin,accounts,accounts-admin}.test.ts: 生成型の AccountView に必要な `effective_max_runs` / `implicit` / `max_runs_source` を追加、concurrency 関連の期待値を削除。
- clusters.tsx（`[[clusters]].concurrency`）・daemon.tsx（`[execution] max_concurrency`）は別物で未変更。
- アカウント編集 UI は足していない（web WU の範囲）。

## 証拠
- `corepack pnpm@11.27.0 -C gui gen:types && git diff --exit-code -- gui/app/celeris/types.ts` → exit 0（再生成で差分なし）。
- `corepack pnpm@11.27.0 -C gui typecheck`（react-router typegen && tsc -b）→ exit 0。
- `corepack pnpm@11.27.0 -C gui test`（vitest run）→ 96 files / 1342 tests 全部通過、exit 0。
  - 直前 run で browser-phase2 が 1 件失敗（unix socket path 長すぎの EINVAL）した件は run の TMPDIR のパス長が原因の環境要因。短い TMPDIR（workspace 直下）で再実行し 1342 全部通過を確認。
- `grep -i concurrency gui/app/routes/providers.tsx gui/app/celeris/providers-admin.server.ts` → 該当なし（types.ts の deprecated フィールド定義と routing-source-state.ts の除外理由ラベルのみ残存）。

## 未解決・提案
- なし。check 2/3（範囲 check）は CELERIS_WU_BASE 未設定で HEAD 比になるため先行 WU の未 commit 成果で必ず exit 1 になる不備だが、人の決定（2026-10-08、運用作業セッションの計画 v7）で ui 段の範囲 check は外しレビュアーが差分で確かめる扱いになっている。
