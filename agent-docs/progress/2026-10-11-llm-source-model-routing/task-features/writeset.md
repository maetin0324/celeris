---
task: 01M4P7CTS42KTCR8HS4JF0252R
unit: writeset
status: done
completed: 2026-10-11
---
# writeset: 触る範囲の書き込み集合を run_write_sets／work_unit_write_sets から抽出

前回の reviewer の差し戻し（`features_for_subject` が `FeaturesInput.write_set` を設定せず、brief は常に「範囲の記述なし」）への対応。

## 変更
- `crates/task-core/src/route_extract.rs`: `prior_write_set` を追加し `features_for_subject` から `FeaturesInput.write_set`／`write_set_gaps` を設定。
  - task subject: `run_write_sets`（`task_id` 一致）。WU subject: `run_write_sets`（`work_unit_id` 一致）と `work_unit_write_sets`（ADR D2 どおり）。
  - 判断時点の境界: `AsOf::Timestamp` は `recorded_at <= ts` かつ run の `started_at <= ts`（WU 集合は `recorded_at` のみ）。`AsOf::EventSeq` は runs と同じく、その run を指す event が `seq <= n` にあることに加え、`recorded_at` が `seq <= n` の event の最大 `ts` 以前であること（run の開始後に記録された集合を入れないため）。
  - 正規化: `paths_json` の各 path を trim・先頭 `./` を落とし `WritePathHint::parse`（末尾 `/` を落とす、絶対 path・`..`・空要素は捨てる）、`<repo>:<path>` にして `BTreeSet` で重複除去・整列。
  - `status` が `incomplete`／`unavailable` の行は path を含めつつ `aux.write_set_gaps`（`source`・`owner_id`・`repo`・`status`、整列）に残す。行が無い、または complete の行も path も無いときは `write_set = null` で `missing` に `write_set`（brief は従来どおり「範囲の記述なし」）。
- `crates/task-core/src/route_record.rs`: `FeaturesAux` に `write_set`・`write_set_gaps`、`FeaturesInput` に `write_set_gaps`、型 `WriteSetGap` を追加。brief の書式は不変（「書き込み集合:」節は既にあり、1 path 1 行）なので `TASK_BRIEF_TEMPLATE` は `task-brief/2` のまま。
- aux の欄が増えたので `route_record_schema_pinned` の hash を `sha256:49393ff8…` に更新（brief・aux の期待値は不変）。ADR 付記の hash と D3(a) 表に `write_set`・`write_set_gaps` の行、付記に境界の規則を追記。task-dispatch `cheap_local_first`・celerisctl `routing_cli` は template 文字列だけを見るので変更なし。
- 注意: `record_write_set` は upsert で `recorded_at` を更新するので、後から上書きされた行は古い時点の判断からは見えなくなる（未来情報を入れない側に倒れる）。

## 試験（fixture: migration 済み `SqliteStore` に runs・events・`run_write_sets`・`work_unit_write_sets` の実列で行を入れる）
- `route_extract_write_set_work_unit_prior_attempt_in_brief_and_aux`: WU に run 2 件＋WU 集合。`./`・重複・`..`・末尾 `/` の正規化、整列、incomplete の gap、brief の節の本文、hash の決定性。
- `route_extract_write_set_task_prior_attempt_in_brief_and_aux`: task は run 集合だけ（WU 集合の path が入らない）。
- `route_extract_write_set_work_unit_without_prior_attempt_is_missing`／`route_extract_write_set_task_without_prior_attempt_is_missing`: 前の試行なし → `null`・`missing`・「範囲の記述なし」。
- `route_extract_write_set_excludes_sets_recorded_after_decision_time`: Timestamp（00:01:30 → なし、00:02:05 → r1 のみ、task 00:03:10 → r1 のみ）と EventSeq（2 → なし、3 → r1＋WU 集合で r2 なし、5 → r2 と gap も入る）。

## 証拠
- `cargo nextest run -p task-core route_extract_write_set_ route_record_schema_pinned` → 6 passed（新規 5 本＋pinned）。
- `bash scripts/dev/test-parallel.sh` → exit 100: passed 5042 / failed 108（nextest、doctest exit 0）。失敗 108 件は全て browser・launcher・credentiald の unix socket 試験で `path must be shorter than SUN_LEN`（run の `$TMPDIR` が 93 字と長いため）。この変更とは無関係の crate。
- 同じ失敗群を短い TMPDIR で取り直し: `TMPDIR=$(mktemp -d /tmp/wsXXXX) cargo nextest run --no-fail-fast -p celeris-credentiald -p task-worker -p task-api -p celeris -p celerisctl -E 'test(/browser|launcher|credential|approve|tmpdir|python_transport|injection|broker|retired_plugin|invalid_control|auth_section|lease/) | binary(/browser|injection_ipc|broker/)'` → exit 0、588 passed（取り直し後に一時 dir は削除）。
- `cargo clippy --workspace -- -D warnings` → exit 0。

## 未解決事項
- なし（D2・D3 の他の欄の突き合わせは並行 unit field-audit）。

## 提案
- worker の `$TMPDIR` が長いと unix socket 試験が SUN_LEN で落ちる。test-parallel.sh が socket 用に短い dir を渡すか、試験側が短い path へ逃がすとよい。
