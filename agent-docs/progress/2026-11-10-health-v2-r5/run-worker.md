---
title: 健全性v2 R5 run-worker — HEAD の task-worker の試験を 2 回、束 2 個を 1 process、base を 1 回走らせ、after-run.json に足す
tasks: [01M76AG298YER7PJB6M781C77X]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R5 run-worker

完了日: 2026-11-11

WorkUnit `run-worker`（measure-run の 4 つ目の leaf）。HEAD はこの WU の base の commit（件名『R5 run-celeris: HEAD の celeris の試験を 2 回と束 4 個を 1 process で走らせ、after-run.json と進捗に記録する』）、base は `git merge-base HEAD main`。crates/ は変えていない。

## 結論
- task-worker の試験は HEAD で 2 回とも、base（merge-base の git worktree）で 1 回、1283 件すべて合格した。失敗・時間切れ・SLOW（60 秒超）は 0 件。
- 束 2 個は `cargo test -p task-worker --test <bundle>` の 1 process の実行で全件合格した（browser_eval_surface_suite 30 件、browser_eval_surface_browser_action_recovery_py_suite 1 件）。時刻を読む `browser_control_gate_wire.rs` を含め、1 process の中の env・CWD・時刻の汚染による失敗は無かった。bundle-fix に回す task-worker の target は無い。
- login の 2 試験（`daemon_post_login_waits_through_slow_shibboleth_hops`・`launcher_credential_post_login_consent_page_is_held_with_its_reason`）は run1・run2・base_run と skip の数え上げの実行の 4 回とも合格した。`login-flake.md` の判定（load average 約 59 の test-affected でだけ落ちる負荷依存の失敗で、R5 の差は 0）と食い違わない。
- task-worker は baseline.json に無いので、base_run は比べる値が無く（needed=true、ratio=null）、base の nextest を 1 回走らせた。HEAD と base の比較はこの 1 回と並べる。
- 1283 件の合格には、`CELERIS_USERNS_TESTS` が無いので本体を走らせずに return した userns の試験 34 件と、ssh の多重接続が無いので return した `ssh_localhost` の 8 件が入っている。本体まで走った試験は 1241 件。

## 結果（HEAD と base を並べた表）
load average は 1 分値（各段の前 → 後）。秒は計測した壁時計、括弧内は nextest の Summary の秒。MEASURE_DEADLINE は run の開始＋3000 秒（00:06:15Z）にした。計測は 23:17:10Z に始まり 857.5 秒で終わり、時間切れ・skipped は無い。

| 項目 | base（base_run、merge-base の git worktree） | HEAD run1 | HEAD run2 |
|---|---|---|---|
| test binary（`-p task-worker`） | 25 | 21 | 21 |
| 試験の実行 | 22.00 秒（20.570） | 25.79 秒（23.968） | 22.39 秒（21.648） |
| 件数（run / passed / failed / 時間切れ） | 1283 / 1283 / 0 / 0 | 1283 / 1283 / 0 / 0 | 1283 / 1283 / 0 / 0 |
| 失敗名 | — | — | — |
| SLOW（60 秒超） | 0 | 0 | 0 |
| nextest の skipped（`#[ignore]`） | 9 | 9 | 9 |
| userns の skip（passed に入る） | 34（推定、下の注） | 34（数え上げの実行） | 同左 |
| ssh 多重接続なしの skip（passed に入る） | 8（推定、下の注） | 8（数え上げの実行） | 同左 |
| load average | 21.89 → 28.17 | 27.38 → 25.86 | 25.86 → 23.12 |

build と edit→verify（HEAD だけ。base_run は試験の前の build だけを測る）:

| 項目 | base | HEAD |
|---|---|---|
| build `--no-run` | 95.05 秒・Compiling 5（task-core・celeris-credentiald・task-ops・task-worker・task-api。依存の外部 crate は HEAD の build の成果を使った） | 138.4 秒・Compiling 76・link 21 / 21 |
| build の load | 記録なし | 14.08 → 34.04 |
| edit→verify（`crates/task-worker/src/lib.rs` の編集、--list ＋再 build） | — | 131.69 秒（list 0.14 秒・再 build 131.55 秒、Compiling 7）・対象 7 crate（celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）・link 265 / 265 |
| edit→verify の前の settle | — | 239.9 秒・Compiling 15 |
| edit→verify の load | — | 54.07 → 41.29 |

- build の秒は base と HEAD で条件が違う（HEAD は scratch の seed の写しの target から 76 単位を作り、base はその後に 5 単位だけを作った）。build の前後の比較は measure-build（`measure-build.md`）の交互の計測で読む。
- 試験の秒の差（base 20.570 秒、HEAD 21.648〜23.968 秒）は 1〜3 秒で、load は 3 回とも 21〜28 と近い。test binary 25 → 21 の寄与と揺れは、1 回ずつの計測では分けられない。

束 2 個（cargo test の 1 process の実行。build は先に `--no-run` で済ませた）:

| 束 | 中の module（元の独立 target） | exit | passed | failed | ignored | 失敗名 | 秒 | build 秒 | load（前 → 後） |
|---|---|---|---|---|---|---|---|---|---|
| browser_eval_surface_suite | browser_control_gate_wire・browser_live_cross・browser_loopback_prod_build・browser_prod_admission・browser_shared_cdp_events | 0 | 30 | 0 | 0 | — | 2.29 | 0.28 | 23.12 → 22.87 |
| browser_eval_surface_browser_action_recovery_py_suite | browser_action_recovery | 0 | 1 | 0 | 0 | — | 0.37 | 0.29 | 23.12 → 23.12 |

- 各束の件数は、HEAD の束の dir の各 file と merge-base の `crates/task-worker/tests/<module>.rs` の `#[test]`・`#[tokio::test]` の数が file ごとに同じ（browser_control_gate_wire 5・5、browser_live_cross 5・5、browser_loopback_prod_build 1・1、browser_prod_admission 18・18、browser_shared_cdp_events 1・1、browser_action_recovery 1・1）。`cargo nextest list -p task-worker` の同じ binary の件数（30・1）とも同じ。
- `browser_control_gate_wire.rs` は `SystemTime::now()`（`now()` で store に渡す）と `Instant::now()`（60 秒の待ちの期限）を読むだけで、時計を差し替えない。束の 2 つの dir に `set_var`・`remove_var`・`set_current_dir` は無い（grep exit 1）。
- 束の中の試験は userns の gate（`tests/userns_gate`）を呼ばない。束の 31 件はどれも本体まで走った。
- browser_action_recovery の HEAD と merge-base の差は、dir が 1 段深くなった分の `include_str!("../browser_action_recovery.py")` の 1 行だけ。

## skip の内訳
- nextest の skipped 9 件は `#[ignore]`: lib の 4 件（`aider::tests::real_aider_binary_end_to_end`、`browser::tests::p4c_conformance_backend_protocol`・`p4c_fallback_real_harness_scenario`・`p4c_runner_record_routes_and_falls_back`）、`browser_runtime_isolated` の helper 3 件、`ssh_cluster_manual` の 2 件。
- userns の skip 34 件: `CELERIS_USERNS_TESTS` 等を設定しない環境（`env | grep` で未設定を確認）で `cargo nextest run -p task-worker --no-fail-fast --success-output immediate --status-level pass` を 1 回走らせ、`SKIPPED (userns test, not passed)` を出した試験を数えた（24.2 秒、1283 passed、load 47.57 → 51.79）。内訳は lib 16、browser_restore_deliver 4、browser_runtime_isolated 4、browser_sandbox_artifacts 2、browser_cdp_sink・browser_egress_relay・browser_h3_wire・browser_injection_attacks・browser_injection_wire・browser_launcher_ptrace・browser_runtime_supervisor・browser_shared_cdp 各 1。試験名の一覧は `after-run-raw/userns-skips-task-worker.txt`。
- ssh の skip 8 件: 同じ実行で `ssh_localhost` の 8 件が `skip: celeris-localhost への多重接続が無い` を出した（`ssh_localhost` は 9 件）。
- base の 34 件・8 件は数え上げの実行をしていない推定。merge-base と HEAD の `crates/task-worker` の差は束ねた 6 file の移動と上の 1 行・束の main.rs 2 file だけで、gate を呼ぶ file（`src/` と `tests/` の束の外の file）の差は 0。

## 落ちた試験ごとの原因の判定
- run1・run2・base_run・束 2 個のどれでも落ちた試験は無い（failed 0、時間切れ 0）。merge-base の worktree での `--exact` の実行と HEAD の単独の実行は、対象が無いので走らせていない。
- login の 2 試験（`login-flake.md` と照らした）:
  - `browser::post_login_tests::daemon_post_login_waits_through_slow_shibboleth_hops`: run1・run2・base_run で合格（failed_tests が空）。数え上げの実行では `PASS [21.390s]`。
  - `browser::launcher_run::tests::launcher_login::launcher_credential_post_login_consent_page_is_held_with_its_reason`: run1・run2・base_run で合格。数え上げの実行では `PASS [6.350s]`。
  - `login-flake.md` では 2 試験は workspace の test-affected（5952 件、load average 約 59）の 3 回目の統合でだけ落ち、単独の 3 回（load 約 13）は合格だった。今回は `-p task-worker` だけの実行で load 21〜52 の 4 回とも合格した。R5 の差に login のコードが無いこと（`git diff --name-only $(git merge-base HEAD main) HEAD -- 'crates/task-worker/src/browser*' crates/celeris-credentiald/ | wc -l` → 0）も同じ。判定は `login-flake.md` のまま（R5 が原因ではない負荷依存の失敗）。落ちなかったので単独の再実行はしていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `sh agent-docs/progress/2026-11-10-health-v2-r5/v8-target-check.sh` → exit 0（`v8-target-check: ok`、v8 の build output 1 件）。`cargo clean -p v8` は要らなかった。
- `MEASURE_DEADLINE=<run の開始＋3000> bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh run head agent-docs/progress/2026-11-10-health-v2-r5/after-run.json task-worker` → exit 0（857.5 秒、`ok: 4 crates, 0 skipped item(s)`）。celerisctl・task-api・celeris の記録は残っている。
- `cargo nextest list -p task-worker --message-format json` → 21 binary・1292 件（ignored 9）。束の件数は 30・1。
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/after-run.json` → exit 0（`ok: 4 crates, 0 skipped item(s)`）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require agent-docs/progress/2026-11-10-health-v2-r5/after-run.json celerisctl,task-api,celeris,task-worker` → exit 0（`required-complete: celerisctl, task-api, celeris, task-worker`）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/baseline.json` → exit 0、`git diff --quiet HEAD -- …/baseline.json` → exit 0（baseline.json は変えていない）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate task-api --base "$(git merge-base HEAD main)"` → exit 0（試験関数 856 → 856）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate task-worker --base "$(git merge-base HEAD main)"` → exit 0（試験関数 1292 → 1292、removals 0 件）
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`（`git merge-base HEAD main` 基準でも ok）。変えた path は after-run.json・after-run-raw/ の task-worker の file と steps.jsonl・worktree.log・この進捗だけ
- `bash scripts/dev/test-affected.sh --list` → 対象なし（この WU は crates/ を変えていない）
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0（1 分 30 秒）
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`

## 未解決
- base の userns・ssh の skip の数は数え上げの実行をしていない推定（gate を呼ぶ file の差が 0 であることからの推定）。
- task-worker には baseline の値が無いので、R5 の前の計測（baseline のラベルの時刻）との比較は無い。比べられるのは同じ時間帯の base の 1 回だけ。

## 提案
- run-report へ: task-worker の行は baseline の列を「—（baseline に無い）」にし、base_run（1 回）と HEAD の 2 回を並べる。合格 1283 件のうち 42 件（userns 34・ssh 8）は本体を走らせていないことを注記する。
- measure.sh を後で直すなら、nextest の Summary だけでは試験の中の skip（return で抜ける試験）が passed に入って見えないので、`--success-output` で `SKIPPED (` の数を数えて記録する段を足すと、crate の間で本体まで走った件数を比べられる。この leaf は measure.sh を変えていない。
