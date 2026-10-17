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

## WU run-resume（実装済み・無課金試験で検証済み。実経路 = 実 LLM/実 task-api は未検証）
- 新規 `crates/campaign/src/lock_exec.rs`（ADR D8）。`preflight::expected_lock` が CLI（`run|watch --executor harness|celeris`）とライブラリ（`run_gated`）で共通の期待 lock を作り、
  `preflight`（check `lock`）→ `runner::run`（`RunOptions.lock`）→ `watch`（毎走査で runner が再照合）に接続。`--measurement-id` を追加。
- 副作用前の拒否: 誤 repo・余分/欠落 file・違う submodule path・資料・KB・外部入力は executor 呼び出し 0・out 未作成。lock 欠落・改変・再封印・grader/テスト/grade-case 差替え・測定 ID 違いは resume 拒否で lock を書き換えない。
- コピー後検査: snapshot/repo と workspace-seed を lock の期待値で照合（失敗時 snapshot 削除）。再開時は snapshot.json の hash が整合していても lock と違えば拒否。
- 複数 case の初期木: 異なる初期木は拒否、初期木なしの case（K4）は repo なしの seed を使う（従来は全 case に K2 repo が配られていた）。
- 既存 out（lock なし）は新 lock に移行せず拒否。
- `lock.rs` の変更: 初期木を持つ implementation case は公開テスト一覧が空でも可（K2 の grade-case は隠しテストのみで、公開テストは初期木の内容 hash に含まれる）。対応する試験を更新。
- 範囲外の最小変更: `RunOptions` に `lock` 欄を足したため `crates/celeris/tests/campaign_acceptance_e2e.rs` に `lock: None`（制御 fixture）を明示。
- 試験 `crates/campaign/tests/lock_execution.rs` 7 件（cargo test -p celeris-campaign --test lock_execution）: 正しい初回/resume/watch、誤実体の副作用 0、異初期木/repo-free、lock・定義差替え、lock なし既存データ、snapshot 再照合、測定 ID 規則。
  K2 初期木は試験用の小さな git repo（本番資産に依存しない）。
- 証拠: `bash scripts/dev/test-parallel.sh` exit 0（5647 passed, 0 failed）／`cargo clippy --workspace -- -D warnings` exit 0／`cargo fmt --all -- --check` exit 0。
- 未解決（後続 WU）: grade-out・bind・export の lock 照合（grade-export）、正式 task-api 経路での受入（connected-verify）。runner は mock 以外の stub executor が lock なしで走るのを強制していない（CLI と run_gated が強制）。

## WU grade-export（実装済み・無課金試験で検証済み。実経路 = 実 LLM/実 task-api は未検証）
- `lock_exec::{load_saved, verify_grade_case}`、`LockedCase::tests_sha256`、`CampaignLock::{check_seal, case}` を追加。run-meta に `locked`（lock 付き実行か）を記録。
- grade-out（`grade_out_expected`）: 保存 lock（封印・manifest 結合・CLI では cases root から作り直した期待 lock とも照合）を読み、case id/split だけで信用せず、grade-case の hash と
  公開/隠しテストのディスク上の実体 hash を lock に照合。不一致は grader command 実行前に拒否（`rejected`、command 0 回）。grade 行に `lock_sha256`・`tests_sha256` を追加。
  `--cases` は `<dir>/<problem>.json`、無ければ `<dir>/<split>/<problem>/<problem>.json`。
- bind: grader hash 非空検査を、lock 封印・case の grade-case hash・テスト digest・case split の照合に置換。重複・古い成果物（`workspace_sha256_final` 照合）は従来どおり拒否。
- checker/export/check-report: 同じ保存 lock（CLI は期待 lock も）を使う。lock が信用できなければ grade を 1 件も有効にせず違反。export は checker 拒否時に dest の既存 `table.*` を削除（旧最終版を有効なまま残さない）。表に `lock_verified`、`real_performance` は lock 検証済みのときだけ。
- 既存不備の修正: checker が初期木なし case（K4 等）の初期 workspace hash を通常 seed と比べて違反にしていた（repo なし seed と比較するよう修正）。
- 新測定 ID なしの判定条件更新は拒否（既存 `verify_lock`/`check_successor`、resume・採点とも保存 lock と期待 lock の完全一致が要る）。
- legacy: lock.json が無く run-meta も `locked` でない out（lock 導入前・無課金 mock）は従来の結合検査のみ。表は `lock_verified=false`・`real_performance=false`。`locked` と記録された out や期待 lock を渡された経路で lock が無ければ拒否。
- 試験 `tests/lock_grading.rs` 5 件: 正常な採点・結合・export／別 grader・隠しテスト実体差替え・公開テスト追加・隠しテスト削除で command 0 回／grader・test・lock・重複・古い成果物の bind 拒否と export の旧最終版削除／lock 欠落・改変・再封印／legacy。
- 証拠: `cargo test -p celeris-campaign --test lock_grading` 5 passed、`bash scripts/dev/test-parallel.sh` exit 0（5652 passed, 0 failed）、clippy/fmt は下の最終実行。

## WU connected-verify（正式 task-api 経路の無課金受入。実 LLM・実経路は未検証）
外部 LLM・別 LLM CLI・有料 job・本番変更・userns なし。ADR D10。

### 追加した試験
- `crates/celeris/tests/campaign_lock_e2e.rs`（4 件）: 実 HTTP の task-api・一時 DB・`CelerisExecutor` の通常 `POST /tasks`、期待 lock は `preflight::gate`→`expected_lock`→`watch`（CLI と同じ）。
  1. `locked_measurement_runs_through_task_api_to_the_final_export`: 正例。lock 保存＝期待 lock、B/C 4 root、再開は再投入 0・lock 不変、grade-out→bind 違反 0→export `lock_verified=true`。
  2. `wrong_initial_repo_submodule_materials_kb_or_inputs_cause_no_post_or_external_run`: 誤 repo/余分 file/別 submodule/別 repo/資料/KB/外部入力で POST 0・A 投入 0・task 0・out 未作成。元の入力なら通る対照付き。
  3. `swapped_grader_or_tests_are_rejected_at_grade_bind_and_export`: grader 定義・隠しテスト実体・split・隠しテスト削除は grader command 0 回で拒否。grade 行の grader/test/lock/artifact hash 差替えは bind 違反、export 拒否（旧 table.json を残さない）。
  4. `changed_lock_or_inputs_refuse_to_resume_an_existing_measurement`: lock 改変・再封印・欠落・隠しテスト差替え・測定 ID 違い・repo 変更で再開拒否（再投入 0）、元の定義なら同じ task を追って完了。
- `crates/campaign/tests/lock_cli.rs`（3 件、実 binary）: CLI 引数（`--measurement-id`/`--cases-root`/`--grader-root`/`--grade-case-root`/`--cases`/`--repo-src`）が期待 lock に配線されていること。
- 既存不備の修正: `lock_execution.rs` が doc link 検査に掛かる相対 path を書いていた（`check-doc-links.sh` 2 件）ため、`lock_grading.rs` と同じ組み立てに直した。

### 証拠（このコミット時点）
- `cargo test -p celeris --test campaign_lock_e2e --test campaign_acceptance_e2e` exit 0（lock_e2e 4 passed、acceptance_e2e 7 passed）。
- `cargo test -p celeris-campaign --test lock_cli` 3 passed。
- `bash scripts/dev/test-parallel.sh` exit 0（5659 passed, 0 failed, 15 ignored）／`cargo clippy --workspace -- -D warnings` exit 0／`cargo fmt --all -- --check` exit 0。
- `sh scripts/dev/check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh --check` はいずれも exit 0（最終コミット後に再実行）。

### 検証区分
| 区分 | 内容 |
|---|---|
| 実装済み | lock 型・内容証明・K2 固定、実行/再開/watch/grade-out/bind/export/CLI への配線 |
| mock 検証済み（無課金） | 上記 `campaign_lock_e2e`・`lock_execution`・`lock_grading`・`lock_cli`・`campaign_lock`。task-api は実物、実行内部と grader command は stub |
| 実経路検証済み | **なし**。実 LLM・実 worker・実 dispatcher・実 grader command（cargo test）を通した測定は未実施 |
| 未検証 | 実 LLM を伴う本測定、実 K2 submodule 入りの BenchFS の実 build と K2 隠しテストの実行、K4 の実 raw 入力（`main-v3-n2-43706.tgz`）での recompute |

### K2 資産の照合結果
- `/local/celeris/data/campaign-assets/k2/` で `sha256sum -c SHA256SUMS` 全 OK（bundle `benchfs-086c4618.bundle`、`ior-7054224d`・`locusta-68f8b210`・`pluvio-9ecd51ee` の tar）。
- `cargo test -p celeris-campaign --test k2_pin --test campaign_lock`: k2_pin 3 passed、campaign_lock 6 passed（資産から組んだ木の `verify_archive` を実行、skip 合格なし）。固定値は `initial.lock.json`（本体 `2862119f…`）。
- 未検証: 組んだ BenchFS 木の実 build（cargo build）は行っていない（時間・負荷と範囲外のため）。e2e の K2 初期木は試験用の小さな git repo で、本番資産ではない。

### 未解決・提案
- 実 LLM を伴う実測定の前に、人が preflight（`celeris-campaign preflight`）と BenchFS 実 build を確認する。
- 提案: `RunOptions.lock = None` を許す無課金制御用の経路が runner に残る。mock 以外の executor で lock 無しを runner 自身が拒否する変更は別 task で検討。

