---
title: 統合の依頼の受信箱投影: 横断試験・記録・全体検査
tasks: [01M3ZCXNXTRTA9GNJ52Q36ZFCP]
status: needs-human
updated: 2026-10-03
---

# 統合の依頼の受信箱投影: 横断試験・記録・全体検査

対象 ADR: `agent-docs/adr/2026-10-02-parallel-integration-auto-resolve.md`（D4 と D4 付記）。
branch: `celeris-wu/01M412QGAJR1252XHBE1WT62CD/record`、HEAD `22c5cc8f`、基点 `af71d4e9`（main）。

## 実装の対応（ADR → commit・ファイル）

- **D4 付記 事象の追記（core-events）**: `Event::IntegrationRequested` / `IntegrationAnswered` を追記事象として持つ。
  - commit `4f739764`, `809f555d`, `92a3658b`, `99bb916d`, `d012cd31`（schema 再生成）, `1cc577be`（schema-fix）, `9c2c7b49`, `822a6fd5`（schema-sync）
  - ファイル: `crates/task-core/src/integration_request.rs`, `crates/task-core/src/store/events.rs`, `crates/task-core/src/model.rs`
- **migration**: `0047_events_integration_request_index.sql`。`json_extract(json,'$.type') IN ('integration_requested','integration_answered')` の部分 index（D4 付記どおり）。
- **record（書き手）**: 依頼の書き手を notice から追記事象へ。`task-dispatch`（phase_integration）と `celeris` delivery。同じ `source_sha`・`target_sha` の未回答依頼は再記録しても増えない。
  - commit `f173c0fd`
  - ファイル: `crates/task-dispatch/src/integration.rs`, `crates/task-dispatch/src/dispatcher/phase_integration.rs`, `crates/celeris/src/delivery.rs`
- **ops-inbox（受信箱投影）**: 未回答依頼を `AttentionItem::IntegrationRequest` → `InboxKind::IntegrationRequest`（wire 名 `integration_request`）へ一対一で投影。
  - commit `86ef0cbe`
  - ファイル: `crates/task-ops/src/human_inbox.rs`, `crates/task-api/src/inbox_notifications.rs`
- **api-answer（回答）**: 既存 `POST /api/v1/inbox/items/{id}/answer` で回答し、`IntegrationAnswered` を追記して受信箱から消す。
  - commit `a49c2158`, `3eda2b3b`（integrate）
- **cross-test（横断試験）**: `integration_request_answer_appends_event_and_removes_only_its_inbox_item`。記録 → 受信箱 1 件 → 同じ head の再記録（戻り値 false、項目 1 件、`IntegrationRequested` 1 件のまま）→ 通知に出ない → 回答で項目が消え、未知 id は 404。
  - commit `22c5cc8f`
  - ファイル: `crates/task-api/tests/inbox_notifications.rs`
- **自動解消（D1〜D3, D5）**: 同じ branch の先行 commit。`crates/task-dispatch/src/auto_resolve*`、`crates/celeris/src/delivery/auto_resolve.rs`、`config/celeris.example.toml` の `[delivery.auto_resolve]`。このタスクでは手を入れていない。

## 証拠（このタスクで実行した結果）

すべて worktree（branch HEAD `22c5cc8f`）で実行。ログは `artifacts/` の下。

| 条件 | コマンド | 結果 |
|---|---|---|
| 全体の試験 | `cargo test --workspace --exclude e2e` | exit 0。合計 passed 3484 / failed 0 / ignored 13 |
| 横断試験 | `cargo test -p task-api --test inbox_notifications` | exit 0。7 passed（横断試験を含む） |
| lint | `cargo clippy --workspace -- -D warnings` | exit 0 |
| e2e の build | `cargo test -p e2e --no-run` | exit 0（実行はしない。下の未解決事項を参照） |
| schema 差分ゼロ | `UPDATE_SCHEMA=1 cargo test --workspace --exclude e2e` の後に `git status --short` | exit 0、試験 failed 0。`docs/`・`gui/` に差分なし |
| 文書リンク | `sh scripts/dev/check-doc-links.sh`（`--self-test` も exit 0） | exit 0、`check-doc-links: ok` |
| ADR 番号 | `sh scripts/dev/check-adr-numbers.sh` | exit 0、`ok (125 files)` |
| 文書配置 | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0、`check-doc-layout: ok` |
| 進捗索引 | `sh scripts/dev/progress-index.sh --check` | exit 0 |
| 構成図 | `python3 scripts/dev/check-architecture-map.py` | exit 0 |
| PROGRESS 不変 | `git diff --quiet HEAD -- agent-docs/PROGRESS.md` と `git diff --quiet $(git merge-base HEAD main) HEAD -- agent-docs/PROGRESS.md` | 両方 exit 0 |

注記: 文書配置の検査は引数（manifest の tsv）を付けて実行した。引数なしの初回実行は usage を出して exit 1 になったが、これは呼び方の誤りで、検査の失敗ではない。

全体検査で落ちたものは無い。

## 未解決事項

1. **既存の notice の扱い**: 配置を変える前に `NoticeKind::Delivery`（target kind `integration_request`）で記録された依頼は notice store に残っている。新しい経路はそれを移さず、受信箱にも投影しない。未回答の古い依頼を受信箱へ移す backfill は行っていない。本番 DB は見ていないので、残っている件数と通知一覧への表示は未確認。
2. **GUI の表示**: 受信箱の `integration_request` 項目の実画面での確認はしていない。型と fixture の更新は branch の commit に入っている。
3. **e2e の実行**: e2e crate は worker sandbox で user namespace を作れず EPERM になる試験を含むため、ここでは build だけにした。実行は daemon の workspace check に任せる。
4. **gui の型生成**: `pnpm gen:types` は実行していない（`gui/node_modules` が無く、install はしない方針）。入力の `docs/api/v1/api-v1.schema.json` は再生成しても差分ゼロなので、`gui/app/celeris/types.ts` は branch の commit の内容のままである。

## 提案

- 1 の既存 notice を新しい受信箱へ移す（または古い依頼を「解決済み」として閉じる）ことを、小さな別 task にする。本番 DB の確認を含めて人が判断する。
- 受信箱の `integration_request` 項目の実画面確認（GUI の表示と回答ボタン）を、daemon の workspace check か人の確認に入れる。
- `gui/` の `gen:types` を、node_modules のある環境で一度実行し、差分ゼロを確かめる。

## v4: 段の統合経路も一対一の受信箱項目にする

最終レビューで、段の統合経路が `IntegrationRequested` に加えて `integration_needs_human` 経由の `QuestionRaised` / `WorkerQuestion` も記録し、同じ依頼を受信箱に二重表示する欠陥が見つかった。統合依頼への回答だけでは question 項目が残り、統合 WU も再開しなかった。

修正を次のように分担した（fix-adr / fix-phase / fix-answer の各 WorkUnit）。

- ADR D4 付記を更新し、段の統合依頼は `integration_request` 1 件だけを表示し、回答で統合 WU を再開する規則を明記。
- 段の統合依頼の `origin` を固定値 `phase:merge` ではなく統合 WU の key に結び付け、通常の question/approval を追加しない。
- 統合依頼への回答を受けた task-api が対応する統合 WU を再開する。
- 回帰試験で段の統合経路の受信箱件数・回答後の項目消失と WU 再開を確認。

### v4 の検査

この close WorkUnit の HEAD `ed96f74c` で実行した。

| 条件 | コマンド | 結果 |
|---|---|---|
| 全体試験（e2e を除く） | `cargo test --workspace --exclude e2e` | **失敗**。`production_h3_injects_once_without_exposure` が `unshare: Operation not permitted` で失敗（sandbox の user namespace 制約）。他の指定検査は失敗後に実行していない。 |
| e2e build | `cargo test -p e2e --no-run` | 未実行。全体試験の失敗で停止。 |
| clippy | `cargo clippy --workspace -- -D warnings` | 未実行。全体試験の失敗で停止。 |
| 文書検査 | `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh`、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`、`sh scripts/dev/progress-index.sh --check` | 未実行。全体試験の失敗で停止。 |

全体検査の成功は確認できていない。sandbox 外の適切な検証環境で失敗試験を含む検査を再実行する必要がある。
