---
title: "健全性v2 R4 cex-late: replan・段の repair の後の遅れた完了と quota group の続けての終わりの反例試験"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-15
---
# 健全性v2 R4 cex-late: replan・段の repair の後の遅れた完了と quota group の続けての終わりの反例試験

完了日: 2026-11-15

## 概要

- 目的: ADR 2026-11-11-dispatcher-state-ownership D7 の表で cex-review に割り当てたのに試験が無かった反例 2 つを、
  `crates/task-dispatch/src/dispatcher/tests/cex_late_completion.rs` に試験として固定する。本体のコードは変えない。
- 基準は `$CELERIS_WU_BASE`。変更は `cex_late_completion.rs`（新規）と `tests/mod.rs` の `mod cex_late_completion;` の宣言だけ。
- 状態: 完了。attempt 2 で区切った残りは task 01M7DYMCVS1PMDK2N0WW3ME9T8 で終えた（「残り」の節）。

## 試験の規約

`cex_review_authority.rs` と同じ。Dispatcher の private field に触らず、store の行・event・`tick()` の戻り値だけを見る。
待ちは `run_until_state`（出来事待ち・壁時計の保険）で、tick の回数で待たない。run の終わりは fake adapter の gate
（semaphore）で試験が決め、統合の検査の止め所は試験が置く file で決める。CPU 負荷は使わない。

## 試験（書いた分）

| 試験 | 守る不変条件（要約） | 結果 |
|---|---|---|
| `cex_quota_group_two_consecutive_ends_are_both_counted` | 同じ account の run 2 本（同じ quota group）の終わりが続けて届くと、2 本とも runs 索引の usage と `QuotaEstimated`（按分）に入る。木の USD 上限の判定は 2 本目の後の値（1.2 USD ≥ 1 USD）で決まり、1 本目だけ（0.6 USD）では止めない | 合格 |
| `cex_quota_group_two_consecutive_ends_unknown_cost_stops` | 2 本目が usage を返さない（費用不明）とき、1 本目の既知の費用が上限の内側でも 2 本目の後に木を止め、決定要求に費用不明 1 件を書く | 合格 |
| `cex_late_review_and_integration_after_replan_are_dropped`（replan × 統合の変種） | 段 `build` の統合の検査が走っている間に人の replan（WU `c` を足す）が確定したら、旧い統合（`a`・`b` だけ）の完了は `PhaseIntegrated` を付けず、統合 WU の done と最終 review は `c` の done の後。Task は `c` を含む候補で Done | 合格 |

観測: 旧い統合の完了は `c` を含まない `PhaseIntegrated` を付けなかった。この節の最初の版は「人の replan は統合 WU を running の
まま持ち越す」と書いたが、誤りだった。`task_ops::execution::plan_replan` は running の統合 WU を pending に戻し、
`on_integration_finished` が running でない統合 WU の結果を捨てる。この組が旧い完了を止めた（[cex-late2.md](cex-late2.md) の
「replan × 統合で旧い完了を止めた guard」）。

## 残り

残り 1〜4（段の repair の後の旧い統合、旧い最終 review の合格の 2 変種、guard の確認、検査）は task
01M7DYMCVS1PMDK2N0WW3ME9T8 で終えた。結果は [cex-late2.md](cex-late2.md)（6 本の試験、guard の一時変更の総表、本体の修正、証拠）。

## 変更（commit の件名）

- `phase cex-late: quota group の続けての終わりと replan 後の旧い統合の完了の反例試験を足す`

## 証拠（途中。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-dispatch -E 'test(/cex_late_completion::/)'`: 3 本すべて合格、exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: `cex_late_completion.rs` と `tests/mod.rs` だけ。
