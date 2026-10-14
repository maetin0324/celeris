# ADR 2026-10-14: 評価試行の起動は埋まった source を tick の中で飛ばす — 先頭詰まりを無くす

- 日付: 2026-10-14
- 状態: 採用・実装済み
- 関連: ADR 2026-10-11-llm-source-model-routing-evaluation 付記 2026-10-12（eval-dispatch）D4、
  ADR-0133 付記（lock count で tick の store 仕事を固定する）

## 1. 文脈

本評価（release 3325c40d6db1、`[routing_eval] reserved_runs = 6`）で、claude の account（各 2 本）が埋まると
codex の account（2 本）が空いていても codex の試行が起動されなかった。`tick_eval_trials` は先頭の `queued` を
`eval_trial_claim_next` で取り、`eval_admit` が account・quota で起こせない（deferred）と `eval_trial_unclaim` して
tick を終えていた。先頭が claude の試行だと、後ろの codex の試行はその tick で見られない（head-of-line blocking）。

## 2. 決定

### D1. deferred の試行の source を tick の中で飛ばし、次の試行を見る

- `tick_eval_trials` は account・quota で deferred になった試行を `queued` に戻し、その `source_id` を tick 内の
  `full_sources` に足して続ける。終わりは「飛ばさない source に `queued` が無い」か「`reserved_runs` に達した」。
- 単位を source にする理由: 評価の admission は source×model が決まった候補 1 件に対し、その source の account を
  `admit_source` で選ぶ。ある試行が deferred なら、同じ tick の中で同じ source の他の試行も起こせない（tick の中で
  run は終わらない）。source の中の作成順（batch・`created_at`）は保たれる。
- 同じ tick で同じ試行を 2 度 claim しない: deferred の試行の source は以後の claim から外れる。経路が宣言に無い
  試行は `error`、起動に失敗した試行は tick を終える（旧来どおり）。

### D2. `eval_trial_claim_next(skip_sources)` — store が埋まった source を除いて最古を返す

- `eval_trial_claim_next` は除外する source の配列を受ける（引数無しの形は消した）。SQL は
  `status = 'queued' AND source_id NOT IN (SELECT value FROM json_each(?1)) ORDER BY created_at, rowid LIMIT 1`。
  `idx_eval_trials_status` を順に辿り最初の 1 行で止まる。queued が数百件あっても、tick の claim は
  「起動した本数 + 埋まった source の数」回で、全件を読み出さない。
- 試験（`crates/task-dispatch/src/dispatcher/tests/eval_trial_dispatch.rs`）:
  `eval_trial_full_account_does_not_block_other_source_in_same_tick`（p1 ×3 が先頭・p1 の枠 1、後ろの p2 が同じ tick で起動）、
  `eval_trial_tick_claims_each_trial_at_most_once`（p1 だけ 3 件と 300 件で tick の lock count が同じ、deferred の試行は 1 度だけ戻る）。
