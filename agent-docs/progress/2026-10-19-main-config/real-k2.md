---
tasks: [01M5A89DB7WQX4DCXDVQSSENT4]
status: blocked
---
# real-k2: 実 K2 無課金 preflight 証跡（operator 待ち）

## 実施
- 修正後 celeris-campaign を `cargo build --release -p celeris-campaign`（8f8d04f6）で build。sha256 de339f1c…1394。承認済み pinned/bin（e8a596e8…）は置換せず区別。
- operator 手順・実行 script・検証 script・証跡索引を artifacts の `pinned/evidence/main-k2/`（operator-check.md, run-preflight.sh, verify-evidence.py, measure-k2.env, argv-recorder.sh）に作成。
- `check-main-pins.py --root <artifacts>`: exit 1、不足は `pinned/main/evidence/k2-preflight.json` のみ。

## 未解決
- operator が測定 daemon を起動して preflight を実行し証跡を置くまで受け入れ条件 0・1 は未達。worker は daemon・実 LLM・watch を行っていない。
- 選択機構 01M59KXY8345HZEE06FVZ3FWVP 未統合: セット別の最終 preflight は task 01M58VG5C4XGK4E70M6WJ6MEX0 に引き継ぐ。
