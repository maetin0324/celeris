---
tasks: [01M5ABHT9RV2PF5526DMSXFTNF]
status: done
completed: 2026-10-19
---
# fix-launcher-bin: launcher.env の CAMPAIGN_BIN を修正後 binary へ揃える

## 実施
- 成果物 root（`/local/celeris/data/workspaces/01M58VFRBA6HG0SSF3CG5BKM91/artifacts`）の `pinned/main/launcher.env` の `CAMPAIGN_BIN` と `pinned/main/daemon.toml` の cap-probe command を `pinned/evidence/main-k2/bin/celeris-campaign-fixed`（sha256 `de339f1c…1394`、実 K2 preflight の argv・hash と同一）へ変更。旧 binary の記述は runbook・launcher.env・pins・operator-check・run-preflight.sh から削除（取得済みの raw 証跡 `run*/hashes.txt` は改変しない）。
- `scripts/campaign/check-main-pins.py`: CAMPAIGN_BIN の path・実在・sha256 と、`run/argv.txt` の binary が同じことを照合（不整合で非 0）。`test_check_main_pins.py` に 3 件追加。
- `docs/ops/measure-run-final.md`: §1 / §3a を更新。jq は前提に残らない（`python3 -c`）。
- operator-check.md: 状態を「preflight 取得済み（exit 0）」に更新。取得後の入力変更と、再実行時の注意を明記。`watch` は未実施。

## 証拠
- `python3 scripts/campaign/check-main-pins.py --root <root>` → ok。`python3 -I scripts/campaign/test_check_main_pins.py` → 32 tests OK。
- `celeris-campaign-fixed offline-lock --verify`（lock-gen）→ env_lock verified, paid_calls_made 0。lock の内容は CAMPAIGN_BIN に依存せず不変（lock.json sha256 8ddc8384…、env-lock 6c2275f3…）。
- launcher.env の値で `config-check` → ok, paid_calls_made 0。launcher.env と measure-k2.env の差は CAMPAIGN_BIN（shim 経由か直接か）のみ。

## 試験の不安定の修正（attempt 2）
- `crates/campaign/tests/harness_exec.rs` の `runner_drives_three_arms_with_stub_and_resumes_without_restart` が稀に落ちた（3 並列負荷で約 3/180）。原因: manifest の全体 wall 上限（1_000_000）が 1 試行の上限と同値で、実時間が 1 秒進むと残りが予約額を割り、C が budget_stop で打ち切られる。全体 wall 上限を 10_000_000 に上げて決定的にした（製品コードは変更なし）。
- 証拠: 修正後、3 並列 × 40 回の harness_exec 全体実行で失敗 0。`cargo test -p celeris-campaign` 全 ok、check-doc-links / check-adr-numbers / progress-index --check ok。

## 未解決
- 取得後に launcher.env / daemon.toml を変えたため `verify-evidence.py` の入力 hash 照合は今の入力では失敗する（正本 k2-preflight.json は設置済み）。daemon 起動中の `/local/celeris/data/measure/main/daemon.toml`（host 側複製）は未変更で、次回 prepare/start 前に operator が揃える。

## 提案
- 次に取り直すときは launcher.env を直接 `--env` に使い、shim を介さず argv を記録する。
