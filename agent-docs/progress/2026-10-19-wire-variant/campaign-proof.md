---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: run観測証拠と全非対象封印の照合（campaign-proof, L2）
work_unit: campaign-proof
status: done
updated: 2026-10-19
---
# campaign-proof（propagation §8.6 の L2）

## 変更（crates/campaign のみ。daemon・wire・hash は不変）
- `variant_proof.rs`（新）: `prove_runs(exp, task_details, live_org, live_config)`。封印期待値 `RunProofExpectation`（spec・承認済み B0 org・arm config・env-lock catalog・`FixedEnv`）に、既存 GET の応答だけを照合する。
  - 木の全 task の `campaign.variant` / `variant_org` が封印値と完全一致（欠落・null・不一致は拒否）。子・孫も対象。
  - 稼働 org（`GET /api/v1/org`）: node 集合・coding 課 hash（`check_live_org`）と B0 との全 node digest（`check_b0_org`。共有関数）。
  - 全 run の selected harness/source/model: 役割（worker/reviewer/planner）ごとの封印モデルと一致、harness は arm の worker harness / review harness / arm harness に限る（builtin harness の混入は拒否）、source×model は env-lock catalog に在ること。selection 未記録・null・未知の役割・run 0 件は拒否。
  - 専用環境の固定: `FixedEnv`（承認済み instance_id・config_sha256）が無い、daemon の `identity` が不一致（再起動・別 config）・null・CLI 上書きありは拒否（fail closed）。
- `CelerisExecutor::prove_variant(task, exp)`: 既存 GET（`/tasks/{id}` の木・`/org`・`/config`）だけを読み、読み取り失敗は `interrupted`、照合失敗は `Refused`（集計しない）。POST なし・新 endpoint なし。
- `variant_pin.rs` の試験 helper を `pub(crate)` 化（再利用のみ。挙動不変）。

## 試験（fake transport のみ。network・LLM・org 変更なし）
正: spec-on / spec-off で封印観測が通る。負: campaign 欠落/null/不一致（子 task 含む）、run 0 件、selection null/pending、worker・reviewer のモデル違い、builtin harness、catalog 外 source、未知の役割、cos・research の org drift、固定環境の立証なし、instance_id 変化（再起動）、config hash null/不一致、overrides。executor 経由は GET のみで再起動を `Refused` にする。

## 証拠
- `cargo test -p celeris-campaign` → 全 binary 0 failed（lib 47 passed、うち variant_proof 5）
- `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0
- `bash scripts/dev/test-parallel.sh` は統合（integrate-verify）で実行する。

## 残る制約・提案
- run ごとの org digest・daemon_instance_id（worker の `campaign-pin.json`）は既存 GET に載らないので、観測は「読んだ時点」の org/config と run の selected 値。run 開始から読むまでの間の reload は identity に出ず、selected 値に現れない変更（genre 割当など）は直接検出できない。専用環境の固定（`FixedEnv`）を立証できなければ拒否し、運用で reload を禁止する（P2）。
- `FixedEnv` と env-lock catalog・B0 を preflight/runner の呼び出し点（`prove_variant` の caller）へ渡す配線は未実施（caller は normal-e2e / 後続）。
- planner の harness は arm の harness と同じと仮定した（封印欄が無いため）。違う運用なら封印欄の追加を提案。
