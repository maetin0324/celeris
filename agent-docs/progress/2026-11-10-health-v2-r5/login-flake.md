---
title: R5 login-flake — 段 final の統合で負荷により落ちた browser login の 2 試験の確認
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-11
---
# R5 login-flake — 段 final の統合で負荷により落ちた browser login の 2 試験の確認

完了日: 2026-11-11

結論: 計画 v6 の integrate-final で落ちた task-worker の 2 試験は、R5 の変更が原因ではない。
R5 は 2 試験の file も login の製品コード（`crates/task-worker/src/browser*`、`crates/celeris-credentiald/`）も変えていない。
2 試験はこの WU の HEAD で単独に 3 回走らせ、3 回とも合格した（load average 約 13）。
直す owner は task-worker の browser login と celeris-credentiald の injection の待ちで、R5 の write set の外なので、R5 では直さず「提案」に書いた。

## 対象の試験
- `browser::post_login_tests::daemon_post_login_waits_through_slow_shibboleth_hops`（`crates/task-worker/src/browser_post_login_tests.rs`）
- `browser::launcher_run::tests::launcher_login::launcher_credential_post_login_consent_page_is_held_with_its_reason`（`crates/task-worker/src/browser_launcher_run_tests.rs`）

## 1. R5 の差に login のコードが無いこと（受け入れ条件 0）
提出時の HEAD で実行した。

```
git diff --name-only $(git merge-base HEAD main) HEAD -- 'crates/task-worker/src/browser*' crates/celeris-credentiald/ | wc -l
0
git diff --stat $(git merge-base HEAD main) HEAD -- 'crates/task-worker/src/browser*' crates/celeris-credentiald/   # 出力なし、exit 0
```

`git merge-base HEAD main` との差の path は、`agent-docs/progress/2026-11-10-health-v2-r5/`（108）、`crates/task-api/tests`（93）、`crates/celeris/tests`（33）、`crates/task-worker/tests`（8）、`crates/celerisctl/tests`（8）、`crates/celeris/src`（2）、`scripts/dev/test-affected.sh`・`crates/task-dispatch/src`・ADR `2026-11-10-test-target-bundles.md`（各 1）だけ。
task-worker で変えたのは `crates/task-worker/tests` の結合試験の束ねだけで、2 試験がある lib の unit test（`src/`）は変えていない。

## 2. 3 回の統合の結果
log の dir は `/local/celeris/data/workspaces/01M6TYVJT53598CPKMGSQGA7XM/integration-checks/integrate-final/`。check 7 は test-affected。時刻は同じ回の log の更新時刻（UTC）から読んだおおよその範囲。

| 回 | log | 統合した HEAD（件名） | check 7 の結果 | 2 試験 |
|---|---|---|---|---|
| 1 | `1794413619190-7.log`（16:18〜16:48） | integrate wu/pick-g4 (phase final) | nextest の途中で celeris の 1200 秒の時間切れ（`[celeris] timed out after 1200s; killed the process group`）。Summary の行は無い | FAIL の行は無い。時間切れの前にこの 2 試験まで進んだかは未確認（nextest は PASS の行を出さない設定） |
| 2 | `1794416889266-7.log`（17:08〜17:32） | integrate wu/pick-g4 (phase final)（1 回目と同じ commit） | `Summary [ 491.307s] 5952 tests run: 5952 passed (18 slow), 22 skipped` | 合格 |
| 3 | `1794419526624-7.log`（17:55〜18:26、load average 約 59） | integrate wu/v8-guard (phase final) | `Summary [ 732.575s] 5952 tests run: 5950 passed (28 slow), 2 failed, 22 skipped` | 2 件とも FAIL |

2 回目と 3 回目の HEAD の差は `agent-docs/progress/2026-11-10-health-v2-r5/v8-guard.md` と `v8-target-check.sh` の 2 file だけで、Rust のコードの差は無い。
3 回目の HEAD とこの WU の開始時の HEAD の tree の差は 0。
1 回目と 2 回目の load average は log に無く、未確認。

3 回目の失敗の log の行（`1794419526624-7.log` の 911〜947 行）:

```
FAIL [   2.054s] (4884/5952) task-worker browser::post_login_tests::daemon_post_login_waits_through_slow_shibboleth_hops
thread 'browser::post_login_tests::daemon_post_login_waits_through_slow_shibboleth_hops' (3326844) panicked at crates/task-worker/src/browser_post_login_tests.rs:749:52:
login: "target_changed"

FAIL [   2.342s] (4889/5952) task-worker browser::launcher_run::tests::launcher_login::launcher_credential_post_login_consent_page_is_held_with_its_reason
thread 'browser::launcher_run::tests::launcher_login::launcher_credential_post_login_consent_page_is_held_with_its_reason' (3326256) panicked at crates/task-worker/src/browser_launcher_run_tests.rs:3338:13:
login not recorded: Recorded(Err("launcher_authenticate_rejected"))
```

## 3. 2 試験の単独の実行（受け入れ条件 1）
提出時の HEAD で、この WU の `$CARGO_TARGET_DIR` を使って実行した。
先に `sh agent-docs/progress/2026-11-10-health-v2-r5/v8-target-check.sh` を走らせ、exit 0（v8 の build output 1 件、古い link-search なし）。rusty_v8 の link error は出ず、`cargo clean -p v8` はしていない。

コマンド（3 回とも同じ。3 回目だけ `--status-level pass` を足して試験ごとの秒を出した）:

```
cargo nextest run -p task-worker -E 'test(=browser::post_login_tests::daemon_post_login_waits_through_slow_shibboleth_hops) | test(=browser::launcher_run::tests::launcher_login::launcher_credential_post_login_consent_page_is_held_with_its_reason)'
```

| 回 | 開始（UTC） | /proc/loadavg（前） | /proc/loadavg（後） | exit | 壁時計 | nextest の Summary |
|---|---|---|---|---|---|---|
| 1 | 2026-11-11T20:56:23Z | 12.88 20.56 28.34 | 13.41 18.37 26.63 | 0 | 113 秒（build 1 分 32 秒を含む） | `[ 19.141s] 2 tests run: 2 passed, 1290 skipped` |
| 2 | 2026-11-11T20:58:32Z | 13.38 18.12 26.42 | 13.85 17.94 26.19 | 0 | 21 秒 | `[ 19.045s] 2 tests run: 2 passed, 1290 skipped` |
| 3 | 2026-11-11T20:59:08Z | 13.69 17.70 25.97 | 13.36 17.36 25.69 | 0 | 21 秒 | `[ 19.101s] 2 tests run: 2 passed, 1290 skipped` |

3 回目の試験ごとの秒: `launcher_credential_post_login_consent_page_is_held_with_its_reason` 4.260 秒、`daemon_post_login_waits_through_slow_shibboleth_hops` 19.094 秒。

単独では落ちなかったので、`git merge-base HEAD main` の写しでの実行はしていない（objective の手順どおり、落ちた時だけの手順）。

## 4. 原因の見立て
見立て: login の待ちが壁時計の固定の窓に依存し、load average 約 59 の統合では窓を超えた。R5 の変更は関係しない。
根拠は次の 3 点。どの窓を超えたかは未確認。

- 落ちた回は試験が早く終わった。合格する単独の実行では 4.3 秒・19.1 秒かかるのに、落ちた回は 2.3 秒・2.1 秒で失敗した。login の途中の判定が、決まった出来事を待たずに途中の状態を見て失敗を返したと読める。
- `daemon_post_login_waits_through_slow_shibboleth_hops` は 749 行の `w.login(...).await.expect("login")` で、post-login の待ちより前の注入で落ちた。`target_changed` は `crates/task-worker/src/browser_cdp_sink.rs` の注入の照合が返す（要求を作った時の frame の `loaderId` と注入の時の `loaderId` が違う）。この fixture は form の前に localStorage の interstitial の auto-POST をはさむ。負荷で、その遷移が終わる前に要求の frame を決めたと見る（未確認）。
- `launcher_credential_post_login_consent_page_is_held_with_its_reason` は `crates/task-worker/src/browser_launcher_run.rs` の `runtime.authenticate` が `AuthenticationStatus::Rejected` を返し、`launcher_authenticate_rejected` になった。credentiald の injection の stream の読み取りには固定の上限（`crates/celeris-credentiald/src/injection_ipc.rs` の `IO_TIMEOUT` 5 秒、`ipc.rs` の read timeout 5 秒）がある。失敗は 2.3 秒で、5 秒の窓そのものを超えたかは未確認。

同じ owner の直しの commit『phase 1: wait for the passed injection stream's first request on the auth section (launcher login regression)』は HEAD に入っている。それでも落ちたので、この直しの外にも固定の窓が残っていると見る。

## 5. owner への提案（R5 の write set の外。R5 では直さない）
- owner: task-worker の browser login（`crates/task-worker/src/browser_cdp_post_login.rs`、`browser_cdp_sink.rs`、`browser_launcher_run.rs`）と celeris-credentiald の injection の待ち（`crates/celeris-credentiald/src/injection_ipc.rs`）。
- 提案: ADR-0125 の出来事待ちに直す。login の注入は、壁時計の窓ではなく「frame の遷移の完了（`Page.frameStoppedLoading` など、loaderId が安定した出来事）」と「password 要素の出現」を待ってから要求の frame を決める。credentiald は、auth section の要求の到着という出来事を待ち、時間の上限は長い保険としてだけ置く。
- 再現: CPU を焼く負荷は使わない。試験専用の遅延フック（interstitial の遷移の完了の前に注入の要求を作らせる遅延、credentiald の stream の最初の要求の前の遅延）か、対象 process への SIGSTOP/SIGCONT で決定的に再現する（`agent-docs/guides/testing.md`）。
- 当面の扱い: 段 final の統合は、load average が低い時（1 分の値が 20 以下）に走らせる（人の決定 login-rerun）。nextest の retries を足すなど、検査を弱める変更はしない。

## 検査
提出時の HEAD で実行した。
- 受け入れ条件 0: 上の `git diff --name-only … | wc -l` が 0。
- 受け入れ条件 1・2: この file の 2〜5 節。
- 受け入れ条件 3: `sh "$CELERIS_WU_SCOPE_PATHS"` と `git diff --name-only "$CELERIS_WU_BASE"` が `agent-docs/progress/2026-11-10-health-v2-r5/login-flake.md` だけを出す。
- `sh scripts/dev/progress-index.sh --check`: exit 0。

## 変更した file
- `agent-docs/progress/2026-11-10-health-v2-r5/login-flake.md`（この file）だけ。`crates/`・`scripts/`・`.config/` は変えていない。

## 未解決事項
- 2 試験が負荷で落ちる窓の特定（上の見立ては未確認）。owner の task で直す。
- 1 回目の統合で、時間切れの前に 2 試験まで進んだかは未確認。
