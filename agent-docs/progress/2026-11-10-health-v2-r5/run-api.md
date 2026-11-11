---
title: 健全性v2 R5 run-api — HEAD の task-api の試験を 2 回と束 10 個を 1 process で走らせ、after-run.json に足す
tasks: [01M76AG298YER7PJB6M781C77X]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R5 run-api

完了日: 2026-11-11

WorkUnit `run-api`（measure-run の 2 つ目の leaf）。HEAD はこの WU の base の commit（件名『R5 tools-ctl: measure.sh の run に crate の追記・束の 1 process 実行・base の実行・--require を足し、celerisctl を測る』）、base は `git merge-base HEAD main`。crates/ は変えていない。

## 結論
- task-api の試験は HEAD で 2 回とも 877 件すべて合格した（baseline は 876 件）。失敗・時間切れは 0 件。
- 束 10 個は `cargo test -p task-api --test <bundle>` の 1 process の実行で全件合格した。各束の件数は `cargo nextest list -p task-api` の同じ binary の件数と同じ（合計 325 件）。1 process の中の env・CWD・時刻の汚染による失敗は無かった。
- 落ちた試験が無いので、merge-base の worktree での単独実行（原因の切り分け）は要らなかった。bundle-fix に回す task-api の target は無い。
- load average の 1 分値の最大は HEAD 48.56、baseline 49.28 で、比は 0.985。2 倍以下なので base_run は走らせていない（needed=false）。
- `browser_eval_surface_suite` の `browser_restore_deliver.rs` と `browser_restore_live_session.rs` の試験は、この計測では時刻を読む前に戻る（下の「時刻を読む 2 試験」）。時刻と順序の影響は実行では確かめられておらず、未確認。

## 結果（baseline.json の task-api と並べた表）
load average は 1 分値（各段の前 → 後）。秒は計測した壁時計、括弧内は nextest の Summary の秒。

| 項目 | baseline（label base、2026-11-10 17:37Z〜） | HEAD（after-run.json、2026-11-11 22:43Z〜） |
|---|---|---|
| test binary（`-p task-api`） | 114 | 65 |
| build `--no-run` | 133.25 秒・Compiling 76・link 114 | 97.16 秒・Compiling 76・link 65 |
| build の load | 23.05 → 59.44 | 39.83 → 38.06 |
| edit→verify（--list ＋再 build、7 crate） | 155.78 秒・link 328 / 333 | 106.47 秒・link 260 / 265 |
| run1 | 59.56 秒（56.135）・876 件・失敗 0 | 55.96 秒（54.172）・877 件・失敗 0 |
| run1 の load | 44.82 → 34.83 | 48.56 → 35.31 |
| run2 | 60.64 秒（57.519）・876 件・失敗 0 | 31.69 秒（29.675）・877 件・失敗 0 |
| run2 の load | 34.83 → 49.28 | 35.31 → 34.92 |
| skip（nextest の skipped） | 2 | 2 |
| run の load の最大 | 49.28 | 48.56（比 0.985） |
| base_run | — | needed=false（0.985 ≤ 2） |

束 10 個（cargo test の 1 process の実行。build は先に `--no-run` で済ませた）:

| 束 | exit | passed | failed | ignored | 失敗名 | 秒 | build 秒 | load（前 → 後） |
|---|---|---|---|---|---|---|---|---|
| browser_eval_surface_suite | 0 | 19 | 0 | 0 | — | 0.80 | 0.29 | 34.92 → 34.92 |
| cos_delivery_suite | 0 | 19 | 0 | 0 | — | 1.07 | 0.29 | 34.92 → 34.92 |
| cos_delivery_suite_a | 0 | 42 | 0 | 0 | — | 1.72 | 0.30 | 34.92 → 34.61 |
| cos_delivery_suite_b | 0 | 36 | 0 | 0 | — | 1.43 | 0.27 | 34.61 → 34.61 |
| cos_delivery_suite_c | 0 | 60 | 0 | 0 | — | 2.78 | 0.27 | 34.61 → 33.12 |
| routing_budget_suite | 0 | 24 | 0 | 0 | — | 7.59 | 0.27 | 33.12 → 31.51 |
| routing_budget_suite_a | 0 | 20 | 0 | 0 | — | 1.48 | 0.96 | 29.46 → 29.46 |
| run_worker_suite | 0 | 15 | 0 | 0 | — | 1.40 | 0.26 | 29.46 → 29.46 |
| unmapped_suite_a | 0 | 62 | 0 | 0 | — | 3.31 | 1.01 | 29.46 → 29.43 |
| unmapped_suite_b | 0 | 28 | 0 | 0 | — | 1.12 | 0.26 | 29.43 → 29.43 |

- 件数 877 と baseline の 876 の差 1 件は、baseline.json の git_head と今の merge-base の間に main に入った試験 `cos_triage.rs` の `cos_chat_triage_mark_and_audit_commit_together`（件名『table-writers: cos_inbox_items・chat_client_requests の書き手を task-core の chat module へ寄せる』）。`#[test]`・`#[tokio::test]` の数は baseline の head で 855、merge-base と HEAD で 856。R5 の変更による増減ではない。
- run2 の秒が run1 より短い（55.96 → 31.69 秒）。同じ時の load は 48.56 → 35.31 で、run2 の方が低い。秒の差が負荷によるかは未確認。
- build の Compiling 76 は baseline と同じ数。HEAD の target は scratch の seed の写しから作ったもので、依存の build の状態は baseline と違う。build の前後の比較は measure-build（`measure-build.md`）の方法で読む。
- 計測全体は 610.5 秒（build・edit→verify・run1・run2・束 10 個・base_run の判定）。

## 落ちた試験ごとの原因の判定
- 落ちた試験は無い（run1・run2・束 10 個のどれでも failed 0、時間切れ 0）。merge-base の worktree での `cargo test -p task-api --test <元の target> -- <試験名> --exact` と HEAD の単独実行は、対象が無いので走らせていない。

## 時刻を読む 2 試験（browser_eval_surface_suite）
- `browser_restore_deliver::restore_enters_observation_stop_until_session_end` と `browser_restore_live_session::restore_http_binds_to_real_isolated_session_and_never_opens_on_refusal` は、本体の先頭で `skip()`（`userns_gate::skip_unless_userns_tests`）が真なら戻る。この task は `CELERIS_USERNS_TESTS` を設定しないので、2 試験は `SystemTime::now` を読む前に戻り、合格として数えられた。束の 1 process の実行でも同じ（`browser_restore` だけを `--test-threads=1` で走らせると 2 passed・0.02 秒）。
- 静的に読んだ範囲では、時刻は各試験の中で読んで同じ試験の中で使う。`browser_restore_deliver.rs` は `now()` を identity の登録時刻に渡し、署名の `expires_at` は署名の時点の `OffsetDateTime::now_utc()`＋30 秒。`browser_restore_live_session.rs` は読んだ時刻を `restore_for_session` に渡す。束の中の他の試験と時刻・時計を共有する仕組み（process 全体の時計の差し替え・static）は無い。束の 10 個の dir と `tests/common/`・`userns_gate` に `set_var`・`remove_var`・`set_current_dir` は無い。
- 順序の確認として、`cargo test -p task-api --test browser_eval_surface_suite -- --test-threads=1`（名前順の直列）を 3 回走らせ、3 回とも 19 passed・0 failed（5.19・5.16・17.68 秒）。並列の既定の実行（上の表）も 19 passed。
- userns を使う実行（`CELERIS_USERNS_TESTS=1`）での時刻と順序の影響は、この task の制約（userns・実 browser の試験を走らせない）で確かめていない。release gate の本物の userns の試験に任せる。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `sh agent-docs/progress/2026-11-10-health-v2-r5/v8-target-check.sh` → exit 0（`v8-target-check: ok`、v8 の build output 1 件）
- `MEASURE_DEADLINE=<run の開始＋3000> bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh run head agent-docs/progress/2026-11-10-health-v2-r5/after-run.json task-api` → exit 0（610 秒、`ok: 2 crates, 0 skipped item(s)`）。celerisctl の記録は残っている。
- `cargo nextest list -p task-api --message-format json` → 束 10 個の件数が上の表の passed と同じ（ignored 0）
- `cargo test -p task-api --test browser_eval_surface_suite -- --test-threads=1` を 3 回 → 3 回とも exit 0・19 passed
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate task-api --base "$(git merge-base HEAD main)"` → exit 0（試験関数 856 → 856、表の行 757 → 757、名前で追った移動 325、removals 0 件）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/after-run.json` → exit 0（`ok: 2 crates, 0 skipped item(s)`）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require agent-docs/progress/2026-11-10-health-v2-r5/after-run.json celerisctl,task-api` → exit 0（`required-complete: celerisctl, task-api`）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/baseline.json` → exit 0（baseline.json は変えていない）
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`（`git merge-base HEAD main` 基準でも ok）。変えた path は after-run.json・after-run-raw/ の task-api の file・この進捗だけ
- `bash scripts/dev/test-affected.sh --list` → 対象なし（exit 0。この WU は crates/ を変えていない）
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0（93 秒）
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`

## 未解決
- 時刻を読む 2 試験の userns の実行での時刻・順序の影響は未確認（上の節）。

## 提案
- 無し。task-api に束ねが原因の失敗は無い。
