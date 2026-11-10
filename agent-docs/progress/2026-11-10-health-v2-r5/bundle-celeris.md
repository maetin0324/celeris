---
title: 健全性v2 R5 celeris の結合試験の束ね
status: done
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 celeris の結合試験の束ね

完了日: 2026-11-10

WorkUnit `bundle-celeris`（段 bundle）。`inventory.json` で leaf が `bundle-celeris` の 15 target を 4 束に移した。前回の試行の commit「phase 1: bundle celeris integration test targets」を cherry-pick して使い回した。reclass の後の `inventory.json` でも担当は同じ 15 target で、r6 に変わった target は束に入っていなかったので、元の path に戻した file は無い。基準は `$CELERIS_WU_BASE`。

## 実装前 brief
1. 根本原因: 同じ capability・fixture の試験が別の Cargo target になり、test binary の build/link が増えている。
2. 責務 owner: `crates/celeris/tests/` の test target 構成。通知は `celeris::notify`、daemon は `celeris::daemon`、script の fixture は `tests/support/script_compare.rs` が持つ（`docs/architecture-map.md` の該当行）。本番コードの責務は変えない。
3. 再利用先: 既存の `crates/celeris/tests/support/mod.rs` と repo root の `tests/support/http_*.rs`。試験本文はそのまま module として読む。
4. 選択肢: 独立 target の維持と、ADR の D1・D3 に沿った同じ capability・fixture ごとの束ねを比べ、後者を採った。全部を 1 束にする案は D1・D3 に反するので採らない。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 担当 15 target の元の path と、動かした file の先頭の支援 module 宣言（`mod support;`・`#[path] mod …;` と `#[allow(clippy::duplicate_mod)]`）。束の `main.rs` に 1 回だけ宣言した。
7. 不変条件: 試験関数・表の case・不変条件の試験の数を base と同じに保つ（`cases.py`）。process 汚染を持つ target は動かさない（`classify.py --verify-leaf`）。束の試験が 1 process でも通ることは各束の `cargo test` で確かめる。

## 変更
commit:
- `phase 1: bundle celeris integration test targets`（前回の試行の commit の cherry-pick。衝突なし）
- `R5 bundle-celeris: 進捗に今回の検査の結果を書く`

束（`crates/celeris/tests/<bundle>/main.rs`）:
- `browser_eval_surface_suite`: browser_startup_reap・fake_http・workspace_roots。`tests/support/http_{async,request,sync}.rs` を `main.rs` で 1 回ずつ宣言した。
- `cos_delivery_suite`: cos_chat_stagnation_notify・cos_chat_triage_notify・cos_chat_triage_scenarios・deployment_audit・releases_api・stranded_default_branch・stranded_integration。`tests/support/http_async.rs` を `fake_http` の名前で 1 回宣言した。
- `routing_budget_suite`: script_compare_execution_gate・script_compare_report・tick_scale_baseline。`support/mod.rs` を 1 回宣言した。
- `unmapped_suite`: legacy_event_startup・variant_propagation。旧 event shape の replay 契約の試験（legacy_event_startup）は消さずにそのまま移した。

試験関数の本文は変えていない。動かした file は先頭の宣言を `use crate::<module>;` に替えただけ。

## 元の path に残した target と理由
inventory の leaf と ADR D3・D4 に従う。担当外の target には触っていない。

daemon を `CARGO_BIN_EXE_celeris` で起動する試験と `current_exe` で自分を起動し直す試験の扱い:
- `current_exe`（ADR D3 の keep 条件に入る）: `script_runtime`（current_exe と可変 static）と `long_run_soak`（current_exe・signal、名前で呼ばれる suite）。自分の binary を子 process として起こすので、束ねると子が束の全試験を含む binary になる。keep のまま独立 binary に残した。
- `CARGO_BIN_EXE_celeris` で daemon を起こすこと自体は D3 の keep 条件に入らない（子は別 process で、試験 process の env・CWD・static を変えない）。この形の target はどれも別の理由で動かさない: `cap_probe_real_daemon`（env の書き換え）、`instance_handoff`（固定 path・固定 port）、`ops_r1_measure`・`ops_r1_pipeline`（file 名の数字）、`bin_smoke`・`worker_db_guard_refuse`・`unpromoted_release`（地図が参照するので r6）。
- 束に移した 15 target は `CARGO_BIN_EXE_celeris` も `current_exe` も使っていない（`grep -rln 'CARGO_BIN_EXE\|current_exe' crates/celeris/tests/` の結果は上の 9 file だけ）。

Objective の例に挙がった target のうち、束に移さなかったもの:
- r6（地図 `docs/architecture-map.md` が path で参照する。地図は R5 の write set の外）: `script_compare_cos_digest`・`script_compare_lane_rules`・`script_compare_route_priority`・`script_generation_daemon`。ほかに `bin_smoke`・`daily_curation`・`integration_api`・`notify`・`script_exhaustive`・`ui_ux_skills_delivery`・`unpromoted_release`・`worker_db_guard_refuse`。
- keep: `credential_wait_notify`（固定 port）、`campaign_lock_e2e`（可変 static と file 名の数字）、`campaign_wiring_e2e`・`script_generation_e2e`（file 名の数字）。
- keep（file 名の数字、write set の外）: `actor_trace_flow1`〜`5`・`ops_r1_attr`・`ops_r1_measure`・`ops_r1_pipeline`・`ops_r1_trace`・`campaign_acceptance_e2e`・`evidence_mode_e2e`・`project_controller_e2e`・`trial_context_canary_e2e`。
- keep（その他）: `cluster_login_script`・`daemon_ready_wait`（束ねる相手の無い 1 件だけの組）、`root_done_summary`・`tick_prod_copy_steps`（固定 port）、`routing_eval_run`（env の書き換え）、`project_controller_replay`・`project_cos_shadow`（名前で呼ばれる suite）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --verify-leaf bundle-celeris agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0、`15 moved target(s) of the leaf, 262 keep/r6 target(s) checked in place, 0 error(s)`。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate celeris --base "$CELERIS_WU_BASE"` → exit 0、試験関数 806 → 806、表の行 561 → 561、名前で追った移動 57、不変条件の試験 321（35 file）、removals 0 件。
- `cargo nextest list -p celeris --message-format json` → 試験 804 件（ignored 2）。base の run1 は 802 run + 2 skipped で同じ数。test binary は base 57 → HEAD 46（`--list-type binaries-only`。lib・bin を含む。test target は 55 → 44）。
- `cargo nextest run -p celeris -E 'binary_id(celeris::browser_eval_surface_suite) | binary_id(celeris::cos_delivery_suite) | binary_id(celeris::routing_budget_suite) | binary_id(celeris::unmapped_suite)'` → exit 0、57 tests run: 57 passed。
- `cargo test -p celeris --test <bundle>`（1 process の実行、D3 の確認）→ 4 束とも exit 0。browser_eval_surface_suite 7 passed・cos_delivery_suite 28 passed・routing_budget_suite 15 passed・unmapped_suite 7 passed。1 process でだけ落ちる試験は無く、束から外した target は無い。
- `bash scripts/dev/test-affected.sh` → exit 0（対象 celeris・celerisctl）、1007 tests run: 1007 passed（8 slow）、2 skipped。
- `cargo clippy --workspace -- -D warnings` → exit 0。束の 4 target も `cargo clippy -p celeris --test browser_eval_surface_suite --test cos_delivery_suite --test routing_budget_suite --test unmapped_suite -- -D warnings` で exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0、`verdict=pass`、new=0・worsened=0、`repo_health_gate: all checks passed`。
- `{ git diff --name-only --no-renames "$CELERIS_WU_BASE"; git ls-files --others --exclude-standard; } | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は `crates/celeris/tests/` の下と、この進捗ファイルだけ。

## 未解決
- `crates/celeris/tests/script_exhaustive.rs:23` の未使用 import `ScriptGenerationControl` が test build で warning を出す。この file は r6 で R5 の担当外なので変えていない（`cargo clippy --workspace` は test target を含まないので落ちない）。
- 前回の試行で未解決だった `unpromoted_release` の分類は、reclass が r6 に直した。元の path のまま。

## 提案
- `script_exhaustive.rs` の未使用 import は、地図の参照を扱う R6 で target を触るときに消す。
- 束の build/link 時間と実行時間の前後比較は measure-build・measure-run の leaf が測る。
