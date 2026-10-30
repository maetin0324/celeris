---
title: 問題の無い計画の承認（plan_gate）を CoS が人を待たずに approve する
tasks: [01M655VRMSE9TV5FHWXJFKSP77]
status: done
updated: 2026-10-30
---
# 問題の無い計画の承認（plan_gate）を CoS が人を待たずに approve する

完了日: 2026-10-30

ADR: [2026-10-30-cos-plan-approval](../adr/2026-10-30-cos-plan-approval.md)
（[2026-10-28-cos-routine-delegated-answers](../adr/2026-10-28-cos-routine-delegated-answers.md) に付記 A3）

## 変更

- 設計: daemon の決定的な approve ではなく、CoS の定型の代答の 6 種類目 `plan_approval` にした。本番の変更・外部公開・破壊的操作・
  秘密・費用の枠・目的の変更は計画の自由文を読まないと決まらないため（ADR D1）。
- `crates/task-api/src/cos/basis/plan_approval.rs`（新規）: API の決定的な検査。plan gate への `approve` だけ、refs の `plan` が待っている版と
  前の版をちょうど指す、`plan.changes`（差分）と `plan.checked`（確認 7 項目を全て）、human check は 403、`near_limit`・daemon の検査の失敗
  （plan_invalid・limit・leaf_too_large の決定）・前の版の後の目的/受け入れ条件の編集は 422 `cos_basis_plan_needs_human`、
  5 版目以降・同じ task で CoS の承認 3 回の後は 422 `cos_basis_plan_limit`。
- `crates/task-api/src/cos/basis.rs`: `BasisCategory::PlanApproval`・`BasisRefKind::Plan`・`DelegatedBasis.plan`。plan gate への `approve` は
  `plan_approval` 必須（従来は推奨と同じなので `documented_default` で通せた）。`within_approved_frame` の対象から plan gate を外した。
- 計画の decision・`review: human` の段は承認で消えない（approve は decision に答えない。試験で確認）。
- 記録・取り消し: 既存の代答と同じ（`cos_operations`・task の `CosOperation` event・`PlanGateDecided{by: cos:…}`・チャットのカード・
  `/cos/operations/{o}/override`）。
- `cos-inbox-triage` skill を version 8 に（§2 の表に計画の承認の行、§3 に (f) と確認 7 項目・例）。`[cos.triage] policy_version` の既定 "8"。
  `config/celeris.example.toml`・`docs/ops/cos-chat.md`・`docs/api/v1/api.md`・`docs/ops/mlsys-main-4-ops-r1.md` §8・skill の SOURCE.md を更新。
  API schema と web の生成型を再生成。
- 試験: `crates/task-api/tests/cos_plan_approval.rs`（新規 5 件、fake harness の CoS run credential、LLM なし）。
  `cos_chat_triage_scenarios.rs` の plan gate の代答を `plan_approval` に改めた。

## 証拠（提出時の HEAD で実行）

- `cargo test -p task-api --test cos_plan_approval --test cos_chat_triage_scenarios --test cos_review_rework --test cos_routine_answers`: 5・3・4・5 passed。
- `bash scripts/dev/test-affected.sh`: exit 0（celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker の 4775 passed、
  web の tsc -b と unit 697 passed）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 未解決事項

- 人の取り消し（override）は plan gate を不可逆の操作として扱う（run が始まっていれば subtree を止め、確認の task を作る）。
  計画の承認待ちに戻す取り消しは無い（既存の phase gate の代答と同じ）。

## 提案

- 計画の spec に「本番の変更」「外部への送信」「有料の資源」などの構造化した印を持たせれば、(f) の確認の一部を API の検査に移せる。
- `config/skills/cos-operator/operations.md` の plan gate の行は action を `continue` と書いているが、plan gate の action は
  `approve`/`replan`/`withdraw`。cos-operator の版を上げて直すとよい（本 task では触っていない）。

## 本番への移行

- 本番 config に `[cos.triage] policy_version = "7"` を明示していれば "8" に書き換える。KB に `cos-inbox-triage` の写しがあれば差し替える。
