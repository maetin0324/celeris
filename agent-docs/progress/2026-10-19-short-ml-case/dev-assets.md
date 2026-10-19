---
wu: dev-assets
task: 01M59KY5FTRJTJ80PACNW3K533
status: done
completed: 2026-10-19
---
# dev-assets: C2 dev case と固定入力・環境 lock・grader pin

判断材料は外部資料（status.json の external_refs）にある。ここには確定した成果だけを書く。

## 作ったもの（すべて crates/campaign 配下）
- `cases/dev/short-ml-campaign/`: case.json（通常 loader、real / dev、stages experiment+analysis）、materials（task.md・job.py・job.pbs）、kb、`env.lock.json`、`contract.json`（実行前契約）
- `fixtures/inputs/short-ml-campaign/`: train.csv（1600 行×x1..x4,y）、eval-features.csv（400 行、y なし）。case.json の `inputs[]` で配送
- `fixtures/grader-private/dev/short-ml-campaign/`: private.json、eval-targets.csv、reference.json（評価側だけ。trial に渡らない）
- `fixtures/grade-cases/dev/short-ml-campaign/`: 既存 `experiment` + `recompute`（新種別なし）
- `fixtures/tools/gen_short_ml.py`: 固定入力の生成器（seed 20261019、標準ライブラリのみ）
- `tests/short_ml_assets.rs`: loader / hash pin / 契約 pin / env lock / 入力隔離 / 改ざん拒否 / 参照値の独立再計算
- `scripts/verify-short-ml-case.py`: `--check` は pending のとき `cases/dev/short-ml-campaign` を許す（deferred と final は従来どおり拒否）

## 固定した契約
- 上限: LLM 0 token・主比較 0 試行・GPU なし・1 core・2 GiB・disk 100 MiB・job walltime 600 s・case wall 1800 s・job 1 本（硬い上限 2）
- 終了: qstat -x -f の Exit_status=0 と results/ の raw 3 file。評価側が observed-jobs を作り自己申告と照合。失敗時は再投入せず停止
- 環境: python 3.9.25 / numpy 1.23.5 は `requested_unverified`（未測定）。実行前に評価側が測定し不一致なら停止

## 証拠
- `cargo test -p celeris-campaign --test short_ml_assets` → 11 passed
- `python3 -m unittest crates/campaign/scripts/test_short_ml_state.py` → 18 OK
- `verify-short-ml-case.py --check` / `--placement-check` → exit 0（採用の証明ではない）
- `bash scripts/dev/test-parallel.sh` → 5827 passed / 0 failed、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0
- 既存 K2/K4・final・manifest/final.json・arm 実行器は無変更（git diff に無い）

## 未解決
- `docs/ops/short-ml-campaign.md` は「dev dir を置かない」と書いたまま（ops-text unit で更新）
- observed-jobs の評価側収集配線（observed-wiring unit）と受入 CLI（acceptance-cli unit）は未着手
- 実 job は投入していない。sirius の python/numpy は未再確認
