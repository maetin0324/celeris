---
title: 健全性v2 G4-R1 fix-review-gap — review 方針の無い汎用 repo の候補を final reviewer に渡す
tasks: [01M74X0XAN0MCFJ4KZ3VVFT2QA]
status: running
updated: 2026-11-11
---
# 健全性v2 G4-R1 fix-review-gap — review 方針の無い汎用 repo の候補を final reviewer に渡す

完了日: （未完了。unit record が書く）

task 01M74X0XAN0MCFJ4KZ3VVFT2QA の進捗。工程は contract → input-owner → request-atomic → request-integrated → record。基準は WorkUnit では `$CELERIS_WU_BASE`、task では `git merge-base HEAD main`。

## 工程 1: contract（Absent の final review 入力契約を改定する）
コードは変えていない。実装の前に契約と実装前 brief を ADR に書いた。

### 変更
- `phase 1: G4-R1 の契約を ADR に書く` — 次の 3 file。
  - 新 ADR [2026-11-11-health-generic-review-input](../../adr/2026-11-11-health-generic-review-input.md): 実装前 brief（7 項目）、R1 Absent の材料（`present` と同じ構築、`owner_rows`・`invariants` だけ空、I7 が及ぶ）、R2 再利用の鍵（4 つ目は選んだ検査器の `definition_digest(&[], Some(&checker))`、base の profile は `target_sha` で束縛）と変更時の失効の表、R3 変えない契約、R4 撤去の表、R5 不変条件と試験、R6 計画の範囲外の path、本番への移行。
  - [ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md): 状態の行、D4.4 と付記「policy の関門の fail closed」の `Absent` の項に改定の注記、末尾に付記「2026-11-11、G4-R1 の改定」。
  - [ADR 2026-11-10 health-generic-fixtures](../../adr/2026-11-10-health-generic-fixtures.md): 付記 (e) に訂正、状態の記録に 1 行。

### 再利用の鍵と失効（受け入れ条件 1 の要約）
鍵は `(repo_id, target_sha, reviewed_sha, definition_digest(&[], Some(&checker)))`。`present`・`absent` で同じ。
- candidate が変わる → `reviewed_sha` が変わり作り直す。
- base が変わる・base の profile が変わる → `target_sha` が変わり作り直す。
- 検査器が変わる（base の検査器、同梱の検査器、組み込み generic の中身） → `checker.sha256` が変わり作り直す。
- 候補が profile・baseline・検査器を変える → `reviewed_sha` で作り直す。値は base の版から作り、`policy.changed = true` で人の承認（I8）。
- policy の状態が不明の材料は再利用しない（今のまま）。

### 証拠（工程 1 の HEAD で実行）
（検査の後に書く）

## 未解決
- 計画の後続 unit の範囲（`expected_write_paths`）に無いが変える必要のある path がある（ADR の R6）。`crates/task-dispatch/src/health_review/policy_review/unknown_tests.rs` の `health_review_policy_unknown_absent_policy_change_needs_approval` は Absent で `HealthReviewInputBuilt` を作らないことを assert していて、input-owner の変更で落ちる。共通の stub の検査器は `source = absent` を返すので、`health` を書かない偽の reviewer を持つ dispatcher 試験（候補: `target_sync.rs`・`health_enforce.rs`・`work_units.rs`）が I7 で落ちる見込み（未確認）。`crates/task-core/src/model/events.rs` の doc comment も古くなる。

## 提案
- input-owner の範囲に、上の 3 つの試験 file と `crates/task-dispatch/src/health_review/policy_review/unknown_tests.rs`、`crates/task-core/src/model/events.rs`（doc comment だけ）を足す。範囲を広げないなら、input-owner は `bash scripts/dev/test-affected.sh` で落ちた試験の一覧を付けて plan_issue で返す。
