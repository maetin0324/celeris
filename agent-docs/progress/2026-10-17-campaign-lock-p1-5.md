---
title: "最終接続修正 P1-5: campaign lock の共通契約"
tasks: [01M54HYGQT7WW6V8K3446X3FK8]
status: done
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

## WU k2-pin（完了: K2 初期 tree の内容 hash を固定）
- 人の承認により運用者が `/local/celeris/data/campaign-assets/k2/`（ローカル・読み取り専用）に配置した資産だけを使用。ネットワーク取得・外部 LLM・本番変更なし。`sha256sum -c SHA256SUMS` 全 OK。
- 組み方: bundle を clone → `086c4618…` checkout → 各 submodule path に tar 展開。`git get-tar-commit-id` で tar の commit が gitlink と 3 件とも一致。tree は case.json と同じ `56d2f021…`。
- 固定値: `crates/campaign/cases/final/k2-bf-xattr-restart-campaign/initial.lock.json`（`LockedInitial`。本体 `2862119f…`、pluvio `a81aeba9…`、locusta `dbc3d1e6…`、ior `fc37ce04…`）。本体は `verify_repo`、組んだ木全体は `verify_archive` で通る。
- 試験 `crates/campaign/tests/k2_pin.rs`: 固定値と case.json gitlink の一致／資産から組んだ木の照合（資産が無ければ失敗、skip 合格にしない。資産の無い host では落ちる）／submodule 1 byte 修正の拒否／実体なしは `Unavailable`。3 件 pass。
- 差: 旧 `k2-initial.tar` の sha256 `b1ae8f58…` は再現できない（その tar は無く、tar の sha256 は pack 方法に依存する）。今回の内容 hash を正とする。本測定の出力はまだ無いので後付けではない。
- io500 submodule は tree の gitlink に無い（`.gitmodules` のみ）ため含めない。
- 注意: submodule 内容 hash は tar 展開（git archive、`.git` 無し）由来。`derive_initial` は submodule の git repo を要するため使わず、同じ正規化の `archive_content_sha256` を使った（git 由来と同一になることは tree-proof の試験で確認済み）。

## WU lock-contract-test（完了）
- `crates/campaign/tests/campaign_lock.rs` 新設（target `campaign_lock`、公開 API の `lock` / `lock_tree` のみ）。6 件:
  順序入力に依らない hash と seal 後改変の拒否／重複 case・path・input・test、未固定値、版違い、測定 ID 違い、公開/隠しテスト空一覧の拒否／
  同一測定 ID での内容変更拒否（隠しテスト・grader・入力・submodule・資料・実行構成・case 追加）と `check_successor` が新測定 ID を要求／
  git 由来と `.git` 無し archive が同一内容で通り、submodule 欠落・差替え・追加・1byte 修正・本体修正を拒否、metadata の有無は拒否理由にならない／
  K2 `initial.lock.json` を `LockedInitial` として case に入れて封印・照合（submodule hash 1 つの差は `SubmoduleMismatch`）／
  資産 `/local/celeris/data/campaign-assets/k2/` があれば clone + tar 展開した木の `verify_archive`（`.git` あり・なし）。資産が無い host では失敗にせず `k2_pin.rs`（資産が無ければ失敗）に任せる旨を stderr に出す。
- 既存 src は変更なし。不備は見つからず。
- ADR `2026-10-17-campaign-lock.md` に D5（内容証明）・D6（K2 固定と資産出典）・D7（後続 API と未着手の配線）を追記。
- 証拠: `cargo test -p celeris-campaign --test campaign_lock` 6 passed／`cargo clippy -p celeris-campaign --all-targets -- -D warnings` exit 0／`cargo fmt --all -- --check` exit 0／`check-doc-links.sh` exit 0。
- 未解決: 実行・再開・grade-out・bind・export への配線と lock ファイル配置は後続（本 task では未着手）。
- 提案: 後続で `<out>/snapshot/lock.json` に封印済み lock を置き、bind / grade-out / export が `verify_lock` と `bind_to_manifest` を呼ぶ。

## 最終確認（WU final-verify）
- コードは変更なし。`cargo test -p celeris-campaign --test campaign_lock --test k2_pin` exit 0（campaign_lock 6 passed、k2_pin 3 passed。K2 資産 `/local/celeris/data/campaign-assets/k2/` ありで実体照合まで実行、skip 合格なし）。
- 記録検査: `check-adr-numbers.sh` exit 0／`check-doc-links.sh` exit 0／`cargo fmt --all -- --check` exit 0。
- status: done を維持。未解決は前節のとおり（実行・再開・grade-out・bind・export への配線は後続）。
