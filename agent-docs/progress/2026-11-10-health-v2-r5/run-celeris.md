---
title: 健全性v2 R5 run-celeris — HEAD の celeris の試験を 2 回と束 4 個を 1 process で走らせ、after-run.json に足す
tasks: [01M76AG298YER7PJB6M781C77X]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R5 run-celeris

完了日: 2026-11-11

WorkUnit `run-celeris`（measure-run の 3 つ目の leaf）。HEAD はこの WU の base の commit（件名『R5 run-api: HEAD の task-api の試験を 2 回と束 10 個を 1 process で走らせ、after-run.json と進捗に記録する』）、base は `git merge-base HEAD main`。crates/ は変えていない。

## 結論
- celeris の試験は HEAD で 2 回とも 801 件すべて合格した（baseline は 802 件）。失敗・時間切れは 0 件。差の 1 件は R5 の dedupe-celeris が人の決定で消した試験で、`removals/celeris.json` に記録がある（下の「件数の差」）。
- 束 4 個は `cargo test -p celeris --test <bundle>` の 1 process の実行で全件合格した。各束の件数は `cargo nextest list -p celeris` の同じ binary の件数と同じ（合計 57 件）。1 process の中の env・CWD・時刻の汚染による失敗は無かった。
- env を書き換える `routing_eval_run.rs` と `cap_probe_real_daemon.rs` は束の main.rs の mod 宣言に無く、独立の test binary のまま（keep）。
- 落ちた試験が無いので、merge-base の worktree での単独実行（原因の切り分け）は要らなかった。bundle-fix に回す celeris の target は無い。
- load average の 1 分値の最大は HEAD 77.21、baseline 83.77 で、比は 0.922。閾値 167.54（baseline の 2 倍）を超えないので base_run は走らせていない（needed=false）。
- run1 が run2 より長い（118.89 秒と 54.85 秒）主な理由は、run1 の中で 2 つの試験 file が `cargo build -p celeris-campaign` を走らせたこと（下の「run1 と run2 の差」）。負荷の寄与はこの計測では分けられない。

## 結果（baseline.json の celeris と並べた表）
load average は 1 分値（各段の前 → 後）。秒は計測した壁時計、括弧内は nextest の Summary の秒。

| 項目 | baseline（label base、2026-11-10 17:37Z〜） | HEAD（after-run.json、2026-11-11 23:00Z〜） |
|---|---|---|
| test binary（`-p celeris`） | 57 | 46 |
| build `--no-run` | 124.64 秒・Compiling 14・link 57 | 130.95 秒・Compiling 17・link 46 |
| build の load | 49.28 → 55.36 | 39.54 → 32.02 |
| edit→verify（--list ＋再 build、celeris・celerisctl） | 37.11 秒・link 76 / 76 | 52.67 秒・link 61 / 61 |
| edit→verify の load | 55.81 → 53.55 | 47.20 → 58.19 |
| run1 | 184.30 秒（181.070）・802 件・失敗 0・SLOW 2 件 | 118.89 秒（117.982）・801 件・失敗 0・SLOW 7 件 |
| run1 の load | 62.46 → 83.77 | 77.21 → 43.90 |
| run2 | 69.98 秒（65.762）・802 件・失敗 0・SLOW 0 件 | 54.85 秒（53.272）・801 件・失敗 0・SLOW 0 件 |
| run2 の load | 83.77 → 62.19 | 43.90 → 44.15 |
| skip（nextest の skipped） | 2 | 2 |
| run の load の最大 | 83.77 | 77.21（比 0.922） |
| base_run | — | needed=false（77.21 ≤ 167.54） |

束 4 個（cargo test の 1 process の実行。build は先に `--no-run` で済ませた）:

| 束 | 中の module（元の独立 target） | exit | passed | failed | ignored | 失敗名 | 秒 | build 秒 | load（前 → 後） |
|---|---|---|---|---|---|---|---|---|---|
| browser_eval_surface_suite | browser_startup_reap・fake_http・workspace_roots | 0 | 7 | 0 | 0 | — | 1.79 | 0.63 | 44.15 → 44.15 |
| cos_delivery_suite | cos_chat_stagnation_notify・cos_chat_triage_notify・cos_chat_triage_scenarios・deployment_audit・releases_api・stranded_default_branch・stranded_integration | 0 | 28 | 0 | 0 | — | 39.02 | 0.43 | 44.15 → 38.68 |
| routing_budget_suite | script_compare_execution_gate・script_compare_report・tick_scale_baseline | 0 | 15 | 0 | 0 | — | 5.42 | 2.15 | 38.55 → 38.98 |
| unmapped_suite | legacy_event_startup・variant_propagation | 0 | 7 | 0 | 0 | — | 2.10 | 0.51 | 38.98 → 38.98 |

- 各束の件数は、HEAD の束の dir の各 file と、merge-base の `crates/celeris/tests/<module>.rs`（元の独立 target）の `#[test]`・`#[tokio::test]` の数が file ごとに同じ（例: releases_api 10・10、script_compare_report 8・8、variant_propagation 6・6）。束ねで落ちた試験・消えた試験は無い。
- 計測全体は 565.6 秒（build・edit→verify・run1・run2・束 4 個・base_run の判定）。
- build の Compiling 17 と baseline の 14 の差、edit→verify の 52.67 秒と 37.11 秒の差は、target の状態（HEAD の target は scratch の seed の写しから作った）と負荷の違いを含む。build の前後の比較は measure-build（`measure-build.md`）の交互の計測で読む。この表の build の数字だけで R5 の効果を判断しない。

## 件数の差（baseline 802 件、HEAD 801 件）
- 消えた 1 件は `src/db_maintenance/tests.rs` の `incremental_backup_progress_restarts_after_other_connection_writes`。dedupe-celeris が人の決定 `test-removal`（a）で消し、mutation は KILLED、記録は `removals/celeris.json`（`dedupe-celeris.md`）。
- `crates/celeris/` の `#[test]`・`#[tokio::test]` の数は、baseline の git_head と merge-base で 806、HEAD で 805（`git grep` で数えた）。`#[ignore]` は baseline の git_head・merge-base・HEAD のどれも 2。
- `cargo nextest list -p celeris` は 46 binary・803 件（ignored 2）で、run の 801 件と skipped 2 件に一致する。

## routing_eval_run と cap_probe_real_daemon が束に無いことの確認
- `grep -rnE 'routing_eval_run|cap_probe_real_daemon' crates/celeris/tests/*/main.rs` → 一致なし（exit 1）。束 4 個の main.rs の mod 宣言は上の表の module と `#[path]` の support（`tests/support/http_*.rs`・`support/git.rs`・`support/webhook.rs`・`support/mod.rs`）だけ。
- 2 つは独立の test binary のまま: `cargo nextest list -p celeris` に `routing_eval_run`（1 件）と `cap_probe_real_daemon`（6 件）がある。
- 2 つが env を書き換える箇所: `routing_eval_run.rs` の `std::env::set_var("PATH", …)`、`cap_probe_real_daemon.rs` の `PATH`・`CAP_MARK`・token の env の `set_var`。inventory.json の `env_mutation` も同じ行を挙げ、`bundle-celeris.md` は 2 つを keep（env の書き換え）に分けている。
- 束 4 個の dir と `crates/celeris/tests/support/`・`tests/support/` に `set_var`・`remove_var`・`set_current_dir` は無い（grep exit 1）。

## 落ちた試験ごとの原因の判定
- 落ちた試験は無い（run1・run2・束 4 個のどれでも failed 0、時間切れ 0）。merge-base の worktree での `cargo test -p celeris --test <元の target> -- <試験名> --exact` と HEAD の単独実行は、対象が無いので走らせていない。

## run1 と run2 の差
- HEAD の run1 では 7 件が 60 秒を超えた（SLOW）: `cap_probe_real_daemon` の 5 件（106.7〜108.2 秒）と `ops_r1_pipeline` の 2 件（98.3・98.9 秒）。run2 では 60 秒を超えた試験は無い。
- 2 つの試験 file は、`campaign_bin()` の `OnceLock` の中で `cargo build -p celeris-campaign --bin celeris-campaign` を走らせる（`cap_probe_real_daemon.rs` は binary が無いときだけ、`ops_r1_pipeline.rs` は毎回。新しければ cargo は何もしない）。measure.sh の build は `-p celeris` の test binary だけを作るので、run1 の時点で `celeris-campaign` の binary は target に無かった。
- target の `debug/celeris-campaign` の mtime は 23:08:29 UTC。run1 は 23:06:34〜23:08:33（prerun と run1 の log の mtime）で、binary は run1 の終わる 4 秒前に作られた。run2 では binary があるので build はすぐ終わる。run1 の壁時計の大半は、この試験の中の build を待った時間と読める。
- baseline の run1（184.30 秒）でも `ops_r1_pipeline` の 2 件が SLOW（152.0・154.0 秒）だった。2 つの試験 file は baseline の git_head と HEAD で差が無い。baseline の target に `celeris-campaign` が無かったかは記録が無く、未確認。
- このため run1 の秒は試験の実行時間より試験の中の build の時間を多く含む。HEAD と baseline の比較には run2（54.85 秒と 69.98 秒）の方が向く。ただし run2 の load は HEAD 43.90〜44.15、baseline 62.19〜83.77 と違い、run2 の秒の差のうち負荷の寄与と R5 の変更（test binary 57 → 46）の寄与は、この計測では分けられない。
- run1 の前の load 77.21 は prerun（`--no-run`）の直後の値で、host の他の process を含む。run1 の中の build が load にどれだけ寄与したかも分けられない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `sh agent-docs/progress/2026-11-10-health-v2-r5/v8-target-check.sh` → exit 0（`v8-target-check: ok`、v8 の build output 1 件）。`cargo clean -p v8` は要らなかった。
- `MEASURE_DEADLINE=<run の開始＋3000> bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh run head agent-docs/progress/2026-11-10-health-v2-r5/after-run.json celeris` → exit 0（565.6 秒、`ok: 3 crates, 0 skipped item(s)`）。celerisctl・task-api の記録は残っている。
- `cargo nextest list -p celeris --message-format json` → 46 binary・803 件（ignored 2）。束 4 個の件数は上の表の passed と同じ（ignored 0）。`routing_eval_run` 1 件、`cap_probe_real_daemon` 6 件。
- `grep -rnE 'routing_eval_run|cap_probe_real_daemon' crates/celeris/tests/*/main.rs` → exit 1（一致なし）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/after-run.json` → exit 0（`ok: 3 crates, 0 skipped item(s)`）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require agent-docs/progress/2026-11-10-health-v2-r5/after-run.json celerisctl,task-api,celeris` → exit 0（`required-complete: celerisctl, task-api, celeris`）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/baseline.json` → exit 0（baseline.json は変えていない）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate celeris --base "$(git merge-base HEAD main)"` → exit 0（試験関数 806 → 805、removals 1 件）
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`（`git merge-base HEAD main` 基準でも ok）。変えた path は after-run.json・after-run-raw/ の celeris の file・この進捗だけ
- `bash scripts/dev/test-affected.sh --list` → 対象なし（この WU は crates/ を変えていない）
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0（53 秒）
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`

## 未解決
- baseline の run1 の時点で target に `celeris-campaign` の binary が無かったかは未確認（baseline-raw に記録が無い）。
- run2 の秒の差（54.85 秒と 69.98 秒）のうち負荷と R5 の変更の寄与は分けられていない。

## 提案
- run-report へ: celeris の run1 は試験の中の `cargo build -p celeris-campaign` を含むので、HEAD と baseline の実行時間の比較は run2 を主に使い、run1 はこの事情を注記して並べる。
- measure.sh を後で直すなら、run1 の前の prerun で `celeris-campaign` の binary も build しておけば、run1 と run2 が同じ条件の試験の実行時間になる。この leaf は measure.sh を変えていない。
