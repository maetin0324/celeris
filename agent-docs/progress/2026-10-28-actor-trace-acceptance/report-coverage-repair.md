---
title: actor trace report照合の補修
tasks: [01M60KY19FKVMTYM7AFPK32W4V]
status: done
updated: 2026-10-28
---
# actor trace report照合の補修

完了日（流れ4）: 2026-10-28
完了日（流れ5）: 2026-10-28

## 流れ4（flow4-report）

- `phase 1: 流れ4の配備観測をhuman-ops reportまで照合` — 一時DBのhuman:webの回答と、隔離fixtureのpromote成功・rollback成功・start失敗からhuman-ops /2の8生記録を作る。`Harness::outputs`から既存report builderへ渡す。既存の成功summary・失敗ファイル・status/exit_code・verifyの検査を維持した。
- 人の判断はhuman/manualの1秒、実行者はexternal_operator:flow4。固定時計はauditのbeginで10秒、finishで12秒を返す。配備の実所要時間は各2秒、計6秒がhistoryに残り、人の時間は1秒・観測区間1件のまま。要求単位の主体別件数はhuman 1・external_operator 1、除外操作は3件、人の介入はsafety_approval 1件。
- report historyはtargets・outcomeを直接出さないため、logical_op_idで入力生記録のtargets・outcomeへ結び、deployment_auditのfrom/to・decision_refs・task_refs・statusと照合する。request_refはrequestsの実要求、判断者はexportの実介入と照合する。policyはdecision参照で一致する。期待report JSONは作らない。
- 消費・人待ちは未観測のnull、exportはusage未取得・quota nullを維持する。配備にrunを推測で付けず、export/requestsも配備前後で変わらない。製品コード・共有helper・root tests/support・scriptsを変更しない。新しい設計判断は無い。
- 初回検査ではログ変数名の衝突、必須credentialの不足、auditのUTC表記（+00:00）を生記録が要求するZ終端へ変換する不足を検出して修正した。operatorのcredentialは観測できないためunknownとする。

### 証拠（提出時のHEADで実行。基準は $CELERIS_WU_BASE）

- `cargo test -p celeris --test actor_trace_flow4` → exit 0、1 passed・0 failed。
- `bash scripts/dev/test-affected.sh --list` → exit 0、celeris・celerisctl。
- `bash scripts/dev/test-affected.sh` → exit 0、813 passed・0 skipped（55 binaries）、doc test 0件・exit 0。前段check-healthも通過。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0、差分なし。
- `sh scripts/dev/check-doc-links.sh` → exit 0。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0、329 files。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `git diff --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0。流れ4試験と本進捗の2ファイルのみ。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/celeris/src crates/task-core crates/task-api crates/task-ops crates/task-worker tests/support scripts` → exit 0。製品コード・root tests/support・共有scriptの差分なし。

### 未解決

main基準の最終全5target検査・ADR/親進捗更新は record で済ませた（下の「全検査と記録」）。

### 提案

なし。

## 流れ5（flow5-report）

- `phase 1: 流れ5の人の観測記録を再送・再オープンしてもreportが不変であることを固定` — `actor_trace/support.rs` に `record_human_answer` を追加した（既存helperは変更なし）。既存の人の回答 HUMAN_DECISION（human:web、要求6秒・回答8秒）に結ぶhuman/manualの生記録4行を、既存の観測ログ `human-ops.jsonl` に追記する。内訳は人待ち（wait_start/wait_end、wait_kind=question、6〜8秒）と人の能動作業（op_start/op_end、safety_approval、7〜8秒）。
- 基準reportは既存の `Harness::outputs`（`human_ops_report::build`）で作る。期待JSONは作らない。基準のassertは次のとおり: 人の介入 safety_approval 1、主体別件数 cos_project 1・human 1、human_time の union/sum 1秒・observed 1・lower_bound false、human_wait の union 2秒・lower_bound false、人待ちと能動時間が異なる値であること、人の項目のhistory 1件（executed_by human:web、counts_as_human true、duration 1秒）。exportの人の介入（actor_id web・同じtask）と対応することも確かめる。
- 再送は2回行う。1回目は既存のrequest・event再配送の直後。2回目はSQLite storeを再オープンして担当を再起動した後に、HumanOpLogを開き直して行う。どちらも4行すべてが `AppendOutcome::Duplicate` を返し、ファイル内容が変わらない。reportを再生成し、人の観点（介入件数・主体別件数・human_time・human_wait・項目・history）とreport全体、既存の `counts` 全体が基準と一致することをassertする。
- 既存の event・operation・usage・request再配送と担当再起動の検証は残した。追加した人の項目に合わせて `report_items` の期待値を1から2へ変えた。別runの実消費（cos:project の input_tokens 合計 120）も従来どおりassertする。
- exportの人の介入の `at` は投影した時刻（壁時計）を持つ。このため観測の終了時刻とは突き合わせず、actor_idとtask_idで対応させた。新しい設計判断は無いため、ADRには付記していない。

### 証拠（流れ5、提出時のHEADで実行。基準は $CELERIS_WU_BASE）

- `cargo test -p celeris --test actor_trace_flow5 --test actor_trace_flow4` → exit 0、各1 passed・0 failed。
- `bash scripts/dev/test-affected.sh --list` → celeris・celerisctl。
- `bash scripts/dev/test-affected.sh` → exit 0、813 passed・0 skipped、doc test 0件。前段のcheck-health（check-doc-links、progress_index_commit_hashes）も通過。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh` → exit 0。`sh scripts/dev/check-adr-numbers.sh` → exit 0（329 files）。`sh scripts/dev/progress-index.sh --check` → exit 0。`git diff --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 流れ5試験・support.rs・本進捗の3ファイルのみ。

### 未解決（流れ5）

なし。全5targetの一括実行とADR・親進捗の更新は record で済ませた。

### 提案（流れ5）

なし。

## 全検査と記録（record）

完了日: 2026-10-28

- `phase 2: 全検査の結果を記録し、ADRと進捗を証拠に合わせる` — コードは変えていない。ADR の「試験境界と共有部品」を流れ 4・5 の report 照合に合わせて書き直した（古い「本 WU は土台と流れ1のみ」を削除）。全体進捗の結果表に流れ 4 の report 照合と流れ 5 の人の観測記録の再送を足し、『未解決』と『提案』の流れ 5 の項目を解消済みに直した。

### 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- `cargo test -p celeris --test actor_trace_flow1 --test actor_trace_flow2 --test actor_trace_flow3 --test actor_trace_flow4 --test actor_trace_flow5` → exit 0。5 target・6 試験が合格（1・1・2・1・1 件）、失敗 0。
- `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh --list` → exit 0。celeris・celeris-campaign・celeris-credentiald・celeris-mcp・celerisctl・e2e・llm-proxy・routing-eval・task-api・task-core・task-dispatch・task-ops・task-worker の 13 crate と web（typecheck+unit）。
- `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh` → exit 0。nextest 6,655 passed・21 skipped（307 binaries）、web vitest 685 passed（99 files）、node test 105 passed・fail 0、doc test 通過。前段の check-doc-links・check-adr-numbers（329 files）・check-human-text（0 findings）・progress-index --check・progress_index_commit_hashes も ok。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0、差分なし。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → 文書更新後に実行し、いずれも exit 0。

### 未解決（record）

なし。

### 提案（record）

なし。
