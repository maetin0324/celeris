---
title: 健全性v2 G3 checker-reap — health の時間切れの試験の後に stub の検査器を残さない、tests/mod.rs の doc comment を戻す
tasks: [01M6TYVJSWEY3ZE50S02ETYDNM]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 G3 checker-reap — health の時間切れの試験の後に stub の検査器を残さない、tests/mod.rs の doc comment を戻す

WorkUnit `checker-reap`（task 01M6TYVJSWEY3ZE50S02ETYDNM）。G3 の final review の差し戻し (2)(3) と、operator のコメント（2026-11-09。limits の試験の stub の検査器が試験の process の後に最長 60 分残った件）を直す。基準は `$CELERIS_WU_BASE`。

結論: 完了。本番のコード（`task_ops::changes::run`・`health_review::gate`）は変えていない。原因は試験の stub の眠りの長さ（1 時間）で、締め切りの kill そのものは本番の経路でも試験でも抜けていなかった。stub の眠りを 10 秒にし、時間切れの試験 2 本を足して「締め切りで検査器の process（孫を含む）が消える」ことを確かめるようにした。`tests/mod.rs` の doc comment は `mod health_review_input` の直上に戻した。

## 調べたこと（どの評価が締め切りの kill を受けずに残るか）
- 検査器（`repo_health.py`）の起動は、適用判定・G2 の材料（`health_review::input`）・review の項目（`review_item::evaluate_all`）・WU の check と段の統合（`check_item`）・棚卸し（`inventory`）のどれも `health_review::policy::BaseChecker::python` → `task_ops::changes::run_with_timeout` → `changes::run` を通る。締め切りは `gate::with_checker` が 1 か所で決める（試験では `test_timeout::cap` が縮める）。
- `changes::run` は子を `process_group(0)` で起こし、呼び出し側の thread が `recv_timeout(締め切りまでの残り)` で待ち、時間切れなら group へ SIGKILL を送ってから子を回収する。kill を送るのは評価を走らせている blocking thread で、評価を待つ future（`spawn_blocking` の `JoinHandle`）の drop・abort では止まらない。tokio の runtime は drop で走っている `spawn_blocking` の終わりを待つ（tokio 1.54 `BlockingPool::drop` → `shutdown(None)`）。dispatcher の検査器の起動に detached な `std::thread::spawn` は無い。
- 実測: 既存の試験 `health_limits_cap_checker_timeout_is_error_without_repair` を単独で走らせた後（2.4 秒で合格）、`pgrep -af "policy-version 1"` に stub は残っていなかった。
- よって、process の中のどの評価も締め切りで kill を受ける。残るのは、試験の process 自身が締め切りより先に終わる・殺される場合（Bash の道具の打ち切り、nextest の terminate、check の run の停止など）だけ。検査器は別の process group にいるので、試験の process group への signal が届かず、stub の眠り（1 時間）の終わりまで `systemd --user` の子として残る。operator が見た 2 本（`--out .../runs/health/*.json` = G2 の材料の評価の引数）はこの形と合う。
- 本番の daemon は `celeris@<sha12>.service` の `KillMode=control-group`（既定）で、daemon が止まると cgroup の残りの process も止まる（ADR-0040・ADR-0070 の記述）。本物の検査器は眠らずに終わる。本番の経路を変える理由は見つからなかった。

## 実装前 brief
1. 根本原因: limits の試験の stub の検査器（`health_limits_budget.rs` の `STUB`）が `HANG` で 1 時間眠る。検査器は締め切りの kill のために別の process group で起こすので、試験の process が締め切り（2 秒）より先に終わると、stub を殺すものが無く眠りの終わりまで残る。gate の試験の `"sleep": 30` と policy の試験の 120 秒の stub も同じ形で残り得る。さらに、締め切りで process が消えたことを確かめる試験が無かった（既存の時間切れの試験は Error の止まり方だけを見る）。
2. 責務 owner: 眠りの長さは試験の stub（`health_limits_budget.rs`・`gate_tests.rs`・`policy/tests.rs`）。締め切りの kill は `task_ops::changes::run`（変えない。上の調べで抜けが無い）。
3. 既存の再利用先: `changes::run` の group kill、`gate::test_timeout::cap_under`（作業ツリーの下だけ締め切りを縮める）、gate の試験の土台（`repo_with_checker`・`applied`・`run_health_check`・`stub.json` の `sleep`）、dispatcher の試験の土台（`Scenario`・`assert_stopped`・`health_evidence`）。pid の確かめは `gate_tests.rs` の `assert_checkers_reaped` の 1 か所に置き、dispatcher の試験も同じものを呼ぶ（`write_engine` と同じ共有の仕方）。
4. 選択肢: (a) stub の眠りを 10 秒に縮め、pid を書かせて締め切り + 1 秒の内に消えたことを確かめる（採用。依頼の指定どおりで、本番の経路を変えない）。(b) `changes::run` の子に `PR_SET_PDEATHSIG` を付け、親の死で子を殺す（signal の crate を task-ops に足す変更で、git・gh を含む全ての子の挙動が変わる。親の thread の終わりで発火する性質も要検討。範囲の外なので採らず「提案」に書く）。(c) 試験の終わりで締め切りまで待つだけ（process が殺された場合に効かない。(a) の確かめに含めた）。
5. 新しい状態・分岐・fallback・port: 無し。本番のコードは変えていない。試験の stub に「pid の file の path」（dispatcher の stub は `HANG` の中身、gate の stub は `stub.json` の `pids`）を足した。
6. 撤去対象: 1 時間・30 秒・120 秒の眠り（10 秒に置き換えた）。`HANG` の中身 `1`（pid の file の path に置き換えた）。消した試験は無い。
7. 不変条件: 検査器の時間切れの後、検査器の process（stub と、同じ group の孫）は締め切りで消える。呼び出し側が評価を待つのをやめても同じ。確かめる試験は下の「試験」。

## 変更
- `crates/task-dispatch/src/dispatcher/tests/health_limits_budget.rs`
  - `STUB`: `HANG` の中身（pid の file の path）に `<pid> <起動の UNIX 秒>` を追記し、10 秒眠る。眠りを終えたら `<pid> woke` を追記する。module の doc を合わせた。
  - `HANG_CAP`（2 秒）と `hang(&Scenario)`（試験の一時 dir の pid の file の path を `HANG` に commit する）を足した。既存の `health_limits_cap_checker_timeout_is_error_without_repair` は `hang` を使う（assert は変えていない）。
  - 試験 `health_limits_cap_checker_timeout_leaves_no_checker_process` を足した。
- `crates/task-dispatch/src/health_review/gate_tests.rs`
  - `STUB`: `stub.json` に `pids` があれば、同じ group の孫（`sys.executable -I -c …`。`sleep` 秒眠って `<pid> woke` を書く）を起こし、自分と孫の `<pid> <起動の UNIX 秒>` を書く。眠りを終えたら `<pid> woke` を書く。
  - `health_evidence_gate_missing_or_broken_report_is_error` の `checker_not_run:` の stub の `"sleep": 30` を 10 にした（締め切りは 3 秒のまま）。
  - `assert_checkers_reaped(file, deadline)`（pid が最も早い起動から `deadline` + 1 秒の内にすべて消え、`woke` の印が無いことを確かめる）と、試験 `health_evidence_gate_timeout_kills_group_after_caller_stops_waiting` を足した。module の doc に眠りを短くした理由を書いた。
- `crates/task-dispatch/src/health_review/policy/tests.rs`: `health_review_policy_timeout_is_unavailable` の stub の眠りを 120 秒から 10 秒にし、戻りの時間の上限を 60 秒から 8 秒にした（kill が抜ければ戻りは 10 秒まで遅れるので見分けられる）。
- `crates/task-dispatch/src/dispatcher/tests/mod.rs`: `mod health_inventory` の上にずれていた `health_review_input` の doc comment（3 行）を `mod health_review_input` の直上に戻した。
- `crates/task-dispatch/src/health_review/check_item.rs` の `repair_count` の doc comment: 基準の時点で既に「段の統合と task の review の修復の bucket（…）」になっていた（closeout までのどこかで直っていた）。変えていない。

## 待ちの上限と理由
- 上限は検査器の起動（stub が書いた最も早い起動の時刻）から「締め切り + 1 秒」。締め切り 2 秒の試験では 3 秒。
- 締め切りは `with_checker` の呼び出しの時点から数えるので、検査器の起動はそれより後。締め切りの kill は「起動 + 2 秒」より前に起きる。1 秒の余裕は、kill の process（`kill -KILL -- -<pgid>`）の起動、子の回収、待ちの刻み（50ms）の分。
- stub は 10 秒眠るので、上限（起動 + 3 秒）は眠りの終わりより 7 秒早い。締め切りの kill が抜けて呼び出し側が待たない場合、上限の時点で stub はまだ眠っていて試験が落ちる。呼び出し側が子の終わりまで待つ場合（`changes::run` は kill の後に子の回収を待つので、kill が抜けると評価の戻りが 10 秒まで遅れる）は、process が消えた後に `<pid> woke` の印が残るので落ちる。
- 「消えた」は `/proc/<pid>` が無いか、`/proc/<pid>/stat` の state が `Z`（殺された後で親の回収を待つだけ）。
- 待ちは 50ms の `tokio::time::sleep` の繰り返しで、CPU を焼く待ちは使わない。時計は差し替えない（子 process の実時間の締め切りを見るため）。

## 試験
| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `health_limits_cap_checker_timeout_leaves_no_checker_process`（dispatcher の review の流れ） | review の時間切れの後、検査器の process が締め切り + 1 秒の内に消え、眠りを終えていない | `health_limits_cap_checker_timeout_is_error_without_repair` は Error の止まり方だけを見る。kill が抜けても評価は子の終わりを待ってから時間切れの Error を返すので合格のまま（下の変異で確認） |
| `health_evidence_gate_timeout_kills_group_after_caller_stops_waiting`（gate） | 評価を待つ future を abort しても、締め切りで検査器の process group（孫を含む）が殺される | 既存の時間切れの試験はどれも評価の戻りを待ち、孫を起こさない。group でなく子だけを殺す変異も検出できない（下の変異で確認） |
| `health_review_policy_timeout_is_unavailable`（変えた） | `--print-review-policy` の時間切れで script を殺す | 眠り 120 秒・戻りの上限 60 秒では、kill が抜けても 60 秒より前に戻らないので落ちたが、眠りを 10 秒にすると上限 60 秒では抜けを見分けられない。上限を 8 秒にした |

変異で確かめたこと（`crates/task-ops/src/changes.rs` を一時的に書き換えて試験を走らせ、元に戻した。差分は残していない）:
- 時間切れの kill（group と子の両方）を外す: `health_limits_cap_checker_timeout_leaves_no_checker_process`（`a checker process slept to the end instead of being killed`）、`health_evidence_gate_timeout_kills_group_after_caller_stops_waiting`（`checker processes [stub, 孫] outlived the deadline (2s) + 1s`）、`health_review_policy_timeout_is_unavailable`（10.03 秒）が落ちた。既存の `health_limits_cap_checker_timeout_is_error_without_repair` と `health_evidence_gate_missing_or_broken_report_is_error` は合格のままだった。
- group への kill だけを外す（子の pid への kill は残す）: `health_evidence_gate_timeout_kills_group_after_caller_stops_waiting` だけが落ちた（孫の pid が上限の時点で生きていた）。

## 検査の結果（提出時の HEAD で実行）
- `cargo nextest run -p task-dispatch --no-tests=fail -E 'test(/^(.*::)?(health_limits_|health_evidence_gate_|health_review_policy_timeout)/)'` → 合格（下の「提出時の検査」）。直後に `pgrep -af "repo_health.py|time.sleep"` で残りの stub が無いことを確かめた。
- 提出時の検査は下の節に書く。

## 未解決
- `crates/task-dispatch/src/health_review/entry_tests.rs` の `health_entry_*` の時間切れの場合も、gate の試験と同じ stub に `"sleep": 30`（締め切り 3 秒）を渡している。この WU の依頼に挙がっていない file なので変えていない。試験の process が締め切りより先に死ぬと、最長 30 秒残り得る。

## 提案
- `entry_tests.rs` の `"sleep": 30` も 10 にする（次に health_review の試験を触る unit で）。
- 試験の process が死んでも別の process group の子を残さないには、`task_ops::changes::run` の子に `PR_SET_PDEATHSIG` を付ける案がある。git・gh を含む全ての子の挙動が変わり、signal の crate を task-ops に足すので、設計の記録を書いてから判断する。本番の daemon は systemd の `KillMode=control-group` で止まるので、今は本番の問題ではない。
