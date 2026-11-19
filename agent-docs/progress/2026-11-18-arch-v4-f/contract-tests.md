---
title: アーキ v4 F contract-tests — 知識の記録と案件の中止の tx 境界の契約・反例試験を旧コードに当てて固定する
tasks: [01M7MZTKB8A4NJQEEA35Q4G5JV]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# contract-tests（WorkUnit）

基準は `$CELERIS_WU_BASE`。`crates/task-api/src`・`crates/task-ops/src`・`crates/task-core/src` は変えていない（差分 0 行）。

## 変更
- `arch-v4 F contract-tests: 知識の記録と案件の中止の tx 境界の契約・反例試験を旧コードに当てて固定する` —
  `crates/task-api/tests/cos_delivery_suite/tx_boundary_contract.rs`（async の試験 8 件）を足し、
  `cos_delivery_suite/main.rs` に `mod tx_boundary_contract;` を 1 行足した（新しい test binary は作らない）。
- 書き込みは公開の経路（`/api/v1/knowledge/inbox`・`/api/v1/projects/{id}/cancel`・`/api/v1/cos/operations`）だけで行う。
  DB は rusqlite で読むだけ。tx の途中の失敗は試験が置く SQLite の trigger `contract_fault`（cos_operations への
  applied の行の挿入を RAISE(ABORT) で落とす）で起こす。busy は試験の別接続が `BEGIN IMMEDIATE` で持つ lock で起こし、
  要求を lock を持つ間に送り、lock を外してから再試行する（合否は lock の有無で決まり、待ち時間に依らない）。
- 添付の設定は `cos_live_fix_d2.rs`・`cos_chat_attach_pin.rs` と同じ（`with_chat_attachments` と HTTP の multipart の upload、
  CoS の bearer は `common::cos_ops::cos_run_bearer`）。busy の試験だけは API の state を busy_timeout 300 ms で作り直す。
- 各試験の不変条件と既存の試験で検出できない理由の全文は file の先頭の comment にある。

| 試験（`tx_boundary_contract::` の下） | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `admin_record_pins_attachments_and_serves_provenance`（K1） | admin の HTTP の record が 201 と応答の欄（attachment_ids・id・path・scope・sha・target）を返し、添付ごとに `chat_attachment_refs`（owner_kind `knowledge_inbox`）を 1 行書き、`expires_at` を消す。詳細と一覧の provenance が name・media_type・size_bytes・sha256・thread_id・pinned_at（参照行の created_at）・発言の message_id と request_text を返す | `cos_live_fix_d2_api_creates_candidate_with_provenance` は発言に載せない添付 1 件で attachment_id・thread_id・name だけを見る。参照行・`expires_at`・発言の結合・sha256・pinned_at を見ない |
| `invalid_attachment_record_leaves_no_candidate_and_no_reference`（K2） | 正しい添付と不正な添付（存在しない id・重複・ULID でない id）を並べた record は 422 `invalid_attachment`（type `urn:celeris:problem:invalid_attachment`）で、候補・参照行を残さず、正しい添付の `expires_at` も変えない | `cos_live_fix_d2_invalid_scope_is_422_and_writes_nothing` は存在しない id 1 件だけで `_inbox/` の file だけを見る。正しい添付を先に pin する変更を検出できない |
| `cos_record_writes_audit_and_reference_in_one_transaction`（K3） | CoS の `knowledge.record` の成功で applied の行・監査 event・参照行が揃う。applied の行の挿入が落ちると参照行・`expires_at` の変更・候補が残らず rejected の行だけが残る。他 thread の添付は 422 と rejected の行で、参照行も候補も残らない | `cos_live_fix_d2_cos_operation_records_audit_row` は参照行を見ず、監査の行を書く段の失敗を起こさない。pin を監査と別の tx で先に commit する変更で落ちない |
| `busy_store_fails_record_without_residue_and_retry_succeeds`（K4） | lock を持つ間の HTTP の record（添付つき）は 5xx の problem で、候補・参照行・`expires_at` の変更を残さない。lock を外した後の同じ要求は 201 で参照行が 1 行入る | record の busy を起こす試験が無い。pin の失敗後に git の候補を消す経路は 422 でしか通っていない |
| `http_project_cancel_response_and_transition_events`（L1） | 応答の欄がちょうど cancelled_milestones・cancelled_tasks・project。cancelled_tasks は非終端だった task（ready・running）だけを TaskRef の形（actions・id・kind・status・title、status cancelled）で返し、cancelled_milestones は非終端の途中目標だけ。event は cancel した task ごとにちょうど 1 件（Transitioned、to cancelled、reason `project_cancelled`）増え、終端の task と他の案件の task には増えない | `cancelling_a_project_cascades_to_its_tasks_and_milestones` は `contains` と最後の遷移の reason だけを見る。遷移 event の重複・余計な event・応答の欄や TaskRef の形の崩れを検出できない |
| `busy_store_fails_project_cancel_without_change_and_retry_succeeds`（L2） | lock を持つ間の HTTP の project cancel（人の token、controller が shadow の案件）は 503 `db_busy` で、task の状態・task の event の数・`human_epoch`・案件と途中目標の行を変えない。lock を外した後の再試行は 200 で `human_epoch` は 1 だけ増える | project_wake_human の 2 件は trigger による tx の中の失敗だけを起こす。tx を始められない busy と、その status・code、再試行を見ない |
| `cos_project_cancel_writes_audit_and_cascade_in_one_transaction`（L3） | CoS の `project.cancel` の applied の行の挿入が落ちると task・途中目標・案件の状態と event の数が変わらず rejected の行だけが残る。別の key では applied、result の cancelled_tasks は非終端だった task だけ、監査 event 1 件。既に cancelled なら 409 `invalid_transition` と rejected の行で何も変わらない | `cos_ops_projects_cron_project_lifecycle_is_audited` は成功と 409 の rejected の行を見るが、監査の行を書く段の失敗を起こさない。cascade を監査と別の tx で先に commit する変更を検出できない |
| `non_admin_record_and_project_cancel_are_refused_without_writes`（L4） | MCP の token の HTTP の record と project cancel は 403 `forbidden` で何も書かない。token を設定しない構成でも bearer の無い record と project cancel は handler の `require_admin` で 401 `unauthorized` になり何も書かない | record の経路の 403・401 を見る試験が無い。`project_wake_api_human_mcp_is_not_counted` は status と `human_epoch` だけを見る。handler の書き直しで `require_admin` が書き込みの後ろに回っても既存の試験は落ちない |

旧コードで観測した失敗の応答（文書のための一時的な print で確認し、print は消した）:
- K4 の busy: 500 `internal`（detail `attachment pin: database is locked`。`knowledge.rs` の自前の接続の busy_timeout 10 秒の後）。
  試験は 5xx と problem+json だけを要求する。knowledge-pin で `write_tx` に寄せると `From<StoreError>` の写像で 503 `db_busy`
  になる見込みで、その変更でも試験は変えずに通る。
- L2 の busy: 503 `db_busy`（試験はこの status と code を固定する）。
- K3・L3 の trigger の失敗: 500 `chat_store_error`（detail `sqlite error: contract_fault`）。試験は成功でないことだけを要求する。

## 重ねて書かなかった既存の試験
trigger の RAISE(ABORT) による HTTP の書き込みと `human_epoch` の rollback、`human_epoch` の加算が 1 要求 1 回であること、
CoS の経路を数えないことは `crates/task-api/tests/routing_budget_suite/project_wake_human.rs` が確かめる:
- `project_wake_api_human_write_failure_keeps_the_epoch`
- `project_wake_api_human_bump_failure_rolls_the_write_back`
- `project_wake_api_human_cos_is_not_counted`
- `project_wake_api_human_each_route_bumps_once`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-api --test cos_delivery_suite tx_boundary_contract --status-level all` → exit 0、8 件 pass（26 件 skip）。
  実行時間（nextest の値）: L1 0.452s、L3 0.517s、K2 0.602s、K1 0.604s、K3 0.704s、L2 0.739s、L4 0.891s、K4 10.578s。
  全体 10.579s（compile 済みの command 全体は real 36.5s）。K4 の 10 秒は旧コードの pin の接続の busy_timeout（10 秒の固定値）で、
  pin を `write_tx` に寄せた後は API の busy_timeout（試験では 300 ms）になる見込み。
- 検出力の確認: src に一時的な不具合を 4 つ入れて同じ command を走らせ、5 件が fail した（3 件 pass）。不具合は `git checkout` で戻し、commit していない。
  - `knowledge.rs` の direct の pin の失敗で候補を消さない（`pin().map_err(discard)?` → `pin()?`）→ K4・K2 が fail。
  - `knowledge.rs` の record の handler で `require_admin` を書き込みの後ろに回す → L4 が fail。
  - `knowledge.rs` の CoS の record で pin を別の `write_tx` で先に commit する → K3 が fail。
  - `lifecycle.rs` の CoS の project cancel で cascade を別の `write_tx` で先に commit する → L3 が fail。
- `git diff $CELERIS_WU_BASE -- crates/task-api/src crates/task-ops/src crates/task-core/src | wc -l` → 0。
- test-affected・clippy・fmt の結果は下の「検査」に書く。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker。
  nextest 6121 件 pass・21 件 skip、web の tsc -b は error 無し、vitest 101 file・705 件 pass、node --test 110 件 pass、doc test pass。real 12m53s）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-api --test cos_delivery_suite -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更 path は `contract-tests.md`・`cos_delivery_suite/main.rs`・`cos_delivery_suite/tx_boundary_contract.rs` の 3 件だけ。
- `sh scripts/dev/progress-index.sh --check` → exit 0。

## 未解決事項
- K1〜K4・L1〜L4 の試験名に K・L の記号は入れていない（file の先頭の comment と上の表で対応を示す）。

## 提案
- 無し。
