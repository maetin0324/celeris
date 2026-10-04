---
task: review-sync-fix
wu: ab-phase5
status: done
completed: 2026-10-03
---
# ab-phase5: Phase 5（write-set gate・stale 優先同期）の決定的 A/B

starve-fix（dd44dd28）の修正の上で測った。偽アダプタと注入した試験時計（SimClock）だけを使い、実 claude・外部ネットワーク・
CPU 負荷・実時間の sleep による判定はない（stale_priority と review_sync の tick 回しの 20ms 待ちは出来事待ちのための poll で、値には効かない）。

## 追加したもの
- `crates/task-dispatch/src/dispatcher/tests/phase_effect_ab/write_set.rs` — scenario `write_set`（ADR-0130 D3）。
- `crates/task-dispatch/src/dispatcher/tests/phase_effect_ab/stale_priority.rs` — scenario `stale_priority`（ADR-0130 D5）。
- `review_sync.rs` に scenario `review_sync_phase2`（ADR-0120 の off/on）。既存の `review_sync` の行と assert は変えていない。
- `phase_effect_ab.rs`: submodule 2 つの `mod` 宣言と `ab_metric_line_with` / `print_ab_metric_with`（既存の行の末尾に
  `key=value` を足す。`ab_metric_line` の形は不変で `phase_effect_ab_metric_line_format` は通る）。
- dispatcher の試験だけの切替（`#[cfg(test)]`、本番の挙動は不変）:
  `test_disable_write_set_gate`（`dispatch_run.rs` で予約を作らない）、`test_disable_stale_priority`（`stale_priority.rs` で
  待ち行列を並べ替えない）、`test_sync_conflict_as_review_fail`（`review_spawn.rs` で同期衝突を IntegrationRepair ではなく
  `Trigger::ReviewFail` で返す）。

## コマンド
```
cargo test -p task-dispatch phase_effect_ab -- --nocapture   # exit 0、8 passed（3 回走らせて同じ値）
cargo fmt --all -- --check                                    # exit 0
cargo clippy --workspace -- -D warnings                       # exit 0（--all-targets でも exit 0）
cargo test --workspace                                        # exit 0、3776 passed / 0 failed / 13 ignored
```

## 出力行（Phase 5 の分）
```
ab-metric write_set off runs=3 wall_secs=1320 input_tokens=120000 fresh_sessions=3 conflicts=1 repairs=1
ab-metric write_set on runs=2 wall_secs=1260 input_tokens=80000 fresh_sessions=2 conflicts=0 repairs=0
ab-metric stale_priority off runs=2 wall_secs=180 input_tokens=80000 fresh_sessions=2 stale_wait_secs=90
ab-metric stale_priority on runs=2 wall_secs=180 input_tokens=80000 fresh_sessions=2 stale_wait_secs=0
ab-metric review_sync_phase2 off runs=3 wall_secs=330 input_tokens=120000 fresh_sessions=3 attempts=1
ab-metric review_sync_phase2 on runs=3 wall_secs=330 input_tokens=120000 fresh_sessions=3 attempts=0
```
既存の行（同じ実行で不変）: `review_sync off runs=4 wall_secs=420 …` / `on runs=3 wall_secs=330 …`、`atomic_route`、`continuation`。

## 解釈
- **write_set**: 同じ作業ディレクトリで `README.md` を書く 2 task（expected write-set はどちらも `README.md`）。off は両方が
  tick 1 に起き、同じ版を読んで編集する。A（600 秒）が先に書き、B（660 秒）は読んだ版と食い違うので衝突（conflicts=1）、
  受け入れ検査に落ちてやり直しの run（repairs=1）が足される。on は B が A の終わりまで ready のまま待つ（attempts・遷移なし）ので
  衝突も repair もない。run 3→2、入力 token 120k→80k、壁時計 1320→1260 秒。待たせた分（B の 600 秒の待ち）より
  やり直しの run（660 秒）の方が高くつく、という比較になっている。
- **stale_priority**: reviewer の枠 1 つに、到着順で先の fresh（behind 0）と後の stale（2 commits 遅れ、10 分前から観測）。
  off は到着順で fresh が先に同期・review され、stale は reviewer run 1 本ぶん（90 秒）待つ。on は stale を先に同期する
  （`ReviewTargetSynced` の順を assert）ので待ち 0。run 数・合計時間は同じで、効くのは長く遅れた task の待ちだけ。
- **review_sync_phase2**（Phase 2 の off を再現できたので足した）: 同期の衝突を `ReviewFail` で worker に戻す旧経路は B の
  attempts を 1 使う（`max_retries = 0` の task ならそこで failed）。IntegrationRepair（on）は attempts を使わない。偽アダプタでは
  worker のやり直しと repair WU が同じ rebase をするので run 数・時間は同じ。違いは attempts（と failed になる危険）だけ。
- 各 scenario は on が off より良いことを assert している（write_set: conflicts・repairs・runs・wall、stale_priority:
  stale_wait_secs、review_sync_phase2: attempts）。

## 未解決事項
- **git worktree の task では write-set gate は統合衝突を減らさない**。待たされた run も起動時の target から切られる
  （atomic task は main、WU は `prepare_work_unit_workspace` の Task ブランチ HEAD）。先行 run の変更は配送・工程の統合まで
  その base に入らず、gate の予約は先行 run の終わりで外れて同じ tick に後続が起きるので、後続は先行の変更を見ないまま同じ
  file を書く。このため `write_set` scenario は worktree を切らない共有の作業ディレクトリ（同時編集の衝突）で測った。
  worktree の統合衝突は review 前同期（ADR-0118）と IntegrationRepair（ADR-0120）が受け持つ（`review_sync` の行）。
- `review_sync_phase2` の off は旧経路そのもの（削除済みのコード）ではなく、衝突の分岐で `Trigger::ReviewFail` を当てる試験用の
  再現。worker への衝突の伝え方（feedback 文面）は再現していない。

## 提案
- write-set gate を worktree の統合衝突にも効かせるなら、(a) 同じ task の WU は待たされた相手の WU ブランチ（終わった時点の
  head）を base にする、または (b) 予約を run の終わりではなく工程の統合・配送まで保つ、のどちらかが要る。どちらも
  ADR-0130 D3 の変更なので、決めてから別の WorkUnit で行う。
