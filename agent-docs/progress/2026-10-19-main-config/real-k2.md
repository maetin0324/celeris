---
tasks: [01M5A89DB7WQX4DCXDVQSSENT4]
status: done
completed: 2026-10-19
---
# real-k2: 実 K2 無課金 preflight 証跡（取得済み）

## 実施
- 修正後 celeris-campaign を `cargo build --release -p celeris-campaign`（8f8d04f6）で build。sha256 de339f1c…1394。承認済み pinned/bin（e8a596e8…）は置換せず区別。
- operator 手順・実行 script・検証 script・証跡索引を artifacts の `pinned/evidence/main-k2/`（operator-check.md, run-preflight.sh, verify-evidence.py, measure-k2.env, argv-recorder.sh）に作成。
- `check-main-pins.py --root <artifacts>`: exit 1、不足は `pinned/main/evidence/k2-preflight.json` のみ。

## 現状（2026-10-19 更新）
- 実 K2 preflight は修正後 binary で取得済み: `pinned/main/evidence/k2-preflight.json`（`ok:true`、`paid_calls_made 0`）。K2 symlink は修正済み binary で解決。`check-main-pins.py --root <artifacts>` は ok（exit 0）。
- 取得後に launcher.env / daemon.toml を CAMPAIGN_BIN の共通化で変えたため `verify-evidence.py` の入力 hash 照合は今の入力では一致しない（正本の証跡は設置済み。fix-launcher-bin の進捗を参照）。

## 未解決
- 副比較（mlsys-sub-1）の preflight と campaign lock は未取得。選択機構 01M59KXY8345HZEE06FVZ3FWVP 未統合のため統合 01M58VG5C4XGK4E70M6WJ6MEX0 に引き継ぐ。主比較は副比較に依存しない。
