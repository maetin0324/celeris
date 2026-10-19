---
wu: acceptance-cli
task: 01M59KY5FTRJTJ80PACNW3K533
status: done
completed: 2026-10-19
---
# acceptance-cli: 受入 CLI

判断材料は外部資料（status.json の external_refs）にある。ここには確定した成果だけを書く。

## 作ったもの
- `verify-short-ml-case.py --acceptance --evidence <file> [--grader-bin <path>]`。`--check` / `--placement-check` は従来どおり（`--check` 通過は採用ではない）。
- evidence の status で分岐: pending・未知・承認不足・未検証は exit 1。引数誤りは exit 2（`--now` は無い）。
- adopted: contract の pin を実ファイルと照合、環境測定と lock、承認と残枠（survey の数字は現残枠にしない）、評価側 observed-jobs（origin=live）と raw hash を確かめ、`celeris-campaign grade` を fresh workdir で正例 1 と独立な負例 3（誤計算 / 条件違い / 未実行）で実際に再実行する。負例は正例との差が 1 点だけであることと、意図した check・理由で拒否されたことを見る。
- deferred: 実時計 >= 確認点（2026-10-22T00:00:00Z）、再確認の実日時（確認点以後・現在以前）・コマンド・exit≠0・外部出力の hash を検証する。
- 時計注入と synthetic 証拠は Python API の test 専用で、実証拠や CLI では使えない。

## 証拠
- `python3 -m unittest crates/campaign/scripts/test_short_ml_acceptance.py crates/campaign/scripts/test_short_ml_state.py`: 46 tests OK
- 現 repo（pending）: `--check` exit 0、`--acceptance --evidence <外部 reassessment>` exit 1（pending は非 0）

## 未解決
- 実 case の成立は未達（試験合格 ≠ case 成立）。adopted の実 evidence 形式（`adoption` / `deferral` ブロック）は本 script の定義が初出。
