---
title: web と文書: 暗黙アカウントを LLM source 単位に合わせる
tasks: [01M4D7RVKXY9A967ZXDADWVHQJ]
status: done
updated: 2026-10-09
---
# web と文書: 暗黙アカウントを LLM source 単位に合わせる

2026-10-09、WorkUnit `source-ui`（run 01M4FSRYTPSAY39D8A9REB0FNG, attempt 3）。
ADR 2026-10-08-account-concurrency-limits §(e)・gui-api.md §3.29a。crates/ は変えていない。

## 前提（人の決定）

非プール行の上限は (ii) implicit-account。暗黙アカウントは LLM source ごとに 1 件（`implicit/<llm source id>`）で、
同じ source を使う行が 1 件を共有する。LLM source の config 側には上限を持たせない。旧 `[[providers]].concurrency` は引き継がない。
決定記録の (i) source は誤操作と訂正されている（決定記録の修正は人が web で行う）。

## 確認した状態（先行 commit 64d419ff・09c10512・b9f206e5 の成果）

- `web/features/ops/accounts-screen.tsx`: 暗黙アカウントは「暗黙アカウント」の印・「アカウントを持たない行の LLM source「<id>」」の説明・
  `実行枠（この source を使う行）` に `providers`（使う行の id 一覧）を出す。ログイン・確認・削除は出さず、上限の編集欄は他のアカウントと同じ。
  PATCH は `/api/accounts/implicit/<source id>`（`:` は path にそのまま書く）。
- `web/features/ops/accounts-screen.test.tsx`: 暗黙アカウントの fixture は `openai_compatible:qwen`・`providers: ["opencode-qwen", "pi-qwen"]`。
  表示・PATCH 先・保存と既定に戻すを検査。
- `web/e2e/support/fake-daemon.mjs`: 暗黙アカウント `openai_compatible:qwen`（providers `qwen-local`）と `PATCH /accounts/{adapter}/{id}`。
- `web/e2e/admin/ops-config.spec.ts`: 暗黙アカウントの保存・既定に戻す・表示（`0 / 実効上限 2（このアカウントに保存した上限）` など）。
- `docs/guides/providers.md`（実効上限の順序・暗黙アカウントの節）と `docs/ops/provider-llm-source-migration.md`
  （非プール行の上限は source の暗黙アカウント、本番 Qwen 行の PATCH 手順と戻し方）は source 単位に書かれている。
  本番操作は人の手順として書くだけで、本番 config・systemd・daemon・本番 DB は変更していない。

## 検査（この run）

| コマンド | 結果 |
| --- | --- |
| `web: node_modules/.bin/tsc -b` | exit 0 |
| `web: node_modules/.bin/biome check .` | exit 0（styles.css の既存 warning 4 件のみ） |
| `web: vitest run features/ops/accounts-screen.test.tsx features/ops/providers-screen.test.tsx features/ops/providers-form` | exit 0（2 files / 37 tests） |
| `web: WEB_E2E_SCOPE=functional playwright test e2e/admin/ops-config.spec.ts e2e/parity/ops.spec.ts e2e/work/narrow-r6.spec.ts` | exit 0（30 passed） |

作業場所の TMPDIR は workspace の下の短い dir を使った（UNIX socket のパス長対策）。

## 残り・申し送り

- 画面の見出しは暗黙アカウントの id（`openai_compatible:qwen`）で、「LLM source」の語は説明文に出す。見出しを「LLM source <id>」に変えるかは
  後の見た目の判断に任せる（現状の試験は説明文を検査している）。
- 決定記録 nonpool-limit の (i) → (ii) の修正は人が web で行う（この run では触らない）。
