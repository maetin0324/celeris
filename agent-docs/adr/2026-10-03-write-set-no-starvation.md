# write-set 待ちの飢餓を起こさない（ADR-0130 D3 付記）

---
tasks: [01M41GTNM7BWHN9MY6P65QVSNQ]
---

- 日付: 2026-10-03
- 状態: 採用（実装済み）
- 関連: [ADR-0130](0130-write-set-parallelism-and-behind.md) D3、[ADR-0074](0074-parallel-work-units-checkpoints-milestones-quota.md) D1.3

## 背景

final review（criterion 5 (c)）で、ADR-0130 D3 の write-set gate に飢餓が二つ見つかった。

1. 兄弟 WU の走査（`dispatch_parallel_work_units`）が `runnable_in_phase` の先頭 1 件だけを見て、
   `dispatch_one` が `false` なら break していた。先頭の WU が write-set で待たされると、後ろの非重複 WU も
   走らない。`runnable_in_phase` は並列上限の空き数で候補を切るので、後ろの WU は候補にすら入らなかった。
2. 公平性の数え（`WriteSetWait`）は「直前の tick に見送られていなければ数え直す」規則で、容量
   （`max_concurrency`・CoS 枠）が満ちて走査が途中で切れた tick には照合されないので、tick ごとに戻っていた。
   さらに先頭へ移す公平性は ready task の並びにしか効かず、Ready の候補が並列 WU より先に照合されるので、
   後から来た重なる ready task が待っている WU を追い越し続けられた。

## 決定

- D1: 兄弟 WU の走査は候補を並列上限で切らずに並べ、この tick にまだ試していない最初の WU を試す。
  起こせなければ飛ばして次へ進む（同じ tick に同じ WU を二度試さない）。並列上限は in-flight の数で判定する。
- D2: 容量切れで走査が途中で切れた tick（`dispatch_ready` の冒頭で枠が無い、途中で枠が尽きた、
  非 CoS の枠が無く候補を飛ばした、並列 WU の走査が `max_concurrency` で切れた）の終わりに
  `carry_write_set_waits` を呼び、直前の tick まで続いていた待機を「待ち続けた」として引き継ぐ
  （連続回数を 1 つ進め、`last_tick` を今の tick にする）。容量に空きがあって全候補を照合した tick に
  見送られなかった待機だけが、従来どおり次 tick に捨てられる。
- D3: 待機は見送られた時点の予約を持つ。連続回数が `WRITE_SET_STARVATION_TICKS`（3）に届いた待機は
  先取りになり、自分より後に待ち始めた（`since_tick`、同じ tick なら待機を作った通し番号の順）重なる候補を
  `write_set_blocker` が止める。最古の待機を止めるのは走っている run だけなので、それが終われば次に容量が
  空いた tick に必ず走る（走っている run は有限時間で終わる前提）。
- D2 の引き継ぎは最大でも「容量に空きのある 1 tick」までなので、他の理由（backoff・クラスタ待ち）で
  止まった待機の先取りが残り続けることはない。

## 試験

`dispatcher/tests/write_set_gate.rs` の `write_set_gate_no_starvation`: 容量 3 が満ちた状態で、待たされた
WU の後ろの非重複 WU が同じ tick に走ること、容量切れの tick でも待機が数え直されないこと、毎 tick 後から
来る優先度の高い重なる ready task に追い越されず、先行の run が終わった最初の tick（tick 6）に待った WU が
走ることを、tick を試験が回し run の終了を run ごとの semaphore で起こして決定的に確かめる。
