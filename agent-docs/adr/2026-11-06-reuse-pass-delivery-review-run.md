---
tasks: [01M6RG9RT6PQAXV1CRN93FVCVT]
---
# reviewer の合格を再利用した review で、配送の行に検査済み SHA を書く

## 背景
本番の N1（01M6QW0GSG1AVARWC334AZCES1）が 2026-11-06 12:30:36Z から、
review_pass → done → rereview → reviewing → reviewer pass reused → review_pass を tick ごとに繰り返した
（13:18Z までに rereview 336 回）。LLM は起動しないので費用はないが、配送が進まず events が増え続ける。
deliveries の行は review_run = worker_run、target_sha・reviewed_sha・merge_candidate_sha が常に NULL だった。

## 原因（コードと試験で確認済み）
`crates/task-dispatch/src/dispatcher/review_spawn.rs` の `spawn_review` は、`task_ops::delivery::begin` に
`reviewer_run.map_or(&run_id, …)`（reviewer run が無ければ worker run の id）を渡す。
直後の行の照合は `old.review_run == reviewer_run.map_or("", …)`（無ければ空文字）だった。
reviewer の合格を再利用した回は `reviewer_run` が None なので、行の review_run は worker run id、
照合の右辺は空文字になり、常に偽になる。target_sha・reviewed_sha・merge_candidate_sha が書かれない。
その結果、`crates/celeris/src/delivery.rs` の MergeQueued で `check_candidate` が
「検査済みの merge candidate が記録されていません」の Stale を返し、`request_rereview` が再 review に戻す。

既存の試験が見逃した理由: 試験の 2 回目の review は worker run id が空文字で呼ばれ、空文字同士が偶然一致した。
本番は `last_run_id`（worker run の id）で呼ばれる。

## 上限が効かなかった理由（試験で確認済み）
review 入口の上限判定 `pre_review_stale_state`（`review_spawn.rs`）は、直後が `ReviewTargetAdvanced` でない `Transitioned` で
stale の数を 0 に戻していた。配送からの再 review の循環は
review_pass（reviewing → done）→ rereview → `ReviewTargetAdvanced` → reviewing → review_pass … で、
`request_rereview` は rereview の直後に `ReviewTargetAdvanced` を置くが、次の周の review_pass が数を 0 に戻す。
そのため数は 1 を超えず、`MAX_TARGET_RESYNCS = 2` に届かず `ReviewResyncHalted` が出なかった。
一方 `request_rereview` の attempt は `target_restale_count`（review_pass では戻さない）で数えるので、336 まで増えた。
`request_rereview` 自身は上限を見ない（D5 の設計どおり、上限は review 入口が持つ）。

## 決定
- `spawn_review` で `review_id`（reviewer run があればその id、無ければ worker run id）を 1 回だけ決め、
  `delivery::begin` への引数、行の照合、`ReviewTargetSynced.review_run` に同じ値を使う。空文字との比較は廃止する。
- 回帰試験（`crates/task-dispatch/src/dispatcher/tests/target_sync.rs`）:
  `reuse_pass_records_candidate`（3 つの SHA が行に書かれる）、
  `reuse_pass_delivery_reaches_merge`（行が `check_candidate` の Fresh 条件を満たす）。
  どちらも worker run id を非空にして本番と同じ呼び出しにしてある。
- 修正前のコードでは 2 件とも失敗することを確認した（行は review_run == worker_run、SHA は NULL）。

- 上限の修正（WorkUnit resync-cap）: `pre_review_stale_state` は reason が `review_pass` の `Transitioned` では数を戻さない。
  review_pass は合格の記録で、stale の連鎖を終えない。人の rereview・reopen・dispatch・review_fail などは従来どおり戻す。
  回帰試験（`target_sync.rs`）: `delivery_rereview_loop_halts_visibly_at_the_cap`
  （循環が MAX_TARGET_RESYNCS + 1 回で `ReviewResyncHalted` 1 件・通知・snapshot が出て review を起動しない。修正前は落ちる）、
  `delivery_rereview_below_the_cap_still_reviews`（上限までは従来どおり review が起動する）。

## 制約
本番の N1 の後始末は、この ADR の範囲外（進捗ファイルの「本番への移行」で扱う）。
