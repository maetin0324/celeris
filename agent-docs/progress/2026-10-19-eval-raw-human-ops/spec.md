---
tasks: [01M59KXY854W5C65GTG9GVQVAC]
status: done
completed: 2026-10-19
---
# 多軸評価 2a/P0: 生記録（human-ops JSONL）の形式・identity・時計・操作種別の固定（WU spec）

## 再利用先の確認（base c9654bff = main head）
- `crates/campaign/src/record.rs` `TrialRecord`: identity と `wall_secs`（u64。未実行・executor_error は 0、timeout で丸め）。
- `crates/campaign/src/ledger.rs`: `TrialKey::idempotency_key()`（`campaign-<sha256>`）、task/run/cluster_job_ids、`started_at`/`submitted_at`。
- `crates/campaign/src/manifest.rs`: campaign_id・sha256・`ConfigId` A/B/C・repeats（実験セットの欄は無い → `experiment_set` だけ新設）。
- `crates/campaign/src/final_accounting.rs` と `task-core` `decision_log_export.rs`: 欠測を 0 にしない集計（`Stat` 4 つ組）。
- `task-core` `decision_log_project_interventions`: 人 event → 介入（`wall_ms` は常に None、safety/system を区別しない）。
- `task_campaigns`/`task_campaign_resolve`、events（Transitioned・ClusterJobWait*・QuestionRaised/Answered・ApprovalRequested/Decided・BrowserWait*・Retried）、`runs`。
- `crates/routing-eval`: 人の操作・時間の欄なし（再利用先にしない）。

## 変更
- `agent-docs/adr/2026-10-19-eval-raw-human-ops-record.md`（D1〜D9）
- `docs/ops/eval-raw-record-definitions.md`（record 種別表、共通欄・identity 表、時計表、record_id 規則、欠測規則 §2.4、操作種別表 §3.2、event 由来/評価者記録の区分表 §4、時間・やり直し §5、限定事項 §6）
- 実装はしていない（core/cli/join/runbook WU が担当）。

## 証拠
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`: ok
- `sh scripts/dev/check-doc-links.sh <2 files>`: ok
- `bash scripts/dev/test-parallel.sh`: exit 0（5816 passed, 0 failed, 15 ignored）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

## 未解決事項
- A の human_wait を Claude Code transcript の ts から取れるか（`time_basis=harness_transcript`）は未確認。取れなければ評価者記録のみ。
- B の介入は生記録を正とし projection は照合用（加算しない）。不一致の報告形式は join WU で決める。

## 提案
- intervention projection の `ApprovalDecided`→unclassified を、生記録の `safety_approval` と照合できるよう join で `celeris_event_ref` を突き合わせる。
