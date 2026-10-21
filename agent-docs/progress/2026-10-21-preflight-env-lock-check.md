---
task: 01M5EKSPKAKTBEYD5RHGZ0S3AQ
status: done
completed: 2026-10-21
adr: agent-docs/adr/2026-10-21-preflight-env-lock-check.md
---
# preflight に env-lock の照合を入れる・offline-lock のモデル一覧は測定 daemon から取る

## 実装
- `env_lock::observe`（現物の観測の組み立て）と `env_lock::verify_existing`（`establish(verify_only)` 相当）を追加。watch が呼ぶ `env-lock`/`offline-lock` と preflight の両方がこれを使う（二重実装なし）。
- preflight: `<out>/env-lock.json` が在れば接続先の測定 daemon の `GET /api/v1/llm/models` で照合。不一致は `missing[].check="env-lock"`、`failure_class="env_lock_mismatch"`。`Inputs.worker_programs` を追加（`--worker-program`）。
- `measure-launcher.sh`: preflight に `--worker-program` を渡す。`lock` 段を追加（`LOCK_OUT`。測定 daemon の一覧を保存→sha256→offline-lock）。
- `docs/ops/measure-run-final.md`: 本番 daemon の保存証跡（main-llm-models.json）を使う手順を削除し、lock 段に置き換え。

## 証拠
- `cargo nextest run -p celeris-campaign --test preflight` → 27 passed（新規 `preflight_rejects_an_env_lock_built_from_another_daemons_catalog`・`preflight_accepts_an_env_lock_built_from_the_measurement_daemon`、偽 Host・LLM なし）
- `bash scripts/dev/test-affected.sh` → exit 0（2533 tests run: 2533 passed, 4 skipped）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `cargo nextest run -p celeris-campaign --test arm_set_e2e --test arm_set_cli_normal --test launcher --test preflight --test env_lock` → 75 passed
- 付随修正: launcher は `common_args` で全 subcommand に `--worker-program` を渡す（run・watch 内の preflight も env-lock と同値で照合）。`env_lock::observe` は catalog を closure で受け、A の解決を daemon 接続より先に行う。saved-lock gate を期待する試験は env-lock mismatch による先行拒否も許容
- `grep -rn main-llm-models docs scripts crates` → 0 件（本番 daemon 一覧を使う旧手順は削除）

## 本番への移行
- 本番 DB・config の変更なし。測定 root の既存 `pinned/main/env-lock.json` が本番一覧由来なら、人が測定 daemon を start した後 `LOCK_OUT=<新しい空 dir> scripts/campaign/measure-launcher.sh lock --env <launcher.env>` で作り直し、`pinned/main/` へ複写して `check-main-pins.py --root $R` を通す。確認: `measure-launcher.sh preflight` が ok true。

## 未解決事項
- なし

## 提案
- なし
