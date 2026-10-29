---
title: reviewer の技術的な差し戻しで failed になった task を CoS が自動で開き直す
tasks: [01M63S650J8GS7GMJ587X3RXFS]
status: done
updated: 2026-10-29
---
# reviewer の技術的な差し戻しで failed になった task を CoS が自動で開き直す

完了日: 2026-10-29

ADR: [2026-10-29-cos-review-rework-reopen](../adr/2026-10-29-cos-review-rework-reopen.md)
（[2026-10-28-cos-routine-delegated-answers](../adr/2026-10-28-cos-routine-delegated-answers.md) に付記 A2）

## 変更

- 設計: daemon の決定的な reopen ではなく、CoS の定型の代答の 5 種類目 `review_rework` にした。reviewer の判定に差し戻しの種類の
  構造化した欄が無く、「技術的な不備か」は判定文を読まないと決まらないため（ADR D1）。
- `crates/task-api/src/cos/basis/review_rework.rs`（新規）: API の決定的な検査。失敗の件への `reopen` だけ、最後の worker run が
  reviewer の判定（`reviewer(<id>): …`）で落ちている、refs の `review` がその reviewer run を指す、human check は 403、
  同じ task で CoS の開き直しは 2 回まで（422 `cos_basis_rework_limit`）、前と同じ差し戻し文は 422 `cos_basis_repeated_failure`。
- `crates/task-api/src/cos/basis.rs`: `BasisCategory::ReviewRework`、`BasisRefKind::Review`、失敗の件への `reopen` は `review_rework` 必須。
- `crates/task-api/src/cos/inbox.rs`: `/cos/operations` の `task.reopen` は、受信箱に失敗の件が開いている task なら待ちへの回答として `basis` 必須。
- 差し戻し文は reopen の後の run の `context.prior_review` に既存の仕組みで載る（試験で確認）。
- 記録・取り消し: 既存の代答と同じ（`cos_operations`・task の `CosOperation` event・チャットのカード・`/cos/operations/{o}/override`）。
- `cos-inbox-triage` skill を version 7 に（(e) の条件と、(e) にしない条件）。`[cos.triage] policy_version` の既定 "7"。
  `config/celeris.example.toml`・`docs/ops/cos-chat.md`・`docs/api/v1/api.md`・`docs/ops/mlsys-main-4-ops-r1.md` §8 を更新。API schema と web の生成型を再生成。
- 試験: `crates/task-api/tests/cos_review_rework.rs`（新規 4 件、fake harness の CoS run credential、LLM なし）と review_rework.rs の単体試験 2 件。

## 証拠（提出時の HEAD で実行）

- `cargo test -p task-api --test cos_review_rework`: 4 passed。
- `UPDATE_SCHEMA=1 cargo test -p task-api`: cos 系を含め通過（browser_live_cross は下の TMPDIR 問題で失敗）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo clippy -p task-api --all-targets -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- web: `tsc -b` exit 0。vitest の `e2e/support/fake-daemon.test.ts` は test-affected の中で 1 件落ち、単独では 17 passed（時間依存の不安定）。
- `cargo test --workspace --no-fail-fast`（この run の TMPDIR、93 字）: exit 101。失敗は全て unix socket の SUN_LEN 超過と、それに続く
  fixture の待ち切れ（credentiald・task-worker の browser・celeris の browser_doctor・task-api の browser_waits/browser_live_cross）。
  短い TMPDIR（/tmp 下）で `cargo test -p celeris-credentiald -p task-worker -p celerisctl` は exit 0、task-api の browser_waits・browser_live_cross と
  `celeris --lib browser_doctor` も全て通過。本 task の変更とは関係しない。

## 未解決事項

- run の TMPDIR が長いと socket 試験が落ちる問題は main にもある（本 task の範囲外。直していない）。

## 提案

- reviewer の判定に差し戻しの種類（技術的な不備・目的の変更・範囲外の要求）を構造化して持たせれば、CoS の判断を API の検査で補える。

## 本番への移行

- 本番 config に `[cos.triage] policy_version = "6"` を明示していれば "7" に書き換える。KB に `cos-inbox-triage` の写しがあれば差し替える。
