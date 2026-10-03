---
task: review-sync-fix
wu: starve-fix
status: done
completed: 2026-10-03
---
# starve-fix: write-set 待ちの飢餓を直す

## 原因
- `dispatcher/work_units.rs` の `dispatch_parallel_work_units` が `runnable_in_phase(&units, in_flight, limit)` の先頭だけを見て、
  `dispatch_one` が `false`（write-set で待たされた等）なら break していた。`runnable_in_phase` は空き数で候補を切るので、
  後ろの非重複 WU は候補にもならなかった。
- `write_set_gate.rs` の待機（`WriteSetWait`）は「直前の tick に見送られていなければ数え直す」。容量が満ちた tick は
  照合されないので数えが戻り、`order_write_set_starved_first` の公平性が効かなかった。並列 WU は Ready の候補の後に
  照合されるので、後から来た重なる ready task に追い越され続けた。

## 修正（設計は `agent-docs/adr/2026-10-03-write-set-no-starvation.md`）
- 兄弟 WU の走査: 候補を全部並べ、この tick に未試行の最初の WU を試す。起こせなければ飛ばして次へ（同じ WU を二度試さない）。
  並列上限は in-flight の数で判定。戻り値を `(dispatched, capacity_cut)` にした。
- `carry_write_set_waits`: 容量切れで走査が切れた tick の終わりに、直前の tick から続く待機を引き継ぐ（連続回数 +1）。
- 先取り: 連続 3 回以上待った待機は見送り時の予約を持ち、後から待ち始めた重なる候補を `write_set_blocker` が止める。
  同じ tick の待機の先後は通し番号（`write_set_hold_seq`）で決める。

## 試験
- 追加: `dispatcher::tests::write_set_gate::write_set_gate_no_starvation`（tick を手で回し、run の終了は run ごとの semaphore。
  sleep・負荷なし）。tick 1 で待たされた WU の後ろの非重複 WU が同じ tick に走ること、tick 2〜5（容量満ち、毎 tick 重なる
  優先度 9 の ready task が増える）で待機の `since_tick` が保たれ連続回数が 2..5 と積み上がること、先行 run の終了後の
  tick 6 で待った WU が走り、後発の ready task は 1 件も走らず attempts・遷移もないことを assert。
- 修正前のコードに対しては「the disjoint sibling behind the held WU runs in the same tick」で落ちることを確かめた。

## 証拠
- `cargo test -p task-dispatch write_set_gate` → 7 passed（既存 6 + 新規 1）
- `cargo test -p task-dispatch` → exit 0、596 passed / 0 failed（stale_priority・work_units 試験を含む）＋ 4 passed
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `cargo test --workspace` → exit 0、3772 passed / 0 failed / 13 ignored

## 未解決事項
- Ready の task の 1 本目（`dispatch_ready` → `dispatch_one(task, None)`）は計画の先頭の WU しか試さないので、先頭の WU が
  write-set で待たされると、その task の後ろの非重複 WU は task が Running になるまで走らない。今回の範囲（兄弟 WU の走査）外。

## 提案
- 上の未解決事項: Ready の task の 1 本目でも、先頭の WU が write-set で待たされたら同じ工程の次の runnable WU を試す。
