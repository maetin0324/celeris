---
title: 健全性v2 R5 measure-run — HEAD の試験の実行時間と flaky を baseline と並べ、束ねた 18 target の 1 process 実行を確かめた結果
tasks: [01M76AG298YER7PJB6M781C77X]
status: done
updated: 2026-11-12
completed: 2026-11-12
---
# 健全性v2 R5 measure-run

完了日: 2026-11-12

task 01M76AG298YER7PJB6M781C77X（R5 の段 measure の unit measure-run）の進捗。WorkUnit `run-report` がこの file を書いた（この leaf の進捗を兼ねる）。基準は `git merge-base HEAD main`。数値は `after-run.json`（label head）と `baseline.json`（label base）から写した。別の file から写した値は出典を書いた。この leaf はコードと計測の file を変えていない。

## 結論
- 4 crate（task-api・celeris・celerisctl・task-worker）の nextest は、HEAD で 2 回とも全件合格した。失敗 0、時間切れ 0。task-worker は base でも 1 回走らせ、全件合格した。
- 束ねた 18 target は、どれも `cargo test -p <crate> --test <bundle>` の 1 process の実行で合格した（437 件、failed 0）。bundle-fix の再実行でも同じ件数が合格した。1 process の中の env・CWD・時刻の汚染による失敗は無かった。
- 落ちた試験が無いので、原因の判定（束ね・負荷・その他）の対象は 0 件。bundle-fix は keep への戻しをしていない。
- base_run の要否: task-api・celeris・celerisctl は load average 1 分値の比が 2 以下なので要らなかった（needed=false）。task-worker は baseline に無いので base を 1 回走らせた（needed=true）。
- 秒の差は、負荷の違いと R5 の変更（test binary の減少）の寄与を分けられない所が多い（下の「秒の差の読み方」）。

## crate ごとの baseline と HEAD
秒は measure.sh が測った壁時計、括弧内は nextest の Summary の秒。件数は「実行 / 合格 / 失敗 / 時間切れ」、skip は nextest の skipped（`#[ignore]`）。load は load average の 1 分値（各回の前 → 後）。baseline は 2026-11-10 17:37Z 開始、HEAD は 2026-11-11 22:30Z〜23:17Z（crate ごとの session の開始時刻は下の表）。

| crate | 回 | 秒 | 件数 | skip | 失敗名 | load（前 → 後） |
|---|---|---|---|---|---|---|
| task-api | baseline run1 | 59.56（56.135） | 876 / 876 / 0 / 0 | 2 | — | 44.82 → 34.83 |
| task-api | baseline run2 | 60.64（57.519） | 876 / 876 / 0 / 0 | 2 | — | 34.83 → 49.28 |
| task-api | HEAD run1 | 55.96（54.172） | 877 / 877 / 0 / 0 | 2 | — | 48.56 → 35.31 |
| task-api | HEAD run2 | 31.69（29.675） | 877 / 877 / 0 / 0 | 2 | — | 35.31 → 34.92 |
| celeris | baseline run1 | 184.30（181.070） | 802 / 802 / 0 / 0 | 2 | — | 62.46 → 83.77 |
| celeris | baseline run2 | 69.98（65.762） | 802 / 802 / 0 / 0 | 2 | — | 83.77 → 62.19 |
| celeris | HEAD run1 | 118.89（117.982） | 801 / 801 / 0 / 0 | 2 | — | 77.21 → 43.90 |
| celeris | HEAD run2 | 54.85（53.272） | 801 / 801 / 0 / 0 | 2 | — | 43.90 → 44.15 |
| celerisctl | baseline run1 | 4.78（3.959） | 205 / 205 / 0 / 0 | 0 | — | 29.00 → 27.64 |
| celerisctl | baseline run2 | 4.45（3.605） | 205 / 205 / 0 / 0 | 0 | — | 27.64 → 27.64 |
| celerisctl | HEAD run1 | 7.37（5.962） | 205 / 205 / 0 / 0 | 0 | — | 55.14 → 52.58 |
| celerisctl | HEAD run2 | 13.65（10.832） | 205 / 205 / 0 / 0 | 0 | — | 52.58 → 54.26 |
| task-worker | baseline | —（baseline に無い） | — | — | — | — |
| task-worker | base_run（base の git worktree） | 22.00（20.570） | 1283 / 1283 / 0 / 0 | 9 | — | 21.89 → 28.17 |
| task-worker | HEAD run1 | 25.79（23.968） | 1283 / 1283 / 0 / 0 | 9 | — | 27.38 → 25.86 |
| task-worker | HEAD run2 | 22.39（21.648） | 1283 / 1283 / 0 / 0 | 9 | — | 25.86 → 23.12 |

test binary の数（build の `--no-run` の記録）と HEAD の session:

| crate | baseline | HEAD | HEAD の session の開始（UTC） | 記録した leaf |
|---|---|---|---|---|
| task-api | 114 | 65 | 2026-11-11 22:43:34 | run-api |
| celeris | 57 | 46 | 2026-11-11 23:00:55 | run-celeris |
| celerisctl | 19 | 15 | 2026-11-11 22:30:52 | tools-ctl |
| task-worker | —（baseline に無い） | 21 | 2026-11-11 23:17:10 | run-worker |

- 件数の差: task-api の 876 → 877 は、baseline の git_head の後に main に入った試験 1 件（`cos_triage.rs` の `cos_chat_triage_mark_and_audit_commit_together`）による。celeris の 802 → 801 は、R5 の dedupe-celeris が人の決定で消した試験 1 件で、`removals/celeris.json` に記録がある（どちらも出典は `run-api.md`・`run-celeris.md`）。
- task-worker の合格 1283 件には、`CELERIS_USERNS_TESTS` が無いので本体を走らせずに戻った userns の試験 34 件と、ssh の多重接続が無いので戻った `ssh_localhost` の 8 件が入っている（HEAD での数え上げ。base の数は推定。出典は `run-worker.md`）。task-api の束 `browser_eval_surface_suite` の時刻を読む 2 試験も、同じ理由で時刻を読む前に戻る（出典は `run-api.md`）。

## base_run の要否と結果
measure.sh は、baseline と HEAD の各 crate の run1・run2 の load（前と後）の 1 分値の最大を比べ、比が 2 を超えるか baseline に crate が無ければ base を 1 回走らせる。

| crate | baseline の最大 | HEAD の最大 | 比 | 要否 | 結果 |
|---|---|---|---|---|---|
| task-api | 49.28 | 48.56 | 0.985 | 不要（needed=false） | 走らせていない |
| celeris | 83.77 | 77.21 | 0.922 | 不要（needed=false） | 走らせていない |
| celerisctl | 29.00 | 55.14 | 1.901 | 不要（needed=false） | 走らせていない |
| task-worker | —（baseline に無い） | 27.38 | — | 要（needed=true） | exit 0・22.00 秒（20.570）・1283 件合格・失敗 0・load 21.89 → 28.17・build 95.05 秒 |

- task-worker の base_run は、計測の時点の `git merge-base HEAD main` の git worktree で `cargo nextest run -p task-worker --no-fail-fast` を 1 回走らせた（build は先に `--no-run`）。

## 束ごとの cargo test の結果（18 束）
「run 段」は `after-run.json` の bundles の記録（各 run の leaf が measure.sh で測った）。「bundle-fix」は bundle-fix の leaf が提出前に同じコマンドで走らせた結果（出典は `bundle-fix.md`。2026-11-12 00:00Z〜00:08Z）。run 段の秒は build（先に `--no-run`）の後の `cargo test` の壁時計、bundle-fix の秒は libtest の `finished in` の値。

| crate | 束 | run 段の exit | passed | failed | ignored | 失敗名 | 秒 | load の前 | bundle-fix の passed / failed | bundle-fix の秒 |
|---|---|---|---|---|---|---|---|---|---|---|
| task-api | browser_eval_surface_suite | 0 | 19 | 0 | 0 | — | 0.80 | 34.92 | 19 / 0 | 1.76 |
| task-api | cos_delivery_suite | 0 | 19 | 0 | 0 | — | 1.07 | 34.92 | 19 / 0 | 2.14 |
| task-api | cos_delivery_suite_a | 0 | 42 | 0 | 0 | — | 1.72 | 34.92 | 42 / 0 | 4.59 |
| task-api | cos_delivery_suite_b | 0 | 36 | 0 | 0 | — | 1.43 | 34.61 | 36 / 0 | 3.05 |
| task-api | cos_delivery_suite_c | 0 | 60 | 0 | 0 | — | 2.78 | 34.61 | 60 / 0 | 11.37 |
| task-api | routing_budget_suite | 0 | 24 | 0 | 0 | — | 7.59 | 33.12 | 24 / 0 | 29.21 |
| task-api | routing_budget_suite_a | 0 | 20 | 0 | 0 | — | 1.48 | 29.46 | 20 / 0 | 3.68 |
| task-api | run_worker_suite | 0 | 15 | 0 | 0 | — | 1.40 | 29.46 | 15 / 0 | 2.75 |
| task-api | unmapped_suite_a | 0 | 62 | 0 | 0 | — | 3.31 | 29.46 | 62 / 0 | 6.50 |
| task-api | unmapped_suite_b | 0 | 28 | 0 | 0 | — | 1.12 | 29.43 | 28 / 0 | 3.22 |
| celeris | browser_eval_surface_suite | 0 | 7 | 0 | 0 | — | 1.79 | 44.15 | 7 / 0 | 2.21 |
| celeris | cos_delivery_suite | 0 | 28 | 0 | 0 | — | 39.02 | 44.15 | 28 / 0 | 38.84 |
| celeris | routing_budget_suite | 0 | 15 | 0 | 0 | — | 5.42 | 38.55 | 15 / 0 | 3.68 |
| celeris | unmapped_suite | 0 | 7 | 0 | 0 | — | 2.10 | 38.98 | 7 / 0 | 2.72 |
| celerisctl | browser_eval_surface_suite | 0 | 22 | 0 | 0 | — | 7.75 | 52.80 | 22 / 0 | 4.40 |
| celerisctl | unmapped_suite | 0 | 2 | 0 | 0 | — | 6.62 | 52.10 | 2 / 0 | 6.97 |
| task-worker | browser_eval_surface_browser_action_recovery_py_suite | 0 | 1 | 0 | 0 | — | 0.37 | 23.12 | 1 / 0 | 0.49 |
| task-worker | browser_eval_surface_suite | 0 | 30 | 0 | 0 | — | 2.29 | 23.12 | 30 / 0 | 1.98 |

- 合計: task-api 10 束 325 件、celeris 4 束 57 件、celerisctl 2 束 24 件、task-worker 2 束 31 件。18 束 437 件で、build の失敗・時間切れ・failed は 0。
- 各束の件数は、各 leaf が `cargo nextest list -p <crate>` の同じ binary の件数と同じことを確かめている（出典は各 leaf の進捗）。

## 落ちた試験ごとの原因の判定と bundle-fix の処置
| 区分 | 件数 | 内容 |
|---|---|---|
| 束ね（同じ試験が base の独立 target で通り、束の中で落ちる） | 0 | 束の 1 process の実行で落ちた試験は無い |
| 負荷 | 0 | この計測（run1・run2・base_run・18 束・bundle-fix の再実行）で落ちた試験は無い |
| その他 | 0 | 同上 |

- 落ちた試験が無いので、base の worktree での `--exact` の単独実行（原因の切り分け）は走らせていない。
- bundle-fix の処置: ADR 2026-11-10-test-target-bundles の D3 による keep への戻しはしていない。`crates/`・`inventory.json`・ADR は変えていない（出典は `bundle-fix.md`）。
- 計測の外の参考: `login-flake.md` の task-worker の login 2 試験（`daemon_post_login_waits_through_slow_shibboleth_hops`・`launcher_credential_post_login_consent_page_is_held_with_its_reason`）は、workspace の test-affected（load average 約 59）でだけ落ちた負荷依存の失敗と判定されている。この計測の task-worker の run1・run2・base_run ではどちらも合格した（failed_tests が空）。判定は `login-flake.md` のまま。

## 秒の差の読み方
- task-api: run1 は baseline 59.56 秒・HEAD 55.96 秒で、前の load は 44.82 と 48.56 で近い。run2 は 60.64 秒 → 31.69 秒と短くなったが、後の load が 49.28 と 34.92 で違う。test binary の 114 → 65 の寄与と負荷の寄与は、この計測では分けられない。
- celeris: run1 は試験の中で `cargo build -p celeris-campaign` を待つ時間を含む（出典は `run-celeris.md`）ので、比べるなら run2 を使う。run2 は 69.98 秒 → 54.85 秒だが、load が baseline 83.77 → 62.19、HEAD 43.90 → 44.15 と違い、test binary の 57 → 46 の寄与と負荷の寄与は分けられない。
- celerisctl: HEAD の方が長い（Summary 3.959・3.605 秒 → 5.962・10.832 秒）。load の最大も 29.00 → 55.14 で、比は 1.901。比が 2 以下なので base_run は走らせておらず、差が R5 の変更によるか負荷によるかは分けられない。参考に、tools-ctl の台本の確認で base（merge-base）の celerisctl を load 36.89〜37.85 で 1 回走らせた Summary は 6.08 秒・205 件合格だった（出典は `tools-ctl.md`。この値は `after-run.json` に無い）。
- task-worker: base 20.570 秒と HEAD 23.968・21.648 秒（Summary）の差は 1〜3 秒で、load は 3 回とも 21〜28。test binary の減少の寄与と揺れは、1 回ずつの計測では分けられない。

## 各 leaf の要約と commit の件名
| leaf | 要約 | commit の件名 |
|---|---|---|
| tools-ctl | measure.sh の run に crate の追記・束の 1 process 実行・base_run・`--require` を足し、celerisctl を測った（2 回 205 件合格、束 2 個合格、needed=false） | 『R5 tools-ctl: measure.sh の run に crate の追記・束の 1 process 実行・base の実行・--require を足し、celerisctl を測る』 |
| run-api | task-api を 2 回（877 件合格）と束 10 個（325 件合格）を測った。needed=false | 『R5 run-api: HEAD の task-api の試験を 2 回と束 10 個を 1 process で走らせ、after-run.json と進捗に記録する』 |
| run-celeris | celeris を 2 回（801 件合格）と束 4 個（57 件合格）を測った。needed=false。run1 の長さは試験の中の build による | 『R5 run-celeris: HEAD の celeris の試験を 2 回と束 4 個を 1 process で走らせ、after-run.json と進捗に記録する』 |
| run-worker | task-worker を 2 回（1283 件合格）と束 2 個（31 件合格）、base を 1 回（1283 件合格）測った | 『R5 run-worker: HEAD の task-worker の試験を 2 回と束 2 個を 1 process、base を 1 回走らせ、after-run.json と進捗に記録する』 |
| integrate-run | run 段の 4 leaf の統合 | 『integrate wu/run-worker (phase run)』 |
| bundle-fix | 束ねが原因の失敗が無いことを確かめ、18 束の cargo test を再実行した（437 件合格）。keep への戻しは無し | 『R5 bundle-fix: 束ねが原因の失敗が無いことを確かめ、18 束の cargo test の結果を進捗に記録する』 |
| run-report | この進捗を書いた | 『phase 1: measure-run: baseline と HEAD の実行時間・flaky・18 束の結果を進捗に並べる』 |

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`、cases.py と slices-check は `git merge-base HEAD main`）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/after-run.json` → exit 0（`ok: 4 crates, 0 skipped item(s)`）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require agent-docs/progress/2026-11-10-health-v2-r5/after-run.json celerisctl,task-api,celeris,task-worker` → exit 0（`required-complete: celerisctl, task-api, celeris, task-worker`）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/baseline.json` → exit 0（`ok: 3 crates, 0 skipped item(s)`）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate <crate> --base "$(git merge-base HEAD main)"` → 4 crate とも exit 0・OK（試験関数 task-api 856 → 856、celeris 806 → 805（removals 1 件）、celerisctl 205 → 205、task-worker 1292 → 1292）
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`（`$CELERIS_WU_BASE` 基準でも ok）
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ 変えた path はこの進捗だけ
- `git diff --quiet HEAD -- <after-run.json・baseline.json・measure.sh・crates/>` → exit 0（計測の file とコードは変えていない）
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → `check-doc-layout: ok`、`bash scripts/dev/check-doc-links.sh` → `check-doc-links: ok`
- `bash scripts/dev/test-affected.sh --list` → 対象なし（exit 0）
- `bash scripts/dev/test-affected.sh`（基準は `$CELERIS_WU_BASE`）→ exit 0（115 秒、`repo_health_gate: all checks passed`、`test-affected: 試験対象の変更なし（docs のみ等）`）
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0（63 秒）
- 検査の間の load average 1 分値は 11.35〜14.59。

## 未解決
- task-api の run2、celeris の run2、celerisctl の 2 回の秒の差は、負荷の違いと R5 の変更の寄与を分けられていない。
- task-worker は baseline に無いので、R5 の前の計測（baseline の時刻）との比較は無い。比べられるのは同じ時間帯の base の 1 回だけ。
- task-worker の base の userns・ssh の skip の数（34・8）は数え上げていない推定（出典は `run-worker.md`）。
- task-api の `browser_eval_surface_suite` の時刻を読む 2 試験の、userns を使う実行での時刻・順序の影響は未確認（この task の制約で userns の試験を走らせていない。release gate に任せる。出典は `run-api.md`）。
- baseline の celeris の run1 の時点で target に `celeris-campaign` の binary が無かったかは未確認（出典は `run-celeris.md`）。

## 提案
- 負荷と変更の寄与を分けたいなら、run でも measure-build の compare と同じく base と HEAD を同じ時間帯に交互に走らせる。今の base_run は load の比が 2 を超えた時だけ走るので、celerisctl のように比 1.901 で秒が 2〜3 倍になった時は比べる値が無い。この leaf は measure.sh を変えていない。
- measure.sh の run1 の前の prerun で `celeris-campaign` の binary も build すれば、celeris の run1 と run2 が同じ条件になる（run-celeris の提案）。
- nextest の Summary だけでは試験の中の skip（return で抜ける試験）が passed に入るので、`--success-output` で `SKIPPED (` の数を数えて記録する段を足すと、本体まで走った件数を crate の間で比べられる（run-worker の提案）。
