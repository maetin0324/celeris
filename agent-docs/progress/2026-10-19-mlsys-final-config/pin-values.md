---
tasks: [01M58VFRBA6HG0SSF3CG5BKM91]
unit: pin-values
status: blocked-on-k2-symlink
updated: 2026-10-19
---
# 主比較セット A0/B0（mlsys-main-1）の実値 pin

成果物と hash 一覧・不足一覧・operator 手順は artifact `measure-pinned-report.md`（`pinned/` 以下に実値の final.json・launcher.env・daemon.toml・cap-env.json・canaries）。秘密値なし。

## 証拠コマンドと結果
- `celeris-campaign validate --manifest pinned/main/final.json` → exit 0、sha256 `8ca0e198…a999`。`dry-run` → within_budget true、6 trials、llm_calls 0。
- `measure-launcher.sh prepare/start`（リハーサル dir、実 release c9654bff5633 の celeris）→ exit 0。
- `celeris-campaign cap-probe --arm a`（K2・K4）→ exit 0。`--arm b`/`--arm c`（測定 daemon 経由）→ exit 0 ok:true。
- `celeris-campaign env-lock … --cap-env pinned/cap-env.json` → `{"env_lock":"created"}`、schema `celeris-env-lock/3`、cap_env 内容 hash `deeb27f1…2fc`。
- `measure-launcher.sh preflight` → **exit 1**: K2 初期 tree の symlink 3 件（lock は含む / isolation は拒否）。他の check は missing に出ない。campaign lock は未生成。
- 発見して直したこと: A の harness_version は `claude --version` の部分一致（`claude-code@…` は env-lock が拒否）、daemon に `[selfdeploy] releases_dir` が要る、CAP_ENV に Rust toolchain（rustc-min / min-build-locusta）と PATH、adapter env の PKG_CONFIG_PATH。runbook §3 / §3c / §4 に反映。

## 未解決事項
- K2 の symlink 矛盾（preflight exit 0 の阻害）。選択機構 01M59KXY8345HZEE06FVZ3FWVP は main に無く、最終 preflight は統合 01M58VG5C4XGK4E70M6WJ6MEX0。

## 提案
- `snapshot::copy_tree` / isolation が tree 内に閉じた相対 symlink を許す、または asset を symlink なしで作り直し新測定 ID で lock を取り直す（人の判断）。
