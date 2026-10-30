---
title: ops-r1 の condition の manifest 宣言と human-op の condition 指定（pipe-manifest）
tasks: [01M655T6CGPRQ2HSQPYQFXYGZX]
status: done
updated: 2026-10-30
---
# ops-r1 の condition の manifest 宣言と human-op の condition 指定（pipe-manifest）

完了日: 2026-10-30

## 結果
- manifest: `contract.conditions`（arm → condition_id・intake `direct_claude_code`|`cos_thread`・trial_ids、空なら直列化しない）を足した。
  型と検査は `crates/campaign/src/ops_r1_conditions.rs`。検査は conditions があるときだけ走る（revision が main-4-ops-r1、有効 arm を全部覆う、
  A=direct_claude_code・B=cos_thread・C は不可、condition_id は空でなく重複しない、trial_ids は repeats 個で arm をまたいで重複しない）。
  `check_resolved` は B=cos_thread のとき B の routing 欠落を許し、routing を書いたら拒否する。
- 投入系の拒否: `run`・`watch`・`dry-run`・`preflight`・`config-check`・`env-check`・`cap-probe` は conditions のある manifest を拒否する（`crates/campaign/src/main.rs`）。
- human-op: `record`・`span`・`correct`・`list` に `--condition`（`--arm` と排他、`--manifest` の path 必須。`--manifest-sha256` だけは拒否、両方あれば一致を検査）。
  `ExperimentSet::OpsR1 = "ops-r1"`、記録の任意欄 `condition_id`（record_id の材料に入れない）、`arm_label` は `ops-r1-A`・`ops-r1-B`。`--scope shared --experiment-set ops-r1` も通る。
- 旧系列の固定: 公開済みの 3 manifest（R2 final・K4 pilot・K2 pilot）の写しを `crates/campaign/fixtures/frozen-manifests/` に置き、
  campaign_sha256 が `docs/ops/mlsys-main-4-pilot-runbook.md` の公開値と一致することを試験で固定した。
  旧 human-op 記録（main-ab・B0）は condition_id を持たず、bytes と record_id（固定値。材料欄を Python で別に hash して同じ値を確認）が変わらないことを試験で固定した。

## 設計からの差（ADR 2026-10-30-ops-r1-pipeline-manifest の付記に記録）
- `conditions` は manifest の最上位でなく `contract.conditions` に置いた（最上位だと既存試験の構造体リテラル約 45 か所が compile できない）。
- 「revision = main-4-ops-r1 なら conditions 必須」は入れなかった（既存の celeris_exec.rs の試験が conditions の無い ops-r1 契約で run を通す）。
- ops-r1 で `fixed.role_models` が空のときだけ B0 の宣言検査を外した。observed の照合は変えていない。
- 既存試験に足した行: `celeris_exec.rs` の `ops_r1()` に `conditions: Default::default()`、`tests/human_ops.rs` と
  `crates/celeris/tests/actor_trace/support.rs` の記録の雛形に `condition_id: None`。assert は変えていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo test -p celeris-campaign --test ops_r1_pipeline_manifest` → 7 passed（ops_r1_pipeline_* の 7 件）
- `bash scripts/dev/test-affected.sh` → exit 0、2838 tests run: 2838 passed、9 skipped、source-size gate 0 failure
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh "$CELERIS_WU_SCOPE_PATHS"` → crates/campaign/・crates/celeris/tests/・docs/ops/・agent-docs/ の中だけ

## 未解決事項
- ops-r1 manifest の雛形と実値（固定票から作る）は pipe-assemble の担当。
- set 宣言の照合（`reconcile_set_fixed`）が routing の無い B を通すかは未確認（pipe-assemble が試験で確かめる）。

## 提案
- なし
