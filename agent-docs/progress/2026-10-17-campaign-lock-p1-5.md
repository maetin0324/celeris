---
title: "最終接続修正 P1-5: campaign lock の共通契約"
tasks: [01M54HYGQT7WW6V8K3446X3FK8]
status: running
updated: 2026-10-17
---
# P1-5 campaign lock（campaign-lock-p1-5）

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

## check の不整合（再実行）
- package 名は `celeris-campaign`。計画の check `cargo test -p campaign lock` / `cargo clippy -p campaign` は「did not match any packages」で成果に依らず落ちる。
- 正しい形 `cargo test -p celeris-campaign lock`・`cargo clippy -p celeris-campaign -- -D warnings`・`cargo fmt --all -- --check` は exit 0。

## WU tree-proof（完了）
- `crates/campaign/src/lock_tree.rs` 新設（`lib.rs` に登録）。外部 LLM・ネットワーク・本番変更なし。
- 内容 hash = path 昇順 × mode（100644/100755/120000）× blob 内容 sha256 の正規化列の sha256。空 dir・root の `.git`・owner/mtime は数えない（`snapshot::hash_tree` は mode を見ないため、git 由来と照合できる別 hash にした。ADR に追記要）。
- API: `derive_initial(repo, rev, submodule_repo)`（commit/tree/gitlink → `LockedInitial`。submodule 実体 repo が無ければ `Unavailable`、成功扱いにしない）、
  `verify_repo(expected, repo)`（渡された repo を期待値にせず lock と照合）、`verify_archive(expected, dir)`（`.git` の有無を問わず本体＋各 submodule を照合）、
  `archive_content_sha256` / `git_content_sha256`、`TreeProofError`。
- 試験（`cargo test -p celeris-campaign --lib lock_tree` 5 件 pass、crate 全体 pass、clippy `-D warnings`・fmt exit 0）:
  git 由来と metadata 無し archive の同一 hash（`git archive` 展開）／submodule 欠落・空 dir・差替え・1byte 修正・追加・本体修正の拒否／
  別 commit・偽 tree・偽 gitlink の lock 拒否／metadata（root `.git`、submodule `.git` file、実 clone）の有無は拒否理由にならない／exec bit。
- 制約: submodule の入れ子は `Unsupported`。symlink は link 先を内容として数える。
- k2-pin への引き継ぎ: K2 の `submodule_repo` に承認済み資産の repo を渡して `derive_initial`。取得不能なら `Unavailable` を未完了として報告する。
