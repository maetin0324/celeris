---
tasks: [01M4R2CDTXZ4N7XZ8SN2M8NHEH]
status: done
title: 旧 gui の画面に残る provider 表示を source・harness に置き換える（gui-provider-ui、差し戻し修正）
updated: 2026-10-12
---

# 旧 gui の画面に残る provider 表示を source・harness に置き換える

final review の差し戻し（「旧 gui の task 概要・run 詳細・accounts・daemon に provider 前提の表示が残る」）を直した。
表示は API の `RunSummary.selection`（harness・source・model・account）から出し、旧 run（`legacy_unresolved`）は
「記録なし（旧 run）」と出す。`crates/` と `web/` は変えていない。

## 変更

| 箇所 | 変更 |
|---|---|
| `gui/app/lib/run-selection.ts`（新規） | `runSelectionParts`・`runSelectionText`・`LEGACY_SELECTION_TEXT`。選択の記録だけを使い、推測で埋めない |
| `gui/app/components/task-detail/OverviewTab.tsx` | run 一覧の `adapter`・`provider` 列を `harness`・`source` 列に置き換え。model・account は selection の値（旧 run は既存の `run.model`・`run.account`） |
| `gui/app/routes/tasks.$id.runs.$runId.tsx` | 見出しの `{adapter} / {provider} / {model}` を `harness / source / model / 口座 account` に |
| `gui/app/components/TaskRoutingPanel.tsx` | `runs` prop（任意）を足し、同じ run_id の `RunSummary.selection` から「選択」行を出す。`run.provider` の表示は消した。`gui/app/routes/tasks.$id.tsx` が `runs={detail.runs}` を渡す |
| `gui/app/routes/accounts.tsx` | 「プロバイダ画面」の文言を消し、存在しない `provider-tier-models-heading` への目次項目を削除。`USED_BY_SCOPE_LABEL` の provider 分岐を削除。env_from_secrets の文言から「プロバイダ」を消す |
| `gui/app/routes/daemon.tsx` | in_flight の列を `source`（`InFlight.provider` の中身は source id。`data-testid` は `in-flight-source`）。cooldown の列を `source / account`、空の文言を「source」に |
| `gui/app/lib/llm-sources.ts` | `providerLlmSourceDisplay`・`llmSourceOriginLabel`（呼び出し元が無くなった）を削除 |
| `gui/app/routes/help.tsx` | 用語「プロバイダ（アカウント）」を `harness`・`LLM source` の 2 項に置換。cooldown・account_pool・アカウント案内・401 節の「プロバイダ」を source・アカウントの語に |
| `gui/app/root.tsx`・`gui/app/routes/clusters.tsx` | 画面区分のコメントの「プロバイダ」を消した（表示文字列ではない） |
| `gui/test/unit/llm-sources.test.ts` | `providerLlmSourceDisplay`・`llmSourceOriginLabel` の試験を削除 |
| `gui/test/unit/run-selection.test.ts`（新規） | 選択の分解・表示文字列・旧 run の「記録なし」を確かめる |
| `gui/e2e/g4.spec.ts` | in_flight の期待を `in-flight-source` に。題の `provider` を `source` に |

`gui/app/celeris/types.ts` の欄名（`RunSummary.provider`・`InFlight.provider`・`RunRoutingAudit.provider` 等）は API の名前に合わせて残した。
`daemon.tsx` の `item.provider` は source id の値として表示する。

## 証拠

- `cd gui && corepack pnpm@11.27.0 install --offline --frozen-lockfile`: exit 0（`Done in 1.8s`）
- `corepack pnpm@11.27.0 typecheck`（`react-router typegen && tsc -b`）: exit 0
- `corepack pnpm@11.27.0 lint`（`biome check .`）: exit 0。警告 1 件（`scripts/check-resume-recovery.mjs` の `useTemplate`）は今回の変更の外（既存）
- `corepack pnpm@11.27.0 test`（`vitest run`）: 93 files・1306 tests passed, exit 0。
  - 最初の試行は `TMPDIR` が長すぎて unix socket の path が 108 字を超え、`browser-phase2` の 1 件が `listen EINVAL` で落ちた。`TMPDIR=/local/x/gp` にして再実行し、全件通った。試験の内容の問題ではない。
- `corepack pnpm@11.27.0 build`（`react-router build`）: exit 0（`built in 1.11s`）
- `grep -rn "run\.provider\|provider-tier\|providerLlmSourceDisplay\|プロバイダ\|>provider<" gui/app/routes gui/app/components gui/app/lib`: 0 件
  - 残る `provider` は `daemon.tsx` の `item.provider`（InFlight の API 欄。値は source id）と `BrowserWaitsPanel.tsx` の `credential.provider`（credential の provider。LLM provider ではない）。どちらも意図した残り。
- `git status`: `crates/`・`web/` に差分なし（変更は `gui/` の 12 files と新規 2 files）

## 未解決事項

- 未実行: `gui/e2e` の g4（実 celeris と fixture が要る）。in_flight の期待値 `fake-local` は fixture の source id と合っているかをまだ確かめていない。
- `gui/app/components/task-detail/TimelineTab.tsx` と `gui/app/lib/console.ts` の run 開始の文言（`{adapter} / {model}`）は、この unit の範囲外として残した。`adapter` は harness に置き換わる語。
- `RunSummary.model`・`account` は旧 run の表示の fallback に使っている（旧 run は event の記録であり推測ではない）。

## 提案

- API の欄名 `provider`（`RunSummary.provider`・`InFlight.provider`・`RunRoutingAudit.provider`・`LaneResolution.provider`）は中身が source id か provider 選択の名残なので、crates の schema 改名（`source` へ）を次の unit で行う。今回は crates を変えない制約のため据え置いた。
- `RunRoutingAudit.adapter` と `RunSummary.adapter` も、harness が入った後は同じ情報の重複。撤去を検討する。
- `TaskRoutingPanel` は `runs` が無いと「記録なし（旧 run）」を出す。呼び出し元を増やすときは `runs` を必ず渡す。
