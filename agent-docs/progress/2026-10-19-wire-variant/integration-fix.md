---
task: 01M5A3JD7TWJB3BK01MBTDHR2W
tasks: [01M5A3JD7TWJB3BK01MBTDHR2W]
title: 契約段の統合失敗を診断し最小修復（integration-fix）
work_unit: diagnose
status: blocked
updated: 2026-10-19
---
# 契約段の統合失敗の診断（integration-fix / diagnose）

親 task `01M59P361KPPHSSHTD64EHAA1J`（選択機構だけの variant 設定と伝播を配線）の WU `integrate-contract`（`01M59P79RDVGP1GB8Z9NTEE279`）が 2026-10-19T12:51:36Z に `integration_failed` になった件の診断。実装は変更していない。variant-contract は再実行していない。

## ログ取得

| 経路 | 結果 |
|---|---|
| DB（`CELERIS_RUN_DB=/local/celeris/data/db/celeris.sqlite3`、run 環境で明示設定済み） | `sqlite3 "file:$CELERIS_RUN_DB?mode=ro"` の read-only 接続で読めた。書き込みはしていない。既定 DB の新規作成もしていない |
| `check_log_root` + `integration-checks/<wu_key>`（`crates/task-dispatch/src/dispatcher/phase_integration.rs`） | `/local/celeris/data/workspaces/01M59P361KPPHSSHTD64EHAA1J/integration-checks/integrate-contract/` に 3 件あり、読めた |
| user journal（`journalctl --user`、celeris daemon） | 読めた。この WU の kill・timeout の記録は無い |
| kernel ログ（`dmesg`）・system journal | 権限が無く読めない（`Operation not permitted`／該当なし） |

DB の event（親 task の seq 45〜52）。HEAD は親 branch の reflog `celeris/01M59P361KPP…@{0}: merge celeris/01M59P79S6NTGRN3X7SQDV9C0G` から読んだ `a0186c4a`（integrate wu/variant-contract）。cwd は `/local/celeris/data/workspaces/01M59P361KPPHSSHTD64EHAA1J/repos/agent-platform`（ログ中の Compiling の path と一致）。

| index | cmd | exit | pass | timed_out | duration |
|---|---|---|---|---|---|
| 0 | `bash scripts/dev/test-parallel.sh` | **143** | false | false | 181.9 s |
| 1 | `cargo clippy --workspace -- -D warnings` | 0 | true | false | 102.1 s |
| 2 | `cargo fmt --all -- --check` | 0 | true | false | 8.6 s |

元のログ `1792414001486-0.log` の要点（写しは `/local/celeris/data/workspaces/01M5A3JD7TWJB3BK01MBTDHR2W/wu/diagnose/artifacts/logs/orig-integrate-contract-1792414001486-{0,1,2}.log`）:

```
   Compiling task-dispatch v0.1.0 (…/crates/task-dispatch)
   Compiling llm-proxy v0.1.0 (…/crates/llm-proxy)
error: Broken pipe (os error 32)
warning: build failed, waiting for other jobs to finish...
Terminated
test-parallel: cargo test --doc --workspace
…
CELERIS_TEST_SUMMARY {… "binaries": null, "passed": 3, "failed": 0, "nextest_exit": 143, "doctest_exit": 0, "nextest_secs": 157.2, "summary_parsed": false …}
test-parallel: cargo nextest run failed (exit 143)
```

- nextest は**ビルドの途中**で止まった。試験は 1 件も走っていない（`binaries: null`、FAIL 行なし）。compile error は出ていない。
- `scripts/dev/test-parallel.sh:90-91` は `cargo nextest run … 2>&1 | tee "$logdir/nextest.log"`（`set -o pipefail`）。`Terminated` は bash が出す「pipeline の要素が SIGTERM で終わった」という表示で、`Broken pipe` は cargo が出力先の pipe（tee）を失ったときのエラー。143 = 128 + 15（SIGTERM）。
- test-parallel の bash 自身は止まらず、続く doc-test は通った（exit 0）。つまり SIGTERM はプロセスグループ全体ではなく、pipeline の一部（tee か cargo）だけに届いた。
- 統合の timeout では `crates/task-worker/src/workspace.rs` がプロセスグループに **SIGKILL** を送り、`timed_out: true` を記録する。今回は `timed_out: false` で信号も SIGTERM なので、Celeris の timeout 経路ではない。

## 再現

- 統合 HEAD `a0186c4a` とこの worktree の HEAD `c8a9dfb7` は tree が同じ（`git diff --quiet a0186c4a c8a9dfb7` → 差分なし。tree `97879580…`）。c8a9dfb7 で、変更を加えずに再現を試した。
- 実行: cwd `/local/celeris/data/workspaces/01M5A3JD7TWJB3BK01MBTDHR2W/wu/diagnose/repos/agent-platform`、`bash scripts/dev/test-parallel.sh`。開始 2026-10-19T13:20:25Z、終了 13:32:10Z。
- 結果: **exit 0。再現しない**。`CELERIS_TEST_SUMMARY {"binaries": 255, "nextest_binaries": 242, "passed": 5849, "failed": 0, "ignored": 15, "nextest_exit": 0, "doctest_exit": 0, "summary_parsed": true}`、`test-parallel: ok`。ログは `/local/celeris/data/workspaces/01M5A3JD7TWJB3BK01MBTDHR2W/wu/diagnose/artifacts/logs/repro-test-parallel.log`。
- clippy と fmt は元の統合で通っている（上表）ので、この run では回していない。

### 原因: 確定したこと／仮説

- **確定**: 失敗したのは `test-parallel.sh` の nextest ビルド段階で、原因は外から届いた SIGTERM（exit 143）。試験の失敗でも compile error でもない。同じ tree で無変更のまま回すと全件通る。clippy と fmt も同じ tree で通っている。したがって、campaign（`crates/campaign`）を含め、契約段のコードの欠陥は示されていない。
- **未確定（仮説）**: SIGTERM の送り元。user journal の 12:48:30〜12:50:00 には、この WU への kill も OOM も記録されていない。kernel ログは権限が無く見られない。当時は scratch gc が high_watermark で他 task の target を立て続けに退避しており、dispatch の tick も遅かった（最大 18 s）。host の負荷が高かったことは観測できたが、それと SIGTERM との因果は確かめていない。earlyoom・systemd-oomd は動いていない（`ps`・`systemctl` で確認）。

## 既存実装

最新 main: `git ls-remote origin refs/heads/main` → `160f1736333f4b498a0a5f3bc5cd905067956ede`（観測 2026-10-19T13:32:34Z。"phase cos-registered-project-deploy"）。HEAD より 1 commit 先にあり、HEAD は main を含まない。差分は 13 files で、cos-chat、`crates/task-worker/src/workspace/tests.rs` など。この失敗とは無関係で、main を取り込む必要は無い。この葉では merge していない。

既存 3 task。状態は DB（read-only）から、実装・試験は git から読んだ。

| task | DB 状態 | git 上の実装・試験 |
|---|---|---|
| `01M58VFRBA6HG0SSF3CG5BKM91`（本測定移行 2. 設定確定） | ready | `celeris/01M58VFRBA6HG0SSF3CG5BKM91`（head `cd7f85ca` integrate wu/symlink-components）。HEAD に含まれない（HEAD に無い commit 22 件、26 files +744/-50）。中身は `crates/campaign/src/{snapshot,isolation}.rs`、`crates/campaign/tests/{inner_symlink,measure_daemon_example,isolation,cap_probe,final_resolved,a_program_pin_and_watch}.rs`、`docs/ops/measure-run-final.md`、`agent-docs/progress/2026-10-19-mlsys-final-config/*` など。HEAD 側も変えている file は `crates/campaign/tests/a_program_pin_and_watch.rs` と progress 4 件。後で合わせるときは衝突し得るが、今回の失敗の原因ではない（今回の統合 tree はこの branch を含まずに全件通った） |
| `01M58VG5C4XGK4E70M6WJ6MEX0`（本測定移行 3. 無 LLM 統合・主比較先行） | cancelled | `celeris/<id>` branch も、この id を含む commit も見つからない。git 上の実装・試験は無い |
| `01M58VGDNQ8GWGJPB8SJ73XSPF`（本測定移行 4. 主比較 A0/B0 最小実 pilot・最終報告） | cancelled | 同上。git 上の実装・試験は無い |

固定 selection（`crates/campaign/fixtures/ablation/selection.json`）と完了済みの契約（contract.md）には触れていない。

## 修復範囲

- **コードの修復範囲は無い**。根拠のある欠陥は、campaign にも他の subtree にも見つかっていない。後続の `contract-repair`（診断済み campaign 契約の局所修復）は、直す対象が無いまま推測で修復することになるので、実行すべきでない。
- 必要なのは、変更を加えずに `integrate-contract` の統合検査（`bash scripts/dev/test-parallel.sh` / clippy / fmt）をもう一度回すことだけ。検査は弱めない。
- そこで plan_issue で replan を求めた。提案は 2 点: (1) `contract-repair` と `integrate-repair` を外すか、「変更なしの統合再検査」に差し替える。(2) 再検査がまた exit 143 で落ちるなら、SIGTERM の送り元（host の kernel ログ・system journal。権限が要るので人の check）を調べる専用葉を置く。main の取り込みは不要。
- 未解決: SIGTERM の送り元（上記の仮説）。

## 証拠コマンド

- `sqlite3 "file:$CELERIS_RUN_DB?mode=ro" "select seq, ts, json from events where task_id='01M59P361KPPHSSHTD64EHAA1J' and json like '%integration_check%'"` → seq 46〜51
- `git diff --quiet a0186c4a c8a9dfb7` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0（5849 passed / 0 failed / 15 ignored）
- `git ls-remote origin refs/heads/main` → `160f1736…`
