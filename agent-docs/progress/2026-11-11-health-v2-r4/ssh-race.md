---
title: "健全性v2 R4 ssh-race: ssh の timeout 試験の helper が回収中の子（状態 X）を生きていると読む競合を直す"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 ssh-race

完了日: 2026-11-14

## 概要

- task の最終 review の `bash scripts/dev/test-affected.sh` が `crates/task-worker` の
  `ssh::tests::remote_command_times_out_and_kills_the_ssh_process_group` で落ちた（`assert_shell_and_child_gone` の
  「the child … was left behind」。試験は 2.01 秒で終わった）。R4 は `crates/task-worker` の本体を変えていない。
- 直したのは試験の helper だけ（`crates/task-worker/src/ssh/tests.rs`）。本体の `ssh.rs`・`process_group.rs` は変えていない。
  - `proc_stat_running` を出した: `/proc/<pid>/stat` の 1 行の最後の `)` の後の状態の文字が `Z`・`X`・`x` か、状態が無ければ
    生きていないとみなす。`process_running` はこれを使い、読めない pid は生きていないとみなす（今までと同じ）。
  - `assert_shell_and_child_gone` は 1 回の観測で shell と子の生死の組を読み、loop を抜けた観測そのもので assert する（読み直さない）。
  - 単体試験 `proc_stat_dead_states_are_not_running` を足した。
- 期限 10 秒・20ms 間隔・`KillLeftoverSleep`・3 本の timeout 試験の中身は変えていない。

## 試験を変えた理由

- 守る不変条件は「timeout の後に shell（偽 ssh）と子が 10 秒以内に消える」で、変わらない。3 本の timeout 試験
  （`command_probe_times_out_and_kills_a_hanging_ssh`・`remote_command_times_out_and_kills_the_ssh_process_group`・
  `run_command_times_out_and_kills_the_ssh_process_group`）は同じ helper で同じ条件を確かめ続ける。
- 旧い helper の `process_running` は状態が `Z` 以外なら生きているとみなした。親が zombie を回収している間、
  process は状態 `X`（dead）を一瞬見せてから消える（Z → X → 消える）。旧い helper はこの回収中の子を生きていると報告する。
  同じ crate の `browser_runtime::same_process_alive` は `Z` と `X` を終わりとして扱っている。
- 旧い `assert_shell_and_child_gone` は loop の条件で `process_running` を読み、loop を抜けた後の assert で読み直した。
  試験は 2.01 秒（timeout 2 秒の直後）で落ちたので、loop は 1 回目の観測で shell と子の両方を「終わった」と見て抜けている。
  子が本当に残っていたなら loop は 10 秒待つので、子は 1 回目の観測の時点で生きていなかった。見込みの順序は、loop の観測で子が
  `Z`（終わり）、直後の assert の読み直しで `X`（旧い helper では生きている）。観測を 1 回にすれば、この食い違いも、
  消えた pid が別の process に再利用される食い違いも起きない。
- 状態 `X` を実機で観測して再現したわけではない（未確認）。kernel の回収の瞬間は試験から止められないので、状態の文字ごとの
  判定は stat の行の例で決定的に確かめる（`proc_stat_dead_states_are_not_running`）。

## 実装前 brief

1. 根本原因: 試験の helper `process_running` が回収中の process（`X`）を生きているとみなし、`assert_shell_and_child_gone` が
   loop の観測と assert の観測を別々に読む。症状は最終 review の `ssh::tests::remote_command_times_out_and_kills_the_ssh_process_group`
   の失敗（2.01 秒）。
2. 責務 owner: `crates/task-worker/src/ssh/tests.rs` の helper（`process_running`・`assert_shell_and_child_gone`）。本体の不具合ではない。
3. 既存の再利用先: `browser_runtime::same_process_alive` の `Z`・`X` の扱い。starttime の照合を含み、引数も違う（pid の starttime が要る）ので
   そのまま呼ばず、状態の文字の規則だけをそろえた。
4. 選択肢: (a) 状態の判定を小さい関数に出して `Z`・`X`・`x` を終わりとし、loop の観測で assert する（採用。両方の食い違いを消す）。
   (b) 期限や間隔を延ばす（読み直しの食い違いは消えないので採らない）。(c) 本体を変える（本体の不具合ではないので採らない）。
5. 新しい状態・分岐・fallback・port: 無し（試験の helper の関数 1 つ）。
6. 撤去対象: 旧い `process_running` の `state != "Z"` の判定と、assert の前の読み直し。
7. 不変条件: timeout の後に shell と子が 10 秒以内に消える（3 本の timeout 試験）。状態 `S`・`R`・`D` は生きている、
   `Z`・`X`・`x` と読めない pid は終わり（`proc_stat_dead_states_are_not_running`）。

## 足した試験

- `proc_stat_dead_states_are_not_running`: 状態 `S`・`R`・`D` の行は生きている、`Z`・`X`・`x` の行は終わり、comm に `)` と空白が
  ある行は最後の `)` の後の文字で決まる、`)` の後が無い行と空の行は終わり、pid の上限を超える pid（`u32::MAX`）は読めないので終わり。
- 既存の試験で検出できない理由: 3 本の timeout 試験は本物の process の回収の瞬間に当たったときだけこの読み違いで落ちる（決定的でない）。
- 判定を旧い `state != "Z"` に一時的に戻すと、この試験は `state X is a finished process` で落ちる（exit 100、0 passed・1 failed）。
  戻した後は合格する。

## 変更（件名）

- `ssh-race: timeout の試験の helper を回収中の子（状態 X）を終わりと読ませ、loop の観測で assert する`
- `ssh-race: 検査の結果を進捗に書く`

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、範囲は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-worker -E 'test(ssh::tests::)'`: exit 0、26 passed（`proc_stat_dead_states_are_not_running` と 3 本の timeout 試験を含む）。
- `cargo nextest run -p task-worker -E 'test(times_out_and_kills) | test(proc_stat_dead_states_are_not_running)'` を 5 回: 5 回とも 5 passed（各 2.03 秒）。
- 判定を旧い `state != "Z"` に一時的に戻した `proc_stat_dead_states_are_not_running`: exit 100（`state X is a finished process`）。戻した後は合格。
- `bash scripts/dev/test-affected.sh`: exit 0。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker。
  6138 passed、22 skipped。repo_health_gate は all checks passed。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh`: exit 0（verdict=pass、fail_grade=0。new=1 は `host_maintenance.rs` の migration.remnant で、この変更とは無関係）。
- `sh scripts/dev/progress-index.sh --check`・`bash scripts/dev/check-doc-links.sh`: exit 0。
- `git diff --name-only $(git merge-base HEAD main) -- crates/task-worker`: `crates/task-worker/src/ssh/tests.rs` の 1 path だけ。
  `git diff --quiet $(git merge-base HEAD main) -- crates/task-worker/src/ssh.rs crates/task-worker/src/process_group.rs`: exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: この file・R4 の進捗・`crates/task-worker/src/ssh/tests.rs` の 3 path。

## 未解決

- 状態 `X` の観測そのものは再現していない（未確認）。この変更の後は、helper が `X` を生きていると読むことも、loop と assert の観測が
  食い違うことも無い。

## 提案

- 無し。
