---
title: 健全性v2 R5 bundle-fix — 束ねが原因で 1 process の実行で落ちた target の有無を確かめ、全束が cargo test で通ることを確かめる
tasks: [01M76AG298YER7PJB6M781C77X]
status: done
updated: 2026-11-12
completed: 2026-11-12
---
# 健全性v2 R5 bundle-fix

完了日: 2026-11-12

WorkUnit `bundle-fix`（measure-run の段 fix の leaf）。HEAD はこの WU の base の commit（件名『integrate wu/run-worker (phase run)』）、base は `git merge-base HEAD main`。

## 結論
- 束ねが原因の試験は無い。run 段の 4 leaf（`tools-ctl.md`・`run-api.md`・`run-celeris.md`・`run-worker.md`）と `after-run.json` の bundles で、18 束はどれも `cargo test -p <crate> --test <bundle>` の 1 process の実行で exit 0・failed 0 だった。base の独立 target で切り分ける対象の試験も無い。
- このため ADR 2026-11-10-test-target-bundles の D3 による keep への戻しはしていない。`crates/`・`inventory.json`・ADR は変えていない。`after-run.json` も測り直していない（変えた crate が無いため）。
- 負荷が原因の失敗も、run 段とこの leaf の実行では出ていない。`login-flake.md` の 2 試験は run-worker の 4 回の実行ですべて合格した（判定は `login-flake.md` のまま）。
- 提出の前に check 1・2 の cargo test（下の表）を走らせ、18 束 437 件がすべて合格した。

## run 段の bundles（after-run.json の記録）
| crate | 束の数 | 合計の passed | failed | 記録した leaf |
|---|---|---|---|---|
| task-api | 10 | 325 | 0 | run-api |
| celeris | 4 | 57 | 0 | run-celeris |
| celerisctl | 2 | 24 | 0 | tools-ctl |
| task-worker | 2 | 31 | 0 | run-worker |

- 4 crate の run1・run2（nextest）も failed 0・時間切れ 0（task-api 877 件、celeris 801 件、celerisctl 205 件、task-worker 1283 件）。

## check 1・2 の cargo test（この leaf で実行、2026-11-12 00:00Z〜00:08Z）
plan の check 1・2 と同じコマンドを、渡された `CARGO_TARGET_DIR` のまま走らせた。先頭の `v8-target-check.sh` は 2 回とも `v8-target-check: ok`（v8 の build output 1 件）。秒は libtest の `finished in` の値。

check 1: `cargo test -p task-api --test <10 束>` → exit 0（194 秒。build 2 分 05 秒を含む。load average 1 分値 18.42 → 62.88）

| 束 | passed | failed | ignored | 秒 |
|---|---|---|---|---|
| browser_eval_surface_suite | 19 | 0 | 0 | 1.76 |
| cos_delivery_suite | 19 | 0 | 0 | 2.14 |
| cos_delivery_suite_a | 42 | 0 | 0 | 4.59 |
| cos_delivery_suite_b | 36 | 0 | 0 | 3.05 |
| cos_delivery_suite_c | 60 | 0 | 0 | 11.37 |
| routing_budget_suite | 24 | 0 | 0 | 29.21 |
| routing_budget_suite_a | 20 | 0 | 0 | 3.68 |
| run_worker_suite | 15 | 0 | 0 | 2.75 |
| unmapped_suite_a | 62 | 0 | 0 | 6.50 |
| unmapped_suite_b | 28 | 0 | 0 | 3.22 |

check 2: `cargo test -p celeris|celerisctl|task-worker --test <各束>` → exit 0（285 秒。build は 1 分 51 秒・57.75 秒・54.70 秒。load average 1 分値 61.65 → 61.75）

| crate | 束 | passed | failed | ignored | 秒 |
|---|---|---|---|---|---|
| celeris | browser_eval_surface_suite | 7 | 0 | 0 | 2.21 |
| celeris | cos_delivery_suite | 28 | 0 | 0 | 38.84 |
| celeris | routing_budget_suite | 15 | 0 | 0 | 3.68 |
| celeris | unmapped_suite | 7 | 0 | 0 | 2.72 |
| celerisctl | browser_eval_surface_suite | 22 | 0 | 0 | 4.40 |
| celerisctl | unmapped_suite | 2 | 0 | 0 | 6.97 |
| task-worker | browser_eval_surface_browser_action_recovery_py_suite | 1 | 0 | 0 | 0.49 |
| task-worker | browser_eval_surface_suite | 30 | 0 | 0 | 1.98 |

- 各束の passed は `after-run.json` の bundles の passed と同じ。束ねが原因の失敗も、負荷による失敗も無かった。
- `cargo test -p <crate>` は crate ごとに features の解決が違うので、check 2 でも 3 crate とも依存の一部を build し直した（Compiling は check 1 で 76 行、check 2 で 55 行）。どの組の成果も同じ target に残るので、check の再実行では build は短くなる見込み（未確認）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`、cases.py と slices-check は `git merge-base HEAD main`）
- check 1・2: 上の表（どちらも exit 0）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate <crate> --base "$(git merge-base HEAD main)"` → 4 crate とも exit 0・OK（試験関数 task-api 856 → 856、celeris 806 → 805（removals 1 件）、celerisctl 205 → 205、task-worker 1292 → 1292）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --check agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0（`352 targets … 18 bundles, 0 error(s)`）
- `python3 scripts/dev/test-target-inventory.py --json >/dev/null` → exit 0
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require agent-docs/progress/2026-11-10-health-v2-r5/after-run.json celerisctl,task-api,celeris,task-worker` → exit 0（`required-complete: celerisctl, task-api, celeris, task-worker`）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/after-run.json` → exit 0（`ok: 4 crates, 0 skipped item(s)`）
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`
- `bash scripts/dev/test-affected.sh --list` → 対象なし（exit 0。この WU は crates/ を変えていない）
- `bash scripts/dev/test-affected.sh`（基準は `$CELERIS_WU_BASE`）→ exit 0（121 秒、`repo_health_gate: all checks passed`、`test-affected: 試験対象の変更なし（docs のみ等）`）
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`
- 範囲 check（plan の check 6 と同じコマンド）→ 範囲外の path なし
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0（97 秒）
- 変えた path はこの進捗だけ。

## 未解決
- 無し。

## 提案
- 無し。
