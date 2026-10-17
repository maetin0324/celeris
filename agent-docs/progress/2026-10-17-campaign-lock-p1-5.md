---
title: "最終接続修正 P1-5: campaign lock の共通契約"
tasks: [01M54HYGQT7WW6V8K3446X3FK8]
status: running
updated: 2026-10-17
---
# P1-5 campaign lock

ADR: `agent-docs/adr/2026-10-17-campaign-lock.md`。外部 LLM・有料ジョブ・本番変更はしていない。

## WU lock-model（完了）
- `crates/campaign/src/lock.rs` 新設、`lib.rs` に登録。`manifest::canonical` を `pub(crate)` にして再利用（新しい hash 方式なし）。
- 公開 API: `CampaignLock` / `LockedCase` / `LockedInitial` / `LockedSubmodule` / `LockedTest`(Public|Hidden) / `GraderPin` / `ExecutionPin` / `LockError`、
  `CampaignLock::{new, validate, normalized, lock_hash, seal, bind_to_manifest}`、`LockedCase::from_declared`、`verify_lock(expected, actual)`、`check_successor(old, new)`。
- 再利用: `record::DeliveredInput`（外部入力）、`manifest::{Split, Config, Budget, Termination, ArmOrder, compute_sha256}`、`grader::case::ImplementationSpec`（公開/隠しテスト）、`snapshot::hash_tree` の値（展開済み内容 hash は後続 WU が入れる）。
- 実装 case は公開・隠しテストを各 1 件以上固定しないと拒否（空一覧で済ませない）。
- 証拠: `cargo test -p celeris-campaign --lib lock` 9 件 pass（重複 case/path、未固定値、版違い、測定 ID 違い、同一測定 ID での内容変更の種別ごと拒否、封印改変、後継規則）。

## 後続 WU への引き継ぎ
- tree-proof: commit/tree/gitlink から `LockedInitial` を作る側。`verify_lock` の expected に信頼側を渡す。
- k2-pin: K2 の submodule 内容 hash を `LockedSubmodule` に入れる（取得不能なら未完了と明記）。
- 実行・再開・grade-out・bind・export への配線は未着手（本 WU の範囲外）。

## 未解決・提案
- なし。lock のファイル配置（`<out>/snapshot/lock.json` 等）は統合 WU で決める。
