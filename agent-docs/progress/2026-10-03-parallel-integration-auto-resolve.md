---
title: 並列取り込みの自動解消と統合の依頼（task の総括と検証）
tasks: [01M3ZCXNXTRTA9GNJ52Q36ZFCP]
status: done
updated: 2026-10-03
---

# 並列取り込みの自動解消と統合の依頼（task の総括と検証）

完了日: 2026-10-03（close WU、取り込み基点 c40ceb22）

ADR: [2026-10-02 並列取り込みの自動解消と統合の依頼](../adr/2026-10-02-parallel-integration-auto-resolve.md)（付記を含む）

## 自動で解くもの（決定的な手順のみ、LLM は使わない）

| 種類 | 手順 | 再 review | 実装 |
|---|---|---|---|
| 追記だけの記録（`agent-docs/PROGRESS.md`・`agent-docs/progress/**/*.md`） | `.gitattributes` の `merge=union` と records resolver。base の行を保ち、両側の末尾追記を両方残す | 不要（gate 再実行） | `crates/task-dispatch/src/auto_resolve/records.rs` |
| migration 番号の衝突 | 取り込み側の未取り込み file だけ空き番号へ `git mv`、参照を追従。main の番号は動かさない | 必要 | `auto_resolve/renumber.rs` |
| 番号付き ADR の衝突 | 取り込み側の file を `agent-docs/adr/<追加日>-<slug>.md` へ移す（ADR-0128 D5 の日付名）。番号は振り直さない（付記） | 必要 | `auto_resolve/renumber.rs` |
| 生成物（`docs/protocol/*.schema.json`・`docs/api/v1/*.schema.json`） | target 側を採ってから設定のコマンドを argv で再実行。対象外の変更・失敗は人へ | 必要 | `auto_resolve/generated.rs` |
| main 追従の再試行（配送） | merge_base 系の失敗の前に scratch worktree で main を取り込み、解けたら候補 SHA を固定して `MergeQueued` に戻す。上限は `max_attempts`（既定 3） | 衝突なしの追従のみ・再 review が要る種類は従来経路へ落とす | `crates/celeris/src/delivery/auto_resolve.rs` |

## 人に統合を頼む条件と依頼の形

条件（ADR D3）: コードの内容衝突、同じ試験・箇所の重複した修正、自動解消後の gate 失敗、main 追従または局所修復の上限到達。記録の既存行変更・削除、曖昧な番号参照、生成コマンドの失敗・対象外変更も人へ回す。

依頼（`IntegrationRequest`、`crates/task-dispatch/src/auto_resolve.rs`）の欄: `source_branch` / `source_sha` / `target_branch` / `target_sha`、`merge_base`、`conflict_files`、各側の `intent`（merge_base..SHA の commit subject と file ごとの diffstat。git の事実だけから組む）、`reason` と `recommendation`（分類から決定的に引く）、`actions` と `candidate_sha`。依頼は段の統合（`integration.rs`→`dispatcher/phase_integration.rs` の `record_integration_request`）と配送（`delivery/auto_resolve.rs` の `record_request`）の両方から出る。配送では detail に `[needs-human] 統合の依頼: …` を記す。衝突 merge は abort して作業 tree を清潔に戻す。

## 配送と段の統合の入口

- 配送: `crates/celeris/src/delivery.rs`（`advance`、merge_base 系失敗の分岐。`auto_resolve::attempt` → `requeue` / `fall_back`）。従来の取り込みは `merge --ff-only`。
- 段の統合: `crates/task-dispatch/src/integration.rs`（`integrate`。衝突時は `auto_resolve::resolve`、`Resolved` なら解消 commit を作って次へ、`NeedsHuman` なら依頼を返す）。従来の `merge --no-ff` と衝突時の abort は残す。

## 設定

```toml
[delivery.auto_resolve]          # 既定 enabled = true, max_attempts = 3
[delivery.auto_resolve.generated]
globs = ["docs/protocol/*.schema.json", "docs/api/v1/*.schema.json"]   # resolver の分類と同じ集合だけ
cmd = ["env", "UPDATE_SCHEMA=1", "cargo", "test", "-p", "task-core", "-p", "task-worker", "-p", "task-api"]  # [] で無効
```

読み込みは `crates/celeris/src/config/delivery.rs`、例は `config/celeris.example.toml`（`# [delivery.auto_resolve]` の節）。

## 証拠（この close WU、基点 c40ceb22 の作業 tree）

- 条件: `cargo test --workspace` が通る
  - 実行: `cargo test --workspace`（log: `artifacts/test-workspace.log`）
  - 結果: exit 0。集計（`test result` 行の合計）は passed 3526、failed 0、ignored 13。
  - 負荷 flaky（e2e）: 今回の全体実行では `tests/e2e` と `browser_e2e` を含めて失敗なし。単体の再実行は不要と判断した。
- 条件: `cargo clippy --workspace -- -D warnings` が通る
  - 実行: `cargo clippy --workspace -- -D warnings`（log: `artifacts/clippy-workspace.log`）
  - 結果: exit 0（`Finished dev profile`、警告なし）。
- 条件: 進捗ファイルの索引が通る
  - 実行: `sh scripts/dev/progress-index.sh --check`
  - 結果: exit 0（`progress-index --check: ok`）。
- 条件: architecture-map の実在検査が通る
  - 実行: `python3 scripts/dev/check-architecture-map.py`
  - 結果: exit 0（`OK: 205 件のパスを確認した（docs/architecture-map.md）`）。追加した 2 行（task-dispatch の自動解消、celeris の配送の自動解消）の source path は実在する。

## 未解決事項

1. **受信箱への投影が未実装（ADR D4 との差）**: 統合の依頼は notice store に `BadNews`（`target.kind = integration_request`）として記録され、配送では detail に `[needs-human]` が出る。しかし `crates/task-ops/src/inbox.rs` は `integration_request` を見ておらず、受信箱の項目としては出ない。回答で受信箱から消す経路もない。ADR D4 が求める「受信箱へ一対一で投影し、未回答・回答済みを依頼側の状態で正とする」は未完。
2. **main 追従の再 review 判定**: 衝突なしの main 追従は従来経路へ落とす（`Fallback`）。ADR D5 の表どおりの扱いで、gate 再実行だけで進める経路は作っていない。
3. 番号だけの参照（`ADR-NNNN`）は移動後も人に回す（付記 3）。
4. `.gitattributes` の union は `agent-docs/` の記録だけに付いている。旧 path `docs/PROGRESS.md`・`docs/progress/` 宛ての移行期間の追記は union の対象外で、records resolver（配送・段の統合の衝突時）だけが扱う。

## 提案

- 受信箱の投影を次の葉にする: `task-ops::inbox` で `notice` の `target.kind = integration_request` を未回答の依頼として読み、回答（依頼 id に対する人の選択）で消す。試験は notice store と inbox を一時 DB で組んで確かめる。
- 依頼の一覧に依頼 id を振り、配送 detail と受信箱の両方から同じ id を指すようにする（二重掲載を避ける）。
- 本番への反映は人が行う: daemon の入れ替え（`celeris` の release と handoff）は本記録の範囲外。launcher 関連の変更（`task-worker/src/browser_launcher/`）は host の `celeris-browser-launcher` binary の入れ替えが要る（`2026-10-03-parallel-integration-auto-resolve/launcher-reap-flake.md` の運用節を参照）。

## 追記（sync-gate WU）: 受信箱投影の完了と main 取り込みゲート

未解決事項 1（受信箱への投影が未実装）は `2026-10-03-parallel-integration-auto-resolve/inbox-request.md` の WU で解消済み: `Event::IntegrationRequested`/`IntegrationAnswered` を追記事象にし（migration `0047_events_integration_request_index.sql`）、`task-ops::human_inbox` が未回答の依頼を `InboxKind::IntegrationRequest` として一対一で受信箱に投影する。回答は既存の `POST /api/v1/inbox/items/{id}/answer` で `IntegrationAnswered` を追記し、受信箱から消える（横断試験 `integration_request_answer_appends_event_and_removes_only_its_inbox_item`）。段の統合経路の依頼も v4 修正で `integration_request` 1 件だけを表示し、回答で統合 WU を再開する（詳細・残課題は同ファイルを参照。既存 notice の backfill と GUI 実画面確認は未実施のまま提案に残る）。

main（`0225c752`）をこのブランチへ `git merge --no-ff` で取り込み（merge commit `1fcb517d`）、取り込み後の tree で `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` を実行し、すべて exit 0（test 集計 passed 3663 / failed 0 / ignored 13）。衝突は `crates/task-core/src/store/migrations.rs`（main の migration 0046 とこのブランチの 0047 を両方残し `SCHEMA_VERSION=47`）・`crates/task-core/src/cluster_job/tests.rs`・`crates/task-core/src/store/tests.rs`（版数 assert を 47 に一本化）・`docs/architecture-map.md`（配送 auto_resolve の行と main の Knowledge GC 行を両方残す）の 4 件で、詳細と証拠は `2026-10-03-parallel-integration-auto-resolve/sync-gate.md` を参照。main 側の userns・db guard の変更とこのブランチの `browser_launcher/` の修正は同じ tree で共存することを確認した。

## 追記（union-scope / final-gate WU）: union の範囲修正と最終ゲート

final review（前回 attempt）が2点の欠陥を返した: (1) `.gitattributes` の `merge=union` が `agent-docs/progress/**/*.md`（task ごとの進捗ファイル。front matter の `status:`/`updated:` を含む）にも効いていたため、両側が同じ front matter 行を別の値に変えても `git merge` が exit 0 で終わり、records resolver に衝突が一切見えないまま front matter に同じ key を2つ持つファイルを黙って作っていた（ADR D1a が求める「union 結果の検査」も未実装だった）。再現: 一時 repo で `status: running→done` と `status: running→blocked` を両側で変えると union 属性のまま merge が通り、front matter に `status:` が2行残る。(2) commit `e49b57cb`（inbox-request 子 task）が repo 根に `artifacts/review.json`（Celeris 自身の判定材料）を追跡ファイルとして入れていた。

union-scope WU（commit `7effd2f8`）で両方を修正した: `.gitattributes` の `merge=union` を `agent-docs/PROGRESS.md` と `docs/PROGRESS.md` の2行（凍結済みの追記専用ファイル）だけに限り、`agent-docs/progress/**/*.md` からは外した（既存行の変更は通常の3-way merge の実衝突になり、records resolver が変更なしで人へ回す）。`artifacts/review.json` を `git rm --cached` し、`.gitignore` に `/artifacts/` が無かった漏れを塞いだ。ADR `agent-docs/adr/2026-10-02-parallel-integration-auto-resolve.md` の D1a を実装に合わせて書き直し、末尾に付記（union の範囲）を足した。試験 `sh scripts/dev/tests/progress_union_merge.sh` を新設し、union 範囲の限定と front matter 衝突が黙って統合されないことの両方を固定した。

final-gate WU で union-scope 後の HEAD（`7effd2f8`）に対し: `git fetch origin main` は `0225c752`（sync-gate 時点と同じ、main は未前進）で `git merge-base --is-ancestor main HEAD` が exit 0 のため main は既に祖先、`git merge-tree --write-tree --name-only HEAD main` も衝突ファイルを出さず merge は不要だった（sha をそのまま記録）。取り込み後の tree（= 現 HEAD）で `cargo fmt --all -- --check` / `cargo clippy --workspace -- -D warnings` / `cargo test --workspace`（passed 3664 / failed 0、断続 flaky なし）/ `sh scripts/dev/check-doc-links.sh` / `sh scripts/dev/check-adr-numbers.sh` / `sh scripts/dev/progress-index.sh --check` / `sh scripts/dev/tests/progress_union_merge.sh` をすべて実行し、すべて exit 0。詳細は `2026-10-03-parallel-integration-auto-resolve/final-gate.md` を参照。

## 追記（close-recheck WU）: 差し戻し 2 点の再確認

最新 main（`0225c752`、HEAD の祖先）上で差し戻し 2 点（union の範囲・`artifacts/review.json` の誤追跡）の解消を再確認し、ゲート（fmt・clippy -D warnings・`cargo test --workspace` passed 3664 / failed 0・文書検査 3 本・union 試験）を全件 exit 0 で記録した。対応表と証拠は `2026-10-03-parallel-integration-auto-resolve/close-recheck.md` を参照。
