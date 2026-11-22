---
title: 戦略層 v1 子 3 dagbench-flake — dagbench_process の通知主導の再開の試験を scenario ごとに有界にする
tasks: [01M7XQ9QE8STND208NXG90F743]
status: done
updated: 2026-11-22
---
# dagbench-flake（WorkUnit。子 3 の段 cite の統合で落ちた試験）

完了日: 2026-11-22

cite 段の統合の check（`bash scripts/dev/test-affected.sh`、8637 試験を並走）で、`crates/campaign/tests/dagbench_process.rs` の
`recovery_notice_continuation` だけが nextest の 600 s で TIMEOUT になった（同じ統合の別の回は全 pass、単独では 18.9 s）。
基準は `$CELERIS_WU_BASE`。

## 原因の切り分け（結論: 遅さで説明でき、来ない出来事を待つ競合は再現しなかった。止まった scenario は特定できなかった）

- 旧い試験は 6 scenario（arm baseline / celeris × 通知なしの probe・受理後の停止・待機後の停止）を 1 つの `#[test]` で直列に回していた。
  失敗の log（`integration-checks/integrate-cite/` の -1.log）には `test recovery_notice_continuation has been running for over 60 seconds`
  しか無く、どの scenario のどの段で止まったかは残っていない。待ちの保険 `INSURANCE` は 600 s で nextest の打ち切りと同じだった。
  control の child の側にも、nextest より短い上限は無い（celeris の `CelerisSide::drive` の `cap_wall` は fixture の `max_wall_secs` = 600 s、
  baseline の control は `logical_cap` を設定せず `u64::MAX`）。このため、どの段が遅くても止まっても、名前が出ずに 600 s で打ち切られた。
- 決定的な方法（`agent-docs/guides/testing.md` 方法 3、CPU は使わない）: 試験の control・External の child だけに SIGSTOP/SIGCONT を
  繰り返し掛け（stutter の台本は `$TMPDIR` に置き、リポジトリには残していない）、6 scenario を並べて回した。新しい保険（下記）で、
  止まった段があれば段の名前で落ちる。

  | 掛けたもの | 回した数 | 結果 | celeris after_accept / after_wait の所要 |
  |---|---|---|---|
  | なし（単独・直列） | 6 | 全 pass | 6.27 s / 6.37 s |
  | control に STOP 20 ms / CONT 5 ms | 6 並走 | 全 pass | 15.71 s / 16.29 s |
  | control に STOP 300 ms / CONT 30 ms | 12 並走（各 2 回） | 全 pass | 49.87 s / 49.86〜50.19 s |
  | 全 role（control・External）に STOP 50 ms / CONT 5 ms | 12 並走（各 2 回） | 全 pass | 31.06〜31.35 s / 31.35〜31.46 s |

  所要時間は動ける時間の割合にほぼ比例して延び（STOP 300 / CONT 30 で約 8 倍）、どの段も来ない出来事を待たなかった。
  stutter の下でも、停止点の順序（celeris の after_accept で `exp-uring` の受理 → `exp-posix` の受理で停止、`exp-uring` の待機開始は worker ledger にだけ載る）
  は単独の run と同じだった。
- 確かめたこと: control の process 全体を止めて遅くする限り、6 scenario はどれも止まらない（遅いだけ）。
- 確かめていないこと（未確認）: 統合の run で止まったのがどの scenario のどの段か。process 内の thread の順序の違い（例: celeris の
  after_accept で、`exp-posix` の受理が `exp-uring` の `due_secs` より先に来る順序）は SIGSTOP では作れず、試していない。
  統合の run で 600 s（単独の約 30 倍）に達したのは、同じ run の他の dagbench 試験の遅れ（70〜161 s）より大きく、遅さだけで説明できるかは未確認。
- 競合を確かめられなかったので、owner（`crates/campaign/src` の driver）の挙動は直していない。次に止まったときは、下の保険が
  止まった scenario（試験名）・段・child の最後の行・stderr の末尾を出す。

## 変更

- `crates/campaign/tests/dagbench_process.rs`
  - `recovery_notice_continuation` を消し、scenario ごとの top level の試験 6 個に分けた:
    `recovery_notice_continuation_baseline_probe`・`_baseline_after_accept`・`_baseline_after_wait`・
    `_celeris_probe`・`_celeris_after_accept`・`_celeris_after_wait`。scenario の中身（`no_notice_probe`・`notice_run`）は変えていない。
  - 保険を 1 scenario 全体の期限 `Insurance`（`INSURANCE` = 240 s）にした。各段の待ちは残りの時間だけ待つ。超えたら
    `stage `<段>` stalled (...)` と child の role・incarnation・pid、stdout の最後の行、stderr の末尾 10 行を出して落ちる。
    段の名前は `external_ready`・`control_1_stop_point`・`control_2_resumed`・`control_2_resend`・`control_2_run_to_completion`。
    2 scenario を直列に回す `ipc_external_survives_control_exit` と `recovery_lost_response` も、合計 480 s で nextest の 600 s に届かない。
    probe の上限 `PROBE_BOUND`（60 s）は保険の残りと小さい方を使う。旧い `wait_line` は消した。
  - 試験 `insurance_names_the_stalled_stage_and_the_last_child_line` を足した。守る不変条件: 保険を超えた段は段の名前と child の最後の行を
    出して落ちる。既存の試験は全て保険の内に終わるので、他の試験ではこの失敗の報告を確かめられない。ready を出して止まらない
    child（`stubborn`）を 0.5 s・0.2 s の保険で待ち、panic の文に段の名前・`waiting for NEVER_PRINTED`・最後の行 `ready`・`control-1.stderr` が
    あることを確かめる。
- `crates/campaign/src` は変えていない。stderr の path は試験の側で `EventLog::dir()` と role・incarnation から導く
  （`Supervisor::spawn` の `<log dir>/<role>-<incarnation>.stderr`）。最初は `OwnedProc::stderr_path()` を足したが、
  repo health の `size.responsibility-growth`（`crates/campaign/src/dagbench` は既に 13187 行で閾値 8000 を超えている）が
  増分で落ちたので、src の変更をやめた。
- `.config/nextest.toml` は変えていない（stutter の結果は遅さに比例するだけで、threads-required・test-group で縛る根拠が無い）。
- 足していないもの: retries、slow-timeout・terminate-after の延長、`#[ignore]`、env による skip。試験は消していない（1 個を 6 個に分けた）。

## 所要時間（単独・直列、`$TMPDIR` の補助台本で各試験を 1 本ずつ起動）

- 変更前: `recovery_notice_continuation` 18.47 s（内訳は raw の `process-events.jsonl` の最後の `t_ms`: baseline probe 0.56 s・accept 1.19 s・
  wait 1.19 s、celeris probe 3.14 s・accept 6.17 s・wait 6.16 s）。
- 変更後: `_baseline_probe` 0.58 s、`_baseline_after_accept` 1.21 s、`_baseline_after_wait` 1.21 s、`_celeris_probe` 3.00 s、
  `_celeris_after_accept` 6.27 s、`_celeris_after_wait` 6.37 s。
- `cargo test -p celeris-campaign --test dagbench_process`: 変更前 9 試験 → 変更後 15 試験（-1 +6 +1）、15 passed、34.23 s。

## LoC（`git diff --numstat`、基準は `$CELERIS_WU_BASE`）

- production: 0（変更なし）。
- test（`crates/campaign/tests/dagbench_process.rs`）: +171 −37。純増の理由: 6 個の試験関数、`Insurance`（`line`・`exit`・`stalled`）、失敗の報告の試験。

## commit（件名）

- `strategy-v1 3 dagbench-flake: recovery_notice_continuation を scenario ごとの 6 試験に分け、保険を 240 s の scenario 期限にして止まった段を出す`
- `strategy-v1 3 dagbench-flake: stderr の path を試験の側で導き、campaign の src を変えない（size の増分を出さない）`
- `strategy-v1 3 dagbench-flake: WU の進捗に検査の結果`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `bash scripts/dev/test-affected.sh` → exit 0。repo-health verdict=pass（fail_grade=0）。nextest 3981 tests run: 3981 passed（9 slow）、
  11 skipped、407 s。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。`dagbench_process` の試験は
  SLOW（60 s 超）に 1 つも出なかった。最初の回は `size.responsibility-growth` で exit 1（src の変更をやめて解いた。上記）。
- `cargo test -p celeris-campaign --test dagbench_process` → 15 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → `agent-docs/progress/2026-11-21-strategy-v1-3/dagbench-flake.md` と
  `crates/campaign/tests/dagbench_process.rs` だけ。
- 足していないものの確認: `git diff $CELERIS_WU_BASE -- .config/` は空、差分に `#[ignore]`・`retries`・`slow-timeout`・`terminate-after`・
  `std::env::var` の新しい呼び出しは無い。

## 未解決

- 統合の run で止まった scenario と段は未確認（上記）。次に止まれば試験名と段の名前が出る。
- process 内の thread の順序による競合は試していない（試験専用の遅延フックを `Tripwire` に入れても、外部 backend の Mutex の中なので
  別 thread の呼び出しを割り込ませられない。driver 側に hook を足す必要がある）。

## 提案

- 再び止まったら、出た段の名前と child の最後の行から、driver（`crates/campaign/src/dagbench`）の該当の待ちに試験専用の遅延フックを置いて
  決定的に再現する task を起こす。
