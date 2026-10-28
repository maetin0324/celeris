---
title: 流れ5 再配送と担当の再起動で操作も集計も重複しない
tasks: [01M60AKVZEB61PRBKX5AX44H3A]
status: done
updated: 2026-10-28
---
# 流れ5 再配送と担当の再起動で操作も集計も重複しない

完了日: 2026-10-28

## 変更

- `phase 1: 再配送と担当の再起動で操作も集計も増えないことを確かめる流れ5の試験を追加` — `crates/celeris/tests/actor_trace_flow5.rs`。共有 helper（`actor_trace/support.rs`）は変えていない。
- 1 回目: 流れ1（認可内の decision を案件 CoS が回答、偽 HTTP の固定応答と usage）と、流れ2の素材（認可外の decision を `human:web` が回答、`operator:claude-code` の取り次ぎ）を流し、decision log export・human-ops report・eval requests を作る。
- 再配送: 同じ source revision の受信箱再取り込み（`cos_triage_ingest_batch`・`cos_triage_reconcile` が 0 件）、同じ client id の取り次ぎ再送（`created=false`）、同じ人の回答の再送（拒否）、評価者の観測記録の再送（同じ record_id で行が増えない）。
- 再起動: store を開き直し、run 喪失の回収（`cos_triage_requeue_run` が 0 件）と再 claim（なし）、再起動後の偽 CoS が同じ idempotency key で操作を再実行（既存の操作が返り、適用の closure は呼ばれない）、終端 telemetry を倍の値で再送（元の usage のまま）。
- 照合: CoS 操作数、受信箱項目数、chat run 数、取り次ぎ message 数、DecisionAnswered 数、介入件数と主体別内訳、usage 行数と共有小計、論理要求件数、report の試行・介入・主体別・除外・項目数が 1 回目と同じ。export の JSONL・report・requests の全体も一致。
- 製品側の欠陥は見つからなかった（製品コードの変更なし）。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `cargo test -p celeris --test actor_trace_flow5` → exit 0、1 passed・0 ignored。
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celerisctl、808 tests passed・0 skipped、doc test 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy -p celeris --test actor_trace_flow5 -- -D warnings` → exit 0（参考。`--tests` 全体は既存の `fake_http` 等の duplicate_mod で落ちる。範囲外）。

## 未解決

- human-ops report には人の操作の観測記録を入れていない（流れ1の CoS 操作の記録だけ）。人の介入の report 側の重複なしは、流れ2の記録が統合された後に closeout で足すとよい。

## 提案

- なし。
