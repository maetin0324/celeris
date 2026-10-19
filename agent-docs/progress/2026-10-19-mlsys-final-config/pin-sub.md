---
tasks: [01M58VFRBA6HG0SSF3CG5BKM91]
unit: pin-sub
status: blocked-on-k2-symlink
updated: 2026-10-19
---
# 副比較セット B1/C1（mlsys-sub-1）の実値 pin

成果物・hash 一覧・不足一覧・operator 手順は artifact `measure-pinned-sub-report.md`（`pinned/sub/`）。秘密値なし、有料実行 0 件。主比較 `pinned/main/` は無変更。

## 証拠コマンドと結果
- `celeris-campaign validate --manifest pinned/sub/final.json` → exit 0、campaign sha256 `cd85b26e…8a23`（main は `8ca0e198…a999`）。`dry-run` → within_budget true。
- `measure-launcher.sh prepare` / `start`（リハーサル dir、port 7734）→ exit 0。`stop` → 停止。
- `celeris-campaign env-lock … --cap-env pinned/cap-env.json` → `{"env_lock":"created"}`、`celeris-env-lock/3`、campaign_sha256 は main と別、B/C は no_evidence_table 以外同一。
- `measure-launcher.sh preflight` → **exit 1**（main と同じ K2 symlink 阻害のみ。他の check は missing に出ない）。

## 未解決事項
- K2 symlink 矛盾（pin-values と共通）。campaign lock は未生成。選択機構 01M59KXY8345HZEE06FVZ3FWVP は main に無く、最終 preflight は統合 01M58VG5C4XGK4E70M6WJ6MEX0。

## 提案
- manifest 型が A/B/C 固定のため副比較のみのセットでも A 欄が残る。選択機構で宣言形（A を外す）が入ったら sub の manifest を新測定 ID で作り直す。
