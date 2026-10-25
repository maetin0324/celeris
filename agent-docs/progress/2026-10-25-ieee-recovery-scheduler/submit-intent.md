---
title: IEEE recovery scheduler — 投入前 intent 台帳と unknown_submit の照合（submit-intent）
tasks: [01M5SSBCP75CTVF2SHD0QQ9ZGG]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# 投入前 intent 台帳と unknown_submit の照合（submit-intent）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。
[ADR](../../adr/2026-10-25-recovery-scheduler-backend.md) D4・実装付記 I3 に従った。実 scheduler・ssh・qsub・外部 network には通信していない（偽 transport のみ）。

## やったこと

- `crates/campaign/src/dagbench/pbs/intent.rs` を追加（`pbs/mod.rs` に `pub mod intent;`）。
  - `SubmitIdentity`（measurement_id・arm・trial_key・logical_job_id・node・workload_attempt）と決定的 `submit_intent_id`（`si-` + sha256(canonical JSON, schema `celeris.submit-intent-id/1`)）。
  - `IntentSpec`: 投入先・資源・script・payload sha256・poll 間隔・試行 deadline。`resource_sha256`、`request()`（両 marker を identity から埋めた `QsubRequest`）、`check_record`（`qstat -x -f` の Job_Owner・server/cluster・Account_Name・queue・Resource_List.select/place/walltime・Rerunable=False・両 marker・Submit_arguments の script。欠けた欄も不一致）。
  - `IntentLedger`: `submit-intents.jsonl`（追記専用、1 行 1 遷移、seq・UTC・epoch、書くたびに `sync_data`）と `submit-intents.lock`（`File::lock`）。lock の中で末尾の壊れた行を切り（`ext::drop_torn_tail` を再利用）全行を読み直してから判断する。`intent_recorded`→`sent` を sync してから qsub を 1 回。結果は `accepted`（job_ref `pbs:<cluster>:<id>`）・`rejected_before_send`（起動していないときだけ。停止扱い）・`unknown_submit`。
  - 再開: 最後が `intent_recorded`/`sent` なら `unknown_submit{interrupted}` を追記し、qsub は送らない。既存 intent は再生のみ。条件違いの同じ intent は `Conflict`（qsub なし）。遷移規則に合わない行（2 本目の intent_recorded など）は台帳エラーで送らない。
  - unknown_submit の照合: marker 検索で候補 1 件 → その job の `qstat_full` を `check_record` → 一致で `reconciled`。複数・不一致・履歴欠落（Unknown Job Id）・非 0（権限不足）・Unsupported・応答不正は `unresolved_stop`（終端、以後 qsub も照会もしない）。0 件と照会の timeout・切断は `reconcile_retry`（原票を保存）で poll 間隔後に再照合、試行 deadline 到達で `unresolved_stop`。時刻は `IntentClock`（試験は注入時計、`resolve` は lock の外で `sleep_until`）。未投入を表す結果は持たない。
- 試験 `crates/campaign/tests/dagbench_pbs_intent.rs`（14 件）: id の決定性、qsub 中に台帳を読んで intent が先に永続化されていること、正常受理と再投入の再生、応答喪失→1 候補一致、PID 形・空・切断・非 0 の unknown、0 候補の 30 秒間隔再照合（60 回）と deadline 停止、照会 timeout の再照合と原票保存、複数候補、10 種の不一致、履歴消失・権限不足・Unsupported・応答不正、送信前拒否、条件違いの Conflict、送信後（qsub 中の panic）・送信前（intent_recorded のみ＋壊れた末尾行）の再起動、lock 待ちを hook の出来事で同期した二重 submit 競合、遷移規則違反の拒否。全 fixture で物理 qsub ≤1 を assert。

## 証拠

提出時の HEAD（commit「phase 1: add submit intent ledger with marker reconciliation」）で実行。

- `cargo test -p celeris-campaign --test dagbench_pbs_intent` → 14 passed。
- `cargo test -p celeris-campaign --test runner_checker --test dagbench_pbs_transport` → 16 / 8 passed（src 走査の禁止語に抵触しない）。
- `bash scripts/dev/test-affected.sh` → TEST_AFFECTED_RESULT
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` は crates/campaign の 3 path と本進捗ファイルのみ。

## 未解決事項

- cluster の照合は `server` 欄（無ければ job ID の server 部）の先頭 label = cluster 名。Sirius の実 PBS server 名が違えば照合は不一致で停止する（安全側）。接続後の read-only 確認で決める。
- payload は intent に sha256 を固定し、PBS 側では Submit_arguments の script path で照合する（PBS 原票に内容 hash は無い）。
- `accepted` 後の `qstat_full` による marker/資源照合（accepted→unresolved_stop）と `human_resolution` 行は ext-pbs unit で `check_record` を使って足す。
- 再照合の対象を 0 件・照会の timeout/切断に限った（他は即停止）。0 件を待つのは PBS の可視化遅れを未投入の証拠にしないため。
