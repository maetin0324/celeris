---
title: アーキ v4 H R4 flag — ChatState.attachment_db_path を欄ごと消し、knowledge.rs の旗を attachment_data_dir から導く
tasks: [01M7TWBX8YYDKHV6C2JWDJS1VR]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# flag（WorkUnit）

完了日: 2026-11-20

## 0. 結論

- `crates/task-api/src/chat/mod.rs` の `ChatState.attachment_db_path`（欄・doc comment・`Default` の行）と、
  `crates/task-api/src/state.rs` の `ApiState::new` の `Some(settings.db_path.clone())` の代入を消した。同じ DB を指す値は
  `Inner.db_path` の 1 つになった。
- `ChatState::attachments_enabled()`（`attachment_data_dir.is_some()`）を 1 つ置き、`crates/task-api/src/knowledge.rs` の
  `inbox`・`record`・`inbox_detail` の 3 か所の旗をこれに替えた（ADR 2026-11-19-chat-attachment-store-connection D4）。
- 反例試験 2 件（`attachment_flag_*`）を `crates/task-api/tests/cos_delivery_suite/attachment_flag_contract.rs` に足した。
- `record` の `write_tx`（`pin_candidate_tx`）と `discard_candidate` の境界は変えていない。knowledge.rs に `Connection::open`・
  `TransactionBehavior`・`busy_timeout` を足していない。API・CLI・DB の契約は変えていない。
- ADR に付記「2026-11-20（WorkUnit `flag`）: D4 の実装」を足した。

## 1. 実装前 brief

1. 根本原因: `ApiState::new` が `ChatState.attachment_db_path` に常に `Some(db_path)` を入れ、`Inner.db_path` と同じ DB を
   2 つ目の値として持っていた（`dual_source_of_truth`）。knowledge.rs はこれを添付の保存の有無の旗に使い、旗が常に真なので
   api.md の「添付の保存が無効なら `provenance[]` は空」「無効で `attachment_ids` を渡せば 503 `attachments_unavailable`」を通らなかった。
2. 責務 owner: `crates/task-api/src/chat/mod.rs` の `ChatState`（添付の設定の正本）。
3. 再利用先: `attachment_data_dir`（`with_chat_attachments` が入れ、`chat/attachments.rs` の 503 が既に使う）。試験は `admin_env`・
   `project_cos_thread`・`assert_problem` と F の `tx_boundary_contract.rs` の helper。
4. 選択肢: (a) knowledge.rs の 3 か所に `attachment_data_dir.is_some()` を直に書く、(b) `ChatState` に判定を 1 つ置く。
   ADR D4 と objective の例どおり (b) を採った（判定の場所が 1 つになる）。
5. 新しい状態・分岐・fallback・port: 無し（`attachments_enabled` は既存の欄から導く `pub(crate)` の method）。
6. 撤去対象: 欄 `attachment_db_path`・その doc comment・`Default` の行、`ApiState::new` の代入、knowledge.rs の 3 か所の読み取り。
   `docs/`・`web/`・`scripts/` には参照が無かった（grep で確認）。
7. 不変条件: write_tx の境界（`pin_candidate_tx`・`discard_candidate`）は変えない → `tx_boundary_contract` の 8 件。
   旗は `attachment_data_dir` から導く → `attachment_flag_*` の 2 件。

## 2. 変えた file

| file | 変更 |
|---|---|
| `crates/task-api/src/chat/mod.rs` | 欄 `attachment_db_path` と `Default` の行を消し、`attachment_data_dir` の doc と `attachments_enabled()` を足した |
| `crates/task-api/src/state.rs` | `ApiState::new` の `ChatState { attachment_db_path: Some(..), ..Default::default() }` を消し、`ChatState::default()` を入れる |
| `crates/task-api/src/knowledge.rs` | 3 か所の旗を `state.chat.attachments_enabled()` に替え、`candidate_provenance`・`record_candidate` の doc に判定の出所を書いた |
| `crates/task-api/tests/cos_delivery_suite/attachment_flag_contract.rs` | 新規。反例試験 2 件 |
| `crates/task-api/tests/cos_delivery_suite/main.rs` | `mod attachment_flag_contract;` |
| `crates/task-api/tests/cos_delivery_suite/tx_boundary_contract.rs` | helper 6 つ（`INBOX`・`upload`・`record_body`・`inbox_files`・`inbox_refs`・`expires_at`）を `pub(crate)` にした。試験の中身は変えていない |

## 3. 足した試験（守る不変条件と、既存の試験で検出できない理由）

同じ DB・同じ KB を指す 2 つの router（`admin_env` の state と、その clone に `with_chat_attachments` を足した state）で同じ候補を読む。

| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `attachment_flag_no_data_dir_hides_provenance_and_refuses_ids` | 添付の保存が無効なら、pin の行を持つ候補でも一覧と詳細の `provenance` が空。`attachment_ids` つきの record は 503 `attachments_unavailable` で、`_inbox/` の file・参照行・添付の `expires_at` が変わらない。`attachment_ids` の無い record は 201 | 旗は常に `Some` の `attachment_db_path` から読んでいたので、無効の側を通る試験が無かった |
| `attachment_flag_data_dir_shows_provenance` | 添付の保存が有効なら、record が 201 で pin の行を 1 行作り、一覧と詳細の `provenance` にその添付が 1 件出る（attachment_id・thread_id） | K1 などは有効の側だけを見て、同じ候補を無効の state から読む試験と対にしていない。判定を偽に倒しすぎる変更は上の試験だけでは検出できない |

反例の確認（試験が誤りを検出すること。確認の後に元に戻した。commit には含まない）:

- `attachments_enabled` を `true`（旧い挙動）に替える → `attachment_flag_no_data_dir_hides_provenance_and_refuses_ids` が FAIL（1 passed・1 failed）。
- `attachments_enabled` を `false` に替える → 2 件とも FAIL（0 passed・2 failed）。

## 4. 受け入れ条件と検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

| 条件 | コマンド | 結果 |
|---|---|---|
| 0. crates・docs・web・scripts に `attachment_db_path` が無い | `! git grep -n 'attachment_db_path' -- crates docs web scripts` | 該当なし（exit 0） |
| 1. task-api の chat/・cos/・knowledge.rs に `Connection::open`・`TransactionBehavior`・`busy_timeout` が無い | `! git grep -nE 'Connection::open\|TransactionBehavior\|busy_timeout' -- crates/task-api/src/knowledge.rs crates/task-api/src/chat/ crates/task-api/src/cos/` | 該当なし（exit 0） |
| 2. `attachment_flag_` が pass | `cargo nextest run --no-tests=fail -p task-api -E 'test(attachment_flag_)'` | exit 0、2 passed・894 skipped |
| 2. `tx_boundary_contract` が pass | `cargo nextest run -p task-api --test cos_delivery_suite tx_boundary_contract` | exit 0、8 passed・30 skipped |
| 2. fmt | `cargo fmt --all -- --check` | exit 0 |
| 2. clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 2. test-affected | `bash scripts/dev/test-affected.sh` | exit 0。対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker と web（typecheck+unit）。nextest 6138 passed・21 skipped、web の unit 705 passed、repo-health verdict=pass（new=0・worsened=0） |
| 範囲 check | `sh "$CELERIS_WU_SCOPE_PATHS"` の path を FLAG_RE に当てる | 範囲外の path なし |

commit（件名）:

- arch-v4 H R4 flag: ChatState.attachment_db_path を消し、knowledge.rs の旗を attachment_data_dir から導く（attachment_flag_ の反例試験）
- arch-v4 H R4 flag: 進捗 flag.md と ADR の付記を書く
- arch-v4 H R4 flag: 反例試験の doc comment から消した欄の名前を外す（grep の check）

最初の提出前の確認で、条件 0 の grep が新しい試験の doc comment に書いた旧い欄の名前に当たって落ちた。comment から識別子を
外した後、上の表の検査（grep・2 つの試験・fmt・clippy・test-affected・範囲 check）をすべて走らせ直し、同じ結果を得た。

## 5. 未解決事項

- 無し。after の測定（metrics.tsv の after）と進捗 `2026-11-19-arch-v4-h-r4.md` の完了は WorkUnit `after` が行う。
- release gate（`scripts/selfdeploy/release.sh`）は未実施。本番へは昇格しない。

## 6. 提案

- 無し。
