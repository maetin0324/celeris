---
title: アーキ v4 F knowledge-pin — record の pin を write_tx で書き、record_op を org_write の形へ移す
tasks: [01M7MZTKB8A4NJQEEA35Q4G5JV]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# knowledge-pin（WorkUnit）

完了日: 2026-11-19。ADR [2026-11-18-knowledge-lifecycle-tx-boundary](../../adr/2026-11-18-knowledge-lifecycle-tx-boundary.md) の D1・D3 を実装した。
基準は `$CELERIS_WU_BASE`（provenance-read の後の tree）。検査は提出時の HEAD で実行した。

## 変更（commit の件名）

- `arch-v4 F knowledge-pin: record_op を record_candidate・pin_candidate_tx・discard_candidate に分け、HTTP は write_tx、CoS は reject・apply_checked で pin する`
- `arch-v4 F knowledge-pin: 進捗を書く`（この file）

### 消したもの

- `crates/task-api/src/knowledge.rs` の `record_op`（`attachment_db: Option<&Path>` と `audit: Option<&OperationAudit>` を受け、`Applied` を返す関数）。
- その中の自前の接続: 添付の DB の file を開き、busy の待ちを 10 秒に置き、IMMEDIATE の transaction を自分で始めて commit する 10 行。
  `knowledge.rs` の `rusqlite` を含む行は 4 → 1（残る 1 行は `pin_candidate_tx` の引数 `&rusqlite::Transaction<'_>`）。
- `record_op` の CoS の腕の `failure` の退避（`audit.apply` の closure の `ChatError::Invalid` を経由して `ApiProblem` を取り出す処理）。`apply_checked` が同じことをする。
- `record` handler の `state.chat.attachment_db_path.clone()`（Path の写し）と `.direct()`。

### 足したもの・変えたもの

- `record_candidate(store, root, attachment_storage: bool, body) -> Result<KnowledgeRecordResult, ApiProblem>`（`pub(crate)`）: `require_kb`・`page_path`・
  添付の保存の有無の検査・`project_refs`・`layout`・`record_in`・結果の組み立て。監査を受けず、`Applied` を返さない。
- `discard_candidate(root, id, problem) -> ApiProblem`（`pub(crate)`）: 旧 `record_op` の closure `discard` をそのまま関数にした。
- `pin_candidate_tx` を private から `pub(crate)` にした（中身は変えていない）。
- HTTP の `record`: `record_candidate` → `attachment_ids` があれば `store.write_tx(|tx| pin_candidate_tx(tx, .., None))` → 失敗なら `discard_candidate`。
- CoS の `knowledge.record`（`crates/task-api/src/cos/ops/decisions.rs`）: `record_candidate(store, &root, true, input)` の失敗は `audit.reject(store, "knowledge", "new", ..)`、
  成功したら `audit.apply_checked(store, "knowledge", &id, "knowledge.record", |tx| pin_candidate_tx(tx, .., Some(thread)) の後に結果の JSON)`、
  失敗なら `discard_candidate`。全体を今と同じく `counted_record` で包む。腕は `CosOperation` をそのまま返す（`audited(Applied)` を通さない）。
- 新しい trait・Port・flag・fallback・状態は無い。`SqliteStore` の method は増えない（task-core の差分 0）。

### ADR D1 との違い（レビューで見てほしい点）

ADR D1 は `attachments_unavailable` とその分岐を消すと書いたが、この WorkUnit の objective は「添付の保存が無効で `attachment_ids` があるときの 503
`attachments_unavailable` は保つ（Path の引数は有無の bool で足りる）」と決めている。objective に従い、`record_candidate` の `attachment_storage: bool`
で保った。検査の順（KB → path → 添付の保存 → 案件の一覧 → 記録）は旧 `record_op` と同じ。CoS は旧コードでも検査しなかった（`audit = Some` のとき飛ばした）ので
`true` を渡す。ADR の本文は close の unit の付記で直す見込み（提案 1）。

### 応答の違い

- HTTP: 成功（201 と `KnowledgeRecordResult`）、検証の拒否（422 `invalid_attachment` など）、pin の書き込みの失敗（500 `internal`）、
  添付の保存が無効なときの 503 `attachments_unavailable` は変わらない。書き込みの接続を取れないとき（busy）は、旧の 500 `internal`（`attachment pin: …`）から
  store の写像（`SQLITE_BUSY` は 503 `db_busy`）になる（ADR D1 のとおり。`write_tx` を使う他の route と同じ）。
- CoS: 応答・状態 code・problem の type は変わらない。`serde_json::to_value` の失敗（`KnowledgeRecordResult` では起きない）だけが `ApiProblem::internal` になる（ADR D3）。

### `state.chat.attachment_db_path` を読み続ける所（提出時の HEAD）

| 所 | 用途 |
|---|---|
| `crates/task-api/src/knowledge.rs:696`（`inbox`）・`:882`（`inbox_detail`） | 有無の旗。偽なら provenance を空にする（provenance-read） |
| `crates/task-api/src/knowledge.rs:735`（`record`） | 有無の旗。偽で `attachment_ids` があれば 503 `attachments_unavailable` |
| `crates/task-api/src/chat/attachments.rs:80・122・243・387` | path として使う。`ChatAttachmentStore`（blob の file と DB の行）の経路。ADR D5 の範囲外 |
| `crates/task-api/src/cos/inbox.rs:886` | path として使う。ADR D5 の範囲外 |
| `crates/task-api/src/state.rs:177` | 値を入れる所（`Some(settings.db_path)`） |

## 行数（`git diff --stat` と `wc -l`、`$CELERIS_WU_BASE` → HEAD）

| file | base | HEAD | 差 |
|---|---|---|---|
| `crates/task-api/src/knowledge.rs` | 1107 | 1087 | −20 |
| `crates/task-api/src/cos/ops/decisions.rs` | 237 | 255 | +18 |
| `agent-docs/adr/2026-10-07-cos-live-fixes.md` | 147 | 168 | +21（付記） |

production の Rust は 2 file で −2 行。行数の純減は F の受け入れ条件に無い（ADR D5）。共通受入条件 (6) の計測は metrics の unit が A と同じ command で行う。

## 検査（提出時の HEAD で実行）

| 条件 | command | 結果 |
|---|---|---|
| 0 | `grep -nE 'Connection::open\|transaction_with_behavior\|TransactionBehavior\|busy_timeout\|record_op' crates/task-api/src/knowledge.rs` | 出力なし（exit 1） |
| 1 | `cargo nextest run -p task-api --test cos_delivery_suite ... -E 'test(tx_boundary_contract) or binary(cos_live_fix_d2) or binary(cos_chat_attach_pin) or binary(knowledge) or test(cos_chat_attach_handoff_kb) or test(knowledge)'` | 27 passed。`tx_boundary_contract` の 8 件（`admin_record_pins_attachments_and_serves_provenance`・`cos_record_writes_audit_and_reference_in_one_transaction`・`busy_store_fails_record_without_residue_and_retry_succeeds`・`invalid_attachment_record_leaves_no_candidate_and_no_reference` ほか）を含む。`crates/task-api/tests/` の差分 0 |
| 2 | `bash scripts/dev/test-affected.sh` | exit 0（769 秒）。nextest 6121 passed・21 skipped、web の tsc -b と vitest 705 passed、repo-health verdict=pass（new=0 worsened=0） |
| 2 | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 2 | `cargo fmt --all -- --check` | exit 0 |
| 3 | `python3 scripts/dev/repo-health/repo_health.py --repo "$(pwd)" --base "$CELERIS_WU_BASE" --head worktree --policy-version 1 --out <json>` | exit 0。`deps.cycle` pass、`size.responsibility-growth` pass |
| 4 | `grep -n '^\| D2 \|' agent-docs/adr/2026-10-07-cos-live-fixes.md \| grep -c record_op` | 0。末尾に「付記: record の pin と監査の transaction を既存の owner へ寄せる（2026-11-19）」があり、F の ADR へ link する |
| 範囲 | `sh "$CELERIS_WU_SCOPE_PATHS"` | `agent-docs/adr/2026-10-07-cos-live-fixes.md`・`crates/task-api/src/cos/ops/decisions.rs`・`crates/task-api/src/knowledge.rs`（と、この file） |

リリースゲート: 未実施（統合の後の release の gate で走る）。本番への昇格はしていない。

## 回帰条件

- `tx_boundary_contract` の record の 4 件と `cos_live_fix_d2` の 3 件が落ちたら、この変更を revert する（旧新を並存させていないので、戻すのは git の revert だけ）。

## 未解決事項

- `agent-docs/adr/2026-11-18-org-write-shared-use-case.md:210` の表（D の付記の 2 本の command の一覧）は `knowledge.rs` の行に `record_op` を挙げたまま。
  `knowledge.rs` は `accept_op`・`reject_op`・`put_page_op` のために一覧に残る。この unit の範囲の外なので変えていない。

## 提案

1. close の unit が F の ADR の D1 に付記を足し、「`attachments_unavailable` は WorkUnit の objective で保った（`record_candidate` の `attachment_storage`）」と書く。
2. 同じく close の unit が org-write の ADR の 210 行の `record_op` を外す（関数の単位の残数を数える子 H の数え方に合わせる）。
