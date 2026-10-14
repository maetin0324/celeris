---
title: "評価試行の先頭詰まり（埋まった account の後ろの別 account の試行を同じ tick で起こす）"
tasks: [01M4WQX3818BYE52MW0W2B1J27]
status: done
updated: 2026-10-14
completed: 2026-10-14
---
# 評価試行の先頭詰まりの修正

設計判断は [ADR 2026-10-14-eval-trial-skip-full-source](../adr/2026-10-14-eval-trial-skip-full-source.md)（D1・D2）。

## 変更

- `crates/task-dispatch/src/dispatcher/eval_trials.rs` `tick_eval_trials`: deferred（account 満杯・quota）の試行は
  `queued` に戻し、その source を tick 内で飛ばして続ける。`Disabled`・`ReservedFull` は従来どおり tick を終える。
- `crates/task-core/src/store/eval_trials.rs` `eval_trial_claim_next(skip_sources: &[String])`（`TaskStore` も同じ）。
  旧い引数無しの形は消した。呼び出し元（`crates/task-api/tests/routing_eval.rs`）は `&[]`。
- 試験 2 件（`dispatcher::tests::eval_trial_dispatch::`）:
  - `eval_trial_full_account_does_not_block_other_source_in_same_tick`: p1 ×3 → p2 ×1 の順、各 source の account は 1 本。
    1 tick で 2 本（p1 の先頭と p2）起動し、p1 の残りは `queued`。修正前の挙動（deferred で return）に戻すと
    `left: 1, right: 2` で落ちることを確かめた。
  - `eval_trial_tick_claims_each_trial_at_most_once`: p1 だけ 3 件と 300 件で、tick の `lock_counts` の差分が同じ。
    deferred の試行は 1 度だけ `queued` に戻る（2 度目の unclaim は失敗）。

## 証拠

- `cargo test -p task-dispatch --lib eval_trial` → `test result: ok. 13 passed`
- `cargo test -p task-core --lib store::eval_trials` → `6 passed`（`claim_skips_the_given_sources_and_keeps_their_order` を追加）
- `bash scripts/dev/test-parallel.sh` → exit 0、`5094 tests run: 5094 passed, 12 skipped`、doctest exit 0
  （`tmp_leftovers: 1` の警告。終了時に消える）
- `cargo clippy --workspace -- -D warnings` → exit 0

## 未解決事項

- なし。account が複数 source に共有される構成では、source 単位の飛ばしは「その source が今は起こせない」ことだけを
  使うので正しい（共有 account の空きは別 source の試行の admission が自分で見る）。

## 本番への移行

本番 config・DB の書き換えは無い（migration 無し）。この変更を含む release を運用セッションが昇格すると、
次の tick から claude の枠が埋まっていても codex の試行が起動する。
