---
title: アーキ v4 H R4 boundary — 高 risk の 5 file の変更を ADR 付記に書き、不変条件の試験を走らせる
tasks: [01M7TWBX8YYDKHV6C2JWDJS1VR]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# boundary（WorkUnit）

完了日: 2026-11-20

## 0. 結論

- task の差分が変えた高 risk の class の 5 file（authz: `crates/task-api/src/cos/operations.rs`・`crates/task-api/src/handlers/authority.rs`、
  credential: `crates/task-api/src/handlers/secrets.rs`、selfdeploy: `crates/task-api/src/releases.rs`、transaction: `crates/task-api/src/knowledge.rs`）の
  class・不変条件・変更の出所（R2 か R4 か）・中身・試験を [ADR の付記](../../adr/2026-11-19-chat-attachment-store-connection.md)
  （「付記 2026-11-20（WorkUnit `boundary`）」）に表で書いた。
- 5 file の `git merge-base HEAD main` からの差分は import・型の path・`cluster_ids()`・旗の読み（`attachments_enabled`）・doc comment だけだった
  （差分の形の検査の出力が空、exit 0）。
- 不変条件の試験は 316 件すべて通った（exit 0、12 binary、skipped 2）。`crates/` は変えていない。repair の申告は無い。

## 1. 経緯（計画 v3 の理由）

v2 の計画は write path を dir で書いた。計画の設計 review は高 risk の class の path に当たらず、final review の health の `diff_design` が
「設計 review が覆わない境界の変更（reviewer run が無く判定できない）」として 5 file と 6 つの不変条件を挙げて落ちた（他の受け入れ条件は pass）。
v3 の計画はこの WorkUnit の `context.paths` に 5 file を file 単位で書き、計画の設計 review に覆わせる。

## 2. 5 file の差分（`git merge-base HEAD main` → HEAD）

| file | +/− | 出所 | commit（件名） |
|---|---|---|---|
| `crates/task-api/src/cos/operations.rs` | +4 −13 | R2 | `arch-v4 H R2 http-tools: HTTP の道具を handlers.rs から http_io.rs へ移し、cluster_ids を ApiState の method にする`、`arch-v4 H R2 accounts: account の plan と作用を account_write.rs へ、Effect を problem.rs へ移す` |
| `crates/task-api/src/handlers/authority.rs` | +3 −0 | R2 | 同上（http-tools） |
| `crates/task-api/src/handlers/secrets.rs` | +2 −1 | R2 | 同上（http-tools） |
| `crates/task-api/src/releases.rs` | +9 −9 | R2 | 同上（http-tools・accounts） |
| `crates/task-api/src/knowledge.rs` | +9 −8 | R2（import +1 −1）と R4（+8 −7） | 同上（http-tools）、`arch-v4 H R4 flag: ChatState.attachment_db_path を消し、knowledge.rs の旗を attachment_data_dir から導く（attachment_flag_ の反例試験）` |

R4 の knowledge.rs の +8 −7 は、`inbox`・`record`・`inbox_detail` の 3 か所の `state.chat.attachment_db_path.is_some()` →
`state.chat.attachments_enabled()`（3 行）と、`candidate_provenance`（−1 +2）・`record_candidate`（−3 +3）の doc comment。
`record_candidate`・`pin_candidate_tx`・`discard_candidate` の本体と transaction の境界は差分に無い。

## 3. 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、WorkUnit の範囲は `$CELERIS_WU_BASE`）

- 差分の形の検査（ADR の付記の 2 本の `git diff -U0 … | grep`。WorkUnit の check と同じ）→ 出力 0 行、exit 0。
- `cargo nextest run -p task-api --lib --test run_worker_suite --test authority_ledger --test knowledge --test cos_operations --test auth_and_guards --test cos_delivery_suite --test cos_delivery_suite_a --test llm_accounts_admin --test cos_triage --test chat_api --test cos_chat_attach_pin`
  → exit 0。`Starting 316 tests across 12 binaries (2 tests skipped)`、`Summary [13.785s] 316 tests run: 316 passed, 2 skipped`。
  skipped の 2 件は lib の `#[ignore]` の計測の試験で、R2 の boundary.md と同じ 2 件。
- ADR の内容の検査（付記の見出しと、5 file・6 つの不変条件・試験名・`attachments_enabled`・R2 の task id の文字列）→ exit 0。
- doc の検査・範囲の検査・test-affected・clippy・fmt の結果は下の §5。

binary ごとの件数（`cargo nextest list` の同じ引数で数えた。ignored を除く）:

| binary | 件数 | うち不変条件に当てた module |
|---|---:|---|
| lib（`task_api`） | 101 | `releases::tests` 2・`secrets::tests` 7・`knowledge::` 1・`cos::operations::` 1・`principal::tests` 3・`middleware::tests` 3 |
| `run_worker_suite` | 15 | `secrets_admin` 12 |
| `authority_ledger` | 22 | 全件 |
| `knowledge` | 10 | 全件 |
| `cos_operations` | 8 | 全件 |
| `auth_and_guards` | 11 | 全件 |
| `cos_delivery_suite` | 38 | `tx_boundary_contract` 8・`attachment_flag_contract`（`attachment_flag_`）2・`cos_ops_admin_config` 6・`org_write_contract` 6 |
| `cos_delivery_suite_a` | 54 | `attach_conn_` 3 |
| `llm_accounts_admin` | 19 | 全件 |
| `cos_triage` | 18 | `triage_conn_` 1 |
| `chat_api` | 19 | 全件 |
| `cos_chat_attach_pin` | 1 | 全件 |
| 計 | 316 | |

## 4. repair の申告

無し。試験と差分の形の check はどちらも通った。

## 5. 文書・範囲・repo の検査（提出時の HEAD で実行）

- `sh scripts/dev/check-doc-links.sh`（ok）・`sh scripts/dev/check-adr-numbers.sh`（ok、464 files）・`sh scripts/dev/progress-index.sh --check`（ok）→ いずれも exit 0。
- 範囲の検査（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ 変更は ADR・進捗・この file の 3 path だけ。範囲外は 0 件。`crates/` の変更は 0。
- `bash scripts/dev/test-affected.sh` → exit 0（WorkUnit の範囲は docs だけなので「試験対象の変更なし（docs のみ等）」、repo_health_gate: all checks passed）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- release gate は未実施（R4 の進捗の「リリースゲート」のとおり。本番へは昇格していない）。

## 6. commit（件名）

- arch-v4 H R4 boundary: 高 risk の 5 file の変更と不変条件を ADR 付記と boundary.md に書く
- arch-v4 H R4 boundary: 検査の結果を boundary.md と進捗に書く
