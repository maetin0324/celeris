---
title: "verify の daemon が本番の評価試行を消す／live 昇格で走行中の評価試行が error になる"
tasks: [01M4WSXTXWQEJVJFT1JF1VQ19K]
status: done
updated: 2026-10-14
completed: 2026-10-14
---
# verify の隔離と live 昇格での評価試行

設計判断は [ADR 2026-10-14-eval-verify-isolation](../adr/2026-10-14-eval-verify-isolation.md)（D1 verify は評価試行に
触れない、D2 削除は root の配下だけ、D3 live 昇格は旧 instance が終端まで走らせ新 instance が採点、D4 本番への移行）。

## 葉

| 葉 | 中身 | 状態 |
|---|---|---|
| adr-audit | ADR と、verify の daemon が本番資源に触れる経路の表（この文書） | 済 |
| verify-iso | ADR D1・D2（評価試行は instance id を持つ dispatcher だけ、試行ファイルの削除は root 配下だけ）と試験 | 済（2026-10-14、`a6a645d7` root-guard・`2a00af24` verify-gate、全体検査は下の証拠） |
| handoff-eval | ADR D3（`eval_trials.instance_id` migration 0078、`in_flight` に評価 run、持ち主の生死を見る回復、`runs` 行の照合、drain 打ち切りで `abort_eval_runs`）と試験 | 済（子 task 01M4WYKAQT、`383f6f41` owner-record・`95ef97d3` drain-run、記録は [2026-10-14-eval-handoff](2026-10-14-eval-handoff.md)） |
| closeout | 全体検査・この文書の完了・本番への移行・architecture-map | 済（2026-10-14、下の「証拠（closeout）」） |

## verify の daemon が本番の資源に触れる経路（2026-10-14 時点の main `de94e591`）

前提: `scripts/selfdeploy/verify.sh` は `celeris --config "$SD_CONFIG"`（**本番の config**）`--mode verify --db
staging.sqlite3 --listen 127.0.0.1:<staging port> --workspace-root <staging>/workspaces` で起動する。DB と
`workspace_root` と API の port だけが staging で、scratch pool・build cache・`[mcp]`・cron・CoS・cluster・
credentiald・KB の path は本番の値を読む。verify の dispatcher は `set_accepting_new_work(true)` で tick を回し、
`set_eligible_tasks`（smoke だけ）を持ち、`set_orphan_takeover` は呼ばない（`daemon_instances` の行が無い）。

| # | 資源 | 触れる経路 | 直す／直さない（理由） | 担当 |
|---|---|---|---|---|
| 1 | 本番の評価試行の作業ツリー（`/local/celeris/data/workspaces/eval/<trial>`）・scratch target（`targets/agent-eval-<trial>`）・lease | `Dispatcher::tick` → `tick_eval_trials` → `reconcile_eval_trials` → `fail_and_cleanup_eval_trial` → `remove_eval_trial_files`（DB の `worktree_path` を `git worktree remove --force` + `remove_dir_all`）・`remove_eval_target`（本番 config の pool） | 直す（今回の事故。ADR D1・D2） | verify-iso |
| 2 | 評価試行の起動（本物の LLM run、本番 pool の target、`<staging>/workspaces/eval`） | `tick_eval_trials` の claim → `start_eval_trial`（`[routing_eval] reserved_runs` は本番 config の値） | 直す（写しの DB の `queued` を本物の adapter で走らせうる。ADR D1） | verify-iso |
| 3 | 評価 `runs` 行・試行の状態（写しの DB） | 同上の回復が `eval_trials` を `error` にする | 直す（写しへの書き込みだが D1 で同じく止まる） | verify-iso |
| 4 | live 昇格での旧 instance の評価 run（作業ツリー・`runs` 行） | 新 active の `reconcile_eval_trials`（持ち主を見ない）、`in_flight` が評価 run を数えない drain、`reconcile_ownerless_runs` の `eval` 除外 | 直す（今回の事故の後半。ADR D3） | handoff-eval |
| 5 | 本番の MCP の port（`[mcp] listen`、本番 18200） | `daemon/run.rs` `build_mcp_state` → `start_mcp`（`services.rs` が `bind_reuseport`）。verify も本番の port に `SO_REUSEPORT` で相乗りし、worker の MCP 呼び出しの一部が staging（写しの DB・staging token）に振られる | 直す（ADR-0139 の llm-proxy と同じ型。この task の範囲外 → 後続 task） | 後続 task（followups） |
| 6 | 本番の scratch pool の target・lease | `tick` → `scratch_gc`（`run_gc` の lookup は写しの DB。snapshot より後に始まった WU の owner は DB に無く P3＝削除に分類されうる） | 直す（本番の走行中の WU の target を消しうる。後続 task） | 後続 task |
| 7 | 本番の scratch seed | `tick` → `seed_housekeeping`（`seed_gc`・seed の更新） | 直す（同上。後続 task） | 後続 task |
| 8 | 本番の build cache（`build_cache_dir`） | scratch が無効なときの `cleanup_work_unit_build_caches`（写しの DB で終端の WU の cache を消す） | 直す（本番は scratch 有効で今は通らないが、同じ型。後続 task） | 後続 task |
| 9 | 本番の scratch target・build cache の掃除（cron の保守 task） | `tick` の cron 評価（`fire_due_with`。`accepting_new_work` だけで判定）→ `run_maintenance_tasks` → `target_sweep`（本番 config の root） | 直す（verify が本番の target を掃除しうる。写しの DB に cron の task も作る。後続 task） | 後続 task |
| 10 | CoS の会話・triage run（本物の LLM、CoS の作業場所） | `tick_cos_chat_launch` → `control_hook`・`claim_queued`・`triage_launch`（`build_dispatcher` が本番 config で `set_cos_chat_launch`） | 直す（写しの DB の queued な会話を本物の harness で走らせうる。`control_hook` が本番の process を止めうるかは後続 task で確かめる） | 後続 task |
| 11 | クラスタへの ssh（job の待ちの poll） | `tick` → `poll_cluster_job_waits`（`eligible` の検査が無い） | 直す（verify が本番の cluster に ssh を打つ。Phase 66c の liveness・tunnel と同じ閉じ方が要る。後続 task） | 後続 task |
| 12 | ディスク使用率（本番の `/`・`/local`・`/tmp`） | `tick_disk_watch`（statvfs を読み、写しの DB の `disk_watch_state` に書く） | 直さない（本番は読むだけ。書くのは写しの DB） | - |
| 13 | llm-proxy の port | `serves_llm_proxy(Verify) = false` | 直さない（ADR-0139 で閉じた） | - |
| 14 | Discord 通知・codex usage・model discovery・routing guardrail | `tick_loop` の `role == Active` の段だけ | 直さない（verify の role は `Verify`） | - |
| 15 | cluster liveness・tunnel | `refresh_cluster_liveness`・`refresh_cluster_tunnels` は `eligible.is_some()` で return | 直さない（Phase 66c で閉じた） | - |
| 16 | 作業場所の build 生成物・repo 直下 target | `prune_one_workspace`・`sweep_repo_targets` は `eligible.is_some()` で return。root も staging | 直さない（閉じている） | - |
| 17 | 中止 task の worktree | `cleanup_cancelled_worktrees` は手元の `task_workspaces`（verify では smoke だけ）を見る。root は staging | 直さない（本番の worktree は対象にならない） | - |
| 18 | lease の回収・孤児の回収・WU の照合・`runs` 行の照合 | `reclaim_expired_leases`（`is_eligible`）、`reconcile_parallel_tasks`・`reconcile_ownerless_runs`（`orphan_takeover` 必須）、`reconcile_terminal_records`（写しの DB だけ） | 直さない（閉じているか、写しの DB だけ） | - |
| 19 | 本番 DB | `install_worker_db_guard`（verify も含む）、`--db` は staging | 直さない（閉じている） | - |
| 20 | DB の checkpoint・backup・chat GC・KB の索引・ControlPath 検査・昇格 gate | `run.rs` の `!verify` 分岐 | 直さない（verify では起こさない） | - |
| 21 | 本番の API の port | `verify.sh` が本番の port（7710・7720・7721）を staging に使うのを拒む（`sd_die`） | 直さない（閉じている） | - |
| 22 | credentiald・browser launcher | browser の isolated runtime（`configure_isolated_runtime`）は非 verify の分岐だけ。API の `browser_credentiald_control_socket` は本番 config のまま組み立てられるが、verify の smoke task は browser を使わない | 直さない（verify で browser の run は起きない。API 経由の credential 操作は staging token の持ち主＝verify.sh だけ） | - |
| 23 | stranded sweep（取り残しのブランチ） | `stranded_sweep::spawn` は `role != Active` で何もしない | 直さない（閉じている） | - |

後続 task（#5〜#11）は `artifacts/followups.json` に起票した（「verify の daemon が本番の資源に触れる残りの経路を閉じる」）。
閉じ方の候補は ADR D1 と同じ「構造で閉じる」: verify の dispatcher は `orphan_takeover` も `eligible` も持つので、
本番の資源に書く裏方の段をこのどちらかで止める、`[mcp]` は llm-proxy と同じく verify で待ち受けない。

## 未解決事項

- #10 の `control_hook` が本番の CoS process を止めうるか（process の特定が thread id か pid か）は確かめていない。後続 task で確かめる。
- 表の #5〜#11（本番の MCP の port 18200 への `SO_REUSEPORT` 相乗り、scratch GC・seed・build cache・cron の target_sweep、
  CoS の会話 launch、cluster job の poll）は閉じていない。後続 task（followups「verify の daemon が本番の資源に触れる残りの経路を閉じる」）で扱う。
- 08:52Z・08:53Z に error になった評価試行 6 件の再実行（batch の再投入）は評価の運用セッションで行う。この task では投入しない。
- `celeris::routing_eval_run daemon_runs_two_stub_trials_with_hidden_checks_and_eval_records` が全体検査の 1 回目で
  1 度落ちた（`routing_eval_run.rs:200` の `!owner.join("target").exists()`）。単独の再実行と全体検査の 2 回目は通った。
  原因は時間依存: `score_eval_trial` は `eval_trial_finish`（`passed`）を書いてから `remove_eval_trial_files`
  （`git worktree remove`→target 削除）を走らせ、試験は終端を見た直後に target の不在を確かめる。順序は
  この task の前（`938e01cc`）から同じで、この task が持ち込んだ退行ではない。直し方は後続（下の提案）。

## 提案

- verify の起動で「本番の config を読む」こと自体が漏れの元。後続 task で、verify の dispatcher に入る本番 path
  （scratch pool・build cache・`[mcp]`・cron・CoS）を staging の値に差し替えるか、裏方の段を `orphan_takeover`
  の有無で一括して止めるかを決める。

- 試験 `daemon_runs_two_stub_trials_with_hidden_checks_and_eval_records` の target 不在の確かめは、終端を見た後の
  出来事待ち（target が消え lease に `released_at` が入るまで待つ、長い保険つき）にする（ADR-0125 の型）。
  あるいは本体で target・作業ツリーを消してから終端を書く（回復の段が `scoring` を拾い直すので後始末は冪等）。

## 証拠（verify-iso、closeout 2026-10-14、HEAD `c9074812`）

- `bash scripts/dev/test-parallel.sh` 1 回目: exit 100、passed 5098 / failed 1 / ignored 13（失敗は上の
  `routing_eval_run` の時間依存）。単独 `cargo test -p celeris --test routing_eval_run`: 1 passed。
- `bash scripts/dev/test-parallel.sh` 2 回目: exit 0、`CELERIS_TEST_SUMMARY` passed 5099 / failed 0 / ignored 13、
  nextest_exit 0・doctest_exit 0（userns: null）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo clippy -p task-dispatch -p celeris --all-targets -- -D warnings`: exit 0。
- `cargo test -p task-dispatch verify_daemon_leaves_foreign_eval_trial_files`: 1 passed。
- `cargo test -p task-dispatch eval_cleanup_refuses_paths_outside_roots`: 1 passed。
- `cargo test -p task-dispatch eval_trial`: 15 passed / 0 failed。

## 証拠（closeout、2026-10-14、HEAD `0937e4a6` = verify-iso・handoff-eval の統合後）

- `bash scripts/dev/test-parallel.sh`: exit 0。`CELERIS_TEST_SUMMARY` passed 5103 / failed 0 / ignored 13、
  nextest_exit 0・doctest_exit 0（binaries 181、userns: null）。前回まで時々落ちた `routing_eval_run` の試験もこの回は通った。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- 受け入れ条件の試験（`cargo test -p task-dispatch <name>`、いずれも `test result: ok. 1 passed`）:
  `verify_daemon_leaves_foreign_eval_trial_files`・`eval_cleanup_refuses_paths_outside_roots`（条件 0）、
  `live_handoff_keeps_running_eval_trial`・`eval_run_row_not_left_running_after_handoff`（条件 1）。
- `docs/architecture-map.md` の「評価 run の試行」行に migration 0078 と ADR eval-verify-isolation D1〜D3 を追記（文書のみ、コード不変）。

## 本番への移行

- verify-iso（D1・D2）は release の昇格だけで効く。config の変更・DB の書き換えは無い。昇格後の verify は評価試行に
  触れず、本番 active の評価試行の後始末は workspace root・scratch pool の配下（実 path）だけを消す。
- migration `0078`（`eval_trials.instance_id`）は昇格で当たる。config の変更・手での DB の書き換えは無い。
- 本番 DB に running のまま残る評価の `runs` 行（08:53Z の 3 行）は、handoff-eval の release が active になった
  最初の照合で `cancelled` になる（ADR D3.5）。
- まとめ: **release の昇格だけ**。config の変更・手での DB 書き換え・systemd の変更は無い（DB の変更は migration `0078` の自動適用だけ）。
- 昇格の時に評価試行が走っていても、新 instance は旧 instance の試行を error にしない（旧 instance が drain の間に終端まで走らせ、
  新 instance が採点する。ADR D3）。昇格前に評価を止める必要は無い。
- 評価の再投入（08:52Z・08:53Z に error になった試行）は昇格の後に評価の運用セッションで行う（この task の範囲外）。
