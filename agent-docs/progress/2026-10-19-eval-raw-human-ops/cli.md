---
tasks: [01M59KXY854W5C65GTG9GVQVAC]
status: done
completed: 2026-10-19
---
# 多軸評価 2a/P0: 評価者用 CLI（WU cli）

## 実装
- `celeris-campaign human-op <record|span|validate|list>`（`crates/campaign/src/human_ops.rs (mod cli)`、`main.rs` は manifest 読み込み前に分岐）。
- `--arm A0|B0|B1|C1` から experiment_set と ConfigId を決める（A/B/C 同じ command 列）。`--manifest-sha256` か `--manifest` で manifest_sha256。`--trial-no` 既定 1（runner と同じ）。
- `record`: op_start を書き、`--ended-at` か `--outcome` があるときだけ op_end を足す。終了が不明なら op_end を書かず missing のまま。
  `--started-at now` は時計の値＋`evaluator_live`、明示時刻は `evaluator_reported`（`--time-basis/--time-precision` で上書き）。
- `span --kind wait|attempt|job|cutoff`: 待ち・試み・scheduler 計上・打ち切り。null の時刻は `missing`（既定 unknown、`--missing-reason` で変更）。
- `validate`: read_records＋resolve。壊れた行は `line N:` 付きで exit 1。file が無くても exit 1。
- `list`: correction 適用後の行（`arm_label` 付き）、op 区間の `duration_secs`（端が欠ければ null）、redo_counts。
- 時計: `--now` / `CELERIS_HUMAN_OPS_NOW`、`--host` / `CELERIS_HUMAN_OPS_HOST`、`--clock-source`。A は task_id/run_id を `not_applicable`。
- 同じ record_id で内容が違う再送（開始時刻の食い違い等）は conflict で exit 1（上書きしない）。

## 証拠
- `cargo test -p celeris-campaign --test human_ops_cli`: 4 passed（A/B 同形式・再実行で file 同一・missing は null・validate 拒否・conflict）。
- 全体 gate は下記（完了報告に出力を記載）。

## 未解決事項
- `--target` の sha256 は常に null（CLI から渡せない）。必要なら後続で追加。
