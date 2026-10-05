---
unit: fix-fixtures-r2
task: visual-qa 01M44C029SCGEZHEK57WEK3QNB（深さ 3 の unit）
status: done
base: 9fc0016c
completed: 2026-10-05
---

# 報告・承認・review 待ち task の既定 fixture を足し、撮影は loading の後にする

## 変更

- `web/e2e/support/fake-daemon.mjs`（rich profile の既定 fixture に追加）
  - `/api/v1/reports`・`/api/v1/reports/RP1|RP2|RP3`（一覧と本文。RP1 は RP2・RP3 を sources に持つ）
  - `/api/v1/approvals`（決めた認可 2 件。`once` と `standing`）
  - `/api/v1/standing-rules`（常設ルール 1 件。承認画面の「常設」節が取得失敗の帯を出さないために足した）
  - `/api/v1/tasks/T1/execution`（phase `verifying`、plan v1・WU 2 件、gate `compound`、R1 の run）
  - `/api/v1/tasks/T1/routing`（担当 `ui-ux`、run R1 の lane・model・org_node・rule_id）
  - 形は `fixtureFor` で schema の required から組み、画面が読む欄だけを正常な値で埋めた。`validateFixture` の errors は全て 0。
- `web/e2e/support/fake-daemon.test.ts`: 上の 5 path を schema と照合する試験を 1 件足した。
- `web/e2e/states/rich-data.spec.ts`: 既定 fixture で `/reports?report=RP1`（幅 390）・`/approvals`・`/tasks/T1`（幅 1280）を開く試験を足した。loading と error の件数が 0、報告・承認・実行と routing の本文が見えることを確かめる。
- `web/scripts/screenshots.mjs`（通常経路のみ）: `goto` の後に `h1` の出現を待ち、`[data-fetch-state="loading"], [aria-busy="true"]` が消えるまで `waitForFunction` で待ってから撮る。固定の時間は使わない。`--states` の保留と `releaseHeld` は変えていない。
- 変更なし: `fixture-gateway.mjs`（既定が rich のため）、`states.ts`（8 状態）、`states.spec.ts`、画面コンポーネント、`web/api`、`mobile-audit.mjs`。

## 既定 fixture の一覧（rich profile）

| path | 中身 |
| --- | --- |
| /api/v1/reports | RP1（result・T1・P1）、RP2（progress・level 2）、RP3（question・level 1） |
| /api/v1/reports/RP1 | 本文（Markdown）と sources_expanded（RP2・RP3） |
| /api/v1/reports/RP2, RP3 | 本文のみ |
| /api/v1/approvals | 2 件（once・standing） |
| /api/v1/standing-rules | 1 件（SR1） |
| /api/v1/tasks/T1/execution | phase verifying、plan v1（WU 2 件）、gate compound、runs R1 |
| /api/v1/tasks/T1/routing | assignee ui-ux、runs R1 |

## 証拠（実行したコマンドと結果）

作業ディレクトリは `repos/agent-platform`。`web/` の依存は `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` で入れた（exit 0）。

- `corepack pnpm@12.6.0 -C web typecheck`（`tsc -b`）: exit 0
- `corepack pnpm@12.6.0 -C web lint`（`biome check .`）: exit 0。警告 5 件はいずれも今回の 4 ファイルには当たっていない（警告の出た file を確かめた）。
- `corepack pnpm@12.6.0 -C web test`: exit 0。vitest 58 files・351 tests passed。node --test 42 pass・0 fail。
- `corepack pnpm@12.6.0 -C web e2e states/`: exit 0。32 passed（新しい試験を含む）。新試験は、常設ルールの fixture（`/api/v1/standing-rules`）を足す前に一度失敗した（`/approvals` の error が 1 件）。足した後は 32 件すべて通った。
- `corepack pnpm@12.6.0 -C web mobile-audit`（リポジトリ直下から実行）: exit 0。`mobile-audit: 31 path(s) x 4 widths ok`。
- 撮影（通常経路、1 回だけ手で実行）: `node scripts/screenshots.mjs --only <path> --out artifacts/fix-fixtures-shots` を `/reports`・`/approvals`・`/tasks/T1` で実行。いずれも exit 0、幅 360・390・412・1440 の 4 枚ずつ（計 12 枚）。`_reports-390.png` と `_tasks_T1-390.png` で正常な本文が写っていることを目視で確かめた。
- `git diff --name-only 9fc0016c -- web` は 4 ファイル（上の変更の一覧と同じ）。

## 残課題

- `/api/v1/org` の fixture はまだ無い。報告の送り手と承認の依頼元は node の id のまま出る（取得失敗の帯は出ない）。名前を出すなら org の fixture を足す。今回は範囲外（web/api の形の確認が要る）として残す。
- `--states` の loading 状態は今どおり保留したまま撮る。`--states` の全件撮影は走らせていない（時間が長い）。
- `/reports` の本文は行を開くまで出ない（既定で閉じている）。撮影の既定の姿は閉じた一覧。本文の姿は e2e の `?report=RP1` で確かめている。
