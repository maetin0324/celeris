---
title: "多軸評価 2a/P0: A/B 共通の人の介入・経過時間・やり直しの生記録"
tasks: [01M59KXY854W5C65GTG9GVQVAC]
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# 多軸評価 2a/P0: 生記録（human-ops）の実装状況と評価者 runbook

WU 別の記録は `2026-10-19-eval-raw-human-ops/{spec,core,cli,join}.md`。評価者向け手順は `docs/ops/eval-human-ops-runbook.md`、定義は `docs/ops/eval-raw-record-definitions.md`、根拠 ADR は `agent-docs/adr/2026-10-19-eval-raw-human-ops-record.md`。

## 区分

### 実装済み（main に統合済み）
- 生記録の型・検証・冪等 JSONL 追記（`crates/campaign/src/human_ops.rs`）
- 評価者 CLI `celeris-campaign human-op record|span|validate|list`（同 `mod cli`）
- event 結合 `human-op join`（`crates/campaign/src/human_ops_join.rs`）
- 定義書・ADR・runbook（この WU）

### 無 LLM 検証済み（決定的試験・実コマンド）
- 冪等・欠測が 0 にならない・A/B 同一入力で同一 record_id・不正入力拒否: `tests/human_ops.rs`（10 passed）
- A/B 同一 command 列で同形式・再実行で行が増えない・conflict 拒否: `tests/human_ops_cli.rs`（4 passed）
- mock の A/B/C trial と同 identity で結合・再 export で出力 byte 一致・A 腕の missing: `tests/human_ops_join.rs`（2 passed）
- runbook の command 例を実バイナリで通した（下記証拠）

### 未測定（測定開始 2026-10-22 目標。この task では測らない）
- 実 A0/B0/B1/C1 の人の介入件数・時間・やり直し回数（実運用の記録は評価者が測定中に書く）
- A の human_wait を Claude Code transcript の ts から取れるか（未確認。取れなければ評価者記録のみ）
- 2 host 間の時計ずれの実測値
- 集計・表（2b）。実 LLM 検証は行っていない

## 再利用先
`TrialRecord.wall_secs`・`TrialKey::idempotency_key()`・`LedgerRow`（started_at/submitted_at/task/run/cluster_job_ids）・`manifest::canonical`・export の `records.jsonl`（run/outcome/intervention）・`decision_log_project_interventions`・`Stat` 4 つ組の欠測規約（`final_accounting`）。新規 crate なし。

## 証拠コマンド
- runbook の例（record 4 形・span wait/attempt/job/cutoff・validate・list・join）を一時 dir で実行: record の再実行は `duplicate`、`validate` は `ok: 11 records`、`list` は op 3 件の `duration_secs`、`join --events <out dir>` は exit 0（A 腕 event 欄は missing、`missing_count` 6）。
- `check-doc-layout.sh` ok / `check-doc-links.sh` ok（両 exit 0）。
- `bash scripts/dev/test-parallel.sh`: exit 0（nextest passed 5832 / failed 0、doctest ok）。`cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## 未解決事項
- CLI に `correction` を書くサブコマンドが無い（型・読み出しは対応）。誤記の訂正は現状 JSONL への手書き追記になるため、必要なら後続 WU で追加。
- `--target` の sha256 は常に null。
- 並行追記は排他せず、読み出し側で重複を潰す。
- B0 と B1 は同じ ConfigId::B。別 campaign で記録する運用に依存する（runbook §1 に明記）。

## 提案
- 測定開始前に評価者が runbook §8 のチェックリストを試し out で通す。
- correction 用 CLI と定義書 §2.1 への `wait_kind`/`result` 値域の付記（core.md の提案）は 2b か後続で扱う。
