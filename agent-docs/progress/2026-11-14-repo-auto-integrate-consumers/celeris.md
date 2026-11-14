---
title: celeris の取り込みの消費側を repo の欄で判定する
tasks: [01M7CFRYZH0AW00M3WAHXX8B57]
status: done
updated: 2026-11-14
---
# celeris の取り込みの消費側を repo の欄で判定する

完了日: 2026-11-14

基準は `$CELERIS_WU_BASE`。この WorkUnit の開始時点で、`crates/celeris/src/delivery.rs` の `tick_with` は
`task_ops::delivery::auto_integrates(store, &task)` が真のときだけ `advance` する形になっていた（task 行が無ければ `continue`、木の子 task も進めない）。
`delivery_repo` は delivery 行の `repo_id` の repo 行から取り込み先を決め、`merge_reviewed` に案件単位の判定は無い。
celeris crate に案件の `auto_integrate`・`delivery_projects` の判定は残っていない（`delivery_projects` は config の読み込みエラーとして試験が残る）。
`delivery/tests.rs`・`cos_delivery_suite`・`integration_api.rs` の案件の欄を使う試験は repo 単位になっていた。

この WorkUnit で足したもの: 走行中に repo を無効化すると次の tick から配送が進まないことを確かめる試験
`tick_stops_advancing_after_repo_is_disabled_mid_run`（`delivery/tests.rs`）。既存の `tick_advances_only_repos_with_auto_integrate` は
無効→有効の向きだけで、有効で進んだ後に無効化する向きを検出できないため。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p celeris delivery stranded integration_api` → 65 passed
- `cargo nextest run -p celeris tick_` → 13 passed
- `cargo clippy -p celeris --all-targets -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決事項・提案
なし。
