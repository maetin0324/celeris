# task-features 再試行: 前回 branch の取り込み (merge-prev)

- WorkUnit: merge-prev（工程: 前回 branch の取り込み）
- 作業ブランチ: `celeris-wu/01M4P7CTS42KTCR8HS4JF0252R/merge-prev`
- base (`CELERIS_WU_BASE`): `a209eb5805b2e6cdb1821700ac3d3877e79e791b`

## 取り込み

- 取り込んだ branch: `celeris/01M4NM332S384YYC4VC5DB7KCY`（tip `816a6a5b`）
- 取り込み前の HEAD: `d4a273af`（`celeris/01M4NM332S384YYC4VC5DB7KCY` は HEAD の祖先ではなかった）
- コマンド: `git merge --no-ff celeris/01M4NM332S384YYC4VC5DB7KCY -m 'merge previous task-features branch (celeris/01M4NM332S384YYC4VC5DB7KCY)'`
- 結果: 衝突なし（ort strategy）。56 files changed, 2761 insertions(+), 3993 deletions(-)
- merge commit: `a4f39bf4`
- 取り込み後の確認: `git merge-base --is-ancestor celeris/01M4NM332S384YYC4VC5DB7KCY HEAD` が成功（前回 branch が HEAD の祖先）

## 試験結果

- コマンド: `cargo test -p task-core route_`（`CARGO_TARGET_DIR` は celeris が渡したローカル scratch）
- 結果: exit 0。`task_core` unittests `29 passed; 0 failed; 0 ignored; 889 filtered out`
- 内訳の要点: `route_record::tests` 11 件、`route_extract::tests` 5 件、`store::route_records::tests` 3 件、`direct_route::tests` 10 件（名前に `route_` を含むもの）
- `tests/sidecar_protocol.rs` は 0 件（フィルタに該当なし）

## コード

- この WorkUnit ではコードを変えていない（merge commit のみ）。
- 次の WorkUnit `writeset`（`features_for_subject` が `run_write_sets` を時点つきで読む修正）で、前回の reviewer 差し戻し（唯一の不備）を直す。
